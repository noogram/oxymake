//! # Optimization Pass Trait
//!
//! Defines the interface for JobGraph optimization passes. Each pass
//! takes a JobGraph and returns a (possibly transformed) JobGraph.
//!
//! Passes are composable, ordered, and can be enabled/disabled via
//! configuration. They follow the DataFusion pattern: each pass is
//! a pure transformation, making the pipeline predictable and debuggable.
//!
//! Built-in passes (in `ox-plan`):
//! - `CachePruning`: mark up-to-date jobs as Skipped
//! - `TaskFusion`: merge sequential call-mode jobs
//! - `MaterializationElimination`: remove unnecessary serialization
//! - `CriticalPathAnalysis`: prioritize bottleneck jobs
//! - `GroupScheduling`: bundle jobs for batch submission
//! - `PartitionPlanning`: route subgraphs to executors

/// Metadata about an optimization pass execution.
#[derive(Debug, Clone)]
pub struct PassResult {
    /// Name of the pass that ran.
    pub pass_name: String,
    /// Number of jobs affected by this pass.
    pub jobs_affected: usize,
    /// Human-readable summary of what changed.
    pub summary: String,
    /// Duration of the pass in microseconds.
    pub duration_us: u64,
}

/// Context available to optimization passes.
#[derive(Debug, Clone)]
pub struct PlanContext {
    /// Global concurrency limit.
    pub max_jobs: usize,
    /// Available executor capabilities.
    pub executor_capabilities: Vec<String>,
    /// Whether this is a dry-run (no execution will follow).
    pub dry_run: bool,
}

// Note: The `OptimizationPass` trait itself will be defined in `ox-plan`
// because it takes and returns `JobGraph`, which depends on `petgraph`
// types that are internal to the graph construction module. Here we
// define only the supporting types that other crates need.
//
// The trait signature (in ox-plan):
//
// pub trait OptimizationPass: Send + Sync {
//     fn name(&self) -> &str;
//     fn optimize(&self, graph: JobGraph, ctx: &PlanContext) -> Result<(JobGraph, PassResult)>;
// }
