//! Implementation of `ox dag` — visualize the workflow DAG.

use std::path::PathBuf;

use anyhow::Result;
use ox_core::dag::RuleGraph;

use super::common;

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct DagArgs {
    /// Group by field(s), comma-separated
    #[arg(long)]
    pub group_by: Option<String>,

    /// Output format: text, dot, mermaid
    #[arg(long, default_value = "text")]
    pub format: String,

    /// Output structured JSON instead of human-readable text
    #[arg(long)]
    pub json: bool,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

pub fn cmd_dag(args: DagArgs, _theme: &ox_render::Theme) -> Result<()> {
    let file_path = PathBuf::from(&args.file);
    let workflow = common::load_workflow(&file_path)?;
    let rule_graph = RuleGraph::build(workflow.rules)?;

    if args.json {
        print_json(&rule_graph)?;
    } else {
        match args.format.as_str() {
            "dot" | "text" => print_dot(&rule_graph)?,
            "mermaid" => print_mermaid(&rule_graph)?,
            other => anyhow::bail!("unsupported format: {other} (expected: text, dot, mermaid)"),
        }
    }

    Ok(())
}

/// Render the DAG as structured JSON.
fn print_json(rule_graph: &RuleGraph) -> Result<()> {
    let mut rules = Vec::new();
    for rule_name in rule_graph.rule_names()? {
        let rule = rule_graph
            .get_rule(rule_name)?
            .expect("BUG: rule_names() returned a name not present in the graph");
        let inputs: Vec<&str> = rule.inputs.iter().map(|i| i.pattern.as_str()).collect();
        let outputs: Vec<&str> = rule.outputs.iter().map(|o| o.pattern.as_str()).collect();
        let upstream: Vec<&str> = rule_graph
            .upstream(rule_name)?
            .iter()
            .map(|n| n.as_str())
            .collect();
        let downstream: Vec<&str> = rule_graph
            .downstream(rule_name)?
            .iter()
            .map(|n| n.as_str())
            .collect();
        rules.push(serde_json::json!({
            "name": rule_name.as_str(),
            "inputs": inputs,
            "outputs": outputs,
            "upstream": upstream,
            "downstream": downstream,
        }));
    }

    let output = serde_json::json!({
        "rules": rules,
        "acyclic": rule_graph.is_acyclic(),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

/// Render the DAG in Graphviz DOT format.
fn print_dot(rule_graph: &RuleGraph) -> Result<()> {
    println!("digraph oxymake {{");
    println!("  rankdir=LR;");
    for rule_name in rule_graph.rule_names()? {
        let rule = rule_graph
            .get_rule(rule_name)?
            .expect("BUG: rule_names() returned a name not present in the graph");
        for output in &rule.outputs {
            println!("  \"{}\" -> \"{}\";", rule_name.as_str(), output.pattern);
        }
        for input in &rule.inputs {
            println!("  \"{}\" -> \"{}\";", input.pattern, rule_name.as_str());
        }
    }
    println!("}}");
    Ok(())
}

/// Render the DAG in Mermaid format.
fn print_mermaid(rule_graph: &RuleGraph) -> Result<()> {
    println!("graph LR");
    for rule_name in rule_graph.rule_names()? {
        let rule = rule_graph
            .get_rule(rule_name)?
            .expect("BUG: rule_names() returned a name not present in the graph");
        for output in &rule.outputs {
            println!(
                "  {} --> {}",
                sanitize_mermaid(rule_name.as_str()),
                sanitize_mermaid(&output.pattern)
            );
        }
        for input in &rule.inputs {
            println!(
                "  {} --> {}",
                sanitize_mermaid(&input.pattern),
                sanitize_mermaid(rule_name.as_str())
            );
        }
    }
    Ok(())
}

/// Sanitize an identifier for Mermaid (replace problematic characters).
fn sanitize_mermaid(s: &str) -> String {
    // Mermaid node IDs cannot contain braces, slashes, or dots easily.
    // Wrap in quotes to handle special characters.
    format!("\"{}\"", s.replace('"', "'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::dag::RuleGraph;
    use ox_core::model::*;
    use std::collections::BTreeMap;

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

    fn test_graph() -> RuleGraph {
        let rules = vec![
            make_rule("compile", &[], &["obj.o"]),
            make_rule("link", &["obj.o"], &["binary"]),
        ];
        RuleGraph::build(rules).unwrap()
    }

    #[test]
    fn print_json_no_panic() {
        let graph = test_graph();
        // Should not panic — exercises the expect() paths.
        print_json(&graph).unwrap();
    }

    #[test]
    fn print_dot_no_panic() {
        let graph = test_graph();
        // Should not panic — exercises the expect() paths.
        print_dot(&graph).unwrap();
    }

    #[test]
    fn print_mermaid_no_panic() {
        let graph = test_graph();
        // Should not panic — exercises the expect() paths.
        print_mermaid(&graph).unwrap();
    }
}
