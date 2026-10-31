//! Read-side derivation: what a job row *effectively* is, right now.
//!
//! A `jobs` row carries a **declared** status — what the owning session
//! last wrote. The ledger also holds two other views of the same job that
//! a reader must join to be honest:
//!
//! - the **projected** view: the owner's lease, `sessions.heartbeat_at`
//!   against [`crate::claim::DEFAULT_LEASE_SECS`] —
//!   exactly what peers and the reclaim path use to decide a claim is dead;
//! - the **observed** view: `sessions.status ∈ {active, completed,
//!   interrupted}`.
//!
//! Reading `jobs.status` verbatim is how `ox status` came to print
//! "Sessions: 0 active" and "1 running" on the same screen. This module is
//! the single derivation that resolves the contradiction, shared by every
//! reader (`ox status`, the dashboard, `ox top`).
//!
//! The rule, in one line: **a `running` row is running only if its session
//! is `active` *and* its heartbeat is younger than the lease.** Otherwise
//! it is [`EffectiveStatus::Orphaned`], with the reason that made it so.
//!
//! Readers stay read-only. Deriving `Orphaned` never writes: reclaiming is
//! the writer's job (`reset_inactive_job_rows` at `ox run` start-up), and a
//! viewer that "fixed" rows could race a session whose heartbeat is merely
//! late under load.
//!
//! ```
//! use ox_state::effective::{EffectiveStatus, OrphanReason, SessionLiveness, effective_status};
//!
//! // A running row whose owner heartbeated one second ago: really running.
//! let owner = SessionLiveness { status: "active".into(), heartbeat_at: 999 };
//! assert_eq!(
//!     effective_status("running", None, Some(&owner), 1000, 90),
//!     EffectiveStatus::Running,
//! );
//!
//! // The same row after the owner was interrupted: orphaned.
//! let owner = SessionLiveness { status: "interrupted".into(), heartbeat_at: 999 };
//! assert!(matches!(
//!     effective_status("running", Some(900), Some(&owner), 1000, 90),
//!     EffectiveStatus::Orphaned { reason: OrphanReason::SessionInterrupted, .. },
//! ));
//! ```

use crate::claim::{DEFAULT_LEASE_SECS, LEASE_ENV_VAR};
use crate::db::StateDb;
use crate::error::StateError;

/// The owner session's liveness inputs, as stored in the `sessions` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLiveness {
    /// `sessions.status`: `active`, `completed` or `interrupted`.
    pub status: String,
    /// `sessions.heartbeat_at`, UNIX seconds.
    pub heartbeat_at: u64,
}

impl SessionLiveness {
    /// The session is alive: `active` with a heartbeat younger than the lease.
    ///
    /// Same predicate as the claim protocol, so readers and peers agree on
    /// which claims are dead.
    pub fn is_live(&self, now: u64, lease_secs: u64) -> bool {
        self.status == "active" && now.saturating_sub(self.heartbeat_at) < lease_secs
    }
}

/// Why a `running` row is not actually running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanReason {
    /// The owning session recorded `interrupted` (Ctrl-C, signal).
    SessionInterrupted,
    /// The owning session is `active` but stopped heartbeating: its last
    /// heartbeat is older than the lease.
    HeartbeatStale,
    /// The owning session recorded `completed` while leaving the row behind.
    SessionCompleted,
    /// The row names no session, or names one that is not in `sessions`.
    SessionMissing,
}

impl OrphanReason {
    /// Stable machine-facing token (`--json`, the dashboard API).
    pub fn as_str(&self) -> &'static str {
        match self {
            OrphanReason::SessionInterrupted => "session_interrupted",
            OrphanReason::HeartbeatStale => "heartbeat_stale",
            OrphanReason::SessionCompleted => "session_completed",
            OrphanReason::SessionMissing => "session_missing",
        }
    }

    /// One-line human-facing explanation, for `ox status` and the TUI.
    pub fn describe(&self) -> &'static str {
        match self {
            OrphanReason::SessionInterrupted => "owning session was interrupted",
            OrphanReason::HeartbeatStale => "owning session stopped heartbeating",
            OrphanReason::SessionCompleted => "owning session completed without it",
            OrphanReason::SessionMissing => "no owning session on record",
        }
    }
}

