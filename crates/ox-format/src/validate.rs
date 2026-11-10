//! Semantic validation for parsed workflows.
//!
//! This module checks a [`Workflow`] for logical consistency after TOML
//! parsing succeeds. It catches issues like duplicate rule names,
//! invalid gate references, and unresolved execution placeholders.

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
        let resources: Vec<&str> = rule.resources.keys().map(String::as_str).collect();
        let params: Vec<&str> = rule.params.keys().map(String::as_str).collect();

        check_placeholders_in_text(
            execution_text(&rule.execution),
            rule,
            &resources,
            &params,
            errors,
        );
        for value in rule.env.values() {
            check_placeholders_in_text(value, rule, &resources, &params, errors);
        }
    }
}

fn check_placeholders_in_text(
    text: &str,
    rule: &ox_core::model::Rule,
    resources: &[&str],
    params: &[&str],
    errors: &mut Vec<ParseError>,
) {
    check_namespaced_placeholders(
        text,
        "resources",
        resources,
        &rule.name.0,
        "declared resource keys",
        errors,
    );
    check_namespaced_placeholders(
        text,
        "params",
        params,
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
            available("declared resource keys", resources),
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_workflow;
    use std::path::Path;

    #[test]
    fn namespaced_placeholder_scanning_handles_unicode_keys_and_multiple_errors() {
        let mut errors = Vec::new();
        check_namespaced_placeholders(
            "{resources.é} {resources.next}",
            "resources",
            &[],
            "a",
            "keys",
            &mut errors,
        );
        let placeholders: Vec<_> = errors
            .iter()
            .map(|error| match error {
                ParseError::UnresolvedPlaceholder { placeholder, .. } => placeholder.as_str(),
                other => panic!("unexpected error: {other:?}"),
            })
            .collect();
        assert_eq!(placeholders, vec!["{resources.é}", "{resources.next}"]);
    }

    #[test]
    fn placeholder_errors_are_deduplicated_and_keep_empty_key_diagnostics() {
        let wf = parse_workflow(
            "[rule.a]\nshell='echo {resources.missing} {resources.missing} {params.absent} {resources.unclosed'",
            Path::new("test.toml"),
        ).unwrap();
        let errors = validate(&wf).unwrap_err();
        assert_eq!(errors.len(), 2);
        for (error, placeholder, label) in [
            (
                &errors[0],
                "{resources.missing}",
                "declared resource keys: none",
            ),
            (
                &errors[1],
                "{params.absent}",
                "declared parameter keys: none",
            ),
        ] {
            match error {
                ParseError::UnresolvedPlaceholder {
                    rule,
                    placeholder: actual,
                    available,
                } => {
                    assert_eq!(rule, "a");
                    assert_eq!(actual, placeholder);
                    assert_eq!(available, label);
                }
                other => panic!("unexpected error: {other:?}"),
            }
        }
    }

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
    fn output_wildcard_not_in_input_is_info_only() {
        // A rule where an output wildcard does not appear in inputs.
        // Config can supply output-only wildcard values after validation.
        let toml = r#"
[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}/{extra_wc}.txt"]
shell = "process {input} > {output}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        // Validation must not reject output-only wildcards before config expansion.
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
    fn rejects_unknown_resource_placeholder_in_environment_value() {
        let toml = r#"
[rule.process]
output = ["out.txt"]
shell = "true"
resources = { cpu = 2 }
env = { OMP_NUM_THREADS = "{resources.typo}" }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let messages: Vec<String> = validate(&wf)
            .unwrap_err()
            .into_iter()
            .map(|error| error.to_string())
            .collect();
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("{resources.typo}"));
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
