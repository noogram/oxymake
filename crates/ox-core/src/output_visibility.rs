//! Existence checks for outputs another host may have just written.
//!
//! On an NFS client, a failed lookup of a path is cached: after a `stat` that
//! found nothing, further `stat` calls on the same path keep failing until
//! the directory's attribute cache expires (between `acdirmin` and
//! `acdirmax`, 30 s and 60 s by default on a Linux NFSv4 client), even after
//! another host has created the file. A
//! `stat` of the parent directory does not help. Listing the parent directory
//! does: it revalidates the directory, and the file becomes visible at once.
//!
//! [`output_exists`] therefore reads the parent directory before concluding
//! that an output is absent, and [`wait_for_outputs`] retries for a bounded
//! time to cover a write that becomes visible slightly later.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long [`wait_for_outputs`] waits, by default, for a missing output to
/// become visible before giving up.
pub const OUTPUT_VISIBILITY_WAIT: Duration = Duration::from_secs(5);

/// Interval between two checks in [`wait_for_outputs`].
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Whether `path` exists, reading its parent directory before reporting it
/// absent so that a cached negative lookup on a network filesystem is
/// revalidated.
///
/// Errors reading the parent directory are ignored; the result is then the
/// plain existence check.
pub fn output_exists(path: &Path) -> bool {
    if path.exists() {
        return true;
    }
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if let Ok(mut entries) = std::fs::read_dir(parent) {
        let _ = entries.next();
    }
    path.exists()
}

/// Wait up to `bound` for every path in `paths` to be visible, checking each
/// with [`output_exists`]. Returns the paths still missing when the bound
/// expires, or an empty vector as soon as all are present.
pub async fn wait_for_outputs(paths: &[PathBuf], bound: Duration) -> Vec<PathBuf> {
    let start = Instant::now();
    loop {
        let missing: Vec<PathBuf> = paths
            .iter()
            .filter(|path| !output_exists(path))
            .cloned()
            .collect();
        let elapsed = start.elapsed();
        if missing.is_empty() || elapsed >= bound {
            return missing;
        }
        tokio::time::sleep(RETRY_INTERVAL.min(bound - elapsed)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_exists_reports_present_and_absent_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("out.txt");
        assert!(!output_exists(&file));
        std::fs::write(&file, "x").unwrap();
        assert!(output_exists(&file));
        assert!(!output_exists(&dir.path().join("missing/out.txt")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wait_returns_once_a_late_output_appears() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("out.txt");
        let writer = {
            let file = file.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                std::fs::write(file, "x").unwrap();
            })
        };
        let start = Instant::now();
        let missing = wait_for_outputs(std::slice::from_ref(&file), Duration::from_secs(5)).await;
        writer.join().unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn wait_gives_up_at_the_bound_and_names_the_missing_output() {
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("present.txt");
        std::fs::write(&present, "x").unwrap();
        let absent = dir.path().join("absent.txt");
        let bound = Duration::from_millis(400);
        let start = Instant::now();
        let missing = wait_for_outputs(&[present, absent.clone()], bound).await;
        let elapsed = start.elapsed();
        assert_eq!(missing, vec![absent]);
        assert!(elapsed >= bound, "{elapsed:?}");
        assert!(elapsed < bound + Duration::from_secs(1), "{elapsed:?}");
    }
}
