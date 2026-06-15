//! JobGraph — the physical graph of concrete jobs and their file dependencies.
//!
//! The JobGraph is the **second** of OxyMake's three graph representations:
//!
//! 1. **RuleGraph** (`dag.rs`) — the logical graph. Rules and pattern-level
//!    dependencies *before* wildcard resolution. Compact and abstract.
//!
//! 2. **JobGraph** (this module) — the physical graph. Concrete jobs with
//!    resolved wildcards, connected by their actual file dependencies. One
//!    node per job instance, one node per concrete output path. This is
//!    the graph that optimization passes transform.
//!
//! 3. **ExecGraph** — the runtime graph. The JobGraph annotated with
//!    execution state (status, metrics, retry counts). Lives only during
//!    a run.
//!
//! # Graph structure
//!
//! Like the RuleGraph, the JobGraph is a **bipartite directed graph**: job
//! nodes and output nodes alternate, connected by `Produces` and `Consumes`
//! edges.
//!
//! ```text
//!   ┌──────────────────┐         ┌──────────────────────────┐         ┌──────────────────┐
//!   │ job: align-A     │──produces──▶│ output: results/A.bam │◀──consumes──│ job: sort-A      │
//!   └──────────────────┘         └──────────────────────────┘         └──────────────────┘
//!         ▲                                                                │
//!         │ consumes                                                  produces
//!         │                                                                ▼
//!   ┌──────────────────────────┐                              ┌────────────────────────────────┐
//!   │ output: data/A.fastq     │                              │ output: results/A.sorted.bam   │
//!   └──────────────────────────┘                              └────────────────────────────────┘
//! ```
//!
//! Edge directions:
//! - `Job ──Produces──▶ Output`: the job creates this output.
//! - `Output ──Consumes──▶ Job`: an output is consumed by this job (edge points
//!   from the output node to the consuming job, so topological sort follows
//!   data flow).
//!
//! # How this differs from RuleGraph
//!
//! The RuleGraph has one node per *rule* and one node per *pattern string*.
//! The JobGraph has one node per *concrete job instance* (e.g., `align-sample_A`,
//! `align-sample_B`) and one node per *concrete output path* (e.g.,
//! `results/sample_A.bam`). Wildcard expansion has already happened.
//!
//! # Optimization passes
//!
//! The JobGraph is designed to be transformed by a pipeline of optimization
//! passes (defined in `ox-plan`). Each pass receives ownership of the graph,
//! transforms it, and returns it. Common passes include:
//!
//! - **Cache pruning**: marks jobs as skipped when their outputs are up-to-date.
//! - **Critical-path analysis**: annotates the longest chain for priority scheduling.
//! - **Resource partitioning**: groups jobs by resource requirements.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use petgraph::Direction;
use petgraph::algo::{is_cyclic_directed, toposort};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

use crate::error::DagError;
use crate::model::*;

// ---------------------------------------------------------------------------
// JobGraph
// ---------------------------------------------------------------------------

/// The physical execution graph: concrete jobs connected by file dependencies.
///
/// Built from the resolver's output (`Vec<ConcreteJob>`), then optimized by
/// passes before execution. Each job node is connected to output nodes via
/// `Produces` and `Consumes` edges, forming a bipartite DAG.
///
/// # Example
///
/// ```
/// use ox_core::job_graph::JobGraph;
/// use ox_core::model::*;
/// use std::collections::BTreeMap;
/// use std::path::PathBuf;
///
/// // Two jobs: "download" produces data/A.fastq, "align" consumes it.
/// let jobs = vec![
///     ConcreteJob {
///         id: JobId::from("download-A"),
///         rule: RuleName::from("download"),
///         wildcards: BTreeMap::from([("sample".into(), "A".into())]),
///         tags: BTreeMap::new(),
///         inputs: vec![],
///         outputs: vec![ResolvedOutput {
///             reference: OutputRef::File(PathBuf::from("data/A.fastq")),
///             name: None,
///             format: None,
///             lifecycle: OutputLifecycle::default(),
///             materialize: MaterializePolicy::default(),
///         }],
///         execution: ExecutionBlock::Shell { command: "wget ...".into() },
///         resources: BTreeMap::new(),
///         environment: None,
///         error_strategy: ErrorStrategy::default(),
///         timeout: None,
///         executor: None,
///         priority: None,
///         benchmark: None,
///         params: BTreeMap::new(),
///         param_files: Vec::new(),
///         log: LogConfig::default(),
///         shell_executable: None,
///         reproducibility: ReproducibilityClass::default(),
///     },
///     ConcreteJob {
///         id: JobId::from("align-A"),
///         rule: RuleName::from("align"),
///         wildcards: BTreeMap::from([("sample".into(), "A".into())]),
///         tags: BTreeMap::new(),
///         inputs: vec![ResolvedInput {
///             reference: OutputRef::File(PathBuf::from("data/A.fastq")),
///             name: None,
///             format: None,
///         }],
///         outputs: vec![ResolvedOutput {
///             reference: OutputRef::File(PathBuf::from("results/A.bam")),
///             name: None,
///             format: None,
///             lifecycle: OutputLifecycle::default(),
///             materialize: MaterializePolicy::default(),
///         }],
///         execution: ExecutionBlock::Shell { command: "bwa mem ...".into() },
///         resources: BTreeMap::new(),
///         environment: None,
///         error_strategy: ErrorStrategy::default(),
///         timeout: None,
///         executor: None,
///         priority: None,
///         benchmark: None,
///         params: BTreeMap::new(),
///         param_files: Vec::new(),
///         log: LogConfig::default(),
///         shell_executable: None,
///         reproducibility: ReproducibilityClass::default(),
///     },
/// ];
///
/// let graph = JobGraph::build(jobs).unwrap();
/// assert_eq!(graph.job_count(), 2);
/// assert!(graph.is_acyclic());
///
/// // "download-A" is upstream of "align-A"
/// let upstream = graph.upstream(&JobId::from("align-A"));
/// assert_eq!(upstream.len(), 1);
/// assert_eq!(upstream[0].as_str(), "download-A");
/// ```
#[derive(Debug)]
pub struct JobGraph {
    graph: DiGraph<JobNode, JobEdge>,
    /// Map from JobId to its node index in the petgraph.
    job_indices: BTreeMap<JobId, NodeIndex>,
    /// Map from output path string to its node index in the petgraph.
    /// Used by optimization passes that need to look up output nodes by path.
    #[allow(dead_code)]
    output_indices: BTreeMap<String, NodeIndex>,
    /// Jobs marked as skipped (e.g. by cache pruning). Skipped jobs keep
    /// their node and edges so traversal can bridge through them, but they
    /// are excluded from enumeration (`job_ids`, `job_count`,
    /// `topological_order`, `ready_jobs`, `jobs_with_tag`, `get_job`).
    skipped: BTreeSet<JobId>,
}

