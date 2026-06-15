//! Cache key computation.
//!
//! The cache key uniquely identifies a job's computation by hashing together
//! the rule source, all input content hashes (bound to their paths), the
//! parameter hash, environment content hash, shell executable, and platform.
//! This ensures that any change in inputs, code, or environment produces a
//! different key.
//!
//! # Key format v2 (injective framing)
//!
//! Every field is framed via [`ox_core::hashing`] (length-prefixed tag +
//! presence byte + length-prefixed value), so the encoding is injective:
//! two different [`CacheKeySpec`]s can never serialize to the same byte
//! stream. Input hashes are hashed as `(path, hash)` pairs sorted by path,
//! binding each content hash to the path it was computed for. Optional
//! fields (params, env, shell) carry explicit presence tags, so an absent
//! field never collides with an empty or shifted one.
//!
//! The format version tag is the first framed field. Bumping
//! [`CACHE_KEY_FORMAT_VERSION`] changes every key, so caches written under
//! an older format are cleanly missed (never mis-reused).

use blake3::Hasher;
use ox_core::hashing::{update_field, update_opt_field};
use ox_core::model::{ContentHash, EnvSpec};

/// Version tag of the cache key format, hashed into every key.
///
/// Bump this whenever the set of hashed ingredients or their encoding
/// changes: old cache entries then become unreachable (clean invalidation)
/// instead of being wrongly reused under the new semantics.
pub const CACHE_KEY_FORMAT_VERSION: &str = "oxymake-cache-key-v2";

/// All ingredients of a cache key (format v2).
#[derive(Debug, Clone)]
pub struct CacheKeySpec<'a> {
    /// Serialized execution block (command, inline code, script path +
    /// language, or function reference).
    pub rule_source: &'a str,
    /// `(path, content_hash)` pairs for every content-tracked file:
    /// declared inputs, param files, and the script file in script mode.
    /// Order does not matter — pairs are sorted by path before hashing.
    pub inputs: &'a [(String, ContentHash)],
    /// Hash of the resolved wildcard/parameter values, if any.
    pub params_hash: Option<&'a str>,
    /// Content hash of the environment spec (see [`env_spec_content_hash`]).
    pub env_hash: Option<&'a str>,
    /// Shell executable override (e.g. `/bin/zsh`), if any.
    pub shell_executable: Option<&'a str>,
    /// Platform string, e.g. `"linux/x86_64"` (see [`current_platform`]).
    pub platform: &'a str,
}

/// Compute the cache key for a job.
///
/// ```text
/// cache_key = blake3(
///     framed(format_version) ‖
///     framed(rule_source)    ‖
///     framed(inputs as sorted (path, hash) pairs) ‖
///     framed_opt(params_hash) ‖
///     framed_opt(env_hash)    ‖
///     framed_opt(shell_executable) ‖
///     framed(platform)
/// )
/// ```
///
/// Input pairs are sorted by path (then hash) for determinism — the order
/// in which inputs are declared does not affect the cache key.
pub fn compute_cache_key(spec: &CacheKeySpec<'_>) -> ContentHash {
    let mut hasher = Hasher::new();
    update_field(&mut hasher, "format", CACHE_KEY_FORMAT_VERSION.as_bytes());
    update_field(&mut hasher, "rule", spec.rule_source.as_bytes());

    // Sort (path, hash) pairs by path for determinism, binding each
    // content hash to the path it was computed for.
    let mut sorted: Vec<&(String, ContentHash)> = spec.inputs.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_str().cmp(b.1.as_str())));
    update_field(
        &mut hasher,
        "inputs.count",
        &(sorted.len() as u64).to_le_bytes(),
    );
    for (path, hash) in sorted {
        update_field(&mut hasher, "input.path", path.as_bytes());
        update_field(&mut hasher, "input.hash", hash.as_str().as_bytes());
    }

    update_opt_field(&mut hasher, "params", spec.params_hash.map(str::as_bytes));
    update_opt_field(&mut hasher, "env", spec.env_hash.map(str::as_bytes));
    update_opt_field(
        &mut hasher,
        "shell",
        spec.shell_executable.map(str::as_bytes),
    );
    update_field(&mut hasher, "platform", spec.platform.as_bytes());

    ContentHash::from(hasher.finalize())
}

