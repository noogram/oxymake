//! Built-in [`MaterializationStrategy`] implementations.
//!
//! Three strategies ship with `ox-cache`, one per identity flavor:
//!
//! | Strategy | Flavor | Identify | Verify |
//! |----------|--------|----------|--------|
//! | [`ContentAddressedStrategy`] | Content | BLAKE3 hash of file bytes | Re-hash and compare |
//! | [`ComputationKeyStrategy`] | Computation | Pre-computed cache key | File exists on disk |
//! | [`ExternalRefStrategy`] | External | URI from `OutputRef::Virtual` | Run check command |

use std::path::Path;

use ox_core::model::{ArtifactIdentity, ComputationHash, ExternalRef, OutputRef};
use ox_core::traits::materialization::{IdentityFlavor, MaterializationStrategy};

use crate::error::CacheError;
use crate::hash::hash_file;

// ---------------------------------------------------------------------------
// Content-addressed strategy
// ---------------------------------------------------------------------------

/// Identifies artifacts by BLAKE3 content hash of their bytes.
///
/// - **identify**: Hashes the file referenced by `OutputRef::File`.
/// - **verify**: Re-hashes the file and checks that it matches.
///
/// Only supports `OutputRef::File`.  Calling `identify` or `verify` on a
/// non-file ref returns an error.
#[derive(Debug, Clone, Default)]
pub struct ContentAddressedStrategy;

impl MaterializationStrategy for ContentAddressedStrategy {
    type Error = CacheError;

    async fn identify(&self, output_ref: &OutputRef) -> Result<ArtifactIdentity, Self::Error> {
        let path = file_path(output_ref)?;
        let hash = hash_file(path)?;
        Ok(ArtifactIdentity::Content(hash))
    }

    async fn verify(
        &self,
        output_ref: &OutputRef,
        identity: &ArtifactIdentity,
    ) -> Result<bool, Self::Error> {
        let ArtifactIdentity::Content(expected) = identity else {
            return Ok(false);
        };
        let path = file_path(output_ref)?;
        if !path.exists() {
            return Ok(false);
        }
        let actual = hash_file(path)?;
        Ok(actual == *expected)
    }

    fn flavor(&self) -> IdentityFlavor {
        IdentityFlavor::Content
    }
}

// ---------------------------------------------------------------------------
// Computation-key strategy
// ---------------------------------------------------------------------------

/// Identifies artifacts by a pre-computed computation hash (cache key).
///
/// Unlike [`ContentAddressedStrategy`], this strategy does not hash file
/// bytes. Instead, the caller supplies the cache key at construction time
/// (computed from the rule source, input hashes, params, env, and platform
/// via [`compute_cache_key`](crate::compute_cache_key)).
///
/// - **identify**: Returns the stored computation hash.
/// - **verify**: Checks that the output file still exists on disk.
///   Re-hashing is unnecessary because the computation hash guarantees
///   that the same inputs + code would produce the same result.
///
/// Only supports `OutputRef::File` for verify (existence check).
#[derive(Debug, Clone)]
pub struct ComputationKeyStrategy {
    /// The pre-computed cache key for this computation.
    cache_key: ComputationHash,
}

impl ComputationKeyStrategy {
    /// Create a new strategy with the given computation hash.
    pub fn new(cache_key: ComputationHash) -> Self {
        Self { cache_key }
    }
}

impl MaterializationStrategy for ComputationKeyStrategy {
    type Error = CacheError;

    async fn identify(&self, _output_ref: &OutputRef) -> Result<ArtifactIdentity, Self::Error> {
        Ok(ArtifactIdentity::Computation(self.cache_key.clone()))
    }

    async fn verify(
        &self,
        output_ref: &OutputRef,
        identity: &ArtifactIdentity,
    ) -> Result<bool, Self::Error> {
        let ArtifactIdentity::Computation(expected) = identity else {
            return Ok(false);
        };
        if *expected != self.cache_key {
            return Ok(false);
        }
        // The computation hash matches — just check that the output exists.
        match output_ref {
            OutputRef::File(path) => Ok(path.exists()),
            OutputRef::Virtual { .. } | OutputRef::InMemory { .. } => {
                // Virtual/in-memory outputs are always "present" if the
                // computation matches.
                Ok(true)
            }
        }
    }

    fn flavor(&self) -> IdentityFlavor {
        IdentityFlavor::Computation
    }
}

// ---------------------------------------------------------------------------
// External-ref strategy
// ---------------------------------------------------------------------------

