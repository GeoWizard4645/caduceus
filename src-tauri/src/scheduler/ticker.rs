//! The heartbeat that turns "a job is due" into "a job ran": once a minute,
//! forever, for the life of the process.
//!
//! ```text
//!  spawn (once, from lib.rs::setup)
//!   │
//!   ▼
//!  loop {
//!      tick()  ───▶ ensure_managed (idempotent)
//!         │
//!         ▼
//!      JobStore::claim_due(now)
//!         │  (under the jobs lock: mark Running + durably advance
//!         │   next_run_at for every due job, BEFORE any of them runs
//!         │   — see job.rs's module doc for why this ordering is the
//!         │   whole at-most-once guarantee)
//!         ▼
//!      dispatch() one spawned task per due job ──▶ run::execute
//!
//!      sleep(TICK_INTERVAL)
//!  }
//! ```
//!
//! Sixty seconds, matching Hermes' own ticker interval — frequent enough
//! that "every 5 minutes" means what it says without a noticeable slip,
//! infrequent enough that idling with zero jobs configured costs nothing
//! worth measuring.

use std::time::Duration;

use chrono::Local;
use tauri::{AppHandle, Manager, Runtime};

use super::executions::ExecutionSource;
use super::{dispatch, ensure_managed, SchedulerRuntime};

pub const TICK_INTERVAL: Duration = Duration::from_secs(60);

/// Start the ticker. Call once, at launch; it runs for the life of the
/// process. Mirrors `update::spawn_update_watcher`'s shape exactly: one
/// `tauri::async_runtime::spawn`, sleep-then-tick forever, reading whatever
/// is current on each wake rather than capturing anything once.
pub fn spawn<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tick(&app).await;
            tokio::time::sleep(TICK_INTERVAL).await;
        }
    });
}

async fn tick<R: Runtime>(app: &AppHandle<R>) {
    // Retried every tick rather than given up on after one failure — a
    // transient problem (disk full for a moment, a permissions hiccup)
    // should not permanently disable the scheduler for the rest of the
    // session. This call is also what performs [`ExecutionLedger::
    // recover_interrupted`] the first time it succeeds after launch, so a
    // crash-interrupted execution from the previous run gets reconciled
    // essentially immediately rather than only reincidentally on demand.
    if let Err(e) = ensure_managed(app) {
        log::error!("scheduler: not ready yet ({e}); will retry next tick");
        return;
    }

    let rt = app.state::<SchedulerRuntime>();
    let now = Local::now();
    let due = match rt.store.claim_due(now) {
        Ok(jobs) => jobs,
        Err(e) => {
            log::error!("scheduler: could not scan for due jobs: {e}");
            return;
        }
    };

    if due.is_empty() {
        return;
    }
    log::info!("scheduler: {} job(s) due", due.len());
    for job in due {
        dispatch(app.clone(), job, ExecutionSource::Ticker);
    }
}
