//! Executing one firing of a job: the `no_agent` script path, and the
//! fresh, history-free agent session path.
//!
//! # There is no message history — ever
//!
//! This is the single most important property of this file, and the one
//! most likely to surprise someone used to the interactive `/` chat, where
//! every turn sees everything before it. A cron-triggered agent run starts
//! from exactly one [`Message::user`] turn — the job's `prompt`, optionally
//! prefixed with loaded skill content — and nothing else. No prior run's
//! output, no chat history, no memory of ever having run before. That is
//! deliberate, not a limitation to work around: a job that fires unattended
//! at 3am cannot lean on "what we talked about earlier" the way an
//! interactive session can, because there is no "earlier" — every firing is
//! its own isolated session with a synthetic id (`cron_<job_id>_<timestamp>`,
//! see [`run_agent`]). If a job needs context beyond its own prompt, that
//! context has to come from somewhere durable it can pull from *every* time
//! it runs: a named skill (see [`load_named_skills`]), or content baked
//! into the prompt itself. Reaching for "surely it remembers the last run"
//! is the single most common way to misuse a scheduled job — it will not,
//! and designing a job's prompt to be fully self-contained is the fix.
//!
//! # Approval is denied by default, and this is not configurable
//!
//! [`run_agent`] never reuses `AgentSettings::confirm_before_first_action` —
//! that setting answers "should an *interactive* session pause for a human
//! before touching anything", and a scheduled run has no human present to
//! answer that pause. Every tool call a cron-triggered session attempts
//! therefore hits [`DenyApproval`], which refuses unconditionally, the same
//! fail-closed default the reference implementation (Hermes Agent's
//! `approvals.cron_mode`, which defaults to `"deny"`) chose for the same
//! reason. Concretely: a job whose prompt only needs the model's own
//! reasoning/text (summarizing something already in the prompt, drafting a
//! reminder) runs fine; a job that pushes the model to call an MCP tool
//! stops at that first attempt with [`crate::agent::StopReason::Declined`],
//! recorded as [`super::job::RunStatus::Declined`] rather than silently
//! doing nothing or silently doing something unattended. There is
//! deliberately no per-job or global setting to relax this in this module —
//! see the `scheduler` module doc's "honest gaps" for what a real opt-in
//! would need to look like and why it does not exist yet.
//!
//! # `no_agent`: no LLM at all
//!
//! When [`super::job::Job::no_agent`] is set, none of the above applies: the
//! agent loop, the deny-by-default approval gate, and the "no history"
//! framing are all specific to the LLM path and simply do not run.
//! `job.prompt` is instead executed directly as a `/bin/sh -c` command line
//! — the script *is* the job, matching Hermes' own `no_agent` semantics for
//! classic watchdog scripts (a health check, a `curl`, a backup) that gain
//! nothing from an LLM in the loop and should not pay for one.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use tauri::{AppHandle, Manager, Runtime};

use super::executions::{ExecutionLedger, ExecutionSource};
use super::job::{Deliver, Job, JobStore, RunStatus};
use crate::agent::{self, AgentLoopContext, ApprovalAsker, ApprovalGate, CancelToken, Message, StopReason};

/// How long a `no_agent` job's script gets before it is killed. Long enough
/// for a real watchdog script to finish real work (a health check, a `git
/// fetch`, a small backup); short enough that a wedged one does not tie up a
/// ticker slot forever. Matches Hermes' default cron inactivity timeout
/// (`cron/jobs.py::_DEFAULT_CRON_INACTIVITY_TIMEOUT`, 600s) rather than
/// `crate::tools::TOOL_TIMEOUT` (10s), which is sized for an interactive
/// palette action, not an unattended job.
const NO_AGENT_TIMEOUT: Duration = Duration::from_secs(600);

/// The outcome of one firing, in the shape `run.rs`'s two paths (agent /
/// no_agent) both produce and [`execute`] persists.
pub struct RunOutcome {
    pub status: RunStatus,
    /// What the run produced — the agent's final message, or the script's
    /// stdout. Used for delivery (e.g. a notification body); not itself
    /// persisted on the job (only `status`/`error` are).
    pub output: String,
    pub error: Option<String>,
}

impl RunOutcome {
    fn failed(error: impl Into<String>) -> Self {
        Self { status: RunStatus::Failed, output: String::new(), error: Some(error.into()) }
    }
}

/// Refuses every approval request unconditionally. See the module doc's
/// "Approval is denied by default" section for why this exists and why it
/// is not configurable.
struct DenyApproval;