/// Convert an [`OutputRef`] to a string key for the output index.
///
/// File outputs use their path string; virtual outputs use their ID;
/// in-memory outputs use a synthetic key. This is also the key used by
/// [`MaterializationSet`] tracking in the scheduler.
pub fn output_ref_key(r: &OutputRef) -> String {
    match r {
        OutputRef::File(p) => p.to_string_lossy().to_string(),
        OutputRef::Virtual { id, .. } => format!("virtual:{id}"),
        OutputRef::InMemory { type_hint } => {
            let hint = type_hint.as_deref().unwrap_or("any");
            format!("memory:{hint}")
        }
    }
}

impl JobGraph {
    /// Build a JobGraph from resolved concrete jobs.
    ///
    /// The build process:
    /// 1. Creates a job node for each `ConcreteJob`.
    /// 2. Creates output nodes for each unique output reference.
    /// 3. Connects jobs to their outputs with `Produces` edges.
    /// 4. Connects output nodes to consuming jobs with `Consumes` edges.
    ///
    /// # Errors
    ///
    /// Returns [`DagError::ConflictingOutputs`] if two jobs produce the same
    /// output path, and [`DagError::DuplicateJobId`] if two jobs carry the
    /// same id (a duplicate would silently shadow the first job in the
    /// index, leaving it unreachable and never executed).
    ///
    /// ```
    /// use ox_core::job_graph::JobGraph;
    /// use ox_core::model::*;
    /// use std::collections::BTreeMap;
    ///
    /// let graph = JobGraph::build(vec![]).unwrap();
    /// assert_eq!(graph.job_count(), 0);
    /// ```
    pub fn build(jobs: Vec<ConcreteJob>) -> Result<Self, DagError> {
        let mut graph = DiGraph::new();
        let mut job_indices = BTreeMap::new();
        let mut output_indices: BTreeMap<String, NodeIndex> = BTreeMap::new();
        // Track which job produces each output, for conflict detection.
        let mut output_owners: BTreeMap<String, String> = BTreeMap::new();

        // Phase 1: Add all job nodes.
        for job in &jobs {
            if let Some(&existing_idx) = job_indices.get(&job.id) {
                let existing_rule = match &graph[existing_idx] {
                    JobNode::Job(j) => j.rule.to_string(),
                    _ => String::from("<unknown>"),
                };
                return Err(DagError::DuplicateJobId {
                    id: job.id.to_string(),
                    a: existing_rule,
                    b: job.rule.to_string(),
                });
            }
            let idx = graph.add_node(JobNode::Job(Box::new(job.clone())));
            job_indices.insert(job.id.clone(), idx);
        }

        // Phase 2: Add output nodes and Produces edges.
        for job in &jobs {
            let job_idx = job_indices[&job.id];

            for output in &job.outputs {
                let key = output_ref_key(&output.reference);

                // Conflict detection: two jobs producing the same output.
                if let Some(existing) = output_owners.get(&key) {
                    if existing.as_str() != job.id.as_str() {
                        return Err(DagError::ConflictingOutputs {
                            a: existing.clone(),
                            b: job.id.to_string(),
                            output: key,
                        });
                    }
                } else {
                    output_owners.insert(key.clone(), job.id.to_string());
                }

                let out_idx = *output_indices
                    .entry(key)
                    .or_insert_with(|| graph.add_node(JobNode::Output(output.reference.clone())));
                graph.add_edge(job_idx, out_idx, JobEdge::Produces);
            }
        }

        // Phase 3: Add Consumes edges (Output -> Job).
        for job in &jobs {
            let job_idx = job_indices[&job.id];

            for input in &job.inputs {
                let key = output_ref_key(&input.reference);

                // If the output node exists, connect it. If not, create a
                // "source" output node (external file not produced by any job).
                let out_idx = *output_indices
                    .entry(key)
                    .or_insert_with(|| graph.add_node(JobNode::Output(input.reference.clone())));
                graph.add_edge(out_idx, job_idx, JobEdge::Consumes);
            }
        }

        Ok(Self {
            graph,
            job_indices,
            output_indices,
            skipped: BTreeSet::new(),
        })
    }

    /// Get all job IDs in the graph, in sorted order.
    ///
    /// ```
    /// use ox_core::job_graph::{JobGraph, make_test_job};
    ///
    /// let jobs = vec![make_test_job("b-job", &[], &["b.txt"]),
    ///                 make_test_job("a-job", &[], &["a.txt"])];
    /// let graph = JobGraph::build(jobs).unwrap();
    /// let ids: Vec<&str> = graph.job_ids().iter().map(|id| id.as_str()).collect();
    /// assert_eq!(ids, vec!["a-job", "b-job"]);
    /// ```
    pub fn job_ids(&self) -> Vec<&JobId> {
        self.job_indices
            .keys()
            .filter(|id| !self.skipped.contains(*id))
            .collect()
    }

    /// Get a job by ID.
    ///
    /// Returns `None` if no job with the given ID exists or if the job has
    /// been marked as skipped.
    pub fn get_job(&self, id: &JobId) -> Option<&ConcreteJob> {
        if self.skipped.contains(id) {
            return None;
        }
        let idx = self.job_indices.get(id)?;
        // Invariant: job_indices only contains indices of Job nodes. Should
        // an optimization pass break it through inner_mut(), degrade to
        // "not found" rather than panicking.
        match &self.graph[*idx] {
            JobNode::Job(job) => Some(job),
            _ => None,
        }
    }

    /// Get jobs that are ready to execute (no unfinished upstream jobs).
    ///
    /// A job is "ready" if it has no incoming path through any output node
    /// that leads back to another job node. In other words, all of its input
    /// output-nodes are either source files (no producing job) or produced
    /// by jobs that have already been marked as skipped.
    ///
    /// For simplicity in this initial implementation, "ready" means the job
    /// has no upstream job dependencies at all (root jobs in the DAG).
    pub fn ready_jobs(&self) -> Vec<&JobId> {
        self.job_indices
            .iter()
            .filter(|(id, idx)| {
                // A job is ready if it is not skipped and has no live
                // (non-skipped) upstream jobs — bridged through skipped ones.
                !self.skipped.contains(*id)
                    && self
                        .bridged_job_neighbors(**idx, Direction::Incoming)
                        .is_empty()
            })
            .map(|(id, _)| id)
            .collect()
    }

