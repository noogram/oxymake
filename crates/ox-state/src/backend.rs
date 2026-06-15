//! Pluggable backend trait for state persistence.
//!
//! [`StateBackend`] defines the full interface that any state storage
//! backend must implement.  The default implementation is
//! [`StateDb`](crate::db::StateDb) (SQLite), but this trait enables
//! alternative backends (e.g., Dolt, in-memory) without changing
//! consumers.
//!
//! # Design
//!
//! The trait surface mirrors [`StateDb`](crate::db::StateDb)'s public
//! methods exactly, so existing code can be transitioned from concrete
//! `&StateDb` to `&dyn StateBackend` (or `impl StateBackend`) with
//! minimal churn.

use ox_core::model::ContentHash;

use crate::db::{
    AllJobDetail, JobCounts, JobHistoryEntry, JobLogInfo, JobRecord, JobWithLog, PipelineStats,
    RunRecord, RunningJobDetail,
};
use crate::error::StateError;
use crate::session::SessionInfo;

/// A pluggable backend for OxyMake state persistence.
///
/// Implementors provide durable (or in-memory) storage for job state,
/// sessions, and audit-trail data.  All methods mirror the
/// [`StateDb`](crate::db::StateDb) API.
pub trait StateBackend {
    // -----------------------------------------------------------------
    // Schema
    // -----------------------------------------------------------------

    /// Return the current schema version.
    fn schema_version(&self) -> Result<u32, StateError>;

    // -----------------------------------------------------------------
    // Job operations
    // -----------------------------------------------------------------

    /// Register a batch of jobs with status `pending`.
    fn register_jobs(&self, jobs: &[JobRecord]) -> Result<(), StateError>;

    /// Atomically claim a job for a session. Returns `true` on success.
    fn claim_job(&self, job_id: &str, session_id: &str) -> Result<bool, StateError>;

    /// Mark a job as completed (`running` → `completed`), only if it is
    /// still claimed by `session_id` (zombie guard, H16).
    /// Returns `true` if the transition happened.
    fn complete_job(
        &self,
        job_id: &str,
        session_id: &str,
        exit_code: i32,
        output_hashes: &str,
    ) -> Result<bool, StateError>;

    /// Mark a job as failed (`running` → `failed`), only if it is still
    /// claimed by `session_id` (zombie guard, H16).
    /// Returns `true` if the transition happened.
    fn fail_job(&self, job_id: &str, session_id: &str, exit_code: i32) -> Result<bool, StateError>;

    /// Mark a job as completed via cache hit (`pending` → `completed` with `cached = 1`).
    /// Returns `true` if the transition happened.
    fn skip_job(&self, job_id: &str) -> Result<bool, StateError>;

    /// Get the status string for a job.
    fn job_status(&self, job_id: &str) -> Result<Option<String>, StateError>;

    /// Return IDs of all jobs with the given status.
    fn jobs_by_status(&self, status: &str) -> Result<Vec<String>, StateError>;

    /// Cancel running/pending jobs matching optional filters.
    /// Returns the IDs of cancelled jobs.
    fn cancel_jobs(
        &self,
        rule: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Vec<String>, StateError>;

    /// Aggregate job counts by status.
    fn job_counts(&self) -> Result<JobCounts, StateError>;

    /// Details of currently running jobs.
    fn running_jobs_detail(&self) -> Result<Vec<RunningJobDetail>, StateError>;

    /// Details of all jobs.
    fn all_jobs_detail(&self) -> Result<Vec<AllJobDetail>, StateError>;

    /// Per-rule aggregate counts for pipeline display.
    fn pipeline_stats(&self) -> Result<Vec<PipelineStats>, StateError>;

    // -----------------------------------------------------------------
    // Session operations
    // -----------------------------------------------------------------

    /// Create a new session and return its ID.
    fn create_session(
        &self,
        pid: u32,
        hostname: &str,
        target_filter: Option<&str>,
    ) -> Result<String, StateError>;

    /// Update heartbeat for an active session.
    fn heartbeat(&self, session_id: &str) -> Result<(), StateError>;

    /// Mark a session as completed.
    fn complete_session(&self, session_id: &str) -> Result<(), StateError>;

    /// Mark a session as interrupted.
    fn interrupt_session(&self, session_id: &str) -> Result<(), StateError>;

    /// Find sessions whose heartbeat is older than `threshold_secs`.
    fn find_stale_sessions(&self, threshold_secs: u64) -> Result<Vec<String>, StateError>;

    /// Reclaim jobs from a stale session. Returns count reclaimed.
    fn reclaim_stale_jobs(&self, session_id: &str) -> Result<usize, StateError>;

    /// List all active sessions.
    fn active_sessions(&self) -> Result<Vec<SessionInfo>, StateError>;

    // -----------------------------------------------------------------
    // Audit trail (runs & job history)
    // -----------------------------------------------------------------

    /// Begin a new run.
    fn begin_run(
        &self,
        run_id: &str,
        workflow_hash: Option<&ContentHash>,
        job_count: usize,
        note: Option<&str>,
    ) -> Result<(), StateError>;

    /// Finalise a run with aggregate counts.
    fn end_run(
        &self,
        run_id: &str,
        succeeded: usize,
        failed: usize,
        skipped: usize,
    ) -> Result<(), StateError>;

    /// Record a job execution in the audit trail.
    fn record_job_history(&self, entry: &JobHistoryEntry) -> Result<(), StateError>;

    /// Finalize audit-trail history from the post-flush jobs table.
    ///
    /// Reads all terminal-state jobs for the given run and inserts
    /// history entries.  Returns the number of entries recorded.
    fn finalize_job_history(
        &self,
        run_id: &str,
        executor: &str,
        hostname: &str,
        wall_times: &std::collections::HashMap<String, u64>,
    ) -> Result<usize, StateError>;

    /// Query job history for a run.
    fn job_history_for_run(&self, run_id: &str) -> Result<Vec<JobHistoryEntry>, StateError>;

    /// List all runs, most recent first.
    fn list_runs(&self) -> Result<Vec<RunRecord>, StateError>;

    /// Return the log_path and status for a specific job.
    fn job_log_info(&self, job_id: &str) -> Result<Option<JobLogInfo>, StateError>;

    /// Return all jobs matching optional filters, with their log_path and status.
    fn jobs_with_logs(
        &self,
        rule: Option<&str>,
        failed_only: bool,
    ) -> Result<Vec<JobWithLog>, StateError>;

    // -----------------------------------------------------------------
    // Job edges (DAG visualization)
    // -----------------------------------------------------------------

    /// Register job-to-job dependency edges. Each tuple is `(from_job, to_job)`.
    fn register_edges(&self, edges: &[(String, String)]) -> Result<(), StateError>;

    /// Query all job-to-job edges. Returns `(from_job, to_job)` pairs.
    fn job_edges(&self) -> Result<Vec<(String, String)>, StateError>;
}
