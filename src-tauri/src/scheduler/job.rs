//! The job schema, and its on-disk store.
//!
//! # Persistence
//!
//! `jobs.json` lives at `<app-data>/scheduler/jobs.json`, one JSON array of
//! [`Job`]. Every write goes through [`JobStore::with_jobs`], which is the
//! one place a mutation happens in this whole module: write to a fresh temp
//! file in the same directory, `fsync` it, then `rename` it over the real
//! path. The rename is atomic on any filesystem this app ships on, so a
//! reader (or a crash) never observes a half-written file — it sees either
//! the old content or the new content, never a torn mix of both. The file is
//! then chmod'd `0600`: a job's `prompt` can carry anything the person who
//! scheduled it typed, and there is no reason another local account should
//! be able to read it.
//!
//! A [`std::fs::File::try_lock`] on a sibling `.jobs.lock` file wraps the
//! whole read-mutate-write cycle. Caduceus is single-instance (see
//! `lib.rs`'s `tauri_plugin_single_instance` wiring), so in practice this
//! lock's job is to serialize concurrent callers *within* this one process —
//! the ticker's per-minute sweep and a person clicking "pause" in the UI at
//! the same moment — rather than to arbitrate between separate OS processes
//! the way Hermes' `cron/jobs.py::_jobs_lock` has to (it supports a separate
//! CLI invocation running alongside a live gateway). It is still a real
//! advisory OS file lock rather than an in-process-only mutex, both because
//! it costs nothing extra to make it one and because it is one less thing to
//! revisit if that assumption ever changes. Acquisition is bounded — see
//! [`LOCK_TIMEOUT`] — and failing to acquire it degrades to proceeding
//! without it (logged) rather than hanging the scheduler forever on a wedged
//! lock.
//!
//! One malformed job record must never take the rest of the file down with
//! it. [`JobStore::read_unlocked`] parses the top level as a bare
//! `Vec<serde_json::Value>` first (which only fails if the file is not even
//! valid JSON), then attempts each element independently — a record that
//! fails to deserialize as a [`Job`] (a hand-edited file, a future format
//! this build does not understand) is logged and skipped, and every *other*
//! job in the file still loads and still runs.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use super::schedule::Schedule;

/// Filename inside `<app-data>/scheduler/`.
const JOBS_FILE: &str = "jobs.json";
/// Advisory cross-process/cross-task lock guarding a read-mutate-write cycle.
const LOCK_FILE: &str = ".jobs.lock";

/// How long [`JobStore::acquire_lock`] waits for a contended lock before
/// giving up and proceeding without it. Generous relative to how briefly the
/// lock is ever actually held (field updates on a small JSON file — no
/// network call, no agent execution happens while it is held), so this
/// should only ever bite if the lock is genuinely wedged, e.g. a previous
/// process crashed while holding a lock a `File` handle. It is scoped much
/// tighter than Hermes' 30s equivalent because nothing here needs to
/// arbitrate across separate OS processes — see the module doc.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// A scheduled job. See the `scheduler` module doc for the execution
/// semantics (at-most-once, no message history, deny-by-default approval)
/// that go with this shape — this file only owns the data and its
/// persistence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub name: String,
    /// The instruction sent to the agent as its (only) user turn — see the
    /// module doc on why there is never more than one. When
    /// [`Job::no_agent`] is set, this is instead a `/bin/sh -c` command line
    /// run directly with no LLM involved at all; see `run.rs`.
    pub prompt: String,
    /// Names of skills to load for this run — see `run.rs::load_named_skills`
    /// for exactly what "load" means today and its one documented gap.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Overrides the primary backend's configured model for this job only.
    /// `None` uses whatever the primary backend is already set to.
    #[serde(default)]
    pub model: Option<String>,
    pub schedule: Schedule,
    /// [`Schedule::describe`], captured at create/update time rather than
    /// recomputed on every read — see that method's doc for why.
    pub schedule_display: String,
    #[serde(default)]
    pub repeat: Repeat,
    /// A hard gate: a disabled job is never due, independent of `state`.
    /// Kept distinct from `state` the same way Hermes keeps its `enabled`
    /// boolean and `state` string separate — pausing/resuming toggles both
    /// together, but the two answer different questions ("should this ever
    /// run" vs. "what is it doing right now").
    pub enabled: bool,
    pub state: JobState,
    #[serde(default)]
    pub paused_at: Option<DateTime<Local>>,
    #[serde(default)]
    pub paused_reason: Option<String>,
    pub created_at: DateTime<Local>,
    /// When this job will next fire, or `None` if it never will again (a
    /// spent one-shot, an exhausted repeat count). This is the field
    /// [`JobStore::claim_due`] advances *before* a run starts — see the
    /// module doc on `ticker.rs` for why that ordering is the whole
    /// at-most-once guarantee.
    #[serde(default)]
    pub next_run_at: Option<DateTime<Local>>,
    #[serde(default)]
    pub last_run_at: Option<DateTime<Local>>,
    #[serde(default)]
    pub last_status: Option<RunStatus>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub deliver: Deliver,
    /// An absolute path a `no_agent` job's script runs in. Agent-mode jobs
    /// accept this field but nothing in `run.rs` wires it any deeper today
    /// (Caduceus's MCP tools each have their own fixed process, not a
    /// per-call working directory to redirect) — an honest gap, not a
    /// silent one; see `run.rs`'s doc.
    #[serde(default)]
    pub workdir: Option<String>,
    /// When true, `prompt` is run as a shell command with no LLM involved at
    /// all. Not one of the fields the task spec enumerated by name, but
    /// implied by "a `no_agent` mode" needing somewhere to live — see the
    /// module doc's persistence section and `run.rs`.
    #[serde(default)]
    pub no_agent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting for `next_run_at`.
    Scheduled,
    /// A firing is in flight right now.
    Running,
    /// Will not run again until [`super::commands::scheduler_resume_job`] is
    /// called — see [`Job::paused_at`] / [`Job::paused_reason`].
    Paused,
    /// A one-shot that has fired, or a repeat count that is exhausted.
    /// Terminal: nothing un-does this except editing the job's schedule
    /// (which computes a fresh `next_run_at` and moves it back to
    /// `Scheduled`).
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Success,
    Failed,
    /// The run reached a tool call and the deny-by-default approval gate
    /// refused it — see the module doc and `run.rs::DenyApproval`. Kept
    /// distinct from `Failed` because it is not really a failure of the job;
    /// it is the scheduler correctly refusing to let an unattended run touch
    /// anything, which is the point of that gate existing.
    Declined,
}

