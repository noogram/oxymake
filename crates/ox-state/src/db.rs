//! Database connection manager and job operations for `state.db`.
//!
//! [`StateDb`] is the primary entry point for all state persistence.
//! It manages three concerns:
//!
//! 1. **Execution state** — what is running, pending, or done right now.
//!    This is ephemeral and reconstructible from the DAG + cache, but
//!    persisting it enables crash recovery and cooperative multi-session
//!    execution (see OXYMAKE-THESIS.md §6.13).
//!
//! 2. **Cache metadata** — content hashes and output hashes that
//!    `ox-cache` queries to decide whether a job can be skipped.
//!
//! 3. **Audit trail** — an append-only history of every job execution,
//!    turning `.oxymake/state.db` into a lightweight research lab
//!    notebook (see OXYMAKE-THESIS.md §6.10).
//!
//! # Why `unchecked_transaction`
//!
//! Several methods use [`Connection::unchecked_transaction`] instead of
//! [`Connection::transaction`].  The checked variant borrows `&mut Connection`,
//! which conflicts with our design: `StateDb` hands out `&Connection` via
//! `StateDb::conn()` and all public methods take `&self`.  Because every
//! database handle is used by a single thread (no concurrent writers),
//! the runtime nesting guard that `transaction()` provides is unnecessary —
//! callers already ensure non-overlapping transactions structurally.
//! `unchecked_transaction` gives us the same ACID semantics with a `&self`
//! receiver, avoiding an `RefCell` / `Mutex` wrapper that would add overhead
//! with no safety benefit for internal-only callers.
//!
//! # Opening a database
//!
//! ```no_run
//! use std::path::Path;
//! use ox_state::db::StateDb;
//!
//! let db = StateDb::open(Path::new(".oxymake/state.db")).unwrap();
//! let version = db.schema_version().unwrap();
//! assert_eq!(version, 2);
//! ```

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use ox_core::model::ContentHash;
use rusqlite::Connection;

use crate::backend::StateBackend;
use crate::error::StateError;
use crate::migration;
use crate::session::SessionInfo;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A record describing a job to be registered in the database.
///
/// ```
/// use ox_state::db::JobRecord;
///
/// let rec = JobRecord {
///     id: "align-sample_A".into(),
///     rule_name: "align".into(),
///     wildcards: "{}".into(),
///     cache_key: None,
///     run_id: None,
/// };
/// assert_eq!(rec.rule_name, "align");
/// ```
pub struct JobRecord {
    /// Unique job identifier (matches [`ox_core::model::JobId`]).
    pub id: String,
    /// Name of the rule that produced this job.
    pub rule_name: String,
    /// JSON-encoded wildcard bindings.
    pub wildcards: String,
    /// Optional cache key for skip decisions.
    pub cache_key: Option<String>,
    /// Optional run ID — associates this job with a specific `ox run`.
    pub run_id: Option<String>,
}

/// Aggregated job counts by status — used for progress display.
///
/// ```
/// use ox_state::db::JobCounts;
///
/// let counts = JobCounts {
///     pending: 10,
///     running: 2,
///     completed: 5,
///     failed: 1,
///     skipped: 0,
///     cached: 3,
///     cancelled: 0,
/// };
/// assert_eq!(counts.pending + counts.running + counts.completed
///            + counts.failed + counts.skipped + counts.cached + counts.cancelled, 21);
/// ```
pub struct JobCounts {
    /// Jobs waiting to be claimed.
    pub pending: usize,
    /// Jobs currently executing.
    pub running: usize,
    /// Jobs that finished successfully (includes cached jobs).
    pub completed: usize,
    /// Jobs that finished with an error.
    pub failed: usize,
    /// Jobs that were skipped (non-cache reasons, e.g. guard failure).
    pub skipped: usize,
    /// Jobs that were completed via cache hit (subset tracked separately).
    pub cached: usize,
    /// Jobs that were cancelled.
    pub cancelled: usize,
}

/// A single entry in the append-only job execution audit trail.
///
/// ```
/// use ox_state::db::JobHistoryEntry;
///
/// let entry = JobHistoryEntry {
///     run_id: "run-1234".into(),
///     job_id: "align-sample_A".into(),
///     rule_name: "align".into(),
///     wildcards: Some(r#"{"sample":"A"}"#.into()),
///     input_hashes: None,
///     output_hashes: None,
///     params_hash: None,
///     env_hash: None,
///     executor: Some("local".into()),
///     hostname: Some("build-host".into()),
///     started_at: Some(1700000000),
///     completed_at: Some(1700000042),
///     wall_time_ms: Some(42000),
///     peak_mem_mb: Some(128),
///     exit_code: Some(0),
///     reproducibility_class: Some("deterministic".into()),
///     artifact_provenance_json: None,
/// };
/// assert_eq!(entry.rule_name, "align");
/// ```
pub struct JobHistoryEntry {
    /// ID of the run this execution belongs to.
    pub run_id: String,
    /// Job identifier.
    pub job_id: String,
    /// Rule that produced the job.
    pub rule_name: String,
    /// JSON-encoded wildcard bindings.
    pub wildcards: Option<String>,
    /// JSON-encoded input content hashes.
    pub input_hashes: Option<String>,
    /// JSON-encoded output content hashes.
    pub output_hashes: Option<String>,
    /// Hash of the job's parameters.
    pub params_hash: Option<String>,
    /// Hash of the job's environment spec.
    pub env_hash: Option<String>,
    /// Executor backend used (e.g., `"local"`).
    pub executor: Option<String>,
    /// Hostname where the job ran.
    pub hostname: Option<String>,
    /// UNIX timestamp (seconds) when execution started.
    pub started_at: Option<u64>,
    /// UNIX timestamp (seconds) when execution completed.
    pub completed_at: Option<u64>,
    /// Wall-clock duration in milliseconds.
    pub wall_time_ms: Option<u64>,
    /// Peak memory usage in megabytes.
    pub peak_mem_mb: Option<u64>,
    /// Process exit code.
    pub exit_code: Option<i32>,
    /// Reproducibility classification from the rule (e.g., "deterministic").
    pub reproducibility_class: Option<String>,
    /// JSON-encoded `ArtifactProvenance` (defined in `ox_core::model`) for cache correctness auditing.
    pub artifact_provenance_json: Option<String>,
}

/// Per-rule duration aggregate from a single run's job history.
#[derive(Debug, Clone)]
pub struct RuleDurationStat {
    /// Rule name.
    pub rule_name: String,
    /// Total wall-clock time across all jobs for this rule (milliseconds).
    pub total_ms: u64,
    /// Maximum wall-clock time among jobs for this rule (milliseconds).
    pub max_ms: u64,
    /// Number of jobs for this rule.
    pub job_count: u64,
}

/// A record from the `runs` table — one per `ox run` invocation.
///
/// ```
/// use ox_core::model::ContentHash;
/// use ox_state::db::RunRecord;
///
/// let run = RunRecord {
///     id: "run-1234".into(),
///     started_at: 1700000000,
///     completed_at: Some(1700000060),
///     note: Some("nightly build".into()),
///     workflow_hash: None,
///     job_count: Some(10),
///     succeeded: Some(8),
///     failed: Some(1),
///     skipped: Some(1),
/// };
/// assert_eq!(run.id, "run-1234");
/// ```
pub struct RunRecord {
    /// Unique run identifier.
    pub id: String,
    /// UNIX timestamp (seconds) when the run started.
    pub started_at: u64,
    /// UNIX timestamp (seconds) when the run completed (if finished).
    pub completed_at: Option<u64>,
    /// Optional annotation for the run.
    pub note: Option<String>,
    /// Hash of the workflow file at run time.
    pub workflow_hash: Option<ContentHash>,
    /// Total number of jobs in the run.
    pub job_count: Option<i64>,
    /// Number of jobs that succeeded.
    pub succeeded: Option<i64>,
    /// Number of jobs that failed.
    pub failed: Option<i64>,
    /// Number of jobs that were skipped.
    pub skipped: Option<i64>,
}

/// A named snapshot of workflow state at a point in time.
pub struct SnapshotRecord {
    /// User-provided snapshot name.
    pub name: String,
    /// UNIX timestamp (seconds) when the snapshot was created.
    pub created_at: u64,
    /// Hash of the Oxymakefile at snapshot time.
    pub workflow_hash: Option<ContentHash>,
    /// Optional description.
    pub description: Option<String>,
    /// Number of jobs captured in this snapshot.
    pub job_count: usize,
}

/// A job's state within a snapshot.
pub struct SnapshotJobEntry {
    /// Job identifier.
    pub job_id: String,
    /// Rule that produced the job.
    pub rule_name: String,
    /// Job status at snapshot time.
    pub status: String,
    /// Optional cache key.
    pub cache_key: Option<String>,
    /// Optional output hashes (JSON).
    pub output_hashes: Option<String>,
}

/// Detail of a currently running job.
pub struct RunningJobDetail {
    /// Job identifier.
    pub id: String,
    /// Rule that produced the job.
    pub rule_name: String,
    /// JSON-encoded wildcard bindings.
    pub wildcards: String,
    /// UNIX timestamp (seconds) when execution started.
    pub started_at: Option<u64>,
}

/// Detail of a pending job with its unsatisfied (blocking) upstream dependencies.
pub struct PendingJobDetail {
    /// Job identifier.
    pub id: String,
    /// Rule that produced the job.
    pub rule_name: String,
    /// JSON-encoded wildcard bindings.
    pub wildcards: String,
    /// Rule names of upstream jobs that are not yet completed/skipped.
    pub waiting_for: Vec<String>,
}

/// Full detail of a job (any status).
pub struct AllJobDetail {
    /// Job identifier.
    pub id: String,
    /// Rule that produced the job.
    pub rule_name: String,
    /// Current status string.
    pub status: String,
    /// JSON-encoded wildcard bindings.
    pub wildcards: String,
    /// UNIX timestamp (seconds) when execution started.
    pub started_at: Option<u64>,
    /// UNIX timestamp (seconds) when execution completed.
    pub completed_at: Option<u64>,
    /// Process exit code.
    pub exit_code: Option<i32>,
}

/// Per-rule aggregate counts for pipeline display.
pub struct PipelineStats {
    /// Rule name.
    pub rule_name: String,
    /// Number of completed jobs (includes failed and skipped).
    pub completed: usize,
    /// Total number of jobs.
    pub total: usize,
    /// Number of currently running jobs.
    pub running: usize,
}

/// Per-rule aggregate counts with timing data for progress/ETA display.
pub struct PipelineStatsWithTiming {
    /// Rule name.
    pub rule_name: String,
    /// Number of completed jobs (includes failed and skipped).
    pub completed: usize,
    /// Total number of jobs.
    pub total: usize,
    /// Number of currently running jobs.
    pub running: usize,
    /// Number of pending jobs.
    pub pending: usize,
    /// Number of failed jobs.
    pub failed: usize,
    /// Average wall-clock duration in milliseconds for completed jobs.
    pub avg_wall_time_ms: u64,
    /// UNIX timestamp of the earliest job start for this rule.
    pub earliest_started_at: Option<u64>,
}

/// Log path and status for a specific job.
pub struct JobLogInfo {
    /// Path to the job's log file, if known.
    pub log_path: Option<String>,
    /// Current status string.
    pub status: String,
}

