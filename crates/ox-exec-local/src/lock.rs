//! Exclusive, non-blocking advisory locks over a job's output paths.
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
//! follow-up. Until then these locks **fail closed**: a session that cannot
//! lock every output path of a job refuses to execute it concurrently,
//! rather than racing into a mixed set. Exactly one physical execution ever
//! writes a given output path at a time.
//!
//! # Semantics
//!
//! - **One lock per output path.** Two jobs contend as soon as their output
//!   sets *intersect* (`{shared, a}` vs `{shared, b}`), not only when they
//!   are equal (round-3 finding 2). Unrelated jobs never contend.
//! - **Filesystem-canonical keys.** The lock name is derived from the
//!   canonicalised deepest existing ancestor of each path, so two lexically
//!   different spellings of the same file (`real/x` vs `alias/x` through a
//!   symlinked directory) map to the same lock.
//! - **Deadlock-free.** Locks are acquired in globally sorted key order, and
//!   every acquisition is **non-blocking**: on the first contended path all
//!   locks already taken are released and [`OutputLocks::acquire`] reports
//!   [`Acquired::Contended`], so the caller fails loudly instead of waiting.
//! - **Tied to a file description.** Each lock is an OS advisory lock
//!   (`flock(2)`) on an open file, released by the kernel when the last
//!   descriptor referring to it closes. The executor hands the job
//!   subprocess an inheritable duplicate of every lock descriptor, so a
//!   lock stays held while *either* the session or the job it spawned is
//!   alive: a `SIGKILL`ed session cannot release a lock its orphaned job
//!   shell still needs (round-3 finding 1).
//! - **Released after commit.** The session's own guards live in the
//!   workspace state and are dropped only after `finalize_workspace`, so a
//!   lock spans execution and the atomic output commit.
//!
//! # Limits
//!
//! The guarantee holds on local filesystems with working `flock(2)`. On
//! NFS and other distributed filesystems `flock` may not be mutually
//! visible between hosts (or at all), and this module cannot detect that;
//! there, two sessions on different hosts are not protected. On targets
//! without `flock` (non-Unix) acquisition fails closed with
//! [`LockError::Unsupported`] rather than pretending to hold a lock
//! (round-3 finding 3).

use std::path::{Path, PathBuf};

