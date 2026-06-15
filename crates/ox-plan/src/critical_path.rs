//! Critical-path analysis pass for job prioritization.
//!
//! Identifies the longest chain of jobs in the DAG (the critical path) using
//! dynamic programming on the topological order. Jobs on the critical path
//! are bottlenecks — their latency directly determines total build time.
//!
//! # Stage 2 integration
//!
//! The scheduler uses critical-path annotations to gate in-memory
//! materialization: only outputs produced by critical-path jobs are held
//! in memory (avoiding memory pressure from off-path data). The eviction
//! policy (Belady largest-first) runs on top of this gating.
//!
//! # Algorithm
//!
//! 1. Topological sort of job nodes.
//! 2. Forward pass: for each job in topological order, compute the length
//!    of the longest path ending at that job (`dist[j] = 1 + max(dist[u]
//!    for u in upstream(j))`).
//! 3. Find the job with maximum `dist` (the critical path tail).
//! 4. Backtrack from the tail through predecessors to reconstruct the path.
//!
//! Complexity: O(V + E) where V = jobs, E = dependency edges.

use std::collections::{HashMap, HashSet};

use ox_core::job_graph::JobGraph;
use ox_core::model::JobId;

use crate::pass::{OptimizationPass, PassResult};

/// Optimization pass that identifies the critical path (longest chain) in the
/// job graph.
///
/// The pass does not modify the graph — it computes metadata (the set of
/// critical-path job IDs) that the scheduler uses for memory-gating decisions.
///
/// ```
/// use ox_plan::critical_path::CriticalPathPass;
/// use ox_plan::pass::OptimizationPass;
/// use ox_core::job_graph::{JobGraph, make_test_job};
///
/// let graph = JobGraph::build(vec![
///     make_test_job("A", &[], &["a.txt"]),
///     make_test_job("B", &["a.txt"], &["b.txt"]),
///     make_test_job("C", &["b.txt"], &["c.txt"]),
/// ]).unwrap();
///
/// let pass = CriticalPathPass::new();
/// let (result_graph, pass_result) = pass.optimize(graph).unwrap();
/// assert_eq!(pass_result.jobs_affected, 3); // A → B → C is the critical path
///
/// let critical_jobs = pass.critical_path_jobs();
/// assert_eq!(critical_jobs.len(), 3);
/// ```
pub struct CriticalPathPass {
    /// The set of job IDs on the critical path, populated after `optimize`.
    critical_path_jobs: std::sync::Mutex<HashSet<JobId>>,
}

impl CriticalPathPass {
    /// Create a new critical-path analysis pass.
    pub fn new() -> Self {
        Self {
            critical_path_jobs: std::sync::Mutex::new(HashSet::new()),
        }
    }

    /// Compute the critical path from a borrowed graph reference.
    ///
    /// Unlike [`OptimizationPass::optimize`], this does not take ownership
    /// of the graph. Use this when you need critical-path information without
    /// participating in the optimization pass pipeline.
    pub fn compute(&self, graph: &JobGraph) -> HashSet<JobId> {
        let path = compute_critical_path(graph);
        let job_set: HashSet<JobId> = path.into_iter().collect();
        *self.critical_path_jobs.lock().unwrap() = job_set.clone();
        job_set
    }

    /// Return the set of job IDs on the critical path.
    ///
    /// Empty before `optimize` or `compute` is called.
    pub fn critical_path_jobs(&self) -> HashSet<JobId> {
        self.critical_path_jobs.lock().unwrap().clone()
    }
}

impl Default for CriticalPathPass {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the critical path (longest chain) in a job graph.
///
/// Returns the ordered list of job IDs on the critical path, from source to
/// sink.
pub fn compute_critical_path(graph: &JobGraph) -> Vec<JobId> {
    let topo = match graph.topological_order() {
        Ok(order) => order,
        Err(_) => return vec![], // cycle — no valid critical path
    };

    if topo.is_empty() {
        return vec![];
    }

    // Forward pass: compute longest path ending at each job.
    let mut dist: HashMap<&JobId, usize> = HashMap::new();
    let mut predecessor: HashMap<&JobId, &JobId> = HashMap::new();

    for job_id in &topo {
        let upstream = graph.upstream(job_id);
        let (best_dist, best_pred) = upstream
            .iter()
            .filter_map(|u| dist.get(u).map(|d| (*d, *u)))
            .max_by_key(|(d, _)| *d)
            .unwrap_or((0, job_id)); // root: distance 0, no predecessor

        let my_dist = best_dist + 1;
        dist.insert(job_id, my_dist);
        if best_pred != *job_id {
            predecessor.insert(job_id, best_pred);
        }
    }

    // Find the tail of the critical path (maximum distance).
    let tail = match topo.iter().max_by_key(|id| dist.get(*id).unwrap_or(&0)) {
        Some(id) => *id,
        None => return vec![],
    };

    // Backtrack to reconstruct the path.
    let mut path = vec![tail.clone()];
    let mut current = tail;
    while let Some(&pred) = predecessor.get(current) {
        path.push(pred.clone());
        current = pred;
    }
    path.reverse();
    path
}

impl OptimizationPass for CriticalPathPass {
    fn name(&self) -> &str {
        "critical_path"
    }