/// A job with its log path and status.
pub struct JobWithLog {
    /// Job identifier.
    pub id: String,
    /// Rule that produced the job.
    pub rule_name: String,
    /// Current status string.
    pub status: String,
    /// Path to the job's log file, if known.
    pub log_path: Option<String>,
}

/// A gate record representing a manual approval point.
pub struct GateRecord {
    /// Auto-incremented gate ID.
    pub id: i64,
    /// Rule that triggered the gate.
    pub rule_name: String,
    /// Job that is waiting for approval.
    pub job_id: String,
    /// Gate status: pending, approved, rejected.
    pub status: String,
    /// UNIX timestamp when the gate was created.
    pub created_at: u64,
    /// UNIX timestamp when the gate was decided.
    pub decided_at: Option<u64>,
    /// Identity of the approver/rejector.
    pub decided_by: Option<String>,
    /// Reason for the decision.
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// StateDb — the Ledger (ADR-011 Stage 3)
// ---------------------------------------------------------------------------

/// SQLite-backed state manager for OxyMake — the **Ledger** (Stage 3 of the
/// three-stage pipeline described in ADR-011).
///
/// Vocabulary note: this type is conceptually the
/// **Ledger** — the persisted audit store at the bottom of the unidirectional
/// `Frontier → EventBus → EventSink → Ledger` pipeline. The Rust symbol
/// retains the name `StateDb` for source-level backward compatibility ; see
/// the [`Ledger`](crate::Ledger) type alias for the new vocabulary.
///
/// Wraps a single `rusqlite::Connection` with WAL mode enabled for
/// concurrent reader access.  All write operations use transactions
/// to maintain atomicity.
///
/// # Example
///
/// ```
/// # use tempfile::NamedTempFile;
/// use ox_state::db::StateDb;
///
/// let tmp = NamedTempFile::new().unwrap();
/// let db = StateDb::open(tmp.path()).unwrap();
///
/// // Database is at schema version 1 after open.
/// assert_eq!(db.schema_version().unwrap(), 9);
///
/// db.close().unwrap();
/// ```
pub struct StateDb {
    conn: Connection,
}

/// Does this SQLite error indicate a corrupt database file?
///
/// `DatabaseCorrupt` (SQLITE_CORRUPT) is internal corruption of a valid
/// SQLite file; `NotADatabase` (SQLITE_NOTADB) is a file that is not an
/// SQLite database at all (truncated header, garbage, partial NFS write).
/// Both mean state.db is unrecoverable in place and must be regenerated.
fn is_corruption(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _)
            if e.code == rusqlite::ErrorCode::DatabaseCorrupt
                || e.code == rusqlite::ErrorCode::NotADatabase
    )
}

impl StateDb {
    /// Open (or create) the state database at `path`.
    ///
    /// Enables WAL mode for concurrent access and runs any pending
    /// schema migrations.  After this call the database is guaranteed
    /// to be at the latest schema version.
    pub fn open(path: &Path) -> Result<Self, StateError> {
        Self::open_inner(path).map_err(|err| match err {
            // A corrupt database must not bubble as a raw SQLite error:
            // state.db is a regenerable cache, and the user needs to learn
            // the escape hatch (`ox clean --state`). SQLite reports
            // corruption lazily — Connection::open succeeds and the first
            // pragma or statement fails — so the mapping wraps the whole
            // open-and-migrate sequence.
            StateError::Db(db_err) if is_corruption(&db_err) => StateError::Corrupt {
                path: path.display().to_string(),
                source: db_err,
            },
            other => other,
        })
    }

    fn open_inner(path: &Path) -> Result<Self, StateError> {
        let conn = Connection::open(path)?;

        // busy_timeout=30s: when another session holds the write lock,
        // wait instead of failing immediately with SQLITE_BUSY. Without
        // this, two concurrent `ox run` on a shared workspace (HPC
        // cluster, two terminals, parallel CI) break the daemon-free
        // "atomic SQLite claims" concurrency model on first contention.
        conn.execute_batch("PRAGMA busy_timeout=30000;")?;
        // WAL mode allows concurrent readers and a single writer —
        // essential for the cooperative multi-session model.
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        // synchronous=FULL: fsync on every commit AND on every WAL checkpoint.
        // The SQLite default with WAL is NORMAL, which skips the per-commit
        // fsync — a power loss or kernel crash between commit and the next
        // checkpoint can silently lose the most recent transactions even
        // though SQLite reported success. The Ledger is durable by contract
        // (ADR-011 Stage 3 + audit-trail role), so we accept the 2-3× write
        // latency cost; job latency is dominated by execution, not DB writes.
        conn.execute_batch("PRAGMA synchronous=FULL;")?;

        migration::migrate(&conn)?;

        Ok(Self { conn })
    }

