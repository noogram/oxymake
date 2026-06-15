//! Maps OxyMake resource specifications to SLURM `#SBATCH` directives.

use std::collections::BTreeMap;
use std::time::Duration;

use ox_core::model::ResourceValue;

use crate::error::SlurmError;

/// A single SLURM directive (e.g., `--cpus-per-task=4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlurmDirective {
    pub flag: String,
    pub value: String,
}

impl std::fmt::Display for SlurmDirective {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#SBATCH {}={}", self.flag, self.value)
    }
}

/// Convert OxyMake resource map + job timeout into SLURM `#SBATCH` directives.
///
/// # Errors
///
/// Returns `SlurmError::ResourceConflict` if both `mem` and `mem_per_cpu`
/// are specified (they are mutually exclusive in SLURM).
pub fn map_resources(
    resources: &BTreeMap<String, ResourceValue>,
    timeout: Option<Duration>,
) -> Result<Vec<SlurmDirective>, SlurmError> {
    let mut directives = Vec::new();
    let mut has_mem = false;
    let mut has_mem_per_cpu = false;

    for (key, value) in resources {
        match key.as_str() {
            "cpu" | "cpus" | "cpus_per_task" => {
                directives.push(SlurmDirective {
                    flag: "--cpus-per-task".into(),
                    value: resource_to_string(value),
                });
            }
            "mem" | "memory" => {
                has_mem = true;
                directives.push(SlurmDirective {
                    flag: "--mem".into(),
                    value: resource_to_string(value),
                });
            }
            "mem_mb" => {
                has_mem = true;
                directives.push(SlurmDirective {
                    flag: "--mem".into(),
                    value: format!("{}M", resource_to_string(value)),
                });
            }
            "mem_per_cpu" => {
                has_mem_per_cpu = true;
                directives.push(SlurmDirective {
                    flag: "--mem-per-cpu".into(),
                    value: resource_to_string(value),
                });
            }
            "gpu" | "gpus" => {
                directives.push(SlurmDirective {
                    flag: "--gpus".into(),
                    value: resource_to_string(value),
                });
            }
            "nodes" => {
                directives.push(SlurmDirective {
                    flag: "--nodes".into(),
                    value: resource_to_string(value),
                });
            }
            "tasks" | "ntasks" => {
                directives.push(SlurmDirective {
                    flag: "--ntasks".into(),
                    value: resource_to_string(value),
                });
            }
            "ntasks_per_node" => {
                directives.push(SlurmDirective {
                    flag: "--ntasks-per-node".into(),
                    value: resource_to_string(value),
                });
            }
            "gres" => {
                directives.push(SlurmDirective {
                    flag: "--gres".into(),
                    value: resource_to_string(value),
                });
            }
            "partition" => {
                directives.push(SlurmDirective {
                    flag: "--partition".into(),
                    value: resource_to_string(value),
                });
            }
            "time" => {
                directives.push(SlurmDirective {
                    flag: "--time".into(),
                    value: resource_to_string(value),
                });
            }
            "qos" => {
                directives.push(SlurmDirective {
                    flag: "--qos".into(),
                    value: resource_to_string(value),
                });
            }
            // Unknown resources are silently ignored — they may be for
            // other executors or custom OxyMake logic.
            _ => {}
        }
    }

    // Validate mutual exclusion: mem/memory/mem_mb all target --mem,
    // and --mem is mutually exclusive with --mem-per-cpu.
    let mem_count = directives.iter().filter(|d| d.flag == "--mem").count();
    if mem_count > 1 {
        return Err(SlurmError::ResourceConflict(
            "multiple memory resources (mem, memory, mem_mb) are mutually exclusive".into(),
        ));
    }
    if has_mem && has_mem_per_cpu {
        return Err(SlurmError::ResourceConflict(
            "--mem and --mem-per-cpu are mutually exclusive in SLURM".into(),
        ));
    }

    // If no explicit time resource, derive from job timeout (with 10% buffer).
    let has_time = directives.iter().any(|d| d.flag == "--time");
    if !has_time {
        if let Some(t) = timeout {
            let secs = t.as_secs();
            let buffered = secs + secs / 10; // +10% buffer
            let hours = buffered / 3600;
            let mins = (buffered % 3600) / 60;
            let secs_rem = buffered % 60;
            directives.push(SlurmDirective {
                flag: "--time".into(),
                value: format!("{hours:02}:{mins:02}:{secs_rem:02}"),
            });
        }
    }

    Ok(directives)
}