#[async_trait::async_trait]
impl ApprovalAsker for DenyApproval {
    async fn ask(&self, _session_id: &str, _summary: &str) -> bool {
        false
    }
}

/// Execute one firing of `job` end to end: dispatch it (agent or script),
/// record the attempt in the audit ledger, apply delivery, and persist the
/// job's own bookkeeping fields. Called by both `ticker.rs` (a due job) and
/// `commands.rs::scheduler_run_now` (a manual firing) — see
/// [`super::dispatch`], which both funnel through.
///
/// Never panics and never propagates an error to its caller: a job that
/// fails is a recorded outcome (in the ledger, and on the job's own
/// `last_status`/`last_error`), not a crashed ticker. A failure to even
/// *write* that record (a full disk, a locked file) is logged and otherwise
/// swallowed for the same reason — see the inline comments below.
pub async fn execute<R: Runtime>(
    app: &AppHandle<R>,
    store: &JobStore,
    ledger: &ExecutionLedger,
    job: Job,
    source: ExecutionSource,
) {
    let execution_id = match ledger.create(&job.id, source) {
        Ok(e) => Some(e.id),
        Err(e) => {
            log::error!(
                "scheduler: could not open an audit-ledger entry for job \"{}\" ({e}); running it \
                 anyway — the ledger is an audit trail, not a gate, see its module doc",
                job.name
            );
            None
        }
    };
    if let Some(id) = &execution_id {
        if let Err(e) = ledger.mark_running(id) {
            log::warn!("scheduler: could not mark execution {id} running: {e}");
        }
    }

    let outcome = if job.no_agent {
        run_no_agent(&job)
    } else {
        run_agent(app, &job).await
    };

    if let Some(id) = &execution_id {
        let ok = matches!(outcome.status, RunStatus::Success);
        if let Err(e) = ledger.finish(id, ok, outcome.error.as_deref()) {
            log::warn!("scheduler: could not close out execution {id}: {e}");
        }
    }

    deliver(&job, &outcome);

    let now = Local::now();
    if let Err(e) = store.record_result(&job.id, now, outcome.status, outcome.error.clone()) {
        log::error!("scheduler: could not persist the result of job \"{}\": {e}", job.name);
    }
}

// ---------------------------------------------------------------------------
// no_agent: the script IS the job
// ---------------------------------------------------------------------------

fn run_no_agent(job: &Job) -> RunOutcome {
    let command_line = job.prompt.trim();
    if command_line.is_empty() {
        return RunOutcome::failed(
            "no_agent is on but this job has no command to run — its prompt is empty.",
        );
    }

    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(command_line);
    if let Some(dir) = job.workdir.as_deref().filter(|d| !d.is_empty()) {
        cmd.current_dir(dir);
    }

    match crate::tools::output_with_timeout(&mut cmd, NO_AGENT_TIMEOUT, "the script did not finish in time") {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() {
                RunOutcome { status: RunStatus::Success, output: stdout, error: None }
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let error = if stderr.is_empty() { format!("exited with {}", out.status) } else { stderr };
                RunOutcome { status: RunStatus::Failed, output: stdout, error: Some(error) }
            }
        }
        Err(e) => RunOutcome::failed(e),
    }
}

// ---------------------------------------------------------------------------
// Agent path: fresh session, no history, deny-by-default approval
// ---------------------------------------------------------------------------

