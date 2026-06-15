//! Fault-injection tests for the warm `WorkerPool`.
//!
//! These tests exercise the catalogued IPC failure modes:
//!
//! - **Rank 3 — Worker half-closed IPC** — the warm worker emits the
//!   ready handshake, accepts a dispatch, then writes a **partial**
//!   JSON line (no terminating newline) and stalls. The pool must
//!   surface a `DispatchTimeout`, evict the broken worker from the
//!   `env_key → worker` map, and remain usable for subsequent
//!   `ensure_warm` calls.
//!
//! The fake worker is implemented as a small `sh` script — no Python is
//! required. The tests gate on `cfg(unix)` because the script is POSIX
//! shell.

#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ox_exec_local::worker_pool::{WorkerError, WorkerPool};
use tempfile::TempDir;

/// Write `body` to `<dir>/<name>`, mark it executable, and return its path
/// as a string. `body` is interpreted as a POSIX shell script.
fn write_executable_script(dir: &std::path::Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write script");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod script");
    path.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Rank 3 — Worker half-closed IPC timeout + eviction
// ---------------------------------------------------------------------------

/// The full ready/partial-response scenario.
///
/// 1. Fake worker prints `{"status":"ready"}` so `ensure_warm` returns Ok.
/// 2. Parent calls `dispatch(...)` with a 750ms timeout.
/// 3. Worker reads the dispatch line, writes a *partial* response
///    (`{"status":"ok"` with no closing brace and no newline) and stalls.
/// 4. `read_json_line` blocks waiting for `\n`; the `tokio::time::timeout`
///    wrapper around it fires and `dispatch` returns `WorkerError::DispatchTimeout`.
/// 5. The pool kills the worker (SIGKILL) and removes it from the map.
/// 6. A subsequent `ensure_warm` for the same `env_key` succeeds — proof
///    that eviction was effective.
#[tokio::test]
async fn half_closed_ipc_dispatch_times_out_and_evicts_worker() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    // The fake worker. The `ensure_warm` path writes the warmup script to
    // the last element of argv; our script ignores that and runs its own
    // logic. We use `exec sh` so the script's PID is the spawned child's
    // PID — this matters for the SIGKILL path inside `dispatch`.
    let script = "\
#!/bin/sh
# Stage 1: announce readiness.
printf '{\"status\":\"ready\"}\\n'

# Stage 2: wait for the first payload. If the parent sends shutdown
# (clean exit path used by post-eviction re-warm + pool.shutdown), honour
# it immediately so the worker does not wedge the test on drop.
read -r payload || exit 0
case \"$payload\" in *shutdown*) exit 0 ;; esac

# Stage 3: write a half-line and stall. No closing brace, no newline. The
# parent's read_json_line will block on \\n until SIGKILLed by dispatch's
# timeout branch (the path under test).
printf '{\"status\":\"ok'

# Stage 4: keep stdin open so SIGKILL is the only exit path. If the
# parent eventually sends shutdown, honour it (defensive).
while read -r line; do
  case \"$line\" in *shutdown*) exit 0 ;; esac
done
";
    let script_path = write_executable_script(&work_dir, "fake_worker.sh", script);

    // ensure_warm writes the supplied warmup_script to argv.last(). We don't
    // want it overwriting our hand-rolled script, so we let it write a
    // sibling file that the real argv never reads.
    let dummy_warmup_path = work_dir.join("unused_warmup_script.sh");
    let argv = vec![
        "/bin/sh".to_string(),
        script_path,
        // argv.last() must point to a writable path that ensure_warm can
        // overwrite without breaking anything. The shell ignores its
        // positional arg here because the script doesn't reference $1.
        dummy_warmup_path.to_string_lossy().into_owned(),
    ];

    let pool = Arc::new(WorkerPool::new(work_dir.clone()));

    // The warmup-script content is irrelevant — our fake worker ignores it.
    pool.ensure_warm("env-a", &argv, "# unused\n")
        .await
        .expect("ensure_warm should observe the {status:ready} handshake");

    // Now dispatch with a short timeout. The worker writes a partial line
    // and blocks; the wrapper around read_json_line must fire.
    let payload = serde_json::json!({"cmd": "exec", "module": "x", "fn": "y"});
    let timeout = Duration::from_millis(750);
    let start = Instant::now();
    let result = pool.dispatch("env-a", &payload, timeout).await;
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(WorkerError::DispatchTimeout)),
        "expected DispatchTimeout, got: {result:?}"
    );

    // Sanity: we should have waited at least the timeout, but not many
    // times more (eviction is synchronous after the timeout fires).
    assert!(
        elapsed >= timeout,
        "dispatch returned before its own timeout window: {elapsed:?}"
    );
    assert!(
        elapsed < timeout * 8,
        "dispatch took dramatically longer than its timeout — eviction may be stuck: {elapsed:?}"
    );

    // Eviction must have removed the entry from the env-key map.
    // We can prove this only behaviourally: a subsequent ensure_warm for
    // the same env-key must spawn a new worker successfully (and not
    // short-circuit on a cached, broken handle).
    pool.ensure_warm("env-a", &argv, "# unused\n")
        .await
        .expect("post-eviction ensure_warm must succeed with a fresh worker");

    // Clean up — the shutdown loop kills the new worker.
    pool.shutdown().await;
}

