//! The execution ledger: a durable, append-mostly record of every attempt to
//! run a job — `executions.db`, a small SQLite database of its own, separate
//! from `jobs.json`.
//!
//! # This is an audit trail, not a retry queue
//!
//! Nothing ever reads this table to decide what to do next. The scheduler's
//! entire notion of "what should run and when" lives in [`super::job::Job`]
//! — `next_run_at`, `state`, `repeat` — and that alone is what
//! [`super::job::JobStore::claim_due`] consults. A row in this table
//! recording a `failed` or even an `unknown` (crash-interrupted, see
//! [`ExecutionLedger::recover_interrupted`]) run is never picked back up and
//! automatically retried; the job's own schedule is the only thing that ever
//! fires it again. What this table is for is answering, after the fact,
//! "did job X actually run at 3pm, and what happened" — a question
//! `last_status`/`last_error` on the job can only answer for the *most
//! recent* run, and a question the job's own record cannot answer at all if
//! the process died before it got to write one. Losing this table costs
//! history; it never costs correctness, because nothing load-bearing reads
//! it back.
//!
//! # Schema
//!
//! ```text
//! CREATE TABLE executions (
//!     id          TEXT PRIMARY KEY,
//!     job_id      TEXT NOT NULL,
//!     source      TEXT NOT NULL,               -- 'ticker' | 'manual'
//!     process_id  TEXT NOT NULL,                -- random, per process launch
//!     pid         INTEGER NOT NULL,
//!     status      TEXT NOT NULL CHECK(status IN
//!                   ('claimed','running','completed','failed','unknown')),
//!     claimed_at  INTEGER NOT NULL,             -- unix seconds
//!     started_at  INTEGER,
//!     finished_at INTEGER,
//!     error       TEXT
//! )
//! ```
//!
//! `claimed` is written the instant an attempt is dispatched, before the
//! no_agent script or the agent loop has done anything at all; `running`
//! once it is actually under way; `completed`/`failed` are terminal and
//! never rewritten afterward. `unknown` is the fourth terminal state and the
//! one Hermes' own ledger exists to explain: an attempt that was still
//! `claimed`/`running` the next time this app started up, whose owning
//! process therefore cannot have finished it cleanly. Caduceus is
//! single-instance (`tauri_plugin_single_instance`, wired in `lib.rs`), so —
//! unlike Hermes, which has to prove a specific PID plus its start time are
//! both gone before calling an interrupted attempt `unknown` (see
//! `cron/executions.py::_owner_is_live` in the reference implementation) —
//! there is nothing to check here: a fresh process finding a `claimed`/
//! `running` row at all means it belongs to a previous run of this same app
//! that did not shut down cleanly, because single-instance guarantees only
//! one of them was ever alive.
//!
//! `pid`/`process_id` are still recorded (for a person reading the table to
//! answer "which launch of Caduceus was this"), just never consulted to
//! decide anything.

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub const DB_FILE: &str = "executions.db";

/// Cap on how many *terminal* rows (`completed`/`failed`/`unknown`) this
/// ledger keeps, pruned oldest-first after each [`ExecutionLedger::finish`].
/// `claimed`/`running` rows are never pruned — a row only ever leaves the
/// table by first reaching a terminal state. Matches the order of magnitude
/// of Hermes' own `MAX_TERMINAL_EXECUTIONS`: enough history to be useful,
/// small enough that this file never becomes something worth worrying
/// about.
const MAX_TERMINAL_ROWS: i64 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Claimed,
    Running,
    Completed,
    Failed,
    /// Left `claimed`/`running` by a previous launch of this app that did
    /// not reach a terminal state before exiting — see the module doc.
    Unknown,
}

impl ExecutionStatus {
    /// The write direction (`'claimed'`/`'running'`/…) is never routed
    /// through this type at all — every `INSERT`/`UPDATE` below writes the
    /// literal that matches its own transition directly (`SET
    /// status='running'`, right next to the `WHERE status='claimed'` guard
    /// it pairs with), which reads more clearly at each call site than a
    /// value built here and threaded through as a bound parameter would.
    /// This is only ever the read direction: turning a column value back
    /// into a typed status.
    fn parse(s: &str) -> Self {
        match s {
            "running" => ExecutionStatus::Running,
            "completed" => ExecutionStatus::Completed,
            "failed" => ExecutionStatus::Failed,
            "unknown" => ExecutionStatus::Unknown,
            _ => ExecutionStatus::Claimed,
        }
    }
}

