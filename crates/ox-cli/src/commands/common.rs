//! Shared utilities used across multiple commands.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};

use ox_core::model::ResourceValue;
use ox_core::resolver::Config;
use ox_format::parse::{ConfigValue, Profile, Workflow};

/// Read and parse an Oxymakefile from disk.
/// Also resolves `{config.X}` references in resource values (config interpolation).
pub fn load_workflow(path: &Path) -> Result<Workflow> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read Oxymakefile: {}", path.display()))?;
    let mut workflow = ox_format::parse::parse_workflow(&content, path)
        .with_context(|| format!("parse error in {}", path.display()))?;

    // Resolve {config.X} references in resource values.
    let scalars: BTreeMap<String, String> = workflow
        .config
        .iter()
        .filter_map(|(k, v)| match v {
            ConfigValue::Scalar(s) => Some((k.clone(), s.clone())),
            ConfigValue::List(l) if l.len() == 1 => Some((k.clone(), l[0].clone())),
            _ => None,
        })
        .collect();

    for rule in &mut workflow.rules {
        for rv in rule.resources.values_mut() {
            if let ResourceValue::Str(s) = rv {
                let mut resolved = s.clone();
                for (ck, cv) in &scalars {
                    resolved = resolved.replace(&format!("{{config.{}}}", ck), cv);
                }
                if resolved != *s {
                    // Try to parse as number after interpolation.
                    if let Ok(n) = resolved.parse::<i64>() {
                        *rv = ResourceValue::Int(n);
                    } else if let Ok(f) = resolved.parse::<f64>() {
                        *rv = ResourceValue::Float(f.into());
                    } else {
                        *s = resolved;
                    }
                }
            }
        }
    }

    Ok(workflow)
}

// Target resolution lives in ox-format::targets — the single shared
// implementation used by the CLI, the public API, and the MCP server (H34).
// These re-exports keep the historical `common::` call sites working.
pub use ox_format::targets::{expand_pattern, resolve_targets, workflow_config};

/// Parse a human-readable byte size into `u64`.
///
/// Supports common suffixes: `K`/`KB`, `M`/`MB`, `G`/`GB`, `T`/`TB` (case-insensitive).
/// Plain integers are treated as raw bytes. Returns an error for invalid input.
///
/// # Examples
///
/// ```ignore
/// assert_eq!(parse_human_size("0").unwrap(), 0);
/// assert_eq!(parse_human_size("512M").unwrap(), 512 * 1024 * 1024);
/// assert_eq!(parse_human_size("1G").unwrap(), 1024 * 1024 * 1024);
/// ```
pub fn parse_human_size(s: &str) -> Result<u64> {
    let s = s.trim();
    if s == "0" {
        return Ok(0);
    }

    // Split into numeric prefix and optional suffix.
    let (num_part, suffix) = match s.find(|c: char| c.is_ascii_alphabetic()) {
        Some(pos) => (&s[..pos], s[pos..].to_ascii_uppercase()),
        None => (s, String::new()),
    };

    let base: u64 = num_part
        .parse()
        .with_context(|| format!("invalid size: {s:?}"))?;

    let multiplier: u64 = match suffix.as_str() {
        "" => 1,
        "K" | "KB" | "KIB" => 1024,
        "M" | "MB" | "MIB" => 1024 * 1024,
        "G" | "GB" | "GIB" => 1024 * 1024 * 1024,
        "T" | "TB" | "TIB" => 1024 * 1024 * 1024 * 1024,
        _ => anyhow::bail!("unknown size suffix: {suffix:?} (expected K, M, G, or T)"),
    };

    base.checked_mul(multiplier)
        .with_context(|| format!("size overflow: {s:?}"))
}

/// Apply `--set KEY=VALUE` overrides to a resolver [`Config`].
///
/// Each override is expected in `KEY=VALUE` form.  A comma-separated value
/// (e.g. `samples=A,B,C`) is inserted as a list; a plain value is inserted as
/// a scalar *and* replaces the corresponding list entry if one exists.
pub fn apply_overrides(config: &mut Config, overrides: &[String]) {
    for entry in overrides {
        if let Some((key, value)) = entry.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim();

            if value.contains(',') {
                // Comma-separated → list override.
                let items: Vec<String> = value.split(',').map(|s| s.trim().to_string()).collect();
                config.lists.insert(key, items);
            } else {
                // Scalar override.
                config.scalars.insert(key.clone(), value.to_string());
                // Also replace a list entry if one exists for this key, so that
                // wildcard expansion picks up the override.
                if config.lists.contains_key(&key) {
                    config.lists.insert(key, vec![value.to_string()]);
                }
            }
        }
    }
}

