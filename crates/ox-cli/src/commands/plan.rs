//! Implementation of the `ox plan` command.

use std::path::PathBuf;

use anyhow::Result;

use ox_core::dag::RuleGraph;
use ox_core::error::{DagError, WildcardError};
use ox_core::job_graph::JobGraph;
use ox_core::resolver;
use ox_plan::PlanError;

use super::common;

#[derive(clap::Args)]
pub struct PlanArgs {
    /// Target files or patterns to build
    pub targets: Vec<String>,

    /// Plan detail level: rules, jobs, or optimized (default)
    #[arg(long, default_value = "optimized")]
    pub level: String,

    /// Skip optimization passes
    #[arg(long)]
    pub no_optimize: bool,

    /// Output NDJSON
    #[arg(long)]
    pub json: bool,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,
}

pub fn cmd_plan(args: PlanArgs, theme: &ox_render::Theme) -> Result<()> {
    let file_path = PathBuf::from(&args.file);
    let workflow = common::load_workflow(&file_path)?;

    // Validate.
    ox_format::validate::validate(&workflow)
        .map_err(|errs| PlanError::validation(errs, &file_path))?;

    let rule_count = workflow.rules.len();

    // Build the RuleGraph.
    let _rule_graph = RuleGraph::build(workflow.rules.clone()).map_err(|e| {
        enrich_with_source_line(
            PlanError::rule_graph_build(e, &file_path),
            &workflow.rules,
            None,
        )
    })?;

    if args.level == "rules" {
        if args.json {
            let rules: Vec<serde_json::Value> = workflow
                .rules
                .iter()
                .map(|rule| {
                    let inputs: Vec<&str> =
                        rule.inputs.iter().map(|i| i.pattern.as_str()).collect();
                    let outputs: Vec<&str> =
                        rule.outputs.iter().map(|o| o.pattern.as_str()).collect();
                    serde_json::json!({
                        "name": rule.name.as_str(),
                        "inputs": inputs,
                        "outputs": outputs,
                    })
                })
                .collect();
            let json = serde_json::json!({
                "level": "rules",
                "rule_count": rule_count,
                "rules": rules,
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        } else {
            println!(
                "{} {} rules",
                theme.header.apply_to("RuleGraph:"),
                rule_count
            );
            for rule in &workflow.rules {
                let inputs: Vec<&str> = rule.inputs.iter().map(|i| i.pattern.as_str()).collect();
                let outputs: Vec<&str> = rule.outputs.iter().map(|o| o.pattern.as_str()).collect();
                println!(
                    "  [{}] inputs=[{}] outputs=[{}]",
                    theme.highlight.apply_to(rule.name.as_str()),
                    theme.muted.apply_to(inputs.join(", ")),
                    theme.muted.apply_to(outputs.join(", "))
                );
            }
        }
        return Ok(());
    }

    // Resolve targets to concrete jobs.
    let config = common::workflow_config(&workflow);
    let targets = common::resolve_targets(&workflow, &args.targets);

    if targets.is_empty() {
        if args.json {
            let json = serde_json::json!({
                "level": args.level,
                "rule_count": rule_count,
                "job_count": 0,
                "source_count": 0,
                "targets": [],
                "jobs": [],
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        } else {
            println!("Nothing to plan (no targets).");
        }
        return Ok(());
    }

    let existing_files = common::discover_existing_files(&file_path);

    let request = resolver::ResolveRequest {
        targets: targets.clone(),
        config,
        existing_files,
    };

    let resolve_result = resolver::resolve(&workflow.rules, &request).map_err(|e| {
        let missing = match &e {
            DagError::Wildcard(WildcardError::NoProducer { path }) => Some(path.clone()),
            _ => None,
        };
        enrich_with_source_line(
            PlanError::resolve_targets(e, &file_path),
            &workflow.rules,
            missing.as_deref(),
        )
    })?;

    let job_count = resolve_result.jobs.len();
    let source_count = resolve_result.sources.len();

    // Build the JobGraph.
    let job_graph = JobGraph::build(resolve_result.jobs).map_err(|e| {
        enrich_with_source_line(
            PlanError::job_graph_build(e, &file_path),
            &workflow.rules,
            None,
        )
    })?;

    if args.json {
        let jobs: Vec<serde_json::Value> = if let Ok(topo) = job_graph.topological_order() {
            topo.iter()
                .enumerate()
                .filter_map(|(i, job_id)| {
                    job_graph.get_job(job_id).map(|job| {
                        let outputs: Vec<String> = job
                            .outputs
                            .iter()
                            .map(|o| match &o.reference {
                                ox_core::model::OutputRef::File(p) => p.display().to_string(),
                                ox_core::model::OutputRef::Virtual { id, .. } => id.clone(),
                                ox_core::model::OutputRef::InMemory { type_hint } => {
                                    type_hint.clone().unwrap_or_else(|| "<memory>".into())
                                }
                            })
                            .collect();
                        let inputs: Vec<String> = job
                            .inputs
                            .iter()
                            .map(|inp| match &inp.reference {
                                ox_core::model::OutputRef::File(p) => p.display().to_string(),
                                ox_core::model::OutputRef::Virtual { id, .. } => id.clone(),
                                ox_core::model::OutputRef::InMemory { type_hint } => {
                                    type_hint.clone().unwrap_or_else(|| "<memory>".into())
                                }
                            })
                            .collect();
                        let depends_on: Vec<&str> = job_graph
                            .upstream(job_id)
                            .iter()
                            .map(|id| id.as_str())
                            .collect();
                        let tags: serde_json::Value = serde_json::to_value(&job.tags)
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        serde_json::json!({
                            "order": i + 1,
                            "job_id": job_id.as_str(),
                            "rule": job.rule.as_str(),
                            "status": "pending",
                            "inputs": inputs,
                            "outputs": outputs,
                            "depends_on": depends_on,
                            "tags": tags,
                        })
                    })
                })
                .collect()
        } else {
            vec![]
        };
        let json = serde_json::json!({
            "level": args.level,
            "rule_count": rule_count,
            "job_count": job_count,
            "source_count": source_count,
            "targets": targets,
            "jobs": jobs,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!(
            "{} {} rules, {} jobs, {} source files",
            theme.header.apply_to("Plan:"),
            rule_count,
            job_count,
            source_count
        );
        println!(
            "{} {}",
            theme.header.apply_to("Targets:"),
            theme.highlight.apply_to(targets.join(", "))
        );

        if let Ok(topo) = job_graph.topological_order() {
            for (i, job_id) in topo.iter().enumerate() {
                if let Some(job) = job_graph.get_job(job_id) {
                    let outputs: Vec<String> = job
                        .outputs
                        .iter()
                        .map(|o| match &o.reference {
                            ox_core::model::OutputRef::File(p) => p.display().to_string(),
                            ox_core::model::OutputRef::Virtual { id, .. } => id.clone(),
                            ox_core::model::OutputRef::InMemory { type_hint } => {
                                type_hint.clone().unwrap_or_else(|| "<memory>".into())
                            }
                        })
                        .collect();
                    println!(
                        "  {}. [{}] rule={} -> [{}]",
                        theme.muted.apply_to(i + 1),
                        theme.highlight.apply_to(job_id.as_str()),
                        theme.info.apply_to(job.rule.as_str()),
                        theme.muted.apply_to(outputs.join(", "))
                    );
                }
            }
        }
    }

    Ok(())
}

/// Inspect a `DagError` and attach `source_snakefile_line` to the
/// `PlanError` envelope when the failing rule can be identified.
///
/// We do best-effort name extraction from the underlying error:
///
/// - `CycleDetected { cycle }`             — first named rule in the cycle
/// - `MissingSource { rule, .. }`          — the named rule
/// - `ConflictingOutputs { a, b, .. }`     — the first of the pair
///
/// `NoProducer` does not name a rule by construction (the whole point is
/// that no rule was found), so we only get the rule line when the cycle /
/// missing-source / conflict variants fire.  The `missing_path` path
/// argument lets the surrounding code carry a separate hint to PlanError
/// via the dropped-rule lookup in `.escalations.toml`.
fn enrich_with_source_line(
    mut err: PlanError,
    rules: &[ox_core::model::Rule],
    _missing_path: Option<&str>,
) -> PlanError {
    let rule_name: Option<&str> = match &err {
        PlanError::RuleGraphBuild { source, .. }
        | PlanError::ResolveTargets { source, .. }
        | PlanError::JobGraphBuild { source, .. } => match source {
            DagError::CycleDetected { cycle } => cycle.first().map(String::as_str),
            DagError::MissingSource { rule, .. } => Some(rule.as_str()),
            DagError::ConflictingOutputs { a, .. } => Some(a.as_str()),
            DagError::DuplicateJobId { a, .. } => Some(a.as_str()),
            DagError::Wildcard(_)
            | DagError::CorruptedGraph { .. }
            | DagError::DependencyChainTooDeep { .. } => None,
        },
        PlanError::Validation { .. } => None,
    };
    let line = rule_name.and_then(|name| {
        rules
            .iter()
            .find(|r| r.name.as_str() == name)
            .and_then(|r| r.source_line)
    });
    match &mut err {
        PlanError::Validation {
            source_snakefile_line,
            ..
        }
        | PlanError::RuleGraphBuild {
            source_snakefile_line,
            ..
        }
        | PlanError::ResolveTargets {
            source_snakefile_line,
            ..
        }
        | PlanError::JobGraphBuild {
            source_snakefile_line,
            ..
        } => {
            if line.is_some() {
                *source_snakefile_line = line;
            }
        }
    }
    err
}