/// Identifies artifacts by an external URI with an optional check command.
///
/// - **identify**: Extracts the URI and check command from
///   `OutputRef::Virtual`.
/// - **verify**: Runs the check command (if present) via `sh -c` and
///   returns `true` if it exits with code 0.  If no check command is
///   stored, verification always succeeds (optimistic).
///
/// Only supports `OutputRef::Virtual`.
///
/// # Security
///
/// `verify` executes the stored check command through `sh -c` — **cache
/// verification is not a read-only operation for this strategy**. The check
/// command originates from the Oxymakefile (`OutputRef::Virtual`), so it is
/// exactly as trusted as the rule commands themselves, but callers exposing
/// verification as a standalone "inspect the cache" surface (e.g. a future
/// `ox cache verify`) must surface this and require explicit opt-in. See
/// SECURITY.md ("Cache integrity"). This strategy is not currently
/// instantiated outside tests.
#[derive(Debug, Clone, Default)]
pub struct ExternalRefStrategy;

impl MaterializationStrategy for ExternalRefStrategy {
    type Error = CacheError;

    async fn identify(&self, output_ref: &OutputRef) -> Result<ArtifactIdentity, Self::Error> {
        match output_ref {
            OutputRef::Virtual { id, check } => {
                let ext = ExternalRef {
                    uri: id.clone(),
                    check: if check.is_empty() {
                        None
                    } else {
                        Some(check.clone())
                    },
                };
                Ok(ArtifactIdentity::External(ext))
            }
            _ => Err(CacheError::Manifest(
                "ExternalRefStrategy requires OutputRef::Virtual".into(),
            )),
        }
    }

    async fn verify(
        &self,
        _output_ref: &OutputRef,
        identity: &ArtifactIdentity,
    ) -> Result<bool, Self::Error> {
        let ArtifactIdentity::External(ext_ref) = identity else {
            return Ok(false);
        };
        match &ext_ref.check {
            Some(cmd) if !cmd.is_empty() => {
                let status = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                    .map_err(CacheError::Io)?;
                Ok(status.success())
            }
            _ => Ok(true), // No check command — optimistic verification.
        }
    }

    fn flavor(&self) -> IdentityFlavor {
        IdentityFlavor::External
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the file path from an `OutputRef::File`, or error.
fn file_path(output_ref: &OutputRef) -> Result<&Path, CacheError> {
    match output_ref {
        OutputRef::File(path) => Ok(path),
        _ => Err(CacheError::Manifest(
            "content-addressed strategy requires OutputRef::File".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic valid ContentHash from a test label (64 hex chars).
    fn ch(label: &str) -> ContentHash {
        let mut hex: String = label.bytes().map(|b| format!("{b:02x}")).collect();
        hex.truncate(64);
        ContentHash::from_hex(format!("{hex:0<64}")).unwrap()
    }

    /// Deterministic valid ComputationHash from a test label (64 hex chars).
    fn cmph(label: &str) -> ComputationHash {
        let mut hex: String = label.bytes().map(|b| format!("{b:02x}")).collect();
        hex.truncate(64);
        ComputationHash::from_hex(format!("{hex:0<64}")).unwrap()
    }
    use ox_core::model::ContentHash;
    use std::path::PathBuf;

    // ── ContentAddressedStrategy ───────────────────────────────────────

    #[tokio::test]
    async fn content_identify_produces_content_hash() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("data.txt");
        std::fs::write(&p, b"hello content").unwrap();

        let strategy = ContentAddressedStrategy;
        let output = OutputRef::File(p);
        let identity = strategy.identify(&output).await.unwrap();

        assert!(matches!(identity, ArtifactIdentity::Content(_)));
        assert_eq!(strategy.flavor(), IdentityFlavor::Content);
    }

    #[tokio::test]
    async fn content_verify_matches() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("data.txt");
        std::fs::write(&p, b"hello verify").unwrap();

        let strategy = ContentAddressedStrategy;
        let output = OutputRef::File(p);
        let identity = strategy.identify(&output).await.unwrap();

        assert!(strategy.verify(&output, &identity).await.unwrap());
    }

    #[tokio::test]
    async fn content_verify_detects_change() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("data.txt");
        std::fs::write(&p, b"version 1").unwrap();

        let strategy = ContentAddressedStrategy;
        let output = OutputRef::File(p.clone());
        let identity = strategy.identify(&output).await.unwrap();

        // Modify the file.
        std::fs::write(&p, b"version 2").unwrap();
        assert!(!strategy.verify(&output, &identity).await.unwrap());
    }

    #[tokio::test]
    async fn content_verify_missing_file() {
        let strategy = ContentAddressedStrategy;
        let output = OutputRef::File(PathBuf::from("/tmp/nonexistent_oxymake_test.txt"));
        let identity = ArtifactIdentity::Content(ch("abc123"));

        assert!(!strategy.verify(&output, &identity).await.unwrap());
    }

    #[tokio::test]
    async fn content_identify_non_file_errors() {
        let strategy = ContentAddressedStrategy;
        let output = OutputRef::Virtual {
            id: "db://test".into(),
            check: "SELECT 1".into(),
        };
        assert!(strategy.identify(&output).await.is_err());
    }

    // ── ComputationKeyStrategy ────────────────────────────────────────

    #[tokio::test]
    async fn computation_identify_returns_key() {
        let key = cmph("abc123def456");
        let strategy = ComputationKeyStrategy::new(key.clone());
        let output = OutputRef::File(PathBuf::from("/tmp/anything"));

        let identity = strategy.identify(&output).await.unwrap();
        assert_eq!(identity, ArtifactIdentity::Computation(key));
        assert_eq!(strategy.flavor(), IdentityFlavor::Computation);
    }

    #[tokio::test]
    async fn computation_verify_checks_existence() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.txt");
        std::fs::write(&p, b"result").unwrap();

        let key = cmph("key123");
        let strategy = ComputationKeyStrategy::new(key.clone());
        let output = OutputRef::File(p);
        let identity = ArtifactIdentity::Computation(key);

        assert!(strategy.verify(&output, &identity).await.unwrap());
    }

    #[tokio::test]
    async fn computation_verify_missing_file_fails() {
        let key = cmph("key123");
        let strategy = ComputationKeyStrategy::new(key.clone());
        let output = OutputRef::File(PathBuf::from("/tmp/nonexistent_oxymake_test.txt"));
        let identity = ArtifactIdentity::Computation(key);

        assert!(!strategy.verify(&output, &identity).await.unwrap());
    }

    #[tokio::test]
    async fn computation_verify_wrong_key_fails() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.txt");
        std::fs::write(&p, b"result").unwrap();

        let strategy = ComputationKeyStrategy::new(cmph("key_A"));
        let output = OutputRef::File(p);
        let identity = ArtifactIdentity::Computation(cmph("key_B"));

        assert!(!strategy.verify(&output, &identity).await.unwrap());
    }

