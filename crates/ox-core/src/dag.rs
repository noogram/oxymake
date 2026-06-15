//! RuleGraph — the logical graph of rules and pattern-level dependencies.
//!
//! The RuleGraph is the **first** of OxyMake's three graph representations:
//!
//! 1. **RuleGraph** (this module) — the logical graph. Represents rules and
//!    their pattern-level dependencies *before* wildcard resolution. One node
//!    per rule, one node per unique pattern string. Compact and abstract: a
//!    single `call` rule node represents *all* variant-call instances.
//!
//! 2. **JobGraph** — the physical graph. The RuleGraph *expanded* (wildcards
//!    resolved, guards evaluated) and *optimized* through a series of passes.
//!    One node per concrete job instance.
//!
//! 3. **ExecGraph** — the runtime graph. The JobGraph annotated with execution
//!    state (status, metrics, retry counts). Lives only during a run.
//!
//! # What analysis the RuleGraph enables
//!
//! - **Cycle detection**: Structural cycles in the rule dependency graph are
//!   caught early, before the expensive wildcard resolution phase.
//! - **Rule ambiguity detection**: Multiple rules producing the same output
//!   pattern are flagged as errors (unless resolved by `ruleorder`).
//! - **Structural validation**: Missing dependencies, orphan rules, etc.
//! - **Pipeline visualization**: `ox dag --group-by stage` renders the
//!   RuleGraph, showing the abstract pipeline structure.
//!
//! # Graph structure
//!
//! The RuleGraph is a **bipartite directed graph**: rule nodes and pattern
//! nodes alternate, connected by `Produces` and `Consumes` edges.
//!
//! ```text
//!   ┌────────────┐         ┌────────────────────┐         ┌────────────┐
//!   │ rule: align │──produces──▶│ pattern: {s}.bam │◀──consumes──│ rule: sort │
//!   └────────────┘         └────────────────────┘         └────────────┘
//!         ▲                                                      │
//!         │ consumes                                        produces
//!         │                                                      ▼
//!   ┌─────────────────────┐                          ┌──────────────────────┐
//!   │ pattern: {s}.fastq  │                          │ pattern: {s}.sorted  │
//!   └─────────────────────┘                          └──────────────────────┘
//! ```
//!
//! Edge directions:
//! - `Rule ──Produces──▶ Pattern`: the rule's output patterns.
//! - `Pattern ──Consumes──▶ Rule`: a rule consumes this pattern (edge points
//!   *from* the pattern node *to* the consuming rule, so that topological
//!   sort follows the data flow direction).
//!
//! Wait — we actually orient edges so that dependencies point upstream:
//! - `Rule ──Produces──▶ Pattern`
//! - `Rule ──Consumes──▶ Pattern`
//!
//! For dependency analysis we follow: rule consumes pattern, which is produced
//! by another rule. The `upstream` and `downstream` methods handle this
//! traversal for you.

use std::collections::BTreeMap;

use petgraph::Direction;
use petgraph::algo::{is_cyclic_directed, toposort};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

use crate::error::DagError;
use crate::model::{Rule, RuleName, TargetPattern};

// ---------------------------------------------------------------------------
// Node and edge types
// ---------------------------------------------------------------------------

/// A node in the RuleGraph.
///
/// The graph is bipartite: every edge connects a [`RuleNode::Rule`] to a
/// [`RuleNode::Pattern`] (or vice versa). Two rule nodes are never directly
/// connected; the pattern node acts as the intermediary.
///
/// ```
/// use ox_core::dag::RuleNode;
/// use ox_core::model::TargetPattern;
///
/// let rule_node = RuleNode::Pattern(TargetPattern::from("data/{sample}.csv"));
/// assert!(matches!(rule_node, RuleNode::Pattern(_)));
/// ```
#[derive(Debug, Clone)]
pub enum RuleNode {
    /// A rule declaration from the Oxymakefile.
    Rule(Box<Rule>),
    /// A file pattern (input or output) that connects rules.
    Pattern(TargetPattern),
}

impl RuleNode {
    /// Returns a reference to the inner [`Rule`] if this is a `Rule` node.
    fn as_rule(&self) -> Option<&Rule> {
        match self {
            Self::Rule(r) => Some(r),
            Self::Pattern(_) => None,
        }
    }
}

/// An edge in the RuleGraph.
///
/// Edges connect rule nodes to pattern nodes:
/// - `Produces`: directed from a rule node to a pattern node.
/// - `Consumes`: directed from a pattern node to a rule node.
///
/// This orientation means a topological sort of the graph yields rules
/// in dependency order (producers before consumers).
///
/// ```
/// use ox_core::dag::RuleEdge;
///
/// let edge = RuleEdge::Produces;
/// assert_eq!(edge, RuleEdge::Produces);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleEdge {
    /// Rule produces this pattern.
    Produces,
    /// Rule consumes this pattern.
    Consumes,
}

// ---------------------------------------------------------------------------
// RuleGraph
// ---------------------------------------------------------------------------

