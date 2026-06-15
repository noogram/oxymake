//! # Reporter Trait
//!
//! Defines the plugin interface for event reporting. Reporters receive
//! structured [`Event`]s and present them to their audience.
//!
//! Built-in reporters:
//! - `ox-report-term`: progress bars, colors, error chains for humans
//! - `ox-report-json`: NDJSON for agents and programmatic consumption

use std::future::Future;
use std::pin::Pin;

use crate::model::Event;

/// Summary of a completed workflow run.
#[derive(Debug, Clone)]
pub struct RunSummary {
    /// Total number of jobs in the DAG.
    pub total_jobs: usize,
    /// Jobs that executed successfully.
    pub succeeded: usize,
    /// Jobs that failed.
    pub failed: usize,
    /// Jobs skipped (cached or downstream of failure).
    pub skipped: usize,
    /// Total wall-clock duration of the run.
    pub duration_ms: u64,
}

/// The reporter trait — the plugin interface for event presentation.
///
/// Reporters are **observers** — they format events for a specific
/// audience but never filter, suppress, or modify them. The scheduler
/// emits events; reporters present them.
///
/// # Implementors
///
/// - `ox-report-term`: terminal output with progress bars and colors
/// - `ox-report-json`: NDJSON output for agent consumption
/// - Future: GitHub Actions reporter, webhook reporter
pub trait Reporter: Send + Sync {
    /// Handle a single event.
    ///
    /// Called for every event emitted by the scheduler. Implementations
    /// should be fast — the scheduler does not wait for reporters to
    /// finish before proceeding.
    fn on_event<'a>(&'a self, event: &'a Event) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called when the run completes (successfully or not).
    ///
    /// Use this for final summary output, cleanup, or flushing buffers.
    fn finish<'a>(
        &'a self,
        summary: &'a RunSummary,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