/// What a job row is, once the owner's liveness is taken into account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectiveStatus {
    /// Waiting to be claimed.
    Pending,
    /// Claimed by a live session: genuinely in flight.
    Running,
    /// Declared `running`, but nobody is executing it any more. The next
    /// `ox run` re-evaluates the row; readers only report it.
    Orphaned {
        /// `jobs.started_at` of the abandoned attempt, when recorded — the
        /// age of the orphan is measured from it.
        since: Option<u64>,
        /// What made the claim dead.
        reason: OrphanReason,
    },
    /// Finished successfully (cache hits included).
    Completed,
    /// Finished with an error.
    Failed,
    /// Skipped for a non-cache reason.
    Skipped,
    /// Cancelled.
    Cancelled,
    /// A declared status this derivation does not model; carried verbatim.
    Other(String),
}

impl EffectiveStatus {
    /// Stable machine-facing token. `Orphaned` renders as `"orphaned"` —
    /// deliberately *not* `"running"`, which is the whole point.
    pub fn as_str(&self) -> &str {
        match self {
            EffectiveStatus::Pending => "pending",
            EffectiveStatus::Running => "running",
            EffectiveStatus::Orphaned { .. } => "orphaned",
            EffectiveStatus::Completed => "completed",
            EffectiveStatus::Failed => "failed",
            EffectiveStatus::Skipped => "skipped",
            EffectiveStatus::Cancelled => "cancelled",
            EffectiveStatus::Other(s) => s.as_str(),
        }
    }

    /// Whether this row is work actually in flight right now.
    pub fn is_running(&self) -> bool {
        matches!(self, EffectiveStatus::Running)
    }

    /// Whether this row was abandoned by a session that is no longer live.
    pub fn is_orphaned(&self) -> bool {
        matches!(self, EffectiveStatus::Orphaned { .. })
    }

    /// The reason, when orphaned.
    pub fn orphan_reason(&self) -> Option<OrphanReason> {
        match self {
            EffectiveStatus::Orphaned { reason, .. } => Some(*reason),
            _ => None,
        }
    }
}

/// Derive the effective status of one job row.
///
/// `declared` is `jobs.status`, `started_at` is `jobs.started_at`, `owner`
/// is the joined `sessions` row (`None` when the job names no session or
/// the session row is gone), `now` is UNIX seconds and `lease_secs` the
/// claim lease.
///
/// Only `running` rows are reinterpreted: a terminal status is a fact the
/// owner already wrote down, and a `pending` row is owned by nobody.
pub fn effective_status(
    declared: &str,
    started_at: Option<u64>,
    owner: Option<&SessionLiveness>,
    now: u64,
    lease_secs: u64,
) -> EffectiveStatus {
    match declared {
        "pending" => EffectiveStatus::Pending,
        "completed" => EffectiveStatus::Completed,
        "failed" => EffectiveStatus::Failed,
        "skipped" => EffectiveStatus::Skipped,
        "cancelled" => EffectiveStatus::Cancelled,
        "running" => match owner {
            Some(o) if o.is_live(now, lease_secs) => EffectiveStatus::Running,
            Some(o) => EffectiveStatus::Orphaned {
                since: started_at,
                reason: match o.status.as_str() {
                    "interrupted" => OrphanReason::SessionInterrupted,
                    "completed" => OrphanReason::SessionCompleted,
                    // `active` but past the lease — the crash case.
                    _ => OrphanReason::HeartbeatStale,
                },
            },
            None => EffectiveStatus::Orphaned {
                since: started_at,
                reason: OrphanReason::SessionMissing,
            },
        },
        other => EffectiveStatus::Other(other.to_string()),
    }
}

/// The claim lease readers must use: `OX_SESSION_LEASE_SECS` when set to a
/// positive integer, else [`DEFAULT_LEASE_SECS`].
///
/// Same resolution as `ox run`, so a reader and the session it observes
/// never disagree about what "stale" means.
pub fn lease_secs_from_env() -> u64 {
    std::env::var(LEASE_ENV_VAR)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_LEASE_SECS)
}

// ---------------------------------------------------------------------------
// Ledger-backed views
// ---------------------------------------------------------------------------

/// One job row joined with its owner's liveness and resolved through
/// [`effective_status`].
#[derive(Debug, Clone)]
pub struct JobView {
    /// Job identifier.
    pub id: String,
    /// Rule that produced the job.
    pub rule_name: String,
    /// JSON-encoded wildcard bindings (`"{}"` when absent).
    pub wildcards: String,
    /// `jobs.status` as last written by the owning session.
    pub declared_status: String,
    /// What the row is once the owner's liveness is joined in.
    pub effective: EffectiveStatus,
    /// UNIX timestamp (seconds) when execution started.
    pub started_at: Option<u64>,
    /// UNIX timestamp (seconds) when execution completed.
    pub completed_at: Option<u64>,
    /// Process exit code.
    pub exit_code: Option<i32>,
    /// Whether the row was observed satisfied from cache. Legacy history
    /// without an observation is false; [`UnitView::cache_hit`] preserves null.
    pub cached: bool,
    /// The claiming session, when the row names one.
    pub session_id: Option<String>,
}