/// Why a lock could not be acquired (other than contention, which is
/// reported through [`Acquired::Contended`]).
#[derive(Debug)]
pub(crate) enum LockError {
    /// The platform provides no cross-process advisory lock with the
    /// required semantics; refusing to run rather than pretend.
    // Only constructed by the non-Unix `try_lock_exclusive`.
    #[cfg_attr(unix, allow(dead_code))]
    Unsupported(&'static str),
    /// Creating, opening or locking a lock file failed.
    Io(std::io::Error),
}

impl From<std::io::Error> for LockError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Outcome of [`OutputLocks::acquire`].
#[derive(Debug)]
pub(crate) enum Acquired {
    /// Every output path is locked by this session.
    Held(OutputLocks),
    /// Another session holds the lock of `path`; nothing is held.
    Contended {
        /// The first output path (in lock order) that could not be locked.
        path: PathBuf,
        /// Description of the holder for the fail-closed message, e.g.
        /// `pid 1234` or `pid 1234, which has exited; …`.
        holder: String,
    },
}

/// The held exclusive locks over every output path of one job.
///
/// Dropping the guard closes the session's descriptors; the kernel releases
/// each `flock(2)` once the job subprocess (which inherited duplicates, see
/// [`OutputLocks::raw_fds`]) has exited as well.
#[derive(Debug)]
pub(crate) struct OutputLocks {
    /// One open, locked file per distinct output path. Kept open for the
    /// lifetime of the guard.
    files: Vec<std::fs::File>,
}

/// Read the PID recorded in the lock file at `lock_path`, best-effort.
///
/// A missing or unreadable value yields `None` and never changes behaviour.
pub(crate) fn holder_pid(lock_path: &Path) -> Option<u32> {
    std::fs::read_to_string(lock_path)
        .ok()
        .and_then(|s| s.trim().lines().next().and_then(|l| l.trim().parse().ok()))
}

/// Describe the holder of the lock at `lock_path` for the fail-closed
/// error message.
///
/// When the recorded session is no longer alive, the lock is necessarily
/// held through a descriptor inherited by a job subprocess that outlived
/// it; the message says so, because an operator looking for that PID would
/// otherwise find nothing.
pub(crate) fn holder_description(lock_path: &Path) -> String {
    match holder_pid(lock_path) {
        None => "pid unknown".to_string(),
        Some(pid) if session_alive(pid) => format!("pid {pid}"),
        Some(pid) => format!(
            "pid {pid}, which has exited; a job subprocess it started is still running \
             and holds the lock until it exits"
        ),
    }
}

/// Whether a process with `pid` currently exists (best-effort; `true` when
/// it cannot be determined).
#[cfg(unix)]
fn session_alive(pid: u32) -> bool {
    // SAFETY: `kill(2)` with signal 0 performs only the existence and
    // permission checks and never delivers a signal; it has no
    // memory-safety implications.
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    ret == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn session_alive(_pid: u32) -> bool {
    true
}

/// Filesystem-canonical key of an output path.
///
/// `path` is absolute and lexically normalised (no `.`/`..`) but may not
/// exist yet, and neither may some of its parent directories. The deepest
/// *existing* ancestor is canonicalised (resolving symlinked directories),
/// and the remaining lexical tail is re-appended. Two spellings of the same
/// final location therefore yield the same key.
pub(crate) fn lock_key(path: &Path) -> PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut ancestor = match path.parent() {
        Some(p) => p,
        None => return path.to_path_buf(),
    };
    tail.push(path.file_name().unwrap_or_default().to_os_string());
    loop {
        if let Ok(canonical) = ancestor.canonicalize() {
            let mut key = canonical;
            for component in tail.iter().rev() {
                key.push(component);
            }
            return key;
        }
        match (ancestor.file_name(), ancestor.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_os_string());
                ancestor = parent;
            }
            // Reached a root that does not exist: nothing to resolve.
            _ => return path.to_path_buf(),
        }
    }
}

/// Path of the lock file for one output path, under `work_dir`.
///
/// The name is a stable hash of the path's [`lock_key`], so every session
/// in the same directory computes the same lock file for the same final
/// location and therefore contends on it.
#[cfg(test)]
pub(crate) fn lock_path_for(work_dir: &Path, output: &Path) -> PathBuf {
    lock_path_for_key(work_dir, &lock_key(output))
}

/// Lock file for an already-computed [`lock_key`]; see [`lock_path_for`].
fn lock_path_for_key(work_dir: &Path, key: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};

    // `DefaultHasher::new()` uses fixed keys (unlike `RandomState`), so the
    // digest is stable across processes — required for two sessions to agree
    // on the lock file name.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    let digest = hasher.finish();
    work_dir
        .join(".oxymake")
        .join("locks")
        .join(format!("{digest:016x}.lock"))
}