/// How many times a job should run, and how many it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Repeat {
    /// `None` = run forever (until paused/deleted). `Some(n)` = stop after
    /// `n` completed runs, successful or not — a run that fails still counts
    /// against the budget, the same way a `once` schedule is spent whether
    /// or not the one run it got actually succeeded.
    pub times: Option<u32>,
    pub completed: u32,
}

/// Where a finished run's result goes beyond the job's own `last_status` /
/// `last_error` and the audit ledger — both of which record every run
/// regardless of this setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Deliver {
    /// Nothing further — the job's own fields and the execution ledger are
    /// the whole record. The default: it is the only option that requires
    /// nothing else in the app to exist.
    #[default]
    None,
    /// A local notification when the run finishes (macOS only today, via
    /// `osascript`; a no-op elsewhere — see `run.rs::notify`).
    Notification,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Handle to one profile's `jobs.json`. Cheap to clone — it is just a
/// directory path — so every caller (each command, the ticker) can hold its
/// own copy rather than needing to share one behind an `Arc`.
#[derive(Debug, Clone)]
pub struct JobStore {
    dir: PathBuf,
}

impl JobStore {
    /// `dir` is the scheduler's data directory (e.g. `<app-data>/scheduler`)
    /// — created on first use, not by this constructor, so building a
    /// `JobStore` itself can never fail.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn jobs_path(&self) -> PathBuf {
        self.dir.join(JOBS_FILE)
    }

    fn lock_path(&self) -> PathBuf {
        self.dir.join(LOCK_FILE)
    }

    // -- locking -------------------------------------------------------

    /// Best-effort acquisition of the advisory lock — see the module doc.
    /// `None` means "proceed without it" (logged at the call site's
    /// discretion is unnecessary; every failure path here already logs),
    /// never a reason to fail the caller's whole operation: a wedged lock
    /// must not brick the scheduler.
    fn acquire_lock(&self) -> Option<std::fs::File> {
        let path = self.lock_path();
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) => {
                log::warn!(
                    "scheduler: could not open the lock file at {} ({e}); proceeding without \
                     cross-process locking",
                    path.display()
                );
                return None;
            }
        };

        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match file.try_lock() {
                Ok(()) => return Some(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        log::warn!(
                            "scheduler: timed out after {LOCK_TIMEOUT:?} waiting for {} — \
                             proceeding without the lock",
                            path.display()
                        );
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(std::fs::TryLockError::Error(e)) => {
                    log::warn!(
                        "scheduler: could not lock {} ({e}); proceeding without cross-process \
                         locking",
                        path.display()
                    );
                    return None;
                }
            }
        }
    }

    // -- raw read/write --------------------------------------------------

    /// Tolerant read: a file that does not exist yet is an empty list, and a
    /// job record that fails to deserialize is skipped (and logged) rather
    /// than taking every other job down with it — see the module doc.
    fn read_unlocked(&self) -> Result<Vec<Job>, String> {
        let path = self.jobs_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("could not read {}: {e}", path.display())),
        };
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }

        let raw: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|e| {
            format!(
                "the jobs file at {} is corrupted and could not be parsed at all: {e}",
                path.display()
            )
        })?;

        let mut jobs = Vec::with_capacity(raw.len());
        for (i, value) in raw.into_iter().enumerate() {
            match serde_json::from_value::<Job>(value) {
                Ok(job) => jobs.push(job),
                Err(e) => log::error!(
                    "scheduler: skipping unreadable job record #{i} in {}: {e}",
                    path.display()
                ),
            }
        }
        Ok(jobs)
    }

    /// Atomic write: temp file in the same directory, `fsync`, `rename`,
    /// then `chmod 0600` — see the module doc.
    fn write_unlocked(&self, jobs: &[Job]) -> Result<(), String> {
        let path = self.jobs_path();
        let parent = path
            .parent()
            .ok_or_else(|| "the jobs file has no parent directory".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;

        let bytes = serde_json::to_vec_pretty(jobs).map_err(|e| format!("could not encode jobs: {e}"))?;
        let tmp_path = parent.join(format!(".jobs.json.tmp-{}", uuid::Uuid::new_v4()));

        let written: Result<(), String> = (|| {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp_path)
                .map_err(|e| format!("could not create a temp file: {e}"))?;
            file.write_all(&bytes)
                .map_err(|e| format!("could not write the temp file: {e}"))?;
            // Durable on disk *before* the rename that makes it visible —
            // fsync-then-rename, never the other order, or the atomicity the
            // rename buys is moot.
            file.sync_all()
                .map_err(|e| format!("could not fsync the temp file: {e}"))?;
            Ok(())
        })();

        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }

        std::fs::rename(&tmp_path, &path)
            .map_err(|e| format!("could not replace {}: {e}", path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
                // The data itself is already safely written; a permissions
                // failure here is worth knowing about, not worth failing the
                // save over.
                log::warn!("scheduler: could not set 0600 permissions on {}: {e}", path.display());
            }
        }
        Ok(())
    }

    /// Read the job list under the lock, writing nothing back — what every
    /// pure read (`list`, `get`) goes through. Kept separate from
    /// [`Self::with_jobs`] so a read never bumps the file's mtime or risks a
    /// write it had no reason to make.
    fn read(&self) -> Result<Vec<Job>, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("could not create {}: {e}", self.dir.display()))?;
        let _lock = self.acquire_lock();
        self.read_unlocked()
    }

    /// The one place a mutation happens: lock, load, let `f` mutate the list
    /// in place, save, unlock. Every `create`/`update`/`delete`/`pause`/
    /// `resume`/`claim_*`/`record_result` below is a thin wrapper around
    /// this, which is what makes "load, mutate, save" one atomic unit a
    /// concurrent caller can never observe half-done.
    fn with_jobs<T>(&self, f: impl FnOnce(&mut Vec<Job>) -> T) -> Result<T, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("could not create {}: {e}", self.dir.display()))?;
        let _lock = self.acquire_lock();
        let mut jobs = self.read_unlocked()?;
        let result = f(&mut jobs);
        self.write_unlocked(&jobs)?;
        Ok(result)
    }

    // -- CRUD --------------------------------------------------------------

    pub fn list(&self) -> Result<Vec<Job>, String> {
        self.read()
    }

    pub fn get(&self, id: &str) -> Result<Option<Job>, String> {
        Ok(self.read()?.into_iter().find(|j| j.id == id))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        name: Option<String>,
        prompt: String,
        skills: Vec<String>,
        model: Option<String>,
        schedule_spec: &str,
        repeat_times: Option<u32>,
        deliver: Deliver,
        workdir: Option<String>,
        no_agent: bool,
        now: DateTime<Local>,
    ) -> Result<Job, String> {
        let schedule = super::schedule::parse(schedule_spec, now)?;
        let workdir = validate_workdir(workdir)?;
        let next_run_at = schedule.compute_next_run(now, None);
        if next_run_at.is_none() && matches!(schedule, Schedule::Once { .. }) {
            return Err(
                "That one-shot time is too far in the past to schedule — pick a time in the future.".into(),
            );
        }
        // Computed before `schedule` is moved into the struct literal below.
        let repeat_times = normalize_repeat_times(repeat_times, &schedule);

        let job = Job {
            id: uuid::Uuid::new_v4().to_string(),
            name: name
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| default_name(&prompt, &skills, no_agent)),
            skills,
            model: model.filter(|m| !m.trim().is_empty()),
            schedule_display: schedule.describe(),
            schedule,
            repeat: Repeat {
                times: repeat_times,
                completed: 0,
            },
            enabled: true,
            state: JobState::Scheduled,
            paused_at: None,
            paused_reason: None,
            created_at: now,
            next_run_at,
            last_run_at: None,
            last_status: None,
            last_error: None,
            deliver,
            workdir,
            no_agent,
            prompt,
        };

        self.with_jobs(|jobs| jobs.push(job.clone()))?;
        Ok(job)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        id: &str,
        name: Option<String>,
        prompt: String,
        skills: Vec<String>,
        model: Option<String>,
        schedule_spec: &str,
        repeat_times: Option<u32>,
        deliver: Deliver,
        workdir: Option<String>,
        no_agent: bool,
        now: DateTime<Local>,
    ) -> Result<Job, String> {
        let schedule = super::schedule::parse(schedule_spec, now)?;
        let workdir = validate_workdir(workdir)?;
        let repeat_times = normalize_repeat_times(repeat_times, &schedule);

        self.with_jobs(|jobs| {
            let job = jobs
                .iter_mut()
                .find(|j| j.id == id)
                .ok_or_else(|| format!("No such job: {id}"))?;

            job.name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| job.name.clone());
            job.prompt = prompt;
            job.skills = skills;
            job.model = model.filter(|m| !m.trim().is_empty());
            job.schedule_display = schedule.describe();
            job.schedule = schedule;
            job.repeat.times = repeat_times;
            job.deliver = deliver;
            job.workdir = workdir;
            job.no_agent = no_agent;

            // A paused job's next_run_at is left exactly as it was — inert
            // either way, since `is_due` never looks at it for a job that is
            // not `Scheduled` — until an explicit resume recomputes it. An
            // update must not accidentally wake a job the user deliberately
            // silenced by recomputing (and thus re-arming) it here too.
            if job.state != JobState::Paused {
                job.next_run_at = job.schedule.compute_next_run(now, job.last_run_at);
                job.state = if job.next_run_at.is_some() {
                    JobState::Scheduled
                } else {
                    JobState::Done
                };
            }
            Ok(job.clone())
        })?
    }

    pub fn delete(&self, id: &str) -> Result<bool, String> {
        self.with_jobs(|jobs| {
            let before = jobs.len();
            jobs.retain(|j| j.id != id);
            jobs.len() != before
        })
    }

    pub fn pause(&self, id: &str, reason: Option<String>, now: DateTime<Local>) -> Result<Job, String> {
        self.with_jobs(|jobs| {
            let job = jobs
                .iter_mut()
                .find(|j| j.id == id)
                .ok_or_else(|| format!("No such job: {id}"))?;
            job.enabled = false;
            job.state = JobState::Paused;
            job.paused_at = Some(now);
            job.paused_reason = reason;
            Ok(job.clone())
        })?
    }

    pub fn resume(&self, id: &str, now: DateTime<Local>) -> Result<Job, String> {
        self.with_jobs(|jobs| {
            let job = jobs
                .iter_mut()
                .find(|j| j.id == id)
                .ok_or_else(|| format!("No such job: {id}"))?;
            job.enabled = true;
            job.paused_at = None;
            job.paused_reason = None;
            job.next_run_at = job.schedule.compute_next_run(now, job.last_run_at);
            job.state = if job.next_run_at.is_some() {
                JobState::Scheduled
            } else {
                JobState::Done
            };
            Ok(job.clone())
        })?
    }

    // -- execution lifecycle -------------------------------------------

    /// The at-most-once core. For every job due at `now`: mark it `Running`
    /// and — for anything recurring — durably advance `next_run_at` to its
    /// *following* occurrence, all before this function returns and before
    /// the caller has run a single one of them. See the `scheduler` module
    /// doc: this ordering, not anything at run time, is what makes a crash
    /// mid-run cost at most one missed firing instead of a re-fire loop.
    pub fn claim_due(&self, now: DateTime<Local>) -> Result<Vec<Job>, String> {
        self.with_jobs(|jobs| {
            let mut due = Vec::new();
            for job in jobs.iter_mut() {
                if !is_due(job, now) {
                    continue;
                }
                claim_one(job, now);
                due.push(job.clone());
            }
            due
        })
    }

    /// The single-job equivalent of [`Self::claim_due`], used by `run_now`.
    /// Claims and advances `id` exactly as if it had come up due on its own
    /// — including recomputing a recurring job's cadence around this firing
    /// — regardless of whether it actually was due. See the module doc on
    /// `commands.rs::scheduler_run_now`.
    pub fn claim_for_run(&self, id: &str, now: DateTime<Local>) -> Result<Job, String> {
        self.with_jobs(|jobs| {
            let job = jobs
                .iter_mut()
                .find(|j| j.id == id)
                .ok_or_else(|| format!("No such job: {id}"))?;
            claim_one(job, now);
            Ok(job.clone())
        })?
    }

    /// Record one run's outcome: `last_run_at`/`last_status`/`last_error`,
    /// increment `repeat.completed`, and move to `Done` if that exhausts the
    /// budget, the schedule was a one-shot, or — belt and suspenders —
    /// `next_run_at` somehow ended up unset. Never re-advances `next_run_at`
    /// itself; that already happened in [`Self::claim_due`] /
    /// [`Self::claim_for_run`] before this run started.
    pub fn record_result(
        &self,
        id: &str,
        now: DateTime<Local>,
        status: RunStatus,
        error: Option<String>,
    ) -> Result<Option<Job>, String> {
        self.with_jobs(|jobs| {
            let job = jobs.iter_mut().find(|j| j.id == id)?;
            job.last_run_at = Some(now);
            job.last_status = Some(status);
            job.last_error = error;
            job.repeat.completed = job.repeat.completed.saturating_add(1);

            let exhausted = job.repeat.times.is_some_and(|t| job.repeat.completed >= t);
            let once_spent = matches!(job.schedule, Schedule::Once { .. });
            if exhausted || once_spent || job.next_run_at.is_none() {
                job.state = JobState::Done;
                job.next_run_at = None;
            } else {
                job.state = JobState::Scheduled;
            }
            Some(job.clone())
        })
    }
}

