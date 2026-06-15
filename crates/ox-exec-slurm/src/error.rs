//! Error types for the SLURM executor.

/// Errors that can occur during SLURM job submission, polling, or cancellation.
#[derive(Debug, thiserror::Error)]
pub enum SlurmError {
    /// `sbatch` command failed or returned unexpected output.
    #[error("sbatch submission failed: {0}")]
    SubmitFailed(String),

    /// SLURM job not found in `sacct` or `squeue` output.
    #[error("SLURM job {slurm_job_id} not found")]
    JobNotFound { slurm_job_id: u32 },

    /// Failed to parse output from `sacct`, `squeue`, or `sinfo`.
    #[error("failed to parse SLURM output: {0}")]
    ParseError(String),

    /// Cannot reach the SLURM controller (slurmctld).
    #[error("SLURM cluster unreachable: {0}")]
    ClusterUnreachable(String),

    /// Mutually exclusive resources specified (e.g., both `mem` and `mem_per_cpu`).
    #[error("resource conflict: {0}")]
    ResourceConflict(String),

    /// A compute node failed during job execution.
    #[error("node failure on {node}: SLURM job {slurm_job_id}")]
    NodeFail { node: String, slurm_job_id: u32 },

    /// Job exceeded its time limit.
    #[error("SLURM job timed out after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },

    /// OxyMake job ID not found in the executor's tracking map.
    #[error("job {job_id} not tracked by SLURM executor")]
    JobNotTracked { job_id: String },

    /// slurmrestd returned a non-success HTTP status.
    #[error("slurmrestd API error (HTTP {status}): {body}")]
    ApiError { status: u16, body: String },

    /// I/O error (file creation, process spawning, etc.).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
