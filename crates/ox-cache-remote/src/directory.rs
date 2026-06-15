//! Local directory backend for the remote cache trait.
//!
//! Stores artifacts as files in a directory tree keyed by content hash.
//! Primarily useful for testing and for single-machine shared caches
//! (e.g., a team NFS mount).
//!
//! ## Layout
//!
//! ```text
//! <root>/
//!   ab/
//!     abcdef0123456789...   (full hash as filename)
//!   cd/
//!     cdef...
//! ```
//!
//! The two-character prefix subdirectory prevents a single directory from
//! accumulating millions of entries (a common filesystem performance issue).

use std::path::{Path, PathBuf};
use std::pin::Pin;

use ox_core::model::ContentHash;
use ox_core::traits::remote_cache::{RemoteCache, RemoteCacheError};

/// A remote cache backed by a local directory.
///
/// Artifacts are stored under a two-level directory structure:
/// `<root>/<first-2-hex-chars>/<full-hash>`.
#[derive(Debug, Clone)]
pub struct DirectoryCache {
    root: PathBuf,
}

impl DirectoryCache {
    /// Create a new directory cache rooted at the given path.
    ///
    /// The directory is created (including parents) on the first `store` call
    /// if it does not already exist.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Return the filesystem path for a given content hash.
    fn artifact_path(&self, key: &ContentHash) -> PathBuf {
        let hex = key.as_str();
        let prefix = &hex[..2.min(hex.len())];
        self.root.join(prefix).join(hex)
    }
}

