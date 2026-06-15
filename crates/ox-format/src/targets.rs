//! Canonical target resolution — the single shared implementation used by
//! every surface (CLI, public API, MCP server).
//!
//! Resolving "what should this run build?" from a [`Workflow`] involves
//! `{config.X}` substitution and wildcard expansion. This logic used to be
//! copied (divergently) into ox-cli, ox-api, and ox-mcp: only the CLI copy
//! had the `{config.X}` fix (ox-7a98), so the same Oxymakefile worked on
//! the command line and failed through the API and MCP (H34). It now lives
//! here, next to the parser, and the three surfaces delegate.

use std::collections::BTreeMap;

use ox_core::resolver::Config;

use crate::parse::{ConfigValue, Workflow};

/// Convert a parsed Workflow's config section into the resolver's [`Config`].
pub fn workflow_config(workflow: &Workflow) -> Config {
    let mut lists = BTreeMap::new();
    let mut scalars = BTreeMap::new();
    for (key, value) in &workflow.config {
        match value {
            ConfigValue::List(items) => {
                lists.insert(key.clone(), items.clone());
            }
            ConfigValue::Scalar(s) => {
                scalars.insert(key.clone(), s.clone());
            }
            ConfigValue::FileSource { .. } => {
                // FileSource should already be resolved to List by parse_workflow.
                // If we reach here, the file was empty — treat as empty list.
                lists.insert(key.clone(), Vec::new());
            }
        }
    }
    Config { lists, scalars }
}

/// Determine target outputs from user-provided targets and the workflow.
///
/// If the user specified explicit targets, `{config.X}` references in them
/// are substituted and the results returned. Otherwise, the rule named
/// `all` (or the first rule) provides the defaults: an aggregation rule's
/// inputs, or a normal rule's outputs, expanded against the config.
pub fn resolve_targets(workflow: &Workflow, user_targets: &[String]) -> Vec<String> {
    if !user_targets.is_empty() {
        // Substitute {config.X} references in user-provided targets so that
        // e.g. `ox run '{config.results_dir}/summary.json'` resolves correctly.
        let config = workflow_config(workflow);
        return user_targets
            .iter()
            .map(|t| substitute_config_refs(t, &config.scalars))
            .collect();
    }

    // Default: prefer a rule named "all" (standard aggregation target), then fall
    // back to the first rule in the list. Rules are stored in a BTreeMap-derived
    // Vec (alphabetical), so without this preference 'align' would beat 'all'.
    let default_rule = workflow
        .rules
        .iter()
        .find(|r| r.name.as_str() == "all")
        .or_else(|| workflow.rules.first());
    if let Some(first_rule) = default_rule {
        if first_rule.outputs.is_empty() {
            // Aggregation rule — its inputs are what we want to build.
            // Expand wildcards from config.
            let config = workflow_config(workflow);
            let mut targets = Vec::new();
            for input in &first_rule.inputs {
                let pattern_str = input.pattern.as_str();
                // Check if pattern has wildcards
                if pattern_str.contains('{') {
                    // Expand from config
                    expand_pattern(pattern_str, &config, &mut targets);
                } else {
                    targets.push(pattern_str.to_string());
                }
            }
            return targets;
        }

        // Normal rule with outputs — return the output patterns as targets.
        // But they may have wildcards too; expand from config.
        let config = workflow_config(workflow);
        let mut targets = Vec::new();
        for output in &first_rule.outputs {
            let pattern_str = output.pattern.as_str();
            if pattern_str.contains('{') {
                expand_pattern(pattern_str, &config, &mut targets);
            } else {
                targets.push(pattern_str.to_string());
            }
        }
        return targets;
    }

    Vec::new()
}

