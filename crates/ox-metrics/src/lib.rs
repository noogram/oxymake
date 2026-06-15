//! # ox-metrics — Prometheus Metrics Export for OxyMake
//!
//! This crate provides Prometheus-compatible metrics for monitoring
//! OxyMake workflow execution. It is part of the monitoring layer
//! described in the [monitoring roadmap](../../docs/design/monitoring-roadmap.md)
//! and follows the "read-only" principle: it observes execution state
//! but never modifies it.
//!
//! ## Architecture
//!
//! ```text
//!   ox-state (state.db)
//!        │
//!        ▼
//!   OxMetrics::refresh_from_db()   ← polling (every scrape interval)
//!        │
//!        ▼
//!   OxMetrics::encode()            → Prometheus text format
//!        │
//!        ▼
//!   GET /metrics (axum)            → Prometheus scraper
//! ```
//!
//! ## Exposed Metrics
//!
//! | Metric | Type | Labels | Description |
//! |--------|------|--------|-------------|
//! | `oxymake_jobs_total` | Gauge | `status` | Job counts by status |
//! | `oxymake_sessions_active` | Gauge | — | Active session count |
//! | `oxymake_job_duration_seconds` | Histogram | `rule` | Job execution duration |
//! | `oxymake_cache_hits_total` | Gauge | — | Total cache hits |
//! | `oxymake_cache_misses_total` | Gauge | — | Total cache misses |
//!
//! ## Quick Start
//!
//! ```no_run
//! use std::sync::Arc;
//! use ox_metrics::OxMetrics;
//! use ox_metrics::server::serve_metrics;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let metrics = Arc::new(OxMetrics::new());
//! // Spawn the metrics server on port 9091.
//! serve_metrics(metrics, 9091).await?;
//! # Ok(())
//! # }
//! ```

pub mod metrics;
pub mod server;

// Re-export the primary type for convenience.
pub use metrics::OxMetrics;
