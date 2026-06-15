//! Cache pruning pass: marks jobs as skipped when outputs are up-to-date.
//!
//! This pass is the primary mechanism for incremental builds. It receives a
//! set of job IDs whose outputs are already cached and up-to-date (determined
//! by `ox-cache`), and marks those jobs as "skipped" in the JobGraph.
//!
//! Skipped jobs are removed from the active job index but their output nodes
//! remain in the graph, preserving connectivity for downstream jobs that
//! still need to execute.
//!
//! # How it works
//!
//! 1. For each job ID in `cached_jobs`:
//!    - If the job exists in the graph, call `mark_skipped`.
//!    - If the job does not exist (already removed by a prior pass), skip it.
//! 2. Return the modified graph with a count of how many jobs were pruned.
//!
//! # Example
//!
//! ```
//! use ox_plan::prune::CachePruningPass;
//! use ox_plan::pass::OptimizationPass;
//! use ox_core::job_graph::{JobGraph, make_test_job};
//! use ox_core::model::JobId;
//! use std::collections::HashSet;
//!
//! let graph = JobGraph::build(vec![
//!     make_test_job("download-A", &[], &["data/A.fastq"]),
//!     make_test_job("align-A", &["data/A.fastq"], &["results/A.bam"]),
//! ]).unwrap();
//!
//! // Suppose "download-A" is cached.
//! let pass = CachePruningPass {
//!     cached_jobs: HashSet::from([JobId::from("download-A")]),
//! };
//!
//! let (pruned_graph, result) = pass.optimize(graph).unwrap();
//! assert_eq!(pruned_graph.job_count(), 1); // only align-A remains
//! assert_eq!(result.jobs_affected, 1);
//! ```

use std::collections::HashSet;

use ox_core::job_graph::JobGraph;
use ox_core::model::JobId;

use crate::pass::{OptimizationPass, PassResult};

// ---------------------------------------------------------------------------
// CachePruningPass
// ---------------------------------------------------------------------------

/// Cache pruning pass: marks jobs as skipped when outputs are up-to-date.
///
/// For now, this is a stub that takes a set of "cached job IDs" and marks
/// them. The actual cache-checking logic (comparing content hashes, checking
/// mtimes, etc.) lives in `ox-cache`. This pass simply acts on the results
/// of that check.
///
/// ```
/// use ox_plan::prune::CachePruningPass;
/// use ox_plan::pass::OptimizationPass;
/// use ox_core::job_graph::{JobGraph, make_test_job};
/// use ox_core::model::JobId;
/// use std::collections::HashSet;
///
/// let graph = JobGraph::build(vec![
///     make_test_job("A", &[], &["a.txt"]),
///     make_test_job("B", &["a.txt"], &["b.txt"]),
///     make_test_job("C", &["b.txt"], &["c.txt"]),
/// ]).unwrap();
///
/// // Mark A and B as cached.
/// let pass = CachePruningPass {
///     cached_jobs: HashSet::from([JobId::from("A"), JobId::from("B")]),
/// };
///
/// let (pruned, result) = pass.optimize(graph).unwrap();
/// assert_eq!(pruned.job_count(), 1); // only C remains
/// assert_eq!(result.jobs_affected, 2);
/// assert!(result.summary.contains("2"));
/// ```
pub struct CachePruningPass {
    /// Job IDs whose outputs are already cached and up-to-date.
    pub cached_jobs: HashSet<JobId>,
}

impl OptimizationPass for CachePruningPass {
    fn name(&self) -> &str {
        "cache_pruning"
    }

    /// Mark cached jobs as skipped in the graph.
    ///
    /// Jobs in `cached_jobs` that exist in the graph are marked as skipped.
    /// Jobs that do not exist are silently ignored (they may have been
    /// removed by a prior pass).
    fn optimize(
        &self,
        mut graph: JobGraph,
    ) -> Result<(JobGraph, PassResult), Box<dyn std::error::Error>> {
        let mut pruned_count = 0;

        for job_id in &self.cached_jobs {
            if graph.get_job(job_id).is_some() {
                graph.mark_skipped(job_id);
                pruned_count += 1;
            }
        }

        let result = PassResult {
            pass_name: self.name().into(),
            jobs_affected: pruned_count,
            summary: format!(
                "Marked {pruned_count} job(s) as skipped (cached), {} remain",
                graph.job_count()
            ),
        };

        Ok((graph, result))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::job_graph::make_test_job;

    #[test]
    fn prune_no_cached_jobs() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ])
        .unwrap();

        let pass = CachePruningPass {
            cached_jobs: HashSet::new(),
        };

        let (result_graph, result) = pass.optimize(graph).unwrap();
        assert_eq!(result_graph.job_count(), 2);
        assert_eq!(result.jobs_affected, 0);
    }

    #[test]
    fn prune_some_cached_jobs() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["b.txt"], &["c.txt"]),
        ])
        .unwrap();

        let pass = CachePruningPass {
            cached_jobs: HashSet::from([JobId::from("A")]),
        };

        let (result_graph, result) = pass.optimize(graph).unwrap();
        assert_eq!(result_graph.job_count(), 2); // B and C remain
        assert_eq!(result.jobs_affected, 1);
        assert!(result_graph.get_job(&JobId::from("A")).is_none());
        assert!(result_graph.get_job(&JobId::from("B")).is_some());
    }

    #[test]
    fn prune_all_cached_jobs() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ])
        .unwrap();

        let pass = CachePruningPass {
            cached_jobs: HashSet::from([JobId::from("A"), JobId::from("B")]),
        };

        let (result_graph, result) = pass.optimize(graph).unwrap();
        assert_eq!(result_graph.job_count(), 0);
        assert_eq!(result.jobs_affected, 2);
    }

    #[test]
    fn prune_nonexistent_job_is_ignored() {
        let graph = JobGraph::build(vec![make_test_job("A", &[], &["a.txt"])]).unwrap();

        let pass = CachePruningPass {
            cached_jobs: HashSet::from([JobId::from("nonexistent")]),
        };

        let (result_graph, result) = pass.optimize(graph).unwrap();
        assert_eq!(result_graph.job_count(), 1); // A still there
        assert_eq!(result.jobs_affected, 0);
    }

    #[test]
    fn prune_in_pipeline() {
        use crate::pass::run_passes;

        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["b.txt"], &["c.txt"]),
        ])
        .unwrap();

        let passes: Vec<Box<dyn OptimizationPass>> = vec![Box::new(CachePruningPass {
            cached_jobs: HashSet::from([JobId::from("A"), JobId::from("B")]),
        })];

        let (result_graph, results) = run_passes(graph, &passes).unwrap();
        assert_eq!(result_graph.job_count(), 1); // only C remains
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].pass_name, "cache_pruning");
        assert_eq!(results[0].jobs_affected, 2);
    }
}
