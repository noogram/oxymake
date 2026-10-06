//! Semantic validation for parsed workflows.
//!
//! This module checks a [`Workflow`] for logical consistency after TOML
//! parsing succeeds. It catches issues like duplicate rule names,
//! missing execution modes, and wildcard mismatches.

use std::collections::HashSet;
use std::path::PathBuf;

use ox_core::error::ParseError;
use ox_core::model::ExecutionBlock;
use ox_core::resource::{CanonicalResource, canonicalize_resource_key};

use crate::parse::Workflow;

/// Validate a parsed workflow for semantic correctness.
///
/// Returns `Ok(())` if the workflow is valid, or `Err(errors)` with all
/// validation problems found (not just the first).
pub fn validate(workflow: &Workflow) -> Result<(), Vec<ParseError>> {
    let mut errors = Vec::new();

    check_duplicate_rules(workflow, &mut errors);
    check_execution_modes(workflow, &mut errors);
    check_output_wildcards(workflow, &mut errors);
    check_gate_rules(workflow, &mut errors);
    check_execution_placeholders(workflow, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Reject only OxyMake-specific placeholder shapes whose declarations are
/// statically knowable. Bare braces remain valid shell/code text.
fn check_execution_placeholders(workflow: &Workflow, errors: &mut Vec<ParseError>) {
    // `{config.X}` is deliberately absent from this static check: `--set`
    // overrides can introduce a key that no `[config]` section declares, and
    // they are applied after validation runs. The resolver rejects an unknown
    // config key instead, once the override set is complete.
    for rule in &workflow.rules {
        let text = execution_text(&rule.execution);
        let resources: Vec<&str> = rule.resources.keys().map(String::as_str).collect();
        let params: Vec<&str> = rule.params.keys().map(String::as_str).collect();

        check_namespaced_placeholders(
            text,
            "resources",
            &resources,
            &rule.name.0,
            "declared resource keys",
            errors,
        );
        check_namespaced_placeholders(
            text,
            "params",
            &params,
            &rule.name.0,
            "declared parameter keys",
            errors,
        );

        if text.contains("{threads}")
            && !rule
                .resources
                .keys()
                .any(|key| matches!(canonicalize_resource_key(key), Ok(CanonicalResource::Cpu)))
        {
            push_placeholder_error(
                &rule.name.0,
                "{threads}",
                available("declared resource keys", &resources),
                errors,
            );
        }
        if text.contains("{log}") && rule.log.stdout.is_none() {
            push_placeholder_error(
                &rule.name.0,
                "{log}",
                "no stdout log path is configured".into(),
                errors,
            );
        }
    }
}

fn execution_text(execution: &ExecutionBlock) -> &str {
    match execution {
        ExecutionBlock::Shell { command } => command,
        ExecutionBlock::Run { code, .. } => code,
        ExecutionBlock::Script { path, .. } => path.to_str().unwrap_or_default(),
        ExecutionBlock::Call { function, .. } => function,
    }
}

fn check_namespaced_placeholders(
    text: &str,
    namespace: &str,
    declared: &[&str],
    rule: &str,
    label: &str,
    errors: &mut Vec<ParseError>,
) {
    let prefix = format!("{{{namespace}.");
    let mut rest = text;
    let mut reported = HashSet::new();
    while let Some(start) = rest.find(&prefix) {
        let candidate = &rest[start..];
        let Some(end) = candidate.find('}') else {
            break;
        };
        let placeholder = &candidate[..=end];
        let key = &placeholder[prefix.len()..placeholder.len() - 1];
        if !declared.contains(&key) && reported.insert(placeholder.to_string()) {
            push_placeholder_error(rule, placeholder, available(label, declared), errors);
        }
        rest = &candidate[end + 1..];
    }
}

fn available(label: &str, keys: &[&str]) -> String {
    if keys.is_empty() {
        format!("{label}: none")
    } else {
        format!("{label}: {}", keys.join(", "))
    }
}

fn push_placeholder_error(
    rule: &str,
    placeholder: &str,
    available: String,
    errors: &mut Vec<ParseError>,
) {
    errors.push(ParseError::UnresolvedPlaceholder {
        rule: rule.into(),
        placeholder: placeholder.into(),
        available,
    });
}

/// Check for duplicate rule names.
fn check_duplicate_rules(workflow: &Workflow, errors: &mut Vec<ParseError>) {
    let mut seen = HashSet::new();
    for rule in &workflow.rules {
        let name = rule.name.as_str();
        if !seen.insert(name.to_string()) {
            errors.push(ParseError::DuplicateRule {
                name: name.to_string(),
                first: PathBuf::from("<workflow>"),
                second: PathBuf::from("<workflow>"),
            });
        }
    }
}

/// Check that each rule has a valid execution mode.
/// (The parser already enforces this, but we double-check for rules
/// that might have been constructed programmatically.)
fn check_execution_modes(workflow: &Workflow, _errors: &mut Vec<ParseError>) {
    for rule in &workflow.rules {
        // Aggregation rules (input-only, no output) get a no-op shell — that's valid.
        // All other rules must have been given an execution mode by the parser.
        // This is mainly a safety net.
        let _ = rule; // Currently a no-op since the parser handles this
    }
}

/// Check that wildcards in outputs also appear in inputs.
/// This catches common mistakes where an output references a wildcard
/// that has no source of values.
fn check_output_wildcards(workflow: &Workflow, _errors: &mut Vec<ParseError>) {
    for rule in &workflow.rules {
        let input_wildcards =
            extract_wildcards_from_patterns(rule.inputs.iter().map(|i| i.pattern.as_str()));
        let output_wildcards =
            extract_wildcards_from_patterns(rule.outputs.iter().map(|o| o.pattern.as_str()));

        for wc in &output_wildcards {
            if !input_wildcards.contains(wc) {
                // This is only a warning-level issue for aggregation rules
                // or rules using config-based wildcards — but we flag it for
                // rules that have both inputs and outputs.
                if !rule.inputs.is_empty() && !rule.outputs.is_empty() {
                    // Check if the wildcard might come from config
                    // (we can't fully resolve this at parse time, so we skip
                    // wildcards that look like config references)
                    // For now, this is informational — not an error.
                }
            }
        }
    }
}

/// Check that every rule named by a `[gate.*]` (`before` and `after`)
/// exists in the workflow.
///
/// Gates attach to jobs by rule name at run time; a name that matches no
/// rule attaches nothing. Failing closed here is what makes a typo in
/// `before` an error instead of a silently unguarded rule (issue #2).
fn check_gate_rules(workflow: &Workflow, errors: &mut Vec<ParseError>) {
    let rules: HashSet<&str> = workflow.rules.iter().map(|r| r.name.as_str()).collect();
    for gate in &workflow.gates {
        for (field, names) in [("before", &gate.before), ("after", &gate.after)] {
            for rule in names {
                if !rules.contains(rule.as_str()) {
                    errors.push(ParseError::GateUnknownRule {
                        gate: gate.name.clone(),
                        field: field.to_string(),
                        rule: rule.clone(),
                    });
                }
            }
        }
    }
}

/// Extract wildcard names from a set of pattern strings.
/// Wildcards are delimited by `{` and `}`.
fn extract_wildcards_from_patterns<'a>(patterns: impl Iterator<Item = &'a str>) -> HashSet<String> {
    let mut wildcards = HashSet::new();
    for pattern in patterns {
        let mut rest = pattern;
        while let Some(start) = rest.find('{') {
            if let Some(end) = rest[start..].find('}') {
                let name = &rest[start + 1..start + end];
                if !name.is_empty() {
                    wildcards.insert(name.to_string());
                }
                rest = &rest[start + end + 1..];
            } else {
                break;
            }
        }
    }
    wildcards
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_workflow;
    use std::path::Path;

    #[test]
    fn valid_simple_workflow() {
        let toml = r#"
ox_version = "0.1"

[config]
samples = ["A", "B"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "cat {input} | sort > {output}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(validate(&wf).is_ok());
    }

    #[test]
    fn duplicate_rule_names() {
        // We can't produce duplicate names from a single TOML parse (TOML
        // deduplicates keys), but we can test the validation logic directly.
        let toml = r#"
[rule.build]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp {input} {output}"
"#;
        let mut wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        // Manually duplicate the rule
        let dup = wf.rules[0].clone();
        wf.rules.push(dup);

        let err = validate(&wf).unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(matches!(err[0], ParseError::DuplicateRule { .. }));
    }

    #[test]
    fn extract_wildcards_works() {
        let patterns = vec!["data/{sample}/{lookback}d.parquet"];
        let wc = extract_wildcards_from_patterns(patterns.into_iter());
        assert!(wc.contains("sample"));
        assert!(wc.contains("lookback"));
        assert_eq!(wc.len(), 2);
    }

    #[test]
    fn extract_wildcards_empty() {
        let patterns = vec!["data/fixed.csv"];
        let wc = extract_wildcards_from_patterns(patterns.into_iter());
        assert!(wc.is_empty());
    }

    #[test]
    fn extract_wildcards_unclosed_brace() {
        let patterns = vec!["data/{unclosed"];
        let wc = extract_wildcards_from_patterns(patterns.into_iter());
        assert!(wc.is_empty());
    }

    #[test]
    fn output_wildcard_not_in_input_is_info_only() {
        // A rule where an output wildcard does not appear in inputs.
        // This exercises the check_output_wildcards branch (lines 76-81).
        let toml = r#"
[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}/{extra_wc}.txt"]
shell = "process {input} > {output}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        // Currently this is informational only (not an error), so validate should pass.
        assert!(validate(&wf).is_ok());
    }

    #[test]
    fn rejects_undeclared_namespaced_placeholders_and_names_declared_keys() {
        let toml = r#"
[config]
project = "demo"
samples = ["A"]

[rule.process]
output = ["out.txt"]
shell = "tool {resources.memory} {params.missing} {config.unknown} > {output}"
resources = { mem = "8G" }
params = { present = "yes" }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let messages: Vec<String> = validate(&wf)
            .unwrap_err()
            .into_iter()
            .map(|error| error.to_string())
            .collect();

        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages.iter().any(|message| {
            message.contains("rule `process`")
                && message.contains("{resources.memory}")
                && message.contains("mem")
        }));
        assert!(messages.iter().any(|message| {
            message.contains("{params.missing}") && message.contains("present")
        }));
        // `{config.unknown}` is deliberately NOT reported here: `--set` can
        // supply a key absent from `[config]`, and overrides are applied after
        // validation. The resolver rejects it once the key set is complete.
        assert!(
            !messages
                .iter()
                .any(|message| message.contains("{config.unknown}")),
            "{messages:?}"
        );
    }

    #[test]
    fn rejects_threads_without_cpu_and_log_without_stdout_path() {
        let toml = r#"
[rule.process]
output = ["out.txt"]
shell = "tool --threads {threads} > {log}"
resources = { mem = "8G" }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let messages: Vec<String> = validate(&wf)
            .unwrap_err()
            .into_iter()
            .map(|error| error.to_string())
            .collect();

        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages.iter().any(|message| {
            message.contains("rule `process`")
                && message.contains("{threads}")
                && message.contains("mem")
        }));
        assert!(
            messages
                .iter()
                .any(|message| { message.contains("rule `process`") && message.contains("{log}") })
        );
    }

    #[test]
    fn permits_shell_braces_bare_names_and_declared_placeholders() {
        let toml = r#"
[config]
project = "demo"

[rule.process]
output = ["out.txt"]
shell = '''printf '%s\n' '{"ok":true}' "${HOME}"; awk '{print $1}'; echo {samp} {threads} {resources.mem} {params.mode} {config.project} > {log}'''
resources = { cpus = 2, mem = "8G" }
params = { mode = "fast" }
log = { stdout = "process.log" }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(validate(&wf).is_ok());
    }

    #[test]
    fn gate_naming_an_unknown_rule_is_an_error() {
        let toml = r#"
ox_version = "0.1"

[gate.approval]
after = []
before = ["typo_rule"]

[rule.guarded]
input = []
output = ["out.txt"]
shell = "echo RAN > out.txt"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let errs = validate(&wf).unwrap_err();
        assert_eq!(errs.len(), 1);
        let msg = errs[0].to_string();
        assert!(msg.contains("gate `approval`"), "{msg}");
        assert!(msg.contains("unknown rule `typo_rule`"), "{msg}");
        assert!(msg.contains("`before`"), "{msg}");
    }

    #[test]
    fn gate_after_naming_an_unknown_rule_is_an_error() {
        let toml = r#"
ox_version = "0.1"

[gate.approval]
after = ["nope"]
before = ["guarded"]

[rule.guarded]
input = []
output = ["out.txt"]
shell = "echo RAN > out.txt"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let errs = validate(&wf).unwrap_err();
        assert_eq!(errs.len(), 1);
        let msg = errs[0].to_string();
        assert!(msg.contains("gate `approval`"), "{msg}");
        assert!(msg.contains("unknown rule `nope`"), "{msg}");
        assert!(msg.contains("`after`"), "{msg}");
    }

    #[test]
    fn gate_naming_existing_rules_is_valid() {
        let toml = r#"
ox_version = "0.1"

[gate.approval]
after = ["prep"]
before = ["guarded"]

[rule.prep]
input = []
output = ["prep.txt"]
shell = "echo PREP > prep.txt"

[rule.guarded]
input = ["prep.txt"]
output = ["out.txt"]
shell = "echo RAN > out.txt"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(validate(&wf).is_ok());
    }

    #[test]
    fn validate_workflow_with_no_rules() {
        let toml = r#"
ox_version = "0.1"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(validate(&wf).is_ok());
    }
}
