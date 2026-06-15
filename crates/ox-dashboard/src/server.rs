//! Axum server setup and routing.
//!
//! [`create_router`] builds the full application router with all API
//! endpoints and the static HTML shell.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;

use crate::api;

/// Shared state for all dashboard handlers.
///
/// Holds the path to `state.db` so each request can open a read-only
/// connection.  The database uses WAL mode, allowing concurrent reads
/// while the OxyMake engine writes.
pub struct DashboardState {
    /// Path to the `.oxymake/state.db` SQLite database.
    pub db_path: PathBuf,
}

/// Build the dashboard [`Router`] with all routes wired up.
///
/// # Routes
///
/// - `GET /` — HTML dashboard shell (index page)
/// - `GET /api/status` — JSON workflow status summary
/// - `GET /api/dag` — JSON DAG structure (nodes + edges)
/// - `GET /api/jobs` — JSON job list (filterable via `?status=`)
/// - `GET /api/events` — SSE stream of live state changes
/// - `GET /api/runs` — JSON run history with timing
/// - `GET /api/runs/:id/jobs` — JSON job history for a specific run
/// - `GET /api/stats/rules` — JSON per-rule pipeline statistics
/// - `GET /api/gates` — JSON approval gates with status
/// - `GET /api/job/:id` — JSON single job detail (status + log info)
pub fn create_router(state: Arc<DashboardState>) -> Router {
    Router::new()
        .route("/", get(api::index_handler))
        .route("/api/status", get(api::api_status))
        .route("/api/dag", get(api::api_dag))
        .route("/api/jobs", get(api::api_jobs))
        .route("/api/events", get(api::api_events_sse))
        .route("/api/runs", get(api::api_runs))
        .route("/api/runs/{id}/jobs", get(api::api_run_jobs))
        .route("/api/stats/rules", get(api::api_stats_rules))
        .route("/api/gates", get(api::api_gates))
        .route("/api/job/{id}", get(api::api_job_detail))
        .with_state(state)
}
