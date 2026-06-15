//! # Async Disk Writer
//!
//! Background tokio task that drains in-memory output data to disk for
//! caching and reproducibility. This is part of Stage 2 (In-Memory Critical
//! Path + Async Disk) of the execution optimization roadmap.
//!
//! ## Architecture
//!
//! The disk writer runs as a background `tokio::spawn` task alongside the
//! scheduler loop. When a job produces output data in memory (via
//! `MaterializationSet`), the scheduler sends a `DiskWriteRequest` through
//! an MPSC channel. The background task drains the channel and writes each
//! output to disk without blocking the scheduler's critical path.
//!
//! ```text
//! Scheduler ──► DiskWriter (mpsc sender)
//!                   │
//!                   ▼
//!             Background Task (mpsc receiver)
//!                   │
//!                   ▼
//!              Filesystem
//! ```
//!
//! ## Consistency guarantees
//!
//! - Writes are atomic per file: data is written to a temporary file in the
//!   same directory (unique name per attempt), fsynced, then renamed to the
//!   target path; the parent directory is fsynced after the rename. This
//!   prevents partial writes from being visible to downstream jobs or the
//!   cache, including across concurrent sessions and crashes.
//! - To flush: drop all `DiskWriterHandle` clones (closes the MPSC channel),
//!   then `.await` the `JoinHandle` returned by [`spawn_disk_writer`]. The
//!   background task drains remaining requests before exiting. The CLI does
//!   this in `run.rs` after the scheduler completes.
//! - On crash before flush, partially written temp files are left behind.
//!   They use a `.oxytmp` suffix and can be cleaned up on the next run.
//!
//! ## Integration with MaterializePolicy
//!
//! The caller (scheduler) is responsible for checking `MaterializePolicy`
//! before sending a write request:
//!
//! - `Always` / `Auto` / `Final` → send write request
//! - `Never` → skip (data stays in memory only)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{Notify, mpsc};

use crate::error::{ExecError, OxError};
use crate::model::JobId;

// ---------------------------------------------------------------------------
// Request type
// ---------------------------------------------------------------------------

/// A request to write in-memory output data to disk.
///
/// Sent from the scheduler to the background disk-writer task via an MPSC
/// channel. Each request represents one output file that was produced in
/// memory and needs to be persisted.
#[derive(Debug)]
pub struct DiskWriteRequest {
    /// The job that produced this output (for diagnostics/logging).
    pub job_id: JobId,
    /// Target path on disk where the data should be written.
    pub target_path: PathBuf,
    /// The output data to write. Shared ownership allows the scheduler to
    /// keep a reference for in-memory consumers while the disk writer
    /// flushes to disk concurrently.
    pub data: Arc<[u8]>,
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Counters tracking disk writer throughput and errors.
#[derive(Debug, Default)]
pub struct DiskWriterStats {
    /// Number of files successfully written.
    pub writes_completed: AtomicU64,
    /// Number of write failures (I/O errors).
    pub writes_failed: AtomicU64,
    /// Total bytes written to disk.
    pub bytes_written: AtomicU64,
}

impl DiskWriterStats {
    fn record_success(&self, bytes: u64) {
        self.writes_completed.fetch_add(1, Ordering::Relaxed);
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
    }

