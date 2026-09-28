//! Error types for the Ray executor.

/// Errors that can occur during Ray job submission, polling, or cancellation.
#[derive(Debug, thiserror::Error)]
pub enum RayError {
    /// Token mode was requested but no token could be loaded.
    #[error(
        "RAY_AUTH_MODE=token but no Ray authentication token was found; tried RAY_AUTH_TOKEN, RAY_AUTH_TOKEN_PATH, and ~/.ray/auth_token"
    )]
    AuthTokenMissing,

    /// A configured token source could not be read.
    #[error("failed to read Ray authentication token from {token_source} ({path}): {reason}")]
    AuthTokenRead {
        token_source: String,
        path: std::path::PathBuf,
        reason: String,
    },

    /// A loaded token cannot be represented as an HTTP header value.
    #[error("Ray authentication token from {token_source} is malformed: {reason}")]
    AuthTokenMalformed {
        token_source: String,
        reason: String,
    },

    /// The dashboard rejected a request because token authentication is required.
    #[error(
        "Ray dashboard returned 401: the cluster requires a Ray token; RAY_AUTH_MODE is {mode}; token sources consulted: {sources}"
    )]
    AuthRequired { mode: String, sources: String },

    /// The live State API snapshot could not be decoded safely.
    #[error(
        "Ray node inspection malformed or unknown payload: {0}\n  use --ray-allow-pending to bypass node inspection and wait for future capacity"
    )]
    NodeInspectionPayload(String),
    /// The bounded node inspection deadline elapsed.
    #[error(
        "Ray node inspection timed out after {0:?}\n  use --ray-allow-pending to bypass node inspection and wait for future capacity"
    )]
    NodeInspectionTimeout(std::time::Duration),
    /// The node inspection request could not reach the dashboard.
    #[error(
        "Ray node inspection connection error: {0}\n  use --ray-allow-pending to bypass node inspection and wait for future capacity"
    )]
    NodeInspectionConnection(String),
    /// No node has the task's complete logical resource request.
    #[error("{0}")]
    InfeasibleRequest(String),

    /// A declared resource cannot be represented by Ray.
    #[error("invalid Ray resource declaration: {0}")]
    ResourceMapping(#[from] crate::resource_mapper::ResourceMapError),

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

    /// The generated driver cannot be submitted as a shared-filesystem path.
    #[error(
        "Ray DAG driver is unavailable at {path}; the Ray executor requires the cluster to see OxyMake's working directory through a shared filesystem"
    )]
    DriverUnavailable { path: std::path::PathBuf },
}
