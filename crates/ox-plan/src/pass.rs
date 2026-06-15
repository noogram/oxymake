//! Optimization pass infrastructure for OxyMake's JobGraph.
//!
//! An optimization pass receives a [`JobGraph`], transforms it, and returns
//! the modified graph along with a [`PassResult`] describing what changed.
//! Passes are composable: the [`run_passes`] function chains them in sequence,
//! threading the graph through each pass and collecting results.
//!
//! # Design
//!
//! Each pass takes **ownership** of the graph and returns it. This avoids
//! lifetime complexity and makes it clear that passes may restructure the
//! graph arbitrarily (add/remove nodes, rewrite edges, etc.).
//!
//! ```text
//!   JobGraph ──▶ Pass 1 ──▶ Pass 2 ──▶ ... ──▶ Pass N ──▶ optimized JobGraph
//!                  │            │                    │
//!                  ▼            ▼                    ▼
//!              PassResult  PassResult           PassResult
//! ```
//!
//! # Built-in passes
//!
//! - [`CachePruningPass`](crate::prune::CachePruningPass): marks cached jobs as skipped.
//! - (future) `CriticalPathPass`: annotates the longest chain.
//! - (future) `ResourcePartitionPass`: groups jobs by resource class.
//!
//! # Example
//!
//! ```
//! use ox_plan::pass::{OptimizationPass, PassResult, run_passes};
//! use ox_core::job_graph::{JobGraph, make_test_job};
//!
//! // Build a simple graph.
//! let graph = JobGraph::build(vec![
//!     make_test_job("A", &[], &["a.txt"]),
//!     make_test_job("B", &["a.txt"], &["b.txt"]),
//! ]).unwrap();
//!
//! // Run with no passes — graph is unchanged.
//! let (result_graph, results) = run_passes(graph, &[]).unwrap();
//! assert_eq!(result_graph.job_count(), 2);
//! assert!(results.is_empty());
//! ```

use ox_core::job_graph::JobGraph;

// ---------------------------------------------------------------------------
// PassResult
// ---------------------------------------------------------------------------

/// Metadata about an optimization pass execution.
///
/// Every pass returns this alongside the modified graph, providing
/// visibility into what the pass did without requiring the caller
/// to diff the graph before and after.
///
/// ```
/// use ox_plan::pass::PassResult;
///
/// let result = PassResult {
///     pass_name: "cache_pruning".into(),
///     jobs_affected: 3,
///     summary: "Marked 3 jobs as skipped (cached)".into(),
/// };
/// assert_eq!(result.pass_name, "cache_pruning");
/// assert_eq!(result.jobs_affected, 3);
/// ```
#[derive(Debug, Clone)]
pub struct PassResult {
    /// Name of the pass that produced this result.
    pub pass_name: String,
    /// Number of jobs affected by the pass.
    pub jobs_affected: usize,
    /// Human-readable summary of what the pass did.
    pub summary: String,
}

// ---------------------------------------------------------------------------
// OptimizationPass trait
// ---------------------------------------------------------------------------

/// An optimization pass that transforms a [`JobGraph`].
///
/// Implementors receive ownership of the graph, apply their transformation,
/// and return the modified graph along with a [`PassResult`].
///
/// Passes must be `Send + Sync` so they can be stored in shared pass
/// registries and invoked from async contexts.
pub trait OptimizationPass: Send + Sync {
    /// The name of this pass (used in logging and [`PassResult`]).
    fn name(&self) -> &str;

    /// Apply the optimization to the graph.
    ///
    /// Returns the modified graph and a result describing what changed.
    fn optimize(
        &self,
        graph: JobGraph,
    ) -> Result<(JobGraph, PassResult), Box<dyn std::error::Error>>;
}

// ---------------------------------------------------------------------------
// Pass pipeline
// ---------------------------------------------------------------------------