/// The platform string hashed into cache keys: `"<os>/<arch>"`.
///
/// Included so cross-platform caches don't collide (outputs built on
/// macos/aarch64 are not served to linux/x86_64).
pub fn current_platform() -> String {
    format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Hash an environment spec by *content*, not by its literal serialization.
///
/// For specs that reference a file on disk (uv requirements, conda YAML,
/// nix expression), the file's bytes are hashed alongside the reference,
/// so editing `requirements.txt` invalidates the cache even though the
/// spec string is unchanged. When the reference is not a readable file
/// (e.g. a named conda environment), only the literal reference is hashed.
///
/// **Documented divergence**: container image references (`docker:`,
/// `apptainer:`) are hashed as written — a mutable tag like
/// `python:3.12-slim` is *not* resolved to a digest, so re-tagged images
/// do not invalidate the cache. Pin images by digest
/// (`python@sha256:…`) when this matters. See the paper's limitations
/// section.
pub fn env_spec_content_hash(env: &EnvSpec) -> String {
    let mut hasher = Hasher::new();
    match env {
        EnvSpec::System => {
            update_field(&mut hasher, "env.system", b"");
        }
        EnvSpec::Uv { requirements } => {
            update_opt_field(
                &mut hasher,
                "env.uv.requirements",
                requirements.as_deref().map(str::as_bytes),
            );
            let content = requirements.as_deref().and_then(|r| std::fs::read(r).ok());
            update_opt_field(
                &mut hasher,
                "env.uv.requirements_content",
                content.as_deref(),
            );
        }
        EnvSpec::Conda { env } => {
            update_field(&mut hasher, "env.conda.spec", env.as_bytes());
            let content = std::fs::read(env).ok();
            update_opt_field(&mut hasher, "env.conda.content", content.as_deref());
        }
        EnvSpec::Docker { image } => {
            update_field(&mut hasher, "env.docker.image", image.as_bytes());
        }
        EnvSpec::Nix { expr } => {
            update_field(&mut hasher, "env.nix.expr", expr.as_bytes());
            let content = std::fs::read(expr).ok();
            update_opt_field(&mut hasher, "env.nix.content", content.as_deref());
        }
        EnvSpec::Apptainer { image } => {
            update_field(&mut hasher, "env.apptainer.image", image.as_bytes());
        }
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a spec with a fixed platform so tests are machine-independent.
    fn spec<'a>(
        rule: &'a str,
        inputs: &'a [(String, ContentHash)],
        params: Option<&'a str>,
        env: Option<&'a str>,
    ) -> CacheKeySpec<'a> {
        CacheKeySpec {
            rule_source: rule,
            inputs,
            params_hash: params,
            env_hash: env,
            shell_executable: None,
            platform: "linux/x86_64",
        }
    }

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, ContentHash)> {
        items
            .iter()
            .map(|(p, h)| (p.to_string(), ContentHash::from(blake3::hash(h.as_bytes()))))
            .collect()
    }

    #[test]
    fn deterministic_same_inputs() {
        let inputs = pairs(&[("a.txt", "aaa"), ("b.txt", "bbb")]);
        let k1 = compute_cache_key(&spec("echo hello", &inputs, None, None));
        let k2 = compute_cache_key(&spec("echo hello", &inputs, None, None));
        assert_eq!(k1, k2);
    }

    #[test]
    fn order_independent() {
        let inputs_a = pairs(&[("a.txt", "aaa"), ("b.txt", "bbb")]);
        let inputs_b = pairs(&[("b.txt", "bbb"), ("a.txt", "aaa")]);
        let k1 = compute_cache_key(&spec("echo hello", &inputs_a, None, None));
        let k2 = compute_cache_key(&spec("echo hello", &inputs_b, None, None));
        assert_eq!(k1, k2, "input order should not affect cache key");
    }

    #[test]
    fn changes_when_rule_changes() {
        let inputs = pairs(&[("a.txt", "aaa")]);
        let k1 = compute_cache_key(&spec("echo hello", &inputs, None, None));
        let k2 = compute_cache_key(&spec("echo world", &inputs, None, None));
        assert_ne!(k1, k2);
    }

    #[test]
    fn changes_when_inputs_change() {
        let inputs1 = pairs(&[("a.txt", "aaa")]);
        let inputs2 = pairs(&[("a.txt", "bbb")]);
        let k1 = compute_cache_key(&spec("echo hello", &inputs1, None, None));
        let k2 = compute_cache_key(&spec("echo hello", &inputs2, None, None));
        assert_ne!(k1, k2);
    }

    #[test]
    fn changes_with_params() {
        let inputs = pairs(&[("a.txt", "aaa")]);
        let k1 = compute_cache_key(&spec("echo hello", &inputs, None, None));
        let k2 = compute_cache_key(&spec("echo hello", &inputs, Some("param1"), None));
        assert_ne!(k1, k2);
    }

    #[test]
    fn changes_with_env() {
        let inputs = pairs(&[("a.txt", "aaa")]);
        let k1 = compute_cache_key(&spec("echo hello", &inputs, None, None));
        let k2 = compute_cache_key(&spec("echo hello", &inputs, None, Some("env1")));
        assert_ne!(k1, k2);
    }

    #[test]
    fn changes_with_shell_executable() {
        let inputs = pairs(&[("a.txt", "aaa")]);
        let mut s1 = spec("echo hello", &inputs, None, None);
        let mut s2 = s1.clone();
        s1.shell_executable = None;
        s2.shell_executable = Some("/bin/zsh");
        let k1 = compute_cache_key(&s1);
        let k2 = compute_cache_key(&s2);
        assert_ne!(k1, k2, "shell executable must enter the cache key");
    }

    #[test]
    fn changes_with_platform() {
        let inputs = pairs(&[("a.txt", "aaa")]);
        let mut s1 = spec("echo hello", &inputs, None, None);
        let mut s2 = s1.clone();
        s1.platform = "linux/x86_64";
        s2.platform = "macos/aarch64";
        assert_ne!(compute_cache_key(&s1), compute_cache_key(&s2));
    }

    #[test]
    fn empty_inputs() {
        let k = compute_cache_key(&spec("echo hello", &[], None, None));
        assert_eq!(k.as_str().len(), 64);
    }

    // ── Injectivity (domain separation) — audit B1 ─────────────────────

    /// Bytes must not be able to migrate across the rule/params field
    /// boundary: ("ab", params "c") and ("a", params "bc") are different
    /// job specs and must have different keys.
    #[test]
    fn field_boundary_injective() {
        let k1 = compute_cache_key(&spec("ab", &[], Some("c"), None));
        let k2 = compute_cache_key(&spec("a", &[], Some("bc"), None));
        assert_ne!(k1, k2, "rule/params boundary must be framed");
    }

    /// A value in the params slot must not collide with the same value in
    /// the env slot (presence tags on optional fields).
    #[test]
    fn option_slots_injective() {
        let k1 = compute_cache_key(&spec("rule", &[], Some("x"), None));
        let k2 = compute_cache_key(&spec("rule", &[], None, Some("x")));
        assert_ne!(k1, k2, "params and env slots must be domain-separated");
    }

    /// Input hashes must be framed individually: the multisets
    /// {"aaa", "bbb"} and {"aaab", "bb"} concatenate to the same byte
    /// string after sorting but are different input sets.
    #[test]
    fn input_list_injective() {
        let a = pairs(&[("x", "aaa"), ("y", "bbb")]);
        let b = pairs(&[("x", "aaab"), ("y", "bb")]);
        let k1 = compute_cache_key(&spec("rule", &a, None, None));
        let k2 = compute_cache_key(&spec("rule", &b, None, None));
        assert_ne!(k1, k2, "input hash list must be length-framed");
    }

    /// Path↔content binding: swapping which hash belongs to which path
    /// must change the key, even though the multiset of hashes is equal.
    #[test]
    fn path_content_binding() {
        let a = pairs(&[("x", "aaa"), ("y", "bbb")]);
        let b = pairs(&[("x", "bbb"), ("y", "aaa")]);
        let k1 = compute_cache_key(&spec("rule", &a, None, None));
        let k2 = compute_cache_key(&spec("rule", &b, None, None));
        assert_ne!(k1, k2, "hashes must be bound to their paths");
    }

    /// Absent params must differ from present-but-empty params.
    #[test]
    fn none_differs_from_empty() {
        let k1 = compute_cache_key(&spec("rule", &[], None, None));
        let k2 = compute_cache_key(&spec("rule", &[], Some(""), None));
        assert_ne!(k1, k2, "None and Some(\"\") must hash differently");
    }

    // ── Format stability — golden key ──────────────────────────────────

    /// Golden key for a fixed spec. If this test fails, the cache key
    /// format changed: bump [`CACHE_KEY_FORMAT_VERSION`] (so old caches
    /// are cleanly invalidated) and update this constant.
    #[test]
    fn golden_key_stability() {
        let inputs = pairs(&[
            ("data/a.csv", "1111111111111111"),
            ("data/b.csv", "2222222222222222"),
        ]);
        let key = compute_cache_key(&CacheKeySpec {
            rule_source: r#"{"type":"shell","command":"echo hello"}"#,
            inputs: &inputs,
            params_hash: Some("0123456789abcdef"),
            env_hash: Some("fedcba9876543210"),
            shell_executable: Some("/bin/bash"),
            platform: "linux/x86_64",
        });
        assert_eq!(
            key.as_str(),
            "0c6d7ae4141e84f4f836e4a90e0a5d9b1752bdc8b89d9f4bdcb85b88967a09e5",
            "cache key format drifted — bump CACHE_KEY_FORMAT_VERSION and update the golden value"
        );
    }

    // ── Environment content hashing — audit H4 ─────────────────────────

    #[test]
    fn env_hash_tracks_requirements_content() {
        let dir = tempfile::tempdir().unwrap();
        let req = dir.path().join("requirements.txt");
        std::fs::write(&req, "numpy==1.0").unwrap();
        let env = EnvSpec::Uv {
            requirements: Some(req.display().to_string()),
        };

        let h1 = env_spec_content_hash(&env);
        std::fs::write(&req, "numpy==2.0").unwrap();
        let h2 = env_spec_content_hash(&env);

        assert_ne!(h1, h2, "requirements content must enter the env hash");
    }

    #[test]
    fn env_hash_distinguishes_variants() {
        let docker = EnvSpec::Docker {
            image: "python:3.12".into(),
        };
        let apptainer = EnvSpec::Apptainer {
            image: "python:3.12".into(),
        };
        assert_ne!(
            env_spec_content_hash(&docker),
            env_spec_content_hash(&apptainer),
            "same reference under different env kinds must differ"
        );
    }

    #[test]
    fn env_hash_missing_file_falls_back_to_literal() {
        let env = EnvSpec::Conda {
            env: "named-env-not-a-file".into(),
        };
        // Must not panic and must be deterministic.
        assert_eq!(env_spec_content_hash(&env), env_spec_content_hash(&env));
    }

    // ── Property tests ─────────────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for a hex-like content hash string.
        fn hash_str() -> impl Strategy<Value = String> {
            "[a-f0-9]{8,16}"
        }

        /// Strategy for (path, hash) input pairs with distinct paths.
        fn input_pairs() -> impl Strategy<Value = Vec<(String, ContentHash)>> {
            proptest::collection::btree_map("[a-z/.]{1,12}", hash_str(), 0..8).prop_map(|m| {
                m.into_iter()
                    .map(|(p, h)| (p, ContentHash::from(blake3::hash(h.as_bytes()))))
                    .collect()
            })
        }

        proptest! {
            /// Cache key is deterministic: same inputs always produce the same key.
            #[test]
            fn deterministic(
                rule in "[a-z ]{1,30}",
                inputs in input_pairs(),
                params in proptest::option::of("[a-z0-9]{1,10}"),
                env in proptest::option::of("[a-z0-9]{1,10}"),
                shell in proptest::option::of("[a-z/]{1,12}"),
            ) {
                let s = CacheKeySpec {
                    rule_source: &rule,
                    inputs: &inputs,
                    params_hash: params.as_deref(),
                    env_hash: env.as_deref(),
                    shell_executable: shell.as_deref(),
                    platform: "linux/x86_64",
                };
                prop_assert_eq!(compute_cache_key(&s), compute_cache_key(&s));
            }

            /// Cache key is order-independent: any permutation of input pairs
            /// produces the same key.
            #[test]
            fn order_independent(
                rule in "[a-z ]{1,30}",
                inputs in input_pairs(),
            ) {
                let mut reversed = inputs.clone();
                reversed.reverse();
                let k1 = compute_cache_key(&spec(&rule, &inputs, None, None));
                let k2 = compute_cache_key(&spec(&rule, &reversed, None, None));
                prop_assert_eq!(k1, k2, "key should be independent of input order");
            }

            /// Changing the rule source changes the cache key.
            #[test]
            fn different_rules_differ(
                rule1 in "[a-z]{1,10}",
                rule2 in "[a-z]{1,10}",
                inputs in input_pairs(),
            ) {
                prop_assume!(rule1 != rule2);
                let k1 = compute_cache_key(&spec(&rule1, &inputs, None, None));
                let k2 = compute_cache_key(&spec(&rule2, &inputs, None, None));
                prop_assert_ne!(k1, k2);
            }

            /// Injectivity on the input axis: two different (path → hash)
            /// maps always produce different keys.
            #[test]
            fn different_inputs_differ(
                rule in "[a-z]{1,10}",
                inputs1 in input_pairs(),
                inputs2 in input_pairs(),
            ) {
                prop_assume!(inputs1 != inputs2);
                let k1 = compute_cache_key(&spec(&rule, &inputs1, None, None));
                let k2 = compute_cache_key(&spec(&rule, &inputs2, None, None));
                prop_assert_ne!(k1, k2);
            }

            /// Injectivity across the optional slots: differing
            /// (params, env, shell) triples produce different keys.
            #[test]
            fn different_option_slots_differ(
                rule in "[a-z]{1,10}",
                params1 in proptest::option::of("[a-z0-9]{0,10}"),
                env1 in proptest::option::of("[a-z0-9]{0,10}"),
                shell1 in proptest::option::of("[a-z/]{0,10}"),
                params2 in proptest::option::of("[a-z0-9]{0,10}"),
                env2 in proptest::option::of("[a-z0-9]{0,10}"),
                shell2 in proptest::option::of("[a-z/]{0,10}"),
            ) {
                prop_assume!((& params1, &env1, &shell1) != (&params2, &env2, &shell2));
                let s1 = CacheKeySpec {
                    rule_source: &rule,
                    inputs: &[],
                    params_hash: params1.as_deref(),
                    env_hash: env1.as_deref(),
                    shell_executable: shell1.as_deref(),
                    platform: "linux/x86_64",
                };
                let s2 = CacheKeySpec {
                    params_hash: params2.as_deref(),
                    env_hash: env2.as_deref(),
                    shell_executable: shell2.as_deref(),
                    ..s1.clone()
                };
                prop_assert_ne!(compute_cache_key(&s1), compute_cache_key(&s2));
            }

            /// Cache key is always a 64-character hex string (blake3 output).
            #[test]
            fn key_is_valid_hex_64(
                rule in ".*",
                inputs in input_pairs(),
                params in proptest::option::of(".*"),
                env in proptest::option::of(".*"),
            ) {
                let key = compute_cache_key(&spec(
                    &rule,
                    &inputs,
                    params.as_deref(),
                    env.as_deref(),
                ));
                prop_assert_eq!(key.as_str().len(), 64);
                prop_assert!(
                    key.as_str().chars().all(|c| c.is_ascii_hexdigit()),
                    "key should be hex, got: {}", key.as_str()
                );
            }
        }
    }
}