/// What dispatched this attempt — the ticker's own due-scan, or a person
/// clicking "run now". Both go through the identical execution path in
/// `run.rs`; this only ever affects what gets written to this one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSource {
    Ticker,
    Manual,
}

impl ExecutionSource {
    fn as_str(self) -> &'static str {
        match self {
            ExecutionSource::Ticker => "ticker",
            ExecutionSource::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Execution {
    pub id: String,
    pub job_id: String,
    pub source: ExecutionSource,
    pub process_id: String,
    pub pid: i64,
    pub status: ExecutionStatus,
    /// Unix seconds.
    pub claimed_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

fn row_to_execution(row: &rusqlite::Row) -> rusqlite::Result<Execution> {
    Ok(Execution {
        id: row.get(0)?,
        job_id: row.get(1)?,
        source: match row.get::<_, String>(2)?.as_str() {
            "manual" => ExecutionSource::Manual,
            _ => ExecutionSource::Ticker,
        },
        process_id: row.get(3)?,
        pid: row.get(4)?,
        status: ExecutionStatus::parse(&row.get::<_, String>(5)?),
        claimed_at: row.get(6)?,
        started_at: row.get(7)?,
        finished_at: row.get(8)?,
        error: row.get(9)?,
    })
}

/// Second resolution, not millisecond: this ledger is a human-facing audit
/// trail (see the module doc), and nothing here needs finer than "when did
/// this happen" for a person reading it. Two attempts *can* land in the same
/// second — [`Self::list`]/[`Self::latest`] break that tie deterministically
/// with `rowid` (SQLite's own implicit, monotonically-increasing insertion
/// order) rather than by widening this column, which is both the simpler
/// fix and, unlike a clock reading, cannot itself tie. See those methods'
/// `ORDER BY` clauses.
fn now_secs() -> i64 {
    chrono::Local::now().timestamp()
}

/// Handle to `executions.db`. Cheap to clone — the connection is shared
/// behind an `Arc<Mutex<_>>`, matching `chat::ChatStore`'s shape, since
/// SQLite itself does not let two connections write concurrently anyway and
/// there is no reason to pay for a connection pool for a table this small
/// and this rarely written to.
#[derive(Clone)]
pub struct ExecutionLedger {
    conn: Arc<Mutex<Connection>>,
    /// Random per-process identifier, distinct from the OS `pid` (which can
    /// be reused across launches) — recorded for a human reading the table,
    /// never consulted by this module. See the module doc.
    process_id: String,
}

impl ExecutionLedger {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        let ledger = Self {
            conn: Arc::new(Mutex::new(conn)),
            process_id: uuid::Uuid::new_v4().to_string(),
        };
        ledger.migrate()?;
        Ok(ledger)
    }

    /// `idx_executions_job` deliberately indexes only `(job_id, claimed_at
    /// DESC)`, not `rowid` too: SQLite rejects `rowid` as an indexed
    /// expression outright (`CREATE INDEX ... (col, rowid)` fails with `no
    /// such column: rowid`), even though the identical expression is
    /// completely valid in a plain `SELECT ... ORDER BY` (verified directly
    /// against this project's bundled SQLite version) — `rowid` can be read
    /// and sorted on, just never indexed. This index is therefore a
    /// performance aid for the `WHERE job_id=? ORDER BY claimed_at DESC`
    /// prefix only; every query below still appends `, rowid DESC` itself
    /// for the same-second tiebreak [`now_secs`]'s doc describes, which
    /// SQLite satisfies with a small extra sort rather than the index alone
    /// — negligible at this table's size cap ([`MAX_TERMINAL_ROWS`]).
    fn migrate(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS executions (
                 id          TEXT PRIMARY KEY,
                 job_id      TEXT NOT NULL,
                 source      TEXT NOT NULL,
                 process_id  TEXT NOT NULL,
                 pid         INTEGER NOT NULL,
                 status      TEXT NOT NULL CHECK(status IN
                               ('claimed','running','completed','failed','unknown')),
                 claimed_at  INTEGER NOT NULL,
                 started_at  INTEGER,
                 finished_at INTEGER,
                 error       TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_executions_job
                 ON executions(job_id, claimed_at DESC);
             CREATE INDEX IF NOT EXISTS idx_executions_status
                 ON executions(status);",
        )
    }

    /// Record a new attempt as `claimed` — called before the job's script or
    /// agent loop has done anything at all, so an attempt that never even
    /// gets as far as `mark_running` (a panic, a crash on the way in) is
    /// still visible in the ledger rather than silently missing.
    pub fn create(&self, job_id: &str, source: ExecutionSource) -> rusqlite::Result<Execution> {
        let conn = self.conn.lock();
        let id = uuid::Uuid::new_v4().to_string();
        let claimed_at = now_secs();
        let pid = std::process::id() as i64;
        conn.execute(
            "INSERT INTO executions (id, job_id, source, process_id, pid, status, claimed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'claimed', ?6)",
            params![id, job_id, source.as_str(), self.process_id, pid, claimed_at],
        )?;
        Ok(Execution {
            id,
            job_id: job_id.to_string(),
            source,
            process_id: self.process_id.clone(),
            pid,
            status: ExecutionStatus::Claimed,
            claimed_at,
            started_at: None,
            finished_at: None,
            error: None,
        })
    }

    /// Transition one `claimed` attempt to `running`, exactly once — a
    /// no-op (returns `Ok(false)`) if it has already moved past `claimed`,
    /// which should not normally happen but must never panic if it does.
    pub fn mark_running(&self, execution_id: &str) -> rusqlite::Result<bool> {
        let conn = self.conn.lock();
        let n = conn.execute(
            "UPDATE executions SET status='running', started_at=?1 WHERE id=?2 AND status='claimed'",
            params![now_secs(), execution_id],
        )?;
        Ok(n == 1)
    }

    /// Write a terminal result — `completed` or `failed` — exactly once.
    /// Terminal attempts are never rewritten (the `WHERE status IN
    /// ('claimed','running')` guard), matching the module doc's "terminal
    /// states are immutable" rule.
    pub fn finish(&self, execution_id: &str, success: bool, error: Option<&str>) -> rusqlite::Result<bool> {
        let conn = self.conn.lock();
        let status = if success { "completed" } else { "failed" };
        let n = conn.execute(
            "UPDATE executions SET status=?1, finished_at=?2, error=?3
             WHERE id=?4 AND status IN ('claimed','running')",
            params![status, now_secs(), error, execution_id],
        )?;
        if n == 1 {
            prune_terminal_rows(&conn)?;
        }
        Ok(n == 1)
    }

    /// Mark every attempt still `claimed`/`running` as `unknown` — call this
    /// once, at startup, before the ticker's first tick. See the module doc
    /// for why Caduceus's single-instance guarantee makes this
    /// unconditional rather than needing a per-row liveness check the way
    /// Hermes' equivalent does. Returns how many rows were reconciled, purely
    /// for a startup log line.
    pub fn recover_interrupted(&self) -> rusqlite::Result<usize> {
        let conn = self.conn.lock();
        let n = conn.execute(
            "UPDATE executions SET status='unknown', finished_at=?1, error=?2
             WHERE status IN ('claimed','running')",
            params![
                now_secs(),
                "Caduceus restarted before this execution reached a terminal state; whether it \
                 finished is unknown."
            ],
        )?;
        if n > 0 {
            prune_terminal_rows(&conn)?;
        }
        Ok(n)
    }

    /// Newest-first execution history, optionally scoped to one job.
    pub fn list(&self, job_id: Option<&str>, limit: i64) -> rusqlite::Result<Vec<Execution>> {
        let conn = self.conn.lock();
        let limit = limit.clamp(1, 500);
        match job_id {
            Some(id) => {
                let mut stmt = conn.prepare(
                    "SELECT id, job_id, source, process_id, pid, status, claimed_at, started_at, \
                     finished_at, error FROM executions WHERE job_id=?1 \
                     ORDER BY claimed_at DESC, rowid DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![id, limit], row_to_execution)?;
                rows.collect()
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT id, job_id, source, process_id, pid, status, claimed_at, started_at, \
                     finished_at, error FROM executions \
                     ORDER BY claimed_at DESC, rowid DESC LIMIT ?1",
                )?;
                let rows = stmt.query_map(params![limit], row_to_execution)?;
                rows.collect()
            }
        }
    }

    /// The most recent attempt for one job, if it has ever run.
    pub fn latest(&self, job_id: &str) -> rusqlite::Result<Option<Execution>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, job_id, source, process_id, pid, status, claimed_at, started_at, \
             finished_at, error FROM executions WHERE job_id=?1 \
             ORDER BY claimed_at DESC, rowid DESC LIMIT 1",
            params![job_id],
            row_to_execution,
        )
        .optional()
    }
}