/// Expand a pattern with `{wildcard}` placeholders using config lists.
///
/// For each wildcard found in the pattern, look up a list in config. If there
/// are multiple wildcards, compute the cartesian product.
///
/// `{config.X}` references are substituted with scalar config values before
/// wildcard extraction, matching the resolver's behavior.
pub fn expand_pattern(pattern: &str, config: &Config, out: &mut Vec<String>) {
    // Substitute {config.X} references with scalar values first.
    let pattern = substitute_config_refs(pattern, &config.scalars);

    // Extract wildcard names from the pattern.
    let wildcards = extract_wildcards(&pattern);

    if wildcards.is_empty() {
        out.push(pattern.to_string());
        return;
    }

    // Build a list of (name, values) for each wildcard that has a config entry.
    // Convention: config key "samples" matches wildcard "{sample}" (plural -> singular).
    let mut axes: Vec<(&str, &[String])> = Vec::new();
    for wc in &wildcards {
        if let Some(values) = config.lists.get(wc.as_str()) {
            axes.push((wc, values));
        } else if let Some(values) = config.lists.get(&format!("{}s", wc)) {
            // Try plural form: wildcard "sample" matches config key "samples".
            axes.push((wc, values));
        } else if wc.ends_with('s') {
            // Try singular form: wildcard "samples" matches config key "sample".
            if let Some(values) = config.lists.get(&wc[..wc.len() - 1]) {
                axes.push((wc, values));
            } else {
                return;
            }
        } else {
            // No config for this wildcard — cannot expand, skip.
            return;
        }
    }

    // Cartesian product expansion.
    let mut combos: Vec<Vec<(&str, &str)>> = vec![vec![]];
    for (name, values) in &axes {
        let mut new_combos = Vec::new();
        for combo in &combos {
            for val in *values {
                let mut new = combo.clone();
                new.push((name, val.as_str()));
                new_combos.push(new);
            }
        }
        combos = new_combos;
    }

    for combo in &combos {
        let mut expanded = pattern.to_string();
        for (name, val) in combo {
            expanded = expanded.replace(&format!("{{{}}}", name), val);
        }
        out.push(expanded);
    }
}

/// Substitute `{config.key}` references in a pattern with scalar config values.
///
/// Unknown keys are left as-is (the pattern may still work as a literal path).
pub fn substitute_config_refs(pattern: &str, scalars: &BTreeMap<String, String>) -> String {
    if !pattern.contains("{config.") {
        return pattern.to_owned();
    }

    let mut result = String::with_capacity(pattern.len());
    let mut rest = pattern;

    while let Some(start) = rest.find("{config.") {
        result.push_str(&rest[..start]);
        let after_open = &rest[start + 1..]; // skip the `{`
        if let Some(close) = after_open.find('}') {
            let placeholder = &after_open[..close]; // e.g. "config.results_dir"
            let key = &placeholder["config.".len()..]; // e.g. "results_dir"
            if let Some(value) = scalars.get(key) {
                result.push_str(value);
            } else {
                // Unknown key — keep original placeholder so it doesn't silently vanish.
                result.push('{');
                result.push_str(placeholder);
                result.push('}');
            }
            rest = &after_open[close + 1..];
        } else {
            // No closing brace — push remainder and stop.
            result.push_str(&rest[start..]);
            rest = "";
            break;
        }
    }
    result.push_str(rest);
    result
}

/// Extract wildcard names from a pattern string like "results/{sample}.txt".
pub fn extract_wildcards(pattern: &str) -> Vec<String> {
    let mut wildcards = Vec::new();
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut name = String::new();
            for inner in chars.by_ref() {
                if inner == '}' {
                    break;
                }
                name.push(inner);
            }
            if !name.is_empty() && !wildcards.contains(&name) {
                wildcards.push(name);
            }
        }
    }
    wildcards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_wildcards_simple() {
        let wcs = extract_wildcards("results/{sample}.txt");
        assert_eq!(wcs, vec!["sample"]);
    }

    #[test]
    fn extract_wildcards_multiple() {
        let wcs = extract_wildcards("{stage}/{sample}.{ext}");
        assert_eq!(wcs, vec!["stage", "sample", "ext"]);
    }

    #[test]
    fn extract_wildcards_none() {
        let wcs = extract_wildcards("plain/file.txt");
        assert!(wcs.is_empty());
    }

    #[test]
    fn substitute_config_refs_known_key() {
        let mut scalars = BTreeMap::new();
        scalars.insert("results_dir".to_string(), "out".to_string());
        assert_eq!(
            substitute_config_refs("{config.results_dir}/x.txt", &scalars),
            "out/x.txt"
        );
    }

    #[test]
    fn substitute_config_refs_unknown_key_kept() {
        let scalars = BTreeMap::new();
        assert_eq!(
            substitute_config_refs("{config.missing}/x.txt", &scalars),
            "{config.missing}/x.txt"
        );
    }
}