/// The logical graph of rules and their pattern dependencies.
///
/// Built from `Vec<Rule>` during the parse phase. Used for:
/// - Cycle detection
/// - Rule ambiguity detection (multiple rules producing same pattern)
/// - Structural validation
/// - Pipeline-level visualization (`ox dag --group-by stage`)
///
/// # Example
///
/// ```
/// use ox_core::dag::RuleGraph;
/// use ox_core::model::*;
/// use std::collections::BTreeMap;
///
/// let rules = vec![
///     Rule {
///         name: RuleName::from("download"),
///         priority: None,
///         inputs: vec![],
///         outputs: vec![OutputPattern {
///             pattern: "data/{sample}.fastq".into(),
///             name: None,
///             format: None,
///             lifecycle: OutputLifecycle::default(),
///             materialize: MaterializePolicy::default(),
///         }],
///         execution: ExecutionBlock::Shell { command: "wget ...".into() },
///         resources: BTreeMap::new(),
///         environment: None,
///         tags: BTreeMap::new(),
///         meta: RuleMeta::default(),
///         wildcard_constraints: BTreeMap::new(),
///         when: None,
///         expand_mode: ExpandMode::default(),
///         error_strategy: ErrorStrategy::default(),
///         timeout: None,
///         executor: None,
///         log: LogConfig::default(),
///         benchmark: None,
///         retries: None,
///         params: BTreeMap::new(),
///         param_files: Vec::new(),
///         shell_executable: None,
///         reproducibility: ReproducibilityClass::default(),
///         source_line: None,
///     },
///     Rule {
///         name: RuleName::from("align"),
///         priority: None,
///         inputs: vec![InputPattern {
///             pattern: "data/{sample}.fastq".into(),
///             name: None,
///             format: None,
///         }],
///         outputs: vec![OutputPattern {
///             pattern: "results/{sample}.bam".into(),
///             name: None,
///             format: None,
///             lifecycle: OutputLifecycle::default(),
///             materialize: MaterializePolicy::default(),
///         }],
///         execution: ExecutionBlock::Shell { command: "bwa mem ...".into() },
///         resources: BTreeMap::new(),
///         environment: None,
///         tags: BTreeMap::new(),
///         meta: RuleMeta::default(),
///         wildcard_constraints: BTreeMap::new(),
///         when: None,
///         expand_mode: ExpandMode::default(),
///         error_strategy: ErrorStrategy::default(),
///         timeout: None,
///         executor: None,
///         log: LogConfig::default(),
///         benchmark: None,
///         retries: None,
///         params: BTreeMap::new(),
///         param_files: Vec::new(),
///         shell_executable: None,
///         reproducibility: ReproducibilityClass::default(),
///         source_line: None,
///     },
/// ];
///
/// let graph = RuleGraph::build(rules).unwrap();
/// assert!(graph.is_acyclic());
/// assert_eq!(graph.rule_count(), 2);
///
/// // "download" is upstream of "align" via the shared pattern
/// let upstream = graph.upstream(&RuleName::from("align")).unwrap();
/// assert_eq!(upstream.len(), 1);
/// assert_eq!(upstream[0].as_str(), "download");
/// ```
#[derive(Debug)]
pub struct RuleGraph {
    graph: DiGraph<RuleNode, RuleEdge>,
    rule_indices: BTreeMap<String, NodeIndex>,
    pattern_indices: BTreeMap<TargetPattern, NodeIndex>,
}

impl RuleGraph {
    /// Build a RuleGraph from parsed rules.
    ///
    /// Returns errors for conflicting outputs (two rules producing the same
    /// pattern). Cycles are not treated as build-time errors — use
    /// [`is_acyclic`](Self::is_acyclic) or [`find_cycle`](Self::find_cycle)
    /// to check after construction.
    ///
    /// # Errors
    ///
    /// - [`DagError::ConflictingOutputs`] if two rules produce the same
    ///   output pattern.
    pub fn build(rules: Vec<Rule>) -> Result<Self, DagError> {
        let mut graph = DiGraph::new();
        let mut rule_indices = BTreeMap::new();
        let mut pattern_indices = BTreeMap::new();
        // Track which rule produces each output pattern, for conflict detection.
        let mut output_owners: BTreeMap<TargetPattern, String> = BTreeMap::new();

        // Phase 1: Add all rule nodes.
        for rule in &rules {
            let idx = graph.add_node(RuleNode::Rule(Box::new(rule.clone())));
            rule_indices.insert(rule.name.0.clone(), idx);
        }

        // Phase 2: Add pattern nodes and edges.
        for rule in &rules {
            let rule_idx = rule_indices[&rule.name.0];

            // Output patterns: Rule ──Produces──▶ Pattern
            for output in &rule.outputs {
                let pat = &output.pattern;

                // Conflict detection: two rules producing the same pattern.
                if let Some(existing) = output_owners.get(pat) {
                    if existing != &rule.name.0 {
                        return Err(DagError::ConflictingOutputs {
                            a: existing.clone(),
                            b: rule.name.0.clone(),
                            output: pat.to_string(),
                        });
                    }
                } else {
                    output_owners.insert(pat.clone(), rule.name.0.clone());
                }

                let pat_idx = *pattern_indices
                    .entry(pat.clone())
                    .or_insert_with(|| graph.add_node(RuleNode::Pattern(pat.clone())));
                graph.add_edge(rule_idx, pat_idx, RuleEdge::Produces);
            }

            // Input patterns: Pattern ──Consumes──▶ Rule
            for input in &rule.inputs {
                let pat = &input.pattern;
                let pat_idx = *pattern_indices
                    .entry(pat.clone())
                    .or_insert_with(|| graph.add_node(RuleNode::Pattern(pat.clone())));
                graph.add_edge(pat_idx, rule_idx, RuleEdge::Consumes);
            }
        }

        Ok(Self {
            graph,
            rule_indices,
            pattern_indices,
        })
    }

