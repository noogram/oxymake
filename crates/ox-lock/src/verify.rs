//! Lockfile verification — drift detection between locked and current state.

use std::collections::BTreeMap;
use std::path::Path;

use ox_core::model::{ContentHash, Rule};

use crate::error::LockError;
use crate::model::Lockfile;
use crate::writer::read_lockfile;

/// A single drift finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    /// Category of drift.
    pub kind: DriftKind,
    /// Human-readable description.
    pub message: String,
}

/// Categories of drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftKind {
    /// The Oxymakefile content changed.
    OxymakefileChanged,
    /// The platform (OS or arch) changed.
    PlatformChanged,
    /// A rule was added that wasn't in the lockfile.
    RuleAdded,
    /// A rule was removed that was in the lockfile.
    RuleRemoved,
    /// A rule's execution block changed.
    RuleExecutionChanged,
    /// A rule's parameters changed.
    RuleParamsChanged,
    /// A rule's environment changed.
    RuleEnvironmentChanged,
    /// An input file's content hash changed.
    InputChanged,
    /// An input file was added.
    InputAdded,
    /// An input file was removed.
    InputRemoved,
}

/// Report of all drifts found.
#[derive(Debug, Clone)]
pub struct DriftReport {
    pub drifts: Vec<Drift>,
}

impl DriftReport {
    /// Returns `true` if no drift was detected.
    pub fn is_clean(&self) -> bool {
        self.drifts.is_empty()
    }

    /// Number of drifts.
    pub fn len(&self) -> usize {
        self.drifts.len()
    }

    /// Whether the report is empty.
    pub fn is_empty(&self) -> bool {
        self.drifts.is_empty()
    }
}

/// Verify an existing lockfile against the current workflow state.
///
/// Returns a drift report listing all differences found.
pub fn verify_lockfile(
    lockfile_path: &Path,
    current_oxymakefile_hash: &ContentHash,
    rules: &[Rule],
    current_input_hashes: &BTreeMap<String, ContentHash>,
) -> Result<DriftReport, LockError> {
    let locked = read_lockfile(lockfile_path)?;
    Ok(compare(
        &locked,
        current_oxymakefile_hash,
        rules,
        current_input_hashes,
    ))
}

