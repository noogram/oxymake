//! Implementation of the `ox test` command.
//!
//! Validates a workflow without executing it: parse check, semantic validation,
//! DAG resolution, dependency cycle detection, missing input detection, and
//! rule name uniqueness. Supports `--dry-run` for execution simulation.

use std::path::PathBuf;

use anyhow::Result;

use ox_core::dag::RuleGraph;
use ox_core::job_graph::JobGraph;
use ox_core::resolver;

use super::common;

#[derive(clap::Args)]
pub struct TestArgs {
    /// Target files or patterns to test
    pub targets: Vec<String>,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,

    /// Simulate execution order without running (dry-run)
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Output NDJSON
    #[arg(long)]
    pub json: bool,
}

/// A single test check with its result.
struct Check {
    name: &'static str,
    passed: bool,
    detail: Option<String>,
}

pub fn cmd_test(args: TestArgs) -> Result<()> {
    let file_path = PathBuf::from(&args.file);
    let mut checks: Vec<Check> = Vec::new();

    // ── Check 1: Parse ──────────────────────────────────────────────
    let workflow = match common::load_workflow(&file_path) {
        Ok(wf) => {
            checks.push(Check {
                name: "parse",
                passed: true,
                detail: None,
            });
            wf
        }
        Err(e) => {
            checks.push(Check {
                name: "parse",
                passed: false,
                detail: Some(format!("{e:#}")),
            });
            return print_results(&checks, args.json);
        }
    };

    let rule_count = workflow.rules.len();

    // ── Check 2: Semantic validation (includes rule name uniqueness) ─
    let validation_ok = match ox_format::validate::validate(&workflow) {
        Ok(()) => {
            checks.push(Check {
                name: "validate",
                passed: true,
                detail: None,
            });
            true
        }
        Err(errs) => {
            let messages: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
            checks.push(Check {
                name: "validate",
                passed: false,
                detail: Some(messages.join("; ")),
            });
            false
        }
    };

    // ── Check 3: Rule name uniqueness (explicit, even though validate covers it) ─
    {
        let mut seen = std::collections::HashSet::new();
        let mut dupes = Vec::new();
        for rule in &workflow.rules {
            let name = rule.name.as_str();
            if !seen.insert(name.to_string()) {
                dupes.push(name.to_string());
            }
        }
        checks.push(Check {
            name: "unique-names",
            passed: dupes.is_empty(),
            detail: if dupes.is_empty() {
                None
            } else {
                Some(format!("duplicate rule names: {}", dupes.join(", ")))
            },
        });
    }

    // ── Check 4: DAG construction & cycle detection ─────────────────
    let dag_ok = match RuleGraph::build(workflow.rules.clone()) {
        Ok(rule_graph) => {
            if rule_graph.is_acyclic() {
                checks.push(Check {
                    name: "dag-acyclic",
                    passed: true,
                    detail: None,
                });
                true
            } else {
                let cycle = rule_graph
                    .find_cycle()
                    .ok()
                    .flatten()
                    .map(|c| {
                        c.iter()
                            .map(|n| n.as_str().to_string())
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    })
                    .unwrap_or_else(|| "(unknown cycle)".into());
                checks.push(Check {
                    name: "dag-acyclic",
                    passed: false,
                    detail: Some(format!("cycle: {cycle}")),
                });
                false
            }
        }
        Err(e) => {
            checks.push(Check {
                name: "dag-acyclic",
                passed: false,
                detail: Some(e.to_string()),
            });
            false
        }
    };

    // ── Check 5: Target resolution & missing inputs ─────────────────
    if validation_ok && dag_ok {
        let config = common::workflow_config(&workflow);
        let targets = common::resolve_targets(&workflow, &args.targets);

        if targets.is_empty() {
            checks.push(Check {
                name: "resolve",
                passed: true,
                detail: Some("no targets to resolve".into()),
            });
        } else {
            let existing_files = common::discover_existing_files(&file_path);
            let request = resolver::ResolveRequest {
                targets: targets.clone(),
                config,
                existing_files,
            };

            match resolver::resolve(&workflow.rules, &request) {
                Ok(resolve_result) => {
                    checks.push(Check {
                        name: "resolve",
                        passed: true,
                        detail: Some(format!(
                            "{} jobs, {} sources",
                            resolve_result.jobs.len(),
                            resolve_result.sources.len()
                        )),
                    });

                    // ── Check 6: JobGraph construction ──────────────
                    match JobGraph::build(resolve_result.jobs) {
                        Ok(job_graph) => {
                            checks.push(Check {
                                name: "job-graph",
                                passed: true,
                                detail: None,
                            });

                            // ── Dry-run: show execution order ───────
                            if args.dry_run {
                                print_dry_run(&job_graph, &targets, rule_count)?;
                            }
                        }
                        Err(e) => {
                            checks.push(Check {
                                name: "job-graph",
                                passed: false,
                                detail: Some(format!("{e:#}")),
                            });
                        }
                    }
                }
                Err(e) => {
                    checks.push(Check {
                        name: "resolve",
                        passed: false,
                        detail: Some(format!("{e:#}")),
                    });
                }
            }
        }
    }

    print_results(&checks, args.json)
}