    /// Find the nearest *non-skipped* job neighbors of a job node in the
    /// given direction, bridging through skipped jobs.
    ///
    /// One hop follows the bipartite structure:
    /// `job → output → job` (Produces then Consumes for `Outgoing`,
    /// Consumes then Produces for `Incoming`). When the neighbor job is
    /// marked skipped, the walk continues through it so that a skipped
    /// job never cuts the transitive closure.
    fn bridged_job_neighbors(&self, start: NodeIndex, dir: Direction) -> Vec<NodeIndex> {
        let (first_w, second_w) = match dir {
            Direction::Incoming => (JobEdge::Consumes, JobEdge::Produces),
            Direction::Outgoing => (JobEdge::Produces, JobEdge::Consumes),
        };
        let mut result = Vec::new();
        let mut visited: HashSet<NodeIndex> = HashSet::from([start]);
        let mut queue = vec![start];
        while let Some(idx) = queue.pop() {
            for edge in self
                .graph
                .edges_directed(idx, dir)
                .filter(|e| *e.weight() == first_w)
            {
                let output_node = match dir {
                    Direction::Incoming => edge.source(),
                    Direction::Outgoing => edge.target(),
                };
                for e2 in self
                    .graph
                    .edges_directed(output_node, dir)
                    .filter(|e| *e.weight() == second_w)
                {
                    let job_node = match dir {
                        Direction::Incoming => e2.source(),
                        Direction::Outgoing => e2.target(),
                    };
                    if !visited.insert(job_node) {
                        continue;
                    }
                    if let JobNode::Job(j) = &self.graph[job_node] {
                        if self.skipped.contains(&j.id) {
                            queue.push(job_node);
                        } else {
                            result.push(job_node);
                        }
                    }
                }
            }
        }
        result
    }

    /// Topological order of jobs.
    ///
    /// Returns job IDs in an order where every job appears after all of its
    /// dependencies. Only job nodes are included (output nodes are filtered out).
    ///
    /// # Errors
    ///
    /// Returns [`DagError::CycleDetected`] if the graph contains a cycle.
    ///
    /// ```
    /// use ox_core::job_graph::JobGraph;
    /// use ox_core::model::*;
    /// use std::collections::BTreeMap;
    /// use std::path::PathBuf;
    ///
    /// let jobs = vec![
    ///     ConcreteJob {
    ///         id: JobId::from("A"),
    ///         rule: RuleName::from("rA"),
    ///         wildcards: BTreeMap::new(),
    ///         tags: BTreeMap::new(),
    ///         inputs: vec![],
    ///         outputs: vec![ResolvedOutput {
    ///             reference: OutputRef::File(PathBuf::from("a.txt")),
    ///             name: None,
    ///             format: None,
    ///             lifecycle: OutputLifecycle::default(),
    ///             materialize: MaterializePolicy::default(),
    ///         }],
    ///         execution: ExecutionBlock::Shell { command: "true".into() },
    ///         resources: BTreeMap::new(),
    ///         environment: None,
    ///         error_strategy: ErrorStrategy::default(),
    ///         timeout: None,
    ///         executor: None,
    ///         priority: None,
    ///         benchmark: None,
    ///         params: BTreeMap::new(),
    ///         param_files: Vec::new(),
    ///         log: LogConfig::default(),
    ///         shell_executable: None,
    ///         reproducibility: ReproducibilityClass::default(),
    ///     },
    ///     ConcreteJob {
    ///         id: JobId::from("B"),
    ///         rule: RuleName::from("rB"),
    ///         wildcards: BTreeMap::new(),
    ///         tags: BTreeMap::new(),
    ///         inputs: vec![ResolvedInput {
    ///             reference: OutputRef::File(PathBuf::from("a.txt")),
    ///             name: None,
    ///             format: None,
    ///         }],
    ///         outputs: vec![ResolvedOutput {
    ///             reference: OutputRef::File(PathBuf::from("b.txt")),
    ///             name: None,
    ///             format: None,
    ///             lifecycle: OutputLifecycle::default(),
    ///             materialize: MaterializePolicy::default(),
    ///         }],
    ///         execution: ExecutionBlock::Shell { command: "true".into() },
    ///         resources: BTreeMap::new(),
    ///         environment: None,
    ///         error_strategy: ErrorStrategy::default(),
    ///         timeout: None,
    ///         executor: None,
    ///         priority: None,
    ///         benchmark: None,
    ///         params: BTreeMap::new(),
    ///         param_files: Vec::new(),
    ///         log: LogConfig::default(),
    ///         shell_executable: None,
    ///         reproducibility: ReproducibilityClass::default(),
    ///     },
    /// ];
    ///
    /// let graph = JobGraph::build(jobs).unwrap();
    /// let order = graph.topological_order().unwrap();
    /// let names: Vec<&str> = order.iter().map(|id| id.as_str()).collect();
    /// assert_eq!(names, vec!["A", "B"]);
    /// ```
    pub fn topological_order(&self) -> Result<Vec<&JobId>, DagError> {
        let sorted = toposort(&self.graph, None).map_err(|_| DagError::CycleDetected {
            cycle: vec!["(cycle in job graph)".into()],
        })?;

        Ok(sorted
            .into_iter()
            .filter_map(|idx| match &self.graph[idx] {
                JobNode::Job(job) if !self.skipped.contains(&job.id) => Some(&job.id),
                _ => None,
            })
            .collect())
    }

    /// Get upstream dependencies of a job (jobs whose outputs are this job's inputs).
    ///
    /// Traversal: for each input of the target job, find the output node,
    /// then find the job that produces it. Skipped jobs are bridged
    /// through: the nearest non-skipped producers are returned, so a job
    /// skipped by cache pruning never cuts the transitive closure.
    pub fn upstream(&self, id: &JobId) -> Vec<&JobId> {
        self.bridged_neighbors_ids(id, Direction::Incoming)
    }

    /// Get downstream dependents of a job (jobs that consume this job's outputs).
    ///
    /// Traversal: for each output of the target job, find the output node,
    /// then find jobs that consume it. Skipped jobs are bridged through
    /// (see [`JobGraph::upstream`]).
    pub fn downstream(&self, id: &JobId) -> Vec<&JobId> {
        self.bridged_neighbors_ids(id, Direction::Outgoing)
    }

    /// Shared implementation of `upstream` / `downstream`: bridged job
    /// neighbors as sorted, deduplicated job IDs (self excluded).
    fn bridged_neighbors_ids(&self, id: &JobId, dir: Direction) -> Vec<&JobId> {
        let job_idx = match self.job_indices.get(id) {
            Some(idx) => *idx,
            None => return vec![],
        };

        let mut ids: Vec<&JobId> = self
            .bridged_job_neighbors(job_idx, dir)
            .into_iter()
            .filter_map(|idx| match &self.graph[idx] {
                JobNode::Job(j) if j.id != *id => Some(&j.id),
                _ => None,
            })
            .collect();

        ids.sort();
        ids.dedup();
        ids
    }

