//! Lockfile writer — generates `ox.lock` from workflow state.

use std::collections::BTreeMap;
use std::path::Path;

use ox_core::hashing::{hash_kv_map, update_field, update_opt_field};
use ox_core::model::{ContentHash, EnvSpec, ExecutionBlock, Rule};

use crate::error::LockError;
use crate::model::{LockedEnvironment, LockedRule, Lockfile};

/// Build and write a lockfile to disk.
///
/// Captures the Oxymakefile hash, all rule definitions, environment specs,
/// and input file hashes into a TOML lockfile.
pub fn write_lockfile(
    lockfile_path: &Path,
    oxymakefile_hash: &ContentHash,
    rules: &[Rule],
    input_file_hashes: &BTreeMap<String, ContentHash>,
) -> Result<Lockfile, LockError> {
    let mut lockfile = Lockfile::new(oxymakefile_hash.clone());

    // Lock each rule.
    for rule in rules {
        let execution_hash = hash_execution_block(rule);
        let env_label = rule.environment.as_ref().map(env_spec_label);

        // Register the environment if not already present.
        if let Some(ref env) = rule.environment {
            let label = env_spec_label(env);
            if !lockfile.environments.contains_key(&label) {
                lockfile
                    .environments
                    .insert(label.clone(), lock_environment(env));
            }
        }

        let inputs: Vec<String> = rule
            .inputs
            .iter()
            .map(|i| i.pattern.as_str().to_string())
            .collect();
        let outputs: Vec<String> = rule
            .outputs
            .iter()
            .map(|o| o.pattern.as_str().to_string())
            .collect();

        let params_hash = hash_rule_params(rule);

        let locked = LockedRule {
            execution_hash,
            environment: env_label,
            inputs,
            outputs,
            params_hash,
        };

        lockfile
            .rules
            .insert(rule.name.as_str().to_string(), locked);
    }

    // Record input file hashes.
    lockfile.inputs = input_file_hashes.clone();

    // Serialize to TOML and write.
    let toml_str = toml::to_string_pretty(&lockfile)?;
    if let Some(parent) = lockfile_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(lockfile_path, toml_str)?;

    Ok(lockfile)
}

/// Read a lockfile from disk.
pub fn read_lockfile(path: &Path) -> Result<Lockfile, LockError> {
    if !path.exists() {
        return Err(LockError::NotFound(path.display().to_string()));
    }
    let content = std::fs::read_to_string(path)?;
    let lockfile: Lockfile = toml::from_str(&content)?;
    Ok(lockfile)
}

/// Compute a BLAKE3 hash of the rule's execution block.
///
/// For script-mode rules, the *content* of the script file enters the hash
/// (when the file is readable), so editing the script registers as drift
/// even though the path in the execution block is unchanged (audit B2).
pub fn hash_execution_block(rule: &Rule) -> ContentHash {
    let serialized =
        serde_json::to_string(&rule.execution).unwrap_or_else(|_| rule.execution.to_string());
    let mut hasher = blake3::Hasher::new();
    update_field(&mut hasher, "execution", serialized.as_bytes());
    if let ExecutionBlock::Script { path, .. } = &rule.execution {
        let content = std::fs::read(path).ok();
        update_opt_field(&mut hasher, "script_content", content.as_deref());
    }
    ContentHash::from(hasher.finalize())
}

/// Compute the hash of a rule's named parameters (`None` when empty).
///
/// Key/value pairs are length-framed (see [`ox_core::hashing::hash_kv_map`])
/// so adjacent keys and values cannot be confused (audit B1 motif).
pub fn hash_rule_params(rule: &Rule) -> Option<ContentHash> {
    if rule.params.is_empty() {
        return None;
    }
    Some(ContentHash::from_hex(hash_kv_map(&rule.params)).expect("hash_kv_map yields valid hex"))
}

