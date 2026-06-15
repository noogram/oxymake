//! # Executor Trait
//!
//! Defines the plugin interface for job execution backends.
//! Implementations determine *where* and *how* jobs run — the trait
//! never decides *what* to run (that's the scheduler's job).
//!
//! Built-in: `LocalExecutor` (ox-exec-local)
//! Phase 2+: SLURM, Kubernetes, Ray

use std::fmt::Debug;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use std::collections::HashMap;

use crate::job_graph::JobGraph;
use crate::memory_map::OutputMemoryMap;
use crate::model::{ConcreteJob, JobId};

/// Result of executing a single job.
#[derive(Debug, Clone)]
pub struct JobResult {
    /// Job that was executed.
    pub job_id: JobId,
    /// Exit code from the process (0 = success).
    pub exit_code: i32,
    /// Wall-clock duration of execution.
    pub duration: Duration,
    /// Peak memory usage in bytes (if available).
    pub peak_memory_bytes: Option<u64>,
    /// CPU time (user + system) if available.
    pub cpu_time: Option<Duration>,
    /// Path to the captured stdout/stderr log file.
    pub log_path: Option<std::path::PathBuf>,
    /// Last N lines of stderr/log output (populated on failure).
    pub stderr_tail: Option<String>,
}

/// Runtime status of a submitted job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    /// Job is queued but not yet running.
    Queued,
    /// Job is actively executing.
    Running,
    /// Job completed successfully.
    Completed,
    /// Job failed with an error.
    Failed(String),
    /// Job was cancelled.
    Cancelled,
}

/// Capabilities advertised by an executor implementation.
///
/// The scheduler uses these to make routing decisions:
/// - Memory-passing jobs can only go to executors with `supports_memory_passing`
/// - GPU jobs require `supports_gpu`
/// - Shadow directories require `supports_shadow_dirs`
#[derive(Debug, Clone, Default)]
pub struct ExecutorCapabilities {
    /// Can run GPU-accelerated jobs.
    pub supports_gpu: bool,
    /// Can stream data between co-scheduled jobs via named pipes.
    pub supports_streaming: bool,
    /// Can create isolated shadow directories for hermetic execution.
    pub supports_shadow_dirs: bool,
    /// Can pass in-memory objects between `call`-mode jobs (same process).
    pub supports_memory_passing: bool,
    /// Maximum timeout this executor can enforce (None = unlimited).
    pub max_timeout: Option<Duration>,
    /// Can batch wildcard-expanded jobs into a single array submission.
    pub supports_job_arrays: bool,
    /// Can accept an entire DAG for submission (fire-and-forget mode).
    ///
    /// When true, the executor supports [`Executor::submit_dag`] which
    /// submits the full uncached subgraph and returns immediately.
    pub supports_dag_submission: bool,
}

/// Result of submitting an entire DAG to a remote executor.
///
/// Returned by [`Executor::submit_dag`]. Contains tracking information
/// for the submitted DAG so that `ox status` can poll progress.
#[derive(Debug, Clone)]
pub struct DagSubmission {
    /// Unique identifier for this DAG submission (matches the run_id).
    pub run_id: String,
    /// Total number of jobs in the DAG.
    pub total_jobs: usize,
    /// Jobs that were immediately submitted (root jobs with no dependencies).
    pub submitted: usize,
    /// Jobs that are pending (waiting for upstream dependencies).
    pub pending: usize,
    /// Jobs that were skipped (cached).
    pub skipped: usize,
    /// Mapping from OxyMake job IDs to executor-specific submission IDs.
    /// For Ray: maps to Ray submission IDs. For SLURM: maps to SLURM job IDs.
    pub job_submissions: HashMap<String, String>,
}

// Default is derived: all bools are false, Option is None.

/// An opaque handle to a prepared workspace for job execution.
///
/// Created by [`Executor::prepare_workspace`], consumed by
/// [`Executor::finalize_workspace`]. The local executor uses this
/// as a no-op; remote executors use it for staging files in/out.
pub struct Workspace {
    /// Working directory for the job.
    pub work_dir: std::path::PathBuf,
    /// Opaque state for the executor (e.g., temp dir handle).
    _private: Box<dyn std::any::Any + Send + Sync>,
}

impl Workspace {
    /// Create a workspace with a working directory and optional private state.
    pub fn new(work_dir: std::path::PathBuf) -> Self {
        Self {
            work_dir,
            _private: Box::new(()),
        }
    }

    /// Create a workspace with private state for cleanup.
    pub fn with_state(
        work_dir: std::path::PathBuf,
        state: impl std::any::Any + Send + Sync + 'static,
    ) -> Self {
        Self {
            work_dir,
            _private: Box::new(state),
        }
    }

    /// Consume the workspace and attempt to downcast its private state.
    ///
    /// Returns `None` if the stored state is not of type `T`.
    pub fn into_state<T: 'static>(self) -> Option<T> {
        self._private.downcast().ok().map(|b| *b)
    }
}