/// Compare locked state against current state.
fn compare(
    locked: &Lockfile,
    current_oxymakefile_hash: &ContentHash,
    rules: &[Rule],
    current_input_hashes: &BTreeMap<String, ContentHash>,
) -> DriftReport {
    let mut drifts = Vec::new();

    // 1. Oxymakefile hash.
    if locked.oxymakefile_hash != *current_oxymakefile_hash {
        let locked_str = locked.oxymakefile_hash.as_str();
        let current_str = current_oxymakefile_hash.as_str();
        drifts.push(Drift {
            kind: DriftKind::OxymakefileChanged,
            message: format!(
                "Oxymakefile changed: locked={} current={}",
                &locked_str[..16.min(locked_str.len())],
                &current_str[..16.min(current_str.len())]
            ),
        });
    }

    // 2. Platform.
    let current_os = std::env::consts::OS;
    let current_arch = std::env::consts::ARCH;
    if locked.platform.os != current_os || locked.platform.arch != current_arch {
        drifts.push(Drift {
            kind: DriftKind::PlatformChanged,
            message: format!(
                "Platform changed: locked={}/{} current={}/{}",
                locked.platform.os, locked.platform.arch, current_os, current_arch
            ),
        });
    }

    // 3. Rules.
    let current_rules: BTreeMap<String, &Rule> = rules
        .iter()
        .map(|r| (r.name.as_str().to_string(), r))
        .collect();

    // Check for removed rules.
    for name in locked.rules.keys() {
        if !current_rules.contains_key(name) {
            drifts.push(Drift {
                kind: DriftKind::RuleRemoved,
                message: format!("Rule removed: {name}"),
            });
        }
    }

    // Check for added or changed rules.
    for (name, rule) in &current_rules {
        match locked.rules.get(name) {
            None => {
                drifts.push(Drift {
                    kind: DriftKind::RuleAdded,
                    message: format!("Rule added: {name}"),
                });
            }
            Some(locked_rule) => {
                // Check execution hash.
                let current_exec_hash = crate::writer::hash_execution_block(rule);
                if locked_rule.execution_hash != current_exec_hash {
                    drifts.push(Drift {
                        kind: DriftKind::RuleExecutionChanged,
                        message: format!("Rule '{name}' execution changed"),
                    });
                }

                // Check params hash.
                let current_params_hash = crate::writer::hash_rule_params(rule);
                if locked_rule.params_hash != current_params_hash {
                    drifts.push(Drift {
                        kind: DriftKind::RuleParamsChanged,
                        message: format!("Rule '{name}' parameters changed"),
                    });
                }
            }
        }
    }

    // 4. Input file hashes.
    for (path, locked_hash) in &locked.inputs {
        match current_input_hashes.get(path) {
            None => {
                drifts.push(Drift {
                    kind: DriftKind::InputRemoved,
                    message: format!("Input removed: {path}"),
                });
            }
            Some(current_hash) => {
                if locked_hash != current_hash {
                    drifts.push(Drift {
                        kind: DriftKind::InputChanged,
                        message: format!("Input changed: {path}"),
                    });
                }
            }
        }
    }

    for path in current_input_hashes.keys() {
        if !locked.inputs.contains_key(path) {
            drifts.push(Drift {
                kind: DriftKind::InputAdded,
                message: format!("Input added: {path}"),
            });
        }
    }

    DriftReport { drifts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LockedRule, Lockfile, PlatformInfo, SCHEMA_VERSION};
    use ox_core::model::{ContentHash, ExecutionBlock, ReproducibilityClass, RuleName};

    fn make_rule(name: &str, command: &str) -> Rule {
        Rule {
            name: RuleName(name.into()),
            priority: None,
            inputs: vec![],
            outputs: vec![],
            execution: ExecutionBlock::Shell {
                command: command.into(),
            },
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

    fn exec_hash(command: &str) -> ContentHash {
        crate::writer::hash_execution_block(&make_rule("any", command))
    }

    fn make_locked(rules: &[(&str, &str)]) -> Lockfile {
        let mut locked_rules = BTreeMap::new();
        for (name, command) in rules {
            locked_rules.insert(
                name.to_string(),
                LockedRule {
                    execution_hash: exec_hash(command),
                    environment: None,
                    inputs: vec![],
                    outputs: vec![],
                    params_hash: None,
                },
            );
        }
        Lockfile {
            schema_version: SCHEMA_VERSION,
            created_at: 0,
            oxymakefile_hash: ContentHash::from(blake3::hash(b"abc123")),
            platform: PlatformInfo {
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
            },
            rules: locked_rules,
            environments: BTreeMap::new(),
            inputs: BTreeMap::new(),
        }
    }

    #[test]
    fn no_drift_when_identical() {
        let locked = make_locked(&[("align", "echo hello")]);
        let rules = vec![make_rule("align", "echo hello")];
        let report = compare(
            &locked,
            &ContentHash::from(blake3::hash(b"abc123")),
            &rules,
            &BTreeMap::new(),
        );
        assert!(report.is_clean());
    }

    #[test]
    fn detects_oxymakefile_change() {
        let locked = make_locked(&[("align", "echo hello")]);
        let rules = vec![make_rule("align", "echo hello")];
        let report = compare(
            &locked,
            &ContentHash::from(blake3::hash(b"different_hash_value")),
            &rules,
            &BTreeMap::new(),
        );
        assert!(!report.is_clean());
        assert!(
            report
                .drifts
                .iter()
                .any(|d| d.kind == DriftKind::OxymakefileChanged)
        );
    }

    #[test]
    fn detects_rule_added() {
        let locked = make_locked(&[("align", "echo hello")]);
        let rules = vec![
            make_rule("align", "echo hello"),
            make_rule("sort", "sort input"),
        ];
        let report = compare(
            &locked,
            &ContentHash::from(blake3::hash(b"abc123")),
            &rules,
            &BTreeMap::new(),
        );
        assert!(report.drifts.iter().any(|d| d.kind == DriftKind::RuleAdded));
    }

    #[test]
    fn detects_rule_removed() {
        let locked = make_locked(&[("align", "echo hello"), ("sort", "sort input")]);
        let rules = vec![make_rule("align", "echo hello")];
        let report = compare(
            &locked,
            &ContentHash::from(blake3::hash(b"abc123")),
            &rules,
            &BTreeMap::new(),
        );
        assert!(
            report
                .drifts
                .iter()
                .any(|d| d.kind == DriftKind::RuleRemoved)
        );
    }

    #[test]
    fn detects_execution_change() {
        let locked = make_locked(&[("align", "echo hello")]);
        let rules = vec![make_rule("align", "echo world")];
        let report = compare(
            &locked,
            &ContentHash::from(blake3::hash(b"abc123")),
            &rules,
            &BTreeMap::new(),
        );
        assert!(
            report
                .drifts
                .iter()
                .any(|d| d.kind == DriftKind::RuleExecutionChanged)
        );
    }

    #[test]
    fn detects_input_change() {
        let locked = {
            let mut l = make_locked(&[("align", "echo hello")]);
            l.inputs.insert(
                "data/input.txt".into(),
                ContentHash::from(blake3::hash(b"hash_old")),
            );
            l
        };
        let rules = vec![make_rule("align", "echo hello")];
        let mut current_inputs = BTreeMap::new();
        current_inputs.insert(
            "data/input.txt".into(),
            ContentHash::from(blake3::hash(b"hash_new")),
        );
        let report = compare(
            &locked,
            &ContentHash::from(blake3::hash(b"abc123")),
            &rules,
            &current_inputs,
        );
        assert!(
            report
                .drifts
                .iter()
                .any(|d| d.kind == DriftKind::InputChanged)
        );
    }
}
