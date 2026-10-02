//! Cold-launch accounting on both output paths. No process-wide counters.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use ox_exec_local::process::{ProcessResult, spawn_shell, spawn_shell_streaming};
use std::time::Duration;

/// Compare against the SIGCHLD-driven wait used before child accounting.
/// Paired samples and medians tolerate unrelated scheduler outliers. Five ms
/// allows log-file/pipe-drain overhead while rejecting the old 10 ms poll floor.
#[tokio::test]
#[serial_test::serial]
async fn trivial_job_wall_time_stays_close_to_tokio_wait() {
    async fn reference() -> Duration {
        let start = std::time::Instant::now();
        let output = tokio::process::Command::new("/bin/bash")
            .args(["-c", "true"])
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        start.elapsed()
    }

    for streaming in [false, true] {
        let mut measured = Vec::new();
        let mut baseline = Vec::new();
        let mut excess = Vec::new();
        for i in 0..31 {
            if i % 2 == 0 {
                baseline.push(reference().await);
            }
            let result = run("true", streaming, None).await;
            assert_eq!(result.exit_code, 0);
            assert!(result.peak_memory_bytes.is_some());
            measured.push(result.duration);
            if i % 2 != 0 {
                baseline.push(reference().await);
            }
            excess.push(result.duration.as_secs_f64() - baseline[i].as_secs_f64());
        }
        measured.sort();
        baseline.sort();
        excess.sort_by(f64::total_cmp);
        let actual = measured[15];
        let reference = baseline[15];
        eprintln!(
            "streaming={streaming}: median local={actual:?}, Tokio={reference:?}, paired excess={} ms",
            excess[15] * 1000.0
        );
        assert!(
            excess[15] <= 0.005,
            "trivial child wait added over 5 ms: local={actual:?}, Tokio={reference:?}"
        );
    }
}

/// SIGCHLD is shared and may coalesce: neighbouring Tokio children must not
/// steal a cold child's wakeup or its wait4 observation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn concurrent_exits_keep_each_child_observation() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut tasks = tokio::task::JoinSet::new();
        for i in 0..64 {
            tasks.spawn(async move {
                let command = format!("exit {}", i % 8);
                let result = run(&command, i % 2 == 0, None).await;
                assert_eq!(result.exit_code, i % 8);
                assert!(result.peak_memory_bytes.is_some());
                assert!(result.cpu_time.is_some());
            });
            tasks.spawn(async {
                let status = tokio::process::Command::new("/bin/bash")
                    .args(["-c", "true"])
                    .status()
                    .await
                    .unwrap();
                assert!(status.success());
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    })
    .await
    .expect("every child must finish even when SIGCHLD notifications coalesce");
}

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
#[serial_test::serial]
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
#[serial_test::serial]
async fn busy_child_uses_more_cpu_than_sleeping_neighbour() {
    for streaming in [false, true] {
        // Bound the busy child by WORK, not by wall time: a loop that stops at
        // a monotonic deadline accumulates less CPU the busier the machine is,
        // so under load it sinks toward the sleeper's interpreter start-up cost
        // and the assertion below fails for reasons that have nothing to do
        // with attribution.
        let busy = "exec python3 -c 'i = 0\nwhile i < 20000000: i += 1'";
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
#[serial_test::serial]
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
#[serial_test::serial]
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