impl Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("work_dir", &self.work_dir)
            .finish()
    }
}

/// Execution context passed to the executor alongside the job.
///
/// Contains runtime information the executor needs but that isn't
/// part of the job definition itself.
#[derive(Debug, Clone)]
pub struct ExecContext {
    /// Maximum concurrent jobs across all executors.
    pub global_job_limit: usize,
    /// Run ID for log file organization.
    pub run_id: String,
    /// Base directory for log files.
    pub log_dir: std::path::PathBuf,
    /// Project root directory (where the Oxymakefile lives).
    ///
    /// Remote executors (e.g. SLURM) must `cd` to this directory so that
    /// relative output paths resolve to the same locations as the local
    /// executor, which is required for the content-addressable cache to
    /// record and match outputs correctly.
    pub project_dir: std::path::PathBuf,
    /// Trusted external directories from `{config.*}` substitution.
    ///
    /// Absolute output paths that fall under one of these directories are
    /// allowed by the path-traversal check in `prepare_workspace`. Config
    /// values are user-declared and trusted.
    #[allow(clippy::doc_markdown)]
    pub trusted_dirs: Vec<std::path::PathBuf>,
    /// In-memory input data from upstream jobs (Stage 2).
    ///
    /// Maps output-ref keys (see [`crate::job_graph::output_ref_key`]) to
    /// the raw bytes of the upstream output. Populated by the scheduler
    /// when the cheapest materialization for an input is `InMemory`.
    /// Executors should check this map before reading inputs from disk.
    pub input_data: HashMap<String, Arc<[u8]>>,
    /// Shared in-memory output data for Stage 2 data transport.
    ///
    /// When present, executors should check this map before reading inputs
    /// from disk. Producers store their output data here after execution;
    /// consumers retrieve it to avoid disk I/O on the critical path.
    pub memory_map: Option<OutputMemoryMap>,
}

/// The core executor trait — the plugin interface for job execution.
///
/// Every executor implementation handles the full lifecycle of a remote
/// or local job: prepare workspace, execute, finalize, cancel.
///
/// # Implementors
///
/// - `ox-exec-local`: Fork/tokio subprocess execution
/// - `ox-exec-slurm` (Phase 2): SLURM sbatch submission
/// - `ox-exec-k8s` (Phase 2): Kubernetes Job creation
/// - `ox-exec-ray` (Phase 3): Ray Jobs API
pub trait Executor: Send + Sync + Debug {
    /// Error type specific to this executor.
    type Error: std::error::Error + Send + Sync + 'static;

    // -- Lifecycle --

