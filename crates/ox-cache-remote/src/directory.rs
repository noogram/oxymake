//! Local directory backend for the remote cache trait.
//!
//! Stores artifacts as files in a directory tree keyed by content hash.
//! Primarily useful for testing and for single-machine shared caches
//! (e.g., a team NFS mount).
//!
//! Computation entries live under `entries/<prefix>/<key>.json`, independently
//! of content-addressed blobs. Callers publish an entry after all its blobs.
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

use ox_core::output_visibility::output_exists;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncReadExt;

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);
const MAX_ENTRY_BYTES: u64 = 1024 * 1024;

use ox_core::model::ContentHash;
use ox_core::traits::remote_cache::{RemoteCache, RemoteCacheError};

/// A remote cache backed by a local directory.
///
/// Artifacts are stored under a two-level directory structure:
/// `<root>/<first-2-hex-chars>/<full-hash>`.
/// Computation metadata is stored separately under `entries/`. Writers of
/// this directory are trusted producers for runs using its entries.
#[derive(Debug, Clone)]
pub struct DirectoryCache {
    root: PathBuf,
    run_id: String,
}

impl DirectoryCache {
    /// Create a new directory cache rooted at the given path.
    ///
    /// The directory is created (including parents) on the first `store` call
    /// if it does not already exist.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_run_id(root, format!("run-{}", uuid::Uuid::new_v4()))
    }

    /// Use the publishing invocation's ADR-020 identity for staging files.
    pub fn with_run_id(root: impl Into<PathBuf>, run_id: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            run_id: run_id.into(),
        }
    }

    fn entry_path(&self, key: &ContentHash) -> PathBuf {
        self.root
            .join("entries")
            .join(&key.as_str()[..2])
            .join(format!("{}.json", key.as_str()))
    }

    fn staging_path(&self, dest: &Path) -> PathBuf {
        dest.with_file_name(format!(
            "{}.tmp.{}.{}",
            dest.file_name().unwrap().to_string_lossy(),
            self.run_id,
            STAGING_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
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
    #[allow(clippy::type_complexity)] // Object-safe async entry transport.
    fn fetch_entry<'a>(
        &'a self,
        key: &'a ContentHash,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<Vec<u8>>, RemoteCacheError>> + Send + 'a,
        >,
    > {
        Box::pin(async move {
            let path = self.entry_path(key);
            if !output_exists(&path) {
                return Ok(None);
            }
            let file = tokio::fs::File::open(&path).await?;
            if file.metadata().await?.len() > MAX_ENTRY_BYTES {
                return Err(RemoteCacheError::transport_msg(
                    "remote entry exceeds 1 MiB",
                ));
            }
            // Bound the read as well: the file may have changed since metadata.
            let mut bytes = Vec::new();
            file.take(MAX_ENTRY_BYTES + 1)
                .read_to_end(&mut bytes)
                .await?;
            if bytes.len() as u64 > MAX_ENTRY_BYTES {
                return Err(RemoteCacheError::transport_msg(
                    "remote entry exceeds 1 MiB",
                ));
            }
            Ok(Some(bytes))
        })
    }

    fn store_entry<'a>(
        &'a self,
        key: &'a ContentHash,
        bytes: &'a [u8],
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), RemoteCacheError>> + Send + 'a>> {
        Box::pin(async move {
            if bytes.len() as u64 > MAX_ENTRY_BYTES {
                return Err(RemoteCacheError::transport_msg(
                    "remote entry exceeds 1 MiB",
                ));
            }
            let dest = self.entry_path(key);
            tokio::fs::create_dir_all(dest.parent().unwrap()).await?;
            let staging = self.staging_path(&dest);
            let result = async {
                tokio::fs::write(&staging, bytes).await?;
                tokio::fs::rename(&staging, &dest).await
            }
            .await;
            if result.is_err() {
                let _ = tokio::fs::remove_file(&staging).await;
            }
            result.map_err(Into::into)
        })
    }

    fn fetch<'a>(
        &'a self,
        key: &'a ContentHash,
        dest: &'a Path,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bool, RemoteCacheError>> + Send + 'a>>
    {
        Box::pin(async move {
            let src = self.artifact_path(key);
            if !output_exists(&src) {
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
            let staging = self.staging_path(&dest);
            if let Err(e) = tokio::fs::copy(source, &staging).await {
                let _ = tokio::fs::remove_file(&staging).await;
                return Err(e.into());
            }
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
            Ok(output_exists(&path))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn entry_roundtrip_and_concurrent_publishers_leave_no_staging() {
        let dir = TempDir::new().unwrap();
        let a = DirectoryCache::new(dir.path());
        let b = DirectoryCache::new(dir.path());
        let key = content_key(b"computation");
        let first = br#"{"outputs": [1]}"#;
        let second = br#"{"outputs": [2]}"#;
        assert!(a.fetch_entry(&key).await.unwrap().is_none());
        a.store_entry(&key, first).await.unwrap();
        assert_eq!(a.fetch_entry(&key).await.unwrap().unwrap(), first);
        let (x, y) = tokio::join!(a.store_entry(&key, first), b.store_entry(&key, second));
        x.unwrap();
        y.unwrap();
        let bytes = a.fetch_entry(&key).await.unwrap().unwrap();
        assert!(bytes == first || bytes == second);
        let parent = dir.path().join("entries").join(&key.as_str()[..2]);
        let names: Vec<_> = std::fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from(format!("{}.json", key.as_str()))]
        );
    }

    #[tokio::test]
    async fn oversize_entry_is_refused() {
        let dir = TempDir::new().unwrap();
        let cache = DirectoryCache::new(dir.path());
        let key = content_key(b"computation");
        let parent = dir.path().join("entries").join(&key.as_str()[..2]);
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::write(
            parent.join(format!("{}.json", key.as_str())),
            vec![0; 1024 * 1024 + 1],
        )
        .unwrap();
        assert!(cache.fetch_entry(&key).await.is_err());
    }

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
