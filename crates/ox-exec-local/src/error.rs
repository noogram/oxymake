//! Error types for the local process executor.
//!
//! [`ExecLocalError`] covers the failure modes specific to spawning and
//! managing child processes on the local machine: spawn failures, timeouts,
//! I/O errors, and cancellation.

/// Errors that can occur during local process execution.
#[derive(Debug, thiserror::Error)]
pub enum ExecLocalError {
    /// The child process could not be spawned (e.g., shell not found).
    #[error("process failed to spawn: {0}")]
    SpawnFailed(std::io::Error),

    /// The job exceeded its configured timeout and was killed.
    #[error("job timed out after {timeout_secs}s")]
    Timeout {
        /// The timeout duration that was exceeded, in seconds.
        timeout_secs: u64,
    },

    /// A generic I/O error (file creation, log writing, etc.).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The job was cancelled by the scheduler or user (e.g., Ctrl+C).
    #[error("job cancelled")]
    Cancelled,

    /// The execution block type is not supported by the local executor.
    #[error("unsupported execution block: {0}")]
    UnsupportedExecution(String),

    /// An output path escapes the project root directory (path traversal).
    #[error("output path escapes project root: {path} resolves outside {root}")]
    OutputPathEscapesRoot {
        /// The offending output path.
        path: String,
        /// The project root that the path must stay within.
        root: String,
    },

    /// A declared output file was not produced by a successful job.
    #[error("output not produced: {path}")]
    OutputMissing {
        /// The path that was expected but not found after job exit 0.
        path: String,
    },

    /// Atomic rename of an output file failed during finalization.
    #[error("atomic write failed for {path}: {reason}")]
    AtomicWriteFailed {
        /// The output path involved in the failed rename.
        path: String,
        /// Description of what went wrong.
        reason: String,
    },

    /// Another `ox run` session is already executing this job's output set.
    ///
    /// Two sessions approved for the same gate both reach the job because
    /// the cooperative claim protocol (`StateDb::claim_job`, ADR-012) is not
    /// yet the scheduling gate. Rather than execute concurrently into the
    /// same paths and risk committing a mixed output set (issue #2), the
    /// session that does not hold the output-set lock fails closed here.
    #[error(
        "job '{job}' is already being executed by another session (pid {holder}); \
         refusing to run it concurrently, which could commit a mixed output set \
         (the claim protocol is not yet the scheduling gate — see ADR-012)"
    )]
    ConcurrentExecution {
        /// The job whose output set is already locked.
        job: String,
        /// PID of the session holding the lock (or `unknown`).
        holder: String,
    },
}