    fn optimize(
        &self,
        graph: JobGraph,
    ) -> Result<(JobGraph, PassResult), Box<dyn std::error::Error>> {
        let path = compute_critical_path(&graph);
        let jobs_affected = path.len();
        let job_set: HashSet<JobId> = path.into_iter().collect();

        *self.critical_path_jobs.lock().unwrap() = job_set;

        Ok((
            graph,
            PassResult {
                pass_name: "critical_path".into(),
                jobs_affected,
                summary: format!("Identified critical path with {} jobs", jobs_affected),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::job_graph::make_test_job;

    #[test]
    fn single_job_critical_path() {
        let graph = JobGraph::build(vec![make_test_job("A", &[], &["a.txt"])]).unwrap();

        let path = compute_critical_path(&graph);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0], JobId::from("A"));
    }

    #[test]
    fn linear_chain_critical_path() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["b.txt"], &["c.txt"]),
        ])
        .unwrap();

        let path = compute_critical_path(&graph);
        assert_eq!(path.len(), 3);
        assert_eq!(path[0], JobId::from("A"));
        assert_eq!(path[1], JobId::from("B"));
        assert_eq!(path[2], JobId::from("C"));
    }

    #[test]
    fn diamond_dag_critical_path() {
        // A → B, A → C, B → D, C → D
        // Both paths A→B→D and A→C→D have length 3.
        // The algorithm picks one deterministically.
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["a.txt"], &["c.txt"]),
            make_test_job("D", &["b.txt", "c.txt"], &["d.txt"]),
        ])
        .unwrap();

        let path = compute_critical_path(&graph);
        assert_eq!(path.len(), 3); // A → {B or C} → D
        assert_eq!(path[0], JobId::from("A"));
        assert_eq!(path[2], JobId::from("D"));
    }

    #[test]
    fn fork_join_longer_branch_wins() {
        // A → B → C → D (length 4)
        // A → E → D (length 3)
        // Critical path should be A → B → C → D
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["b.txt"], &["c.txt"]),
            make_test_job("E", &["a.txt"], &["e.txt"]),
            make_test_job("D", &["c.txt", "e.txt"], &["d.txt"]),
        ])
        .unwrap();

        let path = compute_critical_path(&graph);
        assert_eq!(path.len(), 4);
        assert_eq!(path[0], JobId::from("A"));
        assert_eq!(path[1], JobId::from("B"));
        assert_eq!(path[2], JobId::from("C"));
        assert_eq!(path[3], JobId::from("D"));
    }

    #[test]
    fn empty_graph_critical_path() {
        let graph = JobGraph::build(vec![]).unwrap();
        let path = compute_critical_path(&graph);
        assert!(path.is_empty());
    }

    #[test]
    fn parallel_independent_chains() {
        // Chain 1: A → B (length 2)
        // Chain 2: C → D → E (length 3) — this is the critical path
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &[], &["c.txt"]),
            make_test_job("D", &["c.txt"], &["d.txt"]),
            make_test_job("E", &["d.txt"], &["e.txt"]),
        ])
        .unwrap();

        let path = compute_critical_path(&graph);
        assert_eq!(path.len(), 3);
        assert_eq!(path[0], JobId::from("C"));
        assert_eq!(path[1], JobId::from("D"));
        assert_eq!(path[2], JobId::from("E"));
    }

    #[test]
    fn compute_borrows_graph() {
        // Verify that `compute` works without taking ownership of the graph,
        // so the caller can reuse the graph for scheduling.
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["b.txt"], &["c.txt"]),
        ])
        .unwrap();

        let pass = CriticalPathPass::new();
        let jobs = pass.compute(&graph);
        assert_eq!(jobs.len(), 3);
        assert!(jobs.contains(&JobId::from("A")));
        assert!(jobs.contains(&JobId::from("B")));
        assert!(jobs.contains(&JobId::from("C")));

        // Graph is still usable after compute.
        assert_eq!(graph.job_count(), 3);

        // critical_path_jobs() returns the same set.
        assert_eq!(pass.critical_path_jobs(), jobs);
    }

    #[test]
    fn pass_name_is_critical_path() {
        let pass = CriticalPathPass::new();
        assert_eq!(pass.name(), "critical_path");
    }

    #[test]
    fn critical_path_pass_trait() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ])
        .unwrap();

        let pass = CriticalPathPass::new();
        assert!(pass.critical_path_jobs().is_empty());

        let (_, result) = pass.optimize(graph).unwrap();
        assert_eq!(result.pass_name, "critical_path");
        assert_eq!(result.jobs_affected, 2);

        let jobs = pass.critical_path_jobs();
        assert!(jobs.contains(&JobId::from("A")));
        assert!(jobs.contains(&JobId::from("B")));
    }
}
