//! Implementation of the `ox lint` command.

use std::path::PathBuf;

use anyhow::Result;

use ox_core::dag::RuleGraph;
use ox_format::parse::Workflow;

use super::common;

/// Warn about `[gate.*]` sections whose enforcement is not wired into the
/// run path (issue #2): `ox gate list/approve/reject` and the parser accept
/// gates, but the scheduler's only production caller passes `None` for the
/// `GateCheck`, so guarded rules run unconditionally without approval.
///
/// Self-contained by design: this is a lint-only stopgap. It should be
/// deleted once gate enforcement is actually wired into the scheduler run
/// path, at which point these warnings become misleading rather than
/// informative.
fn gate_enforcement_warnings(workflow: &Workflow) -> Vec<String> {
    workflow
        .gates
        .iter()
        .filter(|gate| !gate.before.is_empty())
        .map(|gate| {
            format!(
                "gate `{}` guards rule(s) `{}`, but gate enforcement is not wired in this version: guarded rules run without approval (see issue #2)",
                gate.name,
                gate.before.join(", ")
            )
        })
        .collect()
}

#[derive(clap::Args)]
pub struct LintArgs {
    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,

    /// Output NDJSON
    #[arg(long)]
    pub json: bool,
}

pub fn cmd_lint(args: LintArgs) -> Result<()> {
    let file_path = PathBuf::from(&args.file);

    // Load and parse the workflow, capturing errors for JSON mode.
    let workflow = match common::load_workflow(&file_path) {
        Ok(wf) => wf,
        Err(e) => {
            if args.json {
                let json = serde_json::json!({
                    "file": file_path.display().to_string(),
                    "valid": false,
                    "rule_count": 0,
                    "errors": [format!("{e:#}")],
                });
                println!("{}", serde_json::to_string_pretty(&json)?);
                std::process::exit(1);
            } else {
                return Err(e);
            }
        }
    };

    let warnings = gate_enforcement_warnings(&workflow);

    // Run semantic validation.
    let validation = ox_format::validate::validate(&workflow);

    // Also try building the RuleGraph for structural checks.
    let dag_result = RuleGraph::build(workflow.rules.clone());

    let mut errors: Vec<String> = Vec::new();

    if let Err(errs) = validation {
        for e in errs {
            errors.push(e.to_string());
        }
    }

    if let Err(e) = dag_result {
        errors.push(e.to_string());
    }

    if args.json {
        let json = serde_json::json!({
            "file": file_path.display().to_string(),
            "valid": errors.is_empty(),
            "rule_count": workflow.rules.len(),
            "errors": errors,
            "warnings": warnings,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
        if !errors.is_empty() {
            std::process::exit(1);
        }
        Ok(())
    } else if errors.is_empty() {
        println!("Oxymakefile is valid ({} rules)", workflow.rules.len());
        for warning in &warnings {
            println!("warning: {warning}");
        }
        Ok(())
    } else {
        for err in &errors {
            eprintln!("error: {err}");
        }
        for warning in &warnings {
            eprintln!("warning: {warning}");
        }
        anyhow::bail!(
            "{} validation error(s) found in {}",
            errors.len(),
            file_path.display()
        )
    }
}
