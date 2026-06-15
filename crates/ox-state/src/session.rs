//! Session lifecycle management.
//!
//! A **session** represents a single `ox run` invocation.  Multiple
//! sessions can be active simultaneously — each claims jobs atomically
//! via [`StateDb::claim_job`](crate::db::StateDb::claim_job) so that
//! no job is executed twice.
//!
//! Sessions emit periodic heartbeats.  If a session crashes without
//! marking itself `completed` or `interrupted`, its heartbeat grows
//! stale and another session can reclaim its running jobs.
//!
//! # Lifecycle
//!
//! ```text
//! create_session → heartbeat → heartbeat → … → complete_session
//!                                                or interrupt_session
//!                                                or (crash → stale detection)
//! ```

use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::db::StateDb;
use crate::error::StateError;

// ---------------------------------------------------------------------------
// SessionInfo
// ---------------------------------------------------------------------------

/// A snapshot of a session's metadata — returned by
/// [`StateDb::active_sessions`].
///
/// ```
/// use ox_state::session::SessionInfo;
///
/// let info = SessionInfo {
///     id: "s-12345-abc".into(),
///     pid: 42,
///     hostname: "build-host".into(),
///     started_at: 1700000000,
///     heartbeat_at: 1700000060,
///     target_filter: Some("human".into()),
/// };
/// assert_eq!(info.pid, 42);
/// ```
pub struct SessionInfo {
    /// Unique session identifier.
    pub id: String,
    /// Process ID of the `ox run` process.
    pub pid: u32,
    /// Hostname where the session is running.
    pub hostname: String,
    /// UNIX timestamp (seconds) when the session was created.
    pub started_at: u64,
    /// UNIX timestamp (seconds) of the last heartbeat.
    pub heartbeat_at: u64,
    /// Optional target filter (e.g., `"human"` for a partial build).
    pub target_filter: Option<String>,
}

// ---------------------------------------------------------------------------
// Session operations on StateDb
// ---------------------------------------------------------------------------

impl StateDb {
    /// Create a new session and return its unique ID.
    ///
    /// The session starts with status `active` and the heartbeat set
    /// to the current time.  The ID is a combination of the PID, a
    /// timestamp, and a random suffix to avoid collisions across
    /// machines.
    ///
    /// ```
    /// # use tempfile::NamedTempFile;
    /// use ox_state::db::StateDb;
    ///
    /// let tmp = NamedTempFile::new().unwrap();
    /// let db = StateDb::open(tmp.path()).unwrap();
    ///
    /// let sid = db.create_session(1234, "build-host", Some("human")).unwrap();
    /// assert!(!sid.is_empty());
    ///
    /// let sessions = db.active_sessions().unwrap();
    /// assert_eq!(sessions.len(), 1);
    /// assert_eq!(sessions[0].pid, 1234);
    /// ```
    pub fn create_session(
        &self,
        pid: u32,
        hostname: &str,
        target_filter: Option<&str>,
    ) -> Result<String, StateError> {
        let now = unix_now();
        let id = generate_session_id(pid, now);

        self.conn().execute(
            "INSERT INTO sessions (id, pid, hostname, started_at, target_filter, heartbeat_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?4, 'active')",
            rusqlite::params![id, pid, hostname, now, target_filter],
        )?;

        Ok(id)
    }

    /// Update the heartbeat timestamp for an active session.
    ///
    /// Should be called periodically (e.g., every 30 seconds) so that
    /// other sessions can detect crashes via `find_stale_sessions`.
    pub fn heartbeat(&self, session_id: &str) -> Result<(), StateError> {
        let now = unix_now();
        self.conn().execute(
            "UPDATE sessions SET heartbeat_at = ?1 WHERE id = ?2 AND status = 'active'",
            rusqlite::params![now, session_id],
        )?;
        Ok(())
    }