/// Compute the blake3 hex digest of a file.
async fn blake3_hex(path: &Path) -> std::io::Result<String> {
    let bytes = tokio::fs::read(path).await?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

impl RemoteCache for DirectoryCache {
    fn fetch<'a>(
        &'a self,
        key: &'a ContentHash,
        dest: &'a Path,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bool, RemoteCacheError>> + Send + 'a>>
    {
        Box::pin(async move {
            let src = self.artifact_path(key);
            if !src.exists() {
                return Ok(false);
            }
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::copy(&src, dest).await?;
            // Integrity contract: re-verify the transferred artifact against
            // its content-addressed key. The cache directory is a shared,
            // writable namespace — an entry whose content does not match its
            // advertised hash (tampering, partial write, bit rot) must be
            // treated as absent, never served.
            let actual = blake3_hex(dest).await?;
            if actual != key.as_str() {
                tokio::fs::remove_file(dest).await?;
                return Ok(false);
            }
            Ok(true)
        })
    }

    fn store<'a>(
        &'a self,
        key: &'a ContentHash,
        source: &'a Path,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), RemoteCacheError>> + Send + 'a>> {
        Box::pin(async move {
            let dest = self.artifact_path(key);
            if dest.exists() {
                // Content-addressed: same hash means same content — but
                // only if the existing bytes actually match the key. A
                // truncated entry left by a crashed pre-atomic writer
                // must be repaired, not frozen forever (H21).
                if blake3_hex(&dest).await? == key.as_str() {
                    return Ok(());
                }
            }
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            // Stage to a unique sibling tmp, then rename into place: the
            // cache directory is a shared namespace (team NFS mount), so
            // a crash mid-copy must never leave a partial artifact at
            // the final path, and concurrent writers must not clobber
            // each other's staging files.
            let unique = format!(
                "{}.tmp.{}.{}",
                key.as_str(),
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0),
            );
            let staging = dest.with_file_name(unique);
            tokio::fs::copy(source, &staging).await?;
            if let Err(e) = tokio::fs::rename(&staging, &dest).await {
                let _ = tokio::fs::remove_file(&staging).await;
                return Err(e.into());
            }
            Ok(())
        })
    }

    fn exists<'a>(
        &'a self,
        key: &'a ContentHash,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bool, RemoteCacheError>> + Send + 'a>>
    {
        Box::pin(async move {
            let path = self.artifact_path(key);
            Ok(path.exists())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_hash(s: &str) -> ContentHash {
        // Deterministic valid hash from a label: hex-encode the bytes and
        // pad to the 64 lowercase hex chars that ContentHash validates.
        let mut hex: String = s.bytes().map(|b| format!("{b:02x}")).collect();
        hex.truncate(64);
        ContentHash::from_hex(format!("{hex:0<64}")).unwrap()
    }

    /// Real content-addressed key: the blake3 hash of the artifact bytes.
    /// `fetch` re-verifies content against the key, so tests that exercise
    /// the fetch path must use genuine hashes.
    fn content_key(bytes: &[u8]) -> ContentHash {
        ContentHash::from(blake3::hash(bytes))
    }

    #[tokio::test]
    async fn store_and_fetch_roundtrip() {
        let dir = TempDir::new().unwrap();
        let cache = DirectoryCache::new(dir.path().join("cache"));

        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("artifact.txt");
        tokio::fs::write(&src_file, b"hello world").await.unwrap();

        let key = content_key(b"hello world");

        // Store the artifact.
        cache.store(&key, &src_file).await.unwrap();

        // Fetch it back to a new location.
        let dest = src_dir.path().join("fetched.txt");
        let found = cache.fetch(&key, &dest).await.unwrap();
        assert!(found);

        let content = tokio::fs::read_to_string(&dest).await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn fetch_rejects_poisoned_entry() {
        // A cache entry whose content does not match its advertised hash
        // (tampering, partial write, bit rot) must be treated as absent:
        // fetch returns false and the destination file is removed.
        let dir = TempDir::new().unwrap();
        let cache = DirectoryCache::new(dir.path().join("cache"));

        let key = content_key(b"legitimate artifact");

        // Plant a poisoned entry directly under the key's path.
        let poisoned_path = cache.artifact_path(&key);
        tokio::fs::create_dir_all(poisoned_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&poisoned_path, b"malicious payload!!")
            .await
            .unwrap();

        let dest = dir.path().join("out.txt");
        let found = cache.fetch(&key, &dest).await.unwrap();
        assert!(!found, "poisoned entry must not be served as a hit");
        assert!(!dest.exists(), "poisoned bytes must not be left at dest");
    }

    #[tokio::test]
    async fn store_repairs_truncated_entry() {
        // H21: a writer that crashed mid-copy (pre-atomic-store) left a
        // truncated artifact at the key's final path. A later store of
        // the same key must repair it — an `if exists` early-return
        // would freeze the poison forever, and every peer on the shared
        // cache would see the entry as present yet unusable.
        let dir = TempDir::new().unwrap();
        let cache = DirectoryCache::new(dir.path().join("cache"));

        let key = content_key(b"complete artifact bytes");

        // Plant the truncated entry at the final path.
        let entry = cache.artifact_path(&key);
        tokio::fs::create_dir_all(entry.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&entry, b"complete arti").await.unwrap();

        // Store the genuine content under the same key.
        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("artifact.bin");
        tokio::fs::write(&src_file, b"complete artifact bytes")
            .await
            .unwrap();
        cache.store(&key, &src_file).await.unwrap();

        // The entry must now be fetchable (content matches the key).
        let dest = src_dir.path().join("out.bin");
        let found = cache.fetch(&key, &dest).await.unwrap();
        assert!(found, "store must repair a truncated entry");
        assert_eq!(
            tokio::fs::read(&dest).await.unwrap(),
            b"complete artifact bytes"
        );
    }

    #[tokio::test]
    async fn store_leaves_no_staging_files() {
        // The atomic store stages to a sibling tmp and renames; a
        // successful store must leave exactly the artifact, nothing else.
        let dir = TempDir::new().unwrap();
        let cache = DirectoryCache::new(dir.path().join("cache"));

        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("artifact.bin");
        tokio::fs::write(&src_file, b"bytes").await.unwrap();

        let key = content_key(b"bytes");
        cache.store(&key, &src_file).await.unwrap();

        let parent = cache.artifact_path(&key);
        let parent = parent.parent().unwrap();
        let mut entries = tokio::fs::read_dir(parent).await.unwrap();
        let mut names = Vec::new();
        while let Some(e) = entries.next_entry().await.unwrap() {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(names, vec![key.as_str().to_string()]);
    }

    #[tokio::test]
    async fn fetch_missing_returns_false() {
        let dir = TempDir::new().unwrap();
        let cache = DirectoryCache::new(dir.path().join("cache"));

        let key = test_hash("0000000000000000000000000000000000000000000000000000000000000000");
        let dest = dir.path().join("nope.txt");
        let found = cache.fetch(&key, &dest).await.unwrap();
        assert!(!found);
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn exists_reflects_stored_state() {
        let dir = TempDir::new().unwrap();
        let cache = DirectoryCache::new(dir.path().join("cache"));

        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("data.bin");
        tokio::fs::write(&src_file, b"data").await.unwrap();

        let key = test_hash("1111111111111111111111111111111111111111111111111111111111111111");

        assert!(!cache.exists(&key).await.unwrap());
        cache.store(&key, &src_file).await.unwrap();
        assert!(cache.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn store_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let cache = DirectoryCache::new(dir.path().join("cache"));

        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("artifact.txt");
        tokio::fs::write(&src_file, b"content").await.unwrap();

        let key = test_hash("2222222222222222222222222222222222222222222222222222222222222222");

        cache.store(&key, &src_file).await.unwrap();
        cache.store(&key, &src_file).await.unwrap(); // should not error
        assert!(cache.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn directory_layout_uses_prefix() {
        let dir = TempDir::new().unwrap();
        let cache = DirectoryCache::new(dir.path().join("cache"));

        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("f.txt");
        tokio::fs::write(&src_file, b"x").await.unwrap();

        let hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let key = ContentHash::from_hex(hash).unwrap();
        cache.store(&key, &src_file).await.unwrap();

        // Verify the two-level layout: <root>/ab/<full-hash>
        let expected = dir.path().join("cache").join("ab").join(hash);
        assert!(
            expected.exists(),
            "artifact should be at {}",
            expected.display()
        );
    }
}