/// A [`JobView`] with where the unit ran: the data behind
/// `ox status --json --units`.
#[derive(Debug, Clone)]
pub struct UnitView {
    /// The job row, resolved through [`effective_status`].
    pub view: JobView,
    /// Whether this unit was satisfied from cache; unknown for legacy history.
    pub cache_hit: Option<bool>,
    /// Executor backend: the run's DAG submission, else the job's history
    /// row; `None` while neither exists.
    pub executor: Option<String>,
    /// Ray dashboard address of the DAG submission, or the host name of a
    /// local job; `None` otherwise (a SLURM submission records none).
    pub endpoint: Option<String>,
    /// Executor submission ID (Ray submission ID, SLURM job ID).
    pub submission_id: Option<String>,
    /// Host the executor ran the job on, from the history row.
    pub node: Option<String>,
    /// Names of the outputs whose hash the history row records. State does
    /// not store declared output paths, so this is empty for a job the
    /// cache layer never keyed.
    pub outputs: Vec<String>,
}

/// Aggregate counts over [`EffectiveStatus`], with `orphaned` split out of
/// `running`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveCounts {
    /// Jobs waiting to be claimed.
    pub pending: usize,
    /// Jobs claimed by a live session — genuinely in flight.
    pub running: usize,
    /// Jobs declared `running` whose owner is gone.
    pub orphaned: usize,
    /// Jobs that finished successfully (cache hits included).
    pub completed: usize,
    /// Jobs that finished with an error.
    pub failed: usize,
    /// Jobs skipped for a non-cache reason.
    pub skipped: usize,
    /// Jobs satisfied from cache (a subset of `completed`).
    pub cached: usize,
    /// Jobs that were cancelled.
    pub cancelled: usize,
    /// Rows with an unmodelled status, including ambiguous legacy history.
    pub other: usize,
}

impl EffectiveCounts {
    /// Total jobs across every status (`cached` is not added: it is a
    /// subset of `completed`).
    pub fn total(&self) -> usize {
        self.pending
            + self.running
            + self.orphaned
            + self.completed
            + self.failed
            + self.skipped
            + self.cancelled
            + self.other
    }

    /// Aggregate a slice of views.
    pub fn from_views(views: &[JobView]) -> Self {
        let mut c = EffectiveCounts::default();
        for v in views {
            match &v.effective {
                EffectiveStatus::Pending => c.pending += 1,
                EffectiveStatus::Running => c.running += 1,
                EffectiveStatus::Orphaned { .. } => c.orphaned += 1,
                EffectiveStatus::Completed => c.completed += 1,
                EffectiveStatus::Failed => c.failed += 1,
                EffectiveStatus::Skipped => c.skipped += 1,
                EffectiveStatus::Cancelled => c.cancelled += 1,
                EffectiveStatus::Other(_) => c.other += 1,
            }
            if v.cached {
                c.cached += 1;
            }
        }
        c
    }
}

/// An upstream job that still blocks a pending one, with its effective status.
#[derive(Debug, Clone)]
pub struct BlockingUpstream {
    /// Rule name of the blocking upstream job.
    pub rule_name: String,
    /// What that upstream job effectively is.
    pub effective: EffectiveStatus,
}

/// A pending job together with the upstream jobs still blocking it.
#[derive(Debug, Clone)]
pub struct PendingJobView {
    /// Job identifier.
    pub id: String,
    /// Rule that produced the job.
    pub rule_name: String,
    /// JSON-encoded wildcard bindings.
    pub wildcards: String,
    /// Upstream jobs that are not yet completed or skipped.
    pub waiting_for: Vec<BlockingUpstream>,
}

impl PendingJobView {
    /// Rule names of the blocking upstreams whose owner is gone.
    pub fn orphaned_upstreams(&self) -> Vec<&str> {
        self.waiting_for
            .iter()
            .filter(|u| u.effective.is_orphaned())
            .map(|u| u.rule_name.as_str())
            .collect()
    }
}

