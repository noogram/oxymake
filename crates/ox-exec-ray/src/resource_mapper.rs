//! Maps OxyMake resource specifications to Ray resource parameters.
//!
//! | OxyMake Resource | Ray Resource       | Notes                           |
//! |------------------|--------------------|---------------------------------|
//! | `cpu` / `cpus`   | `entrypoint_num_cpus`  | Direct mapping              |
//! | `mem` / `memory`  | (runtime_env memory) | Bytes (Ray uses bytes)      |
//! | `gpu` / `gpus`   | `entrypoint_num_gpus`  | Supports fractional (0.5)   |
//! | `custom:*`       | `entrypoint_resources` | Arbitrary custom resources  |

use std::collections::{BTreeMap, HashMap};

use ox_core::model::ResourceValue;

/// Mapped Ray resources extracted from OxyMake resource specifications.
#[derive(Debug, Clone, Default)]
pub struct RayResources {
    /// Number of CPUs for the entrypoint process.
    pub num_cpus: Option<f64>,
    /// Number of GPUs for the entrypoint process.
    pub num_gpus: Option<f64>,
    /// Memory in bytes.
    pub memory_bytes: Option<u64>,
    /// Custom resources (key → amount).
    pub custom: HashMap<String, f64>,
}

/// Convert OxyMake resource specifications to Ray resource parameters.
pub fn map_resources(resources: &BTreeMap<String, ResourceValue>) -> RayResources {
    let mut result = RayResources::default();

    for (key, value) in resources {
        match key.as_str() {
            "cpu" | "cpus" => {
                result.num_cpus = Some(resource_to_f64(value));
            }
            "gpu" | "gpus" => {
                result.num_gpus = Some(resource_to_f64(value));
            }
            "mem" | "memory" => {
                result.memory_bytes = Some(parse_memory_bytes(value));
            }
            other => {
                // Custom resources (strip "custom:" prefix if present).
                let name = other.strip_prefix("custom:").unwrap_or(other);
                result
                    .custom
                    .insert(name.to_string(), resource_to_f64(value));
            }
        }
    }

    result
}

/// Convert a ResourceValue to f64.
fn resource_to_f64(value: &ResourceValue) -> f64 {
    match value {
        ResourceValue::Int(n) => *n as f64,
        ResourceValue::Float(f) => f.into_inner(),
        ResourceValue::Str(s) => s.parse::<f64>().unwrap_or(0.0),
    }
}

/// Parse a memory value to bytes.
///
/// Accepts:
/// - Integer values (interpreted as bytes)
/// - Strings with suffixes: "K"/"KB", "M"/"MB", "G"/"GB", "T"/"TB"
fn parse_memory_bytes(value: &ResourceValue) -> u64 {
    match value {
        ResourceValue::Int(n) => *n as u64,
        ResourceValue::Float(f) => f.into_inner() as u64,
        ResourceValue::Str(s) => parse_memory_string(s),
    }
}

fn parse_memory_string(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }

    // Find where the numeric part ends.
    let num_end = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());

    let (num_str, suffix) = s.split_at(num_end);
    let num: f64 = num_str.parse().unwrap_or(0.0);
    let suffix = suffix.trim().to_uppercase();

    let multiplier: u64 = match suffix.as_str() {
        "" | "B" => 1,
        "K" | "KB" | "KIB" => 1024,
        "M" | "MB" | "MIB" => 1024 * 1024,
        "G" | "GB" | "GIB" => 1024 * 1024 * 1024,
        "T" | "TB" | "TIB" => 1024 * 1024 * 1024 * 1024,
        _ => 1,
    };

    (num * multiplier as f64) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_memory_string() {
        assert_eq!(parse_memory_string("1024"), 1024);
        assert_eq!(parse_memory_string("1K"), 1024);
        assert_eq!(parse_memory_string("1KB"), 1024);
        assert_eq!(parse_memory_string("2M"), 2 * 1024 * 1024);
        assert_eq!(parse_memory_string("4G"), 4 * 1024 * 1024 * 1024);
        assert_eq!(parse_memory_string("1T"), 1024 * 1024 * 1024 * 1024);
        assert_eq!(parse_memory_string("512MB"), 512 * 1024 * 1024);
        assert_eq!(parse_memory_string(""), 0);
    }

    #[test]
    fn test_map_resources_cpu_gpu() {
        let mut resources = BTreeMap::new();
        resources.insert("cpu".to_string(), ResourceValue::Int(4));
        resources.insert("gpu".to_string(), ResourceValue::Float(0.5.into()));

        let mapped = map_resources(&resources);
        assert_eq!(mapped.num_cpus, Some(4.0));
        assert_eq!(mapped.num_gpus, Some(0.5));
        assert!(mapped.memory_bytes.is_none());
    }

    #[test]
    fn test_map_resources_memory() {
        let mut resources = BTreeMap::new();
        resources.insert("memory".to_string(), ResourceValue::Str("2G".into()));

        let mapped = map_resources(&resources);
        assert_eq!(mapped.memory_bytes, Some(2 * 1024 * 1024 * 1024));
    }

    #[test]
    fn test_map_resources_custom() {
        let mut resources = BTreeMap::new();
        resources.insert("custom:tpu".to_string(), ResourceValue::Int(2));

        let mapped = map_resources(&resources);
        assert_eq!(mapped.custom.get("tpu"), Some(&2.0));
    }
}