    /// Return the current schema version.
    pub fn schema_version(&self) -> Result<u32, StateError> {
        let version: u32 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )?;
        Ok(version)
    }

    /// Close the database cleanly, consuming `self`.
    ///
    /// This is a deliberate close — calling `drop` also works but
    /// swallows any error from the underlying connection.
    pub fn close(self) -> Result<(), StateError> {
        self.conn.close().map_err(|(_conn, e)| StateError::Db(e))
    }

    // -------------------------------------------------------------------
    // Job operations
    // -------------------------------------------------------------------

    /// Register a batch of jobs with status `pending`.
    ///
    /// Existing jobs with the same ID are silently skipped (`INSERT OR
    /// IGNORE`) so that re-running `ox run` after a crash does not
    /// duplicate entries.
    pub fn register_jobs(&self, jobs: &[JobRecord]) -> Result<(), StateError> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO jobs (id, rule_name, wildcards, status, cache_key, run_id)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET run_id = excluded.run_id",
            )?;
            for job in jobs {
                stmt.execute(rusqlite::params![
                    job.id,
                    job.rule_name,
                    job.wildcards,
                    job.cache_key,
                    job.run_id,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Atomically claim a job for execution.
    ///
    /// Returns `true` if the calling session successfully claimed the
    /// job, `false` if it was already claimed by another session.
    /// This is the core of cooperative multi-session execution:
    ///
    /// ```sql
    /// UPDATE jobs SET status='running', session_id=?, locked_by=?
    /// WHERE id=? AND status='pending'
    /// ```
    ///
    /// Because SQLite serialises writers, only one session can win.
    pub fn claim_job(&self, job_id: &str, session_id: &str) -> Result<bool, StateError> {
        let now = unix_now();
        let rows = self.conn.execute(
            "UPDATE jobs SET status = 'running', session_id = ?1, locked_by = ?1,
                             started_at = ?2
             WHERE id = ?3 AND status = 'pending'",
            rusqlite::params![session_id, now, job_id],
        )?;
        Ok(rows > 0)
    }

    /// Mark a job as completed with its exit code and output hashes.
    ///
    /// Only transitions from `running` → `completed`, and only when the
    /// job is still claimed by `session_id`.  Returns `true` if the
    /// transition happened, `false` if the job was not in `running`
    /// state, was claimed by another session, or does not exist.
    ///
    /// The session filter is the zombie guard (H16): a session whose
    /// claim was reclaimed after a stale heartbeat must not terminalize
    /// the job once another session has re-claimed it — its output
    /// hashes would silently overwrite the new claim holder's run.
    pub fn complete_job(
        &self,
        job_id: &str,
        session_id: &str,
        exit_code: i32,
        output_hashes: &str,
    ) -> Result<bool, StateError> {
        let now = unix_now();
        let rows = self.conn.execute(
            "UPDATE jobs SET status = 'completed', exit_code = ?1,
                             output_hashes = ?2, completed_at = ?3
             WHERE id = ?4 AND status = 'running' AND session_id = ?5",
            rusqlite::params![exit_code, output_hashes, now, job_id, session_id],
        )?;
        Ok(rows > 0)
    }

    /// Mark a job as failed with its exit code.
    ///
    /// Only transitions from `running` → `failed`, and only when the
    /// job is still claimed by `session_id` (same zombie guard as
    /// [`StateDb::complete_job`]).  Returns `true` if the transition
    /// happened.
    pub fn fail_job(
        &self,
        job_id: &str,
        session_id: &str,
        exit_code: i32,
    ) -> Result<bool, StateError> {
        let now = unix_now();
        let rows = self.conn.execute(
            "UPDATE jobs SET status = 'failed', exit_code = ?1, completed_at = ?2
             WHERE id = ?3 AND status = 'running' AND session_id = ?4",
            rusqlite::params![exit_code, now, job_id, session_id],
        )?;
        Ok(rows > 0)
    }

    /// Mark a job as completed regardless of which session claimed it.
    ///
    /// Reconciliation variant of [`StateDb::complete_job`] for external
    /// executors (SLURM sacct / results.json sync): the scheduler is the
    /// authority on terminal state there, and the syncing process is not
    /// the session that claimed the job.  Never use this from a live
    /// execution path — it bypasses the zombie guard on purpose.
    pub fn reconcile_complete_job(
        &self,
        job_id: &str,
        exit_code: i32,
        output_hashes: &str,
    ) -> Result<bool, StateError> {
        let now = unix_now();
        let rows = self.conn.execute(
            "UPDATE jobs SET status = 'completed', exit_code = ?1,
                             output_hashes = ?2, completed_at = ?3
             WHERE id = ?4 AND status = 'running'",
            rusqlite::params![exit_code, output_hashes, now, job_id],
        )?;
        Ok(rows > 0)
    }

    /// Mark a job as failed regardless of which session claimed it.
    ///
    /// Reconciliation variant of [`StateDb::fail_job`] — see
    /// [`StateDb::reconcile_complete_job`] for when this is legitimate.
    pub fn reconcile_fail_job(&self, job_id: &str, exit_code: i32) -> Result<bool, StateError> {
        let now = unix_now();
        let rows = self.conn.execute(
            "UPDATE jobs SET status = 'failed', exit_code = ?1, completed_at = ?2
             WHERE id = ?3 AND status = 'running'",
            rusqlite::params![exit_code, now, job_id],
        )?;
        Ok(rows > 0)
    }

    /// Mark a job as completed via cache hit (no execution needed).
    ///
    /// Only transitions from `pending` → `completed` with `cached = 1`.
    /// Returns `true` if the transition happened, `false` if the job was
    /// not in `pending` state (or does not exist).
    pub fn skip_job(&self, job_id: &str) -> Result<bool, StateError> {
        let now = unix_now();
        let rows = self.conn.execute(
            "UPDATE jobs SET status = 'completed', cached = 1, completed_at = ?1
             WHERE id = ?2 AND status = 'pending'",
            rusqlite::params![now, job_id],
        )?;
        Ok(rows > 0)
    }

    /// Cancel running and pending jobs, optionally filtered by rule name
    /// and/or session ID.
    ///
    /// Returns the IDs of cancelled jobs.  Only jobs with status `running`
    /// or `pending` are affected — completed, failed, and skipped jobs are
    /// left unchanged.
    pub fn cancel_jobs(
        &self,
        rule: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Vec<String>, StateError> {
        // First collect matching job IDs, then update them in a transaction.
        let cancellable = self.cancellable_job_ids(rule, session_id)?;
        if cancellable.is_empty() {
            return Ok(Vec::new());
        }

        let now = unix_now();
        let tx = self.conn.unchecked_transaction()?;
        for id in &cancellable {
            tx.execute(
                "UPDATE jobs SET status = 'cancelled', completed_at = ?1
                 WHERE id = ?2 AND status IN ('running', 'pending')",
                rusqlite::params![now, id],
            )?;
        }
        tx.commit()?;
        Ok(cancellable)
    }

    /// Cancel specific jobs by their IDs.
    ///
    /// Only jobs with status `running` or `pending` are affected.
    /// Returns the IDs that were actually cancelled.
    pub fn cancel_job_ids(&self, ids: &[String]) -> Result<Vec<String>, StateError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let now = unix_now();
        let mut cancelled = Vec::new();
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            let affected = tx.execute(
                "UPDATE jobs SET status = 'cancelled', completed_at = ?1
                 WHERE id = ?2 AND status IN ('running', 'pending')",
                rusqlite::params![now, id],
            )?;
            if affected > 0 {
                cancelled.push(id.clone());
            }
        }
        tx.commit()?;
        Ok(cancelled)
    }

    /// Return IDs of running/pending jobs matching the given filters.
    fn cancellable_job_ids(
        &self,
        rule: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Vec<String>, StateError> {
        let (sql, params) = match (rule, session_id) {
            (Some(r), Some(s)) => (
                "SELECT id FROM jobs WHERE status IN ('running','pending') AND rule_name = ?1 AND session_id = ?2",
                vec![r.to_string(), s.to_string()],
            ),
            (Some(r), None) => (
                "SELECT id FROM jobs WHERE status IN ('running','pending') AND rule_name = ?1",
                vec![r.to_string()],
            ),
            (None, Some(s)) => (
                "SELECT id FROM jobs WHERE status IN ('running','pending') AND session_id = ?1",
                vec![s.to_string()],
            ),
            (None, None) => (
                "SELECT id FROM jobs WHERE status IN ('running','pending')",
                vec![],
            ),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// Get the current status string for a job, or `None` if not found.
    pub fn job_status(&self, job_id: &str) -> Result<Option<String>, StateError> {
        let mut stmt = self.conn.prepare("SELECT status FROM jobs WHERE id = ?1")?;
        let mut rows = stmt.query(rusqlite::params![job_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// Return (started_at, completed_at) timestamps for a job from the `jobs` table.
    pub fn job_timing(&self, job_id: &str) -> Result<(Option<u64>, Option<u64>), StateError> {
        let mut stmt = self
            .conn
            .prepare("SELECT started_at, completed_at FROM jobs WHERE id = ?1")?;
        let mut rows = stmt.query(rusqlite::params![job_id])?;
        match rows.next()? {
            Some(row) => Ok((row.get(0)?, row.get(1)?)),
            None => Ok((None, None)),
        }
    }

    /// Return the IDs of all jobs with the given status.
    pub fn jobs_by_status(&self, status: &str) -> Result<Vec<String>, StateError> {
        let mut stmt = self.conn.prepare("SELECT id FROM jobs WHERE status = ?1")?;
        let rows = stmt.query_map(rusqlite::params![status], |row| row.get(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// Aggregate job counts by status.
    pub fn job_counts(&self) -> Result<JobCounts, StateError> {
        let mut counts = JobCounts {
            pending: 0,
            running: 0,
            completed: 0,
            failed: 0,
            skipped: 0,
            cached: 0,
            cancelled: 0,
        };
        let mut stmt = self
            .conn
            .prepare("SELECT status, COUNT(*) FROM jobs GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })?;
        for row in rows {
            let (status, count) = row?;
            match status.as_str() {
                "pending" => counts.pending = count,
                "running" => counts.running = count,
                "completed" => counts.completed = count,
                "failed" => counts.failed = count,
                "skipped" => counts.skipped = count,
                "cancelled" => counts.cancelled = count,
                _ => {}
            }
        }
        // Query cached count separately (cached jobs have status='completed').
        counts.cached =
            self.conn
                .query_row("SELECT COUNT(*) FROM jobs WHERE cached = 1", [], |row| {
                    row.get(0)
                })?;
        Ok(counts)
    }

    /// Return the ID of the most recent run, or `None` if no runs exist.
    pub fn latest_run_id(&self) -> Result<Option<String>, StateError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM runs ORDER BY started_at DESC LIMIT 1")?;
        let mut rows = stmt.query([])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// Aggregate job counts by status, scoped to a specific run.
    pub fn job_counts_for_run(&self, run_id: &str) -> Result<JobCounts, StateError> {
        let mut counts = JobCounts {
            pending: 0,
            running: 0,
            completed: 0,
            failed: 0,
            skipped: 0,
            cached: 0,
            cancelled: 0,
        };
        let mut stmt = self
            .conn
            .prepare("SELECT status, COUNT(*) FROM jobs WHERE run_id = ?1 GROUP BY status")?;
        let rows = stmt.query_map(rusqlite::params![run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })?;
        for row in rows {
            let (status, count) = row?;
            match status.as_str() {
                "pending" => counts.pending = count,
                "running" => counts.running = count,
                "completed" => counts.completed = count,
                "failed" => counts.failed = count,
                "skipped" => counts.skipped = count,
                "cancelled" => counts.cancelled = count,
                _ => {}
            }
        }
        // Query cached count separately (cached jobs have status='completed').
        counts.cached = self.conn.query_row(
            "SELECT COUNT(*) FROM jobs WHERE run_id = ?1 AND cached = 1",
            rusqlite::params![run_id],
            |row| row.get(0),
        )?;
        Ok(counts)
    }

    /// Return details of running jobs scoped to a specific run.
    pub fn running_jobs_detail_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<RunningJobDetail>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_name, COALESCE(wildcards, '{}'), started_at
             FROM jobs WHERE status = 'running' AND run_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id], |row| {
            Ok(RunningJobDetail {
                id: row.get(0)?,
                rule_name: row.get(1)?,
                wildcards: row.get(2)?,
                started_at: row.get(3)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Return pending jobs with blockers, scoped to a specific run.
    pub fn pending_jobs_with_blockers_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<PendingJobDetail>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_name, COALESCE(wildcards, '{}')
             FROM jobs WHERE status = 'pending' AND run_id = ?1",
        )?;
        let pending: Vec<(String, String, String)> = stmt
            .query_map(rusqlite::params![run_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut blocker_stmt = self.conn.prepare(
            "SELECT j.rule_name
             FROM job_edges e
             JOIN jobs j ON j.id = e.from_job
             WHERE e.to_job = ?1
               AND j.status NOT IN ('completed', 'skipped')",
        )?;

        let mut result = Vec::new();
        for (id, rule_name, wildcards) in pending {
            let waiting_for: Vec<String> = blocker_stmt
                .query_map(rusqlite::params![&id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            result.push(PendingJobDetail {
                id,
                rule_name,
                wildcards,
                waiting_for,
            });
        }
        Ok(result)
    }

    /// Return per-rule aggregate counts scoped to a specific run.
    pub fn pipeline_stats_for_run(&self, run_id: &str) -> Result<Vec<PipelineStats>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT rule_name, status, COUNT(*) FROM jobs WHERE run_id = ?1 GROUP BY rule_name, status",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, usize>(2)?,
            ))
        })?;

        let mut rules: std::collections::BTreeMap<String, (usize, usize, usize)> =
            std::collections::BTreeMap::new();
        for row in rows {
            let (rule, status, count) = row?;
            let entry = rules.entry(rule).or_insert((0, 0, 0));
            entry.1 += count; // total
            match status.as_str() {
                "completed" | "failed" | "skipped" => entry.0 += count,
                "running" => {
                    entry.2 += count;
                }
                _ => {}
            }
        }
        Ok(rules
            .into_iter()
            .map(|(rule_name, (completed, total, running))| PipelineStats {
                rule_name,
                completed,
                total,
                running,
            })
            .collect())
    }

    /// Return details of running jobs.
    ///
    /// Used by the TUI dashboard to display currently executing jobs.
    pub fn running_jobs_detail(&self) -> Result<Vec<RunningJobDetail>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_name, COALESCE(wildcards, '{}'), started_at
             FROM jobs WHERE status = 'running'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RunningJobDetail {
                id: row.get(0)?,
                rule_name: row.get(1)?,
                wildcards: row.get(2)?,
                started_at: row.get(3)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Return pending jobs with their unsatisfied upstream dependencies.
    ///
    /// For each pending job, includes the rule names of upstream jobs that are
    /// not yet completed or skipped (i.e., the jobs still blocking this one).
    pub fn pending_jobs_with_blockers(&self) -> Result<Vec<PendingJobDetail>, StateError> {
        // First get all pending jobs.
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_name, COALESCE(wildcards, '{}')
             FROM jobs WHERE status = 'pending'",
        )?;
        let pending: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        // For each pending job, find unsatisfied upstream dependencies.
        // An edge (from_job, to_job) means from_job is upstream of to_job.
        // We want upstream jobs of each pending job whose status is NOT
        // completed/skipped.
        let mut blocker_stmt = self.conn.prepare(
            "SELECT j.rule_name
             FROM job_edges e
             JOIN jobs j ON j.id = e.from_job
             WHERE e.to_job = ?1
               AND j.status NOT IN ('completed', 'skipped')",
        )?;

        let mut result = Vec::new();
        for (id, rule_name, wildcards) in pending {
            let waiting_for: Vec<String> = blocker_stmt
                .query_map(rusqlite::params![&id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            result.push(PendingJobDetail {
                id,
                rule_name,
                wildcards,
                waiting_for,
            });
        }
        Ok(result)
    }

    /// Return details of all jobs.
    ///
    /// Used by the dashboard Gantt chart and DAG endpoint.
    pub fn all_jobs_detail(&self) -> Result<Vec<AllJobDetail>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_name, status, COALESCE(wildcards, '{}'),
                    started_at, completed_at, exit_code
             FROM jobs",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AllJobDetail {
                id: row.get(0)?,
                rule_name: row.get(1)?,
                status: row.get(2)?,
                wildcards: row.get(3)?,
                started_at: row.get(4)?,
                completed_at: row.get(5)?,
                exit_code: row.get(6)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Return per-rule aggregate counts for pipeline display.
    pub fn pipeline_stats(&self) -> Result<Vec<PipelineStats>, StateError> {
        let mut stmt = self
            .conn
            .prepare("SELECT rule_name, status, COUNT(*) FROM jobs GROUP BY rule_name, status")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, usize>(2)?,
            ))
        })?;

        // Aggregate per rule
        let mut rules: std::collections::BTreeMap<String, (usize, usize, usize)> =
            std::collections::BTreeMap::new();
        for row in rows {
            let (rule, status, count) = row?;
            let entry = rules.entry(rule).or_insert((0, 0, 0));
            entry.1 += count; // total
            match status.as_str() {
                "completed" | "failed" | "skipped" => entry.0 += count, // completed
                "running" => {
                    entry.2 += count; // running
                }
                _ => {} // pending
            }
        }
        Ok(rules
            .into_iter()
            .map(|(rule_name, (completed, total, running))| PipelineStats {
                rule_name,
                completed,
                total,
                running,
            })
            .collect())
    }

    /// Return per-rule aggregate counts with timing data for progress/ETA display.
    pub fn pipeline_stats_with_timing(&self) -> Result<Vec<PipelineStatsWithTiming>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT rule_name, status, COUNT(*),
                    COALESCE(SUM(CASE WHEN status = 'completed' AND completed_at IS NOT NULL AND started_at IS NOT NULL
                                     THEN completed_at - started_at ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status = 'completed' AND completed_at IS NOT NULL AND started_at IS NOT NULL
                                     THEN 1 ELSE 0 END), 0),
                    MIN(started_at)
             FROM jobs GROUP BY rule_name, status",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, usize>(2)?,
                row.get::<_, u64>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, Option<u64>>(5)?,
            ))
        })?;

        // Aggregate per rule: (completed, total, running, pending, failed,
        //                      sum_wall_sec, count_with_wall, earliest_started)
        #[allow(clippy::type_complexity)]
        let mut rules: std::collections::BTreeMap<
            String,
            (usize, usize, usize, usize, usize, u64, u64, Option<u64>),
        > = std::collections::BTreeMap::new();
        for row in rows {
            let (rule, status, count, sum_wall, count_wall, earliest) = row?;
            let entry = rules.entry(rule).or_insert((0, 0, 0, 0, 0, 0, 0, None));
            entry.1 += count; // total
            entry.5 += sum_wall; // sum_wall_sec
            entry.6 += count_wall; // count_with_wall
            match entry.7 {
                None => entry.7 = earliest,
                Some(existing) => {
                    if let Some(e) = earliest {
                        if e < existing {
                            entry.7 = Some(e);
                        }
                    }
                }
            }
            match status.as_str() {
                "completed" | "skipped" => entry.0 += count,
                "failed" => {
                    entry.0 += count;
                    entry.4 += count;
                }
                "running" => entry.2 += count,
                "pending" => entry.3 += count,
                _ => {}
            }
        }
        Ok(rules
            .into_iter()
            .map(
                |(
                    rule_name,
                    (completed, total, running, pending, failed, sum_wall, count_wall, earliest),
                )| {
                    let avg_ms = sum_wall
                        .checked_mul(1000)
                        .and_then(|v| v.checked_div(count_wall))
                        .unwrap_or(0);
                    PipelineStatsWithTiming {
                        rule_name,
                        completed,
                        total,
                        running,
                        pending,
                        failed,
                        avg_wall_time_ms: avg_ms,
                        earliest_started_at: earliest,
                    }
                },
            )
            .collect())
    }

    // -------------------------------------------------------------------
    // Run & job-history (audit trail)
    // -------------------------------------------------------------------

    /// Begin a new run and return its unique ID.
    ///
    /// A **run** groups all jobs executed by a single `ox run` invocation.
    /// The run is "open" until [`end_run`](Self::end_run) is called.
    pub fn begin_run(
        &self,
        run_id: &str,
        workflow_hash: Option<&ContentHash>,
        job_count: usize,
        note: Option<&str>,
    ) -> Result<(), StateError> {
        let now = unix_now();
        self.conn.execute(
            "INSERT INTO runs (id, started_at, workflow_hash, job_count, note)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                run_id,
                now,
                workflow_hash.map(ContentHash::as_str),
                job_count as i64,
                note
            ],
        )?;
        Ok(())
    }

    /// Finalise an open run with aggregate counts.
    pub fn end_run(
        &self,
        run_id: &str,
        succeeded: usize,
        failed: usize,
        skipped: usize,
    ) -> Result<(), StateError> {
        let now = unix_now();
        self.conn.execute(
            "UPDATE runs SET completed_at = ?1, succeeded = ?2, failed = ?3, skipped = ?4
             WHERE id = ?5",
            rusqlite::params![now, succeeded as i64, failed as i64, skipped as i64, run_id],
        )?;
        Ok(())
    }

    /// Record a single job execution in the append-only audit trail.
    pub fn record_job_history(&self, entry: &JobHistoryEntry) -> Result<(), StateError> {
        self.conn.execute(
            "INSERT INTO job_history
                (run_id, job_id, rule_name, wildcards, input_hashes, output_hashes,
                 params_hash, env_hash, executor, hostname,
                 started_at, completed_at, wall_time_ms, peak_mem_mb, exit_code,
                 reproducibility_class, artifact_provenance_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            rusqlite::params![
                entry.run_id,
                entry.job_id,
                entry.rule_name,
                entry.wildcards,
                entry.input_hashes,
                entry.output_hashes,
                entry.params_hash,
                entry.env_hash,
                entry.executor,
                entry.hostname,
                entry.started_at,
                entry.completed_at,
                entry.wall_time_ms,
                entry.peak_mem_mb,
                entry.exit_code,
                entry.reproducibility_class,
                entry.artifact_provenance_json,
            ],
        )?;
        Ok(())
    }

    /// Finalize audit-trail history from the post-flush jobs table.
    ///
    /// Reads all jobs for `run_id` that reached a terminal state
    /// (completed, failed, skipped) and inserts a `job_history` row for
    /// each.  This replaces the former in-memory finalization loop: after
    /// the EventSink (ADR-011 Stage 2 → Stage 3 transition, ex-"state-db
    /// bridge") has been awaited, the jobs table is authoritative and we
    /// can read it directly (hq-9in00).
    ///
    /// `wall_times` maps `job_id → duration_ms` collected from
    /// `Event::JobCompleted` events.  `executor` and `hostname` are
    /// constant for the whole run.
    pub fn finalize_job_history(
        &self,
        run_id: &str,
        executor: &str,
        hostname: &str,
        wall_times: &std::collections::HashMap<String, u64>,
    ) -> Result<usize, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_name, wildcards, status, started_at, completed_at, exit_code
             FROM jobs
             WHERE run_id = ?1 AND status IN ('completed', 'failed', 'skipped')",
        )?;
        let entries: Vec<JobHistoryEntry> = stmt
            .query_map(rusqlite::params![run_id], |row| {
                let job_id: String = row.get(0)?;
                let status: String = row.get(3)?;
                let stored_exit: Option<i32> = row.get(6)?;
                let exit_code = stored_exit.or(match status.as_str() {
                    "completed" => Some(0),
                    "failed" => Some(1),
                    _ => None,
                });
                let wall_time_ms = wall_times.get(job_id.as_str()).copied();
                Ok(JobHistoryEntry {
                    run_id: run_id.to_string(),
                    job_id,
                    rule_name: row.get(1)?,
                    wildcards: row.get(2)?,
                    input_hashes: None,
                    output_hashes: None,
                    params_hash: None,
                    env_hash: None,
                    executor: Some(executor.to_string()),
                    hostname: Some(hostname.to_string()),
                    started_at: row.get(4)?,
                    completed_at: row.get(5)?,
                    wall_time_ms,
                    peak_mem_mb: None,
                    exit_code,
                    reproducibility_class: None,
                    artifact_provenance_json: None,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let count = entries.len();
        for entry in &entries {
            self.record_job_history(entry)?;
        }
        Ok(count)
    }

    /// Query the job history for a given run.
    pub fn job_history_for_run(&self, run_id: &str) -> Result<Vec<JobHistoryEntry>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, job_id, rule_name, wildcards, input_hashes, output_hashes,
                    params_hash, env_hash, executor, hostname,
                    started_at, completed_at, wall_time_ms, peak_mem_mb, exit_code,
                    reproducibility_class, artifact_provenance_json
             FROM job_history WHERE run_id = ?1
             ORDER BY id",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id], |row| {
            Ok(JobHistoryEntry {
                run_id: row.get(0)?,
                job_id: row.get(1)?,
                rule_name: row.get(2)?,
                wildcards: row.get(3)?,
                input_hashes: row.get(4)?,
                output_hashes: row.get(5)?,
                params_hash: row.get(6)?,
                env_hash: row.get(7)?,
                executor: row.get(8)?,
                hostname: row.get(9)?,
                started_at: row.get(10)?,
                completed_at: row.get(11)?,
                wall_time_ms: row.get(12)?,
                peak_mem_mb: row.get(13)?,
                exit_code: row.get(14)?,
                reproducibility_class: row.get(15)?,
                artifact_provenance_json: row.get(16)?,
            })
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// Return per-rule duration statistics for a run, ordered by total wall time descending.
    ///
    /// Only includes rules that have at least one job with a recorded `wall_time_ms`.
    /// Use `limit` to restrict to the N slowest rules.
    pub fn slowest_rules_for_run(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<RuleDurationStat>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT rule_name,
                    SUM(wall_time_ms) AS total_ms,
                    MAX(wall_time_ms) AS max_ms,
                    COUNT(*) AS job_count
             FROM job_history
             WHERE run_id = ?1 AND wall_time_ms IS NOT NULL
             GROUP BY rule_name
             ORDER BY total_ms DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id, limit as i64], |row| {
            Ok(RuleDurationStat {
                rule_name: row.get(0)?,
                total_ms: row.get(1)?,
                max_ms: row.get(2)?,
                job_count: row.get(3)?,
            })
        })?;
        let mut stats = Vec::new();
        for row in rows {
            stats.push(row?);
        }
        Ok(stats)
    }

    /// List all runs, most recent first.
    pub fn list_runs(&self) -> Result<Vec<RunRecord>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, started_at, completed_at, note, workflow_hash,
                    job_count, succeeded, failed, skipped
             FROM runs ORDER BY started_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RunRecord {
                id: row.get(0)?,
                started_at: row.get(1)?,
                completed_at: row.get(2)?,
                note: row.get(3)?,
                workflow_hash: row
                    .get::<_, Option<String>>(4)?
                    .and_then(|h| ContentHash::from_hex(h).ok()),
                job_count: row.get(5)?,
                succeeded: row.get(6)?,
                failed: row.get(7)?,
                skipped: row.get(8)?,
            })
        })?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    /// Return the log_path and status for a specific job, or `None` if not found.
    pub fn job_log_info(&self, job_id: &str) -> Result<Option<JobLogInfo>, StateError> {
        let mut stmt = self
            .conn
            .prepare("SELECT log_path, status FROM jobs WHERE id = ?1")?;
        let mut rows = stmt.query(rusqlite::params![job_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(JobLogInfo {
                log_path: row.get(0)?,
                status: row.get(1)?,
            })),
            None => Ok(None),
        }
    }

    /// Return all jobs matching optional filters, with their log_path and status.
    pub fn jobs_with_logs(
        &self,
        rule: Option<&str>,
        failed_only: bool,
    ) -> Result<Vec<JobWithLog>, StateError> {
        let mut result = Vec::new();

        let extract = |row: &rusqlite::Row| -> rusqlite::Result<JobWithLog> {
            Ok(JobWithLog {
                id: row.get(0)?,
                rule_name: row.get(1)?,
                status: row.get(2)?,
                log_path: row.get(3)?,
            })
        };

        match (rule, failed_only) {
            (Some(r), true) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, rule_name, status, log_path FROM jobs \
                     WHERE rule_name = ?1 AND status = 'failed' ORDER BY id",
                )?;
                let rows = stmt.query_map(rusqlite::params![r], extract)?;
                for row in rows {
                    result.push(row?);
                }
            }
            (Some(r), false) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, rule_name, status, log_path FROM jobs \
                     WHERE rule_name = ?1 ORDER BY id",
                )?;
                let rows = stmt.query_map(rusqlite::params![r], extract)?;
                for row in rows {
                    result.push(row?);
                }
            }
            (None, true) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, rule_name, status, log_path FROM jobs \
                     WHERE status = 'failed' ORDER BY id",
                )?;
                let rows = stmt.query_map([], extract)?;
                for row in rows {
                    result.push(row?);
                }
            }
            (None, false) => {
                let mut stmt = self
                    .conn
                    .prepare("SELECT id, rule_name, status, log_path FROM jobs ORDER BY id")?;
                let rows = stmt.query_map([], extract)?;
                for row in rows {
                    result.push(row?);
                }
            }
        }

        Ok(result)
    }

    /// Clear ephemeral execution state (jobs and sessions tables) while
    /// preserving the audit trail (runs and job_history).
    ///
    /// This is used by `ox clean` to reset the build state without losing
    /// historical records.
    pub fn clear_execution_state(&self) -> Result<(), StateError> {
        self.conn.execute_batch(
            "BEGIN;
             DELETE FROM jobs;
             DELETE FROM sessions;
             COMMIT;",
        )?;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Snapshot operations
    // -------------------------------------------------------------------

    /// Create a named snapshot of current workflow state.
    ///
    /// Captures all jobs and their statuses at the current moment.
    /// Returns an error if a snapshot with the same name already exists.
    pub fn create_snapshot(
        &self,
        name: &str,
        workflow_hash: Option<&ContentHash>,
        description: Option<&str>,
    ) -> Result<(), StateError> {
        let now = unix_now();
        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "INSERT INTO snapshots (name, created_at, workflow_hash, description)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                name,
                now,
                workflow_hash.map(ContentHash::as_str),
                description
            ],
        )?;

        tx.execute(
            "INSERT INTO snapshot_jobs (snapshot_name, job_id, rule_name, status, cache_key, output_hashes)
             SELECT ?1, id, rule_name, status, cache_key, output_hashes FROM jobs",
            rusqlite::params![name],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// List all snapshots, most recent first.
    pub fn list_snapshots(&self) -> Result<Vec<SnapshotRecord>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT s.name, s.created_at, s.workflow_hash, s.description,
                    (SELECT COUNT(*) FROM snapshot_jobs WHERE snapshot_name = s.name)
             FROM snapshots s
             ORDER BY s.created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SnapshotRecord {
                name: row.get(0)?,
                created_at: row.get(1)?,
                workflow_hash: row
                    .get::<_, Option<String>>(2)?
                    .and_then(|h| ContentHash::from_hex(h).ok()),
                description: row.get(3)?,
                job_count: row.get::<_, usize>(4)?,
            })
        })?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    /// Get the jobs captured in a snapshot.
    pub fn snapshot_jobs(&self, name: &str) -> Result<Vec<SnapshotJobEntry>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT job_id, rule_name, status, cache_key, output_hashes
             FROM snapshot_jobs WHERE snapshot_name = ?1
             ORDER BY job_id",
        )?;
        let rows = stmt.query_map(rusqlite::params![name], |row| {
            Ok(SnapshotJobEntry {
                job_id: row.get(0)?,
                rule_name: row.get(1)?,
                status: row.get(2)?,
                cache_key: row.get(3)?,
                output_hashes: row.get(4)?,
            })
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// Get a single snapshot by name.
    pub fn get_snapshot(&self, name: &str) -> Result<Option<SnapshotRecord>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT s.name, s.created_at, s.workflow_hash, s.description,
                    (SELECT COUNT(*) FROM snapshot_jobs WHERE snapshot_name = s.name)
             FROM snapshots s WHERE s.name = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![name])?;
        match rows.next()? {
            Some(row) => Ok(Some(SnapshotRecord {
                name: row.get(0)?,
                created_at: row.get(1)?,
                workflow_hash: row
                    .get::<_, Option<String>>(2)?
                    .and_then(|h| ContentHash::from_hex(h).ok()),
                description: row.get(3)?,
                job_count: row.get::<_, usize>(4)?,
            })),
            None => Ok(None),
        }
    }

    /// Get the current live jobs in the same format as `snapshot_jobs`.
    ///
    /// This allows comparing a snapshot against the current state.
    pub fn current_jobs(&self) -> Result<Vec<SnapshotJobEntry>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_name, status, cache_key, output_hashes
             FROM jobs ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SnapshotJobEntry {
                job_id: row.get(0)?,
                rule_name: row.get(1)?,
                status: row.get(2)?,
                cache_key: row.get(3)?,
                output_hashes: row.get(4)?,
            })
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// Count the current live jobs.
    pub fn current_job_count(&self) -> Result<usize, StateError> {
        let count: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Delete a snapshot and its associated job entries.
    pub fn delete_snapshot(&self, name: &str) -> Result<bool, StateError> {
        let deleted = self.conn.execute(
            "DELETE FROM snapshots WHERE name = ?1",
            rusqlite::params![name],
        )?;
        Ok(deleted > 0)
    }

    /// Provide read access to the underlying connection.
    ///
    /// Visibility is `pub(crate)` by design: external consumers interact
    /// through typed methods on [`StateDb`], while crate-internal modules
    /// (e.g. `session`) need the raw [`Connection`] for ad-hoc queries and
    /// [`Connection::unchecked_transaction`] calls.  Keeping this accessor
    /// crate-private prevents downstream code from depending on the SQLite
    /// schema, preserving our freedom to evolve it without breaking changes.
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    // -----------------------------------------------------------------
    // Gate operations
    // -----------------------------------------------------------------

    /// Create a new pending gate for a job.
    pub fn create_gate(&self, rule_name: &str, job_id: &str) -> Result<i64, StateError> {
        let now = unix_now();
        self.conn.execute(
            "INSERT INTO gates (rule_name, job_id, status, created_at) VALUES (?1, ?2, 'pending', ?3)",
            rusqlite::params![rule_name, job_id, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// List all gates, optionally filtered by status.
    pub fn list_gates(&self) -> Result<Vec<GateRecord>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_name, job_id, status, created_at, decided_at, decided_by, reason FROM gates ORDER BY id",
        )?;
        let gates = stmt
            .query_map([], |row| {
                Ok(GateRecord {
                    id: row.get(0)?,
                    rule_name: row.get(1)?,
                    job_id: row.get(2)?,
                    status: row.get(3)?,
                    created_at: row.get(4)?,
                    decided_at: row.get(5)?,
                    decided_by: row.get(6)?,
                    reason: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(gates)
    }

    /// Approve a pending gate.
    pub fn approve_gate(
        &self,
        gate_id: i64,
        approver: &str,
        reason: &str,
    ) -> Result<(), StateError> {
        let now = unix_now();
        let updated = self.conn.execute(
            "UPDATE gates SET status = 'approved', decided_at = ?1, decided_by = ?2, reason = ?3 WHERE id = ?4 AND status = 'pending'",
            rusqlite::params![now, approver, reason, gate_id],
        )?;
        if updated == 0 {
            return Err(StateError::Db(rusqlite::Error::QueryReturnedNoRows));
        }
        Ok(())
    }

    /// Reject a pending gate.
    pub fn reject_gate(
        &self,
        gate_id: i64,
        approver: &str,
        reason: &str,
    ) -> Result<(), StateError> {
        let now = unix_now();
        let updated = self.conn.execute(
            "UPDATE gates SET status = 'rejected', decided_at = ?1, decided_by = ?2, reason = ?3 WHERE id = ?4 AND status = 'pending'",
            rusqlite::params![now, approver, reason, gate_id],
        )?;
        if updated == 0 {
            return Err(StateError::Db(rusqlite::Error::QueryReturnedNoRows));
        }
        Ok(())
    }

    // -------------------------------------------------------------------
    // Job edges (DAG visualization)
    // -------------------------------------------------------------------

    /// Register job-to-job dependency edges for DAG visualization.
    ///
    /// Each tuple is `(from_job, to_job)` where `from_job` is upstream of
    /// `to_job`. Existing edges are silently skipped via `INSERT OR IGNORE`.
    pub fn register_edges(&self, edges: &[(String, String)]) -> Result<(), StateError> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO job_edges (from_job, to_job, edge_type)
                 VALUES (?1, ?2, 'dependency')",
            )?;
            for (from, to) in edges {
                stmt.execute(rusqlite::params![from, to])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Record a DAG-level submission to a remote executor.
    pub fn record_dag_submission(
        &self,
        run_id: &str,
        executor: &str,
        dashboard_address: Option<&str>,
        total_jobs: usize,
    ) -> Result<(), StateError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.conn.execute(
            "INSERT OR REPLACE INTO dag_submissions (run_id, executor, dashboard_address, total_jobs, submitted_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![run_id, executor, dashboard_address, total_jobs as i64, now as i64],
        )?;
        Ok(())
    }

    /// Set the executor-specific submission ID for a job (e.g., Ray submission ID).
    pub fn set_executor_submission_id(
        &self,
        job_id: &str,
        submission_id: &str,
    ) -> Result<(), StateError> {
        self.conn.execute(
            "UPDATE jobs SET executor_submission_id = ?1 WHERE id = ?2",
            rusqlite::params![submission_id, job_id],
        )?;
        Ok(())
    }

    /// Query the most recent DAG submission. Returns `(run_id, executor, dashboard_address)`.
    pub fn latest_dag_submission(
        &self,
    ) -> Result<Option<(String, String, Option<String>)>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, executor, dashboard_address FROM dag_submissions ORDER BY submitted_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        match rows.next() {
            Some(Ok(row)) => Ok(Some(row)),
            Some(Err(e)) => Err(StateError::from(e)),
            None => Ok(None),
        }
    }

    /// Query the DAG submission for a specific run. Returns `(executor, dashboard_address)`.
    pub fn dag_submission_for_run(
        &self,
        run_id: &str,
    ) -> Result<Option<(String, Option<String>)>, StateError> {
        let mut stmt = self
            .conn
            .prepare("SELECT executor, dashboard_address FROM dag_submissions WHERE run_id = ?1")?;
        let mut rows = stmt.query_map(rusqlite::params![run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        match rows.next() {
            Some(Ok(row)) => Ok(Some(row)),
            Some(Err(e)) => Err(StateError::from(e)),
            None => Ok(None),
        }
    }

    /// Query all job-to-job edges. Returns `(from_job, to_job)` pairs.
    pub fn job_edges(&self) -> Result<Vec<(String, String)>, StateError> {
        let mut stmt = self
            .conn
            .prepare("SELECT from_job, to_job FROM job_edges")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut edges = Vec::new();
        for row in rows {
            edges.push(row?);
        }
        Ok(edges)
    }

    // -------------------------------------------------------------------
    // OX-6 invariant — Exactly-once Terminal
    // -------------------------------------------------------------------

    /// Return every job_id that holds more than one terminal status row.
    ///
    /// Defends the OX-6 invariant:
    /// each job must reach at most one terminal status per run. The
    /// `jobs` table's CHECK constraint and PRIMARY KEY make duplicates
    /// structurally impossible there, but the `job_history` audit trail
    /// is append-only and could accidentally record a second terminal
    /// row for the same `(run_id, job_id)` after a regression in the
    /// EventSink. This helper exposes the canonical SQL query that
    /// integration tests use to assert the invariant after exercising
    /// the Ledger.
    ///
    /// Returns the list of `(run_id, job_id, count)` triples for which
    /// `count > 1` — i.e., a violation. An empty vector means the
    /// invariant holds.
    ///
    /// Restored in task-20260527-0bb8 — was unintentionally removed
    /// alongside other vocabulary edits in commit aea7509 while the
    /// integration test that depends on it (`exactly_once_terminal.rs`)
    /// was still in tree; only `--lib` tests were run as the gate so
    /// the regression was not caught.
    pub fn terminal_status_violations(&self) -> Result<Vec<(String, String, i64)>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, job_id, COUNT(*) AS n
             FROM job_history
             GROUP BY run_id, job_id
             HAVING n > 1
             ORDER BY run_id, job_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut violations = Vec::new();
        for row in rows {
            violations.push(row?);
        }
        Ok(violations)
    }
}

// ---------------------------------------------------------------------------
// StateBackend implementation
// ---------------------------------------------------------------------------

impl StateBackend for StateDb {
    fn schema_version(&self) -> Result<u32, StateError> {
        self.schema_version()
    }

    fn register_jobs(&self, jobs: &[JobRecord]) -> Result<(), StateError> {
        self.register_jobs(jobs)
    }

    fn claim_job(&self, job_id: &str, session_id: &str) -> Result<bool, StateError> {
        self.claim_job(job_id, session_id)
    }

    fn complete_job(
        &self,
        job_id: &str,
        session_id: &str,
        exit_code: i32,
        output_hashes: &str,
    ) -> Result<bool, StateError> {
        self.complete_job(job_id, session_id, exit_code, output_hashes)
    }

    fn fail_job(&self, job_id: &str, session_id: &str, exit_code: i32) -> Result<bool, StateError> {
        self.fail_job(job_id, session_id, exit_code)
    }

    fn skip_job(&self, job_id: &str) -> Result<bool, StateError> {
        self.skip_job(job_id)
    }

    fn job_status(&self, job_id: &str) -> Result<Option<String>, StateError> {
        self.job_status(job_id)
    }

    fn jobs_by_status(&self, status: &str) -> Result<Vec<String>, StateError> {
        self.jobs_by_status(status)
    }

    fn cancel_jobs(
        &self,
        rule: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Vec<String>, StateError> {
        self.cancel_jobs(rule, session_id)
    }

    fn job_counts(&self) -> Result<JobCounts, StateError> {
        self.job_counts()
    }

    fn running_jobs_detail(&self) -> Result<Vec<RunningJobDetail>, StateError> {
        self.running_jobs_detail()
    }

    fn all_jobs_detail(&self) -> Result<Vec<AllJobDetail>, StateError> {
        self.all_jobs_detail()
    }

    fn pipeline_stats(&self) -> Result<Vec<PipelineStats>, StateError> {
        self.pipeline_stats()
    }

    fn create_session(
        &self,
        pid: u32,
        hostname: &str,
        target_filter: Option<&str>,
    ) -> Result<String, StateError> {
        self.create_session(pid, hostname, target_filter)
    }

    fn heartbeat(&self, session_id: &str) -> Result<(), StateError> {
        self.heartbeat(session_id)
    }

    fn complete_session(&self, session_id: &str) -> Result<(), StateError> {
        self.complete_session(session_id)
    }

    fn interrupt_session(&self, session_id: &str) -> Result<(), StateError> {
        self.interrupt_session(session_id)
    }

    fn find_stale_sessions(&self, threshold_secs: u64) -> Result<Vec<String>, StateError> {
        self.find_stale_sessions(threshold_secs)
    }

    fn reclaim_stale_jobs(&self, session_id: &str) -> Result<usize, StateError> {
        self.reclaim_stale_jobs(session_id)
    }

    fn active_sessions(&self) -> Result<Vec<SessionInfo>, StateError> {
        self.active_sessions()
    }

    fn begin_run(
        &self,
        run_id: &str,
        workflow_hash: Option<&ContentHash>,
        job_count: usize,
        note: Option<&str>,
    ) -> Result<(), StateError> {
        self.begin_run(run_id, workflow_hash, job_count, note)
    }

    fn end_run(
        &self,
        run_id: &str,
        succeeded: usize,
        failed: usize,
        skipped: usize,
    ) -> Result<(), StateError> {
        self.end_run(run_id, succeeded, failed, skipped)
    }

    fn record_job_history(&self, entry: &JobHistoryEntry) -> Result<(), StateError> {
        self.record_job_history(entry)
    }

    fn finalize_job_history(
        &self,
        run_id: &str,
        executor: &str,
        hostname: &str,
        wall_times: &std::collections::HashMap<String, u64>,
    ) -> Result<usize, StateError> {
        self.finalize_job_history(run_id, executor, hostname, wall_times)
    }

    fn job_history_for_run(&self, run_id: &str) -> Result<Vec<JobHistoryEntry>, StateError> {
        self.job_history_for_run(run_id)
    }

    fn list_runs(&self) -> Result<Vec<RunRecord>, StateError> {
        self.list_runs()
    }

    fn job_log_info(&self, job_id: &str) -> Result<Option<JobLogInfo>, StateError> {
        self.job_log_info(job_id)
    }

    fn jobs_with_logs(
        &self,
        rule: Option<&str>,
        failed_only: bool,
    ) -> Result<Vec<JobWithLog>, StateError> {
        self.jobs_with_logs(rule, failed_only)
    }

    fn register_edges(&self, edges: &[(String, String)]) -> Result<(), StateError> {
        self.register_edges(edges)
    }

    fn job_edges(&self) -> Result<Vec<(String, String)>, StateError> {
        self.job_edges()
    }
}

/// Current UNIX timestamp in seconds.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn temp_db() -> (NamedTempFile, StateDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    #[test]
    fn open_creates_schema() {
        let (_tmp, db) = temp_db();
        assert_eq!(db.schema_version().unwrap(), 9);
    }

    #[test]
    fn open_sets_busy_timeout() {
        // Two concurrent `ox run` sessions on a shared workspace must wait
        // for each other, not fail immediately with SQLITE_BUSY.
        let (_tmp, db) = temp_db();
        let timeout: i64 = db
            .conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(timeout, 30_000);
    }

    #[test]
    fn open_corrupt_db_surfaces_recovery_message() {
        // A corrupt state.db must not bubble a raw SQLite error — the user
        // needs to learn it is a regenerable cache with an escape hatch.
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"this is definitely not a sqlite database").unwrap();

        let err = match StateDb::open(tmp.path()) {
            Ok(_) => panic!("open() must fail on a corrupt database"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(msg.contains("regenerable cache"), "got: {msg}");
        assert!(msg.contains("ox clean --state"), "got: {msg}");
    }

    #[test]
    fn register_and_query_jobs() {
        let (_tmp, db) = temp_db();
        let jobs = vec![
            JobRecord {
                id: "j1".into(),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "j2".into(),
                rule_name: "test".into(),
                wildcards: "{}".into(),
                cache_key: Some("abc123".into()),
                run_id: None,
            },
        ];

        db.register_jobs(&jobs).unwrap();

        assert_eq!(db.job_status("j1").unwrap(), Some("pending".into()));
        assert_eq!(db.job_status("j2").unwrap(), Some("pending".into()));
        assert_eq!(db.job_status("j999").unwrap(), None);
    }

    #[test]
    fn register_jobs_is_idempotent() {
        let (_tmp, db) = temp_db();
        let jobs = vec![JobRecord {
            id: "j1".into(),
            rule_name: "build".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }];

        db.register_jobs(&jobs).unwrap();
        db.register_jobs(&jobs).unwrap(); // should not fail
        assert_eq!(db.jobs_by_status("pending").unwrap().len(), 1);
    }

    #[test]
    fn atomic_claiming_only_first_succeeds() {
        let (_tmp, db) = temp_db();

        // Create a session first (required by FK constraint awareness,
        // though SQLite defers FK checks by default).
        db.create_session(1234, "host-a", None).unwrap();
        let s2 = db.create_session(5678, "host-b", None).unwrap();

        // Use the first session ID from active_sessions.
        let sessions = db.active_sessions().unwrap();
        let s1 = &sessions[0].id;

        let jobs = vec![JobRecord {
            id: "j1".into(),
            rule_name: "build".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }];
        db.register_jobs(&jobs).unwrap();

        // First claim succeeds.
        assert!(db.claim_job("j1", s1).unwrap());
        // Second claim for the same job fails — it's already running.
        assert!(!db.claim_job("j1", &s2).unwrap());

        assert_eq!(db.job_status("j1").unwrap(), Some("running".into()));
    }

    #[test]
    fn job_status_transitions() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        let jobs = vec![JobRecord {
            id: "j1".into(),
            rule_name: "build".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }];
        db.register_jobs(&jobs).unwrap();
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("pending"));

        db.claim_job("j1", &sid).unwrap();
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("running"));

        db.complete_job("j1", &sid, 0, r#"{"out.txt":"abc"}"#)
            .unwrap();
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("completed"));
    }

    #[test]
    fn fail_and_skip_transitions() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        let jobs = vec![
            JobRecord {
                id: "j1".into(),
                rule_name: "r1".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "j2".into(),
                rule_name: "r2".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();

        db.claim_job("j1", &sid).unwrap();
        db.fail_job("j1", &sid, 1).unwrap();
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("failed"));

        db.skip_job("j2").unwrap();
        assert_eq!(db.job_status("j2").unwrap().as_deref(), Some("completed"));
    }

    #[test]
    fn zombie_session_cannot_terminalize_reclaimed_job() {
        // H16: a session whose job was reclaimed (stale heartbeat) must not
        // be able to terminalize that job after another session re-claims it.
        let (_tmp, db) = temp_db();
        let s1 = db.create_session(1, "host-a", None).unwrap();

        let jobs = vec![JobRecord {
            id: "j1".into(),
            rule_name: "build".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }];
        db.register_jobs(&jobs).unwrap();
        assert!(db.claim_job("j1", &s1).unwrap());

        // s1 goes stale; its job is reclaimed and re-claimed by s2.
        db.reclaim_stale_jobs(&s1).unwrap();
        let s2 = db.create_session(2, "host-b", None).unwrap();
        assert!(db.claim_job("j1", &s2).unwrap());

        // Zombie s1 resurfaces and tries to terminalize the job it
        // believes it still owns. Both writes must be rejected: j1 now
        // belongs to s2, and its (possibly stale) output hashes must
        // never reach the DB.
        assert!(!db.complete_job("j1", &s1, 0, "{}").unwrap());
        assert!(!db.fail_job("j1", &s1, 1).unwrap());
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("running"));

        // The legitimate claim holder can still terminalize.
        assert!(db.complete_job("j1", &s2, 0, "{}").unwrap());
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("completed"));
    }

    #[test]
    fn job_counts_aggregation() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        let jobs: Vec<JobRecord> = (0..5)
            .map(|i| JobRecord {
                id: format!("j{i}"),
                rule_name: "r".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            })
            .collect();
        db.register_jobs(&jobs).unwrap();

        db.claim_job("j0", &sid).unwrap();
        db.complete_job("j0", &sid, 0, "{}").unwrap();
        db.claim_job("j1", &sid).unwrap();
        db.fail_job("j1", &sid, 1).unwrap();
        db.skip_job("j2").unwrap();
        db.claim_job("j3", &sid).unwrap();
        // j3 left running, j4 left pending

        let counts = db.job_counts().unwrap();
        assert_eq!(counts.completed, 2); // j0 (executed) + j2 (cached)
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.skipped, 0);
        assert_eq!(counts.cached, 1);
        assert_eq!(counts.running, 1);
        assert_eq!(counts.pending, 1);
    }

    #[test]
    fn close_succeeds() {
        let (_tmp, db) = temp_db();
        db.close().unwrap();
    }

    #[test]
    fn begin_and_end_run() {
        let (_tmp, db) = temp_db();

        db.begin_run(
            "run-1",
            Some(&ContentHash::from_hex("ab".repeat(32)).unwrap()),
            5,
            Some("test run"),
        )
        .unwrap();

        let runs = db.list_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "run-1");
        assert_eq!(runs[0].note.as_deref(), Some("test run"));
        assert_eq!(
            runs[0].workflow_hash.as_ref().map(ContentHash::as_str),
            Some("ab".repeat(32).as_str())
        );
        assert_eq!(runs[0].job_count, Some(5));
        assert!(runs[0].completed_at.is_none());

        db.end_run("run-1", 3, 1, 1).unwrap();

        let runs = db.list_runs().unwrap();
        assert!(runs[0].completed_at.is_some());
        assert_eq!(runs[0].succeeded, Some(3));
        assert_eq!(runs[0].failed, Some(1));
        assert_eq!(runs[0].skipped, Some(1));
    }

    #[test]
    fn record_and_query_job_history() {
        let (_tmp, db) = temp_db();

        db.begin_run("run-1", None, 2, None).unwrap();

        let entry1 = JobHistoryEntry {
            run_id: "run-1".into(),
            job_id: "j1".into(),
            rule_name: "build".into(),
            wildcards: Some("{}".into()),
            input_hashes: Some(r#"{"src.c":"abc"}"#.into()),
            output_hashes: Some(r#"{"out.o":"def"}"#.into()),
            params_hash: None,
            env_hash: None,
            executor: Some("local".into()),
            hostname: Some("localhost".into()),
            started_at: Some(1000),
            completed_at: Some(1042),
            wall_time_ms: Some(42000),
            peak_mem_mb: Some(128),
            exit_code: Some(0),
            reproducibility_class: Some("deterministic".into()),
            artifact_provenance_json: None,
        };
        let entry2 = JobHistoryEntry {
            run_id: "run-1".into(),
            job_id: "j2".into(),
            rule_name: "test".into(),
            wildcards: None,
            input_hashes: None,
            output_hashes: None,
            params_hash: None,
            env_hash: None,
            executor: Some("local".into()),
            hostname: Some("localhost".into()),
            started_at: Some(1042),
            completed_at: Some(1050),
            wall_time_ms: Some(8000),
            peak_mem_mb: Some(64),
            exit_code: Some(1),
            reproducibility_class: None,
            artifact_provenance_json: None,
        };

        db.record_job_history(&entry1).unwrap();
        db.record_job_history(&entry2).unwrap();

        let history = db.job_history_for_run("run-1").unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].job_id, "j1");
        assert_eq!(history[0].rule_name, "build");
        assert_eq!(history[0].wall_time_ms, Some(42000));
        assert_eq!(history[0].peak_mem_mb, Some(128));
        assert_eq!(history[0].exit_code, Some(0));
        assert_eq!(history[1].job_id, "j2");
        assert_eq!(history[1].exit_code, Some(1));

        // Verify reproducibility fields round-trip.
        assert_eq!(
            history[0].reproducibility_class.as_deref(),
            Some("deterministic")
        );
        assert!(history[0].artifact_provenance_json.is_none());
        assert!(history[1].reproducibility_class.is_none());
    }

    #[test]
    fn job_history_reproducibility_round_trip() {
        let (_tmp, db) = temp_db();
        db.begin_run("run-prov", None, 1, None).unwrap();

        let prov_json = r#"{"input_hashes":[["abc","input.csv"]],"job_spec_hash":"spec1","reproducibility":"seed_deterministic"}"#;
        let entry = JobHistoryEntry {
            run_id: "run-prov".into(),
            job_id: "j-prov".into(),
            rule_name: "train".into(),
            wildcards: None,
            input_hashes: None,
            output_hashes: None,
            params_hash: None,
            env_hash: None,
            executor: Some("local".into()),
            hostname: None,
            started_at: None,
            completed_at: None,
            wall_time_ms: None,
            peak_mem_mb: None,
            exit_code: Some(0),
            reproducibility_class: Some("seed_deterministic".into()),
            artifact_provenance_json: Some(prov_json.into()),
        };
        db.record_job_history(&entry).unwrap();

        let history = db.job_history_for_run("run-prov").unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].reproducibility_class.as_deref(),
            Some("seed_deterministic")
        );
        assert_eq!(
            history[0].artifact_provenance_json.as_deref(),
            Some(prov_json)
        );
    }

    #[test]
    fn job_history_empty_for_unknown_run() {
        let (_tmp, db) = temp_db();
        let history = db.job_history_for_run("nonexistent").unwrap();
        assert!(history.is_empty());
    }

    #[test]
    fn finalize_job_history_from_db() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();
        let run_id = "run-finalize";

        db.begin_run(run_id, None, 3, None).unwrap();

        let jobs = vec![
            JobRecord {
                id: "j1".into(),
                rule_name: "build".into(),
                wildcards: r#"{"x":"1"}"#.into(),
                cache_key: None,
                run_id: Some(run_id.into()),
            },
            JobRecord {
                id: "j2".into(),
                rule_name: "test".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: Some(run_id.into()),
            },
            JobRecord {
                id: "j3".into(),
                rule_name: "lint".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: Some(run_id.into()),
            },
        ];
        db.register_jobs(&jobs).unwrap();

        // Simulate event-bus transitions: j1 completed, j2 failed, j3 still pending.
        db.claim_job("j1", &sid).unwrap();
        db.complete_job("j1", &sid, 0, "").unwrap();
        db.claim_job("j2", &sid).unwrap();
        db.fail_job("j2", &sid, 1).unwrap();

        let mut wall_times = std::collections::HashMap::new();
        wall_times.insert("j1".to_string(), 500u64);

        let count = db
            .finalize_job_history(run_id, "local", "localhost", &wall_times)
            .unwrap();
        // j1 (completed) and j2 (failed) — j3 is still pending so excluded.
        assert_eq!(count, 2);

        let history = db.job_history_for_run(run_id).unwrap();
        assert_eq!(history.len(), 2);

        let h1 = history.iter().find(|h| h.job_id == "j1").unwrap();
        assert_eq!(h1.rule_name, "build");
        assert_eq!(h1.wildcards.as_deref(), Some(r#"{"x":"1"}"#));
        assert_eq!(h1.exit_code, Some(0));
        assert_eq!(h1.wall_time_ms, Some(500));
        assert_eq!(h1.executor.as_deref(), Some("local"));
        assert_eq!(h1.hostname.as_deref(), Some("localhost"));

        let h2 = history.iter().find(|h| h.job_id == "j2").unwrap();
        assert_eq!(h2.rule_name, "test");
        assert_eq!(h2.exit_code, Some(1));
        assert_eq!(h2.wall_time_ms, None); // no entry in wall_times map
    }

    #[test]
    fn finalize_job_history_includes_skipped() {
        let (_tmp, db) = temp_db();
        let run_id = "run-skip";

        db.begin_run(run_id, None, 1, None).unwrap();

        let jobs = vec![JobRecord {
            id: "j1".into(),
            rule_name: "cached".into(),
            wildcards: "{}".into(),
            cache_key: Some("abc".into()),
            run_id: Some(run_id.into()),
        }];
        db.register_jobs(&jobs).unwrap();
        db.skip_job("j1").unwrap();

        let count = db
            .finalize_job_history(
                run_id,
                "local",
                "localhost",
                &std::collections::HashMap::new(),
            )
            .unwrap();
        assert_eq!(count, 1);

        let history = db.job_history_for_run(run_id).unwrap();
        assert_eq!(history.len(), 1);
        // Skipped/cached jobs are stored as status=completed in the DB,
        // so they get exit_code 0 from the fallback.
        assert_eq!(history[0].exit_code, Some(0));
    }

    #[test]
    fn job_timing_returns_timestamps() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1234, "host-a", None).unwrap();
        let jobs = vec![JobRecord {
            id: "j1".into(),
            rule_name: "build".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }];
        db.register_jobs(&jobs).unwrap();
        db.claim_job("j1", &sid).unwrap();
        db.complete_job("j1", &sid, 0, "").unwrap();

        let (started, completed) = db.job_timing("j1").unwrap();
        assert!(started.is_some());
        assert!(completed.is_some());

        // Unknown job returns (None, None).
        let (s, c) = db.job_timing("unknown").unwrap();
        assert!(s.is_none());
        assert!(c.is_none());
    }

    #[test]
    fn slowest_rules_for_run_aggregates_durations() {
        let (_tmp, db) = temp_db();
        db.begin_run("run-1", None, 3, None).unwrap();

        // Insert three history entries with different rules and durations.
        for (job_id, rule, wall_ms) in [
            ("j1", "optimize", 5000),
            ("j2", "data", 3000),
            ("j3", "optimize", 8000),
        ] {
            let entry = JobHistoryEntry {
                run_id: "run-1".into(),
                job_id: job_id.into(),
                rule_name: rule.into(),
                wildcards: None,
                input_hashes: None,
                output_hashes: None,
                params_hash: None,
                env_hash: None,
                executor: None,
                hostname: None,
                started_at: None,
                completed_at: None,
                wall_time_ms: Some(wall_ms),
                peak_mem_mb: None,
                exit_code: Some(0),
                reproducibility_class: None,
                artifact_provenance_json: None,
            };
            db.record_job_history(&entry).unwrap();
        }

        let stats = db.slowest_rules_for_run("run-1", 3).unwrap();
        assert_eq!(stats.len(), 2);
        // optimize should be first (total 13000ms > data 3000ms).
        assert_eq!(stats[0].rule_name, "optimize");
        assert_eq!(stats[0].total_ms, 13000);
        assert_eq!(stats[0].max_ms, 8000);
        assert_eq!(stats[0].job_count, 2);
        assert_eq!(stats[1].rule_name, "data");
        assert_eq!(stats[1].total_ms, 3000);

        // Empty for unknown run.
        let empty = db.slowest_rules_for_run("nonexistent", 3).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn list_runs_most_recent_first() {
        let (_tmp, db) = temp_db();

        // Insert two runs with explicit timestamps to control ordering.
        db.conn
            .execute(
                "INSERT INTO runs (id, started_at) VALUES ('run-old', 1000)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO runs (id, started_at) VALUES ('run-new', 2000)",
                [],
            )
            .unwrap();

        let runs = db.list_runs().unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "run-new");
        assert_eq!(runs[1].id, "run-old");
    }

    #[test]
    fn register_and_query_edges() {
        let (_tmp, db) = temp_db();
        let jobs = vec![
            JobRecord {
                id: "build".into(),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "test".into(),
                rule_name: "test".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "deploy".into(),
                rule_name: "deploy".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();

        let edges = vec![
            ("build".to_string(), "test".to_string()),
            ("test".to_string(), "deploy".to_string()),
        ];
        db.register_edges(&edges).unwrap();

        let result = db.job_edges().unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&("build".into(), "test".into())));
        assert!(result.contains(&("test".into(), "deploy".into())));
    }

    #[test]
    fn register_edges_is_idempotent() {
        let (_tmp, db) = temp_db();
        let edges = vec![("a".to_string(), "b".to_string())];
        db.register_edges(&edges).unwrap();
        db.register_edges(&edges).unwrap(); // should not fail
        assert_eq!(db.job_edges().unwrap().len(), 1);
    }

    /// Verify that `StateDb` can be used through the `StateBackend` trait.
    #[test]
    fn trait_dispatch_works() {
        let (_tmp, db) = temp_db();
        let backend: &dyn StateBackend = &db;

        assert_eq!(backend.schema_version().unwrap(), 9);

        let sid = backend.create_session(1, "localhost", None).unwrap();

        let jobs = vec![JobRecord {
            id: "j1".into(),
            rule_name: "build".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }];
        backend.register_jobs(&jobs).unwrap();
        assert!(backend.claim_job("j1", &sid).unwrap());
        backend.complete_job("j1", &sid, 0, "{}").unwrap();

        let counts = backend.job_counts().unwrap();
        assert_eq!(counts.completed, 1);

        let sessions = backend.active_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn jobs_with_logs_failed_filter() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        let jobs = vec![
            JobRecord {
                id: "j1".into(),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "j2".into(),
                rule_name: "test".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();

        // j1 succeeds, j2 fails
        db.claim_job("j1", &sid).unwrap();
        db.complete_job("j1", &sid, 0, "{}").unwrap();
        db.claim_job("j2", &sid).unwrap();
        db.fail_job("j2", &sid, 1).unwrap();

        // Without filter: both jobs returned.
        let all = db.jobs_with_logs(None, false).unwrap();
        assert_eq!(all.len(), 2);

        // With --failed filter: only the failed job.
        let failed = db.jobs_with_logs(None, true).unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].id, "j2");
        assert_eq!(failed[0].status, "failed");

        // With rule + failed filter.
        let build_failed = db.jobs_with_logs(Some("build"), true).unwrap();
        assert!(build_failed.is_empty());
        let test_failed = db.jobs_with_logs(Some("test"), true).unwrap();
        assert_eq!(test_failed.len(), 1);
    }

    #[test]
    fn current_jobs_returns_live_state() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        let jobs = vec![
            JobRecord {
                id: "j1".into(),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: Some("ck1".into()),
                run_id: None,
            },
            JobRecord {
                id: "j2".into(),
                rule_name: "test".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();
        db.claim_job("j1", &sid).unwrap();
        db.complete_job("j1", &sid, 0, r#"{"out":"abc"}"#).unwrap();

        let current = db.current_jobs().unwrap();
        assert_eq!(current.len(), 2);
        assert_eq!(current[0].job_id, "j1");
        assert_eq!(current[0].status, "completed");
        assert_eq!(
            current[0].output_hashes.as_deref(),
            Some(r#"{"out":"abc"}"#)
        );
        assert_eq!(current[1].job_id, "j2");
        assert_eq!(current[1].status, "pending");

        assert_eq!(db.current_job_count().unwrap(), 2);
    }

    #[test]
    fn snapshot_diff_against_current_state() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        // Register jobs and create a snapshot while j1 is pending.
        let jobs = vec![
            JobRecord {
                id: "j1".into(),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "j2".into(),
                rule_name: "test".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();
        db.create_snapshot("before", None, None).unwrap();

        // Now advance j1 to completed — current state diverges from snapshot.
        db.claim_job("j1", &sid).unwrap();
        db.complete_job("j1", &sid, 0, r#"{"out":"hash"}"#).unwrap();

        let snap_jobs = db.snapshot_jobs("before").unwrap();
        let current = db.current_jobs().unwrap();

        // Snapshot has j1 as pending; current has j1 as completed.
        assert_eq!(snap_jobs[0].status, "pending");
        assert_eq!(current[0].status, "completed");
        // j2 unchanged in both.
        assert_eq!(snap_jobs[1].status, "pending");
        assert_eq!(current[1].status, "pending");
    }

    #[test]
    fn gate_operations_use_safe_timestamp() {
        // Regression test for ox-qbi: gate operations must not panic
        // on clock anomalies. They should use unix_now() (unwrap_or_default)
        // instead of raw .unwrap() on duration_since(UNIX_EPOCH).
        let (_tmp, db) = temp_db();
        let jobs = vec![JobRecord {
            id: "g1".into(),
            rule_name: "deploy".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }];
        db.register_jobs(&jobs).unwrap();

        // create_gate should succeed and return a valid id
        let gate_id = db.create_gate("deploy", "g1").unwrap();
        assert!(gate_id > 0);

        // list_gates should show the pending gate with a non-zero timestamp
        let gates = db.list_gates().unwrap();
        assert_eq!(gates.len(), 1);
        assert_eq!(gates[0].status, "pending");
        assert!(gates[0].created_at > 0);

        // approve_gate should succeed
        let gate_id2 = db.create_gate("deploy", "g1").unwrap();
        db.approve_gate(gate_id2, "admin", "looks good").unwrap();
        let gates = db.list_gates().unwrap();
        let approved = gates.iter().find(|g| g.id == gate_id2).unwrap();
        assert_eq!(approved.status, "approved");
        assert!(approved.decided_at.unwrap() > 0);

        // reject_gate should succeed
        let gate_id3 = db.create_gate("deploy", "g1").unwrap();
        db.reject_gate(gate_id3, "admin", "not ready").unwrap();
        let gates = db.list_gates().unwrap();
        let rejected = gates.iter().find(|g| g.id == gate_id3).unwrap();
        assert_eq!(rejected.status, "rejected");
        assert!(rejected.decided_at.unwrap() > 0);
    }

    #[test]
    fn pending_jobs_with_blockers_shows_unsatisfied_deps() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        // DAG: build -> test -> deploy
        let jobs = vec![
            JobRecord {
                id: "build".into(),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "test".into(),
                rule_name: "test".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "deploy".into(),
                rule_name: "deploy".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();
        let edges = vec![
            ("build".to_string(), "test".to_string()),
            ("test".to_string(), "deploy".to_string()),
        ];
        db.register_edges(&edges).unwrap();

        // Initially all pending: test blocked by build, deploy blocked by test.
        let pending = db.pending_jobs_with_blockers().unwrap();
        assert_eq!(pending.len(), 3);
        let build_p = pending.iter().find(|j| j.id == "build").unwrap();
        assert!(build_p.waiting_for.is_empty()); // no upstream
        let test_p = pending.iter().find(|j| j.id == "test").unwrap();
        assert_eq!(test_p.waiting_for, vec!["build"]);
        let deploy_p = pending.iter().find(|j| j.id == "deploy").unwrap();
        assert_eq!(deploy_p.waiting_for, vec!["test"]);

        // Complete build -> test should now only be blocked by nothing (build done).
        db.claim_job("build", &sid).unwrap();
        db.complete_job("build", &sid, 0, "{}").unwrap();
        let pending = db.pending_jobs_with_blockers().unwrap();
        assert_eq!(pending.len(), 2); // build is no longer pending
        let test_p = pending.iter().find(|j| j.id == "test").unwrap();
        assert!(test_p.waiting_for.is_empty()); // build is completed
        let deploy_p = pending.iter().find(|j| j.id == "deploy").unwrap();
        assert_eq!(deploy_p.waiting_for, vec!["test"]); // test still pending
    }

    #[test]
    fn cancel_job_ids_targets_specific_jobs() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        let jobs = vec![
            JobRecord {
                id: "j1".into(),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "j2".into(),
                rule_name: "test".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "j3".into(),
                rule_name: "deploy".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();

        // j1 running, j2 pending, j3 completed
        db.claim_job("j1", &sid).unwrap();
        db.claim_job("j3", &sid).unwrap();
        db.complete_job("j3", &sid, 0, "{}").unwrap();

        // Cancel j1 (running) and j2 (pending) by explicit IDs.
        let cancelled = db
            .cancel_job_ids(&["j1".into(), "j2".into(), "j3".into()])
            .unwrap();

        // j3 was already completed, so only j1 and j2 should be cancelled.
        assert_eq!(cancelled.len(), 2);
        assert!(cancelled.contains(&"j1".into()));
        assert!(cancelled.contains(&"j2".into()));

        assert_eq!(db.job_status("j1").unwrap(), Some("cancelled".into()));
        assert_eq!(db.job_status("j2").unwrap(), Some("cancelled".into()));
        assert_eq!(db.job_status("j3").unwrap(), Some("completed".into()));
    }

    #[test]
    fn cancel_job_ids_empty_input() {
        let (_tmp, db) = temp_db();
        let cancelled = db.cancel_job_ids(&[]).unwrap();
        assert!(cancelled.is_empty());
    }

    // -----------------------------------------------------------------------
    // Guarded state transition tests
    // -----------------------------------------------------------------------

    #[test]
    fn complete_job_only_from_running() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        let jobs = vec![
            JobRecord {
                id: "j1".into(),
                rule_name: "r".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "j2".into(),
                rule_name: "r".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();

        // Completing a pending job must fail (not running).
        assert!(!db.complete_job("j1", &sid, 0, "{}").unwrap());
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("pending"));

        // Claim j1 → running, then complete → true.
        db.claim_job("j1", &sid).unwrap();
        assert!(db.complete_job("j1", &sid, 0, "{}").unwrap());
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("completed"));

        // Completing an already-completed job must return false.
        assert!(!db.complete_job("j1", &sid, 0, "{}").unwrap());

        // Completing a nonexistent job must return false.
        assert!(!db.complete_job("j999", &sid, 0, "{}").unwrap());
    }

    #[test]
    fn fail_job_only_from_running() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        let jobs = vec![JobRecord {
            id: "j1".into(),
            rule_name: "r".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }];
        db.register_jobs(&jobs).unwrap();

        // Failing a pending job must fail.
        assert!(!db.fail_job("j1", &sid, 1).unwrap());
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("pending"));

        // Claim → running, then fail → true.
        db.claim_job("j1", &sid).unwrap();
        assert!(db.fail_job("j1", &sid, 1).unwrap());
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("failed"));

        // Failing an already-failed job must return false.
        assert!(!db.fail_job("j1", &sid, 1).unwrap());
    }

    #[test]
    fn skip_job_only_from_pending() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        let jobs = vec![
            JobRecord {
                id: "j1".into(),
                rule_name: "r".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "j2".into(),
                rule_name: "r".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();

        // Skipping a pending job succeeds (status becomes completed+cached).
        assert!(db.skip_job("j1").unwrap());
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("completed"));

        // Skipping an already-completed (cached) job must return false.
        assert!(!db.skip_job("j1").unwrap());

        // Skipping a running job must return false.
        db.claim_job("j2", &sid).unwrap();
        assert!(!db.skip_job("j2").unwrap());
        assert_eq!(db.job_status("j2").unwrap().as_deref(), Some("running"));
    }
}