    /// Initialize the executor (validate configuration, check connectivity).
    fn init(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Check that the executor is healthy and ready to accept jobs.
    fn health_check(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Clean up executor resources on shutdown.
    fn cleanup(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;

    // -- Capabilities --

    /// Query the capabilities of this executor.
    fn capabilities(&self) -> ExecutorCapabilities;

    /// Maximum number of concurrent jobs this executor supports.
    /// `None` means unlimited (up to the scheduler's global limit).
    fn max_concurrency(&self) -> Option<usize>;

    // -- Execution --

    /// Prepare a workspace for job execution.
    ///
    /// For local executors, this may be a no-op. For remote executors
    /// (SLURM, K8s), this stages input files to the compute node.
    fn prepare_workspace(
        &self,
        job: &ConcreteJob,
        ctx: &ExecContext,
    ) -> impl Future<Output = Result<Workspace, Self::Error>> + Send;

    /// Execute a job in the prepared workspace.
    ///
    /// For local executors, this spawns a subprocess and waits.
    /// For remote executors, this submits the job and polls for completion.
    fn execute(
        &self,
        job: &ConcreteJob,
        workspace: &Workspace,
        ctx: &ExecContext,
    ) -> impl Future<Output = Result<JobResult, Self::Error>> + Send;

    /// Finalize the workspace after execution.
    ///
    /// For remote executors, this collects output files from the compute
    /// node. For all executors, this cleans up temporary resources.
    fn finalize_workspace(
        &self,
        workspace: Workspace,
        result: &JobResult,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Cancel a running job.
    ///
    /// This is called during graceful shutdown (Ctrl+C) and when
    /// upstream failures trigger downstream cancellation.
    fn cancel(&self, job_id: &JobId) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Poll the status of a submitted job.
    ///
    /// Used by remote executors where `execute()` returns before the
    /// job completes. The scheduler polls until the job reaches a
    /// terminal state.
    fn poll_status(
        &self,
        job_id: &JobId,
    ) -> impl Future<Output = Result<JobStatus, Self::Error>> + Send;

    /// Submit an entire DAG for remote execution.
    ///
    /// Remote executors (Ray, SLURM) can override this to submit the full
    /// uncached subgraph in one batch and return immediately. The caller
    /// can then use `poll_status` or `ox status` to track progress.
    ///
    /// Local executors should return an error — they execute synchronously
    /// via the scheduler loop instead.
    ///
    /// # Arguments
    ///
    /// * `graph` — The JobGraph containing all concrete jobs and their
    ///   dependency edges.
    /// * `ctx` — Execution context (project dir, run ID, log dir, etc.).
    fn submit_dag(
        &self,
        graph: &JobGraph,
        ctx: &ExecContext,
    ) -> impl Future<Output = Result<DagSubmission, Self::Error>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_capabilities_default() {
        let caps = ExecutorCapabilities::default();
        assert!(!caps.supports_gpu);
        assert!(!caps.supports_streaming);
        assert!(!caps.supports_shadow_dirs);
        assert!(!caps.supports_memory_passing);
        assert!(caps.max_timeout.is_none());
        assert!(!caps.supports_dag_submission);
    }

    #[test]
    fn workspace_new() {
        let ws = Workspace::new(std::path::PathBuf::from("/tmp/test"));
        assert_eq!(ws.work_dir, std::path::PathBuf::from("/tmp/test"));
    }

    #[test]
    fn workspace_with_state() {
        let ws = Workspace::with_state(std::path::PathBuf::from("/tmp/test"), 42u32);
        assert_eq!(ws.work_dir, std::path::PathBuf::from("/tmp/test"));
    }

    #[test]
    fn workspace_into_state_recovers_typed_state() {
        let ws = Workspace::with_state(std::path::PathBuf::from("/tmp/test"), 42u32);
        assert_eq!(ws.into_state::<u32>(), Some(42));
    }

    #[test]
    fn workspace_into_state_returns_none_on_mismatch() {
        let ws = Workspace::with_state(std::path::PathBuf::from("/tmp/test"), 42u32);
        assert_eq!(ws.into_state::<String>(), None);
    }

    #[test]
    fn workspace_into_state_returns_none_for_default() {
        let ws = Workspace::new(std::path::PathBuf::from("/tmp/test"));
        assert_eq!(ws.into_state::<u32>(), None);
    }

    #[test]
    fn workspace_debug() {
        let ws = Workspace::new(std::path::PathBuf::from("/tmp/test"));
        let debug = format!("{:?}", ws);
        assert!(debug.contains("Workspace"));
        assert!(debug.contains("/tmp/test"));
    }

    #[test]
    fn exec_context_construction() {
        let ctx = ExecContext {
            global_job_limit: 8,
            run_id: "run-001".into(),
            log_dir: std::path::PathBuf::from("/tmp/logs"),
            project_dir: std::path::PathBuf::from("/tmp/project"),
            trusted_dirs: vec![],
            input_data: HashMap::new(),
            memory_map: None,
        };
        assert_eq!(ctx.global_job_limit, 8);
        assert_eq!(ctx.run_id, "run-001");
        assert_eq!(ctx.log_dir, std::path::PathBuf::from("/tmp/logs"));
        assert_eq!(ctx.project_dir, std::path::PathBuf::from("/tmp/project"));
    }

    #[test]
    fn exec_context_clone_and_debug() {
        let ctx = ExecContext {
            global_job_limit: 4,
            run_id: "run-002".into(),
            log_dir: std::path::PathBuf::from("/tmp/logs"),
            project_dir: std::path::PathBuf::from("/tmp/project"),
            trusted_dirs: vec![],
            input_data: HashMap::new(),
            memory_map: None,
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.run_id, "run-002");
        let debug = format!("{:?}", cloned);
        assert!(debug.contains("ExecContext"));
    }

    #[test]
    fn job_result_clone_and_debug() {
        let result = JobResult {
            job_id: crate::model::JobId::from("j-001"),
            exit_code: 0,
            duration: Duration::from_secs(10),
            peak_memory_bytes: Some(1024),
            cpu_time: Some(Duration::from_secs(5)),
            log_path: Some(std::path::PathBuf::from("/tmp/log.txt")),
            stderr_tail: None,
        };
        let cloned = result.clone();
        assert_eq!(cloned.exit_code, 0);
        let debug = format!("{:?}", cloned);
        assert!(debug.contains("j-001"));
    }

    #[test]
    fn job_status_variants() {
        let statuses = vec![
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Completed,
            JobStatus::Failed("err".into()),
            JobStatus::Cancelled,
        ];
        assert_eq!(statuses[0], JobStatus::Queued);
        assert_ne!(statuses[0], JobStatus::Running);
        let debug = format!("{:?}", statuses[3]);
        assert!(debug.contains("Failed"));
    }

    #[test]
    fn executor_capabilities_clone_and_debug() {
        let caps = ExecutorCapabilities {
            supports_gpu: true,
            supports_streaming: false,
            supports_shadow_dirs: true,
            supports_memory_passing: false,
            max_timeout: Some(Duration::from_secs(300)),
            supports_job_arrays: false,
            supports_dag_submission: false,
        };
        let cloned = caps.clone();
        assert!(cloned.supports_gpu);
        assert!(cloned.supports_shadow_dirs);
        let debug = format!("{:?}", cloned);
        assert!(debug.contains("ExecutorCapabilities"));
    }
}