/// Run a pipeline of optimization passes on a [`JobGraph`].
///
/// Passes are applied in order, each receiving the graph returned by the
/// previous pass. Results from all passes are collected and returned.
///
/// # Errors
///
/// Returns the first error encountered by any pass. The graph is consumed
/// by the failing pass and cannot be recovered.
///
/// # Example
///
/// ```
/// use ox_plan::pass::{run_passes, OptimizationPass, PassResult};
/// use ox_plan::prune::CachePruningPass;
/// use ox_core::job_graph::{JobGraph, make_test_job};
/// use ox_core::model::JobId;
/// use std::collections::HashSet;
///
/// let graph = JobGraph::build(vec![
///     make_test_job("A", &[], &["a.txt"]),
///     make_test_job("B", &["a.txt"], &["b.txt"]),
/// ]).unwrap();
///
/// let passes: Vec<Box<dyn OptimizationPass>> = vec![
///     Box::new(CachePruningPass {
///         cached_jobs: HashSet::from([JobId::from("A")]),
///     }),
/// ];
///
/// let (result_graph, results) = run_passes(graph, &passes).unwrap();
/// assert_eq!(result_graph.job_count(), 1); // A was pruned
/// assert_eq!(results.len(), 1);
/// assert_eq!(results[0].pass_name, "cache_pruning");
/// assert_eq!(results[0].jobs_affected, 1);
/// ```
pub fn run_passes(
    mut graph: JobGraph,
    passes: &[Box<dyn OptimizationPass>],
) -> Result<(JobGraph, Vec<PassResult>), Box<dyn std::error::Error>> {
    let mut results = Vec::with_capacity(passes.len());

    for pass in passes {
        let (new_graph, result) = pass.optimize(graph)?;
        graph = new_graph;
        results.push(result);
    }

    Ok((graph, results))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::job_graph::make_test_job;

    /// A no-op pass for testing the pipeline.
    struct NoOpPass;

    impl OptimizationPass for NoOpPass {
        fn name(&self) -> &str {
            "noop"
        }

        fn optimize(
            &self,
            graph: JobGraph,
        ) -> Result<(JobGraph, PassResult), Box<dyn std::error::Error>> {
            Ok((
                graph,
                PassResult {
                    pass_name: "noop".into(),
                    jobs_affected: 0,
                    summary: "Did nothing".into(),
                },
            ))
        }
    }

    #[test]
    fn run_passes_empty_pipeline() {
        let graph = JobGraph::build(vec![make_test_job("A", &[], &["a.txt"])]).unwrap();

        let (result_graph, results) = run_passes(graph, &[]).unwrap();
        assert_eq!(result_graph.job_count(), 1);
        assert!(results.is_empty());
    }

    #[test]
    fn run_passes_single_noop() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ])
        .unwrap();

        let passes: Vec<Box<dyn OptimizationPass>> = vec![Box::new(NoOpPass)];

        let (result_graph, results) = run_passes(graph, &passes).unwrap();
        assert_eq!(result_graph.job_count(), 2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].pass_name, "noop");
        assert_eq!(results[0].jobs_affected, 0);
    }

    #[test]
    fn run_passes_multiple_passes() {
        let graph = JobGraph::build(vec![make_test_job("A", &[], &["a.txt"])]).unwrap();

        let passes: Vec<Box<dyn OptimizationPass>> =
            vec![Box::new(NoOpPass), Box::new(NoOpPass), Box::new(NoOpPass)];

        let (result_graph, results) = run_passes(graph, &passes).unwrap();
        assert_eq!(result_graph.job_count(), 1);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn pass_name_method() {
        let pass = NoOpPass;
        assert_eq!(pass.name(), "noop");
    }

    #[test]
    fn pass_result_clone_and_debug() {
        let result = PassResult {
            pass_name: "test".into(),
            jobs_affected: 5,
            summary: "did stuff".into(),
        };
        let cloned = result.clone();
        assert_eq!(cloned.pass_name, "test");
        let debug = format!("{:?}", cloned);
        assert!(debug.contains("test"));
    }
}
