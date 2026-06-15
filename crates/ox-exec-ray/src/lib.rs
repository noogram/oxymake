//! # Ray Executor
//!
//! Submits OxyMake jobs to Ray clusters via the Ray Jobs API and polls
//! completion via HTTP. Implements the `Executor` trait from `ox-core`.
//!
//! ## Architecture
//!
//! - **Job submission**: builds an entrypoint command from the execution block,
//!   submits via `POST /api/jobs/` to the Ray dashboard, captures the
//!   submission ID.
//! - **Status polling**: adaptive backoff using `GET /api/jobs/{id}` — faster
//!   than SLURM since the Ray Jobs API is lightweight HTTP.
//! - **Cancellation**: `POST /api/jobs/{id}/stop` by submission ID.
//!
//! ## Features
//!
//! ### Phase 1 (Core)
//! - Shell and Script execution blocks via Ray Jobs API
//! - Resource mapping: cpu, mem, gpu → Ray entrypoint resources
//! - Health check via `GET /api/version`
//! - Adaptive polling with backoff
//!
//! ### Phase 2 (Object Store & Call Mode)
//! - `MaterializePolicy` mapping to Ray object store
//! - `OutputRef::InMemory` support via `ray.put()`/`ray.get()`
//! - Call-mode execution with Arrow IPC through object store
//! - Object reference manifest for cross-job data passing
//!
//! ### Phase 3 (Advanced)
//! - **Runtime environments**: Conda, pip/uv, Docker → Ray `runtime_env`
//! - **Autoscaler-aware concurrency**: Dynamic limits from cluster state
//! - **Placement groups**: Multi-node job co-scheduling (PACK, SPREAD)
//! - **Fractional GPU**: Sub-GPU scheduling (e.g., 0.25 GPU per job)
//! - **Dashboard metrics**: Cluster monitoring for ox-monitor-tui
//! - **Job arrays**: Wildcard expansion emulation via batched submissions

pub mod autoscaler;
pub mod call_mode;
pub mod dashboard;
pub mod driver_script;
pub mod error;
pub mod executor;
pub mod job_array;
pub mod object_store;
pub mod placement_group;
pub mod ray_client;
pub mod resource_mapper;
pub mod runtime_env;

pub use autoscaler::{AutoscalerAdvisor, ClusterResources};
pub use dashboard::{ClusterMetrics, JobListResponse, JobSummary, OxymakeClusterSummary};
pub use error::RayError;
pub use executor::RayConfig;
pub use executor::RayExecutor;
pub use job_array::{JobArraySpec, JobArrayStatus};
pub use placement_group::{PlacementGroupConfig, PlacementStrategy};

/// Create an HTTP client suitable for Ray API calls.
///
/// Convenience helper for consumers that need a `reqwest::Client` to
/// construct a [`ray_client::RayClient`] without depending on `reqwest`
/// directly.
pub fn ray_client_http(timeout: std::time::Duration) -> Result<reqwest::Client, RayError> {
    Ok(reqwest::Client::builder().timeout(timeout).build()?)
}