/// Generate a label for an environment spec (used as map key).
fn env_spec_label(env: &EnvSpec) -> String {
    match env {
        EnvSpec::System => "system".to_string(),
        EnvSpec::Uv { requirements } => {
            if let Some(req) = requirements {
                format!("uv:{req}")
            } else {
                "uv".to_string()
            }
        }
        EnvSpec::Conda { env } => format!("conda:{env}"),
        EnvSpec::Docker { image } => format!("docker:{image}"),
        EnvSpec::Nix { expr } => format!("nix:{expr}"),
        EnvSpec::Apptainer { image } => format!("apptainer:{image}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::model::{ExecutionBlock, ReproducibilityClass, RuleName};

    fn make_rule(name: &str, execution: ExecutionBlock) -> Rule {
        Rule {
            name: RuleName(name.into()),
            priority: None,
            inputs: vec![],
            outputs: vec![],
            execution,
            environment: None,
            resources: Default::default(),
            tags: Default::default(),
            meta: Default::default(),
            wildcard_constraints: Default::default(),
            when: None,
            expand_mode: Default::default(),
            error_strategy: Default::default(),
            timeout: None,
            executor: None,
            log: Default::default(),
            benchmark: None,
            retries: None,
            params: Default::default(),
            param_files: Vec::new(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
            source_line: None,
        }
    }

    /// Audit B2 — editing a script file must change the locked execution
    /// hash, otherwise drift detection misses script edits entirely.
    #[test]
    fn execution_hash_tracks_script_content() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("script.py");
        std::fs::write(&script, "print('v1')").unwrap();

        let rule = make_rule(
            "analyze",
            ExecutionBlock::Script {
                path: script.clone(),
                lang: Some("python".into()),
            },
        );
        let h1 = hash_execution_block(&rule);

        std::fs::write(&script, "print('v2')").unwrap();
        let h2 = hash_execution_block(&rule);

        assert_ne!(h1, h2, "script content must enter the execution hash");
    }

    /// Audit B1 (same motif) — params key/value pairs must be framed so
    /// that {"ab": "c"} and {"a": "bc"} hash differently.
    #[test]
    fn params_hash_kv_boundary_injective() {
        let mut r1 = make_rule(
            "a",
            ExecutionBlock::Shell {
                command: "echo".into(),
            },
        );
        r1.params.insert("ab".into(), "c".into());
        let mut r2 = r1.clone();
        r2.params.clear();
        r2.params.insert("a".into(), "bc".into());

        assert_ne!(
            hash_rule_params(&r1),
            hash_rule_params(&r2),
            "params k/v boundary must be framed"
        );
    }
}

/// Create a locked environment record from an EnvSpec.
fn lock_environment(env: &EnvSpec) -> LockedEnvironment {
    match env {
        EnvSpec::System => LockedEnvironment {
            kind: "system".to_string(),
            spec_hash: None,
            image: None,
            flake: None,
        },
        EnvSpec::Uv { requirements } => {
            let spec_hash = requirements.as_ref().and_then(|req| {
                std::fs::read(req)
                    .ok()
                    .map(|data| ContentHash::from(blake3::hash(&data)))
            });
            LockedEnvironment {
                kind: "uv".to_string(),
                spec_hash,
                image: None,
                flake: None,
            }
        }
        EnvSpec::Conda { env } => {
            let spec_hash = std::fs::read(env)
                .ok()
                .map(|data| ContentHash::from(blake3::hash(&data)));
            LockedEnvironment {
                kind: "conda".to_string(),
                spec_hash,
                image: None,
                flake: None,
            }
        }
        EnvSpec::Docker { image } => LockedEnvironment {
            kind: "docker".to_string(),
            spec_hash: None,
            image: Some(image.clone()),
            flake: None,
        },
        EnvSpec::Nix { expr } => {
            // Hash the expression file's content when it is a readable
            // file (same motif as uv/conda above).
            let spec_hash = std::fs::read(expr)
                .ok()
                .map(|data| ContentHash::from(blake3::hash(&data)));
            LockedEnvironment {
                kind: "nix".to_string(),
                spec_hash,
                image: None,
                flake: Some(expr.clone()),
            }
        }
        EnvSpec::Apptainer { image } => LockedEnvironment {
            kind: "apptainer".to_string(),
            spec_hash: None,
            image: Some(image.clone()),
            flake: None,
        },
    }
}
