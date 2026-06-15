//! Implementation of `ox query` — Bazel-style graph query language for the DAG.
//!
//! Supported query expressions:
//! - `deps(X)` — transitive dependencies of target X
//! - `rdeps(X)` — transitive reverse dependencies (dependents) of target X
//! - `allpaths(X, Y)` — all nodes on any path from X to Y

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

use anyhow::{Context, Result};

use ox_core::job_graph::JobGraph;
use ox_core::model::{JobId, OutputRef};
use ox_core::resolver;

use super::common;

#[derive(clap::Args)]
pub struct QueryArgs {
    /// Query expression, e.g. 'deps(annotate)', 'rdeps(data)', 'allpaths(data, annotate)'
    pub expression: String,

    /// Output JSON instead of human-readable text
    #[arg(long)]
    pub json: bool,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,
}

/// Parsed query expression.
enum QueryExpr {
    /// Transitive dependencies (upstream).
    Deps(String),
    /// Transitive reverse dependencies (downstream).
    Rdeps(String),
    /// All nodes on any path from source to target.
    AllPaths(String, String),
}

pub fn cmd_query(args: QueryArgs) -> Result<()> {
    let expr = parse_expression(&args.expression)?;

    let file_path = PathBuf::from(&args.file);
    let workflow = common::load_workflow(&file_path)?;

    ox_format::validate::validate(&workflow).map_err(|errs| {
        let messages: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        anyhow::anyhow!("validation errors:\n  {}", messages.join("\n  "))
    })?;

    // Resolve all targets in the workflow to build the full graph.
    let config = common::workflow_config(&workflow);
    let targets = common::resolve_targets(&workflow, &[]);

    if targets.is_empty() {
        anyhow::bail!("no targets found in workflow");
    }

    let existing_files = common::discover_existing_files(&file_path);
    let request = resolver::ResolveRequest {
        targets,
        config,
        existing_files,
    };

    let resolve_result =
        resolver::resolve(&workflow.rules, &request).context("failed to resolve targets")?;
    let job_graph = JobGraph::build(resolve_result.jobs).context("failed to build JobGraph")?;

    let result = execute_query(&expr, &job_graph)?;

    if args.json {
        print_json(&args.expression, &result, &job_graph);
    } else {
        print_text(&args.expression, &result, &job_graph);
    }

    Ok(())
}

/// Parse a query expression string into a structured enum.
fn parse_expression(expr: &str) -> Result<QueryExpr> {
    let expr = expr.trim();

    if let Some(inner) = strip_func(expr, "deps") {
        Ok(QueryExpr::Deps(inner))
    } else if let Some(inner) = strip_func(expr, "rdeps") {
        Ok(QueryExpr::Rdeps(inner))
    } else if let Some(inner) = strip_func(expr, "allpaths") {
        let (a, b) = split_two_args(&inner)?;
        Ok(QueryExpr::AllPaths(a, b))
    } else {
        anyhow::bail!(
            "unknown query expression: '{}'\n\
             Supported: deps(X), rdeps(X), allpaths(X, Y)",
            expr
        );
    }
}

/// Strip a function call like `func(...)` and return the inner content.
fn strip_func(expr: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}(");
    if expr.starts_with(&prefix) && expr.ends_with(')') {
        Some(expr[prefix.len()..expr.len() - 1].trim().to_string())
    } else {
        None
    }
}

/// Split a string like "A, B" into two trimmed parts.
fn split_two_args(s: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = s.splitn(2, ',').collect();
    if parts.len() != 2 {
        anyhow::bail!("expected two arguments separated by comma, got: '{s}'");
    }
    Ok((parts[0].trim().to_string(), parts[1].trim().to_string()))
}

/// Find the job matching a target name (by job ID or output path).
fn find_target_job(job_graph: &JobGraph, target: &str) -> Result<JobId> {
    // Try matching by job ID directly.
    let target_id = JobId::from(target);
    if job_graph.get_job(&target_id).is_some() {
        return Ok(target_id);
    }

    // Try matching by output path.
    for job_id in job_graph.job_ids() {
        let job = job_graph.get_job(job_id).unwrap();
        for output in &job.outputs {
            let key = match &output.reference {
                OutputRef::File(p) => p.to_string_lossy().to_string(),
                OutputRef::Virtual { id, .. } => id.clone(),
                OutputRef::InMemory { type_hint } => {
                    type_hint.clone().unwrap_or_else(|| "<memory>".into())
                }
            };
            if key == target {
                return Ok(job_id.clone());
            }
        }
    }

    // Try matching by rule name (return all jobs for that rule).
    let matching: Vec<_> = job_graph
        .job_ids()
        .into_iter()
        .filter(|id| {
            job_graph
                .get_job(id)
                .map(|j| j.rule.as_str() == target)
                .unwrap_or(false)
        })
        .collect();

    if matching.len() == 1 {
        return Ok(matching[0].clone());
    }

    anyhow::bail!(
        "no job found matching '{}'. Use `ox plan` to see available jobs.",
        target
    )
}