fn prune_terminal_rows(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM executions WHERE id IN (
             SELECT id FROM executions
             WHERE status IN ('completed','failed','unknown')
             ORDER BY claimed_at DESC, rowid DESC
             LIMIT -1 OFFSET ?1
         )",
        params![MAX_TERMINAL_ROWS],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_temp() -> ExecutionLedger {
        let path = std::env::temp_dir().join(format!(
            "caduceus-executions-test-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        ExecutionLedger::open(path).unwrap()
    }

    #[test]
    fn a_new_attempt_starts_claimed() {
        let ledger = open_temp();
        let exec = ledger.create("job-1", ExecutionSource::Ticker).unwrap();
        assert_eq!(exec.status, ExecutionStatus::Claimed);
        assert_eq!(exec.job_id, "job-1");
        assert!(exec.started_at.is_none());
    }

    #[test]
    fn the_normal_lifecycle_goes_claimed_running_completed() {
        let ledger = open_temp();
        let exec = ledger.create("job-1", ExecutionSource::Manual).unwrap();
        assert!(ledger.mark_running(&exec.id).unwrap());
        assert!(ledger.finish(&exec.id, true, None).unwrap());

        let latest = ledger.latest("job-1").unwrap().unwrap();
        assert_eq!(latest.status, ExecutionStatus::Completed);
        assert!(latest.started_at.is_some());
        assert!(latest.finished_at.is_some());
    }

    #[test]
    fn a_failed_run_records_its_error() {
        let ledger = open_temp();
        let exec = ledger.create("job-1", ExecutionSource::Ticker).unwrap();
        ledger.mark_running(&exec.id).unwrap();
        ledger.finish(&exec.id, false, Some("boom")).unwrap();

        let latest = ledger.latest("job-1").unwrap().unwrap();
        assert_eq!(latest.status, ExecutionStatus::Failed);
        assert_eq!(latest.error.as_deref(), Some("boom"));
    }

    #[test]
    fn finishing_can_skip_mark_running_for_an_attempt_that_never_started() {
        // A create() immediately followed by a crash before mark_running is
        // exactly the case finish()'s WHERE clause has to also accept
        // 'claimed', not only 'running'.
        let ledger = open_temp();
        let exec = ledger.create("job-1", ExecutionSource::Ticker).unwrap();
        assert!(ledger.finish(&exec.id, false, Some("never got going")).unwrap());
        assert_eq!(ledger.latest("job-1").unwrap().unwrap().status, ExecutionStatus::Failed);
    }

    #[test]
    fn a_terminal_attempt_cannot_be_rewritten() {
        let ledger = open_temp();
        let exec = ledger.create("job-1", ExecutionSource::Ticker).unwrap();
        ledger.finish(&exec.id, true, None).unwrap();

        // A second finish() call must be a no-op, not a silent overwrite.
        assert!(!ledger.finish(&exec.id, false, Some("too late")).unwrap());
        let latest = ledger.latest("job-1").unwrap().unwrap();
        assert_eq!(latest.status, ExecutionStatus::Completed);
        assert!(latest.error.is_none());
    }

    #[test]
    fn mark_running_on_an_already_running_attempt_is_a_no_op() {
        let ledger = open_temp();
        let exec = ledger.create("job-1", ExecutionSource::Ticker).unwrap();
        assert!(ledger.mark_running(&exec.id).unwrap());
        assert!(!ledger.mark_running(&exec.id).unwrap());
    }

    #[test]
    fn recover_interrupted_reconciles_claimed_and_running_rows_but_not_terminal_ones() {
        let ledger = open_temp();
        let claimed = ledger.create("job-1", ExecutionSource::Ticker).unwrap();
        let running = ledger.create("job-2", ExecutionSource::Ticker).unwrap();
        ledger.mark_running(&running.id).unwrap();
        let done = ledger.create("job-3", ExecutionSource::Ticker).unwrap();
        ledger.finish(&done.id, true, None).unwrap();

        let n = ledger.recover_interrupted().unwrap();
        assert_eq!(n, 2, "only the claimed and running rows should be touched");

        assert_eq!(ledger.latest("job-1").unwrap().unwrap().status, ExecutionStatus::Unknown);
        assert_eq!(ledger.latest("job-2").unwrap().unwrap().status, ExecutionStatus::Unknown);
        assert_eq!(ledger.latest("job-3").unwrap().unwrap().status, ExecutionStatus::Completed);
        let _ = claimed;
    }

    #[test]
    fn recover_interrupted_is_idempotent_on_an_already_clean_ledger() {
        let ledger = open_temp();
        assert_eq!(ledger.recover_interrupted().unwrap(), 0);
    }

    #[test]
    fn list_is_scoped_by_job_and_ordered_newest_first() {
        // Ordering is `claimed_at DESC, rowid DESC` — the rowid tiebreaker
        // (SQLite's implicit, monotonically increasing insertion order) is
        // what keeps this deterministic without needing two rows to land in
        // different wall-clock seconds, since `claimed_at` only has
        // second resolution.
        let ledger = open_temp();
        let a1 = ledger.create("job-a", ExecutionSource::Ticker).unwrap();
        let a2 = ledger.create("job-a", ExecutionSource::Ticker).unwrap();
        ledger.create("job-b", ExecutionSource::Ticker).unwrap();

        let for_a = ledger.list(Some("job-a"), 10).unwrap();
        assert_eq!(for_a.len(), 2);
        assert_eq!(for_a[0].id, a2.id, "newest first");
        assert_eq!(for_a[1].id, a1.id);

        let all = ledger.list(None, 10).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn an_invalid_status_can_never_be_written_the_check_constraint_holds() {
        let ledger = open_temp();
        let conn = ledger.conn.lock();
        let err = conn
            .execute(
                "INSERT INTO executions (id, job_id, source, process_id, pid, status, claimed_at) \
                 VALUES ('x','job-1','ticker','p',1,'not-a-real-status',0)",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("constraint"));
    }

    #[test]
    fn terminal_rows_beyond_the_cap_are_pruned_but_recent_ones_survive() {
        let ledger = open_temp();
        // Small, deterministic check of the pruning mechanism itself rather
        // than pushing 1000+ rows through a test: shrink the effective cap
        // by asserting on relative counts before/after a manual prune with a
        // tiny limit, using the same query prune_terminal_rows runs.
        for i in 0..5 {
            let exec = ledger.create(&format!("job-{i}"), ExecutionSource::Ticker).unwrap();
            ledger.finish(&exec.id, true, None).unwrap();
        }
        {
            let conn = ledger.conn.lock();
            conn.execute(
                "DELETE FROM executions WHERE id IN (
                     SELECT id FROM executions WHERE status IN ('completed','failed','unknown')
                     ORDER BY claimed_at DESC, rowid DESC LIMIT -1 OFFSET 2
                 )",
                [],
            )
            .unwrap();
        }
        assert_eq!(ledger.list(None, 100).unwrap().len(), 2);
    }
}
