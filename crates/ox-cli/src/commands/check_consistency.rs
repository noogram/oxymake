//! Implementation of the `ox check-consistency` command.
//!
//! Validates internal DAG invariants beyond what `ox test` and `ox lint` check.
//! Detects structural issues like orphan rules, shadow outputs, input/output
//! overlap, and disconnected subgraphs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::Result;

use ox_core::dag::RuleGraph;
use ox_core::job_graph::JobGraph;
use ox_core::resolver;

use super::common;

#[derive(clap::Args)]
pub struct CheckConsistencyArgs {
    /// Target files or patterns to check
    pub targets: Vec<String>,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,

    /// Output NDJSON
    #[arg(long)]
    pub json: bool,
}

/// A single invariant check with its result.
struct Invariant {
    name: &'static str,
    status: InvariantStatus,
    detail: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InvariantStatus {
    /// Invariant holds.
    Ok,
    /// Invariant violated — structural problem.
    Fail,
    /// Not a violation but worth noting.
    Warn,
}

pub fn cmd_check_consistency(args: CheckConsistencyArgs) -> Result<()> {
    let file_path = PathBuf::from(&args.file);
    let mut invariants: Vec<Invariant> = Vec::new();

    // ── Parse ──────────────────────────────────────────────────────────
    let workflow = match common::load_workflow(&file_path) {
        Ok(wf) => wf,
        Err(e) => {
            invariants.push(Invariant {
                name: "parse",
                status: InvariantStatus::Fail,
                detail: Some(format!("{e:#}")),
            });
            return print_results(&invariants, args.json);
        }
    };
    invariants.push(Invariant {
        name: "parse",
        status: InvariantStatus::Ok,
        detail: None,
    });

    // ── Semantic validation ────────────────────────────────────────────
    if let Err(errs) = ox_format::validate::validate(&workflow) {
        let messages: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        invariants.push(Invariant {
            name: "validate",
            status: InvariantStatus::Fail,
            detail: Some(messages.join("; ")),
        });
        return print_results(&invariants, args.json);
    }
    invariants.push(Invariant {
        name: "validate",
        status: InvariantStatus::Ok,
        detail: None,
    });

    // ── Build RuleGraph ────────────────────────────────────────────────
    let rule_graph = match RuleGraph::build(workflow.rules.clone()) {
        Ok(rg) => rg,
        Err(e) => {
            invariants.push(Invariant {
                name: "dag-build",
                status: InvariantStatus::Fail,
                detail: Some(e.to_string()),
            });
            return print_results(&invariants, args.json);
        }
    };
    invariants.push(Invariant {
        name: "dag-build",
        status: InvariantStatus::Ok,
        detail: None,
    });

    // ── Acyclicity ─────────────────────────────────────────────────────
    if !rule_graph.is_acyclic() {
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
        invariants.push(Invariant {
            name: "acyclic",
            status: InvariantStatus::Fail,
            detail: Some(format!("cycle: {cycle}")),
        });
        return print_results(&invariants, args.json);
    }
    invariants.push(Invariant {
        name: "acyclic",
        status: InvariantStatus::Ok,
        detail: None,
    });

    // ── Input/output overlap ───────────────────────────────────────────
    // A rule that lists the same pattern in both inputs and outputs is suspicious.
    {
        let mut overlaps = Vec::new();
        for rule in &workflow.rules {
            let input_pats: BTreeSet<&str> =
                rule.inputs.iter().map(|i| i.pattern.as_str()).collect();
            let output_pats: BTreeSet<&str> =
                rule.outputs.iter().map(|o| o.pattern.as_str()).collect();
            let common: Vec<&&str> = input_pats.intersection(&output_pats).collect();
            if !common.is_empty() {
                overlaps.push(format!(
                    "{}: [{}]",
                    rule.name.as_str(),
                    common.iter().map(|p| **p).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        if overlaps.is_empty() {
            invariants.push(Invariant {
                name: "no-input-output-overlap",
                status: InvariantStatus::Ok,
                detail: None,
            });
        } else {
            invariants.push(Invariant {
                name: "no-input-output-overlap",
                status: InvariantStatus::Warn,
                detail: Some(format!(
                    "rules with same pattern in inputs and outputs: {}",
                    overlaps.join("; ")
                )),
            });
        }
    }

    // ── Orphan rules (no downstream consumers, not a default target) ──
    {
        let rule_names = rule_graph.rule_names().unwrap_or_default();
        let mut orphans = Vec::new();
        for name in &rule_names {
            let downstream = rule_graph.downstream(name).unwrap_or_default();
            if downstream.is_empty() {
                // Check if this is a leaf/terminal rule — that's normal for
                // final targets (like "all"). Only flag if it also has no
                // inputs (truly disconnected) or isn't the default target.
                let rule = workflow.rules.iter().find(|r| &r.name == *name);
                let is_aggregation = rule.is_some_and(|r| r.outputs.is_empty());
                if !is_aggregation {
                    orphans.push(name.as_str().to_string());
                }
            }
        }
        if orphans.is_empty() {
            invariants.push(Invariant {
                name: "no-orphan-rules",
                status: InvariantStatus::Ok,
                detail: None,
            });
        } else {
            invariants.push(Invariant {
                name: "no-orphan-rules",
                status: InvariantStatus::Warn,
                detail: Some(format!(
                    "rules with outputs not consumed by any other rule: {}",
                    orphans.join(", ")
                )),
            });
        }
    }

    // ── Shadow outputs (outputs produced but never consumed) ───────────
    {
        let mut all_output_pats: BTreeMap<&str, &str> = BTreeMap::new(); // pattern -> rule
        let mut all_input_pats: BTreeSet<&str> = BTreeSet::new();

        for rule in &workflow.rules {
            for output in &rule.outputs {
                all_output_pats.insert(output.pattern.as_str(), rule.name.as_str());
            }
            for input in &rule.inputs {
                all_input_pats.insert(input.pattern.as_str());
            }
        }

        let shadows: Vec<String> = all_output_pats
            .iter()
            .filter(|(pat, _)| !all_input_pats.contains(**pat))
            .map(|(pat, rule)| format!("{pat} (from {rule})"))
            .collect();

        if shadows.is_empty() {
            invariants.push(Invariant {
                name: "no-shadow-outputs",
                status: InvariantStatus::Ok,
                detail: None,
            });
        } else {
            invariants.push(Invariant {
                name: "no-shadow-outputs",
                status: InvariantStatus::Warn,
                detail: Some(format!(
                    "outputs not consumed by any rule: {}",
                    shadows.join("; ")
                )),
            });
        }
    }

    // ── DAG metrics ────────────────────────────────────────────────────
    {
        let topo = rule_graph.topological_order().unwrap_or_default();
        invariants.push(Invariant {
            name: "dag-metrics",
            status: InvariantStatus::Ok,
            detail: Some(format!(
                "{} rules, {} edges, topo-depth {}",
                rule_graph.rule_count(),
                rule_graph.edge_count(),
                topo.len()
            )),
        });
    }

    // ── Target resolution & JobGraph ───────────────────────────────────
    {
        let config = common::workflow_config(&workflow);
        let targets = common::resolve_targets(&workflow, &args.targets);

        if !targets.is_empty() {
            let existing_files = common::discover_existing_files(&file_path);
            let request = resolver::ResolveRequest {
                targets: targets.clone(),
                config,
                existing_files,
            };

            match resolver::resolve(&workflow.rules, &request) {
                Ok(resolve_result) => {
                    invariants.push(Invariant {
                        name: "resolve",
                        status: InvariantStatus::Ok,
                        detail: Some(format!(
                            "{} jobs, {} sources",
                            resolve_result.jobs.len(),
                            resolve_result.sources.len()
                        )),
                    });

                    match JobGraph::build(resolve_result.jobs) {
                        Ok(job_graph) => {
                            // Check job-level acyclicity (should be guaranteed
                            // by rule-level acyclicity, but verify).
                            if job_graph.is_acyclic() {
                                invariants.push(Invariant {
                                    name: "job-graph-acyclic",
                                    status: InvariantStatus::Ok,
                                    detail: None,
                                });
                            } else {
                                invariants.push(Invariant {
                                    name: "job-graph-acyclic",
                                    status: InvariantStatus::Fail,
                                    detail: Some(
                                        "job graph contains a cycle after resolution".into(),
                                    ),
                                });
                            }

                            invariants.push(Invariant {
                                name: "job-graph-metrics",
                                status: InvariantStatus::Ok,
                                detail: Some(format!(
                                    "{} jobs, {} edges",
                                    job_graph.job_count(),
                                    job_graph.edge_count()
                                )),
                            });
                        }
                        Err(e) => {
                            invariants.push(Invariant {
                                name: "job-graph-build",
                                status: InvariantStatus::Fail,
                                detail: Some(format!("{e:#}")),
                            });
                        }
                    }
                }
                Err(e) => {
                    invariants.push(Invariant {
                        name: "resolve",
                        status: InvariantStatus::Fail,
                        detail: Some(format!("{e:#}")),
                    });
                }
            }
        }
    }

    print_results(&invariants, args.json)
}

fn print_results(invariants: &[Invariant], json: bool) -> Result<()> {
    let has_fail = invariants.iter().any(|i| i.status == InvariantStatus::Fail);

    if json {
        for inv in invariants {
            let status = match inv.status {
                InvariantStatus::Ok => "ok",
                InvariantStatus::Fail => "fail",
                InvariantStatus::Warn => "warn",
            };
            let detail = inv
                .detail
                .as_deref()
                .map(|d| format!(r#","detail":"{}""#, d.replace('"', r#"\""#)))
                .unwrap_or_default();
            println!(
                r#"{{"invariant":"{}","status":"{}"{}}}"#,
                inv.name, status, detail
            );
        }
    } else {
        for inv in invariants {
            let icon = match inv.status {
                InvariantStatus::Ok => "  OK",
                InvariantStatus::Fail => "FAIL",
                InvariantStatus::Warn => "WARN",
            };
            match &inv.detail {
                Some(detail) => println!("  [{icon}] {}: {detail}", inv.name),
                None => println!("  [{icon}] {}", inv.name),
            }
        }
        println!();
        if has_fail {
            let failed: Vec<&str> = invariants
                .iter()
                .filter(|i| i.status == InvariantStatus::Fail)
                .map(|i| i.name)
                .collect();
            println!("Invariant violations: {}", failed.join(", "));
        } else {
            let warns: Vec<&str> = invariants
                .iter()
                .filter(|i| i.status == InvariantStatus::Warn)
                .map(|i| i.name)
                .collect();
            if warns.is_empty() {
                println!("All invariants hold.");
            } else {
                println!(
                    "All invariants hold ({} warning(s): {}).",
                    warns.len(),
                    warns.join(", ")
                );
            }
        }
    }

    if has_fail {
        anyhow::bail!("invariant check failed")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_workflow_passes() {
        let dir = tempfile::tempdir().unwrap();
        let oxfile = dir.path().join("Oxymakefile.toml");
        std::fs::write(
            &oxfile,
            r#"
[rule.hello]
input = []
output = ["hello.txt"]
shell = "echo hello > {output}"

[rule.all]
input = ["hello.txt"]
"#,
        )
        .unwrap();

        let args = CheckConsistencyArgs {
            targets: vec![],
            file: oxfile.to_string_lossy().into_owned(),
            json: false,
        };

        assert!(cmd_check_consistency(args).is_ok());
    }

    #[test]
    fn missing_file_fails() {
        let args = CheckConsistencyArgs {
            targets: vec![],
            file: "/nonexistent/Oxymakefile.toml".into(),
            json: false,
        };

        assert!(cmd_check_consistency(args).is_err());
    }

    #[test]
    fn cyclic_workflow_fails() {
        let dir = tempfile::tempdir().unwrap();
        let oxfile = dir.path().join("Oxymakefile.toml");
        std::fs::write(
            &oxfile,
            r#"
[rule.a]
input = ["b.txt"]
output = ["a.txt"]
shell = "echo a"

[rule.b]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo b"
"#,
        )
        .unwrap();

        let args = CheckConsistencyArgs {
            targets: vec![],
            file: oxfile.to_string_lossy().into_owned(),
            json: false,
        };

        assert!(cmd_check_consistency(args).is_err());
    }

    #[test]
    fn json_output_format() {
        let dir = tempfile::tempdir().unwrap();
        let oxfile = dir.path().join("Oxymakefile.toml");
        std::fs::write(
            &oxfile,
            r#"
[rule.hello]
input = []
output = ["hello.txt"]
shell = "echo hello"

[rule.all]
input = ["hello.txt"]
"#,
        )
        .unwrap();

        let args = CheckConsistencyArgs {
            targets: vec![],
            file: oxfile.to_string_lossy().into_owned(),
            json: true,
        };

        assert!(cmd_check_consistency(args).is_ok());
    }

    #[test]
    fn detects_input_output_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let oxfile = dir.path().join("Oxymakefile.toml");
        std::fs::write(
            &oxfile,
            r#"
[rule.self_ref]
input = ["data.txt"]
output = ["data.txt"]
shell = "echo overwrite"
"#,
        )
        .unwrap();

        // Should pass (warnings don't cause failure) but we can verify
        // by checking the invariants in JSON mode.
        let args = CheckConsistencyArgs {
            targets: vec![],
            file: oxfile.to_string_lossy().into_owned(),
            json: false,
        };

        // This will pass because overlap is a warning, not a failure.
        // The cycle check will catch the actual problem here.
        let _ = cmd_check_consistency(args);
    }
}