/// Execute a query expression against the job graph.
fn execute_query<'a>(expr: &QueryExpr, graph: &'a JobGraph) -> Result<Vec<&'a JobId>> {
    match expr {
        QueryExpr::Deps(target) => {
            let root = find_target_job(graph, target)?;
            Ok(bfs_collect(graph, &root, Direction::Upstream))
        }
        QueryExpr::Rdeps(target) => {
            let root = find_target_job(graph, target)?;
            Ok(bfs_collect(graph, &root, Direction::Downstream))
        }
        QueryExpr::AllPaths(source, target) => {
            let src = find_target_job(graph, source)?;
            let dst = find_target_job(graph, target)?;
            all_paths(graph, &src, &dst)
        }
    }
}

enum Direction {
    Upstream,
    Downstream,
}

/// BFS traversal collecting all transitive deps or rdeps (excluding the root).
fn bfs_collect<'a>(graph: &'a JobGraph, root: &JobId, dir: Direction) -> Vec<&'a JobId> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut result = Vec::new();

    visited.insert(root.clone());
    queue.push_back(root.clone());

    while let Some(current) = queue.pop_front() {
        let neighbors = match dir {
            Direction::Upstream => graph.upstream(&current),
            Direction::Downstream => graph.downstream(&current),
        };

        for neighbor in neighbors {
            if visited.insert(neighbor.clone()) {
                result.push(neighbor);
                queue.push_back(neighbor.clone());
            }
        }
    }

    result
}

/// Find all nodes on any path from source to target.
///
/// Algorithm: compute the set of nodes reachable from `source` (downstream)
/// and the set of nodes that can reach `target` (upstream from target).
/// The intersection gives nodes on some path from source to target.
fn all_paths<'a>(graph: &'a JobGraph, source: &JobId, target: &JobId) -> Result<Vec<&'a JobId>> {
    // Nodes reachable from source (including source).
    let mut reachable_from_source = HashSet::new();
    reachable_from_source.insert(source.clone());
    for id in bfs_collect(graph, source, Direction::Downstream) {
        reachable_from_source.insert(id.clone());
    }

    // Nodes that can reach target (including target).
    let mut can_reach_target = HashSet::new();
    can_reach_target.insert(target.clone());
    for id in bfs_collect(graph, target, Direction::Upstream) {
        can_reach_target.insert(id.clone());
    }

    // Intersection = nodes on some path from source to target.
    let on_path: HashSet<_> = reachable_from_source
        .intersection(&can_reach_target)
        .cloned()
        .collect();

    if on_path.is_empty() {
        anyhow::bail!(
            "no path found from '{}' to '{}'",
            source.as_str(),
            target.as_str()
        );
    }

    // Return in topological order by filtering the full topo order.
    let topo = graph.topological_order().unwrap_or_default();
    let result: Vec<&JobId> = topo
        .into_iter()
        .filter(|id| on_path.contains(*id))
        .collect();

    Ok(result)
}

fn format_output_ref(r: &OutputRef) -> String {
    match r {
        OutputRef::File(p) => p.display().to_string(),
        OutputRef::Virtual { id, .. } => id.clone(),
        OutputRef::InMemory { type_hint } => type_hint.clone().unwrap_or_else(|| "<memory>".into()),
    }
}

fn print_text(expression: &str, results: &[&JobId], job_graph: &JobGraph) {
    println!("Query: {}", expression);
    println!("Results: {} job(s)", results.len());
    println!();

    for job_id in results {
        if let Some(job) = job_graph.get_job(job_id) {
            let outputs: Vec<String> = job
                .outputs
                .iter()
                .map(|o| format_output_ref(&o.reference))
                .collect();
            println!(
                "  {} (rule={}, outputs=[{}])",
                job_id.as_str(),
                job.rule.as_str(),
                outputs.join(", ")
            );
        } else {
            println!("  {}", job_id.as_str());
        }
    }

    if results.is_empty() {
        println!("  (no results)");
    }
}

