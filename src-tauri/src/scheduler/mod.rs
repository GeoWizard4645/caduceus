//! Scheduled agent jobs: run a prompt (or a plain script) on a timer, in a
//! fresh session that starts with no memory of ever having run before.
//!
//! ```text
//!  ticker (every 60s)                          commands.rs
//!  ──────────────────                          ───────────
//!   JobStore::claim_due(now) ──due jobs──┐       CRUD, pause, resume
//!     (advances next_run_at              │       ────────────────────┐
//!      BEFORE returning — the             │                          │
//!      at-most-once guarantee)            ▼                          ▼
//!                                      dispatch() ◀── run_now ── JobStore
//!                                         │
//!                                         ▼
//!                                    run::execute
//!                                    ┌────┴────┐
//!                              no_agent      agent-mode
//!                              (script)      (fresh session,
//!                                             no history,
//!                                             deny-by-default
//!                                             approval)
//!                                         │
//!                                         ▼
//!                              ExecutionLedger (audit trail)
//!                              JobStore::record_result (bookkeeping)
//! ```
//!
//! # Why this exists, and why it is not `tools::cron`
//!
//! [`crate::tools::cron`] parses a 5-field cron expression, describes it in
//! English, and lists its next few occurrences — for a settings page where
//! someone is *checking* what an expression means. It has never run
//! anything and does not know what a job is; nothing here edits that file,
//! and this module is named `scheduler`, not `cron`, specifically so the two
//! are never confused for one another. What this module reuses from it is
//! narrow and explicit: [`schedule::Schedule::Cron`] calls
//! [`crate::tools::cron::parse`] / [`crate::tools::cron::next_occurrences`]
//! to validate an expression and walk it forward, because that walk is
//! already correct across month/leap-year boundaries and there is no reason
//! to write a second one.
//!
//! # Reference implementation
//!
//! This tracks Hermes Agent's `cron/` package (MIT-licensed, vendored
//! locally at `~/.hermes/hermes-agent/cron/` on this machine) as the
//! authoritative design for what a scheduled job needs: the three schedule
//! formats, the job fields, the atomic-write/lock-file persistence, the
//! separate audit ledger, and — most load-bearingly — the at-most-once
//! execution ordering and the deny-by-default approval default. Caduceus is
//! a single-user, single-instance desktop app rather than a multi-profile
//! gateway daemon that a separate CLI can talk to concurrently, so a fair
//! amount of Hermes' own complexity (multi-machine `fire_claim` arbitration,
//! per-row PID-liveness proofs, parallel/sequential worker pools keyed by a
//! process-global `TERMINAL_CWD`) has no equivalent need here and is
//! deliberately not carried over — each such simplification is called out
//! at its own site rather than silently dropped.
//!
//! # Job schema
//!
//! See [`job::Job`] for the full field list and [`job::JobStore`] for how it
//! is persisted (atomic write, `0600` permissions, an advisory lock file —
//! see that module's doc) and mutated (create/update/delete/pause/resume,
//! plus the at-most-once claim/record-result pair the ticker and `run_now`
//! both funnel through).
//!
//! # Execution semantics — the properties that actually matter
//!
//! - **At-most-once.** [`job::JobStore::claim_due`] marks every due job
//!   `Running` and durably advances its `next_run_at` *before* a single one
//!   of them has actually run — see that function's doc. A crash between the
//!   claim and the run finishing therefore costs at most one missed firing,
//!   never a re-fire loop. This is the single most important correctness
//!   property in this module; do not "optimize" it into run-then-advance.
//! - **No message history, ever.** Every firing is a brand-new agent session
//!   with a synthetic id (`cron_<job_id>_<timestamp>`) and exactly one
//!   [`crate::agent::Message::user`] turn. See `run.rs`'s module doc — this
//!   is called out there at length because it is the single most common way
//!   people misuse a scheduled job, and the fix (skills, or a self-contained
//!   prompt) is simple once the constraint is understood.
//! - **Approval denies by default.** No human is present to answer an
//!   approval prompt at 3am, so every tool call a cron-triggered session
//!   attempts is refused unconditionally — see `run.rs`'s `DenyApproval` and
//!   its module doc. This is Hermes' own `approvals.cron_mode: deny` default,
//!   carried over deliberately and not weakened into something
//!   configurable.
//! - **`no_agent` skips the LLM entirely.** [`job::Job::no_agent`] runs
//!   `prompt` as a plain `/bin/sh -c` command with no agent loop, no tools,
//!   and none of the above framing — see `run.rs`.
//! - **The execution ledger is an audit trail, not a retry queue.** See
//!   [`executions`]'s module doc. Nothing here ever reads it to decide what
//!   to run next.
//!
//! # Commands
//!
//! See [`commands`]: CRUD (`scheduler_list_jobs`, `scheduler_get_job`,
//! `scheduler_create_job`, `scheduler_update_job`, `scheduler_delete_job`),
//! `scheduler_pause_job` / `scheduler_resume_job`, `scheduler_run_now` (fire
//! a job immediately rather than waiting on its schedule — essential for
//! testing one without waiting), and `scheduler_list_executions` (read the
//! audit ledger). Every command returns a plain, already-serializable
//! [`job::Job`] (or [`executions::Execution`]) for a UI to render directly.
//!
//! # Skills integration
//!
//! [`job::Job::skills`] names are loaded through the real
//! `crate::skills` system ([`crate::skills::tiers::view_skill`]) — see
//! `run.rs::load_named_skills`'s doc. The reverse integration point that
//! module's own doc explicitly leaves open — `skills::lifecycle
//! ::apply_transitions`'s `protected: &HashSet<String>` parameter, for "do
//! not archive a skill a scheduled job still refers to" — is
//! [`referenced_skill_names`] below, called from `lib.rs::setup`'s
//! once-per-launch curator sweep exactly the way this doc originally asked
//! whoever added that sweep to call it.
//!
//! # Honest gaps
//!
//! - **`workdir` only does something for `no_agent` jobs.** An agent-mode
//!   job accepts and stores the field, but nothing routes it into the MCP
//!   tool layer (which has no per-call working-directory concept today) —
//!   see [`job::Job::workdir`]'s doc.
//! - **No live "stop" button for an in-flight cron run.** A cron session is
//!   not registered with [`crate::agent::AgentRuntime`] — its
//!   registration API is private to the `agent` module by design, and
//!   rightly so; this module does not need (and was not asked) to change
//!   that. A stuck run still ends via [`crate::agent::MAX_ITERATIONS`] or a
//!   backend timeout.
//! - **`Deliver` has exactly two variants** (`None`, a macOS notification).
//!   Richer delivery (appending to Chat history, a webhook) is a natural
//!   follow-up but out of scope here — see [`job::Deliver`].

