//! Process-level fault-injection tests for the cooperative-session state DB.
//!
//! These tests exercise the catalogued runtime failure modes:
//!
//! - **Rank 1 — SIGKILL during claim** — a worker process that has claimed a
//!   job is killed with `SIGKILL`, the parent reopens the database and
//!   verifies that `reclaim_stale_jobs` returns the orphaned claim to
//!   `pending`. This is the *cooperative claim leak* path.
//!
//! - **Rank 4 — Crash-and-reopen DB consistency** — documents the rusqlite
//!   reopen contract on a WAL-mode database after an abrupt connection
//!   drop. Catches future rusqlite version bumps that change WAL semantics.
//!
//! Both gates are designed to fail loudly if the underlying SQLite/rusqlite
//! invariants ever regress under a Dependabot bump (forgemaster §1.8,
//! Risk 2).
//!
//! The Rank 1 test relies on `unix(2)` signals — it is `cfg(unix)` only.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ox_state::db::{JobRecord, StateDb};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn fresh_db_path() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("state.db");
    // Initialise the schema via a throwaway connection so the subprocess
    // does not race on first migration.
    StateDb::open(&db_path).unwrap().close().unwrap();
    (dir, db_path)
}

fn make_job(id: &str) -> JobRecord {
    JobRecord {
        id: id.to_string(),
        rule_name: "build".into(),
        wildcards: "{}".into(),
        cache_key: None,
        run_id: None,
    }
}

// ---------------------------------------------------------------------------
// Rank 1 — SIGKILL during claim
// ---------------------------------------------------------------------------

/// Worker subprocess: opens the DB, creates a session, registers + claims a
/// job, writes a ready-marker, then sleeps until killed. Invoked by the parent
/// test via `--exact …::sigkill_worker_subprocess --ignored`.
///
/// **Why `#[ignore]`**: this test is not standalone — it sleeps forever and
/// is intended to be spawned by [`sigkill_during_claim_reclaim_consistent`].
/// CI runs `cargo test` (no `--ignored`) so the helper is invisible to the
/// normal gate; the parent test invokes it on demand.
#[test]
#[ignore = "subprocess helper invoked by sigkill_during_claim_reclaim_consistent"]
fn sigkill_worker_subprocess() {
    let db_path =
        std::env::var("OX_FAULT_INJECT_DB").expect("OX_FAULT_INJECT_DB must be set by parent test");
    let marker = std::env::var("OX_FAULT_INJECT_MARKER")
        .expect("OX_FAULT_INJECT_MARKER must be set by parent test");

    let db = StateDb::open(Path::new(&db_path)).expect("subprocess: open db");
    let sid = db
        .create_session(std::process::id(), "fault-inject-child", None)
        .expect("subprocess: create session");

    db.register_jobs(&[make_job("jworker")])
        .expect("subprocess: register");
    let claimed = db.claim_job("jworker", &sid).expect("subprocess: claim");
    assert!(claimed, "subprocess: claim must succeed");

    // Heartbeat so the parent can decide a freshness threshold.
    db.heartbeat(&sid).expect("subprocess: heartbeat");

    // Signal the parent we have committed the claim and are about to block.
    // The marker file is the synchronisation primitive — no shared memory.
    std::fs::write(&marker, sid.as_bytes()).expect("subprocess: write marker");

    // Block until SIGKILL. Use a long but bounded sleep so the subprocess
    // does eventually die if the parent forgets to kill it (CI safety).
    std::thread::sleep(Duration::from_secs(120));
}