    /// Total number of (non-skipped) jobs in the graph.
    pub fn job_count(&self) -> usize {
        self.job_indices.len() - self.skipped.len()
    }

    /// Total number of edges (both Produces and Consumes) in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Return all job-to-job dependency edges as `(upstream_id, downstream_id)` pairs.
    ///
    /// This traverses the bipartite graph to find direct job-to-job dependencies
    /// (job A produces an output that job B consumes). Used for persisting edges
    /// in `state.db` for DAG visualization.
    pub fn job_edges(&self) -> Vec<(&JobId, &JobId)> {
        let mut edges = Vec::new();
        for job_id in self.job_ids() {
            // Nearest non-skipped consumers, bridged through skipped jobs.
            for downstream_id in self.downstream(job_id) {
                edges.push((job_id, downstream_id));
            }
        }
        edges.sort();
        edges.dedup();
        edges
    }

    /// Check if the graph is acyclic.
    ///
    /// A cycle in the JobGraph means a circular dependency between jobs,
    /// which would make execution impossible.
    pub fn is_acyclic(&self) -> bool {
        !is_cyclic_directed(&self.graph)
    }

    /// Mark a job as skipped.
    ///
    /// This is used by the cache pruning pass to indicate that a job's
    /// outputs are already up-to-date and the job need not execute.
    ///
    /// The job's node and edges stay in the graph, so `upstream`,
    /// `downstream` and `job_edges` bridge *through* skipped jobs and the
    /// transitive closure is preserved. The skipped job itself disappears
    /// from enumeration: `get_job` returns `None`, and `job_ids`,
    /// `job_count`, `topological_order`, `ready_jobs` and `jobs_with_tag`
    /// no longer report it.
    pub fn mark_skipped(&mut self, id: &JobId) {
        if self.job_indices.contains_key(id) {
            self.skipped.insert(id.clone());
        }
    }

    /// Get all jobs with a specific tag key-value pair.
    ///
    /// Scans all job nodes and returns those whose `tags` map contains
    /// the given key with the given value.
    ///
    /// ```
    /// use ox_core::job_graph::JobGraph;
    /// use ox_core::model::*;
    /// use std::collections::BTreeMap;
    /// use std::path::PathBuf;
    ///
    /// let mut tags = BTreeMap::new();
    /// tags.insert("stage".into(), "align".into());
    ///
    /// let jobs = vec![ConcreteJob {
    ///     id: JobId::from("align-A"),
    ///     rule: RuleName::from("align"),
    ///     wildcards: BTreeMap::new(),
    ///     tags,
    ///     inputs: vec![],
    ///     outputs: vec![ResolvedOutput {
    ///         reference: OutputRef::File(PathBuf::from("a.bam")),
    ///         name: None,
    ///         format: None,
    ///         lifecycle: OutputLifecycle::default(),
    ///         materialize: MaterializePolicy::default(),
    ///     }],
    ///     execution: ExecutionBlock::Shell { command: "true".into() },
    ///     resources: BTreeMap::new(),
    ///     environment: None,
    ///     error_strategy: ErrorStrategy::default(),
    ///     timeout: None,
    ///     executor: None,
    ///     priority: None,
    ///     benchmark: None,
    ///     params: BTreeMap::new(),
    ///     param_files: Vec::new(),
    ///     log: LogConfig::default(),
    ///     shell_executable: None,
    ///     reproducibility: ReproducibilityClass::default(),
    /// }];
    ///
    /// let graph = JobGraph::build(jobs).unwrap();
    /// let tagged = graph.jobs_with_tag("stage", "align");
    /// assert_eq!(tagged.len(), 1);
    /// assert_eq!(tagged[0].as_str(), "align-A");
    ///
    /// let empty = graph.jobs_with_tag("stage", "sort");
    /// assert!(empty.is_empty());
    /// ```
    pub fn jobs_with_tag(&self, key: &str, value: &str) -> Vec<&JobId> {
        self.job_indices
            .iter()
            .filter(|(id, _)| !self.skipped.contains(*id))
            .filter_map(|(id, idx)| {
                // Invariant: job_indices only contains indices of Job nodes.
                // Degrade gracefully (skip) if an optimization pass broke it
                // through inner_mut() instead of panicking.
                match &self.graph[*idx] {
                    JobNode::Job(job) => Some(job),
                    _ => None,
                }?
                .tags
                .get(key)
                .filter(|v| v.as_str() == value)
                .map(|_| id)
            })
            .collect()
    }

    /// Add a gate node that blocks a job.
    ///
    /// The gate node is added to the graph with a `Blocks` edge pointing
    /// from the gate to the blocked job. The scheduler checks blocking
    /// gates before dispatching a job — if any gate is still `pending`
    /// in the state database, the job waits.
    ///
    /// Returns the gate's node index.
    pub fn add_gate(&mut self, gate_id: &GateId, blocked_job: &JobId) -> Option<NodeIndex> {
        let job_idx = self.job_indices.get(blocked_job)?;
        let gate_idx = self.graph.add_node(JobNode::Gate(gate_id.clone()));
        self.graph.add_edge(gate_idx, *job_idx, JobEdge::Blocks);
        Some(gate_idx)
    }

    /// Return the gate IDs that block a given job.
    ///
    /// Traversal: find incoming `Blocks` edges to the job node, and
    /// return the `GateId` of each source gate node.
    pub fn blocking_gates(&self, id: &JobId) -> Vec<&GateId> {
        let job_idx = match self.job_indices.get(id) {
            Some(idx) => *idx,
            None => return vec![],
        };

        self.graph
            .edges_directed(job_idx, Direction::Incoming)
            .filter(|e| *e.weight() == JobEdge::Blocks)
            .filter_map(|edge| match &self.graph[edge.source()] {
                JobNode::Gate(gid) => Some(gid),
                _ => None,
            })
            .collect()
    }

    /// Initialize a [`MaterializationSet`] for each output in the graph.
    ///
    /// Each output's `pending_consumers` count is set to the number of
    /// downstream jobs that consume it (the number of outgoing `Consumes`
    /// edges from the output node). This is the initial token demand in
    /// Petri net terms — how many transitions still need to fire from this
    /// place before it can be safely evicted.
    pub fn init_output_materializations(&self) -> HashMap<String, MaterializationSet> {
        let mut result = HashMap::new();

        for (key, &idx) in &self.output_indices {
            let output_ref = match &self.graph[idx] {
                JobNode::Output(r) => r.clone(),
                _ => continue,
            };
            // Count outgoing Consumes edges from this output node — each
            // leads to a job that needs to read this output.
            let consumer_count = self
                .graph
                .edges_directed(idx, Direction::Outgoing)
                .filter(|e| matches!(e.weight(), JobEdge::Consumes))
                .count();
            result.insert(
                key.clone(),
                MaterializationSet::new(output_ref, consumer_count),
            );
        }

        result
    }