pub mod commands;
pub mod executions;
pub mod job;
pub mod run;
pub mod schedule;
pub mod ticker;

use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use executions::{ExecutionLedger, ExecutionSource};
use job::{Job, JobStore};

/// Emitted (carrying just the affected job's id as a `&str`) after every
/// mutation this module makes — create/update/delete/pause/resume/run_now,
/// and again whenever a dispatched run finishes. Deliberately payload-light
/// (a "something changed, re-fetch" signal rather than the full [`Job`]) so
/// there is no risk of the event's shape drifting out of sync with the
/// command responses that already carry the authoritative data — a UI
/// listens for this and calls `scheduler_list_jobs` / `scheduler_get_job`.
pub const SCHEDULER_CHANGED_EVENT: &str = "caduceus://scheduler-changed";

/// Everything a running job needs beyond its own prompt: where jobs are
/// persisted, the audit ledger, and the in-process dedup guard that keeps
/// the ticker from double-firing a job whose previous run is still going.
///
/// Lazily managed — see [`ensure_managed`] — the same pattern
/// `mcp::McpRuntime` and `widgets::WidgetRuntime` already use in this
/// codebase specifically so a new subsystem needs no changes to
/// `lib.rs::setup()`'s body to exist. (This module *does* still add one line
/// to `setup()`, to start [`ticker::spawn`] — see that function's doc for
/// why a scheduler, unlike those two, cannot be purely lazy: jobs need to
/// fire whether or not anyone ever opens a settings page.)
///
/// `Clone` because [`dispatch`] needs to move its own copy of the store/
/// ledger handles onto a spawned task — cheap, since [`JobStore`] is just a
/// directory path and [`ExecutionLedger`] is an `Arc`-wrapped connection;
/// see both types' own docs.
#[derive(Clone)]
pub struct SchedulerRuntime {
    store: JobStore,
    executions: ExecutionLedger,
    /// Job ids with a firing currently in flight, guarding against the
    /// ticker and a manual `run_now` (or two overlapping ticks, if a run
    /// somehow outlives a minute) dispatching the same job twice at once.
    /// Purely in-process and never persisted — unlike `next_run_at`
    /// advancement, this is not the at-most-once guarantee itself (that is
    /// durable, on disk); it is just a cheap belt-and-suspenders against a
    /// same-process double-dispatch race.
    running: Arc<Mutex<HashSet<String>>>,
}