/// Whether `job` should fire now. Also repairs a missing `next_run_at` in
/// place (a hand-edited file, or any future bug that leaves one unset) by
/// recomputing it from the schedule — which is why this takes `&mut Job`
/// rather than `&Job` despite reading like a pure predicate.
fn is_due(job: &mut Job, now: DateTime<Local>) -> bool {
    if !job.enabled || !matches!(job.state, JobState::Scheduled) {
        return false;
    }
    match job.next_run_at {
        Some(t) => t <= now,
        None => {
            let recomputed = job.schedule.compute_next_run(now, job.last_run_at);
            job.next_run_at = recomputed;
            recomputed.is_some_and(|t| t <= now)
        }
    }
}

/// Shared by [`JobStore::claim_due`] and [`JobStore::claim_for_run`]: mark a
/// job as firing right now, and durably advance a recurring schedule's
/// `next_run_at` before the caller runs it. A `Once` schedule has nothing to
/// advance to — its next occurrence *is* this firing, which
/// `record_result`'s `once_spent` check retires afterward.
fn claim_one(job: &mut Job, now: DateTime<Local>) {
    job.state = JobState::Running;
    if !matches!(job.schedule, Schedule::Once { .. }) {
        job.next_run_at = job.schedule.compute_next_run(now, Some(now));
    }
}