    /// Return the [`Rule`] stored at `idx`, or a [`DagError::CorruptedGraph`]
    /// if the node is not a `Rule` variant. The invariant is established by
    /// [`build`](Self::build); this helper lets read-path methods propagate
    /// violations as `Result` instead of panicking.
    fn rule_at(&self, idx: NodeIndex) -> Result<&Rule, DagError> {
        self.graph[idx]
            .as_rule()
            .ok_or_else(|| DagError::CorruptedGraph {
                detail: "expected Rule node in rule_indices".into(),
            })
    }

    /// Get all rule names in the graph.
    ///
    /// The order is deterministic (sorted by rule name) because the internal
    /// index uses a `BTreeMap`.
    ///
    /// # Errors
    ///
    /// Returns [`DagError::CorruptedGraph`] if the internal index is
    /// inconsistent (should never happen after a successful [`build`](Self::build)).
    pub fn rule_names(&self) -> Result<Vec<&RuleName>, DagError> {
        self.rule_indices
            .values()
            .map(|idx| Ok(&self.rule_at(*idx)?.name))
            .collect()
    }

    /// Get a rule by name.
    ///
    /// Returns `Ok(None)` if no rule with the given name exists.
    ///
    /// # Errors
    ///
    /// Returns [`DagError::CorruptedGraph`] if the internal index is
    /// inconsistent.
    pub fn get_rule(&self, name: &RuleName) -> Result<Option<&Rule>, DagError> {
        match self.rule_indices.get(&name.0) {
            Some(idx) => Ok(Some(self.rule_at(*idx)?)),
            None => Ok(None),
        }
    }

    /// Get upstream dependencies of a rule (rules whose outputs are this
    /// rule's inputs).
    ///
    /// Traversal: for each input pattern of the target rule, find all rules
    /// that produce that pattern.
    ///
    /// Returns `Ok(vec![])` if `name` is not in the graph.
    ///
    /// # Errors
    ///
    /// Returns [`DagError::CorruptedGraph`] if the internal graph structure
    /// is inconsistent.
    pub fn upstream(&self, name: &RuleName) -> Result<Vec<&RuleName>, DagError> {
        let rule_idx = match self.rule_indices.get(&name.0) {
            Some(idx) => *idx,
            None => return Ok(vec![]),
        };

        let rule = self.rule_at(rule_idx)?;

        let mut upstream = Vec::new();
        for input in &rule.inputs {
            let pat_idx = self.pattern_indices.get(&input.pattern).ok_or_else(|| {
                DagError::CorruptedGraph {
                    detail: format!(
                        "input pattern `{}` not registered in pattern_indices",
                        input.pattern
                    ),
                }
            })?;
            // Find rules that produce this pattern: look for edges
            // Rule ──Produces──▶ Pattern, i.e. incoming edges to the
            // pattern node with Produces label.
            for edge in self.graph.edges_directed(*pat_idx, Direction::Incoming) {
                debug_assert_eq!(*edge.weight(), RuleEdge::Produces);
                let r = self.rule_at(edge.source())?;
                if r.name != *name {
                    upstream.push(&r.name);
                }
            }
        }
        upstream.sort_by_key(|n| &n.0);
        upstream.dedup_by_key(|n| &n.0);
        Ok(upstream)
    }

    /// Get downstream dependents of a rule (rules that consume this rule's
    /// outputs).
    ///
    /// Traversal: for each output pattern of the target rule, find all rules
    /// that consume that pattern.
    ///
    /// Returns `Ok(vec![])` if `name` is not in the graph.
    ///
    /// # Errors
    ///
    /// Returns [`DagError::CorruptedGraph`] if the internal graph structure
    /// is inconsistent.
    pub fn downstream(&self, name: &RuleName) -> Result<Vec<&RuleName>, DagError> {
        let rule_idx = match self.rule_indices.get(&name.0) {
            Some(idx) => *idx,
            None => return Ok(vec![]),
        };

        let rule = self.rule_at(rule_idx)?;

        let mut downstream = Vec::new();
        for output in &rule.outputs {
            let pat_idx = self.pattern_indices.get(&output.pattern).ok_or_else(|| {
                DagError::CorruptedGraph {
                    detail: format!(
                        "output pattern `{}` not registered in pattern_indices",
                        output.pattern
                    ),
                }
            })?;
            // Find rules that consume this pattern: look for edges
            // Pattern ──Consumes──▶ Rule, i.e. outgoing edges from the
            // pattern node with Consumes label.
            for edge in self.graph.edges_directed(*pat_idx, Direction::Outgoing) {
                debug_assert_eq!(*edge.weight(), RuleEdge::Consumes);
                let r = self.rule_at(edge.target())?;
                if r.name != *name {
                    downstream.push(&r.name);
                }
            }
        }
        downstream.sort_by_key(|n| &n.0);
        downstream.dedup_by_key(|n| &n.0);
        Ok(downstream)
    }