fn resource_to_string(value: &ResourceValue) -> String {
    match value {
        ResourceValue::Int(n) => n.to_string(),
        ResourceValue::Float(f) => format!("{f}"),
        ResourceValue::Str(s) => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_cpu_mem_mapping() {
        let mut resources = BTreeMap::new();
        resources.insert("cpu".into(), ResourceValue::Int(4));
        resources.insert("mem".into(), ResourceValue::Str("8G".into()));

        let directives = map_resources(&resources, None).unwrap();
        assert!(
            directives
                .iter()
                .any(|d| d.flag == "--cpus-per-task" && d.value == "4")
        );
        assert!(
            directives
                .iter()
                .any(|d| d.flag == "--mem" && d.value == "8G")
        );
    }

    #[test]
    fn gpu_mapping() {
        let mut resources = BTreeMap::new();
        resources.insert("gpu".into(), ResourceValue::Int(2));

        let directives = map_resources(&resources, None).unwrap();
        assert!(
            directives
                .iter()
                .any(|d| d.flag == "--gpus" && d.value == "2")
        );
    }

    #[test]
    fn mem_conflict_rejected() {
        let mut resources = BTreeMap::new();
        resources.insert("mem".into(), ResourceValue::Str("8G".into()));
        resources.insert("mem_per_cpu".into(), ResourceValue::Str("2G".into()));

        let result = map_resources(&resources, None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("mutually exclusive")
        );
    }

    #[test]
    fn timeout_derived_when_no_time_resource() {
        let resources = BTreeMap::new();
        let timeout = Some(Duration::from_secs(3600)); // 1 hour

        let directives = map_resources(&resources, timeout).unwrap();
        let time_dir = directives.iter().find(|d| d.flag == "--time").unwrap();
        assert_eq!(time_dir.value, "01:06:00"); // 1h + 10% = 1h6m
    }

    #[test]
    fn explicit_time_not_overridden() {
        let mut resources = BTreeMap::new();
        resources.insert("time".into(), ResourceValue::Str("2:00:00".into()));

        let directives = map_resources(&resources, Some(Duration::from_secs(60))).unwrap();
        let time_dirs: Vec<_> = directives.iter().filter(|d| d.flag == "--time").collect();
        assert_eq!(time_dirs.len(), 1);
        assert_eq!(time_dirs[0].value, "2:00:00");
    }

    #[test]
    fn unknown_resources_ignored() {
        let mut resources = BTreeMap::new();
        resources.insert("custom_thing".into(), ResourceValue::Str("foo".into()));

        let directives = map_resources(&resources, None).unwrap();
        assert!(directives.is_empty());
    }

    #[test]
    fn mem_mb_maps_to_mem_with_suffix() {
        let mut resources = BTreeMap::new();
        resources.insert("mem_mb".into(), ResourceValue::Int(8000));

        let directives = map_resources(&resources, None).unwrap();
        assert!(
            directives
                .iter()
                .any(|d| d.flag == "--mem" && d.value == "8000M"),
            "mem_mb=8000 should map to --mem=8000M, got: {directives:?}"
        );
    }

    #[test]
    fn mem_mb_string_passed_through() {
        let mut resources = BTreeMap::new();
        resources.insert("mem_mb".into(), ResourceValue::Str("4096".into()));

        let directives = map_resources(&resources, None).unwrap();
        assert!(
            directives
                .iter()
                .any(|d| d.flag == "--mem" && d.value == "4096M"),
            "mem_mb string should also get M suffix, got: {directives:?}"
        );
    }

    #[test]
    fn mem_mb_conflicts_with_mem() {
        let mut resources = BTreeMap::new();
        resources.insert("mem_mb".into(), ResourceValue::Int(8000));
        resources.insert("mem".into(), ResourceValue::Str("8G".into()));

        let result = map_resources(&resources, None);
        assert!(result.is_err(), "mem_mb and mem should conflict");
    }

    #[test]
    fn ntasks_per_node_mapping() {
        let mut resources = BTreeMap::new();
        resources.insert("nodes".into(), ResourceValue::Int(2));
        resources.insert("ntasks_per_node".into(), ResourceValue::Int(4));

        let directives = map_resources(&resources, None).unwrap();
        assert!(
            directives
                .iter()
                .any(|d| d.flag == "--nodes" && d.value == "2")
        );
        assert!(
            directives
                .iter()
                .any(|d| d.flag == "--ntasks-per-node" && d.value == "4")
        );
    }

    #[test]
    fn gres_mapping() {
        let mut resources = BTreeMap::new();
        resources.insert("gres".into(), ResourceValue::Str("gpu:1".into()));

        let directives = map_resources(&resources, None).unwrap();
        assert!(
            directives
                .iter()
                .any(|d| d.flag == "--gres" && d.value == "gpu:1")
        );
    }

    #[test]
    fn mem_mb_conflicts_with_mem_per_cpu() {
        let mut resources = BTreeMap::new();
        resources.insert("mem_mb".into(), ResourceValue::Int(8000));
        resources.insert("mem_per_cpu".into(), ResourceValue::Str("2G".into()));

        let result = map_resources(&resources, None);
        assert!(result.is_err(), "mem_mb and mem_per_cpu should conflict");
    }
}
