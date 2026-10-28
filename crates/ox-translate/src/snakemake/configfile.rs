//! Resolve `configfile:` directives by reading the referenced YAML file
//! and converting its contents into [`ConfigEntryIR`] values.

use std::path::Path;

use anyhow::{Context, Result, bail};
use saphyr::{LoadableYamlNode, Scalar, Yaml};

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
    let mut docs = Yaml::load_from_str(yaml).context("failed to parse YAML config")?;
    // A config file is a single document, as required by the previous parser.
    if docs.len() > 1 {
        bail!(
            "failed to parse YAML config: expected one document, found {}",
            docs.len()
        );
    }
    let mapping = match docs.pop() {
        Some(Yaml::Mapping(mapping)) => mapping,
        _ => return Ok(vec![]),
    };

    let mut entries = Vec::new();
    for (key, value) in &mapping {
        let key = match key {
            Yaml::Value(Scalar::String(key)) => key.to_string(),
            _ => continue,
        };
        let values: Vec<String> = match value {
            Yaml::Value(scalar) => scalar_to_string(scalar).into_iter().collect(),
            Yaml::Sequence(sequence) => sequence
                .iter()
                .filter_map(|item| match item {
                    Yaml::Value(scalar) => scalar_to_string(scalar),
                    _ => None,
                })
                .collect(),
            // Nested mappings and other complex types are skipped.
            _ => continue,
        };
        if !values.is_empty() {
            entries.push(ConfigEntryIR { key, values });
        }
    }
    Ok(entries)
}

/// Render scalars for both top-level values and list items, skipping nulls.
fn scalar_to_string(scalar: &Scalar<'_>) -> Option<String> {
    match scalar {
        Scalar::String(value) => Some(value.to_string()),
        Scalar::Integer(value) => Some(value.to_string()),
        // Debug retains the decimal point in values such as 1.0.
        Scalar::FloatingPoint(value) => Some(format!("{:?}", value.0)),
        Scalar::Boolean(value) => Some(value.to_string()),
        Scalar::Null => None,
    }
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
