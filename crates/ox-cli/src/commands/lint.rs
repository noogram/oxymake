//! Implementation of the `ox lint` command.

use std::path::PathBuf;

use anyhow::Result;

use super::common;
use ox_core::dag::RuleGraph;

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

    // Non-fatal diagnostics. Currently none are produced: the gate
    // enforcement stopgap warning was removed when gates started to block
    // (issue #2); an unknown rule in a gate is now a validation *error*.
    let warnings: Vec<String> = Vec::new();

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
