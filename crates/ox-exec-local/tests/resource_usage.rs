//! Cold-launch accounting on both output paths. No process-wide counters.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use ox_exec_local::process::{ProcessResult, spawn_shell, spawn_shell_streaming};
use std::time::Duration;

async fn run(command: &str, streaming: bool, timeout: Option<Duration>) -> ProcessResult {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("job.log");
    let result = if streaming {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_shell_streaming(
            command,
            dir.path(),
            &log,
            timeout,
            &[],
            "/bin/bash",
            &[],
            |_| {},
            tx,
        )
        .await
    } else {
        spawn_shell(command, dir.path(), &log, timeout, &[], "/bin/bash").await
    };
    result.unwrap()
}

#[tokio::test]
async fn allocation_has_child_peak_on_both_paths() {
    for streaming in [false, true] {
        let result = run(
            "exec python3 -c 'x = bytearray(64 * 1024 * 1024)'",
            streaming,
            None,
        )
        .await;
        assert_eq!(result.exit_code, 0);
        let rss = result.peak_memory_bytes.expect("child RSS");
        // 64 MiB touched allocation plus interpreter overhead: 64..192 MiB,
        // broad enough for Linux/macOS, narrow enough to catch a 1024x error.
        assert!(
            (64 * 1024 * 1024..192 * 1024 * 1024).contains(&rss),
            "RSS: {rss}"
        );
        assert!(result.cpu_time.unwrap() > Duration::ZERO);
    }
}

#[tokio::test]
async fn busy_child_uses_more_cpu_than_sleeping_neighbour() {
    for streaming in [false, true] {
        let busy = "exec python3 -c 'import time; end = time.monotonic() + 0.5\nwhile time.monotonic() < end: pass'";
        let sleep = "exec python3 -c 'import time; time.sleep(0.5)'";
        let (busy, sleep) = tokio::join!(run(busy, streaming, None), run(sleep, streaming, None));
        assert_eq!(busy.exit_code, 0);
        assert_eq!(sleep.exit_code, 0);
        assert!(
            busy.cpu_time.unwrap() > sleep.cpu_time.unwrap() + Duration::from_millis(100),
            "busy={busy:?}, sleep={sleep:?}"
        );
    }
}

#[tokio::test]
async fn unsuccessful_children_keep_observed_usage() {
    for streaming in [false, true] {
        for (command, timeout, code) in [
            (
                "exec python3 -c 'x = bytearray(32 * 1024 * 1024); raise SystemExit(7)'",
                None,
                7,
            ),
            (
                "exec python3 -c 'import os, signal; x = bytearray(32 * 1024 * 1024); os.kill(os.getpid(), signal.SIGKILL)'",
                None,
                -1,
            ),
            (
                "exec python3 -c 'import time; x = bytearray(32 * 1024 * 1024); time.sleep(30)'",
                Some(Duration::from_secs(1)),
                137,
            ),
        ] {
            let result = run(command, streaming, timeout).await;
            assert_eq!(result.exit_code, code);
            assert_eq!(result.killed_by_timeout, timeout.is_some());
            // A timeout may interrupt interpreter startup before allocation;
            // it must still preserve the nonzero observation actually taken.
            let minimum = if timeout.is_some() {
                1
            } else {
                32 * 1024 * 1024
            };
            assert!(result.peak_memory_bytes.unwrap() >= minimum, "{result:?}");
            assert!(result.cpu_time.unwrap() > Duration::ZERO, "{result:?}");
        }
    }
}

#[tokio::test]
async fn aborted_caller_transfers_reaper_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().to_path_buf();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        ox_exec_local::process::spawn_shell_with_callback(
            "exec sleep 30",
            &work,
            &work.join("abort.log"),
            None,
            &[],
            "/bin/bash",
            &[],
            |pid| {
                let _ = tx.send(pid);
            },
        )
        .await
    });
    let pid = rx.await.unwrap() as libc::pid_t;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    // SAFETY: this test owns the still-running child's process group.
    assert_eq!(unsafe { libc::killpg(pid, libc::SIGKILL) }, 0);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            // SAFETY: signal 0 observes existence without changing the process.
            if unsafe { libc::kill(pid, 0) } == -1 {
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::ESRCH)
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("abandoned child must be reaped");
}
