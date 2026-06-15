//! Implementation of `ox explain` — show the full dependency chain for a target.

use std::collections::VecDeque;
use std::path::PathBuf;

use anyhow::{Context, Result};

use ox_core::job_graph::JobGraph;
use ox_core::model::{JobId, OutputRef};
use ox_core::resolver;

use super::common;

#[derive(clap::Args)]
pub struct ExplainArgs {
    /// The output file path or job ID to explain
    pub target: String,

    /// Output JSON instead of human-readable text
    #[arg(long)]
    pub json: bool,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,
}

pub fn cmd_explain(args: ExplainArgs) -> Result<()> {
    let file_path = PathBuf::from(&args.file);
    let workflow = common::load_workflow(&file_path)?;

    ox_format::validate::validate(&workflow).map_err(|errs| {
        let messages: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        anyhow::anyhow!("validation errors:\n  {}", messages.join("\n  "))
    })?;

    let config = common::workflow_config(&workflow);
    let targets = common::resolve_targets(&workflow, std::slice::from_ref(&args.target));

    if targets.is_empty() {
        anyhow::bail!("no targets resolved for '{}'", args.target);
    }

    let mut existing_files = common::discover_existing_files(&file_path);
    // Remove target outputs from existing files so the resolver doesn't treat
    // them as sources — explain should always resolve the full dependency chain
    // even when outputs already exist on disk.
    let target_paths: std::collections::HashSet<_> =
        targets.iter().map(std::path::PathBuf::from).collect();
    existing_files.retain(|p| !target_paths.contains(p));

    let request = resolver::ResolveRequest {
        targets,
        config,
        existing_files,
    };

    let resolve_result =
        resolver::resolve(&workflow.rules, &request).context("failed to resolve targets")?;

    let job_graph = JobGraph::build(resolve_result.jobs).context("failed to build JobGraph")?;

    // Find the job that produces the target output, or a job matching by ID.
    let root_job_id = find_target_job(&job_graph, &args.target)?;

    // BFS backward through upstream dependencies to build the chain.
    let chain = build_chain(&job_graph, &root_job_id);

    if args.json {
        print_json(&args.target, &chain, &job_graph);
    } else {
        print_text(&args.target, &chain, &job_graph);
    }

    Ok(())
}

/// Find the job that produces the given target (file path or job ID).
fn find_target_job(job_graph: &JobGraph, target: &str) -> Result<JobId> {
    // First, try matching by job ID directly.
    let target_id = JobId::from(target);
    if job_graph.get_job(&target_id).is_some() {
        return Ok(target_id);
    }

    // Otherwise, find the job that produces an output matching the target path.
    for job_id in job_graph.job_ids() {
        let job = job_graph
            .get_job(job_id)
            .expect("BUG: job_ids() returned an ID not present in the graph");
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

    anyhow::bail!(
        "no job found that produces '{}'. Use `ox plan` to see available targets.",
        target
    )
}

/// Build the dependency chain by walking upstream from the root job.
/// Returns jobs ordered from the root (index 0) back to sources.
fn build_chain<'a>(job_graph: &'a JobGraph, root: &JobId) -> Vec<&'a JobId> {
    let mut chain = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut queue = VecDeque::new();

    queue.push_back(root.clone());
    visited.insert(root.clone());

    while let Some(current) = queue.pop_front() {
        if let Some(id) = job_graph.job_ids().into_iter().find(|id| **id == current) {
            chain.push(id);
        }

        for upstream_id in job_graph.upstream(&current) {
            if visited.insert(upstream_id.clone()) {
                queue.push_back(upstream_id.clone());
            }
        }
    }

    chain
}

fn format_output_ref(r: &OutputRef) -> String {
    match r {
        OutputRef::File(p) => p.display().to_string(),
        OutputRef::Virtual { id, .. } => id.clone(),
        OutputRef::InMemory { type_hint } => type_hint.clone().unwrap_or_else(|| "<memory>".into()),
    }
}

fn print_text(target: &str, chain: &[&JobId], job_graph: &JobGraph) {
    println!("Dependency chain for: {}", target);
    println!();

    for (i, job_id) in chain.iter().enumerate() {
        let job = job_graph
            .get_job(job_id)
            .expect("BUG: chain contains a job ID not present in the graph");
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

        let indent = if i == 0 { "► " } else { "  " };
        println!(
            "{}{}. [{}] rule={}",
            indent,
            i + 1,
            job_id.as_str(),
            job.rule.as_str()
        );
        println!("     inputs:  [{}]", inputs.join(", "));
        println!("     outputs: [{}]", outputs.join(", "));
    }

    if chain.is_empty() {
        println!("  (no dependency chain found)");
    }
}

fn print_json(target: &str, chain: &[&JobId], job_graph: &JobGraph) {
    let chain_json: Vec<serde_json::Value> = chain
        .iter()
        .map(|job_id| {
            let job = job_graph
                .get_job(job_id)
                .expect("BUG: chain contains a job ID not present in the graph");
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
        })
        .collect();

    let result = serde_json::json!({
        "target": target,
        "chain": chain_json,
    });

    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::job_graph::{JobGraph, make_test_job};

    fn test_graph() -> JobGraph {
        let jobs = vec![
            make_test_job("compile", &[], &["obj.o"]),
            make_test_job("link", &["obj.o"], &["binary"]),
        ];
        JobGraph::build(jobs).unwrap()
    }

    #[test]
    fn find_target_job_by_id() {
        let graph = test_graph();
        let id = find_target_job(&graph, "compile").unwrap();
        assert_eq!(id.as_str(), "compile");
    }

    #[test]
    fn find_target_job_by_output() {
        let graph = test_graph();
        let id = find_target_job(&graph, "binary").unwrap();
        assert_eq!(id.as_str(), "link");
    }

    #[test]
    fn find_target_job_missing() {
        let graph = test_graph();
        assert!(find_target_job(&graph, "nonexistent").is_err());
    }

    #[test]
    fn build_chain_no_panic() {
        let graph = test_graph();
        let chain = build_chain(&graph, &JobId::from("link"));
        assert!(!chain.is_empty());
    }

    #[test]
    fn print_text_no_panic() {
        let graph = test_graph();
        let root = find_target_job(&graph, "binary").unwrap();
        let chain = build_chain(&graph, &root);
        // Should not panic — exercises the expect() paths.
        print_text("binary", &chain, &graph);
    }

    #[test]
    fn print_json_no_panic() {
        let graph = test_graph();
        let root = find_target_job(&graph, "binary").unwrap();
        let chain = build_chain(&graph, &root);
        // Should not panic — exercises the expect() paths.
        print_json("binary", &chain, &graph);
    }
}
