//! Maps OxyMake `EnvSpec` to Ray `runtime_env` JSON.
//!
//! Ray's `runtime_env` supports pip packages, conda environments,
//! container images, working directories, and environment variables.
//! This module converts OxyMake's environment specifications into the
//! corresponding Ray runtime_env JSON structure.

use ox_core::model::EnvSpec;
use serde_json::{Value, json};

/// Convert an OxyMake `EnvSpec` to a Ray `runtime_env` JSON value.
///
/// Returns `None` for `EnvSpec::System` since no runtime env is needed.
pub fn env_spec_to_runtime_env(env: &EnvSpec) -> Option<Value> {
    match env {
        EnvSpec::System => None,

        EnvSpec::Uv { requirements } => {
            // Ray uses pip for Python package installation.
            // uv requirements are pip-compatible.
            let mut runtime_env = json!({});
            if let Some(reqs) = requirements {
                // If it looks like a file path, use it as a requirements file.
                // Otherwise, treat as inline package list.
                let packages: Vec<&str> = reqs
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .collect();
                if !packages.is_empty() {
                    runtime_env["pip"] = json!(packages);
                }
            }
            Some(runtime_env)
        }

        EnvSpec::Conda { env: env_spec } => {
            // Ray supports conda environments via runtime_env.
            let mut runtime_env = json!({});
            if env_spec.ends_with(".yml") || env_spec.ends_with(".yaml") {
                // Conda environment YAML file — Ray accepts it directly.
                runtime_env["conda"] = json!(env_spec);
            } else {
                // Named conda environment or inline deps.
                // Ray accepts a dict with channels + dependencies.
                runtime_env["conda"] = json!({
                    "dependencies": [env_spec],
                });
            }
            Some(runtime_env)
        }

        EnvSpec::Docker { image } => {
            // Ray supports container runtime_env.
            Some(json!({
                "container": {
                    "image": image,
                    "run_options": ["--network=host"],
                }
            }))
        }

        EnvSpec::Nix { .. } | EnvSpec::Apptainer { .. } => {
            // Nix and Apptainer are not natively supported by Ray's runtime_env.
            // These would require wrapping the entrypoint command instead.
            // Return None so the executor can handle them via command wrapping.
            None
        }
    }
}

/// Merge a base `runtime_env` with an overlay (e.g., memory limits from resources).
///
/// The overlay values take precedence over base values for top-level keys.
pub fn merge_runtime_env(base: Option<Value>, overlay: Option<Value>) -> Option<Value> {
    match (base, overlay) {
        (None, None) => None,
        (Some(b), None) => Some(b),
        (None, Some(o)) => Some(o),
        (Some(Value::Object(mut b)), Some(Value::Object(o))) => {
            for (k, v) in o {
                b.insert(k, v);
            }
            Some(Value::Object(b))
        }
        (_, Some(o)) => Some(o),
    }
}

/// Build a `runtime_env` overlay for memory limits from resource mapping.
pub fn memory_runtime_env(memory_bytes: u64) -> Value {
    // Ray's runtime_env doesn't directly limit memory, but we can set
    // environment variables that Ray or user code can respect.
    json!({
        "env_vars": {
            "OXYMAKE_MEMORY_LIMIT_BYTES": memory_bytes.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_env_returns_none() {
        assert!(env_spec_to_runtime_env(&EnvSpec::System).is_none());
    }

    #[test]
    fn test_uv_with_requirements() {
        let env = EnvSpec::Uv {
            requirements: Some("numpy\npandas\n# comment\nscipy".into()),
        };
        let rt = env_spec_to_runtime_env(&env).unwrap();
        let packages = rt["pip"].as_array().unwrap();
        assert_eq!(packages.len(), 3);
        assert_eq!(packages[0], "numpy");
        assert_eq!(packages[1], "pandas");
        assert_eq!(packages[2], "scipy");
    }

    #[test]
    fn test_uv_without_requirements() {
        let env = EnvSpec::Uv { requirements: None };
        let rt = env_spec_to_runtime_env(&env).unwrap();
        assert!(rt.get("pip").is_none());
    }

    #[test]
    fn test_conda_yaml_file() {
        let env = EnvSpec::Conda {
            env: "environment.yml".into(),
        };
        let rt = env_spec_to_runtime_env(&env).unwrap();
        assert_eq!(rt["conda"], "environment.yml");
    }

    #[test]
    fn test_conda_named_env() {
        let env = EnvSpec::Conda {
            env: "pytorch-2.5".into(),
        };
        let rt = env_spec_to_runtime_env(&env).unwrap();
        let deps = rt["conda"]["dependencies"].as_array().unwrap();
        assert_eq!(deps[0], "pytorch-2.5");
    }

    #[test]
    fn test_docker_image() {
        let env = EnvSpec::Docker {
            image: "python:3.12-slim".into(),
        };
        let rt = env_spec_to_runtime_env(&env).unwrap();
        assert_eq!(rt["container"]["image"], "python:3.12-slim");
    }

    #[test]
    fn test_nix_returns_none() {
        let env = EnvSpec::Nix {
            expr: "nixpkgs#python3".into(),
        };
        assert!(env_spec_to_runtime_env(&env).is_none());
    }

    #[test]
    fn test_merge_runtime_env() {
        let base = Some(json!({"pip": ["numpy"]}));
        let overlay = Some(json!({"env_vars": {"FOO": "bar"}}));
        let merged = merge_runtime_env(base, overlay).unwrap();
        assert_eq!(merged["pip"][0], "numpy");
        assert_eq!(merged["env_vars"]["FOO"], "bar");
    }

    #[test]
    fn test_merge_none_base() {
        let merged = merge_runtime_env(None, Some(json!({"pip": ["torch"]})));
        assert!(merged.is_some());
    }

    #[test]
    fn test_merge_both_none() {
        assert!(merge_runtime_env(None, None).is_none());
    }
}
