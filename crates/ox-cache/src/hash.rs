//! File hashing utilities using BLAKE3.
//!
//! Provides content hashing of files and an mtime-based fast path to skip
//! hashing when file metadata hasn't changed.

use std::path::Path;

use blake3::Hasher;
use ox_core::model::ContentHash;

use crate::error::CacheError;

/// Compute the BLAKE3 hash of a file's contents.
///
/// Reads the file in 16 KiB chunks to limit memory usage on large files.
pub fn hash_file(path: &Path) -> Result<ContentHash, CacheError> {
    let mut hasher = Hasher::new();
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; 16 * 1024];
    loop {
        use std::io::Read;
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(ContentHash::from(hasher.finalize()))
}

/// Fast-path: check if a file's mtime (nanoseconds since epoch) and size
/// match the cached values. Returns `true` if the metadata matches, meaning
/// the file very likely hasn't changed and can be skipped for re-hashing.
pub fn mtime_matches(path: &Path, cached_mtime: u64, cached_size: u64) -> bool {
    match file_meta(path) {
        Ok((mtime, size)) => mtime == cached_mtime && size == cached_size,
        Err(_) => false,
    }
}

/// Get file metadata (mtime in nanoseconds since epoch, size in bytes).
///
/// Uses nanosecond precision for mtime to minimize the window for undetected
/// same-size edits. APFS (macOS) and ext4/btrfs (Linux) provide nanosecond
/// timestamps; filesystems with coarser resolution simply have zeros in the
/// sub-second portion, which is still correct (just a wider collision window).
pub fn file_meta(path: &Path) -> Result<(u64, u64), CacheError> {
    let meta = std::fs::metadata(path)?;
    let duration = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Store as nanoseconds. u64 holds nanos up to year ~2554.
    let mtime_nanos = duration.as_secs() * 1_000_000_000 + u64::from(duration.subsec_nanos());
    let size = meta.len();
    Ok((mtime_nanos, size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn hash_file_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hello.txt");
        std::fs::write(&p, b"hello world").unwrap();

        let h1 = hash_file(&p).unwrap();
        let h2 = hash_file(&p).unwrap();
        assert_eq!(h1, h2, "same file should produce the same hash");
        // The hash should be a 64-char hex string (BLAKE3 produces 256 bits).
        assert_eq!(h1.as_str().len(), 64);
    }

    #[test]
    fn hash_file_changes_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("data.txt");
        std::fs::write(&p, b"version 1").unwrap();
        let h1 = hash_file(&p).unwrap();

        std::fs::write(&p, b"version 2").unwrap();
        let h2 = hash_file(&p).unwrap();
        assert_ne!(h1, h2, "different content should produce different hashes");
    }

    #[test]
    fn mtime_matches_positive() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"abc").unwrap();
        let (mt, sz) = file_meta(&p).unwrap();
        assert!(mtime_matches(&p, mt, sz));
    }

    #[test]
    fn mtime_matches_negative_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"abc").unwrap();
        let (mt, _sz) = file_meta(&p).unwrap();
        assert!(!mtime_matches(&p, mt, 9999));
    }

    #[test]
    fn mtime_matches_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nonexistent.txt");
        assert!(!mtime_matches(&p, 0, 0));
    }

    #[test]
    fn file_meta_works() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.txt");
        {
            let mut f = std::fs::File::create(&p).unwrap();
            f.write_all(b"12345").unwrap();
        }
        let (mt, sz) = file_meta(&p).unwrap();
        assert_eq!(sz, 5);
        assert!(mt > 0);
    }
}