    /// Check if the graph is acyclic.
    ///
    /// A cycle in the RuleGraph means a circular dependency between rules,
    /// which would make execution impossible.
    pub fn is_acyclic(&self) -> bool {
        !is_cyclic_directed(&self.graph)
    }

    /// Detect cycles and return the cycle path if found.
    ///
    /// Returns `Ok(None)` if the graph is acyclic. If a cycle exists, returns
    /// the rule names forming the cycle (may not be the shortest cycle).
    ///
    /// # Errors
    ///
    /// Returns [`DagError::CorruptedGraph`] if a node in the cycle has no
    /// outgoing edges (should be impossible for a true cycle).
    pub fn find_cycle(&self) -> Result<Option<Vec<RuleName>>, DagError> {
        // petgraph's toposort returns Err(Cycle { node_id }) when a cycle
        // exists. We use that to find the cycle.
        match toposort(&self.graph, None) {
            Ok(_) => Ok(None),
            Err(cycle) => {
                // The cycle error gives us one node in the cycle.
                // Walk from that node to reconstruct a cycle path through
                // rule nodes only.
                let start = cycle.node_id();
                let mut path = Vec::new();
                let mut visited = std::collections::HashSet::new();
                let mut current = start;

                loop {
                    // Collect the rule name if this is a rule node.
                    if let RuleNode::Rule(r) = &self.graph[current] {
                        if visited.contains(&r.name.0) {
                            path.push(r.name.clone());
                            break;
                        }
                        visited.insert(r.name.0.clone());
                        path.push(r.name.clone());
                    }

                    // Follow outgoing edges to find the next node.
                    // A node in a cycle always has at least one outgoing edge.
                    current = self
                        .graph
                        .edges_directed(current, Direction::Outgoing)
                        .next()
                        .ok_or_else(|| DagError::CorruptedGraph {
                            detail: "node in a cycle has no outgoing edges".into(),
                        })?
                        .target();
                }

                // In a bipartite graph (Rule <-> Pattern), a cycle always
                // passes through at least one rule node.
                debug_assert!(!path.is_empty());
                Ok(Some(path))
            }
        }
    }

    /// Topological sort of rules (for execution ordering).
    ///
    /// Returns rule names in an order where every rule appears after all of
    /// its dependencies. Only rule nodes are included (pattern nodes are
    /// filtered out).
    ///
    /// # Errors
    ///
    /// Returns [`DagError::CycleDetected`] if the graph contains a cycle.
    pub fn topological_order(&self) -> Result<Vec<&RuleName>, DagError> {
        let sorted = toposort(&self.graph, None).map_err(|_| {
            let cycle = self
                .find_cycle()
                .ok()
                .flatten()
                .unwrap_or_else(|| vec![RuleName("(unknown)".into())]);
            DagError::CycleDetected {
                cycle: cycle.into_iter().map(|n| n.0).collect(),
            }
        })?;

        Ok(sorted
            .into_iter()
            .filter_map(|idx| match &self.graph[idx] {
                RuleNode::Rule(r) => Some(&r.name),
                _ => None,
            })
            .collect())
    }

    /// Find all rules that produce outputs matching a given pattern string.
    ///
    /// This performs an exact string match on the output pattern. For
    /// wildcard-aware matching (e.g., does `results/{sample}.bam` match
    /// `results/A.bam`?), use the resolver module.
    /// # Errors
    ///
    /// Returns [`DagError::CorruptedGraph`] if the internal graph structure
    /// is inconsistent.
    pub fn producers_of(&self, pattern: &str) -> Result<Vec<&RuleName>, DagError> {
        let pat_idx = match self.pattern_indices.get(pattern) {
            Some(idx) => *idx,
            None => return Ok(vec![]),
        };

        let mut producers: Vec<&RuleName> = self
            .graph
            .edges_directed(pat_idx, Direction::Incoming)
            .map(|edge| {
                debug_assert_eq!(*edge.weight(), RuleEdge::Produces);
                Ok(&self.rule_at(edge.source())?.name)
            })
            .collect::<Result<Vec<_>, DagError>>()?;
        producers.sort_by_key(|n| &n.0);
        Ok(producers)
    }

    /// Get the total number of rules.
    pub fn rule_count(&self) -> usize {
        self.rule_indices.len()
    }