    /// Access the underlying petgraph (for optimization passes and analysis).
    pub fn inner(&self) -> &DiGraph<JobNode, JobEdge> {
        &self.graph
    }

    /// Mutable access to the underlying petgraph (for optimization passes).
    pub fn inner_mut(&mut self) -> &mut DiGraph<JobNode, JobEdge> {
        &mut self.graph
    }
}

// ---------------------------------------------------------------------------
// Test helper (public for doc tests and ox-plan)
// ---------------------------------------------------------------------------

/// Create a minimal [`ConcreteJob`] for testing purposes.
///
/// This is intentionally simple: it creates file-based inputs and outputs
/// with default settings, and a no-op shell command.
pub fn make_test_job(name: &str, inputs: &[&str], outputs: &[&str]) -> ConcreteJob {
    ConcreteJob {
        id: JobId::from(name),
        rule: RuleName::from(name),
        wildcards: BTreeMap::new(),
        tags: BTreeMap::new(),
        inputs: inputs
            .iter()
            .map(|p| ResolvedInput {
                reference: OutputRef::File(PathBuf::from(p)),
                name: None,
                format: None,
            })
            .collect(),
        outputs: outputs
            .iter()
            .map(|p| ResolvedOutput {
                reference: OutputRef::File(PathBuf::from(p)),
                name: None,
                format: None,
                lifecycle: OutputLifecycle::default(),
                materialize: MaterializePolicy::default(),
            })
            .collect(),
        execution: ExecutionBlock::Shell {
            command: "true".into(),
        },
        resources: BTreeMap::new(),
        environment: None,
        error_strategy: ErrorStrategy::default(),
        timeout: None,
        executor: None,
        priority: None,
        benchmark: None,
        params: BTreeMap::new(),
        param_files: Vec::new(),
        log: LogConfig::default(),
        shell_executable: None,
        reproducibility: ReproducibilityClass::default(),
    }
}

