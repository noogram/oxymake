//! Maps checked OxyMake resource specifications to Ray parameters.
//!
//! | OxyMake Resource                 | Ray Resource              |
//! |----------------------------------|---------------------------|
//! | `cpu` / `cpus`                   | `entrypoint_num_cpus`     |
//! | `gpu` / `gpus`                   | `entrypoint_num_gpus`     |
//! | `mem` / `memory` / `mem_mb` / `mem_gb` | runtime-env memory |
//! | custom key / `custom:*`          | `entrypoint_resources`    |

use std::collections::BTreeMap;

use ox_core::model::ResourceValue;
use ox_core::resource::{ResourceError, TOKEN_SCALE, TokenAmount, normalize_resources};

/// Mapped Ray resources extracted from OxyMake resource specifications.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RayResources {
    /// Number of CPUs for the entrypoint process.
    pub num_cpus: Option<f64>,
    /// Number of GPUs for the entrypoint process.
    pub num_gpus: Option<f64>,
    /// Memory in bytes.
    pub memory_bytes: Option<u64>,
    /// Custom resources (key → amount).
    pub custom: BTreeMap<String, f64>,
}

/// A checked resource declaration cannot be represented by Ray.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceMapError {
    /// Custom names must be unambiguous Ray scheduling tokens.
    #[error("invalid Ray custom resource `{name}`: {reason}")]
    CustomName {
        /// Rejected resource name.
        name: String,
        /// Why the name cannot be forwarded.
        reason: &'static str,
    },
    /// Shared key or value normalization failed.
    #[error(transparent)]
    InvalidDeclaration(#[from] ResourceError),
    /// Ray only accepts whole multi-GPU counts; fractional GPUs are at most one.
    #[error("Ray cannot request fractional GPU count {amount} above one GPU")]
    FractionalGpuAboveOne {
        /// Exact rejected GPU amount.
        amount: TokenAmount,
    },
}

/// Convert OxyMake resource specifications to Ray resource parameters.
pub fn map_resources(
    resources: &BTreeMap<String, ResourceValue>,
) -> Result<RayResources, ResourceMapError> {
    let normalized = normalize_resources(resources)?;

    if let Some(gpu) = normalized.gpu
        && gpu.ten_thousandths() > TOKEN_SCALE
        && gpu.ten_thousandths() % TOKEN_SCALE != 0
    {
        return Err(ResourceMapError::FractionalGpuAboveOne { amount: gpu });
    }

    for name in normalized.custom.keys() {
        let lower = name.to_ascii_lowercase();
        let reason = if name.trim() != name {
            Some("leading or trailing whitespace is not allowed")
        } else if matches!(
            lower.as_str(),
            "cpu" | "cpus" | "gpu" | "gpus" | "memory" | "object_store_memory"
        ) || lower.starts_with("node:")
            || lower.starts_with("accelerator_type:")
        {
            Some("collides with a Ray built-in resource or reserved prefix")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(ResourceMapError::CustomName {
                name: name.clone(),
                reason,
            });
        }
    }

    Ok(RayResources {
        num_cpus: normalized.cpu.map(TokenAmount::as_f64),
        num_gpus: normalized.gpu.map(TokenAmount::as_f64),
        memory_bytes: normalized.memory_bytes,
        custom: normalized
            .custom
            .into_iter()
            .map(|(name, amount)| (name, amount.as_f64()))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::OrderedFloat;

    #[test]
    fn rejects_reserved_and_padded_custom_names() {
        for name in [
            "cpu",
            "cpus",
            "CPU",
            "gpu",
            "gpus",
            "GPU",
            "memory",
            "object_store_memory",
            "node:abc",
            "accelerator_type:A100",
            " metal",
            "metal ",
        ] {
            let resources = BTreeMap::from([
                ("cpu".into(), ResourceValue::Int(1)),
                (format!("custom:{name}"), ResourceValue::Int(1)),
            ]);
            let error = map_resources(&resources).unwrap_err().to_string();
            assert!(error.contains(name), "{error}");
        }
    }

    #[test]
    fn maps_checked_cpu_and_fractional_gpu() {
        let resources = BTreeMap::from([
            ("cpu".to_string(), ResourceValue::Int(4)),
            ("gpu".to_string(), ResourceValue::Float(OrderedFloat(0.5))),
        ]);

        let mapped = map_resources(&resources).unwrap();
        assert_eq!(mapped.num_cpus, Some(4.0));
        assert_eq!(mapped.num_gpus, Some(0.5));
        assert!(mapped.memory_bytes.is_none());
    }

    #[test]
    fn maps_memory_aliases_before_custom_resources() {
        let resources = BTreeMap::from([
            ("mem_mb".to_string(), ResourceValue::Int(1024)),
            ("custom:tpu".to_string(), ResourceValue::Int(2)),
        ]);

        let mapped = map_resources(&resources).unwrap();
        assert_eq!(mapped.memory_bytes, Some(1_073_741_824));
        assert_eq!(mapped.custom.get("tpu"), Some(&2.0));
        assert!(!mapped.custom.contains_key("mem_mb"));
    }

    #[test]
    fn rejects_fractional_gpu_above_one() {
        let resources =
            BTreeMap::from([("gpu".to_string(), ResourceValue::Float(OrderedFloat(1.5)))]);

        assert!(matches!(
            map_resources(&resources),
            Err(ResourceMapError::FractionalGpuAboveOne { .. })
        ));
    }

    #[test]
    fn propagates_shared_normalization_errors() {
        let resources = BTreeMap::from([("memory".to_string(), ResourceValue::Str("1XB".into()))]);

        assert!(matches!(
            map_resources(&resources),
            Err(ResourceMapError::InvalidDeclaration(_))
        ));
    }
}