/// A second dispatch attempt against an env-key with no warm worker must
/// surface a `PythonError`, not panic or hang. This is the **defensive
/// boundary** for cases where eviction happened but the caller did not
/// re-warm before re-dispatching.
#[tokio::test]
async fn dispatch_on_unknown_env_returns_error_not_hang() {
    let dir = TempDir::new().unwrap();
    let pool = WorkerPool::new(dir.path().to_path_buf());

    let payload = serde_json::json!({"cmd": "exec"});
    let result = pool
        .dispatch("never-warmed", &payload, Duration::from_millis(100))
        .await;

    assert!(
        matches!(result, Err(WorkerError::PythonError(_))),
        "expected PythonError(\"no warm worker\"), got: {result:?}"
    );
}

/// Document the contract: a worker that emits a *non-ready* status line at
/// handshake time causes `ensure_warm` to fail with `BadReady`, not panic.
/// This is the failure path opposite to the partial-write scenario above —
/// here the worker speaks, but speaks wrong.
#[tokio::test]
async fn ensure_warm_rejects_non_ready_handshake() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    let script = "\
#!/bin/sh
printf '{\"status\":\"explode\"}\\n'
exit 0
";
    let script_path = write_executable_script(&work_dir, "bad_ready.sh", script);
    let dummy = work_dir.join("unused.sh");
    let argv = vec![
        "/bin/sh".to_string(),
        script_path,
        dummy.to_string_lossy().into_owned(),
    ];

    let pool = WorkerPool::new(work_dir.clone());
    let err = pool
        .ensure_warm("env-bad", &argv, "# unused\n")
        .await
        .expect_err("non-ready handshake must error");

    assert!(
        matches!(err, WorkerError::BadReady(_)),
        "expected BadReady, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// H22 — timeout kill must take the whole process group, not just the template
// ---------------------------------------------------------------------------

/// The fork-after-import pattern means the template process forks a child
/// for each dispatch. On dispatch timeout the pool used to SIGKILL the
/// template PID only — the forked grandchild kept running (and kept
/// writing into `results/`). The template must be spawned in its own
/// process group and the timeout path must `killpg` the group.
#[tokio::test]
async fn dispatch_timeout_kills_forked_grandchild() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();
    let pid_file = work_dir.join("grandchild.pid");

    // Fake worker: announces ready, then on dispatch forks a grandchild
    // (records its PID, loops forever — simulating the fork-after-import
    // child mid-execution), writes a partial response and stalls so the
    // dispatch timeout fires.
    let script = format!(
        "\
#!/bin/sh
printf '{{\"status\":\"ready\"}}\\n'
read -r payload || exit 0
case \"$payload\" in *shutdown*) exit 0 ;; esac
sh -c 'echo $$ > \"{pid_file}\"; while :; do sleep 0.05; done' &
printf '{{\"status\":\"ok'
while read -r line; do
  case \"$line\" in *shutdown*) exit 0 ;; esac
done
",
        pid_file = pid_file.display()
    );
    let script_path = write_executable_script(&work_dir, "forking_worker.sh", &script);

    let dummy_warmup_path = work_dir.join("unused_warmup_script.sh");
    let argv = vec![
        "/bin/sh".to_string(),
        script_path,
        dummy_warmup_path.to_string_lossy().into_owned(),
    ];

    let pool = Arc::new(WorkerPool::new(work_dir.clone()));
    pool.ensure_warm("env-fork", &argv, "# unused\n")
        .await
        .expect("ensure_warm should observe the ready handshake");

    let payload = serde_json::json!({"cmd": "exec", "module": "x", "fn": "y"});
    let result = pool
        .dispatch("env-fork", &payload, Duration::from_millis(750))
        .await;
    assert!(
        matches!(result, Err(WorkerError::DispatchTimeout)),
        "expected DispatchTimeout, got: {result:?}"
    );

    // The grandchild recorded its PID before the timeout fired.
    let mut pid: Option<i32> = None;
    for _ in 0..20 {
        if let Ok(s) = std::fs::read_to_string(&pid_file) {
            if let Ok(p) = s.trim().parse::<i32>() {
                pid = Some(p);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pid = pid.expect("grandchild must have recorded its PID");

    // Give SIGKILL a moment to be delivered, then probe with signal 0.
    // ESRCH (-1) means the grandchild is gone — the group kill worked.
    let mut alive = true;
    for _ in 0..20 {
        alive = unsafe { libc::kill(pid, 0) } == 0;
        if !alive {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Cleanup before asserting so a red run does not leak the looper.
    if alive {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }

    assert!(
        !alive,
        "forked grandchild (pid {pid}) survived the dispatch-timeout kill — \
         the pool killed the template only, not its process group"
    );
}