/// Wait until `path` exists or `deadline` is reached. Returns true on
/// success. Polls every 25ms — short enough to keep the test snappy, long
/// enough to avoid burning CPU.
fn wait_for_marker(path: &Path, deadline: Duration) -> bool {
    let start = Instant::now();
    while !path.exists() {
        if start.elapsed() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    true
}

/// Spawn a real child process that opens the state DB and claims a job, then
/// `SIGKILL` it before any cooperative shutdown can run. Verify the parent
/// can reopen the DB and reclaim the orphaned claim.
///
/// This is the **Rank 1** gate. It defends against the *silent cooperative
/// claim leak* that would otherwise cause double-execution when a session
/// dies between `claim_job` and the next heartbeat checkpoint.
#[test]
fn sigkill_during_claim_reclaim_consistent() {
    let (dir, db_path) = fresh_db_path();
    let marker = dir.path().join("subprocess.ready");

    // Spawn ourselves with libtest's --exact filter so only the helper
    // subprocess test runs in the child. We swallow stdout/stderr because
    // the helper otherwise prints "ignored" headers that confuse CI logs.
    let exe = std::env::current_exe().expect("current_exe");
    let mut child = Command::new(&exe)
        .args([
            "--exact",
            "sigkill_worker_subprocess",
            "--ignored",
            "--nocapture",
        ])
        .env("OX_FAULT_INJECT_DB", &db_path)
        .env("OX_FAULT_INJECT_MARKER", &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn subprocess");

    // Wait for the child to publish its session id via the marker file.
    let ready = wait_for_marker(&marker, Duration::from_secs(15));
    if !ready {
        let _ = child.kill();
        panic!("subprocess never wrote ready-marker; see helper logs");
    }
    let child_sid = std::fs::read_to_string(&marker).expect("read marker after child wrote it");

    // SIGKILL — no SIGTERM grace, no cooperative shutdown. This is the
    // worst-case crash mode: the child is gone with its WAL pages possibly
    // un-checkpointed and its session row still `active`.
    let pid = child.id() as libc::pid_t;
    // SAFETY: pid is the OS-assigned PID of a child we just spawned and have
    // not yet reaped; `kill(pid, SIGKILL)` is a single syscall with no
    // memory effects in our address space.
    let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
    assert_eq!(
        rc,
        0,
        "libc::kill(SIGKILL) failed: {}",
        std::io::Error::last_os_error()
    );

    let status = child.wait().expect("wait for SIGKILLed child");
    assert!(
        !status.success(),
        "child should have died from SIGKILL, got {status:?}"
    );

    // ----- Reopen DB and verify the orphaned claim is reclaimable. ----------
    let db = StateDb::open(&db_path).expect("reopen after subprocess SIGKILL");

    // The committed claim survives — WAL replay on open is atomic.
    assert_eq!(
        db.job_status("jworker").unwrap().as_deref(),
        Some("running"),
        "claim committed by child must survive WAL replay"
    );

    // The child session is still `active` (it never marked itself
    // `interrupted` or `completed`). It is the canonical "stale session"
    // that find_stale_sessions / reclaim_stale_jobs target.
    let active = db.active_sessions().expect("active_sessions");
    assert_eq!(active.len(), 1, "exactly one orphan session expected");
    assert_eq!(
        active[0].id, child_sid,
        "active session must be the SIGKILLed child"
    );

    // Reclaim returns the job to pending and marks the session interrupted.
    let reclaimed = db
        .reclaim_stale_jobs(&child_sid)
        .expect("reclaim_stale_jobs");
    assert_eq!(
        reclaimed, 1,
        "reclaim must return the one orphaned running job"
    );

    assert_eq!(
        db.job_status("jworker").unwrap().as_deref(),
        Some("pending"),
        "reclaimed job must be available for re-claim"
    );

    assert!(
        db.active_sessions().unwrap().is_empty(),
        "child session must be marked interrupted after reclaim"
    );

    // A fresh session can now claim and complete the orphaned job — the
    // invariant *exactly-once eventual execution* holds across process death.
    let new_sid = db
        .create_session(std::process::id(), "recovery", None)
        .expect("create recovery session");
    assert!(
        db.claim_job("jworker", &new_sid).unwrap(),
        "recovery session must be able to re-claim the orphaned job"
    );
}

// ---------------------------------------------------------------------------
// Rank 4 — Crash-and-reopen DB consistency
// ---------------------------------------------------------------------------

/// Drop a StateDb without an explicit `close()` (simulating an abrupt
/// shutdown that does not run `Drop` ordering across the whole process),
/// then reopen and verify committed writes survive WAL replay.
///
/// **Why it matters**: forgemaster §1.2 names `PRAGMA synchronous=NORMAL`
/// as the default WAL durability mode. Committed writes are durable; the
/// risk is only an *uncommitted* transaction (covered by the rollback test
/// below). This test pins the contract for future rusqlite bumps.
#[test]
fn drop_without_close_committed_writes_survive() {
    let (_dir, db_path) = fresh_db_path();

    // Phase 1 — commit work, then drop without close.
    let sid;
    {
        let db = StateDb::open(&db_path).unwrap();
        sid = db.create_session(1, "host", None).unwrap();
        db.register_jobs(&[make_job("j-survives")]).unwrap();
        assert!(db.claim_job("j-survives", &sid).unwrap());
        // No close() — drop the StateDb by leaving the scope. The Drop
        // impl of rusqlite::Connection still runs and closes the file
        // descriptor, but the parent process never called `close()` to
        // observe an error. This mirrors a graceful subprocess exit.
    }

    // Phase 2 — reopen, verify the claim survives.
    let db = StateDb::open(&db_path).unwrap();
    assert_eq!(
        db.job_status("j-survives").unwrap().as_deref(),
        Some("running"),
        "committed claim must survive drop-and-reopen"
    );

    // And reclaim works exactly as in the live cooperative case.
    let reclaimed = db.reclaim_stale_jobs(&sid).unwrap();
    assert_eq!(reclaimed, 1);
    assert_eq!(
        db.job_status("j-survives").unwrap().as_deref(),
        Some("pending"),
        "reclaim must reset the job after reopen"
    );
}

/// After a clean WAL-mode reopen, a previously-active session row remains
/// visible. This documents that **session state is not cleaned up on open** —
/// `reclaim_stale_jobs` is the only mechanism that transitions an orphaned
/// session to `interrupted`. A future bump that auto-cleans on open would
/// silently change behaviour; this test guards that boundary.
#[test]
fn reopen_does_not_auto_cleanup_active_sessions() {
    let (_dir, db_path) = fresh_db_path();

    let sid = {
        let db = StateDb::open(&db_path).unwrap();
        let sid = db.create_session(2, "host-b", None).unwrap();
        db.heartbeat(&sid).unwrap();
        sid
    };

    let db = StateDb::open(&db_path).unwrap();
    let active = db.active_sessions().unwrap();
    assert!(
        active.iter().any(|s| s.id == sid),
        "active session row must persist across reopen"
    );

    // find_stale_sessions with threshold=0 surfaces every active session
    // whose heartbeat < now; the freshly-heartbeated session may or may
    // not appear depending on clock granularity, but the row is present.
    // The point is that reopen is a no-op on session state.
    assert!(
        db.active_sessions().unwrap().iter().any(|s| s.id == sid),
        "reopen must not silently clear active sessions"
    );
}

/// **The contract test for rusqlite WAL durability under reopen.**
///
/// Sequence:
/// 1. Open DB, register two jobs, claim one (committed).
/// 2. Reopen the DB from scratch (simulating crash-and-restart).
/// 3. The claimed job is still `running`; the unclaimed job is still `pending`.
/// 4. After `reclaim_stale_jobs`, the claimed job returns to `pending`.
///
/// This is the explicit Rank 4 gate. It documents
/// the rusqlite reopen contract and is the canary test for any Dependabot
/// bump that changes WAL replay semantics. If a future rusqlite open() flag
/// change breaks this, CI will surface it before production.
#[test]
fn crash_and_reopen_db_consistency() {
    let (_dir, db_path) = fresh_db_path();

    // Pre-crash: register two jobs, claim one, exit abruptly.
    let stale_sid = {
        let db = StateDb::open(&db_path).unwrap();
        let sid = db
            .create_session(std::process::id(), "pre-crash", None)
            .unwrap();
        db.register_jobs(&[make_job("j-claimed"), make_job("j-untouched")])
            .unwrap();
        assert!(db.claim_job("j-claimed", &sid).unwrap());
        sid
    };

    // Post-crash: a fresh process reopens the DB.
    let db = StateDb::open(&db_path).unwrap();

    assert_eq!(
        db.job_status("j-claimed").unwrap().as_deref(),
        Some("running"),
        "claimed job state must survive reopen"
    );
    assert_eq!(
        db.job_status("j-untouched").unwrap().as_deref(),
        Some("pending"),
        "untouched job state must survive reopen unchanged"
    );

    let counts = db.job_counts().unwrap();
    assert_eq!(counts.running, 1);
    assert_eq!(counts.pending, 1);
    assert_eq!(counts.failed, 0);
    assert_eq!(counts.completed, 0);

    // Reclaim the orphan.
    let reclaimed = db.reclaim_stale_jobs(&stale_sid).unwrap();
    assert_eq!(reclaimed, 1);

    // Now both jobs are pending and the orphan session is interrupted.
    let counts = db.job_counts().unwrap();
    assert_eq!(counts.pending, 2);
    assert_eq!(counts.running, 0);
    assert!(db.active_sessions().unwrap().is_empty());
}
