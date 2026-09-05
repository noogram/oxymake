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
//! first peer that looks at them ([`StateDb::reclaim_stale_jobs`], the same
//! reclaim `ox clean` uses). There is exactly one lease, the heartbeat one.
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
//! A completion is consumed whoever recorded it — the outputs are on disk.
//! A failure or cancellation is only mirrored while its author is live: a
//! verdict left behind by a finished or dead session is history, not a
//! peer's decision, so the row is reset and the job re-run here.
//!
//! # Fresh runs and old rows
//!
//! `register_jobs` keeps the status of rows that already exist, so without
//! further care a job `completed` by *yesterday's* run would make today's
//! claim lose and today's session consume a stale completion instead of
//! re-evaluating the job. [`StateDb::reset_inactive_job_rows`] runs right
//! after registration and resets every non-pending row of this run's jobs
//! whose owner is not live. Rows owned by a live peer are left alone — that
//! peer is the concurrent session this protocol exists for.
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
                "SELECT j.status, j.session_id, j.exit_code, s.status, s.heartbeat_at
                 FROM jobs j LEFT JOIN sessions s ON s.id = j.session_id
                 WHERE j.id = ?1",
                rusqlite::params![job_id],
                |row| {
                    let owner_status: Option<String> = row.get(3)?;
                    let owner_heartbeat: Option<u64> = row.get(4)?;
                    Ok(JobRow {
                        status: row.get(0)?,
                        session_id: row.get(1)?,
                        exit_code: row.get(2)?,
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
    /// owned by a live session, `pending` rows and (unless `reset_completed`)
    /// `completed` rows are left alone. Returns whether the row is now
    /// `pending`.
    fn release_if_owner_dead(
        &self,
        job_id: &str,
        row: &JobRow,
        lease_secs: u64,
        reset_completed: bool,
    ) -> Result<bool, StateError> {
        let now = unix_now();
        match row.status.as_str() {
            "pending" => Ok(true),
            _ if row.owner_live(lease_secs, now) => Ok(false),
            "running" => match &row.session_id {
                Some(owner) => {
                    self.reclaim_stale_jobs(owner)?;
                    Ok(true)
                }
                None => self.reset_job_row(job_id, row),
            },
            "completed" | "skipped" if !reset_completed => Ok(false),
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
    /// - a row whose owner is not live any more is released (reclaimed or
    ///   reset, see the module docs) and the CAS retried once;
    /// - a row owned by a live peer, or terminalized, is `Lost`.
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
        if self.release_if_owner_dead(job_id, &row, lease_secs, false)?
            && self.claim_job(job_id, session_id)?
        {
            return Ok(ClaimOutcome::Won);
        }
        // Re-read: the owner may have changed under us.
        let owner = self
            .job_row(job_id)?
            .and_then(|r| r.session_id)
            .or(row.session_id);
        Ok(ClaimOutcome::Lost { owner })
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
                self.release_if_owner_dead(job_id, &row, lease_secs, false)?;
                PeerJobState::Unclaimed
            }
        })
    }

    /// Reset the rows of `job_ids` that a previous, no longer live session
    /// left behind (any non-`pending` status, `completed` included), so a
    /// fresh run re-evaluates them instead of consuming stale verdicts.
    /// Rows owned by a live session are kept for the claim protocol.
    /// Returns the number of rows made `pending`.
    pub fn reset_inactive_job_rows(
        &self,
        job_ids: &[String],
        lease_secs: u64,
    ) -> Result<usize, StateError> {
        let mut reset = 0;
        for job_id in job_ids {
            let Some(row) = self.job_row(job_id)? else {
                continue;
            };
            if row.status == "pending" {
                continue;
            }
            if self.release_if_owner_dead(job_id, &row, lease_secs, true)? {
                reset += 1;
            }
        }
        Ok(reset)
    }
}

/// Claimer that reads and writes the `jobs` / `sessions` tables of `state.db`.
///
/// Holds its own [`StateDb`] connection behind a mutex (a `rusqlite`
/// connection is `Send` but not `Sync`); every operation is a few short
/// SQLite statements executed without awaiting, so the lock is never held
/// across a suspension point.
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
        assert!(db.skip_job("j").unwrap());
        assert_eq!(
            db.peer_job_state("j", LEASE).unwrap(),
            PeerJobState::Completed
        );
        let s2 = db.create_session(2, "h", None).unwrap();
        assert!(matches!(
            db.claim_job_for_session("j", &s2, LEASE).unwrap(),
            ClaimOutcome::Lost { .. }
        ));
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
        assert_eq!(db.reset_inactive_job_rows(&ids, LEASE).unwrap(), 3);
        assert_eq!(
            db.job_status("done_by_old").unwrap().as_deref(),
            Some("pending")
        );
        assert_eq!(
            db.job_status("cached_old").unwrap().as_deref(),
            Some("pending")
        );
        assert_eq!(
            db.job_status("running_dead").unwrap().as_deref(),
            Some("pending")
        );
        assert_eq!(
            db.job_status("done_by_live").unwrap().as_deref(),
            Some("completed"),
            "a live peer's completion is the concurrent case the claim protocol serves"
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