    #[tokio::test]
    async fn computation_verify_virtual_always_ok() {
        let key = cmph("key123");
        let strategy = ComputationKeyStrategy::new(key.clone());
        let output = OutputRef::Virtual {
            id: "db://table".into(),
            check: "".into(),
        };
        let identity = ArtifactIdentity::Computation(key);

        assert!(strategy.verify(&output, &identity).await.unwrap());
    }

    // ── ExternalRefStrategy ───────────────────────────────────────────

    #[tokio::test]
    async fn external_identify_from_virtual() {
        let strategy = ExternalRefStrategy;
        let output = OutputRef::Virtual {
            id: "db://warehouse.results".into(),
            check: "SELECT 1 FROM results LIMIT 1".into(),
        };

        let identity = strategy.identify(&output).await.unwrap();
        match identity {
            ArtifactIdentity::External(ext) => {
                assert_eq!(ext.uri, "db://warehouse.results");
                assert_eq!(ext.check.as_deref(), Some("SELECT 1 FROM results LIMIT 1"));
            }
            _ => panic!("expected External identity"),
        }
        assert_eq!(strategy.flavor(), IdentityFlavor::External);
    }

    #[tokio::test]
    async fn external_identify_non_virtual_errors() {
        let strategy = ExternalRefStrategy;
        let output = OutputRef::File(PathBuf::from("/tmp/test.txt"));
        assert!(strategy.identify(&output).await.is_err());
    }

    #[tokio::test]
    async fn external_verify_true_command() {
        let strategy = ExternalRefStrategy;
        let output = OutputRef::Virtual {
            id: "test".into(),
            check: "true".into(),
        };
        let identity = ArtifactIdentity::External(ExternalRef {
            uri: "test".into(),
            check: Some("true".into()),
        });

        assert!(strategy.verify(&output, &identity).await.unwrap());
    }

    #[tokio::test]
    async fn external_verify_false_command() {
        let strategy = ExternalRefStrategy;
        let output = OutputRef::Virtual {
            id: "test".into(),
            check: "false".into(),
        };
        let identity = ArtifactIdentity::External(ExternalRef {
            uri: "test".into(),
            check: Some("false".into()),
        });

        assert!(!strategy.verify(&output, &identity).await.unwrap());
    }

    #[tokio::test]
    async fn external_verify_no_check_optimistic() {
        let strategy = ExternalRefStrategy;
        let output = OutputRef::Virtual {
            id: "test".into(),
            check: "".into(),
        };
        let identity = ArtifactIdentity::External(ExternalRef {
            uri: "test".into(),
            check: None,
        });

        assert!(strategy.verify(&output, &identity).await.unwrap());
    }

    #[tokio::test]
    async fn external_verify_wrong_flavor_fails() {
        let strategy = ExternalRefStrategy;
        let output = OutputRef::Virtual {
            id: "test".into(),
            check: "".into(),
        };
        let identity = ArtifactIdentity::Content(ch("abc"));

        assert!(!strategy.verify(&output, &identity).await.unwrap());
    }
}