/// Look up and return a named profile from the workflow.
///
/// Returns an error if the profile name is not defined.
pub fn resolve_profile<'a>(workflow: &'a Workflow, name: &str) -> Result<&'a Profile> {
    workflow.profiles.get(name).with_context(|| {
        let available: Vec<&str> = workflow.profiles.keys().map(|s| s.as_str()).collect();
        if available.is_empty() {
            format!("profile '{name}' not found (no profiles defined in Oxymakefile)")
        } else {
            format!(
                "profile '{name}' not found (available: {})",
                available.join(", ")
            )
        }
    })
}

/// Apply profile `--set` overrides to a resolver [`Config`].
pub fn apply_profile_config(config: &mut Config, profile: &Profile) {
    let overrides: Vec<String> = profile
        .set
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    apply_overrides(config, &overrides);
}

/// Discover files that exist on disk, relative to the Oxymakefile's directory.
///
/// Delegates to [`ox_api::discover::discover_existing_files`] which caches
/// results per base directory with mtime invalidation.
pub fn discover_existing_files(oxymakefile_path: &Path) -> Vec<std::path::PathBuf> {
    ox_api::discover::discover_existing_files(oxymakefile_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_overrides_scalar() {
        let mut config = Config::default();
        config.scalars.insert("genome".into(), "hg19".into());
        apply_overrides(&mut config, &["genome=hg38".into()]);
        assert_eq!(config.scalars["genome"], "hg38");
    }

    #[test]
    fn apply_overrides_list() {
        let mut config = Config::default();
        config
            .lists
            .insert("sample".into(), vec!["A".into(), "B".into()]);
        apply_overrides(&mut config, &["sample=X,Y,Z".into()]);
        assert_eq!(config.lists["sample"], vec!["X", "Y", "Z"]);
    }

    #[test]
    fn apply_overrides_scalar_replaces_list() {
        let mut config = Config::default();
        config
            .lists
            .insert("sample".into(), vec!["A".into(), "B".into()]);
        apply_overrides(&mut config, &["sample=ONLY".into()]);
        assert_eq!(config.scalars["sample"], "ONLY");
        assert_eq!(config.lists["sample"], vec!["ONLY"]);
    }

    #[test]
    fn expand_pattern_simple() {
        let mut config = Config::default();
        config
            .lists
            .insert("sample".into(), vec!["A".into(), "B".into()]);
        let mut out = Vec::new();
        expand_pattern("results/{sample}.txt", &config, &mut out);
        assert_eq!(out, vec!["results/A.txt", "results/B.txt"]);
    }

    /// Verify that the "all" rule is used as the default target even when
    /// other rules sort alphabetically before it (e.g. "align" < "all").
    #[test]
    fn resolve_targets_prefers_all_rule() {
        use std::path::Path;

        // Define a workflow where "align" comes before "all" alphabetically.
        let toml = r#"
[rule.align]
input = ["data/{sample}.fastq"]
output = ["aligned/{sample}.bam"]
shell = "echo align"

[rule.all]
input = ["aligned/A.bam", "aligned/B.bam"]
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();

        // With no user targets, "all" rule should be chosen (aggregation rule).
        let targets = resolve_targets(&wf, &[]);
        assert_eq!(targets, vec!["aligned/A.bam", "aligned/B.bam"]);
    }

    /// Aggregation rule with {config.X} references in inputs should resolve
    /// the config scalar before wildcard expansion (bug ox-7a98).
    #[test]
    fn resolve_targets_aggregation_with_config_ref() {
        use std::path::Path;

        let toml = r#"
[config]
results_dir = "output/v2"

[rule.all]
input = ["{config.results_dir}/annotate/summary.json"]
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();

        let targets = resolve_targets(&wf, &[]);
        assert_eq!(targets, vec!["output/v2/annotate/summary.json"]);
    }

    /// Aggregation rule with both {config.X} and {wildcard} in inputs
    /// should substitute config refs, then expand wildcards (bug ox-7a98).
    #[test]
    fn resolve_targets_aggregation_config_ref_with_wildcard() {
        use std::path::Path;

        let toml = r#"
[config]
results_dir = "output/v2"
sample = ["A", "B"]

[rule.all]
input = ["{config.results_dir}/{sample}.json"]
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();

        let targets = resolve_targets(&wf, &[]);
        assert_eq!(targets, vec!["output/v2/A.json", "output/v2/B.json"]);
    }

    /// Explicit CLI targets containing {config.X} should have the config
    /// references substituted before being returned (bug ox-wj0m).
    #[test]
    fn resolve_targets_substitutes_config_in_user_targets() {
        use std::path::Path;

        let toml = r#"
[config]
results_dir = "output/v2"

[rule.summarize]
input = ["data/raw.csv"]
output = ["{config.results_dir}/annotate/summary.json"]
shell = "echo summarize"
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();

        // Simulate: ox run '{config.results_dir}/annotate/summary.json'
        let targets = resolve_targets(
            &wf,
            &["{config.results_dir}/annotate/summary.json".to_string()],
        );
        assert_eq!(targets, vec!["output/v2/annotate/summary.json"]);
    }

    /// When there is no "all" rule, fall back to the first rule in the vec.
    #[test]
    fn resolve_targets_falls_back_to_first_rule() {
        use std::path::Path;

        let toml = r#"
[rule.build]
input = ["src/main.rs"]
output = ["target/main"]
shell = "echo build"
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();

        let targets = resolve_targets(&wf, &[]);
        assert_eq!(targets, vec!["target/main"]);
    }

    // -- Path handling edge cases (ox-58w3) ------------------------------------

    #[test]
    fn expand_pattern_with_spaces_in_config_dir() {
        let mut config = Config::default();
        config.scalars.insert("out_dir".into(), "my results".into());
        config
            .lists
            .insert("sample".into(), vec!["A".into(), "B".into()]);
        let mut out = Vec::new();
        expand_pattern("{config.out_dir}/{sample}.csv", &config, &mut out);
        assert_eq!(out, vec!["my results/A.csv", "my results/B.csv"]);
    }

    #[test]
    fn expand_pattern_with_unicode_in_config_dir() {
        let mut config = Config::default();
        config
            .scalars
            .insert("out_dir".into(), "données/résultats".into());
        config
            .lists
            .insert("sample".into(), vec!["échantillon_1".into()]);
        let mut out = Vec::new();
        expand_pattern("{config.out_dir}/{sample}.csv", &config, &mut out);
        assert_eq!(out, vec!["données/résultats/échantillon_1.csv"]);
    }

    #[test]
    fn resolve_targets_with_spaces_in_paths() {
        use std::path::Path;

        let toml = r#"
[config]
results_dir = "my results"
samples = ["A", "B"]

[rule.process]
input = ["input data/{sample}.csv"]
output = ["{config.results_dir}/{sample}_out.csv"]
shell = "echo process"
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();

        let targets = resolve_targets(&wf, &[]);
        // Config substitution should preserve spaces in dir names.
        assert_eq!(
            targets,
            vec!["my results/A_out.csv", "my results/B_out.csv"]
        );
    }

    #[test]
    fn resolve_targets_with_unicode_in_paths() {
        use std::path::Path;

        let toml = r#"
[config]
results_dir = "résultats"
samples = ["échantillon_1"]

[rule.traiter]
input = ["données/{sample}.csv"]
output = ["{config.results_dir}/{sample}_sortie.csv"]
shell = "echo process"
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();

        let targets = resolve_targets(&wf, &[]);
        assert_eq!(targets, vec!["résultats/échantillon_1_sortie.csv"]);
    }

    #[test]
    fn resolve_explicit_target_with_spaces() {
        use std::path::Path;

        let toml = r#"
[config]
results_dir = "my results"

[rule.process]
input = ["input data/{sample}.csv"]
output = ["{config.results_dir}/{sample}_out.csv"]
shell = "echo process"
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();

        let targets = resolve_targets(&wf, &["{config.results_dir}/A_out.csv".to_string()]);
        assert_eq!(targets, vec!["my results/A_out.csv"]);
    }

    // -- Profile tests ---------------------------------------------------------

    #[test]
    fn resolve_profile_found() {
        let toml = r#"
[profile.ci]
jobs = 4
cache_validation = "hash"

[rule.build]
input = ["a"]
output = ["b"]
shell = "echo"
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();
        let profile = resolve_profile(&wf, "ci").unwrap();
        assert_eq!(profile.jobs, Some(4));
        assert_eq!(profile.cache_validation.as_deref(), Some("hash"));
    }

    #[test]
    fn resolve_profile_not_found() {
        let toml = r#"
[profile.ci]
jobs = 4

[rule.build]
input = ["a"]
output = ["b"]
shell = "echo"
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();
        let err = resolve_profile(&wf, "prod").unwrap_err();
        assert!(err.to_string().contains("prod"));
        assert!(err.to_string().contains("ci"));
    }

    #[test]
    fn resolve_profile_no_profiles() {
        let toml = r#"
[rule.build]
input = ["a"]
output = ["b"]
shell = "echo"
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();
        let err = resolve_profile(&wf, "ci").unwrap_err();
        assert!(err.to_string().contains("no profiles defined"));
    }

    #[test]
    fn apply_profile_config_overrides() {
        let profile = Profile {
            jobs: None,
            cache_validation: None,
            verbose: None,
            executor: None,
            no_cache: None,
            keep_going: None,
            partition: None,
            account: None,
            qos: None,
            open_dashboard: None,
            set: {
                let mut m = BTreeMap::new();
                m.insert("genome".into(), "hg38".into());
                m
            },
        };
        let mut config = Config::default();
        config.scalars.insert("genome".into(), "hg19".into());
        apply_profile_config(&mut config, &profile);
        assert_eq!(config.scalars["genome"], "hg38");
    }

    // -- parse_human_size tests ------------------------------------------------

    #[test]
    fn parse_human_size_zero() {
        assert_eq!(parse_human_size("0").unwrap(), 0);
    }

    #[test]
    fn parse_human_size_raw_bytes() {
        assert_eq!(parse_human_size("1024").unwrap(), 1024);
        assert_eq!(parse_human_size("999999").unwrap(), 999999);
    }

    #[test]
    fn parse_human_size_kilobytes() {
        assert_eq!(parse_human_size("1K").unwrap(), 1024);
        assert_eq!(parse_human_size("4KB").unwrap(), 4 * 1024);
    }

    #[test]
    fn parse_human_size_megabytes() {
        assert_eq!(parse_human_size("512M").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_human_size("512MB").unwrap(), 512 * 1024 * 1024);
    }

    #[test]
    fn parse_human_size_gigabytes() {
        assert_eq!(parse_human_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_human_size("2GB").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_human_size_terabytes() {
        assert_eq!(parse_human_size("1T").unwrap(), 1024 * 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_human_size_case_insensitive() {
        assert_eq!(parse_human_size("512m").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_human_size("1g").unwrap(), 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_human_size_invalid_suffix() {
        assert!(parse_human_size("512X").is_err());
    }

    #[test]
    fn parse_human_size_not_a_number() {
        assert!(parse_human_size("abc").is_err());
    }

    #[test]
    fn parse_human_size_empty_string() {
        assert!(parse_human_size("").is_err());
    }

    #[test]
    fn parse_human_size_whitespace_only() {
        assert!(parse_human_size("   ").is_err());
    }

    #[test]
    fn parse_human_size_zero_with_suffix() {
        // "0M" should be 0 * 1024^2 = 0 (not an error).
        assert_eq!(parse_human_size("0M").unwrap(), 0);
    }

    #[test]
    fn cli_set_overrides_profile_set() {
        let profile = Profile {
            jobs: None,
            cache_validation: None,
            verbose: None,
            executor: None,
            no_cache: None,
            keep_going: None,
            partition: None,
            account: None,
            qos: None,
            open_dashboard: None,
            set: {
                let mut m = BTreeMap::new();
                m.insert("genome".into(), "hg38".into());
                m
            },
        };
        let mut config = Config::default();
        config.scalars.insert("genome".into(), "hg19".into());
        // Profile applies first
        apply_profile_config(&mut config, &profile);
        assert_eq!(config.scalars["genome"], "hg38");
        // Then CLI --set overrides
        apply_overrides(&mut config, &["genome=mm10".into()]);
        assert_eq!(config.scalars["genome"], "mm10");
    }
}
