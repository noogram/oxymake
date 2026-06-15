//! Resolve `configfile:` directives by reading the referenced YAML file
//! and converting its contents into [`ConfigEntryIR`] values.

use std::path::Path;

use anyhow::{Context, Result};
use serde_yaml_ng::Value;

use crate::ir::ConfigEntryIR;

/// Read a YAML config file and convert its top-level entries to [`ConfigEntryIR`].
///
/// `base_dir` is the directory containing the Snakefile (used to resolve
/// relative config paths). Only top-level scalar and list-of-scalar entries
/// are converted; nested mappings are silently skipped.
pub fn resolve_configfile(config_path: &str, base_dir: &Path) -> Result<Vec<ConfigEntryIR>> {
    let full_path = base_dir.join(config_path);
    let content = std::fs::read_to_string(&full_path)
        .with_context(|| format!("failed to read config file: {}", full_path.display()))?;
    parse_yaml_config(&content)
}

/// Parse YAML content into config entries.
pub fn parse_yaml_config(yaml: &str) -> Result<Vec<ConfigEntryIR>> {
    let root: Value = serde_yaml_ng::from_str(yaml).context("failed to parse YAML config")?;

    let mapping = match root {
        Value::Mapping(m) => m,
        _ => return Ok(vec![]),
    };

    let mut entries = Vec::new();
    for (key, value) in &mapping {
        let key_str = match key {
            Value::String(s) => s.clone(),
            _ => continue,
        };
        match value {
            Value::String(s) => {
                entries.push(ConfigEntryIR {
                    key: key_str,
                    values: vec![s.clone()],
                });
            }
            Value::Number(n) => {
                entries.push(ConfigEntryIR {
                    key: key_str,
                    values: vec![n.to_string()],
                });
            }
            Value::Bool(b) => {
                entries.push(ConfigEntryIR {
                    key: key_str,
                    values: vec![b.to_string()],
                });
            }
            Value::Sequence(seq) => {
                let mut vals = Vec::new();
                for item in seq {
                    match item {
                        Value::String(s) => vals.push(s.clone()),
                        Value::Number(n) => vals.push(n.to_string()),
                        Value::Bool(b) => vals.push(b.to_string()),
                        _ => continue,
                    }
                }
                if !vals.is_empty() {
                    entries.push(ConfigEntryIR {
                        key: key_str,
                        values: vals,
                    });
                }
            }
            // Nested mappings and other complex types are skipped
            _ => {}
        }
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_yaml_scalars() {
        let yaml = r#"
normalize: true
n_components: 2
seed: 42
name: "test"
"#;
        let entries = parse_yaml_config(yaml).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].key, "normalize");
        assert_eq!(entries[0].values, vec!["true"]);
        assert_eq!(entries[1].key, "n_components");
        assert_eq!(entries[1].values, vec!["2"]);
        assert_eq!(entries[2].key, "seed");
        assert_eq!(entries[2].values, vec!["42"]);
        assert_eq!(entries[3].key, "name");
        assert_eq!(entries[3].values, vec!["test"]);
    }

    #[test]
    fn test_parse_yaml_list() {
        let yaml = r#"
samples:
  - iris
  - wine
"#;
        let entries = parse_yaml_config(yaml).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "samples");
        assert_eq!(entries[0].values, vec!["iris", "wine"]);
    }

    #[test]
    fn test_parse_yaml_mixed() {
        let yaml = r#"
samples:
  - iris
  - wine
normalize: true
n_components: 2
seed: 42
"#;
        let entries = parse_yaml_config(yaml).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].key, "samples");
        assert_eq!(entries[0].values, vec!["iris", "wine"]);
        assert_eq!(entries[1].key, "normalize");
        assert_eq!(entries[1].values, vec!["true"]);
    }

    #[test]
    fn test_parse_yaml_nested_mapping_skipped() {
        let yaml = r#"
simple: "value"
nested:
  key1: val1
  key2: val2
"#;
        let entries = parse_yaml_config(yaml).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "simple");
    }

    #[test]
    fn test_parse_yaml_empty() {
        let entries = parse_yaml_config("").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_yaml_numeric_list() {
        let yaml = r#"
thresholds:
  - 0.1
  - 0.5
  - 0.9
"#;
        let entries = parse_yaml_config(yaml).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "thresholds");
        assert_eq!(entries[0].values, vec!["0.1", "0.5", "0.9"]);
    }

    #[test]
    fn test_resolve_configfile() {
        // Find the workspace root (cargo sets CARGO_MANIFEST_DIR to the crate dir)
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
        let dir = workspace_root.join("benchmark/advanced");
        let entries = resolve_configfile("config.yaml", &dir).unwrap();
        assert!(entries.iter().any(|e| e.key == "samples"));
        assert!(entries.iter().any(|e| e.key == "seed"));
    }
}