/// Create a minimal [`ConcreteJob`] with tags for testing purposes.
pub fn make_test_job_with_tags(
    name: &str,
    inputs: &[&str],
    outputs: &[&str],
    tags: BTreeMap<String, String>,
) -> ConcreteJob {
    let mut job = make_test_job(name, inputs, outputs);
    job.tags = tags;
    job
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub mod tests {
    use super::*;

    // Re-export the helper so doc tests can use it.
    pub use super::make_test_job;

    // ── Empty graph ─────────────────────────────────────────────────────

    #[test]
    fn empty_graph() {
        let graph = JobGraph::build(vec![]).unwrap();
        assert_eq!(graph.job_count(), 0);
        assert_eq!(graph.edge_count(), 0);
        assert!(graph.is_acyclic());
        assert!(graph.job_ids().is_empty());
        assert!(graph.topological_order().unwrap().is_empty());
        assert!(graph.ready_jobs().is_empty());
    }

    // ── Single job with no dependencies ─────────────────────────────────

    #[test]
    fn single_job_no_deps() {
        let jobs = vec![make_test_job("build", &[], &["out.txt"])];
        let graph = JobGraph::build(jobs).unwrap();

        assert_eq!(graph.job_count(), 1);
        assert!(graph.is_acyclic());
        assert!(graph.upstream(&JobId::from("build")).is_empty());
        assert!(graph.downstream(&JobId::from("build")).is_empty());

        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].as_str(), "build");

        // A job with no upstream is ready.
        let ready = graph.ready_jobs();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].as_str(), "build");
    }

    // ── Linear chain: A → B → C ────────────────────────────────────────

    #[test]
    fn linear_chain() {
        let jobs = vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["b.txt"], &["c.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        assert_eq!(graph.job_count(), 3);
        assert!(graph.is_acyclic());

        // Upstream / downstream checks.
        assert!(graph.upstream(&JobId::from("A")).is_empty());
        assert_eq!(graph.upstream(&JobId::from("B")), vec![&JobId::from("A")]);
        assert_eq!(graph.upstream(&JobId::from("C")), vec![&JobId::from("B")]);

        assert_eq!(graph.downstream(&JobId::from("A")), vec![&JobId::from("B")]);
        assert_eq!(graph.downstream(&JobId::from("B")), vec![&JobId::from("C")]);
        assert!(graph.downstream(&JobId::from("C")).is_empty());

        // Topological order: A must come before B, B before C.
        let order = graph.topological_order().unwrap();
        let names: Vec<&str> = order.iter().map(|id| id.as_str()).collect();
        let pos_a = names.iter().position(|n| *n == "A").unwrap();
        let pos_b = names.iter().position(|n| *n == "B").unwrap();
        let pos_c = names.iter().position(|n| *n == "C").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);

        // Only A is ready (no upstream).
        let ready = graph.ready_jobs();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].as_str(), "A");
    }

    // ── Diamond dependency: A → B, A → C, B → D, C → D ────────────────

    #[test]
    fn diamond_dependency() {
        let jobs = vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["a.txt"], &["c.txt"]),
            make_test_job("D", &["b.txt", "c.txt"], &["d.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        assert_eq!(graph.job_count(), 4);
        assert!(graph.is_acyclic());

        // D depends on both B and C.
        let mut upstream_d: Vec<String> = graph
            .upstream(&JobId::from("D"))
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        upstream_d.sort();
        assert_eq!(upstream_d, vec!["B", "C"]);

        // A feeds both B and C.
        let mut downstream_a: Vec<String> = graph
            .downstream(&JobId::from("A"))
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        downstream_a.sort();
        assert_eq!(downstream_a, vec!["B", "C"]);

        // Topological order: A before B and C, both before D.
        let order = graph.topological_order().unwrap();
        let names: Vec<&str> = order.iter().map(|id| id.as_str()).collect();
        let pos_a = names.iter().position(|n| *n == "A").unwrap();
        let pos_b = names.iter().position(|n| *n == "B").unwrap();
        let pos_c = names.iter().position(|n| *n == "C").unwrap();
        let pos_d = names.iter().position(|n| *n == "D").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
        assert!(pos_b < pos_d);
        assert!(pos_c < pos_d);

        // Only A is ready.
        let ready = graph.ready_jobs();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].as_str(), "A");
    }

    // ── Ready jobs: disconnected graph ──────────────────────────────────

    #[test]
    fn ready_jobs_disconnected() {
        let jobs = vec![
            make_test_job("X", &[], &["x.txt"]),
            make_test_job("Y", &[], &["y.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        // Both X and Y are ready (no dependencies).
        let mut ready: Vec<&str> = graph.ready_jobs().iter().map(|id| id.as_str()).collect();
        ready.sort();
        assert_eq!(ready, vec!["X", "Y"]);
    }

    // ── Duplicate job ids (B5) ──────────────────────────────────────────

    #[test]
    fn duplicate_job_id_is_rejected() {
        // B5: two jobs with the same id silently overwrote each other in the
        // job index — one job became unreachable, was never executed, and the
        // run still reported success. Construction must fail loudly instead.
        let jobs = vec![
            make_test_job("same-id", &[], &["a.txt"]),
            make_test_job("same-id", &[], &["b.txt"]),
        ];
        let err = JobGraph::build(jobs).unwrap_err();
        assert!(
            matches!(err, DagError::DuplicateJobId { .. }),
            "expected DuplicateJobId, got: {err:?}"
        );
        assert!(err.to_string().contains("same-id"), "got: {err}");
    }

    // ── Conflicting outputs ─────────────────────────────────────────────

    #[test]
    fn conflicting_outputs() {
        let jobs = vec![
            make_test_job("job1", &[], &["same.txt"]),
            make_test_job("job2", &[], &["same.txt"]),
        ];
        let err = JobGraph::build(jobs).unwrap_err();
        let DagError::ConflictingOutputs { a, b, output } = err else {
            panic!("expected ConflictingOutputs, got: {err:?}")
        };
        assert_eq!(output, "same.txt");
        let mut names = vec![a, b];
        names.sort();
        assert_eq!(names, vec!["job1", "job2"]);
    }

    // ── mark_skipped ────────────────────────────────────────────────────

    #[test]
    fn mark_skipped_removes_from_job_index() {
        let jobs = vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ];
        let mut graph = JobGraph::build(jobs).unwrap();

        assert_eq!(graph.job_count(), 2);
        assert!(graph.get_job(&JobId::from("A")).is_some());

        graph.mark_skipped(&JobId::from("A"));

        assert_eq!(graph.job_count(), 1);
        assert!(graph.get_job(&JobId::from("A")).is_none());
        assert!(graph.get_job(&JobId::from("B")).is_some());
    }

    // ── mark_skipped preserves transitive closure (H9) ─────────────────

    #[test]
    fn skipped_job_does_not_cut_transitive_closure() {
        // H9: A → B → C with B skipped. The tombstone implementation cut
        // the upstream/downstream traversal at B, contradicting the
        // documented contract ("downstream jobs can still traverse through").
        let jobs = vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["b.txt"], &["c.txt"]),
        ];
        let mut graph = JobGraph::build(jobs).unwrap();
        graph.mark_skipped(&JobId::from("B"));

        assert_eq!(graph.downstream(&JobId::from("A")), vec![&JobId::from("C")]);
        assert_eq!(graph.upstream(&JobId::from("C")), vec![&JobId::from("A")]);
        // job_edges bridges through the skipped job as well.
        assert_eq!(
            graph.job_edges(),
            vec![(&JobId::from("A"), &JobId::from("C"))]
        );
    }

    #[test]
    fn skipped_middle_job_does_not_make_downstream_ready() {
        // H9 corollary: with B skipped but A still live, C must NOT be
        // ready — its transitive upstream A has not produced a.txt yet.
        // The tombstone version wrongly reported C as ready.
        let jobs = vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["b.txt"], &["c.txt"]),
        ];
        let mut graph = JobGraph::build(jobs).unwrap();
        graph.mark_skipped(&JobId::from("B"));

        let ready: Vec<&str> = graph.ready_jobs().iter().map(|id| id.as_str()).collect();
        assert_eq!(ready, vec!["A"]);
    }

    #[test]
    fn skipped_root_unblocks_downstream() {
        // Cache-pruning contract: skipping a root job makes its consumer ready.
        let jobs = vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ];
        let mut graph = JobGraph::build(jobs).unwrap();
        graph.mark_skipped(&JobId::from("A"));

        let ready: Vec<&str> = graph.ready_jobs().iter().map(|id| id.as_str()).collect();
        assert_eq!(ready, vec!["B"]);
        // The skipped job is excluded from enumeration and topological order.
        assert_eq!(graph.job_count(), 1);
        assert!(graph.topological_order().unwrap().len() == 1);
    }

    // ── Corrupted node degrades gracefully (H10) ────────────────────────

    #[test]
    fn corrupted_job_node_does_not_panic() {
        // H10: get_job / jobs_with_tag hit `unreachable!()` when a job index
        // pointed at a non-Job node (a state an optimization pass can create
        // through inner_mut()). They must degrade to "not found" instead.
        let jobs = vec![make_test_job_with_tags(
            "A",
            &[],
            &["a.txt"],
            BTreeMap::from([("stage".into(), "build".into())]),
        )];
        let mut graph = JobGraph::build(jobs).unwrap();

        let idx = graph
            .inner()
            .node_indices()
            .find(|i| matches!(&graph.inner()[*i], JobNode::Job(_)))
            .unwrap();
        graph.inner_mut()[idx] = JobNode::Output(OutputRef::File(PathBuf::from("<corrupt>")));

        assert!(graph.get_job(&JobId::from("A")).is_none());
        assert!(graph.jobs_with_tag("stage", "build").is_empty());
    }

    // ── jobs_with_tag ───────────────────────────────────────────────────

    #[test]
    fn jobs_with_tag_filtering() {
        let jobs = vec![
            make_test_job_with_tags(
                "align-A",
                &[],
                &["a.bam"],
                BTreeMap::from([("stage".into(), "align".into())]),
            ),
            make_test_job_with_tags(
                "sort-A",
                &["a.bam"],
                &["a.sorted.bam"],
                BTreeMap::from([("stage".into(), "sort".into())]),
            ),
            make_test_job_with_tags(
                "align-B",
                &[],
                &["b.bam"],
                BTreeMap::from([("stage".into(), "align".into())]),
            ),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        let align_jobs: Vec<&str> = graph
            .jobs_with_tag("stage", "align")
            .iter()
            .map(|id| id.as_str())
            .collect();
        assert_eq!(align_jobs.len(), 2);

        let sort_jobs: Vec<&str> = graph
            .jobs_with_tag("stage", "sort")
            .iter()
            .map(|id| id.as_str())
            .collect();
        assert_eq!(sort_jobs.len(), 1);
        assert_eq!(sort_jobs[0], "sort-A");

        let none = graph.jobs_with_tag("stage", "missing");
        assert!(none.is_empty());

        let none2 = graph.jobs_with_tag("nonexistent_key", "align");
        assert!(none2.is_empty());
    }

    // ── get_job lookup ──────────────────────────────────────────────────

    #[test]
    fn get_job_lookup() {
        let jobs = vec![
            make_test_job("alpha", &[], &["a.txt"]),
            make_test_job("beta", &["a.txt"], &["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        let alpha = graph.get_job(&JobId::from("alpha")).unwrap();
        assert_eq!(alpha.id.as_str(), "alpha");

        let beta = graph.get_job(&JobId::from("beta")).unwrap();
        assert_eq!(beta.id.as_str(), "beta");

        assert!(graph.get_job(&JobId::from("nonexistent")).is_none());
    }

    // ── upstream/downstream of nonexistent job ──────────────────────────

    #[test]
    fn upstream_downstream_nonexistent() {
        let graph = JobGraph::build(vec![make_test_job("A", &[], &["a.txt"])]).unwrap();
        assert!(graph.upstream(&JobId::from("nope")).is_empty());
        assert!(graph.downstream(&JobId::from("nope")).is_empty());
    }

    // ── edge_count ──────────────────────────────────────────────────────

    #[test]
    fn edge_count_linear() {
        // A --produces--> a.txt --consumes--> B --produces--> b.txt
        // That's 3 edges total.
        let jobs = vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        // A produces a.txt (1), a.txt consumed by B (1), B produces b.txt (1)
        assert_eq!(graph.edge_count(), 3);
    }

    // ── inner access ────────────────────────────────────────────────────

    #[test]
    fn inner_access() {
        let jobs = vec![make_test_job("A", &[], &["a.txt"])];
        let mut graph = JobGraph::build(jobs).unwrap();

        // Read-only access.
        assert_eq!(graph.inner().node_count(), 2); // 1 job + 1 output
        assert_eq!(graph.inner().edge_count(), 1); // 1 Produces edge

        // Mutable access.
        let _ = graph.inner_mut();
    }

    // ── Multiple inputs from different producers ────────────────────────

    #[test]
    fn multiple_inputs_different_producers() {
        let jobs = vec![
            make_test_job("gen_a", &[], &["a.txt"]),
            make_test_job("gen_b", &[], &["b.txt"]),
            make_test_job("merge", &["a.txt", "b.txt"], &["merged.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        let mut upstream: Vec<String> = graph
            .upstream(&JobId::from("merge"))
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        upstream.sort();
        assert_eq!(upstream, vec!["gen_a", "gen_b"]);
    }

    // ── Disconnected components topological order ───────────────────────

    #[test]
    fn disconnected_components() {
        let jobs = vec![
            make_test_job("X", &[], &["x.txt"]),
            make_test_job("Y", &[], &["y.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        assert!(graph.is_acyclic());

        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 2);
    }

    // ── Virtual and InMemory output ref keys ─────────────────────────

    #[test]
    fn output_ref_key_virtual() {
        let key = super::output_ref_key(&OutputRef::Virtual {
            id: "db_table".into(),
            check: "SELECT 1".into(),
        });
        assert_eq!(key, "virtual:db_table");
    }

    #[test]
    fn output_ref_key_in_memory() {
        let key = super::output_ref_key(&OutputRef::InMemory {
            type_hint: Some("DataFrame".into()),
        });
        assert_eq!(key, "memory:DataFrame");
    }

    #[test]
    fn output_ref_key_in_memory_no_hint() {
        let key = super::output_ref_key(&OutputRef::InMemory { type_hint: None });
        assert_eq!(key, "memory:any");
    }

    // ── Same job duplicate output is ok ──────────────────────────────

    #[test]
    fn same_job_duplicate_output_ok() {
        let mut job = make_test_job("A", &[], &["out.txt"]);
        job.outputs.push(ResolvedOutput {
            reference: OutputRef::File(PathBuf::from("out.txt")),
            name: None,
            format: None,
            lifecycle: OutputLifecycle::default(),
            materialize: MaterializePolicy::default(),
        });
        let graph = JobGraph::build(vec![job]).unwrap();
        assert_eq!(graph.job_count(), 1);
    }

    // ── upstream/downstream self-referencing excludes self ────────────

    #[test]
    fn upstream_self_referencing_excludes_self() {
        // A job that consumes its own output should not list itself as upstream.
        let mut job = make_test_job("A", &[], &["a.txt"]);
        job.inputs.push(ResolvedInput {
            reference: OutputRef::File(PathBuf::from("a.txt")),
            name: None,
            format: None,
        });
        let graph = JobGraph::build(vec![job]).unwrap();
        assert!(graph.upstream(&JobId::from("A")).is_empty());
    }

    #[test]
    fn downstream_self_referencing_excludes_self() {
        let mut job = make_test_job("A", &[], &["a.txt"]);
        job.inputs.push(ResolvedInput {
            reference: OutputRef::File(PathBuf::from("a.txt")),
            name: None,
            format: None,
        });
        let graph = JobGraph::build(vec![job]).unwrap();
        assert!(graph.downstream(&JobId::from("A")).is_empty());
    }

    // ── mark_skipped on nonexistent job is no-op ─────────────────────

    #[test]
    fn mark_skipped_nonexistent_noop() {
        let jobs = vec![make_test_job("A", &[], &["a.txt"])];
        let mut graph = JobGraph::build(jobs).unwrap();
        graph.mark_skipped(&JobId::from("nonexistent"));
        assert_eq!(graph.job_count(), 1);
    }

    // ── jobs_with_tag after mark_skipped ─────────────────────────────

    #[test]
    fn jobs_with_tag_after_skipped() {
        let jobs = vec![make_test_job_with_tags(
            "A",
            &[],
            &["a.txt"],
            BTreeMap::from([("stage".into(), "build".into())]),
        )];
        let mut graph = JobGraph::build(jobs).unwrap();
        assert_eq!(graph.jobs_with_tag("stage", "build").len(), 1);
        graph.mark_skipped(&JobId::from("A"));
        // After skipping, the tag query should no longer find it.
        assert!(graph.jobs_with_tag("stage", "build").is_empty());
    }

    // ── Cycle detection in topological_order ─────────────────────────

    #[test]
    fn topological_order_cycle_error() {
        // Mutual dependency creates a cycle: A -> a.txt -> B -> b.txt -> A
        let jobs = vec![
            make_test_job("A", &["b.txt"], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        assert!(!graph.is_acyclic());
        let err = graph.topological_order().unwrap_err();
        assert!(
            matches!(err, DagError::CycleDetected { .. }),
            "expected CycleDetected, got: {err:?}"
        );
    }

    // ── make_test_job_with_tags helper ───────────────────────────────

    #[test]
    fn test_make_test_job_with_tags() {
        let tags = BTreeMap::from([("env".into(), "prod".into())]);
        let job = make_test_job_with_tags("x", &["in.txt"], &["out.txt"], tags.clone());
        assert_eq!(job.tags, tags);
        assert_eq!(job.id.as_str(), "x");
    }

    // ── Property tests ─────────────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;
        use std::collections::HashSet;

        /// Strategy for a unique job name.
        fn job_name() -> impl Strategy<Value = String> {
            "[a-z]{1,6}"
        }

        /// Strategy for a linear chain of N jobs (guaranteed acyclic).
        /// Returns Vec<(name, inputs, outputs)>.
        fn linear_chain(max_jobs: usize) -> impl Strategy<Value = Vec<ConcreteJob>> {
            (1..=max_jobs).prop_flat_map(|n| {
                proptest::collection::vec(job_name(), n).prop_filter_map("unique names", |names| {
                    let mut seen = HashSet::new();
                    if !names.iter().all(|n| seen.insert(n.clone())) {
                        return None;
                    }
                    let mut jobs = Vec::new();
                    for (i, name) in names.iter().enumerate() {
                        let inputs: Vec<&str> = if i == 0 {
                            vec![]
                        } else {
                            vec![]
                            // Will be set below
                        };
                        let output = format!("{name}.txt");
                        let mut job = make_test_job(name, &inputs, &[&output]);
                        // Chain: job i consumes output of job i-1
                        if i > 0 {
                            let prev_output = format!("{}.txt", &names[i - 1]);
                            job.inputs = vec![ResolvedInput {
                                reference: OutputRef::File(PathBuf::from(&prev_output)),
                                name: None,
                                format: None,
                            }];
                        }
                        jobs.push(job);
                    }
                    Some(jobs)
                })
            })
        }

        /// Strategy for a forest of independent jobs (no dependencies).
        fn independent_jobs(max_jobs: usize) -> impl Strategy<Value = Vec<ConcreteJob>> {
            (1..=max_jobs).prop_flat_map(|n| {
                proptest::collection::vec(job_name(), n).prop_filter_map("unique names", |names| {
                    let mut seen = HashSet::new();
                    if !names.iter().all(|n| seen.insert(n.clone())) {
                        return None;
                    }
                    let jobs: Vec<ConcreteJob> = names
                        .iter()
                        .map(|name| {
                            let output = format!("{name}.txt");
                            make_test_job(name, &[], &[&output])
                        })
                        .collect();
                    Some(jobs)
                })
            })
        }

        // ── init_output_materializations ─────────────────────────────

        #[test]
        fn init_output_materializations_counts_consumers() {
            // A → mid.txt → B → out.txt
            let a = make_test_job("A", &[], &["mid.txt"]);
            let b = make_test_job("B", &["mid.txt"], &["out.txt"]);
            let graph = JobGraph::build(vec![a, b]).unwrap();

            let mats = graph.init_output_materializations();
            // mid.txt has 1 consumer (B)
            let mid = mats.get("mid.txt").expect("mid.txt should exist");
            assert_eq!(mid.pending_consumers(), 1);

            // out.txt has 0 consumers (it's a leaf)
            let out = mats.get("out.txt").expect("out.txt should exist");
            assert_eq!(out.pending_consumers(), 0);
        }

        #[test]
        fn init_output_materializations_multiple_consumers() {
            // A → mid.txt → B
            //             → C
            let a = make_test_job("A", &[], &["mid.txt"]);
            let b = make_test_job("B", &["mid.txt"], &["b_out.txt"]);
            let c = make_test_job("C", &["mid.txt"], &["c_out.txt"]);
            let graph = JobGraph::build(vec![a, b, c]).unwrap();

            let mats = graph.init_output_materializations();
            let mid = mats.get("mid.txt").expect("mid.txt should exist");
            assert_eq!(mid.pending_consumers(), 2);
        }

        proptest! {
            /// Any linear chain of jobs forms an acyclic graph.
            #[test]
            fn linear_chain_is_acyclic(jobs in linear_chain(8)) {
                let graph = JobGraph::build(jobs).unwrap();
                prop_assert!(graph.is_acyclic());
            }

            /// Topological order of a linear chain respects dependency order.
            #[test]
            fn linear_chain_topo_order(jobs in linear_chain(8)) {
                let job_names: Vec<String> = jobs.iter().map(|j| j.id.0.to_string()).collect();
                let graph = JobGraph::build(jobs).unwrap();
                let order = graph.topological_order().unwrap();
                let order_names: Vec<&str> = order.iter().map(|id| id.as_str()).collect();

                // Each job must appear after its predecessor in the chain
                for (i, name) in job_names.iter().enumerate() {
                    if i > 0 {
                        let pos_prev = order_names
                            .iter()
                            .position(|n| *n == job_names[i - 1])
                            .unwrap();
                        let pos_curr = order_names
                            .iter()
                            .position(|n| *n == name.as_str())
                            .unwrap();
                        prop_assert!(
                            pos_prev < pos_curr,
                            "{} should come before {} in topo order",
                            job_names[i - 1], name
                        );
                    }
                }
            }

            /// All independent jobs are ready (no upstream dependencies).
            #[test]
            fn independent_jobs_all_ready(jobs in independent_jobs(8)) {
                let n = jobs.len();
                let graph = JobGraph::build(jobs).unwrap();
                let ready = graph.ready_jobs();
                prop_assert_eq!(
                    ready.len(), n,
                    "all {} independent jobs should be ready, got {}",
                    n, ready.len()
                );
            }

            /// In a linear chain, only the first job is ready.
            #[test]
            fn linear_chain_one_ready(jobs in linear_chain(8)) {
                let first_id = jobs[0].id.clone();
                let graph = JobGraph::build(jobs).unwrap();
                let ready = graph.ready_jobs();
                prop_assert_eq!(ready.len(), 1);
                prop_assert_eq!(ready[0], &first_id);
            }

            /// Job count matches input count for valid (no-conflict) graphs.
            #[test]
            fn job_count_matches(jobs in independent_jobs(8)) {
                let n = jobs.len();
                let graph = JobGraph::build(jobs).unwrap();
                prop_assert_eq!(graph.job_count(), n);
            }

            /// Every job ID returned by job_ids() can be looked up with get_job().
            #[test]
            fn all_job_ids_retrievable(jobs in linear_chain(8)) {
                let graph = JobGraph::build(jobs).unwrap();
                for id in graph.job_ids() {
                    prop_assert!(
                        graph.get_job(id).is_some(),
                        "job_ids() returned {:?} but get_job() returned None",
                        id
                    );
                }
            }

            /// Upstream/downstream are inverses: if A is upstream of B, then B is downstream of A.
            #[test]
            fn upstream_downstream_inverse(jobs in linear_chain(8)) {
                let graph = JobGraph::build(jobs).unwrap();
                for id in graph.job_ids() {
                    for upstream_id in graph.upstream(id) {
                        let downstream_of_upstream = graph.downstream(upstream_id);
                        prop_assert!(
                            downstream_of_upstream.contains(&id),
                            "{:?} is upstream of {:?} but {:?} is not in downstream of {:?}",
                            upstream_id, id, id, upstream_id
                        );
                    }
                }
            }
        }
    }
}