fn print_json(expression: &str, results: &[&JobId], job_graph: &JobGraph) {
    let jobs_json: Vec<serde_json::Value> = results
        .iter()
        .map(|job_id| {
            if let Some(job) = job_graph.get_job(job_id) {
                let inputs: Vec<String> = job
                    .inputs
                    .iter()
                    .map(|i| format_output_ref(&i.reference))
                    .collect();
                let outputs: Vec<String> = job
                    .outputs
                    .iter()
                    .map(|o| format_output_ref(&o.reference))
                    .collect();
                serde_json::json!({
                    "job_id": job_id.as_str(),
                    "rule": job.rule.as_str(),
                    "inputs": inputs,
                    "outputs": outputs,
                })
            } else {
                serde_json::json!({
                    "job_id": job_id.as_str(),
                })
            }
        })
        .collect();

    let result = serde_json::json!({
        "query": expression,
        "count": results.len(),
        "jobs": jobs_json,
    });

    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::job_graph::make_test_job;

    /// Build a diamond-shaped test graph:
    ///   download -> align -> sort -> merge
    fn diamond_graph() -> JobGraph {
        let jobs = vec![
            make_test_job("download", &[], &["data.fastq"]),
            make_test_job("align", &["data.fastq"], &["aligned.bam"]),
            make_test_job("sort", &["aligned.bam"], &["sorted.bam"]),
            make_test_job("merge", &["sorted.bam"], &["final.bam"]),
        ];
        JobGraph::build(jobs).unwrap()
    }

    #[test]
    fn parse_deps() {
        let expr = parse_expression("deps(annotate)").unwrap();
        assert!(matches!(expr, QueryExpr::Deps(ref s) if s == "annotate"));
    }

    #[test]
    fn parse_rdeps() {
        let expr = parse_expression("rdeps(data)").unwrap();
        assert!(matches!(expr, QueryExpr::Rdeps(ref s) if s == "data"));
    }

    #[test]
    fn parse_allpaths() {
        let expr = parse_expression("allpaths(data, annotate)").unwrap();
        assert!(
            matches!(expr, QueryExpr::AllPaths(ref a, ref b) if a == "data" && b == "annotate")
        );
    }

    #[test]
    fn parse_allpaths_no_spaces() {
        let expr = parse_expression("allpaths(data,annotate)").unwrap();
        assert!(
            matches!(expr, QueryExpr::AllPaths(ref a, ref b) if a == "data" && b == "annotate")
        );
    }

    #[test]
    fn parse_unknown_func() {
        assert!(parse_expression("unknown(x)").is_err());
    }

    #[test]
    fn deps_linear_chain() {
        let graph = diamond_graph();
        let result = bfs_collect(&graph, &JobId::from("merge"), Direction::Upstream);
        let names: Vec<&str> = result.iter().map(|id| id.as_str()).collect();
        assert!(names.contains(&"sort"));
        assert!(names.contains(&"align"));
        assert!(names.contains(&"download"));
        assert!(!names.contains(&"merge")); // root excluded
    }

    #[test]
    fn rdeps_linear_chain() {
        let graph = diamond_graph();
        let result = bfs_collect(&graph, &JobId::from("download"), Direction::Downstream);
        let names: Vec<&str> = result.iter().map(|id| id.as_str()).collect();
        assert!(names.contains(&"align"));
        assert!(names.contains(&"sort"));
        assert!(names.contains(&"merge"));
        assert!(!names.contains(&"download")); // root excluded
    }

    #[test]
    fn deps_leaf_node() {
        let graph = diamond_graph();
        let result = bfs_collect(&graph, &JobId::from("download"), Direction::Upstream);
        assert!(result.is_empty());
    }

    #[test]
    fn rdeps_leaf_node() {
        let graph = diamond_graph();
        let result = bfs_collect(&graph, &JobId::from("merge"), Direction::Downstream);
        assert!(result.is_empty());
    }

    #[test]
    fn allpaths_linear() {
        let graph = diamond_graph();
        let result = all_paths(&graph, &JobId::from("download"), &JobId::from("merge")).unwrap();
        let names: Vec<&str> = result.iter().map(|id| id.as_str()).collect();
        assert_eq!(names, vec!["download", "align", "sort", "merge"]);
    }

    #[test]
    fn allpaths_subset() {
        let graph = diamond_graph();
        let result = all_paths(&graph, &JobId::from("align"), &JobId::from("sort")).unwrap();
        let names: Vec<&str> = result.iter().map(|id| id.as_str()).collect();
        assert_eq!(names, vec!["align", "sort"]);
    }

    #[test]
    fn allpaths_no_path() {
        let graph = diamond_graph();
        // merge -> download has no forward path
        assert!(all_paths(&graph, &JobId::from("merge"), &JobId::from("download")).is_err());
    }

    #[test]
    fn find_target_by_job_id() {
        let graph = diamond_graph();
        let id = find_target_job(&graph, "align").unwrap();
        assert_eq!(id.as_str(), "align");
    }

    #[test]
    fn find_target_by_output() {
        let graph = diamond_graph();
        let id = find_target_job(&graph, "final.bam").unwrap();
        assert_eq!(id.as_str(), "merge");
    }

    #[test]
    fn find_target_missing() {
        let graph = diamond_graph();
        assert!(find_target_job(&graph, "nonexistent").is_err());
    }
}