/// A job's live row wins only while it still names the selected run.
/// Terminal history fills each missing job independently (ADR-012 read path).
const RUN_JOBS: &str = "WITH run_jobs AS (
    SELECT id, rule_name, wildcards, status, started_at, completed_at, exit_code,
           COALESCE(cached, 0) AS cached, session_id, executor_submission_id, run_id
    FROM jobs WHERE run_id = ?1
    UNION ALL
    SELECT h.job_id, h.rule_name, h.wildcards,
           COALESCE(h.status, CASE WHEN h.exit_code = 0 THEN 'completed'
                                  WHEN h.exit_code IS NOT NULL THEN 'failed'
                                  ELSE 'unknown' END),
           h.started_at, h.completed_at, h.exit_code,
           CASE WHEN h.status IS NULL THEN NULL ELSE h.cached END,
           NULL, CASE WHEN h.status IS NULL THEN NULL ELSE h.submission_id END, h.run_id
    FROM job_history h
    WHERE h.run_id = ?1 AND NOT EXISTS (
        SELECT 1 FROM jobs live WHERE live.id = h.job_id AND live.run_id = ?1)
)";

/// Columns every view query selects, in the order [`row_to_view`] expects.
const VIEW_COLUMNS: &str = "j.id, j.rule_name, COALESCE(j.wildcards, '{}'), j.status,
     j.started_at, j.completed_at, j.exit_code, COALESCE(j.cached, 0),
     j.session_id, s.status, s.heartbeat_at";

fn row_to_view(row: &rusqlite::Row<'_>, now: u64, lease_secs: u64) -> rusqlite::Result<JobView> {
    let declared_status: String = row.get(3)?;
    let started_at: Option<u64> = row.get(4)?;
    let owner_status: Option<String> = row.get(9)?;
    let owner_heartbeat: Option<u64> = row.get(10)?;
    let owner = owner_status
        .zip(owner_heartbeat)
        .map(|(status, heartbeat_at)| SessionLiveness {
            status,
            heartbeat_at,
        });
    Ok(JobView {
        id: row.get(0)?,
        rule_name: row.get(1)?,
        wildcards: row.get(2)?,
        effective: effective_status(
            &declared_status,
            started_at,
            owner.as_ref(),
            now,
            lease_secs,
        ),
        declared_status,
        started_at,
        completed_at: row.get(5)?,
        exit_code: row.get(6)?,
        cached: row.get::<_, i64>(7)? != 0,
        session_id: row.get(8)?,
    })
}

/// Where a unit ran, as joined from `dag_submissions` and `job_history`.
#[derive(Default)]
struct UnitPlace {
    cache_hit: Option<bool>,
    executor: Option<String>,
    endpoint: Option<String>,
    submission_id: Option<String>,
    node: Option<String>,
    outputs: Vec<String>,
}

impl UnitPlace {
    /// Columns 1.. of the `unit_views` query.
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let dag_executor: Option<String> = row.get(1)?;
        let dag_address: Option<String> = row.get(2)?;
        let history_executor: Option<String> = row.get(4)?;
        let history_host: Option<String> = row.get(5)?;
        let output_hashes: Option<String> = row.get(7)?;
        let executor = dag_executor.or(history_executor);
        let endpoint = dag_address
            .or_else(|| history_host.filter(|_| executor.as_deref().is_none_or(|e| e == "local")));
        let outputs = output_hashes
            .and_then(|json| {
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json).ok()
            })
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        Ok(Self {
            cache_hit: row.get(8)?,
            executor,
            endpoint,
            submission_id: row.get(3)?,
            node: row.get(6)?,
            outputs,
        })
    }
}

