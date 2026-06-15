//! Lockfile data model.
//!
//! The lockfile is a TOML document capturing the exact state of a workflow run.
//! It enables drift detection on subsequent runs by comparing the current state
//! against the locked state.

use std::collections::BTreeMap;

use ox_core::model::ContentHash;
use serde::{Deserialize, Serialize};

/// Schema version for forward compatibility.
///
/// v2: framed hashes — `execution_hash` includes script file content for
/// script-mode rules, `params_hash` length-frames key/value pairs, and
/// nix environments lock the expression file's content hash.
pub const SCHEMA_VERSION: u32 = 2;

/// The top-level lockfile structure, written to `ox.lock`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Lockfile {
    /// Schema version for forward compatibility.
    pub schema_version: u32,

    /// Unix timestamp (seconds) when the lockfile was generated.
    pub created_at: u64,

    /// BLAKE3 hash of the Oxymakefile content.
    pub oxymakefile_hash: ContentHash,

    /// Platform information at lock time.
    pub platform: PlatformInfo,

    /// Per-rule locked state, keyed by rule name.
    pub rules: BTreeMap<String, LockedRule>,

    /// Environment specifications, keyed by a label like "uv:requirements.txt".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environments: BTreeMap<String, LockedEnvironment>,

    /// Input files and their content hashes at lock time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, ContentHash>,
}

/// Platform information captured at lock time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlatformInfo {
    /// Operating system (e.g., "macos", "linux").
    pub os: String,
    /// CPU architecture (e.g., "aarch64", "x86_64").
    pub arch: String,
}

/// Locked state for a single rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedRule {
    /// BLAKE3 hash of the serialized execution block.
    pub execution_hash: ContentHash,

    /// Environment spec label (references an entry in `environments`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,

    /// Input patterns declared by the rule.
    pub inputs: Vec<String>,

    /// Output patterns declared by the rule.
    pub outputs: Vec<String>,

    /// BLAKE3 hash of the serialized parameters/resources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params_hash: Option<ContentHash>,
}

/// Locked environment specification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedEnvironment {
    /// Environment kind: "system", "uv", "conda", "docker", "nix", "apptainer".
    pub kind: String,

    /// For file-based envs (uv, conda): BLAKE3 hash of the spec file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_hash: Option<ContentHash>,

    /// For image-based envs (docker): the image reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,

    /// For nix: the flake reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flake: Option<String>,
}

#[cfg(test)]
mod forward_compat_tests {
    //! Forward-compatibility guards for `ox.lock` (STATUS.md §7).
    //!
    //! The FAIR forward-compat audit (`fair-forward-compat.md`, 2026-06-14)
    //! rests on one load-bearing claim: a future `ox` can grow the lockfile
    //! with **additive** fields (e.g. an output-hash table, a PROV bundle
    //! pointer) without breaking the v1 contract. These tests make that
    //! claim executable rather than merely asserted — they are the
    //! "self-applied falsifier" the project values (STATUS.md cross-cutting
    //! principles). They guard *oxymake's own* responsibility: reading a
    //! lockfile. They cannot speak for external consumers.

    use super::*;

    /// A lockfile that omits the optional `environments` and `inputs` tables
    /// must parse, filling them from their `#[serde(default)]`. This is the
    /// backward-compat direction: a *new* `ox` reading an *old*, sparser
    /// lockfile must never error on a missing optional table.
    ///
    /// Note: `rules` itself is *not* `#[serde(default)]` today, so the table
    /// header must be present even when empty — see the audit's
    /// "observations" section for this pre-existing wrinkle.
    #[test]
    fn optional_tables_default_when_absent() {
        let toml = r#"
schema_version = 2
created_at = 1700000000
oxymakefile_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[platform]
os = "linux"
arch = "x86_64"

[rules]
"#;
        let lock: Lockfile = toml::from_str(toml).expect("sparse lockfile must parse");
        assert_eq!(lock.schema_version, 2);
        assert!(lock.rules.is_empty());
        assert!(
            lock.environments.is_empty(),
            "absent optional table defaults to empty"
        );
        assert!(lock.inputs.is_empty());
    }

    /// An *unknown future field* — exactly what a v1.1 PROV/RO-Crate
    /// addition looks like — must be **ignored**, not rejected. This is the
    /// forward-compat direction and the crux of the audit's "additive =
    /// non-breaking" verdict. If someone adds `#[serde(deny_unknown_fields)]`
    /// to `Lockfile`, this test goes red and the contract is broken.
    #[test]
    fn unknown_future_field_is_ignored() {
        let toml = r#"
schema_version = 2
created_at = 1700000000
oxymakefile_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
prov_bundle_path = "provenance/ro-crate-metadata.json"

[platform]
os = "linux"
arch = "x86_64"

[platform.future_subtable]
some_v1_1_field = "tolerated"

[rules]
"#;
        let lock: Lockfile =
            toml::from_str(toml).expect("unknown future fields must be tolerated, not rejected");
        assert_eq!(lock.platform.os, "linux");
    }

    /// A `LockedRule` with only its required fields parses — proving the
    /// optional `environment`/`params_hash` are genuinely additive and a
    /// future `output_hashes` table could join them the same way.
    #[test]
    fn locked_rule_optional_fields_default() {
        let toml = r#"
execution_hash = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
inputs = ["a.txt"]
outputs = ["b.txt"]
"#;
        let rule: LockedRule = toml::from_str(toml).expect("sparse rule must parse");
        assert!(rule.environment.is_none());
        assert!(rule.params_hash.is_none());
    }
}

impl Lockfile {
    /// Create a new empty lockfile with current platform info.
    pub fn new(oxymakefile_hash: ContentHash) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            schema_version: SCHEMA_VERSION,
            created_at: now,
            oxymakefile_hash,
            platform: PlatformInfo {
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
            },
            rules: BTreeMap::new(),
            environments: BTreeMap::new(),
            inputs: BTreeMap::new(),
        }
    }
}