    /// Get the total number of edges (dependencies) in the graph.
    ///
    /// This counts all edges including both `Produces` and `Consumes` edges.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use std::collections::BTreeMap;

    // ── Test helpers ─────────────────────────────────────────────────────

    /// Create a minimal rule with the given name, inputs, and outputs.
    fn make_rule(name: &str, inputs: &[&str], outputs: &[&str]) -> Rule {
        Rule {
            name: RuleName::from(name),
            priority: None,
            inputs: inputs
                .iter()
                .map(|p| InputPattern {
                    pattern: (*p).into(),
                    name: None,
                    format: None,
                })
                .collect(),
            outputs: outputs
                .iter()
                .map(|p| OutputPattern {
                    pattern: (*p).into(),
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
            tags: BTreeMap::new(),
            meta: RuleMeta::default(),
            wildcard_constraints: BTreeMap::new(),
            when: None,
            expand_mode: ExpandMode::default(),
            error_strategy: ErrorStrategy::default(),
            timeout: None,
            executor: None,
            log: LogConfig::default(),
            benchmark: None,
            retries: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
            source_line: None,
        }
    }

    // ── Empty graph ─────────────────────────────────────────────────────

    #[test]
    fn empty_graph() {
        let graph = RuleGraph::build(vec![]).unwrap();
        assert_eq!(graph.rule_count(), 0);
        assert_eq!(graph.edge_count(), 0);
        assert!(graph.is_acyclic());
        assert!(graph.rule_names().unwrap().is_empty());
        assert!(graph.topological_order().unwrap().is_empty());
    }

    // ── Single rule with no dependencies ────────────────────────────────

    #[test]
    fn single_rule_no_deps() {
        let rules = vec![make_rule("build", &[], &["out.txt"])];
        let graph = RuleGraph::build(rules).unwrap();

        assert_eq!(graph.rule_count(), 1);
        assert!(graph.is_acyclic());
        assert!(graph.upstream(&RuleName::from("build")).unwrap().is_empty());
        assert!(
            graph
                .downstream(&RuleName::from("build"))
                .unwrap()
                .is_empty()
        );

        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].as_str(), "build");
    }

    // ── Linear chain: A → B → C ────────────────────────────────────────

