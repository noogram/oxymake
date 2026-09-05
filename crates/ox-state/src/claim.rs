//! [`JobClaim`] backed by the `jobs` and `sessions` tables — the scheduling
//! gate of the cooperative multi-session protocol (ADR-012, issue #3).
//!
//! `ox-core` defines the [`JobClaim`] trait and ships only the permissive
//! [`NoClaims`](ox_core::traits::claim::NoClaims). [`StateJobClaimer`] is the
//! implementation `ox run` hands to the scheduler so that, before a job is
//! dispatched, the session asks `state.db` whether a peer already owns it.
//!
//! # Ownership and lease
//!
//! A job row is *owned* by the session recorded in `jobs.session_id`. The
//! owner is **live** while its session row is `active` and its heartbeat is
//! younger than the lease (`lease_secs`). `ox run` heartbeats every third of
//! the lease; a session that stops heartbeating — `kill -9`, power loss —
//! goes stale after one lease and its `running` rows are reclaimed by the
//! first peer that looks at them ([`StateDb::reclaim_stale_jobs_if_stale`],
//! the same reclaim `ox clean` uses). The reclaim re-checks the heartbeat
//! inside its own transaction: what a peer *observed* only decides whether
//! to try, the row's heartbeat at write time decides whether it happens.
//! There is exactly one lease, the heartbeat one.
//!
//! # What a losing session sees
//!
//! [`StateDb::peer_job_state`] maps a row to the scheduler's
//! [`PeerJobState`]:
//!
//! | row status                    | owner live | result                        |
//! |-------------------------------|------------|-------------------------------|
//! | `pending`                     | —          | `Unclaimed`                   |
//! | `running`                     | yes        | `Running` (keep waiting)      |
//! | `running`                     | no         | reclaim → `Unclaimed`         |
//! | `completed` (incl. cached)    | any        | `Completed`                   |
//! | `failed` / `cancelled`        | yes        | `Failed` / `Cancelled`        |
//! | `failed` / `cancelled`        | no         | reset row → `Unclaimed`       |
//!
//! A completion recorded after this session started is consumed whoever
//! recorded it — the outputs are on disk. A failure or cancellation is only
//! mirrored while its author is live: a verdict left behind by a finished
//! or dead session is history, not a peer's decision, so the row is reset
//! and the job re-run here.
//!
//! # Fresh runs and old rows
//!
//! `register_jobs` keeps the status of rows that already exist, so without
//! further care a job `completed` by *yesterday's* run would make today's
//! claim lose and today's session consume a stale completion instead of
//! re-evaluating the job. A completion is therefore consumed only while its
//! author is live or when it was recorded strictly after this session
//! started (in seconds; the same second counts as history); an older one is
//! history and the claim resets the row and wins. This happens
//! lazily, on the claim of a job that is about to execute — a cache hit
//! never claims, so yesterday's rows of a warm run are not touched.
//! [`StateDb::reset_inactive_job_rows`] runs right after registration for
//! the other stale statuses (`running`, `failed`, `cancelled` left by a
//! session that is not live), so that a dead peer's verdicts are not
//! mirrored. Rows owned by a live peer are left alone — that peer is the
//! concurrent session this protocol exists for.
//!
//! # Failure policy
//!
//! A database error while claiming yields `Won`: the local executor's
//! per-output-path locks still fail closed on a real double execution, and
//! a single-session run must not stall on a transient SQLite error. A
//! database error while observing a peer yields `Running` (keep waiting and
//! retry on the next poll); both are logged through `tracing`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ox_core::model::JobId;
use ox_core::traits::claim::{ClaimOutcome, JobClaim, PeerJobState};
use rusqlite::OptionalExtension;

use crate::db::StateDb;
use crate::error::StateError;

/// Default lease of a session's claims, in seconds: a session whose
/// heartbeat is older than this is considered dead and its running jobs
/// may be reclaimed. `ox run` heartbeats every [`heartbeat_interval`].
pub const DEFAULT_LEASE_SECS: u64 = 90;

/// Environment variable overriding [`DEFAULT_LEASE_SECS`] for `ox run`
/// (integration tests use it to make lease expiry observable in seconds).
pub const LEASE_ENV_VAR: &str = "OX_SESSION_LEASE_SECS";

/// Heartbeat period for a session holding claims under `lease_secs`: a third
/// of the lease, at least one second, so two consecutive heartbeats can be
/// missed before the session reads as stale.
pub fn heartbeat_interval(lease_secs: u64) -> std::time::Duration {
    std::time::Duration::from_secs((lease_secs / 3).max(1))
}

/// What the `jobs` row of a job and its owner's session row say.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JobRow {
    status: String,
    session_id: Option<String>,
    exit_code: Option<i32>,
    /// When the row reached its terminal status (UNIX seconds).
    completed_at: Option<u64>,
    /// `Some` when the owning session row exists: `(status, heartbeat_at)`.
    owner: Option<(String, u64)>,
}

impl JobRow {
    /// The owner is alive: an `active` session whose heartbeat is younger
    /// than the lease.
    fn owner_live(&self, lease_secs: u64, now: u64) -> bool {
        match &self.owner {
            Some((status, heartbeat_at)) => {
                status == "active" && now.saturating_sub(*heartbeat_at) < lease_secs
            }
            None => false,
        }
    }
}