    fn record_failure(&self) {
        self.writes_failed.fetch_add(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Pending-write tracking (H7)
// ---------------------------------------------------------------------------

/// Tracks target paths whose writes have been enqueued but not yet
/// completed (successfully or not).
///
/// The scheduler's cache check hashes input files that the disk writer may
/// still be flushing; waiting on this registry before hashing prevents the
/// torn read (H7). Paths are registered in `DiskWriterHandle::write` before
/// the request is enqueued and released by the background loop after the
/// write lands or fails.
#[derive(Debug, Default)]
struct PendingWrites {
    /// Pending count per target path (a path can be enqueued twice).
    paths: std::sync::Mutex<HashMap<PathBuf, usize>>,
    /// Woken whenever a write completes.
    notify: Notify,
}

impl PendingWrites {
    fn register(&self, path: &std::path::Path) {
        let mut map = self.paths.lock().expect("pending writes lock poisoned");
        *map.entry(path.to_path_buf()).or_insert(0) += 1;
    }

    fn complete(&self, path: &std::path::Path) {
        {
            let mut map = self.paths.lock().expect("pending writes lock poisoned");
            if let Some(count) = map.get_mut(path) {
                *count -= 1;
                if *count == 0 {
                    map.remove(path);
                }
            }
        }
        self.notify.notify_waiters();
    }

    fn any_pending(&self, paths: &[PathBuf]) -> bool {
        let map = self.paths.lock().expect("pending writes lock poisoned");
        paths.iter().any(|p| map.contains_key(p))
    }
}

// ---------------------------------------------------------------------------
// Handle (sender side)
// ---------------------------------------------------------------------------

/// Handle for sending write requests to the background disk-writer task.
///
/// This is the scheduler-facing API. It is cheap to clone (wraps an MPSC
/// sender and shared stats). Dropping all handles causes the background
/// task to drain remaining requests and exit.
#[derive(Debug, Clone)]
pub struct DiskWriterHandle {
    tx: mpsc::Sender<DiskWriteRequest>,
    stats: Arc<DiskWriterStats>,
    pending: Arc<PendingWrites>,
}

impl DiskWriterHandle {
    /// Enqueue a write request. Returns an error if the background task
    /// has already exited (channel closed).
    pub async fn write(&self, request: DiskWriteRequest) -> Result<(), OxError> {
        // Register before enqueueing so a waiter can never observe the
        // request as neither pending nor written.
        let path = request.target_path.clone();
        self.pending.register(&path);
        self.tx.send(request).await.map_err(|_| {
            // The request never reached the loop — release waiters.
            self.pending.complete(&path);
            OxError::Exec(ExecError::Executor {
                message: "disk writer channel closed".into(),
            })
        })
    }

    /// Wait until none of the given paths has a write in flight (H7).
    ///
    /// When this returns, every previously enqueued write to these paths
    /// has completed (or failed) — hashing them cannot observe a torn or
    /// missing file. Paths never enqueued return immediately.
    pub async fn wait_for_paths(&self, paths: &[PathBuf]) {
        loop {
            // Create the Notified future BEFORE checking, so a completion
            // between the check and the await still wakes us.
            let notified = self.pending.notify.notified();
            if !self.pending.any_pending(paths) {
                return;
            }
            notified.await;
        }
    }

    /// Snapshot of the writer's stats.
    pub fn stats(&self) -> &DiskWriterStats {
        &self.stats
    }

    /// Shared handle to the writer's stats, usable after the writer is
    /// flushed (all handles dropped). The CLI consults `writes_failed`
    /// at the final flush to fail the run on lost outputs (H14).
    pub fn stats_arc(&self) -> Arc<DiskWriterStats> {
        self.stats.clone()
    }
}

// ---------------------------------------------------------------------------
// Background task
// ---------------------------------------------------------------------------

/// Suffix used for temporary files during atomic writes.
const TEMP_SUFFIX: &str = ".oxytmp";

/// Spawn the background disk-writer task. Returns a handle for sending
/// requests and a `JoinHandle` for awaiting completion.
///
/// The background task runs until the sender half is dropped (or all
/// `DiskWriterHandle` clones are dropped). After the channel closes, it
/// drains any remaining buffered requests and exits.
///
/// # Arguments
///
/// * `buffer_size` — MPSC channel capacity. Controls backpressure: if the
///   channel is full, `DiskWriterHandle::write` will await until a slot
///   opens. A reasonable default is 64–256.
pub fn spawn_disk_writer(buffer_size: usize) -> (DiskWriterHandle, tokio::task::JoinHandle<()>) {
    spawn_disk_writer_confined(buffer_size, None)
}

/// Spawn the background disk-writer task with an optional workspace
/// confinement root (H13).
///
/// When `workspace_root` is set, every target path is resolved against it
/// (relative targets are joined to the root) and must stay inside it:
/// `../` traversal is rejected lexically before any directory is created,
/// and symlink escapes are rejected by canonicalizing the parent directory
/// after creation. Rejected targets count as write failures. Target paths
/// come from the Oxymakefile, which is untrusted input — a shared workflow
/// file must not be able to write outside the project workspace.
pub fn spawn_disk_writer_confined(
    buffer_size: usize,
    workspace_root: Option<PathBuf>,
) -> (DiskWriterHandle, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(buffer_size);
    let stats = Arc::new(DiskWriterStats::default());
    let pending = Arc::new(PendingWrites::default());

    let handle = DiskWriterHandle {
        tx,
        stats: stats.clone(),
        pending: pending.clone(),
    };

    let join_handle = tokio::spawn(disk_writer_loop(rx, stats, pending, workspace_root));

    (handle, join_handle)
}

/// The background loop that drains write requests from the channel.
async fn disk_writer_loop(
    mut rx: mpsc::Receiver<DiskWriteRequest>,
    stats: Arc<DiskWriterStats>,
    pending: Arc<PendingWrites>,
    workspace_root: Option<PathBuf>,
) {
    // Canonicalize the root once so symlink checks compare like with like
    // (e.g. /tmp vs /private/tmp on macOS).
    let confinement = match workspace_root {
        Some(root) => {
            let canonical = tokio::fs::canonicalize(&root)
                .await
                .unwrap_or_else(|_| root.clone());
            Some((root, canonical))
        }
        None => None,
    };

    while let Some(req) = rx.recv().await {
        let outcome = async {
            let target = match &confinement {
                Some((root, canonical_root)) => {
                    match confine_target(root, canonical_root, &req.target_path).await {
                        Ok(t) => t,
                        Err(e) => {
                            return Err(format!(
                                "disk_writer: refusing to write {} for job {}: {}",
                                req.target_path.display(),
                                req.job_id,
                                e
                            ));
                        }
                    }
                }
                None => req.target_path.clone(),
            };
            write_atomic(&target, &req.data).await.map_err(|e| {
                format!(
                    "disk_writer: failed to write {} for job {}: {}",
                    target.display(),
                    req.job_id,
                    e
                )
            })
        }
        .await;

        match outcome {
            Ok(()) => stats.record_success(req.data.len() as u64),
            Err(msg) => {
                // Log the error but don't propagate — disk writes are
                // best-effort from the scheduler's perspective. The
                // in-memory materialization remains available for
                // downstream consumers. The failure counter is consulted
                // at the final flush (H14) so lost outputs fail the run
                // instead of vanishing silently.
                stats.record_failure();
                eprintln!("{msg}");
            }
        }

        // Release waiters (H7) — keyed on the path as requested, success
        // or failure alike: the waiter needs "no longer in flight", not
        // "succeeded".
        pending.complete(&req.target_path);
    }
    // Channel closed — all handles dropped. Any buffered requests above
    // have been drained by the while-let loop.
}

/// Flush the disk writer and surface persistence failures (H14).
///
/// Drops the handle (the caller must have dropped every other clone first,
/// or the channel never closes), awaits the background task so all buffered
/// requests drain, then checks the failure counter: any write that did not
/// land means run outputs are missing on disk, and the run must fail rather
/// than report success with silently lost outputs.
pub async fn flush_disk_writer(
    handle: DiskWriterHandle,
    join: tokio::task::JoinHandle<()>,
) -> Result<(), OxError> {
    let stats = handle.stats_arc();
    drop(handle);
    let _ = join.await;
    let failed = stats.writes_failed.load(Ordering::Relaxed);
    if failed > 0 {
        return Err(OxError::Exec(ExecError::Executor {
            message: format!(
                "{failed} output file(s) failed to persist to disk — run outputs \
                 are incomplete (see disk_writer errors above)"
            ),
        }));
    }
    Ok(())
}

/// Lexically normalize a path: resolve `.` and `..` components without
/// touching the filesystem. `..` at the root is preserved (and will fail
/// the containment check).
fn lexical_normalize(path: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve and validate a write target against the workspace root (H13).
///
/// Two-layer defense:
/// 1. **Lexical**: the target (joined to the root if relative) is normalized
///    without filesystem access and must start with the root — this rejects
///    `../` traversal *before* any directory is created.
/// 2. **Physical**: after creating the parent directory, its canonicalized
///    path must start with the canonicalized root — this rejects escapes
///    through symlinks placed inside the workspace.
async fn confine_target(
    root: &std::path::Path,
    canonical_root: &std::path::Path,
    target: &std::path::Path,
) -> Result<PathBuf, OxError> {
    let abs = if target.is_absolute() {
        target.to_path_buf()
    } else {
        root.join(target)
    };
    let norm = lexical_normalize(&abs);
    if !(norm.starts_with(root) || norm.starts_with(canonical_root)) {
        return Err(OxError::Exec(ExecError::Executor {
            message: format!(
                "target {} escapes the workspace root {}",
                target.display(),
                root.display()
            ),
        }));
    }

    // Create the parent now so it can be canonicalized for the symlink check.
    if let Some(parent) = norm.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            OxError::Exec(ExecError::Executor {
                message: format!(
                    "failed to create parent directory {}: {}",
                    parent.display(),
                    e
                ),
            })
        })?;
        let canon_parent = tokio::fs::canonicalize(parent).await.map_err(|e| {
            OxError::Exec(ExecError::Executor {
                message: format!(
                    "failed to canonicalize parent directory {}: {}",
                    parent.display(),
                    e
                ),
            })
        })?;
        if !canon_parent.starts_with(canonical_root) {
            return Err(OxError::Exec(ExecError::Executor {
                message: format!(
                    "target {} resolves through a symlink outside the workspace root {}",
                    target.display(),
                    root.display()
                ),
            }));
        }
    }

    Ok(norm)
}

/// Monotonic counter making temp names unique within this process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a temp path for a write attempt: same directory as the target
/// (required for an atomic rename), unique per attempt (B6).
///
/// The name embeds the process id and a process-local counter so two
/// concurrent `ox run` sessions — or two attempts within one session —
/// never interleave their writes in a shared temp file. The `.oxytmp`
/// suffix is kept last so crash leftovers remain identifiable for cleanup.
fn temp_path_for(target: &std::path::Path) -> PathBuf {
    let pid = std::process::id();
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = target.file_name().map(|s| s.to_owned()).unwrap_or_default();
    name.push(format!(".{pid}.{n}{TEMP_SUFFIX}"));
    target.with_file_name(name)
}

/// Write data to a file atomically and durably: write to a unique temp
/// file, fsync it, rename over the target, then fsync the parent directory.
///
/// This ensures that the target path either contains the complete data or
/// doesn't exist — no partial writes are visible — and that after a crash
/// the rename itself is not lost (B6: without the fsync, a truncated file
/// could appear at the final path and be consumed as a valid output).
async fn write_atomic(target: &std::path::Path, data: &[u8]) -> Result<(), OxError> {
    use tokio::io::AsyncWriteExt;

    // Ensure the parent directory exists.
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            OxError::Exec(ExecError::Executor {
                message: format!(
                    "failed to create parent directory {}: {}",
                    parent.display(),
                    e
                ),
            })
        })?;
    }

    let temp_path = temp_path_for(target);

    // Write to the temp file and fsync before the rename — the rename must
    // never publish a file whose data blocks are still in the page cache.
    let write_result = async {
        let mut file = tokio::fs::File::create(&temp_path).await?;
        file.write_all(data).await?;
        file.sync_all().await?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    if let Err(e) = write_result {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(OxError::Exec(ExecError::Executor {
            message: format!("failed to write temp file {}: {}", temp_path.display(), e),
        }));
    }

    // Atomic rename to target.
    if let Err(e) = tokio::fs::rename(&temp_path, target).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(OxError::Exec(ExecError::Executor {
            message: format!(
                "failed to rename {} -> {}: {}",
                temp_path.display(),
                target.display(),
                e
            ),
        }));
    }

    // Fsync the parent directory so the rename (directory entry) itself
    // survives a crash. Unix only: directories cannot be opened as files
    // on Windows. Best-effort — some filesystems reject directory fsync.
    #[cfg(unix)]
    if let Some(parent) = target.parent() {
        if let Ok(dir) = tokio::fs::File::open(parent).await {
            let _ = dir.sync_all().await;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    #[tokio::test]
    async fn write_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("output.dat");
        let data: Arc<[u8]> = Arc::from(b"hello world" as &[u8]);

        let (handle, join) = spawn_disk_writer(16);

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("test-job"),
                target_path: target.clone(),
                data: data.clone(),
            })
            .await
            .unwrap();

        // Drop handle to close channel, then wait for background task.
        drop(handle);
        join.await.unwrap();

        let written = std::fs::read(&target).unwrap();
        assert_eq!(written, b"hello world");
    }

    #[tokio::test]
    async fn write_multiple_files() {
        let dir = tempfile::tempdir().unwrap();

        let (handle, join) = spawn_disk_writer(16);

        for i in 0..5 {
            let target = dir.path().join(format!("out_{i}.dat"));
            let data: Arc<[u8]> = Arc::from(format!("data-{i}").into_bytes());
            handle
                .write(DiskWriteRequest {
                    job_id: JobId::from("multi-job"),
                    target_path: target,
                    data,
                })
                .await
                .unwrap();
        }

        drop(handle);
        join.await.unwrap();

        for i in 0..5 {
            let content = std::fs::read_to_string(dir.path().join(format!("out_{i}.dat"))).unwrap();
            assert_eq!(content, format!("data-{i}"));
        }
    }

    #[tokio::test]
    async fn stats_track_writes() {
        let dir = tempfile::tempdir().unwrap();

        let (handle, join) = spawn_disk_writer(16);
        let stats = handle.stats.clone();

        for i in 0..3 {
            let target = dir.path().join(format!("s_{i}.dat"));
            let data: Arc<[u8]> = Arc::from(vec![0u8; 100]);
            handle
                .write(DiskWriteRequest {
                    job_id: JobId::from("stats-job"),
                    target_path: target,
                    data,
                })
                .await
                .unwrap();
        }

        drop(handle);
        join.await.unwrap();

        assert_eq!(stats.writes_completed.load(Ordering::Relaxed), 3);
        assert_eq!(stats.bytes_written.load(Ordering::Relaxed), 300);
        assert_eq!(stats.writes_failed.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested").join("deep").join("output.dat");

        let (handle, join) = spawn_disk_writer(16);

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("nested-job"),
                target_path: target.clone(),
                data: Arc::from(b"nested data" as &[u8]),
            })
            .await
            .unwrap();

        drop(handle);
        join.await.unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "nested data");
    }

    #[tokio::test]
    async fn atomic_write_no_partial_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("atomic.dat");

        // Write atomically.
        write_atomic(&target, b"complete data").await.unwrap();

        // No temp file should remain in the directory.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(TEMP_SUFFIX))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );

        // Target should have complete data.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "complete data");
    }

    /// H13: a target that lexically escapes the workspace root (`../`)
    /// must be rejected — an Oxymakefile is untrusted input and must not
    /// write outside the workspace.
    #[tokio::test]
    async fn traversal_target_outside_root_is_rejected() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();

        let (handle, join) = spawn_disk_writer_confined(16, Some(root.clone()));
        let stats = handle.stats_arc();

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("evil-job"),
                target_path: PathBuf::from("../escape.dat"),
                data: Arc::from(b"pwned" as &[u8]),
            })
            .await
            .unwrap();

        drop(handle);
        join.await.unwrap();

        assert_eq!(stats.writes_failed.load(Ordering::Relaxed), 1);
        assert!(
            !outer.path().join("escape.dat").exists(),
            "file must not be written outside the workspace root"
        );
    }

    /// H13: an absolute target outside the root is rejected too.
    #[tokio::test]
    async fn absolute_target_outside_root_is_rejected() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let escape = outer.path().join("abs-escape.dat");

        let (handle, join) = spawn_disk_writer_confined(16, Some(root.clone()));
        let stats = handle.stats_arc();

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("evil-job"),
                target_path: escape.clone(),
                data: Arc::from(b"pwned" as &[u8]),
            })
            .await
            .unwrap();

        drop(handle);
        join.await.unwrap();

        assert_eq!(stats.writes_failed.load(Ordering::Relaxed), 1);
        assert!(!escape.exists());
    }

    /// H13: a symlinked directory inside the root that points outside must
    /// not be followed.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escape_is_rejected() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let outside = outer.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let (handle, join) = spawn_disk_writer_confined(16, Some(root.clone()));
        let stats = handle.stats_arc();

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("evil-job"),
                target_path: root.join("link").join("escape.dat"),
                data: Arc::from(b"pwned" as &[u8]),
            })
            .await
            .unwrap();

        drop(handle);
        join.await.unwrap();

        assert_eq!(stats.writes_failed.load(Ordering::Relaxed), 1);
        assert!(!outside.join("escape.dat").exists());
    }

    /// H13: legitimate targets — relative and absolute — inside the root
    /// still work, including nested directories.
    #[tokio::test]
    async fn confined_writes_inside_root_succeed() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();

        let (handle, join) = spawn_disk_writer_confined(16, Some(root.clone()));
        let stats = handle.stats_arc();

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("good-job"),
                target_path: PathBuf::from("sub/dir/out.dat"),
                data: Arc::from(b"relative" as &[u8]),
            })
            .await
            .unwrap();
        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("good-job"),
                target_path: root.join("abs.dat"),
                data: Arc::from(b"absolute" as &[u8]),
            })
            .await
            .unwrap();

        drop(handle);
        join.await.unwrap();

        assert_eq!(stats.writes_failed.load(Ordering::Relaxed), 0);
        assert_eq!(
            std::fs::read_to_string(root.join("sub/dir/out.dat")).unwrap(),
            "relative"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("abs.dat")).unwrap(),
            "absolute"
        );
    }

    /// B6 (a): the temp path must be unique per write attempt. A
    /// deterministic `target + ".oxytmp"` name is shared between concurrent
    /// `ox run` sessions writing the same output — their writes interleave
    /// in the SAME temp file and the rename publishes a chimera.
    #[test]
    fn temp_paths_are_unique_per_attempt() {
        let target = PathBuf::from("/some/dir/output.dat");
        let t1 = temp_path_for(&target);
        let t2 = temp_path_for(&target);
        assert_ne!(t1, t2, "two attempts must not share a temp path");
        // Both stay in the target's directory (required for atomic rename)
        // and keep the .oxytmp suffix (required for crash cleanup).
        assert_eq!(t1.parent(), target.parent());
        assert_eq!(t2.parent(), target.parent());
        assert!(t1.to_string_lossy().ends_with(TEMP_SUFFIX));
        assert!(t2.to_string_lossy().ends_with(TEMP_SUFFIX));
    }

    /// B6 (a): two concurrent atomic writes to the same target must each
    /// publish a complete payload — the final content is exactly one of the
    /// two, never an interleaved chimera.
    #[tokio::test]
    async fn concurrent_writes_same_target_no_chimera() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("contended.dat");

        let a = vec![b'a'; 1 << 20];
        let b = vec![b'b'; 1 << 20];

        let (ta, tb) = (target.clone(), target.clone());
        let (ra, rb) = tokio::join!(
            tokio::spawn(async move { write_atomic(&ta, &a).await }),
            tokio::spawn(async move { write_atomic(&tb, &b).await }),
        );
        ra.unwrap().unwrap();
        rb.unwrap().unwrap();

        let content = std::fs::read(&target).unwrap();
        assert_eq!(content.len(), 1 << 20);
        let all_same = content.windows(2).all(|w| w[0] == w[1]);
        assert!(
            all_same,
            "target contains interleaved data from two write sessions"
        );
    }

    #[tokio::test]
    async fn stats_track_failures() {
        // Write to a path where the parent can't be created (read-only).
        // On macOS/Linux we can use /dev/null/impossible.
        let (handle, join) = spawn_disk_writer(16);
        let stats = handle.stats.clone();

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("fail-job"),
                target_path: PathBuf::from("/dev/null/impossible/output.dat"),
                data: Arc::from(b"will fail" as &[u8]),
            })
            .await
            .unwrap();

        drop(handle);
        join.await.unwrap();

        assert_eq!(stats.writes_completed.load(Ordering::Relaxed), 0);
        assert_eq!(stats.writes_failed.load(Ordering::Relaxed), 1);
    }

    /// H7: `wait_for_paths` must not return while a write to one of the
    /// paths is still in flight — when it returns, the full content is on
    /// disk, so a subsequent cache hash cannot observe a torn/missing file.
    #[tokio::test]
    async fn wait_for_paths_blocks_until_write_lands() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("input.dat");
        let data = vec![b'x'; 4 << 20];
        let len = data.len();

        let (handle, join) = spawn_disk_writer(16);

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("upstream-job"),
                target_path: target.clone(),
                data: Arc::from(data),
            })
            .await
            .unwrap();

        handle.wait_for_paths(&[target.clone()]).await;

        // The wait returned — the file must be fully on disk NOW, while
        // the writer is still alive (no flush yet).
        assert_eq!(
            std::fs::read(&target).unwrap().len(),
            len,
            "wait_for_paths returned before the write landed"
        );

        drop(handle);
        join.await.unwrap();
    }

    /// H7: waiting on paths with no pending writes returns immediately.
    #[tokio::test]
    async fn wait_for_paths_returns_immediately_when_nothing_pending() {
        let (handle, join) = spawn_disk_writer(16);
        tokio::time::timeout(
            Duration::from_secs(1),
            handle.wait_for_paths(&[PathBuf::from("/nowhere/never-written.dat")]),
        )
        .await
        .expect("wait must not block on paths that were never enqueued");
        drop(handle);
        join.await.unwrap();
    }

    /// H7: a FAILED write must also release its waiters — otherwise a
    /// disk-full error turns the scheduler's cache check into a hang.
    #[tokio::test]
    async fn wait_for_paths_released_on_write_failure() {
        let target = PathBuf::from("/dev/null/impossible/input.dat");
        let (handle, join) = spawn_disk_writer(16);

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("doomed-job"),
                target_path: target.clone(),
                data: Arc::from(b"lost" as &[u8]),
            })
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), handle.wait_for_paths(&[target]))
            .await
            .expect("failed write must release waiters");

        drop(handle);
        join.await.unwrap();
    }

    /// H14: a persistence failure must surface as an error at the final
    /// flush — previously it was an eprintln plus a counter nothing read,
    /// so a full disk meant every output lost while the run reported
    /// success.
    #[tokio::test]
    async fn flush_fails_when_writes_failed() {
        let (handle, join) = spawn_disk_writer(16);

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("doomed-job"),
                target_path: PathBuf::from("/dev/null/impossible/output.dat"),
                data: Arc::from(b"lost" as &[u8]),
            })
            .await
            .unwrap();

        let result = flush_disk_writer(handle, join).await;
        assert!(
            result.is_err(),
            "flush must fail when outputs were not persisted"
        );
    }

    /// H14: a clean flush returns Ok.
    #[tokio::test]
    async fn flush_succeeds_when_all_writes_landed() {
        let dir = tempfile::tempdir().unwrap();
        let (handle, join) = spawn_disk_writer(16);

        handle
            .write(DiskWriteRequest {
                job_id: JobId::from("ok-job"),
                target_path: dir.path().join("fine.dat"),
                data: Arc::from(b"fine" as &[u8]),
            })
            .await
            .unwrap();

        flush_disk_writer(handle, join).await.unwrap();
        assert!(dir.path().join("fine.dat").exists());
    }

    #[tokio::test]
    async fn dropped_handle_closes_channel() {
        let (handle, join) = spawn_disk_writer(16);

        // Drop immediately — background task should exit cleanly.
        drop(handle);
        join.await.unwrap();
    }

    #[tokio::test]
    async fn write_after_abort_returns_error() {
        let (handle, join) = spawn_disk_writer(16);

        // Abort the background task — the receiver is dropped.
        join.abort();
        let _ = join.await;

        // Channel receiver is gone — write should fail.
        let result = handle
            .write(DiskWriteRequest {
                job_id: JobId::from("late-job"),
                target_path: PathBuf::from("/tmp/never.dat"),
                data: Arc::from(b"too late" as &[u8]),
            })
            .await;

        assert!(result.is_err());
    }
}