impl SchedulerRuntime {
    /// Claim `id` for execution. Returns `false` (meaning: do not dispatch)
    /// if a firing of the same job is already in flight.
    fn begin(&self, id: &str) -> bool {
        self.running.lock().insert(id.to_string())
    }

    fn finish(&self, id: &str) {
        self.running.lock().remove(id);
    }
}

/// Ensure [`SchedulerRuntime`] is registered as Tauri-managed state,
/// building it (and its directory, and its SQLite connection) on first use.
/// Every command in `commands.rs` and [`ticker::spawn`]'s per-tick body
/// calls this first — it is cheap once already managed (a single
/// `try_state` check).
fn ensure_managed<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if app.try_state::<SchedulerRuntime>().is_some() {
        return Ok(());
    }

    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("could not find the app data directory: {e}"))?;
    let dir = base.join("scheduler");
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    let executions = ExecutionLedger::open(dir.join(executions::DB_FILE))
        .map_err(|e| format!("could not open the scheduler's audit ledger: {e}"))?;

    // Caduceus is single-instance (see `lib.rs`'s `tauri_plugin_single_instance`
    // wiring), so any execution still `claimed`/`running` here can only be
    // left over from a previous launch of this same app that ended without
    // finishing it — a crash, a force-quit. There is no live owner to check
    // for the way Hermes' multi-process gateway/CLI model needs to (see
    // `executions.rs`'s module doc): proving the previous owner is gone is
    // simply "this is a new process, and only one of us is ever alive."
    match executions.recover_interrupted() {
        Ok(0) => {}
        Ok(n) => log::warn!(
            "scheduler: {n} execution(s) left mid-run by a previous session were marked unknown"
        ),
        Err(e) => log::warn!("scheduler: could not reconcile interrupted executions: {e}"),
    }

    app.manage(SchedulerRuntime {
        store: JobStore::new(dir),
        executions,
        running: Arc::new(Mutex::new(HashSet::new())),
    });
    Ok(())
}

/// Fire one job in the background: guard against a second concurrent
/// dispatch of the same job id, then hand off to [`run::execute`] on a
/// spawned task and report a change once it finishes. Both [`ticker::tick`]
/// (for each job it finds due) and
/// [`commands::scheduler_run_now`] call this — it is the one place a job
/// actually starts running, so both callers behave identically from here on.
pub(crate) fn dispatch<R: Runtime>(app: AppHandle<R>, job: Job, source: ExecutionSource) {
    if ensure_managed(&app).is_err() {
        // ensure_managed already logged the reason; a job that cannot even
        // find its own runtime state has nowhere to record an outcome, so
        // there is nothing more useful to do than skip this firing.
        return;
    }
    let rt = app.state::<SchedulerRuntime>().inner().clone();
    if !rt.begin(&job.id) {
        log::info!("scheduler: job \"{}\" is already running; skipping this firing", job.name);
        return;
    }

    let job_id = job.id.clone();
    let store = rt.store.clone();
    let executions = rt.executions.clone();
    tauri::async_runtime::spawn(async move {
        run::execute(&app, &store, &executions, job, source).await;
        rt.finish(&job_id);
        let _ = app.emit(SCHEDULER_CHANGED_EVENT, &job_id);
    });
}

/// Every skill name referenced by any job that could still run again —
/// enabled or paused, but not `Done` (a spent one-shot or an exhausted
/// repeat count will never load a skill again, so it has nothing left to
/// protect). Shaped as the exact `HashSet<String>` `skills::lifecycle
/// ::apply_transitions`'s `protected` parameter expects — see this module's
/// "Skills integration" doc for the call site.
pub fn referenced_skill_names<R: Runtime>(app: &AppHandle<R>) -> HashSet<String> {
    if ensure_managed(app).is_err() {
        return HashSet::new();
    }
    let Ok(jobs) = app.state::<SchedulerRuntime>().store.list() else {
        return HashSet::new();
    };
    jobs.into_iter()
        .filter(|j| j.state != job::JobState::Done)
        .flat_map(|j| j.skills)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
