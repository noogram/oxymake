//! # ox-dashboard — Web Dashboard for OxyMake
//!
//! Browser-based monitoring and DAG visualization for OxyMake workflows.
//! Part of the Phase 2 monitoring tier (see `monitoring-roadmap.md`).
//!
//! ## Architecture
//!
//! - **axum** serves HTML (via askama templates) and JSON API endpoints
//! - **htmx** provides dynamic updates without custom JavaScript
//! - **SSE** (Server-Sent Events) streams live state changes to the browser
//! - **state.db** is the sole data source (read-only SQLite via WAL mode)
//!
//! ## Endpoints
//!
//! | Route | Description |
//! |-------|-------------|
//! | `GET /` | Dashboard shell (HTML) |
//! | `GET /api/status` | Current workflow status (JSON) |
//! | `GET /api/dag` | DAG structure as nodes + edges (JSON) |
//! | `GET /api/jobs` | Job list with optional `?status=` filter (JSON) |
//! | `GET /api/events` | SSE stream of live state changes |
//!
//! ## Quick start
//!
//! ```no_run
//! use std::sync::Arc;
//! use ox_dashboard::server::{DashboardState, create_router};
//!
//! # async fn run() {
//! let state = Arc::new(DashboardState {
//!     db_path: ".oxymake/state.db".into(),
//! });
//! let app = create_router(state);
//! let listener = tokio::net::TcpListener::bind("127.0.0.1:9876").await.unwrap();
//! axum::serve(listener, app).await.unwrap();
//! # }
//! ```

pub mod api;
pub mod server;

/// Start the dashboard HTTP server on the given address.
///
/// This is the main entry point used by `ox dashboard`. The `app` router
/// is created by [`server::create_router`].
pub async fn serve(addr: &str, app: axum::Router) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