impl StateDb {
    /// Every job row, resolved through [`effective_status`].
    ///
    /// `run_id` scopes the result to one run when given. `now` is UNIX
    /// seconds and `lease_secs` the claim lease
    /// ([`lease_secs_from_env`] resolves the one readers should use).
    ///
    /// This is a **read-only** derivation: it never reclaims, and never
    /// writes. Reclaiming stays with `ox run`.
    ///
    /// ```
    /// # use tempfile::NamedTempFile;
    /// use ox_state::db::{StateDb, JobRecord};
    /// use ox_state::effective::{EffectiveCounts, lease_secs_from_env};
    ///
    /// let tmp = NamedTempFile::new().unwrap();
    /// let db = StateDb::open(tmp.path()).unwrap();
    /// let sid = db.create_session(1, "host", None).unwrap();
    /// db.register_jobs(&[JobRecord {
    ///     id: "j".into(), rule_name: "r".into(), wildcards: "{}".into(),
    ///     cache_key: None, run_id: None,
    /// }]).unwrap();
    /// db.claim_job("j", &sid).unwrap();
    ///
    /// // The owning session is interrupted: the row stops counting as running.
    /// db.interrupt_session(&sid).unwrap();
    /// let views = db.job_views(None, 1_000_000, lease_secs_from_env()).unwrap();
    /// let counts = EffectiveCounts::from_views(&views);
    /// assert_eq!(counts.running, 0);
    /// assert_eq!(counts.orphaned, 1);
    /// ```
    pub fn job_views(
        &self,
        run_id: Option<&str>,
        now: u64,
        lease_secs: u64,
    ) -> Result<Vec<JobView>, StateError> {
        let sql = match run_id {
            Some(_) => format!(
                "{RUN_JOBS} SELECT {VIEW_COLUMNS}
                FROM run_jobs j LEFT JOIN sessions s ON s.id = j.session_id"
            ),
            None => format!(
                "SELECT {VIEW_COLUMNS}
                FROM jobs j LEFT JOIN sessions s ON s.id = j.session_id"
            ),
        };
        let mut stmt = self.conn().prepare(&sql)?;
        let map = |row: &rusqlite::Row<'_>| row_to_view(row, now, lease_secs);
        let rows = match run_id {
            Some(rid) => stmt.query_map(rusqlite::params![rid], map)?,
            None => stmt.query_map([], map)?,
        };
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// One [`UnitView`] per job of `run_id`: [`StateDb::job_views`] joined
    /// with the run's DAG submission and the job's history row. Read-only.
    pub fn unit_views(
        &self,
        run_id: &str,
        now: u64,
        lease_secs: u64,
    ) -> Result<Vec<UnitView>, StateError> {
        let mut stmt = self.conn().prepare(&format!(
            "{RUN_JOBS}
             SELECT j.id, d.executor, d.dashboard_address, j.executor_submission_id,
                    h.executor, h.hostname, h.node, h.output_hashes, j.cached
             FROM run_jobs j
             LEFT JOIN dag_submissions d ON d.run_id = j.run_id
             LEFT JOIN job_history h ON h.run_id = j.run_id AND h.job_id = j.id
             WHERE j.run_id = ?1",
        ))?;
        let mut places = std::collections::HashMap::new();
        let rows = stmt.query_map(rusqlite::params![run_id], |row| {
            Ok((row.get::<_, String>(0)?, UnitPlace::from_row(row)?))
        })?;
        for row in rows {
            let (id, place) = row?;
            places.insert(id, place);
        }
        Ok(self
            .job_views(Some(run_id), now, lease_secs)?
            .into_iter()
            .map(|view| {
                let place = places.remove(&view.id).unwrap_or_default();
                UnitView {
                    view,
                    cache_hit: place.cache_hit,
                    executor: place.executor,
                    endpoint: place.endpoint,
                    submission_id: place.submission_id,
                    node: place.node,
                    outputs: place.outputs,
                }
            })
            .collect())
    }

    /// Aggregate [`EffectiveCounts`] over [`StateDb::job_views`].
    pub fn effective_job_counts(
        &self,
        run_id: Option<&str>,
        now: u64,
        lease_secs: u64,
    ) -> Result<EffectiveCounts, StateError> {
        Ok(EffectiveCounts::from_views(
            &self.job_views(run_id, now, lease_secs)?,
        ))
    }

    /// Pending jobs with their blocking upstreams, each carrying its
    /// effective status so a reader can say "waiting for X (orphaned)".
    pub fn pending_job_views(
        &self,
        run_id: Option<&str>,
        now: u64,
        lease_secs: u64,
    ) -> Result<Vec<PendingJobView>, StateError> {
        let pending: Vec<JobView> = self
            .job_views(run_id, now, lease_secs)?
            .into_iter()
            .filter(|v| v.effective == EffectiveStatus::Pending)
            .collect();

        let mut blocker_stmt = self.conn().prepare(&format!(
            "SELECT {VIEW_COLUMNS}
             FROM job_edges e
             JOIN jobs j ON j.id = e.from_job
             LEFT JOIN sessions s ON s.id = j.session_id
             WHERE e.to_job = ?1
               AND j.status NOT IN ('completed', 'skipped')"
        ))?;

        let mut out = Vec::new();
        for p in pending {
            let blockers = blocker_stmt
                .query_map(rusqlite::params![&p.id], |row| {
                    row_to_view(row, now, lease_secs)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            out.push(PendingJobView {
                id: p.id,
                rule_name: p.rule_name,
                wildcards: p.wildcards,
                waiting_for: blockers
                    .into_iter()
                    .map(|b| BlockingUpstream {
                        rule_name: b.rule_name,
                        effective: b.effective,
                    })
                    .collect(),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEASE: u64 = 90;
    const NOW: u64 = 1_000_000;

    fn owner(status: &str, heartbeat_at: u64) -> SessionLiveness {
        SessionLiveness {
            status: status.into(),
            heartbeat_at,
        }
    }

    #[test]
    fn old_run_views_survive_partial_and_full_reregistration() {
        use crate::db::JobRecord;
        for moved in [vec!["j"], vec!["j", "k"]] {
            let tmp = tempfile::NamedTempFile::new().unwrap();
            let db = StateDb::open(tmp.path()).unwrap();
            db.begin_run("run-1", None, 2, None).unwrap();
            let jobs = |run: &str, ids: &[&str]| {
                ids.iter()
                    .map(|id| JobRecord {
                        id: (*id).into(),
                        rule_name: (*id).into(),
                        wildcards: "{}".into(),
                        cache_key: None,
                        run_id: Some(run.into()),
                    })
                    .collect::<Vec<_>>()
            };
            db.register_jobs(&jobs("run-1", &["j", "k"])).unwrap();
            db.skip_job("j").unwrap();
            db.exclude_job("k").unwrap();
            db.conn()
                .execute_batch("UPDATE jobs SET executor_submission_id = 'submission-1';")
                .unwrap();
            db.finalize_job_history_with_executors(
                "run-1",
                "local",
                "",
                &Default::default(),
                &Default::default(),
                &Default::default(),
                &Default::default(),
                &Default::default(),
                &[("j".into(), "node-1".into()), ("k".into(), "node-2".into())].into(),
            )
            .unwrap();
            db.begin_run("run-2", None, moved.len(), None).unwrap();
            db.register_jobs(&jobs("run-2", &moved)).unwrap();
            db.conn()
                .execute(
                    "UPDATE jobs SET status='failed', cached=0, exit_code=7,
                executor_submission_id='submission-2' WHERE run_id='run-2'",
                    [],
                )
                .unwrap();
            let units = db.unit_views("run-1", NOW, LEASE).unwrap();
            assert_eq!(units.len(), 2, "moved {moved:?}");
            let j = units.iter().find(|u| u.view.id == "j").unwrap();
            let k = units.iter().find(|u| u.view.id == "k").unwrap();
            assert_eq!(j.view.effective, EffectiveStatus::Completed);
            assert!(j.view.cached);
            assert_eq!(j.cache_hit, Some(true));
            assert_eq!(j.view.exit_code, Some(0));
            assert_eq!(j.node.as_deref(), Some("node-1"));
            assert_eq!(j.submission_id.as_deref(), Some("submission-1"));
            assert_eq!(k.view.effective, EffectiveStatus::Skipped);
            assert!(!k.view.cached);
            assert_eq!(k.cache_hit, Some(false));
            assert_eq!(k.view.exit_code, None);
            assert_eq!(k.node.as_deref(), Some("node-2"));
            assert_eq!(db.job_views(Some("run-1"), NOW, LEASE).unwrap().len(), 2);
            assert_eq!(
                db.unit_views("run-2", NOW, LEASE).unwrap().len(),
                moved.len()
            );
            assert_eq!(db.job_views(None, NOW, LEASE).unwrap().len(), 2);
        }
    }

    #[test]
    fn history_preserves_all_terminal_states_and_live_rows_win() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();
        db.begin_run("run-1", None, 4, None).unwrap();
        db.conn()
            .execute_batch(
                "INSERT INTO jobs (id, rule_name, run_id, status, cached, exit_code)
            VALUES ('ok', 'r', 'run-1', 'completed', 0, 0),
                   ('bad', 'r', 'run-1', 'failed', 0, 7),
                   ('cancelled', 'r', 'run-1', 'cancelled', 0, NULL),
                   ('skipped', 'r', 'run-1', 'skipped', 0, NULL);",
            )
            .unwrap();
        db.finalize_job_history(
            "run-1",
            "local",
            "",
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
        db.conn()
            .execute_batch("UPDATE jobs SET status='pending' WHERE id='ok';")
            .unwrap();
        let live = db.job_views(Some("run-1"), NOW, LEASE).unwrap();
        assert_eq!(live.len(), 4);
        assert_eq!(
            live.iter().find(|v| v.id == "ok").unwrap().effective,
            EffectiveStatus::Pending
        );
        db.begin_run("run-2", None, 4, None).unwrap();
        db.conn()
            .execute_batch("UPDATE jobs SET run_id='run-2';")
            .unwrap();
        let units = db.unit_views("run-1", NOW, LEASE).unwrap();
        assert_eq!(units.len(), 4);
        for (id, expected, exit) in [
            ("ok", "completed", Some(0)),
            ("bad", "failed", Some(7)),
            ("cancelled", "cancelled", None),
            ("skipped", "skipped", None),
        ] {
            let unit = units.iter().find(|unit| unit.view.id == id).unwrap();
            assert_eq!(unit.view.effective.as_str(), expected);
            assert_eq!(unit.view.exit_code, exit);
            assert_eq!(unit.cache_hit, Some(false));
        }
    }

    #[test]
    fn active_and_fresh_is_running() {
        let o = owner("active", NOW - 1);
        assert_eq!(
            effective_status("running", Some(NOW - 30), Some(&o), NOW, LEASE),
            EffectiveStatus::Running
        );
    }

    #[test]
    fn active_but_stale_is_orphaned_heartbeat_stale() {
        let o = owner("active", NOW - LEASE - 1);
        assert_eq!(
            effective_status("running", Some(NOW - 700), Some(&o), NOW, LEASE),
            EffectiveStatus::Orphaned {
                since: Some(NOW - 700),
                reason: OrphanReason::HeartbeatStale,
            }
        );
    }

    #[test]
    fn heartbeat_exactly_one_lease_old_is_stale() {
        // Same boundary as the claim protocol: `now - heartbeat < lease`.
        let o = owner("active", NOW - LEASE);
        assert!(!o.is_live(NOW, LEASE));
        assert!(effective_status("running", None, Some(&o), NOW, LEASE).is_orphaned());
    }

    #[test]
    fn interrupted_session_is_orphaned_even_with_a_fresh_heartbeat() {
        let o = owner("interrupted", NOW);
        assert_eq!(
            effective_status("running", Some(NOW - 10), Some(&o), NOW, LEASE),
            EffectiveStatus::Orphaned {
                since: Some(NOW - 10),
                reason: OrphanReason::SessionInterrupted,
            }
        );
    }

    #[test]
    fn completed_session_with_a_running_row_is_orphaned() {
        let o = owner("completed", NOW);
        assert_eq!(
            effective_status("running", Some(NOW - 5), Some(&o), NOW, LEASE),
            EffectiveStatus::Orphaned {
                since: Some(NOW - 5),
                reason: OrphanReason::SessionCompleted,
            }
        );
    }

    #[test]
    fn running_row_without_a_session_is_orphaned() {
        assert_eq!(
            effective_status("running", None, None, NOW, LEASE),
            EffectiveStatus::Orphaned {
                since: None,
                reason: OrphanReason::SessionMissing,
            }
        );
    }

    #[test]
    fn non_running_statuses_are_carried_verbatim() {
        // A dead owner never rewrites a terminal status.
        let o = owner("interrupted", 0);
        for (declared, expected) in [
            ("pending", EffectiveStatus::Pending),
            ("completed", EffectiveStatus::Completed),
            ("failed", EffectiveStatus::Failed),
            ("skipped", EffectiveStatus::Skipped),
            ("cancelled", EffectiveStatus::Cancelled),
        ] {
            assert_eq!(
                effective_status(declared, None, Some(&o), NOW, LEASE),
                expected
            );
        }
    }

    #[test]
    fn unknown_declared_status_round_trips() {
        let s = effective_status("gated", None, None, NOW, LEASE);
        assert_eq!(s, EffectiveStatus::Other("gated".into()));
        assert_eq!(s.as_str(), "gated");
    }

    #[test]
    fn orphaned_renders_as_orphaned_not_running() {
        let s = effective_status("running", None, None, NOW, LEASE);
        assert_eq!(s.as_str(), "orphaned");
        assert!(!s.is_running());
        assert_eq!(s.orphan_reason(), Some(OrphanReason::SessionMissing));
    }

    #[test]
    fn reason_tokens_are_stable() {
        assert_eq!(
            OrphanReason::SessionInterrupted.as_str(),
            "session_interrupted"
        );
        assert_eq!(OrphanReason::HeartbeatStale.as_str(), "heartbeat_stale");
        assert_eq!(OrphanReason::SessionCompleted.as_str(), "session_completed");
        assert_eq!(OrphanReason::SessionMissing.as_str(), "session_missing");
        assert!(!OrphanReason::HeartbeatStale.describe().is_empty());
    }

    #[test]
    fn lease_from_env_defaults_when_unset_or_invalid() {
        // The env var is process-global; only assert the pure fallback path
        // through the same filter the resolver applies.
        let parse = |v: &str| {
            v.trim()
                .parse::<u64>()
                .ok()
                .filter(|s| *s > 0)
                .unwrap_or(DEFAULT_LEASE_SECS)
        };
        assert_eq!(parse("5"), 5);
        assert_eq!(parse("0"), DEFAULT_LEASE_SECS);
        assert_eq!(parse("nonsense"), DEFAULT_LEASE_SECS);
    }

    // -- Ledger-backed views (seeded state.db) --

    use crate::db::{JobRecord, StateDb};
    use tempfile::NamedTempFile;

    fn seeded() -> (NamedTempFile, StateDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();
        db.register_jobs(&[
            JobRecord {
                id: "substrate_tests".into(),
                rule_name: "substrate_tests".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "repro_spheres".into(),
                rule_name: "repro_spheres".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ])
        .unwrap();
        db.register_edges(&[("substrate_tests".into(), "repro_spheres".into())])
            .unwrap();
        (tmp, db)
    }

    #[test]
    fn interrupted_session_leaves_an_orphaned_row_not_a_running_one() {
        let (_tmp, db) = seeded();
        let sid = db.create_session(1, "host", None).unwrap();
        db.claim_job("substrate_tests", &sid).unwrap();
        db.interrupt_session(&sid).unwrap();

        let views = db.job_views(None, unix_now_for_test(), LEASE).unwrap();
        let counts = EffectiveCounts::from_views(&views);
        assert_eq!(counts.running, 0);
        assert_eq!(counts.orphaned, 1);
        assert_eq!(counts.pending, 1);
        assert_eq!(counts.total(), 2);

        let orphan = views.iter().find(|v| v.id == "substrate_tests").unwrap();
        assert_eq!(orphan.declared_status, "running");
        assert_eq!(orphan.effective.as_str(), "orphaned");
        assert_eq!(
            orphan.effective.orphan_reason(),
            Some(OrphanReason::SessionInterrupted)
        );
        assert_eq!(orphan.session_id.as_deref(), Some(sid.as_str()));
    }

    #[test]
    fn a_live_session_keeps_its_row_running() {
        let (_tmp, db) = seeded();
        let sid = db.create_session(1, "host", None).unwrap();
        db.claim_job("substrate_tests", &sid).unwrap();

        let counts = db
            .effective_job_counts(None, unix_now_for_test(), LEASE)
            .unwrap();
        assert_eq!(counts.running, 1);
        assert_eq!(counts.orphaned, 0);
    }

    #[test]
    fn a_stale_heartbeat_orphans_the_row_without_writing_to_it() {
        let (_tmp, db) = seeded();
        let sid = db.create_session(1, "host", None).unwrap();
        db.claim_job("substrate_tests", &sid).unwrap();

        // Read far enough in the future that the heartbeat is past the lease.
        let later = unix_now_for_test() + LEASE + 10;
        let counts = db.effective_job_counts(None, later, LEASE).unwrap();
        assert_eq!(counts.orphaned, 1);
        assert_eq!(counts.running, 0);

        // The derivation is read-only: the declared row is untouched.
        assert_eq!(
            db.job_status("substrate_tests").unwrap().as_deref(),
            Some("running")
        );
    }

    #[test]
    fn pending_views_report_an_orphaned_upstream() {
        let (_tmp, db) = seeded();
        let sid = db.create_session(1, "host", None).unwrap();
        db.claim_job("substrate_tests", &sid).unwrap();
        db.interrupt_session(&sid).unwrap();

        let pending = db
            .pending_job_views(None, unix_now_for_test(), LEASE)
            .unwrap();
        assert_eq!(pending.len(), 1);
        let p = &pending[0];
        assert_eq!(p.id, "repro_spheres");
        assert_eq!(p.waiting_for.len(), 1);
        assert_eq!(p.waiting_for[0].rule_name, "substrate_tests");
        assert!(p.waiting_for[0].effective.is_orphaned());
        assert_eq!(p.orphaned_upstreams(), vec!["substrate_tests"]);
    }

    #[test]
    fn views_can_be_scoped_to_a_run() {
        let (_tmp, db) = seeded();
        let views = db
            .job_views(Some("run-absent"), unix_now_for_test(), LEASE)
            .unwrap();
        assert!(views.is_empty());
    }

    fn unix_now_for_test() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}
