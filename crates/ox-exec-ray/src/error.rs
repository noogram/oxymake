//! Error types for the Ray executor.

/// Errors that can occur during Ray job submission, polling, or cancellation.
#[derive(Debug, thiserror::Error)]
pub enum RayError {
    /// HTTP request to the Ray Jobs API failed.
    #[error("Ray API request failed: {0}")]
    ApiRequest(#[from] reqwest::Error),

    /// Ray Jobs API returned a non-success HTTP status.
    #[error("Ray API returned {status}: {body}")]
    ApiStatus { status: u16, body: String },

    /// Failed to parse a Ray Jobs API response.
    #[error("failed to parse Ray API response: {0}")]
    ParseError(String),

    /// Ray dashboard is unreachable.
    #[error("Ray cluster unreachable: {0}")]
    ClusterUnreachable(String),

    /// Ray job not found.
    #[error("Ray job {submission_id} not found")]
    JobNotFound { submission_id: String },

    /// OxyMake job ID not found in the executor's tracking map.
    #[error("job {job_id} not tracked by Ray executor")]
    JobNotTracked { job_id: String },

    /// Call-mode wrapper generation or execution error.
    #[error("Ray call mode error: {0}")]
    CallModeError(String),

    /// Placement group operation failed.
    #[error("placement group error: {0}")]
    PlacementGroup(String),

    /// Unsupported environment type for Ray runtime_env.
    #[error("unsupported environment for Ray: {0}")]
    UnsupportedEnv(String),

    /// I/O error (file creation, etc.).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