/// `times<=0` and "no explicit count on a one-shot schedule" both collapse
/// to the same defaults Hermes uses: a non-positive count means "forever" is
/// meaningless, so it is treated as unset, and a bare one-shot with no count
/// at all defaults to exactly one run rather than silently meaning
/// "forever" for a schedule that can only ever fire once anyway.
fn normalize_repeat_times(times: Option<u32>, schedule: &Schedule) -> Option<u32> {
    let times = times.filter(|&t| t > 0);
    match (times, schedule) {
        (None, Schedule::Once { .. }) => Some(1),
        (t, _) => t,
    }
}

fn default_name(prompt: &str, skills: &[String], no_agent: bool) -> String {
    let source = if !prompt.trim().is_empty() {
        prompt.trim()
    } else if let Some(first) = skills.first().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        first
    } else if no_agent {
        "script job"
    } else {
        "scheduled job"
    };
    let truncated: String = source.chars().take(50).collect();
    let truncated = truncated.trim();
    if truncated.is_empty() {
        "scheduled job".to_string()
    } else {
        truncated.to_string()
    }
}

/// Validate a job's working directory the same way Hermes'
/// `cron.jobs._normalize_workdir` does: empty/absent means "off", `~`
/// expands, and anything else must already be an absolute, existing
/// directory — a scheduled job runs with no shell of its own to resolve a
/// relative path against, and a typo is better caught at save time than
/// discovered the first time the job fires.
fn validate_workdir(workdir: Option<String>) -> Result<Option<String>, String> {
    let Some(raw) = workdir else { return Ok(None) };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
        dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(trimmed))
    } else if trimmed == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(trimmed))
    } else {
        PathBuf::from(trimmed)
    };

    if !expanded.is_absolute() {
        return Err(format!(
            "The working directory must be an absolute path (got \"{trimmed}\") — a scheduled \
             job has no shell of its own to resolve a relative one against."
        ));
    }
    if !expanded.is_dir() {
        return Err(format!("\"{}\" does not exist or is not a directory.", expanded.display()));
    }
    Ok(Some(expanded.to_string_lossy().to_string()))
}