impl OutputLocks {
    /// Try to lock every path of `output_files` under `work_dir`.
    ///
    /// Paths are locked one by one in globally sorted key order (duplicates
    /// collapsed). On the first path already held by another session, the
    /// locks taken so far are released and [`Acquired::Contended`] names
    /// that path and its holder. Returns [`LockError::Unsupported`] on
    /// platforms without a usable cross-process lock, and
    /// [`LockError::Io`] if a lock file could not be created or locked for
    /// any other reason.
    pub(crate) fn acquire(
        work_dir: &Path,
        output_files: &[PathBuf],
    ) -> Result<Acquired, LockError> {
        let mut keyed: Vec<(PathBuf, &PathBuf)> =
            output_files.iter().map(|p| (lock_key(p), p)).collect();
        keyed.sort();
        keyed.dedup_by(|a, b| a.0 == b.0);

        let mut held = Vec::with_capacity(keyed.len());
        for (key, original) in keyed {
            let path = lock_path_for_key(work_dir, &key);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .read(true)
                .open(&path)?;

            if !try_lock_exclusive(&file)? {
                // Release everything taken so far (in `held`'s drop) before
                // reporting, so a losing session never keeps partial locks.
                drop(held);
                return Ok(Acquired::Contended {
                    path: original.clone(),
                    holder: holder_description(&path),
                });
            }

            // Record our PID so a losing peer can name us. Best-effort;
            // truncate to keep only the current holder's pid.
            use std::io::{Seek, SeekFrom, Write};
            let mut f = &file;
            let _ = f.set_len(0);
            let _ = f.seek(SeekFrom::Start(0));
            let _ = writeln!(f, "{}", std::process::id());
            let _ = f.flush();
            held.push(file);
        }
        Ok(Acquired::Held(Self { files: held }))
    }

    /// Raw descriptors of the held locks, for the job subprocess to inherit
    /// (see [`crate::process::spawn_shell_with_callback`]). A duplicate of
    /// each shares the open file description and therefore the `flock`, so
    /// the lock persists while the child lives even if this session dies.
    #[cfg(unix)]
    pub(crate) fn raw_fds(&self) -> Vec<i32> {
        use std::os::unix::io::AsRawFd;
        self.files.iter().map(|f| f.as_raw_fd()).collect()
    }

    #[cfg(not(unix))]
    pub(crate) fn raw_fds(&self) -> Vec<i32> {
        Vec::new()
    }
}

/// Attempt a non-blocking exclusive `flock`. Returns `Ok(true)` if acquired,
/// `Ok(false)` if another handle holds it, `Err` on any other failure.
#[cfg(unix)]
fn try_lock_exclusive(file: &std::fs::File) -> Result<bool, LockError> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: `flock` with a valid, open file descriptor is a standard POSIX
    // syscall with no memory-safety implications.
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => Ok(false),
        _ => Err(LockError::Io(err)),
    }
}