    #[test]
    fn linear_chain() {
        let rules = vec![
            make_rule("A", &[], &["a.txt"]),
            make_rule("B", &["a.txt"], &["b.txt"]),
            make_rule("C", &["b.txt"], &["c.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();

        assert_eq!(graph.rule_count(), 3);
        assert!(graph.is_acyclic());

        // Upstream / downstream checks.
        assert!(graph.upstream(&RuleName::from("A")).unwrap().is_empty());
        assert_eq!(
            graph.upstream(&RuleName::from("B")).unwrap(),
            vec![&RuleName::from("A")]
        );
        assert_eq!(
            graph.upstream(&RuleName::from("C")).unwrap(),
            vec![&RuleName::from("B")]
        );

        assert_eq!(
            graph.downstream(&RuleName::from("A")).unwrap(),
            vec![&RuleName::from("B")]
        );
        assert_eq!(
            graph.downstream(&RuleName::from("B")).unwrap(),
            vec![&RuleName::from("C")]
        );
        assert!(graph.downstream(&RuleName::from("C")).unwrap().is_empty());

        // Topological order: A must come before B, B before C.
        let order = graph.topological_order().unwrap();
        let names: Vec<&str> = order.iter().map(|n| n.as_str()).collect();
        let pos_a = names.iter().position(|n| *n == "A").unwrap();
        let pos_b = names.iter().position(|n| *n == "B").unwrap();
        let pos_c = names.iter().position(|n| *n == "C").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    // ── Diamond dependency: A → B, A → C, B → D, C → D ────────────────

    #[test]
    fn diamond_dependency() {
        let rules = vec![
            make_rule("A", &[], &["a.txt"]),
            make_rule("B", &["a.txt"], &["b.txt"]),
            make_rule("C", &["a.txt"], &["c.txt"]),
            make_rule("D", &["b.txt", "c.txt"], &["d.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();

        assert_eq!(graph.rule_count(), 4);
        assert!(graph.is_acyclic());

        // D depends on both B and C.
        let mut upstream_d = graph
            .upstream(&RuleName::from("D"))
            .unwrap()
            .iter()
            .map(|n| n.as_str().to_owned())
            .collect::<Vec<_>>();
        upstream_d.sort();
        assert_eq!(upstream_d, vec!["B", "C"]);

        // A feeds both B and C.
        let mut downstream_a = graph
            .downstream(&RuleName::from("A"))
            .unwrap()
            .iter()
            .map(|n| n.as_str().to_owned())
            .collect::<Vec<_>>();
        downstream_a.sort();
        assert_eq!(downstream_a, vec!["B", "C"]);

        // Topological order: A before B and C, both before D.
        let order = graph.topological_order().unwrap();
        let names: Vec<&str> = order.iter().map(|n| n.as_str()).collect();
        let pos_a = names.iter().position(|n| *n == "A").unwrap();
        let pos_b = names.iter().position(|n| *n == "B").unwrap();
        let pos_c = names.iter().position(|n| *n == "C").unwrap();
        let pos_d = names.iter().position(|n| *n == "D").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
        assert!(pos_b < pos_d);
        assert!(pos_c < pos_d);
    }

    // ── Cycle detection: A → B → A ─────────────────────────────────────

    #[test]
    fn cycle_detection() {
        let rules = vec![
            make_rule("A", &["b.txt"], &["a.txt"]),
            make_rule("B", &["a.txt"], &["b.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();

        assert!(!graph.is_acyclic());
        let cycle = graph.find_cycle().unwrap();
        assert!(cycle.is_some());

        // Topological order should fail.
        let err = graph.topological_order().unwrap_err();
        assert!(matches!(err, DagError::CycleDetected { .. }));
    }

    // ── Self-cycle: A depends on its own output ────────────────────────

    #[test]
    fn self_cycle() {
        let rules = vec![make_rule("A", &["a.txt"], &["a.txt"])];
        let graph = RuleGraph::build(rules).unwrap();

        assert!(!graph.is_acyclic());
    }

    // ── Conflicting outputs ─────────────────────────────────────────────

    #[test]
    fn conflicting_outputs() {
        let rules = vec![
            make_rule("rule1", &[], &["same.txt"]),
            make_rule("rule2", &[], &["same.txt"]),
        ];
        let err = RuleGraph::build(rules).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("same.txt"), "error should mention the pattern");
        assert!(msg.contains("rule1"), "error should mention rule1");
        assert!(msg.contains("rule2"), "error should mention rule2");
        assert!(matches!(err, DagError::ConflictingOutputs { .. }));
    }

    // ── get_rule lookup ─────────────────────────────────────────────────

    #[test]
    fn get_rule_lookup() {
        let rules = vec![
            make_rule("alpha", &[], &["a.txt"]),
            make_rule("beta", &["a.txt"], &["b.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();

        let alpha = graph.get_rule(&RuleName::from("alpha")).unwrap().unwrap();
        assert_eq!(alpha.name.as_str(), "alpha");

        let beta = graph.get_rule(&RuleName::from("beta")).unwrap().unwrap();
        assert_eq!(beta.name.as_str(), "beta");

        assert!(
            graph
                .get_rule(&RuleName::from("nonexistent"))
                .unwrap()
                .is_none()
        );
    }

    // ── producers_of lookup ─────────────────────────────────────────────

    #[test]
    fn producers_of_lookup() {
        let rules = vec![
            make_rule("A", &[], &["shared.txt", "extra.txt"]),
            make_rule("B", &["shared.txt"], &["out.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();

        let producers = graph.producers_of("shared.txt").unwrap();
        assert_eq!(producers.len(), 1);
        assert_eq!(producers[0].as_str(), "A");

        let producers = graph.producers_of("extra.txt").unwrap();
        assert_eq!(producers.len(), 1);
        assert_eq!(producers[0].as_str(), "A");

        // No rule produces "missing.txt".
        assert!(graph.producers_of("missing.txt").unwrap().is_empty());
    }

    // ── upstream/downstream of nonexistent rule ─────────────────────────

    #[test]
    fn upstream_downstream_nonexistent() {
        let graph = RuleGraph::build(vec![make_rule("A", &[], &["a.txt"])]).unwrap();
        assert!(graph.upstream(&RuleName::from("nope")).unwrap().is_empty());
        assert!(
            graph
                .downstream(&RuleName::from("nope"))
                .unwrap()
                .is_empty()
        );
    }

    // ── edge_count ──────────────────────────────────────────────────────

    #[test]
    fn edge_count_linear() {
        // A --produces--> a.txt --consumes--> B --produces--> b.txt
        // That's 3 edges total.
        let rules = vec![
            make_rule("A", &[], &["a.txt"]),
            make_rule("B", &["a.txt"], &["b.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();
        // A produces a.txt (1), a.txt consumed by B (1), B produces b.txt (1)
        assert_eq!(graph.edge_count(), 3);
    }

    // ── rule_names ordering ─────────────────────────────────────────────

    #[test]
    fn rule_names_sorted() {
        let rules = vec![
            make_rule("zebra", &[], &["z.txt"]),
            make_rule("alpha", &[], &["a.txt"]),
            make_rule("mid", &[], &["m.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();
        let names: Vec<&str> = graph
            .rule_names()
            .unwrap()
            .iter()
            .map(|n| n.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "mid", "zebra"]);
    }

    // ── Multiple inputs from different producers ────────────────────────

    #[test]
    fn multiple_inputs_different_producers() {
        let rules = vec![
            make_rule("gen_a", &[], &["a.txt"]),
            make_rule("gen_b", &[], &["b.txt"]),
            make_rule("merge", &["a.txt", "b.txt"], &["merged.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();

        let mut upstream = graph
            .upstream(&RuleName::from("merge"))
            .unwrap()
            .iter()
            .map(|n| n.as_str().to_owned())
            .collect::<Vec<_>>();
        upstream.sort();
        assert_eq!(upstream, vec!["gen_a", "gen_b"]);
    }

    // ── Topological order with disconnected components ──────────────────

    #[test]
    fn disconnected_components() {
        let rules = vec![
            make_rule("X", &[], &["x.txt"]),
            make_rule("Y", &[], &["y.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();
        assert!(graph.is_acyclic());

        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 2);
    }

    // ── Same rule producing same output twice ─────────────────────────

    #[test]
    fn same_rule_duplicate_output_is_ok() {
        // A rule listing the same output pattern twice should not error.
        let mut rule = make_rule("A", &[], &["out.txt"]);
        rule.outputs.push(crate::model::OutputPattern {
            pattern: "out.txt".into(),
            name: None,
            format: None,
            lifecycle: crate::model::OutputLifecycle::default(),
            materialize: crate::model::MaterializePolicy::default(),
        });
        let graph = RuleGraph::build(vec![rule]).unwrap();
        assert_eq!(graph.rule_count(), 1);
    }

    // ── find_cycle with a real cycle returns rule names ────────────────

    #[test]
    fn find_cycle_returns_rule_names() {
        let rules = vec![
            make_rule("A", &["b.txt"], &["a.txt"]),
            make_rule("B", &["a.txt"], &["b.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();
        let cycle = graph.find_cycle().unwrap().unwrap();
        // The cycle should contain rule names (not empty).
        assert!(!cycle.is_empty());
        let names: Vec<&str> = cycle.iter().map(|n| n.as_str()).collect();
        // Both A and B should appear in the cycle path.
        assert!(names.contains(&"A") || names.contains(&"B"));
    }

    // ── find_cycle on acyclic graph returns None ──────────────────────

    #[test]
    fn find_cycle_acyclic_returns_none() {
        let rules = vec![
            make_rule("A", &[], &["a.txt"]),
            make_rule("B", &["a.txt"], &["b.txt"]),
        ];
        let graph = RuleGraph::build(rules).unwrap();
        assert!(graph.find_cycle().unwrap().is_none());
    }

    // ── RuleNode Clone and Debug ──────────────────────────────────────

    #[test]
    fn rule_node_clone_and_debug() {
        let node = RuleNode::Pattern("test.txt".into());
        let cloned = node.clone();
        let debug = format!("{:?}", cloned);
        assert!(debug.contains("Pattern"));
    }

    // ── RuleEdge Clone and Debug ──────────────────────────────────────

    #[test]
    fn rule_edge_clone_and_debug() {
        let edge = RuleEdge::Consumes;
        let cloned = edge.clone();
        assert_eq!(cloned, RuleEdge::Consumes);
        let debug = format!("{:?}", cloned);
        assert!(debug.contains("Consumes"));
    }

    // ── upstream of self-consuming rule ───────────────────────────────

    #[test]
    fn upstream_self_consuming_excludes_self() {
        // A rule whose input is its own output should not list itself as upstream.
        let rules = vec![make_rule("A", &["a.txt"], &["a.txt"])];
        let graph = RuleGraph::build(rules).unwrap();
        assert!(graph.upstream(&RuleName::from("A")).unwrap().is_empty());
    }

    // ── downstream of self-consuming rule ────────────────────────────

    #[test]
    fn downstream_self_consuming_excludes_self() {
        let rules = vec![make_rule("A", &["a.txt"], &["a.txt"])];
        let graph = RuleGraph::build(rules).unwrap();
        assert!(graph.downstream(&RuleName::from("A")).unwrap().is_empty());
    }

    // ── producers_of with no producing rule ──────────────────────────

    #[test]
    fn producers_of_input_only_pattern() {
        // A pattern that is only consumed (no producer) should return empty.
        let rules = vec![make_rule("A", &["input.txt"], &["output.txt"])];
        let graph = RuleGraph::build(rules).unwrap();
        // "input.txt" exists as a pattern node but has no Produces edge.
        assert!(graph.producers_of("input.txt").unwrap().is_empty());
    }

    // ── RuleNode::as_rule ────────────────────────────────────────────

    #[test]
    fn rule_node_as_rule() {
        let rule = make_rule("test", &[], &["out.txt"]);
        let node = RuleNode::Rule(Box::new(rule));
        assert!(node.as_rule().is_some());
        assert_eq!(node.as_rule().unwrap().name.as_str(), "test");

        let pattern_node = RuleNode::Pattern("file.txt".into());
        assert!(pattern_node.as_rule().is_none());
    }

    // ── RuleGraph Debug impl ─────────────────────────────────────────

    #[test]
    fn rule_graph_debug() {
        let graph = RuleGraph::build(vec![make_rule("A", &[], &["a.txt"])]).unwrap();
        let debug = format!("{:?}", graph);
        assert!(debug.contains("RuleGraph"));
    }

    // ── Property-based tests ─────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy that generates a random set of rules forming a DAG topology.
        ///
        /// Produces `num_files` file names and `num_rules` rules.  Each rule
        /// produces exactly one unique output and may consume any subset of
        /// other files as inputs.  The resulting graph may be acyclic or cyclic.
        fn arb_dag_topology() -> impl Strategy<Value = Vec<Rule>> {
            (2..=10usize, 1..=8usize)
                .prop_flat_map(|(nf, nr)| {
                    let nr = nr.min(nf);
                    let input_flags = proptest::collection::vec(
                        proptest::collection::vec(proptest::bool::ANY, nf),
                        nr,
                    );
                    (Just(nf), Just(nr), input_flags)
                })
                .prop_map(|(nf, nr, inp)| {
                    let files: Vec<String> = (0..nf).map(|i| format!("f{i}.txt")).collect();
                    (0..nr)
                        .map(|i| {
                            let inputs: Vec<&str> = inp[i]
                                .iter()
                                .enumerate()
                                .filter(|e| *e.1 && e.0 != i)
                                .map(|(j, _)| files[j].as_str())
                                .collect();
                            make_rule(&format!("r{i}"), &inputs, &[files[i].as_str()])
                        })
                        .collect()
                })
        }

        /// Strategy for an acyclic DAG: rule i can only consume files j
        /// where j < i (topological ordering by construction).
        fn arb_acyclic_dag() -> impl Strategy<Value = Vec<Rule>> {
            (2..=10usize, 1..=8usize)
                .prop_flat_map(|(nf, nr)| {
                    let nr = nr.min(nf);
                    let input_flags = (0..nr)
                        .map(|i| proptest::collection::vec(proptest::bool::ANY, i.max(1)))
                        .collect::<Vec<_>>();
                    (Just(nf), Just(nr), input_flags)
                })
                .prop_map(|(nf, nr, inp)| {
                    let files: Vec<String> = (0..nf).map(|i| format!("f{i}.txt")).collect();
                    (0..nr)
                        .map(|i| {
                            let inputs: Vec<&str> = if i == 0 {
                                vec![]
                            } else {
                                inp[i]
                                    .iter()
                                    .enumerate()
                                    .filter(|e| *e.1 && e.0 < i)
                                    .map(|(j, _)| files[j].as_str())
                                    .collect()
                            };
                            make_rule(&format!("r{i}"), &inputs, &[files[i].as_str()])
                        })
                        .collect()
                })
        }

        proptest! {
            /// RuleGraph::build must never panic on any random topology.
            #[test]
            fn build_never_panics(rules in arb_dag_topology()) {
                let _ = RuleGraph::build(rules);
            }

            /// When build succeeds, is_acyclic and find_cycle must agree.
            #[test]
            fn acyclic_and_find_cycle_consistent(rules in arb_dag_topology()) {
                if let Ok(graph) = RuleGraph::build(rules) {
                    let is_acyclic = graph.is_acyclic();
                    if let Ok(cycle) = graph.find_cycle() {
                        prop_assert_eq!(
                            is_acyclic,
                            cycle.is_none(),
                            "is_acyclic and find_cycle must agree",
                        );
                    }
                    // find_cycle returning Err is acceptable on some topologies
                }
            }

            /// Acyclic-by-construction graphs must always pass the acyclicity check.
            #[test]
            fn acyclic_by_construction(rules in arb_acyclic_dag()) {
                let graph = RuleGraph::build(rules).unwrap();
                prop_assert!(
                    graph.is_acyclic(),
                    "acyclic-by-construction graph flagged as cyclic",
                );
            }

            /// rule_count matches the number of input rules on successful build.
            #[test]
            fn rule_count_matches(rules in arb_dag_topology()) {
                let n = rules.len();
                if let Ok(graph) = RuleGraph::build(rules) {
                    prop_assert_eq!(
                        graph.rule_count(),
                        n,
                        "rule_count must match input length",
                    );
                }
            }

            /// Topological order of acyclic graphs has correct length.
            #[test]
            fn topological_order_length(rules in arb_acyclic_dag()) {
                let n = rules.len();
                let graph = RuleGraph::build(rules).unwrap();
                let order = graph.topological_order().unwrap();
                prop_assert_eq!(
                    order.len(),
                    n,
                    "topological order length must match rule count",
                );
            }

            /// RuleGraph::build is deterministic: same input → same result.
            #[test]
            fn build_is_deterministic(rules in arb_acyclic_dag()) {
                let g1 = RuleGraph::build(rules.clone()).unwrap();
                let g2 = RuleGraph::build(rules).unwrap();
                prop_assert_eq!(
                    g1.rule_count(),
                    g2.rule_count(),
                    "determinism: rule counts must match",
                );
                let o1 = g1.topological_order().unwrap();
                let o2 = g2.topological_order().unwrap();
                let n1: Vec<&str> = o1.iter().map(|r| r.as_str()).collect();
                let n2: Vec<&str> = o2.iter().map(|r| r.as_str()).collect();
                prop_assert_eq!(n1, n2, "determinism: topological order must match");
            }
        }
    }
}