#[allow(dead_code)] // exercised by tests below; kept private to this module otherwise
fn jobs_path_for_test(dir: &Path) -> PathBuf {
    dir.join(JOBS_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "caduceus-scheduler-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn now() -> DateTime<Local> {
        Local::now()
    }

    // -----------------------------------------------------------------
    // CRUD
    // -----------------------------------------------------------------

    #[test]
    fn create_then_list_round_trips_a_job() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store
            .create(
                Some("My job".into()),
                "do the thing".into(),
                vec!["research".into()],
                None,
                "every 30m",
                None,
                Deliver::None,
                None,
                false,
                now(),
            )
            .unwrap();
        assert_eq!(job.name, "My job");
        assert_eq!(job.state, JobState::Scheduled);
        assert!(job.enabled);
        assert_eq!(job.repeat, Repeat { times: None, completed: 0 });

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, job.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_job_with_no_name_gets_one_derived_from_its_prompt() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store
            .create(None, "summarize my inbox every morning".into(), vec![], None, "every 1d", None, Deliver::None, None, false, now())
            .unwrap();
        assert_eq!(job.name, "summarize my inbox every morning");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_one_shot_schedule_defaults_repeat_to_exactly_one() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store
            .create(None, "p".into(), vec![], None, "30m", None, Deliver::None, None, false, now())
            .unwrap();
        assert_eq!(job.repeat.times, Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recurring_schedule_with_no_explicit_count_repeats_forever() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store
            .create(None, "p".into(), vec![], None, "every 30m", None, Deliver::None, None, false, now())
            .unwrap();
        assert_eq!(job.repeat.times, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zero_repeat_count_normalizes_to_forever() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store
            .create(None, "p".into(), vec![], None, "every 30m", Some(0), Deliver::None, None, false, now())
            .unwrap();
        assert_eq!(job.repeat.times, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_replaces_editable_fields_and_recomputes_the_schedule() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store.create(None, "old".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now()).unwrap();

        let updated = store
            .update(&job.id, Some("renamed".into()), "new prompt".into(), vec!["skill-a".into()], Some("gpt-x".into()), "every 20m", Some(3), Deliver::Notification, None, false, now())
            .unwrap();

        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.prompt, "new prompt");
        assert_eq!(updated.skills, vec!["skill-a".to_string()]);
        assert_eq!(updated.model.as_deref(), Some("gpt-x"));
        assert_eq!(updated.schedule, Schedule::Interval { minutes: 20 });
        assert_eq!(updated.repeat.times, Some(3));
        assert_eq!(updated.deliver, Deliver::Notification);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn updating_an_unknown_job_is_an_error_not_a_silent_no_op() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let err = store
            .update("nope", None, "p".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now())
            .unwrap_err();
        assert!(err.contains("No such job"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_the_job_and_reports_whether_it_existed() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store.create(None, "p".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now()).unwrap();
        assert!(store.delete(&job.id).unwrap());
        assert!(!store.delete(&job.id).unwrap(), "deleting twice must not error, just report false");
        assert!(store.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pause_then_resume_round_trips_through_state_and_recomputes_next_run() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store.create(None, "p".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now()).unwrap();

        let paused = store.pause(&job.id, Some("testing".into()), now()).unwrap();
        assert_eq!(paused.state, JobState::Paused);
        assert!(!paused.enabled);
        assert_eq!(paused.paused_reason.as_deref(), Some("testing"));

        let resumed = store.resume(&job.id, now()).unwrap();
        assert_eq!(resumed.state, JobState::Scheduled);
        assert!(resumed.enabled);
        assert!(resumed.paused_at.is_none());
        assert!(resumed.next_run_at.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_on_a_paused_job_does_not_wake_it() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store.create(None, "p".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now()).unwrap();
        store.pause(&job.id, None, now()).unwrap();

        let updated = store.update(&job.id, None, "new prompt".into(), vec![], None, "every 20m", None, Deliver::None, None, false, now()).unwrap();
        assert_eq!(updated.state, JobState::Paused, "editing a paused job must not silently resume it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------
    // At-most-once: claim_due advances next_run_at before the caller runs
    // anything, so a second scan immediately after never re-claims the same
    // firing — this is the whole guarantee, tested without any agent or
    // async machinery at all.
    // -----------------------------------------------------------------

    #[test]
    fn claim_due_returns_a_job_whose_next_run_has_already_passed() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store.create(None, "p".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now()).unwrap();
        // Force it due immediately, as if 10 minutes had already passed.
        let due_now = now();
        store.with_jobs(|jobs| jobs[0].next_run_at = Some(due_now)).unwrap();

        let due = store.claim_due(due_now).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, job.id);
        assert_eq!(due[0].state, JobState::Running);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_claim_due_scan_never_reclaims_the_same_firing() {
        // This is the core "advance before run" property: even though the
        // job in this test never actually finishes running (record_result is
        // never called), a second due-scan right after the first must not
        // see it as due again — next_run_at was already moved forward
        // durably by the first scan, simulating a crash mid-run.
        let dir = tempdir();
        let store = JobStore::new(&dir);
        store.create(None, "p".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now()).unwrap();
        let due_now = now();
        store.with_jobs(|jobs| jobs[0].next_run_at = Some(due_now)).unwrap();

        let first_scan = store.claim_due(due_now).unwrap();
        assert_eq!(first_scan.len(), 1, "the job should be due on the first scan");

        let second_scan = store.claim_due(due_now).unwrap();
        assert!(
            second_scan.is_empty(),
            "a crash between claim_due and the run finishing must cost at most one missed \
             firing, never a re-fire — next_run_at must already be in the future"
        );

        let persisted = store.get(&store.list().unwrap()[0].id).unwrap().unwrap();
        assert!(persisted.next_run_at.unwrap() > due_now, "next_run_at must have moved forward");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_paused_job_is_never_due_even_if_its_next_run_has_passed() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store.create(None, "p".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now()).unwrap();
        store.pause(&job.id, None, now()).unwrap();
        let due_now = now();
        store.with_jobs(|jobs| jobs[0].next_run_at = Some(due_now)).unwrap();

        assert!(store.claim_due(due_now).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claim_for_run_fires_a_job_that_was_not_otherwise_due() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store.create(None, "p".into(), vec![], None, "every 1d", None, Deliver::None, None, false, now()).unwrap();
        assert!(job.next_run_at.unwrap() > now() + chrono::Duration::hours(1), "sanity: not due for a day");

        let claimed = store.claim_for_run(&job.id, now()).unwrap();
        assert_eq!(claimed.state, JobState::Running);

        // It must not still show up on a normal due-scan afterward (it is
        // already Running, and its cadence was recomputed around this
        // firing).
        assert!(store.claim_due(now()).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------
    // record_result / repeat exhaustion
    // -----------------------------------------------------------------

    #[test]
    fn a_completed_run_moves_a_recurring_job_back_to_scheduled() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store.create(None, "p".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now()).unwrap();
        store.claim_for_run(&job.id, now()).unwrap();

        let after = store.record_result(&job.id, now(), RunStatus::Success, None).unwrap().unwrap();
        assert_eq!(after.state, JobState::Scheduled);
        assert_eq!(after.repeat.completed, 1);
        assert!(after.next_run_at.is_some());
    }

    #[test]
    fn a_once_schedule_is_done_after_its_single_run_even_on_failure() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store.create(None, "p".into(), vec![], None, "30m", None, Deliver::None, None, false, now()).unwrap();
        store.claim_for_run(&job.id, now()).unwrap();

        let after = store
            .record_result(&job.id, now(), RunStatus::Failed, Some("boom".into()))
            .unwrap()
            .unwrap();
        assert_eq!(after.state, JobState::Done);
        assert!(after.next_run_at.is_none());
        assert_eq!(after.last_error.as_deref(), Some("boom"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_repeat_count_is_exhausted_after_its_final_run() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let job = store.create(None, "p".into(), vec![], None, "every 10m", Some(2), Deliver::None, None, false, now()).unwrap();

        store.claim_for_run(&job.id, now()).unwrap();
        let after_first = store.record_result(&job.id, now(), RunStatus::Success, None).unwrap().unwrap();
        assert_eq!(after_first.state, JobState::Scheduled, "one run left in the budget");

        store.claim_for_run(&job.id, now()).unwrap();
        let after_second = store.record_result(&job.id, now(), RunStatus::Success, None).unwrap().unwrap();
        assert_eq!(after_second.state, JobState::Done, "budget exhausted");
        assert!(after_second.next_run_at.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------
    // Atomic persistence
    // -----------------------------------------------------------------

    #[test]
    fn the_jobs_file_has_owner_only_permissions() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        store.create(None, "p".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now()).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(jobs_path_for_test(&dir)).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_temp_files_are_left_behind_after_a_save() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        for i in 0..5 {
            store
                .create(None, format!("job {i}"), vec![], None, "every 10m", None, Deliver::None, None, false, now())
                .unwrap();
        }
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp files must never survive a save: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_file_actually_serializes_concurrent_writers_no_update_is_lost() {
        // The correctness property "cross-process safety via a lock file"
        // is asking for: without it, two threads racing a read-modify-write
        // cycle on the same file can each read the same starting state and
        // each write back a version missing the other's change — a lost
        // update. Twenty threads each creating five jobs against the same
        // on-disk store is exactly that race, hammered hard enough that a
        // missing or broken lock would reliably lose at least one of the
        // 100 total jobs; a working lock (in-process serialization would
        // already suffice for same-process threads, but this exercises the
        // exact `with_jobs` path the cross-process file lock also guards)
        // must lose none.
        let dir = tempdir();
        let store = JobStore::new(&dir);

        std::thread::scope(|scope| {
            for t in 0..20 {
                let store = &store;
                scope.spawn(move || {
                    for i in 0..5 {
                        store
                            .create(None, format!("thread {t} job {i}"), vec![], None, "every 10m", None, Deliver::None, None, false, now())
                            .unwrap();
                    }
                });
            }
        });

        let jobs = store.list().unwrap();
        assert_eq!(jobs.len(), 100, "every job from every thread must survive — none lost to a lock race");
        let unique_ids: std::collections::HashSet<_> = jobs.iter().map(|j| &j.id).collect();
        assert_eq!(unique_ids.len(), 100, "every id must be distinct — no job silently overwritten another");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_jobs_file_reads_as_an_empty_list_not_an_error() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        assert_eq!(store.list().unwrap(), Vec::<Job>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_corrupted_job_record_does_not_take_the_rest_of_the_file_down() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let good = store.create(None, "good job".into(), vec![], None, "every 10m", None, Deliver::None, None, false, now()).unwrap();

        // Hand-corrupt the file: append a record missing required fields
        // (as if a future/older build, or a manual edit, wrote something
        // this schema cannot read) alongside the good one.
        let path = jobs_path_for_test(&dir);
        let mut jobs_json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        jobs_json.as_array_mut().unwrap().push(serde_json::json!({ "id": "broken", "not": "a real job" }));
        std::fs::write(&path, serde_json::to_string_pretty(&jobs_json).unwrap()).unwrap();

        let jobs = store.list().unwrap();
        assert_eq!(jobs.len(), 1, "the unreadable record must be skipped, not fatal");
        assert_eq!(jobs[0].id, good.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_round_trip_through_disk_preserves_every_field() {
        let dir = tempdir();
        let store = JobStore::new(&dir);
        let created = store
            .create(
                Some("full job".into()),
                "prompt text".into(),
                vec!["a".into(), "b".into()],
                Some("gpt-x".into()),
                "0 9 * * 1-5",
                Some(5),
                Deliver::Notification,
                None,
                false,
                now(),
            )
            .unwrap();

        // Force a fresh read straight off disk rather than any in-memory
        // state, by building a brand new store over the same directory.
        let reloaded = JobStore::new(&dir).get(&created.id).unwrap().unwrap();
        assert_eq!(reloaded.name, created.name);
        assert_eq!(reloaded.prompt, created.prompt);
        assert_eq!(reloaded.skills, created.skills);
        assert_eq!(reloaded.model, created.model);
        assert_eq!(reloaded.schedule, created.schedule);
        assert_eq!(reloaded.repeat, created.repeat);
        assert_eq!(reloaded.deliver, created.deliver);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------
    // validate_workdir
    // -----------------------------------------------------------------

    #[test]
    fn a_relative_workdir_is_rejected() {
        assert!(validate_workdir(Some("relative/path".into())).is_err());
    }

    #[test]
    fn a_nonexistent_workdir_is_rejected() {
        assert!(validate_workdir(Some("/definitely/not/a/real/path/xyz".into())).is_err());
    }

    #[test]
    fn an_empty_workdir_means_unset() {
        assert_eq!(validate_workdir(None).unwrap(), None);
        assert_eq!(validate_workdir(Some("   ".into())).unwrap(), None);
    }

    #[test]
    fn an_existing_absolute_workdir_is_accepted() {
        let dir = tempdir();
        let got = validate_workdir(Some(dir.to_string_lossy().to_string())).unwrap();
        assert!(got.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