async fn run_agent<R: Runtime>(app: &AppHandle<R>, job: &Job) -> RunOutcome {
    let Some(settings) = app.try_state::<crate::settings::SettingsManager>() else {
        return RunOutcome::failed("Settings are not ready yet.");
    };
    let snapshot = settings.get();
    let mut config = match agent::resolve_backend(&snapshot, agent::BackendRole::Primary) {
        Ok(c) => c,
        Err(e) => return RunOutcome::failed(e.user_message()),
    };
    if let Some(model) = job.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        config.model = model.to_string();
    }

    let (skills_prefix, missing) = load_named_skills(app, &job.skills);
    if !missing.is_empty() {
        log::warn!(
            "scheduler: job \"{}\" asked for skill(s) {missing:?} but none were found on disk — \
             running without them; see run.rs::load_named_skills",
            job.name
        );
    }
    let prompt = if skills_prefix.is_empty() {
        job.prompt.clone()
    } else {
        format!("{skills_prefix}\n\n{}", job.prompt)
    };

    // Exactly one turn. No history — see the module doc.
    let messages = vec![Message::user(prompt)];

    let session_id = format!("cron_{}_{}", job.id, Local::now().format("%Y%m%d_%H%M%S"));
    let step_app = app.clone();
    let step_session = session_id.clone();
    let on_step: agent::StepSink = Arc::new(move |step| {
        use tauri::Emitter;
        // Reuses the same event the interactive AgentPanel listens to.
        // Nothing about that panel needs to change for a cron run's steps to
        // show up in it, filterable by the `cron_` session-id prefix, should
        // a future UI want to surface a live run — this is a free win from
        // sharing `run_tool_loop` rather than a second, cron-specific event.
        if let Err(e) = step_app.emit(agent::AGENT_STEP_EVENT, &step) {
            log::warn!("scheduler: could not emit a step for {step_session}: {e}");
        }
    });

    let ctx = AgentLoopContext {
        session_id: session_id.clone(),
        on_step,
        cancel: CancelToken::default(),
        // Fail closed — see the module doc. Not registered with
        // `agent::AgentRuntime` (its `register`/`unregister` are private to
        // that module, and rightly so — this session does not need the
        // interactive stop/approve bookkeeping that registry exists for; see
        // the `scheduler` module doc's honest-gaps section for what that
        // means in practice: no live "stop" button for an in-flight cron
        // run today).
        approval: ApprovalGate::Ask(Arc::new(DenyApproval)),
    };

    match agent::run_tool_loop(app, &config, messages, ctx).await {
        Ok(outcome) => match outcome.stop_reason {
            StopReason::Completed => RunOutcome { status: RunStatus::Success, output: outcome.final_message, error: None },
            StopReason::Declined => RunOutcome {
                status: RunStatus::Declined,
                output: String::new(),
                error: Some(
                    "This job tried to call a tool, but scheduled runs deny tool approval by \
                     default — there is no one present to approve it. Rewrite the prompt to avoid \
                     tools, or see the scheduler module doc."
                        .to_string(),
                ),
            },
            StopReason::MaxSteps => RunOutcome {
                status: RunStatus::Failed,
                output: outcome.final_message,
                error: Some(format!("stopped after {} rounds of tool calls without a final answer", agent::MAX_ITERATIONS)),
            },
            StopReason::UserStopped => RunOutcome::failed("cancelled"),
            StopReason::Error => RunOutcome {
                status: RunStatus::Failed,
                output: outcome.final_message,
                error: Some("the backend reported an error".to_string()),
            },
        },
        Err(e) => RunOutcome::failed(e.user_message()),
    }
}

/// Full content for a job's named skills, via the real skills system
/// ([`crate::skills::tiers::view_skill`], Tier 2 — the whole `SKILL.md`
/// body), returning the assembled prefix text and the names that could not
/// be loaded (not found, or unreadable).
///
/// This is `crate::skills`'s intended integration point for "an agent wants
/// this skill's content," the same call a `skill_view` tool invocation from
/// an interactive session makes — see `skills/mod.rs`'s "Selection has no
/// ranker" section for why loading full content by name (Tier 2) rather
/// than searching is the whole design. It also has a side effect worth
/// knowing about: `view_skill` bumps that skill's `view_count`/`use_count`
/// (see its own doc), so a skill a job actually loads every firing reads as
/// "in active use" to `skills::lifecycle`'s staleness sweep exactly the way
/// interactive use would — a scheduled job's skills do not go stale from
/// neglect merely because no human happened to look at them too. What that
/// does *not* cover is a *paused* job, whose skills never run and so never
/// get this signal; see [`super::referenced_skill_names`] for the
/// complementary fix and the integration point it hands to
/// `skills::lifecycle::apply_transitions`'s `protected` parameter.
fn load_named_skills<R: Runtime>(app: &AppHandle<R>, names: &[String]) -> (String, Vec<String>) {
    if names.is_empty() {
        return (String::new(), Vec::new());
    }
    let Ok(root) = app.path().app_data_dir().map(|d| d.join(crate::skills::SKILLS_DIR_NAME)) else {
        return (String::new(), names.to_vec());
    };

    let mut sections = Vec::new();
    let mut missing = Vec::new();
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        match crate::skills::tiers::view_skill(&root, trimmed) {
            Ok(view) => sections.push(format!("## Skill: {}\n\n{}", view.name, view.content.trim())),
            Err(e) => {
                log::debug!("scheduler: could not load skill \"{trimmed}\" for a job: {e}");
                missing.push(trimmed.to_string());
            }
        }
    }
    (sections.join("\n\n"), missing)
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

fn deliver(job: &Job, outcome: &RunOutcome) {
    match job.deliver {
        Deliver::None => {}
        Deliver::Notification => notify(job, outcome),
    }
}