impl StateDb {
    /// Read a job row together with its owner's session status and heartbeat.
    fn job_row(&self, job_id: &str) -> Result<Option<JobRow>, StateError> {
        let row = self
            .conn()
            .query_row(
                "SELECT j.status, j.session_id, j.exit_code, j.completed_at,
                        s.status, s.heartbeat_at
                 FROM jobs j LEFT JOIN sessions s ON s.id = j.session_id
                 WHERE j.id = ?1",
                rusqlite::params![job_id],
                |row| {
                    let owner_status: Option<String> = row.get(4)?;
                    let owner_heartbeat: Option<u64> = row.get(5)?;
                    Ok(JobRow {
                        status: row.get(0)?,
                        session_id: row.get(1)?,
                        exit_code: row.get(2)?,
                        completed_at: row.get(3)?,
                        owner: owner_status.zip(owner_heartbeat),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Reset one row to `pending`, guarded by the status and owner that
    /// were observed so a concurrent transition is never overwritten.
    /// Returns whether the reset happened.
    fn reset_job_row(&self, job_id: &str, observed: &JobRow) -> Result<bool, StateError> {
        let rows = self.conn().execute(
            "UPDATE jobs SET status = 'pending', session_id = NULL, locked_by = NULL,
                             started_at = NULL, completed_at = NULL, exit_code = NULL,
                             output_hashes = NULL, cached = 0
             WHERE id = ?1 AND status = ?2 AND session_id IS ?3",
            rusqlite::params![job_id, observed.status, observed.session_id],
        )?;
        Ok(rows > 0)
    }

    /// Make the row of `job_id` claimable again if its owner is not live:
    /// a `running` row is reclaimed together with the rest of the dead
    /// owner's running rows, a `failed` / `cancelled` row is reset. Rows
    /// owned by a live session, `pending` rows and `completed` rows are
    /// left alone (completions are handled by
    /// [`claim_job_for_session`](StateDb::claim_job_for_session)). Returns
    /// whether the row is now `pending`.
    ///
    /// `row` is what the caller *observed*; the decision to reclaim is
    /// taken again inside the reclaim transaction on the current heartbeat
    /// ([`StateDb::reclaim_stale_jobs_if_stale`]), so an owner that
    /// heartbeated between the observation and this call keeps its rows
    /// and this returns `false`.
    fn release_if_owner_dead(
        &self,
        job_id: &str,
        row: &JobRow,
        lease_secs: u64,
    ) -> Result<bool, StateError> {
        let now = unix_now();
        match row.status.as_str() {
            "pending" => Ok(true),
            _ if row.owner_live(lease_secs, now) => Ok(false),
            "running" => match &row.session_id {
                Some(owner) => {
                    let cutoff = now.saturating_sub(lease_secs);
                    Ok(self.reclaim_stale_jobs_if_stale(owner, cutoff)? > 0)
                }
                None => self.reset_job_row(job_id, row),
            },
            "completed" | "skipped" => Ok(false),
            _ => self.reset_job_row(job_id, row),
        }
    }

    /// Claim `job_id` for `session_id` as the scheduling gate.
    ///
    /// Wins the plain [`claim_job`](StateDb::claim_job) CAS when the row is
    /// `pending`; otherwise:
    ///
    /// - a row this session already owns is `Won` again (idempotent claim,
    ///   and a `failed` row of ours goes back to `running` for a retry);
    /// - a `completed` row is history when its author is not live and it
    ///   was not recorded strictly after this session started: the row is
    ///   reset and the CAS retried once. A completion by a live peer, or
    ///   recorded after this session started, is `Lost` and consumed by
    ///   the waiter;
    /// - a `running` / `failed` / `cancelled` row whose owner is not live
    ///   any more is released (reclaimed or reset, see the module docs) and
    ///   the CAS retried once;
    /// - a row owned by a live peer, or terminalized by one, is `Lost`.
    pub fn claim_job_for_session(
        &self,
        job_id: &str,
        session_id: &str,
        lease_secs: u64,
    ) -> Result<ClaimOutcome, StateError> {
        if self.claim_job(job_id, session_id)? {
            return Ok(ClaimOutcome::Won);
        }
        let Some(row) = self.job_row(job_id)? else {
            // Not registered: nothing to coordinate on, run it.
            return Ok(ClaimOutcome::Won);
        };
        if row.session_id.as_deref() == Some(session_id) {
            match row.status.as_str() {
                "running" => return Ok(ClaimOutcome::Won),
                "failed" => {
                    // Retry of our own failed attempt (ErrorStrategy::Retry).
                    let now = unix_now();
                    self.conn().execute(
                        "UPDATE jobs SET status = 'running', started_at = ?1, completed_at = NULL,
                                         exit_code = NULL
                         WHERE id = ?2 AND session_id = ?3 AND status = 'failed'",
                        rusqlite::params![now, job_id, session_id],
                    )?;
                    return Ok(ClaimOutcome::Won);
                }
                _ => {}
            }
        }
        let released = match row.status.as_str() {
            "completed" | "skipped" => {
                self.completion_is_history(&row, session_id, lease_secs)?
                    && self.reset_job_row(job_id, &row)?
            }
            _ => self.release_if_owner_dead(job_id, &row, lease_secs)?,
        };
        if released && self.claim_job(job_id, session_id)? {
            return Ok(ClaimOutcome::Won);
        }
        // Re-read: the owner may have changed under us.
        let owner = self
            .job_row(job_id)?
            .and_then(|r| r.session_id)
            .or(row.session_id);
        Ok(ClaimOutcome::Lost { owner })
    }

    /// A `completed` row is *history* for `session_id` — a verdict of an
    /// earlier run to re-evaluate, not a peer's result to consume — unless
    /// its author is live or it was recorded strictly after this session
    /// started (UNIX seconds). A row completed in the same second as the
    /// session start is history: two runs of one script can start and
    /// finish within a second, and consuming a completion whose outputs
    /// were removed in between would report a job done that was not run.
    /// The cost of the strict comparison is one wasted re-execution when a
    /// peer completes and exits within that second.
    fn completion_is_history(
        &self,
        row: &JobRow,
        session_id: &str,
        lease_secs: u64,
    ) -> Result<bool, StateError> {
        if row.owner_live(lease_secs, unix_now()) {
            return Ok(false);
        }
        let started_at: Option<u64> = self
            .conn()
            .query_row(
                "SELECT started_at FROM sessions WHERE id = ?1",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match (row.completed_at, started_at) {
            (Some(completed_at), Some(started_at)) => completed_at <= started_at,
            _ => true,
        })
    }

    /// Observe a job owned by a peer — see the table in the module docs.
    pub fn peer_job_state(
        &self,
        job_id: &str,
        lease_secs: u64,
    ) -> Result<PeerJobState, StateError> {
        let Some(row) = self.job_row(job_id)? else {
            return Ok(PeerJobState::Unclaimed);
        };
        let live = row.owner_live(lease_secs, unix_now());
        Ok(match row.status.as_str() {
            "pending" => PeerJobState::Unclaimed,
            "completed" | "skipped" => PeerJobState::Completed,
            "running" if live => PeerJobState::Running,
            "failed" if live => PeerJobState::Failed {
                exit_code: row.exit_code.filter(|c| *c != 0).unwrap_or(1),
            },
            "cancelled" if live => PeerJobState::Cancelled,
            _ => {
                // running / failed / cancelled with a dead (or no) owner.
                self.release_if_owner_dead(job_id, &row, lease_secs)?;
                PeerJobState::Unclaimed
            }
        })
    }

    /// Reset the `running`, `failed` and `cancelled` rows of `job_ids`
    /// that a previous, no longer live session left behind, so a fresh run
    /// re-evaluates them instead of mirroring stale verdicts. Rows owned by
    /// a live session are kept for the claim protocol. `completed` rows are
    /// left alone: a cache hit never claims, and a job that does execute
    /// resets yesterday's completion lazily in
    /// [`claim_job_for_session`](StateDb::claim_job_for_session) — resetting
    /// every completed row of a 1001-job graph at run start turned its 999
    /// cache-hit `skip_job` writes from no-ops into fsynced autocommits,
    /// about 100 ms per warm run (issue #3, round-1 QA finding 4).
    /// Returns the number of rows made `pending`.
    ///
    /// One `BEGIN IMMEDIATE` transaction for the whole run: the `running`
    /// rows of each dead owner are reclaimed through the guarded reclaim
    /// (which also closes that session), every other stale row is reset by
    /// one set-based `UPDATE` per chunk of ids.
    pub fn reset_inactive_job_rows(
        &self,
        job_ids: &[String],
        lease_secs: u64,
    ) -> Result<usize, StateError> {
        if job_ids.is_empty() {
            return Ok(0);
        }
        let cutoff = unix_now().saturating_sub(lease_secs);
        let conn = self.conn();
        conn.execute_batch("BEGIN IMMEDIATE")?;
        match reset_inactive_job_rows_in(conn, job_ids, cutoff) {
            Ok(reset) => {
                conn.execute_batch("COMMIT")?;
                Ok(reset)
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

/// Bound variables per statement, well under SQLite's default limit.
const ID_CHUNK: usize = 500;

/// Body of [`StateDb::reset_inactive_job_rows`], inside the caller's
/// transaction. `?1` is the cutoff, the ids follow.
fn reset_inactive_job_rows_in(
    conn: &rusqlite::Connection,
    job_ids: &[String],
    cutoff: u64,
) -> Result<usize, StateError> {
    let mut reset = 0;
    for chunk in job_ids.chunks(ID_CHUNK) {
        let placeholders = (0..chunk.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(",");
        let params: Vec<&dyn rusqlite::ToSql> = std::iter::once(&cutoff as &dyn rusqlite::ToSql)
            .chain(chunk.iter().map(|id| id as &dyn rusqlite::ToSql))
            .collect();

        // Dead owners of `running` rows: reclaim all their running rows
        // (as the claim path does) and close their session.
        let owners: Vec<String> = {
            let mut stmt = conn.prepare(&format!(
                "SELECT DISTINCT session_id FROM jobs
                 WHERE id IN ({placeholders}) AND status = 'running'
                   AND session_id IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM sessions s
                                   WHERE s.id = jobs.session_id AND s.status = 'active'
                                     AND s.heartbeat_at > ?1)"
            ))?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        for owner in &owners {
            reset += crate::session::reclaim_jobs_unless_live_in(conn, owner, cutoff)?;
        }

        // Every other non-pending row without a live owner (a running row
        // with no session, a verdict or cache hit of a session that is
        // gone) is reset in one statement.
        reset += conn.execute(
            &format!(
                "UPDATE jobs SET status = 'pending', session_id = NULL, locked_by = NULL,
                                 started_at = NULL, completed_at = NULL, exit_code = NULL,
                                 output_hashes = NULL, cached = 0
                 WHERE id IN ({placeholders})
                   AND status NOT IN ('pending', 'completed', 'skipped')
                   AND NOT EXISTS (SELECT 1 FROM sessions s
                                   WHERE s.id = jobs.session_id AND s.status = 'active'
                                     AND s.heartbeat_at > ?1)"
            ),
            rusqlite::params_from_iter(params.iter()),
        )?;
    }
    Ok(reset)
}

/// Claimer that reads and writes the `jobs` / `sessions` tables of `state.db`.
///
/// Holds its own [`StateDb`] connection behind a mutex (a `rusqlite`
/// connection is `Send` but not `Sync`); every operation is a few short
/// SQLite statements executed without awaiting, so the lock is never held
/// across a suspension point.
///
/// # Clock assumption
///
/// The lease is measured on the wall clock (UNIX seconds): the owner's
/// heartbeat stores its `now`, and this claimer compares that value against
/// *its* `now`. Both are assumed to come from one non-decreasing clock — the
/// system clock when every session runs on one host, NTP-synchronised
/// clocks when hosts share a `state.db`. A skew or backward clock step
/// larger than the lease makes a live owner read as dead, or a dead one as
/// live, for one lease; nothing detects it. The reclaim re-checks the
/// heartbeat inside its transaction ([`StateDb::reclaim_stale_jobs_if_stale`]),
/// which closes the read-then-reclaim race but not a wrong clock.
///
/// A session that stops heartbeating because its process is suspended
/// (laptop sleep, `SIGSTOP`) is reclaimed like a crashed one. On resume it
/// fails closed: [`StateDb::heartbeat`] only touches an `active` row and
/// [`StateDb::complete_job`] / [`StateDb::fail_job`] update zero rows once
/// a peer has re-claimed the job, so the resumed owner never overwrites a
/// peer's result (ADR-012, "Suspend / resume").
pub struct StateJobClaimer {
    db: Mutex<StateDb>,
    session_id: String,
    lease_secs: u64,
}

impl StateJobClaimer {
    /// Create a claimer acting for `session_id` under `lease_secs`.
    pub fn new(db: StateDb, session_id: String, lease_secs: u64) -> Self {
        Self {
            db: Mutex::new(db),
            session_id,
            lease_secs,
        }
    }

    fn with_db<T>(&self, f: impl FnOnce(&StateDb) -> T) -> T {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(poisoned) => poisoned.into_inner(),
        };
        f(&db)
    }

    fn claim_sync(&self, job_id: &JobId) -> ClaimOutcome {
        self.with_db(|db| {
            match db.claim_job_for_session(job_id.as_str(), &self.session_id, self.lease_secs) {
                Ok(outcome) => outcome,
                Err(e) => {
                    tracing::error!(
                        target: "ox.state.claim",
                        job_id = %job_id,
                        error = %e,
                        "claim failed; executing without coordination (output locks still apply)"
                    );
                    ClaimOutcome::Won
                }
            }
        })
    }

    fn peer_state_sync(&self, job_id: &JobId) -> PeerJobState {
        self.with_db(
            |db| match db.peer_job_state(job_id.as_str(), self.lease_secs) {
                Ok(state) => state,
                Err(e) => {
                    tracing::error!(
                        target: "ox.state.claim",
                        job_id = %job_id,
                        error = %e,
                        "peer state lookup failed; keeping the job waiting"
                    );
                    PeerJobState::Running
                }
            },
        )
    }
}

impl JobClaim for StateJobClaimer {
    fn claim<'a>(
        &'a self,
        job_id: &'a JobId,
    ) -> Pin<Box<dyn Future<Output = ClaimOutcome> + Send + 'a>> {
        Box::pin(async move { self.claim_sync(job_id) })
    }

    fn peer_state<'a>(
        &'a self,
        job_id: &'a JobId,
    ) -> Pin<Box<dyn Future<Output = PeerJobState> + Send + 'a>> {
        Box::pin(async move { self.peer_state_sync(job_id) })
    }
}

/// Current UNIX timestamp in seconds.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::JobRecord;
    use tempfile::NamedTempFile;

    const LEASE: u64 = 60;

    fn db_with_job(job: &str) -> (NamedTempFile, StateDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();
        db.register_jobs(&[JobRecord {
            id: job.into(),
            rule_name: "r".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }])
        .unwrap();
        (tmp, db)
    }

    fn age_heartbeat(db: &StateDb, session: &str, secs: u64) {
        let past = unix_now() - secs;
        db.conn()
            .execute(
                "UPDATE sessions SET heartbeat_at = ?1 WHERE id = ?2",
                rusqlite::params![past, session],
            )
            .unwrap();
    }

    #[test]
    fn first_claim_wins_second_session_loses_to_the_owner() {
        let (_tmp, db) = db_with_job("j");
        let s1 = db.create_session(1, "h", None).unwrap();
        let s2 = db.create_session(2, "h", None).unwrap();

        assert_eq!(
            db.claim_job_for_session("j", &s1, LEASE).unwrap(),
            ClaimOutcome::Won
        );
        assert_eq!(
            db.claim_job_for_session("j", &s2, LEASE).unwrap(),
            ClaimOutcome::Lost {
                owner: Some(s1.clone())
            }
        );
        assert_eq!(
            db.peer_job_state("j", LEASE).unwrap(),
            PeerJobState::Running
        );
    }

    #[test]
    fn claim_is_idempotent_for_the_owner() {
        let (_tmp, db) = db_with_job("j");
        let s1 = db.create_session(1, "h", None).unwrap();
        assert_eq!(
            db.claim_job_for_session("j", &s1, LEASE).unwrap(),
            ClaimOutcome::Won
        );
        assert_eq!(
            db.claim_job_for_session("j", &s1, LEASE).unwrap(),
            ClaimOutcome::Won
        );
        assert_eq!(db.job_status("j").unwrap().as_deref(), Some("running"));
    }

    #[test]
    fn owner_retries_its_own_failed_attempt() {
        let (_tmp, db) = db_with_job("j");
        let s1 = db.create_session(1, "h", None).unwrap();
        db.claim_job_for_session("j", &s1, LEASE).unwrap();
        assert!(db.fail_job("j", &s1, 2).unwrap());
        assert_eq!(
            db.claim_job_for_session("j", &s1, LEASE).unwrap(),
            ClaimOutcome::Won
        );
        assert_eq!(db.job_status("j").unwrap().as_deref(), Some("running"));
    }

    #[test]
    fn loser_consumes_completion_failure_and_cancellation_of_a_live_owner() {
        let (_tmp, db) = db_with_job("j");
        let s1 = db.create_session(1, "h", None).unwrap();
        db.claim_job_for_session("j", &s1, LEASE).unwrap();
        db.complete_job("j", &s1, 0, "").unwrap();
        assert_eq!(
            db.peer_job_state("j", LEASE).unwrap(),
            PeerJobState::Completed
        );

        let (_tmp, db) = db_with_job("k");
        let s1 = db.create_session(1, "h", None).unwrap();
        db.claim_job_for_session("k", &s1, LEASE).unwrap();
        db.fail_job("k", &s1, 3).unwrap();
        assert_eq!(
            db.peer_job_state("k", LEASE).unwrap(),
            PeerJobState::Failed { exit_code: 3 }
        );

        let (_tmp, db) = db_with_job("l");
        let s1 = db.create_session(1, "h", None).unwrap();
        db.claim_job_for_session("l", &s1, LEASE).unwrap();
        db.cancel_job_ids(&["l".into()]).unwrap();
        assert_eq!(
            db.peer_job_state("l", LEASE).unwrap(),
            PeerJobState::Cancelled
        );
    }

    #[test]
    fn cached_completion_counts_as_completed() {
        let (_tmp, db) = db_with_job("j");
        let s2 = db.create_session(2, "h", None).unwrap();
        assert!(db.skip_job("j").unwrap());
        assert_eq!(
            db.peer_job_state("j", LEASE).unwrap(),
            PeerJobState::Completed
        );
        // Recorded by a peer after this session started: consumed.
        db.conn()
            .execute(
                "UPDATE jobs SET completed_at = (SELECT started_at + 1 FROM sessions WHERE id = ?1)
                 WHERE id = 'j'",
                rusqlite::params![s2],
            )
            .unwrap();
        assert!(matches!(
            db.claim_job_for_session("j", &s2, LEASE).unwrap(),
            ClaimOutcome::Lost { .. }
        ));
        // Recorded at or before this session's start with no live owner
        // (yesterday's cache hit): history, the claim resets it and wins.
        db.conn()
            .execute(
                "UPDATE jobs SET completed_at = (SELECT started_at FROM sessions WHERE id = ?1)
                 WHERE id = 'j'",
                rusqlite::params![s2],
            )
            .unwrap();
        assert_eq!(
            db.claim_job_for_session("j", &s2, LEASE).unwrap(),
            ClaimOutcome::Won
        );
    }

    #[test]
    fn stale_owner_is_reclaimed_and_the_waiter_claims() {
        let (_tmp, db) = db_with_job("j");
        let s1 = db.create_session(1, "h", None).unwrap();
        let s2 = db.create_session(2, "h", None).unwrap();
        db.claim_job_for_session("j", &s1, LEASE).unwrap();
        assert!(matches!(
            db.claim_job_for_session("j", &s2, LEASE).unwrap(),
            ClaimOutcome::Lost { .. }
        ));

        // Heartbeat one second younger than the lease: still live.
        age_heartbeat(&db, &s1, LEASE - 1);
        assert_eq!(
            db.peer_job_state("j", LEASE).unwrap(),
            PeerJobState::Running
        );

        // Lease expired: the poll reclaims and the waiter wins the next claim.
        age_heartbeat(&db, &s1, LEASE);
        assert_eq!(
            db.peer_job_state("j", LEASE).unwrap(),
            PeerJobState::Unclaimed
        );
        assert_eq!(
            db.claim_job_for_session("j", &s2, LEASE).unwrap(),
            ClaimOutcome::Won
        );
        // The dead owner's session is closed, and its late terminal write
        // is rejected by the zombie guard.
        assert!(db.active_sessions().unwrap().iter().all(|s| s.id != s1));
        assert!(!db.complete_job("j", &s1, 0, "").unwrap());
    }

    #[test]
    fn heartbeat_between_observation_and_reclaim_keeps_the_live_owner() {
        // Round-1 QA finding 1 (issue #3): the waiter observes a stale
        // heartbeat, then the owner heartbeats, then the waiter reclaims.
        // The reclaim must decide on the row's own heartbeat inside its
        // transaction, so the now-live owner keeps its running rows.
        let (tmp, owner_db) = db_with_job("j");
        let waiter_db = StateDb::open(tmp.path()).unwrap();
        let s1 = owner_db.create_session(1, "h", None).unwrap();
        let s2 = owner_db.create_session(2, "h", None).unwrap();
        owner_db.claim_job_for_session("j", &s1, LEASE).unwrap();
        age_heartbeat(&owner_db, &s1, LEASE + 5);

        // The waiter reads the row: the owner looks dead.
        let observed = waiter_db.job_row("j").unwrap().unwrap();
        assert!(!observed.owner_live(LEASE, unix_now()));

        // The owner's heartbeat lands before the waiter acts on what it saw.
        owner_db.heartbeat(&s1).unwrap();

        // The reclaim must not go through on the stale observation.
        assert!(
            !waiter_db
                .release_if_owner_dead("j", &observed, LEASE)
                .unwrap()
        );
        assert_eq!(
            waiter_db.claim_job_for_session("j", &s2, LEASE).unwrap(),
            ClaimOutcome::Lost {
                owner: Some(s1.clone())
            }
        );
        assert_eq!(
            owner_db.job_status("j").unwrap().as_deref(),
            Some("running")
        );
        assert!(
            owner_db
                .active_sessions()
                .unwrap()
                .iter()
                .any(|s| s.id == s1)
        );
        // The owner's terminal write still lands.
        assert!(owner_db.complete_job("j", &s1, 0, "").unwrap());
    }

    #[test]
    fn reclaim_if_stale_decides_on_the_row_heartbeat_in_one_transaction() {
        let (_tmp, db) = db_with_job("j");
        let s1 = db.create_session(1, "h", None).unwrap();
        db.claim_job_for_session("j", &s1, LEASE).unwrap();
        let now = unix_now();

        // Live owner (heartbeat younger than the cutoff): nothing happens.
        assert_eq!(db.reclaim_stale_jobs_if_stale(&s1, now - LEASE).unwrap(), 0);
        assert_eq!(db.job_status("j").unwrap().as_deref(), Some("running"));
        assert_eq!(db.active_sessions().unwrap().len(), 1);

        // Stale owner: the row and the session flip together.  The cutoff
        // is re-read after ageing the heartbeat: `now` was captured before
        // it, so a wall-clock second crossing in between would leave the
        // aged heartbeat just younger than a stale cutoff.
        age_heartbeat(&db, &s1, LEASE);
        assert_eq!(
            db.reclaim_stale_jobs_if_stale(&s1, unix_now() - LEASE)
                .unwrap(),
            1
        );
        assert_eq!(db.job_status("j").unwrap().as_deref(), Some("pending"));
        assert!(db.active_sessions().unwrap().is_empty());

        // A session that already closed itself is not live either: its
        // leftover running rows are reclaimable regardless of heartbeat.
        let s2 = db.create_session(2, "h", None).unwrap();
        db.claim_job_for_session("j", &s2, LEASE).unwrap();
        db.interrupt_session(&s2).unwrap();
        assert_eq!(
            db.reclaim_stale_jobs_if_stale(&s2, unix_now() - LEASE)
                .unwrap(),
            1
        );
        assert_eq!(db.job_status("j").unwrap().as_deref(), Some("pending"));
    }

    #[test]
    fn claim_against_a_stale_owner_reclaims_directly() {
        let (_tmp, db) = db_with_job("j");
        let s1 = db.create_session(1, "h", None).unwrap();
        let s2 = db.create_session(2, "h", None).unwrap();
        db.claim_job_for_session("j", &s1, LEASE).unwrap();
        age_heartbeat(&db, &s1, LEASE + 5);
        assert_eq!(
            db.claim_job_for_session("j", &s2, LEASE).unwrap(),
            ClaimOutcome::Won
        );
    }

    #[test]
    fn verdicts_of_a_finished_session_are_not_mirrored() {
        // A failure recorded by a session that has since exited is history:
        // the row is reset and the job re-run, not mirrored.
        let (_tmp, db) = db_with_job("j");
        let s1 = db.create_session(1, "h", None).unwrap();
        db.claim_job_for_session("j", &s1, LEASE).unwrap();
        db.fail_job("j", &s1, 1).unwrap();
        db.complete_session(&s1).unwrap();
        assert_eq!(
            db.peer_job_state("j", LEASE).unwrap(),
            PeerJobState::Unclaimed
        );
        assert_eq!(db.job_status("j").unwrap().as_deref(), Some("pending"));

        // Same for a cancellation of a row nobody owned (pending cancel).
        let (_tmp, db) = db_with_job("k");
        db.cancel_job_ids(&["k".into()]).unwrap();
        let s2 = db.create_session(2, "h", None).unwrap();
        assert_eq!(
            db.claim_job_for_session("k", &s2, LEASE).unwrap(),
            ClaimOutcome::Won
        );
    }

    #[test]
    fn reset_inactive_rows_keeps_live_peer_rows_only() {
        let (_tmp, db) = db_with_job("done_by_old");
        for id in ["done_by_live", "cached_old", "running_dead"] {
            db.register_jobs(&[JobRecord {
                id: id.into(),
                rule_name: "r".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            }])
            .unwrap();
        }
        let old = db.create_session(1, "h", None).unwrap();
        db.claim_job_for_session("done_by_old", &old, LEASE)
            .unwrap();
        db.complete_job("done_by_old", &old, 0, "").unwrap();
        db.complete_session(&old).unwrap();

        let live = db.create_session(2, "h", None).unwrap();
        db.claim_job_for_session("done_by_live", &live, LEASE)
            .unwrap();
        db.complete_job("done_by_live", &live, 0, "").unwrap();

        db.skip_job("cached_old").unwrap();

        let dead = db.create_session(3, "h", None).unwrap();
        db.claim_job_for_session("running_dead", &dead, LEASE)
            .unwrap();
        age_heartbeat(&db, &dead, LEASE + 1);

        let ids: Vec<String> = ["done_by_old", "done_by_live", "cached_old", "running_dead"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(db.reset_inactive_job_rows(&ids, LEASE).unwrap(), 1);
        assert_eq!(
            db.job_status("running_dead").unwrap().as_deref(),
            Some("pending")
        );
        // Completions are left for the claim path (a cache hit never
        // claims; an executing job resets history lazily, see
        // `yesterdays_completion_is_history_for_a_new_session`).
        assert_eq!(
            db.job_status("done_by_old").unwrap().as_deref(),
            Some("completed")
        );
        assert_eq!(
            db.job_status("cached_old").unwrap().as_deref(),
            Some("completed")
        );
        assert_eq!(
            db.job_status("done_by_live").unwrap().as_deref(),
            Some("completed"),
            "a live peer's completion is the concurrent case the claim protocol serves"
        );
    }

    #[test]
    fn reset_inactive_rows_handles_more_ids_than_one_chunk() {
        // 1203 rows: two full chunks and a partial one, mixing a dead
        // owner's running rows, old completions and one live peer row.
        let (_tmp, db) = db_with_job("seed");
        let n = ID_CHUNK * 2 + 203;
        let ids: Vec<String> = (0..n).map(|i| format!("j{i}")).collect();
        db.register_jobs(
            &ids.iter()
                .map(|id| JobRecord {
                    id: id.clone(),
                    rule_name: "r".into(),
                    wildcards: "{}".into(),
                    cache_key: None,
                    run_id: None,
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let old = db.create_session(1, "h", None).unwrap();
        let dead = db.create_session(2, "h", None).unwrap();
        let live = db.create_session(3, "h", None).unwrap();
        for (i, id) in ids.iter().enumerate() {
            match i % 3 {
                0 => {
                    db.claim_job_for_session(id, &old, LEASE).unwrap();
                    db.complete_job(id, &old, 0, "").unwrap();
                }
                1 => {
                    db.claim_job_for_session(id, &dead, LEASE).unwrap();
                }
                _ => {
                    db.skip_job(id).unwrap();
                }
            }
        }
        db.complete_session(&old).unwrap();
        age_heartbeat(&db, &dead, LEASE);
        db.claim_job_for_session("seed", &live, LEASE).unwrap();

        let mut all = ids.clone();
        all.push("seed".into());
        let dead_running = ids.iter().enumerate().filter(|(i, _)| i % 3 == 1).count();
        assert_eq!(
            db.reset_inactive_job_rows(&all, LEASE).unwrap(),
            dead_running
        );
        for (i, id) in ids.iter().enumerate() {
            let expected = if i % 3 == 1 { "pending" } else { "completed" };
            assert_eq!(db.job_status(id).unwrap().as_deref(), Some(expected));
        }
        assert_eq!(db.job_status("seed").unwrap().as_deref(), Some("running"));
        let statuses = db.session_statuses().unwrap();
        assert!(statuses.contains(&(dead.clone(), "interrupted".into())));
        assert!(statuses.contains(&(live.clone(), "active".into())));
    }

    #[test]
    fn yesterdays_completion_is_history_for_a_new_session() {
        // A row completed by a session that exited before this one started
        // is reset and claimed; one completed by a live peer, or since this
        // session started, is a peer's result to consume.
        let (_tmp, db) = db_with_job("old");
        for id in ["fresh", "same_second", "by_live"] {
            db.register_jobs(&[JobRecord {
                id: id.into(),
                rule_name: "r".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            }])
            .unwrap();
        }
        let yesterday = db.create_session(1, "h", None).unwrap();
        db.claim_job_for_session("old", &yesterday, LEASE).unwrap();
        db.complete_job("old", &yesterday, 0, "").unwrap();
        db.complete_session(&yesterday).unwrap();
        db.conn()
            .execute(
                "UPDATE jobs SET completed_at = completed_at - 3600 WHERE id = 'old'",
                [],
            )
            .unwrap();

        let me = db.create_session(2, "h", None).unwrap();
        let live = db.create_session(3, "h", None).unwrap();
        db.claim_job_for_session("by_live", &live, LEASE).unwrap();
        db.complete_job("by_live", &live, 0, "").unwrap();
        let peer_that_exited = db.create_session(4, "h", None).unwrap();
        for id in ["fresh", "same_second"] {
            db.claim_job_for_session(id, &peer_that_exited, LEASE)
                .unwrap();
            db.complete_job(id, &peer_that_exited, 0, "").unwrap();
        }
        db.complete_session(&peer_that_exited).unwrap();
        // `same_second` keeps completed_at == my started_at (both this
        // second); `fresh` is moved strictly after my start.
        db.conn()
            .execute(
                "UPDATE jobs SET completed_at = (SELECT started_at + 1 FROM sessions WHERE id = ?1)
                 WHERE id = 'fresh'",
                rusqlite::params![me],
            )
            .unwrap();
        db.conn()
            .execute(
                "UPDATE jobs SET completed_at = (SELECT started_at FROM sessions WHERE id = ?1)
                 WHERE id = 'same_second'",
                rusqlite::params![me],
            )
            .unwrap();

        assert_eq!(
            db.claim_job_for_session("old", &me, LEASE).unwrap(),
            ClaimOutcome::Won,
            "yesterday's completion is history"
        );
        assert_eq!(
            db.claim_job_for_session("by_live", &me, LEASE).unwrap(),
            ClaimOutcome::Lost {
                owner: Some(live.clone())
            }
        );
        assert_eq!(
            db.claim_job_for_session("fresh", &me, LEASE).unwrap(),
            ClaimOutcome::Lost {
                owner: Some(peer_that_exited.clone())
            },
            "a completion recorded after this session started is consumed"
        );
        assert_eq!(
            db.peer_job_state("fresh", LEASE).unwrap(),
            PeerJobState::Completed
        );
        assert_eq!(
            db.claim_job_for_session("same_second", &me, LEASE).unwrap(),
            ClaimOutcome::Won,
            "a completion in the same second as this session's start is history"
        );
    }

    #[test]
    fn session_scoped_cancel_spares_a_peer_running_row() {
        let (_tmp, db) = db_with_job("theirs");
        db.register_jobs(&[JobRecord {
            id: "mine".into(),
            rule_name: "r".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }])
        .unwrap();
        let s1 = db.create_session(1, "h", None).unwrap();
        let s2 = db.create_session(2, "h", None).unwrap();
        db.claim_job_for_session("theirs", &s1, LEASE).unwrap();
        db.claim_job_for_session("mine", &s2, LEASE).unwrap();

        let cancelled = db
            .cancel_job_ids_for_session(&["theirs".into(), "mine".into()], &s2)
            .unwrap();
        assert_eq!(cancelled, vec!["mine".to_string()]);
        assert_eq!(db.job_status("theirs").unwrap().as_deref(), Some("running"));
        assert_eq!(db.job_status("mine").unwrap().as_deref(), Some("cancelled"));
    }

    #[test]
    fn heartbeat_interval_is_a_third_of_the_lease_at_least_one_second() {
        assert_eq!(heartbeat_interval(90).as_secs(), 30);
        assert_eq!(heartbeat_interval(2).as_secs(), 1);
        assert_eq!(heartbeat_interval(0).as_secs(), 1);
    }

    #[tokio::test]
    async fn claimer_trait_wraps_the_db_operations() {
        let tmp = NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();
        db.register_jobs(&[JobRecord {
            id: "j".into(),
            rule_name: "r".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }])
        .unwrap();
        let s1 = db.create_session(1, "h", None).unwrap();
        let s2 = db.create_session(2, "h", None).unwrap();
        let c1 = StateJobClaimer::new(StateDb::open(tmp.path()).unwrap(), s1.clone(), LEASE);
        let c2 = StateJobClaimer::new(StateDb::open(tmp.path()).unwrap(), s2, LEASE);
        let job = JobId::from("j");

        assert_eq!(c1.claim(&job).await, ClaimOutcome::Won);
        assert_eq!(
            c2.claim(&job).await,
            ClaimOutcome::Lost {
                owner: Some(s1.clone())
            }
        );
        assert_eq!(c2.peer_state(&job).await, PeerJobState::Running);
        db.complete_job("j", &s1, 0, "").unwrap();
        assert_eq!(c2.peer_state(&job).await, PeerJobState::Completed);
    }
}