    /// Mark a session as `completed` (normal exit).
    pub fn complete_session(&self, session_id: &str) -> Result<(), StateError> {
        self.conn().execute(
            "UPDATE sessions SET status = 'completed' WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }

    /// Mark a session as `interrupted` (e.g., Ctrl+C).
    pub fn interrupt_session(&self, session_id: &str) -> Result<(), StateError> {
        self.conn().execute(
            "UPDATE sessions SET status = 'interrupted' WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }

    /// Find sessions whose heartbeat is older than `threshold_secs`
    /// seconds ago and are still marked `active`.
    ///
    /// Returns the session IDs.  The caller typically follows up with
    /// `reclaim_stale_jobs` for each stale session.
    pub fn find_stale_sessions(&self, threshold_secs: u64) -> Result<Vec<String>, StateError> {
        let cutoff = unix_now().saturating_sub(threshold_secs);
        let mut stmt = self
            .conn()
            .prepare("SELECT id FROM sessions WHERE status = 'active' AND heartbeat_at < ?1")?;
        let rows = stmt.query_map(rusqlite::params![cutoff], |row| row.get(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// Reclaim jobs from a stale session by resetting them to `pending`.
    ///
    /// Also marks the stale session as `interrupted`.  Returns the
    /// number of jobs reclaimed.
    pub fn reclaim_stale_jobs(&self, session_id: &str) -> Result<usize, StateError> {
        let tx = self.conn().unchecked_transaction()?;
        let reclaimed = tx.execute(
            "UPDATE jobs SET status = 'pending', session_id = NULL, locked_by = NULL,
                             started_at = NULL
             WHERE session_id = ?1 AND status = 'running'",
            rusqlite::params![session_id],
        )?;
        tx.execute(
            "UPDATE sessions SET status = 'interrupted' WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        tx.commit()?;
        Ok(reclaimed)
    }

    /// PID of the *active* session that owns (claimed) the given job.
    ///
    /// Returns `None` if the job was never claimed or its owning session
    /// is no longer active — in that case the recorded PID may already
    /// have been recycled by an unrelated process and must not be
    /// signalled (B8).
    pub fn job_session_pid(&self, job_id: &str) -> Result<Option<u32>, StateError> {
        let mut stmt = self.conn().prepare(
            "SELECT s.pid FROM jobs j
             JOIN sessions s ON j.session_id = s.id
             WHERE j.id = ?1 AND s.status = 'active'",
        )?;
        let mut rows = stmt.query(rusqlite::params![job_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// List all sessions with status `active`.
    pub fn active_sessions(&self) -> Result<Vec<SessionInfo>, StateError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, pid, hostname, started_at, heartbeat_at, target_filter
             FROM sessions WHERE status = 'active'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SessionInfo {
                id: row.get(0)?,
                pid: row.get(1)?,
                hostname: row.get(2)?,
                started_at: row.get(3)?,
                heartbeat_at: row.get(4)?,
                target_filter: row.get(5)?,
            })
        })?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Current UNIX timestamp in seconds.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Generate a collision-resistant session ID.
///
/// Format: `s-{pid}-{timestamp}-{uuid}`.  Uses UUID v4 for the random
/// suffix to guarantee uniqueness across machines and processes.
fn generate_session_id(pid: u32, timestamp: u64) -> String {
    let uuid = Uuid::new_v4();
    format!("s-{pid}-{timestamp}-{uuid}")
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
    fn session_lifecycle() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(42, "test-host", None).unwrap();

        let sessions = db.active_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].pid, 42);
        assert_eq!(sessions[0].hostname, "test-host");

        db.heartbeat(&sid).unwrap();
        db.complete_session(&sid).unwrap();

        // No longer active.
        let sessions = db.active_sessions().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn interrupt_session_removes_from_active() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(42, "test-host", None).unwrap();
        db.interrupt_session(&sid).unwrap();
        assert!(db.active_sessions().unwrap().is_empty());
    }

    #[test]
    fn job_session_pid_returns_owning_session() {
        // `ox cancel` must signal the session that OWNS the job — not the
        // first active session in the table (B8).
        let (_tmp, db) = temp_db();

        // Two active sessions; the first (older) does NOT own the job.
        let _bystander = db.create_session(1111, "host-a", None).unwrap();
        let owner = db.create_session(2222, "host-b", None).unwrap();

        db.register_jobs(&[crate::db::JobRecord {
            id: "job-x".into(),
            rule_name: "build".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }])
        .unwrap();
        db.claim_job("job-x", &owner).unwrap();

        assert_eq!(db.job_session_pid("job-x").unwrap(), Some(2222));
    }

    #[test]
    fn job_session_pid_none_for_unclaimed_job() {
        let (_tmp, db) = temp_db();
        let _sid = db.create_session(1111, "host-a", None).unwrap();
        db.register_jobs(&[crate::db::JobRecord {
            id: "job-y".into(),
            rule_name: "build".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }])
        .unwrap();

        // Never claimed: no owning session, no PID — never a bystander's.
        assert_eq!(db.job_session_pid("job-y").unwrap(), None);
    }

    #[test]
    fn job_session_pid_none_for_inactive_owner() {
        let (_tmp, db) = temp_db();
        let owner = db.create_session(2222, "host-b", None).unwrap();
        db.register_jobs(&[crate::db::JobRecord {
            id: "job-z".into(),
            rule_name: "build".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }])
        .unwrap();
        db.claim_job("job-z", &owner).unwrap();
        db.complete_session(&owner).unwrap();

        // The owning session exited: its PID may be recycled by an
        // unrelated process — do not signal it (B8).
        assert_eq!(db.job_session_pid("job-z").unwrap(), None);
    }

    #[test]
    fn stale_session_detection() {
        let (_tmp, db) = temp_db();

        // Insert a session with a heartbeat far in the past.
        let old_time: u64 = 1_000_000;
        let sid = "s-stale-test";
        db.conn()
            .execute(
                "INSERT INTO sessions (id, pid, hostname, started_at, heartbeat_at, status)
                 VALUES (?1, 99, 'ghost', ?2, ?2, 'active')",
                rusqlite::params![sid, old_time],
            )
            .unwrap();

        // With a 60-second threshold, this session is definitely stale.
        let stale = db.find_stale_sessions(60).unwrap();
        assert!(stale.contains(&sid.to_string()));
    }

    #[test]
    fn reclaim_stale_jobs_resets_to_pending() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "host", None).unwrap();

        // Register and claim a job.
        let jobs = vec![crate::db::JobRecord {
            id: "j1".into(),
            rule_name: "r".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }];
        db.register_jobs(&jobs).unwrap();
        db.claim_job("j1", &sid).unwrap();
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("running"));

        // Reclaim resets the job and marks the session interrupted.
        let count = db.reclaim_stale_jobs(&sid).unwrap();
        assert_eq!(count, 1);
        assert_eq!(db.job_status("j1").unwrap().as_deref(), Some("pending"));
        assert!(db.active_sessions().unwrap().is_empty());
    }

    #[test]
    fn target_filter_round_trips() {
        let (_tmp, db) = temp_db();
        db.create_session(1, "host", Some("human")).unwrap();

        let sessions = db.active_sessions().unwrap();
        assert_eq!(sessions[0].target_filter.as_deref(), Some("human"));
    }

    #[test]
    fn session_ids_are_unique_even_with_same_pid_and_timestamp() {
        // Regression test for ox-1j45: previously, two invocations with
        // the same PID and timestamp could produce identical session IDs.
        let ids: Vec<String> = (0..100)
            .map(|_| generate_session_id(1, 1_700_000_000))
            .collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "session IDs must be unique");
    }

    #[test]
    fn multiple_sessions_are_independent() {
        let (_tmp, db) = temp_db();
        let s1 = db.create_session(1, "host-a", None).unwrap();
        let s2 = db.create_session(2, "host-b", Some("mouse")).unwrap();

        assert_eq!(db.active_sessions().unwrap().len(), 2);

        db.complete_session(&s1).unwrap();
        let active = db.active_sessions().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, s2);
    }
}