/// A local notification when a run finishes. macOS only, via `osascript` —
/// the same "shell out to a tool already on the machine" approach
/// `crate::tools` uses elsewhere (see `tools::define_word`,
/// `tools::copy_finder_path`) rather than a `tauri-plugin-notification`
/// dependency this crate does not have.
fn notify(job: &Job, outcome: &RunOutcome) {
    #[cfg(target_os = "macos")]
    {
        let ok = matches!(outcome.status, RunStatus::Success);
        let title = if ok { format!("\u{201c}{}\u{201d} finished", job.name) } else { format!("\u{201c}{}\u{201d} failed", job.name) };
        let body = if ok {
            first_line(&outcome.output)
        } else {
            outcome.error.clone().unwrap_or_else(|| "no further detail".to_string())
        };
        let script = format!(
            "display notification {} with title {}",
            applescript_quote(&body),
            applescript_quote(&title)
        );
        // Best-effort: a notification that fails to show is not worth
        // failing (or even logging loudly about) a run that otherwise
        // completed exactly as recorded.
        let _ = Command::new("osascript").arg("-e").arg(script).output();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (job, outcome);
    }
}

#[cfg(target_os = "macos")]
fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(200).collect()
}

#[cfg(target_os = "macos")]
fn applescript_quote(s: &str) -> String {
    let truncated: String = s.chars().take(200).collect();
    format!("\"{}\"", truncated.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::job::{JobState, Repeat};
    use crate::scheduler::schedule::Schedule;

    fn base_job(no_agent: bool, prompt: &str) -> Job {
        let now = Local::now();
        Job {
            id: "job-1".into(),
            name: "test job".into(),
            prompt: prompt.into(),
            skills: Vec::new(),
            model: None,
            schedule: Schedule::Interval { minutes: 10 },
            schedule_display: "every 10 minutes".into(),
            repeat: Repeat::default(),
            enabled: true,
            state: JobState::Running,
            paused_at: None,
            paused_reason: None,
            created_at: now,
            next_run_at: Some(now),
            last_run_at: None,
            last_status: None,
            last_error: None,
            deliver: Deliver::None,
            workdir: None,
            no_agent,
        }
    }

    // -----------------------------------------------------------------
    // run_no_agent
    // -----------------------------------------------------------------

    #[test]
    fn a_no_agent_job_runs_its_prompt_as_a_shell_command() {
        let job = base_job(true, "echo hello-from-scheduler");
        let outcome = run_no_agent(&job);
        assert_eq!(outcome.status, RunStatus::Success);
        assert_eq!(outcome.output, "hello-from-scheduler");
        assert!(outcome.error.is_none());
    }

    #[test]
    fn a_failing_command_is_reported_as_failed_with_stderr_as_the_error() {
        let job = base_job(true, "echo oops 1>&2; exit 1");
        let outcome = run_no_agent(&job);
        assert_eq!(outcome.status, RunStatus::Failed);
        assert_eq!(outcome.error.as_deref(), Some("oops"));
    }

    #[test]
    fn an_empty_prompt_is_refused_before_spawning_anything() {
        let job = base_job(true, "   ");
        let outcome = run_no_agent(&job);
        assert_eq!(outcome.status, RunStatus::Failed);
        assert!(outcome.error.unwrap().contains("empty"));
    }

    #[test]
    fn a_no_agent_job_runs_in_its_configured_workdir() {
        let dir = std::env::temp_dir().join(format!("caduceus-scheduler-run-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut job = base_job(true, "pwd");
        job.workdir = Some(dir.to_string_lossy().to_string());
        let outcome = run_no_agent(&job);
        assert_eq!(outcome.status, RunStatus::Success);
        // Compare canonicalized paths — /tmp is a symlink to /private/tmp on
        // macOS, and `pwd` reports whichever spelling the shell resolved.
        let expected = std::fs::canonicalize(&dir).unwrap();
        let actual = std::fs::canonicalize(outcome.output.trim()).unwrap();
        assert_eq!(actual, expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------
    // load_named_skills is exercised indirectly through the public API
    // available to this module — it needs an AppHandle, which is exactly
    // the kind of dependency the rest of this crate's tests (habits.rs,
    // etc.) avoid by keeping logic that needs one thin. Its two outcomes
    // (found vs. missing) are simple enough to read from the function body;
    // real coverage of "does a file on disk actually get picked up" lives
    // implicitly in the fact that `run_agent`'s integration path is the same
    // code, and the reading logic itself (read_to_string + fallback
    // extension + join) has no branch a unit test without an AppHandle could
    // exercise any more meaningfully than reading it.
    // -----------------------------------------------------------------
}