fn print_dry_run(job_graph: &JobGraph, targets: &[String], rule_count: usize) -> Result<()> {
    println!();
    println!("--- dry-run simulation ---");
    println!("Rules: {}, Targets: {}", rule_count, targets.join(", "));

    if let Ok(topo) = job_graph.topological_order() {
        println!("Execution order ({} jobs):", topo.len());
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
                    i + 1,
                    job_id.as_str(),
                    job.rule.as_str(),
                    outputs.join(", ")
                );
            }
        }
    }

    Ok(())
}

fn print_results(checks: &[Check], json: bool) -> Result<()> {
    let all_passed = checks.iter().all(|c| c.passed);

    if json {
        for check in checks {
            let status = if check.passed { "pass" } else { "fail" };
            let detail = check
                .detail
                .as_deref()
                .map(|d| format!(r#","detail":"{}""#, d.replace('"', r#"\""#)))
                .unwrap_or_default();
            println!(
                r#"{{"check":"{}","status":"{}"{}}}"#,
                check.name, status, detail
            );
        }
    } else {
        for check in checks {
            let icon = if check.passed { "PASS" } else { "FAIL" };
            match &check.detail {
                Some(detail) => println!("  [{icon}] {}: {detail}", check.name),
                None => println!("  [{icon}] {}", check.name),
            }
        }
        println!();
        if all_passed {
            println!("All checks passed.");
        } else {
            let failed: Vec<&str> = checks
                .iter()
                .filter(|c| !c.passed)
                .map(|c| c.name)
                .collect();
            println!("Failed checks: {}", failed.join(", "));
        }
    }

    if all_passed {
        Ok(())
    } else {
        anyhow::bail!("workflow test failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let oxfile = dir.path().join("Oxymakefile.toml");
        std::fs::write(
            &oxfile,
            r#"
[rule.hello]
input = []
output = ["hello.txt"]
shell = "echo hello > {output}"
"#,
        )
        .unwrap();

        let args = TestArgs {
            targets: vec![],
            file: oxfile.to_string_lossy().into_owned(),
            dry_run: false,
            json: false,
        };

        assert!(cmd_test(args).is_ok());
    }

    #[test]
    fn test_missing_file() {
        let args = TestArgs {
            targets: vec![],
            file: "/nonexistent/Oxymakefile.toml".into(),
            dry_run: false,
            json: false,
        };

        assert!(cmd_test(args).is_err());
    }

    #[test]
    fn test_json_output() {
        let dir = tempfile::tempdir().unwrap();
        let oxfile = dir.path().join("Oxymakefile.toml");
        std::fs::write(
            &oxfile,
            r#"
[rule.hello]
input = []
output = ["hello.txt"]
shell = "echo hello > {output}"
"#,
        )
        .unwrap();

        let args = TestArgs {
            targets: vec![],
            file: oxfile.to_string_lossy().into_owned(),
            dry_run: false,
            json: true,
        };

        assert!(cmd_test(args).is_ok());
    }
}
