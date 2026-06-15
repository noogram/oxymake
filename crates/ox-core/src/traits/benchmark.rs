//! # Benchmark Sink Trait
//!
//! Defines the plugin interface for persisting benchmark timing data.
//! The scheduler calls [`BenchmarkSink::write_benchmark`] after a job
//! completes successfully and has a `benchmark` path configured.
//!
//! This trait keeps file I/O out of ox-core: the scheduler delegates
//! benchmark persistence to the caller-provided implementation.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::traits::executor::JobResult;

/// Format benchmark timing data as TSV content.
///
/// Returns a string with a header row and one data row containing:
/// `s` (wall-clock seconds), `h:m:s` (wall-clock formatted), `max_rss`
/// (peak RSS in MB), `cpu_time` (CPU seconds).
pub fn format_benchmark_tsv(result: &JobResult) -> String {
    let wall_secs = result.duration.as_secs_f64();
    let total_secs = result.duration.as_secs();
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    let hms = format!("{h}:{m:02}:{s:02}");

    let max_rss_str = match result.peak_memory_bytes {
        Some(b) => format!("{:.2}", b as f64 / (1024.0 * 1024.0)),
        None => "-".to_string(),
    };
    let cpu_str = match result.cpu_time {
        Some(d) => format!("{:.4}", d.as_secs_f64()),
        None => "-".to_string(),
    };

    format!("s\th:m:s\tmax_rss\tcpu_time\n{wall_secs:.4}\t{hms}\t{max_rss_str}\t{cpu_str}\n")
}

/// A sink for benchmark data that the scheduler calls on job completion.
///
/// The trait is object-safe and `Send + Sync` so it can be wrapped in `Arc`
/// and shared across async tasks.
pub trait BenchmarkSink: Send + Sync {
    /// Persist benchmark data for a completed job.
    ///
    /// `path` is the benchmark output path from the job's configuration.
    /// `result` contains timing and resource-usage data.
    fn write_benchmark<'a>(
        &'a self,
        path: &'a Path,
        result: &'a JobResult,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