/// Non-Unix: no kernel-enforced advisory lock with the required
/// cross-process, crash-releasing semantics is available, so acquisition
/// fails closed. Pretending to hold the lock would let two sessions commit
/// a mixed output set (round-3 finding 3).
#[cfg(not(unix))]
fn try_lock_exclusive(_file: &std::fs::File) -> Result<bool, LockError> {
    Err(LockError::Unsupported(
        "output-path locking requires flock(2), which this platform does not provide",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn held(r: Result<Acquired, LockError>) -> OutputLocks {
        match r.unwrap() {
            Acquired::Held(l) => l,
            other => panic!("expected the locks to be acquired, got {other:?}"),
        }
    }

    /// Acquire, retrying briefly. Sibling unit tests spawn job children;
    /// between `fork` and `exec` such a child transiently holds copies of
    /// every descriptor of this process, including a lock another test
    /// just dropped. A genuine leak persists; this window is microseconds.
    fn held_eventually(work_dir: &Path, outputs: &[PathBuf]) -> OutputLocks {
        let start = std::time::Instant::now();
        loop {
            match OutputLocks::acquire(work_dir, outputs).unwrap() {
                Acquired::Held(l) => return l,
                Acquired::Contended { path, holder } => {
                    assert!(
                        start.elapsed() < std::time::Duration::from_secs(2),
                        "lock on {path:?} was not released after drop (holder: {holder})"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
    }

    fn contended(r: Result<Acquired, LockError>) -> (PathBuf, String) {
        match r.unwrap() {
            Acquired::Contended { path, holder } => (path, holder),
            other => panic!("expected contention, got {other:?}"),
        }
    }

    #[test]
    fn second_acquire_of_the_same_output_set_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let outputs = vec![dir.path().join("a.txt"), dir.path().join("b.txt")];

        let first = held(OutputLocks::acquire(dir.path(), &outputs));

        // A concurrent session for the same output set is refused, naming
        // the holder (this process, which is alive).
        let (path, holder) = contended(OutputLocks::acquire(dir.path(), &outputs));
        assert_eq!(path, outputs[0]);
        assert_eq!(holder, format!("pid {}", std::process::id()));
        assert_eq!(
            holder_pid(&lock_path_for(dir.path(), &outputs[0])),
            Some(std::process::id())
        );

        // Once the first guard drops, the lock is free again (sequential
        // runs are fine — only concurrent execution is refused).
        drop(first);
        held_eventually(dir.path(), &outputs);
    }

    #[test]
    fn disjoint_output_sets_do_not_contend() {
        let dir = tempfile::tempdir().unwrap();
        let set_a = vec![dir.path().join("a.txt")];
        let set_b = vec![dir.path().join("b.txt")];

        let _a = held(OutputLocks::acquire(dir.path(), &set_a));
        held(OutputLocks::acquire(dir.path(), &set_b));
    }

    /// Round-3 finding 2: sets that intersect without being equal must
    /// contend — the lock is per path, not per set.
    #[test]
    fn partially_overlapping_output_sets_contend() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.txt");
        let set_a = vec![shared.clone(), dir.path().join("a.txt")];
        let set_b = vec![shared.clone(), dir.path().join("b.txt")];

        let a = held(OutputLocks::acquire(dir.path(), &set_a));
        let (path, _) = contended(OutputLocks::acquire(dir.path(), &set_b));
        assert_eq!(path, shared, "the contended path is the shared one");

        // The loser released the locks it took before hitting the shared
        // path (`b.txt` sorts first): `b.txt` is free for a third party,
        // and once `a` is gone the whole of `set_b` can be taken.
        held_eventually(dir.path(), &[dir.path().join("b.txt")]);
        drop(a);
        held_eventually(dir.path(), &set_b);
    }

    /// Round-3 finding 2: two spellings of one file through a symlinked
    /// directory must map to the same lock.
    #[test]
    fn symlinked_ancestor_aliases_contend() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        let via_real = vec![real.join("shared.txt")];
        let via_alias = vec![alias.join("shared.txt")];
        assert_eq!(
            lock_path_for(dir.path(), &via_real[0]),
            lock_path_for(dir.path(), &via_alias[0]),
        );

        let _a = held(OutputLocks::acquire(dir.path(), &via_real));
        let (path, _) = contended(OutputLocks::acquire(dir.path(), &via_alias));
        assert_eq!(path, via_alias[0]);
    }

    /// A not-yet-existing subdirectory under a symlinked ancestor still
    /// resolves through the deepest existing ancestor.
    #[test]
    fn lock_key_resolves_deepest_existing_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        let expected = real.canonicalize().unwrap().join("sub/deep/out.txt");
        assert_eq!(lock_key(&alias.join("sub/deep/out.txt")), expected);
        assert_eq!(lock_key(&real.join("sub/deep/out.txt")), expected);
    }

    #[test]
    fn the_same_path_listed_twice_does_not_self_contend() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        let outputs = vec![real.join("x"), alias.join("x"), real.join("x")];
        let locks = held(OutputLocks::acquire(dir.path(), &outputs));
        assert_eq!(locks.raw_fds().len(), 1, "duplicates collapse to one lock");
    }

    #[test]
    fn lock_order_is_independent_of_declaration_order() {
        // Acquisition sorts by key, so both sessions try `a` before `b`:
        // whichever wins `a` also gets `b`, and no acquire ever waits.
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        let _first = held(OutputLocks::acquire(dir.path(), &[b.clone(), a.clone()]));
        let (path, _) = contended(OutputLocks::acquire(dir.path(), &[a.clone(), b]));
        assert_eq!(path, a);
    }
}
