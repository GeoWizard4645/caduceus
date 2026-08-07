//! The `#[tauri::command]`s a UI drives this module through: CRUD, plus
//! `pause`/`resume`/`run_now`.
//!
//! Every command here is a thin adapter — resolve the [`SchedulerRuntime`]
//! (lazily, via [`ensure_managed`]), call straight into [`JobStore`] or
//! [`dispatch`], and hand back a plain, already-`Serialize`-able [`Job`].
//! None of the actual logic (schedule parsing, at-most-once claiming, run
//! dispatch) lives in this file — see `job.rs`, `schedule.rs`, and `run.rs`
//! for that, and their own unit tests for coverage of it. That split is
//! deliberate: a `#[tauri::command]` needs a live `AppHandle`, which makes it
//! awkward to unit test directly, so the rule this whole module follows is
//! that nothing worth testing on its own lives only inside a command
//! function — mirrored from how `tools::habits`'s commands are thin wrappers
//! around pure, independently-tested functions.
//!
//! `scheduler_run_now` is the one command worth calling out: it exists
//! specifically so a job can be tested without waiting up to a minute for
//! the ticker's own sweep to reach it — see its own doc below.

use chrono::Local;
use tauri::{AppHandle, Manager, Runtime};

use super::executions::{Execution, ExecutionSource};
use super::job::{Deliver, Job};
use super::{dispatch, ensure_managed, SchedulerRuntime};

type Res<T> = Result<T, String>;

#[tauri::command]
pub fn scheduler_list_jobs<R: Runtime>(app: AppHandle<R>) -> Res<Vec<Job>> {
    ensure_managed(&app)?;
    app.state::<SchedulerRuntime>().store.list()
}

#[tauri::command]
pub fn scheduler_get_job<R: Runtime>(app: AppHandle<R>, id: String) -> Res<Job> {
    ensure_managed(&app)?;
    app.state::<SchedulerRuntime>()
        .store
        .get(&id)?
        .ok_or_else(|| format!("No such job: {id}"))
}

/// Create a job. `schedule` is the raw string a person typed (or a UI
/// assembled) — `"every 30m"`, `"0 9 * * 1-5"`, `"2026-08-10T14:00"`, `"2h"`
/// — parsed and validated by [`super::schedule::parse`]; see that module's
/// doc for the three accepted forms.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn scheduler_create_job<R: Runtime>(
    app: AppHandle<R>,
    name: Option<String>,
    prompt: String,
    skills: Vec<String>,
    model: Option<String>,
    schedule: String,
    repeat_times: Option<u32>,
    deliver: Deliver,
    workdir: Option<String>,
    no_agent: bool,
) -> Res<Job> {
    ensure_managed(&app)?;
    let job = app.state::<SchedulerRuntime>().store.create(
        name, prompt, skills, model, &schedule, repeat_times, deliver, workdir, no_agent, Local::now(),
    )?;
    notify_changed(&app, &job.id);
    Ok(job)
}

/// Replace every editable field of an existing job — a full replace, not a
/// sparse patch, the same convention `mcp::mcp_update_server` already uses
/// in this codebase: the caller submits the whole form it is editing.
/// `id`, `created_at`, `state`, and the run-history fields are not
/// accepted here; they are either immutable or system-managed. See
/// [`super::job::JobStore::update`] for exactly what does and does not
/// change (in particular: a paused job stays paused).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn scheduler_update_job<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    name: Option<String>,
    prompt: String,
    skills: Vec<String>,
    model: Option<String>,
    schedule: String,
    repeat_times: Option<u32>,
    deliver: Deliver,
    workdir: Option<String>,
    no_agent: bool,
) -> Res<Job> {
    ensure_managed(&app)?;
    let job = app.state::<SchedulerRuntime>().store.update(
        &id, name, prompt, skills, model, &schedule, repeat_times, deliver, workdir, no_agent, Local::now(),
    )?;
    notify_changed(&app, &job.id);
    Ok(job)
}

#[tauri::command]
pub fn scheduler_delete_job<R: Runtime>(app: AppHandle<R>, id: String) -> Res<bool> {
    ensure_managed(&app)?;
    let deleted = app.state::<SchedulerRuntime>().store.delete(&id)?;
    notify_changed(&app, &id);
    Ok(deleted)
}

#[tauri::command]
pub fn scheduler_pause_job<R: Runtime>(app: AppHandle<R>, id: String, reason: Option<String>) -> Res<Job> {
    ensure_managed(&app)?;
    let job = app.state::<SchedulerRuntime>().store.pause(&id, reason, Local::now())?;
    notify_changed(&app, &job.id);
    Ok(job)
}

#[tauri::command]
pub fn scheduler_resume_job<R: Runtime>(app: AppHandle<R>, id: String) -> Res<Job> {
    ensure_managed(&app)?;
    let job = app.state::<SchedulerRuntime>().store.resume(&id, Local::now())?;
    notify_changed(&app, &job.id);
    Ok(job)
}

/// Fire `id` immediately rather than waiting for its schedule (or the
/// ticker's up-to-a-minute latency) — the "trigger now" button a job needs
/// to be testable at all. Goes through exactly the same claim-then-dispatch
/// path a naturally due job does ([`super::job::JobStore::claim_for_run`] +
/// [`dispatch`]), so a manual run still durably advances a recurring job's
/// `next_run_at`, still counts against `repeat`, and still writes an
/// [`Execution`] row — tagged [`ExecutionSource::Manual`] rather than
/// [`ExecutionSource::Ticker`], which is the one thing that actually differs.
///
/// Returns as soon as the job is claimed and dispatched, with the job's
/// `state` already `Running` — it does not wait for the run to finish, the
/// same "returns immediately, progress follows as events" shape
/// `agent_start_tool_session` already uses. Poll `scheduler_get_job` (or
/// listen for [`super::SCHEDULER_CHANGED_EVENT`]) for the result.
#[tauri::command]
pub fn scheduler_run_now<R: Runtime>(app: AppHandle<R>, id: String) -> Res<Job> {
    ensure_managed(&app)?;
    let job = app.state::<SchedulerRuntime>().store.claim_for_run(&id, Local::now())?;
    dispatch(app.clone(), job.clone(), ExecutionSource::Manual);
    Ok(job)
}

/// Recent execution history — the audit ledger, not the job list. `job_id`
/// narrows to one job; omitted, every job's attempts interleave newest
/// first. See `executions.rs`'s module doc for why this is a read-only
/// audit trail and never a queue anything replays from.
#[tauri::command]
pub fn scheduler_list_executions<R: Runtime>(
    app: AppHandle<R>,
    job_id: Option<String>,
    limit: i64,
) -> Res<Vec<Execution>> {
    ensure_managed(&app)?;
    app.state::<SchedulerRuntime>()
        .executions
        .list(job_id.as_deref(), limit)
        .map_err(|e| e.to_string())
}

fn notify_changed<R: Runtime>(app: &AppHandle<R>, job_id: &str) {
    use tauri::Emitter;
    let _ = app.emit(super::SCHEDULER_CHANGED_EVENT, job_id);
}
