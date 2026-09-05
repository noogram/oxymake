//! Exclusive, non-blocking advisory lock over a job's output set.
//!
//! # Why this exists
//!
//! Two `ox run` sessions approved for the same gate both reach the guarded
//! job, because the cooperative claim protocol (`StateDb::claim_job`,
//! ADR-012) is not yet consulted by the scheduler before dispatch. If both
//! sessions then *execute* the job into the same output paths, their writes
//! interleave and the committed set can mix files from two physical
//! executions — a corrupt, non-reproducible result that is nonetheless
//! reported as success and cached (issue #2, round-2 finding 1).
//!
//! The sound fix is to make the claim the scheduling gate; that is the open
//! follow-up. Until then this lock **fails closed**: the session that does
//! not hold the output-set lock refuses to execute the job concurrently,
//! rather than racing into a mixed set. Exactly one physical execution ever
//! contributes files to a given output set.
//!
//! # Semantics
//!
//! - The lock is keyed on the job's *output set* (the sorted absolute output
//!   paths), so two sessions producing the same outputs contend, while
//!   unrelated jobs never do.
//! - It is **exclusive** and **non-blocking**: [`OutputSetLock::acquire`]
//!   returns `Ok(None)` immediately when a peer holds it, so the caller can
//!   fail loudly instead of blocking.
//! - It is an OS advisory lock (`flock(2)`) tied to an open file
//!   description, so the kernel releases it when the process exits — a
//!   crashed session never strands the lock (no stale-marker problem).
//! - The lock is released when the returned guard is dropped, which the
//!   local executor arranges to happen only after `finalize_workspace`
//!   commits (the guard travels inside the workspace state), so the lock
//!   spans both execution and the atomic output commit.

use std::path::{Path, PathBuf};

/// A held exclusive lock over a job's output set.
///
/// Dropping the guard closes the underlying file descriptor, which releases
/// the `flock(2)`. On non-Unix targets the guard is a best-effort marker
/// file with no kernel enforcement.
#[derive(Debug)]
pub(crate) struct OutputSetLock {
    /// Kept open for the lifetime of the guard: closing the fd releases the
    /// advisory lock.
    _file: std::fs::File,
}

/// Read the PID recorded in the lock file at `lock_path`, best-effort.
///
/// Used only to name the holder in the fail-closed error message; a missing
/// or unreadable value yields `"unknown"` and never changes behaviour.
pub(crate) fn holder_pid(lock_path: &Path) -> String {
    std::fs::read_to_string(lock_path)
        .ok()
        .and_then(|s| s.trim().lines().next().map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Path of the lock file for the given output set, under `work_dir`.
///
/// The name is a stable hash of the sorted absolute output paths, so both
/// sessions running the same job in the same directory compute the same
/// path and therefore contend on the same lock.
pub(crate) fn lock_path_for(work_dir: &Path, output_files: &[PathBuf]) -> PathBuf {
    use std::hash::{Hash, Hasher};

    let mut sorted: Vec<&PathBuf> = output_files.iter().collect();
    sorted.sort();
    // `DefaultHasher::new()` uses fixed keys (unlike `RandomState`), so the
    // digest is stable across processes — required for two sessions to agree
    // on the lock file name.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for p in sorted {
        p.hash(&mut hasher);
    }
    let digest = hasher.finish();
    work_dir
        .join(".oxymake")
        .join("locks")
        .join(format!("{digest:016x}.lock"))
}

impl OutputSetLock {
    /// Try to acquire the exclusive lock for `output_files` under `work_dir`.
    ///
    /// Returns `Ok(Some(guard))` when the lock was acquired, `Ok(None)` when
    /// a concurrent session already holds it (fail-closed signal), or an
    /// error if the lock file could not be created or locked for any other
    /// reason.
    pub(crate) fn acquire(
        work_dir: &Path,
        output_files: &[PathBuf],
    ) -> std::io::Result<Option<Self>> {
        let path = lock_path_for(work_dir, output_files);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&path)?;

        match try_lock_exclusive(&file)? {
            true => {
                // Record our PID so a losing peer can name us. Best-effort;
                // truncate to keep only the current holder's pid.
                use std::io::{Seek, SeekFrom, Write};
                let mut f = &file;
                let _ = f.set_len(0);
                let _ = f.seek(SeekFrom::Start(0));
                let _ = writeln!(f, "{}", std::process::id());
                let _ = f.flush();
                Ok(Some(Self { _file: file }))
            }
            false => Ok(None),
        }
    }
}

/// Attempt a non-blocking exclusive `flock`. Returns `Ok(true)` if acquired,
/// `Ok(false)` if another handle holds it, `Err` on any other failure.
#[cfg(unix)]
fn try_lock_exclusive(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    // Safety: `flock` with a valid, open file descriptor is a standard POSIX
    // syscall with no memory-safety implications.
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => Ok(false),
        _ => Err(err),
    }
}

/// Non-Unix fallback: no kernel-enforced advisory lock is available, so the
/// open itself is treated as success. The cooperative model targets Unix
/// hosts (the executor also uses `killpg` for cancellation there).
#[cfg(not(unix))]
fn try_lock_exclusive(_file: &std::fs::File) -> std::io::Result<bool> {
    Ok(true)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_of_the_same_output_set_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let outputs = vec![dir.path().join("a.txt"), dir.path().join("b.txt")];

        let first = OutputSetLock::acquire(dir.path(), &outputs).unwrap();
        assert!(first.is_some(), "first session must acquire the lock");

        // A concurrent session for the same output set is refused.
        let second = OutputSetLock::acquire(dir.path(), &outputs).unwrap();
        assert!(
            second.is_none(),
            "a second session must not acquire the same output-set lock"
        );

        // The holder's PID is recorded for the fail-closed message.
        let path = lock_path_for(dir.path(), &outputs);
        assert_eq!(holder_pid(&path), std::process::id().to_string());

        // Once the first guard drops, the lock is free again (sequential
        // runs are fine — only concurrent execution is refused).
        drop(first);
        let third = OutputSetLock::acquire(dir.path(), &outputs).unwrap();
        assert!(
            third.is_some(),
            "the lock must be released when the guard is dropped"
        );
    }

    #[test]
    fn different_output_sets_do_not_contend() {
        let dir = tempfile::tempdir().unwrap();
        let set_a = vec![dir.path().join("a.txt")];
        let set_b = vec![dir.path().join("b.txt")];

        let _a = OutputSetLock::acquire(dir.path(), &set_a).unwrap().unwrap();
        let b = OutputSetLock::acquire(dir.path(), &set_b).unwrap();
        assert!(b.is_some(), "unrelated output sets must not contend");
    }

    #[test]
    fn lock_path_is_stable_regardless_of_output_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        assert_eq!(
            lock_path_for(dir.path(), &[a.clone(), b.clone()]),
            lock_path_for(dir.path(), &[b, a]),
        );
    }
}
