//! CLI witnesses for both DAG adapters. Retry-count injection lives in the
//! private follow controller tests; these verify real wiring and exit codes.
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn fixture(ray: bool) -> (MockServer, tempfile::TempDir, std::process::Command, String) {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Oxymakefile.toml"), format!(
        "ox_version = \"0.1\"\n[executor.slurm]\nstaging_dir = {:?}\n[rule.a]\noutput = [\"a.out\"]\nshell = \"touch a.out\"\n",
        dir.path().join("staging").to_str().unwrap()
    )).unwrap();
    let (init, submit, poll) = if ray {
        ("/api/version", "/api/jobs/", "/api/jobs/driver")
    } else {
        (
            "/slurm/v0.0.44/nodes",
            "/slurm/v0.0.44/job/submit",
            "/slurm/v0.0.44/job/42",
        )
    };
    Mock::given(method("GET"))
        .and(path(init))
        .respond_with(ResponseTemplate::new(200).set_body_json(if ray {
            json!({"ray_version":"2.40.0"})
        } else {
            json!({"nodes":[{"name":"n1","state":["idle"]}]})
        }))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(submit))
        .respond_with(ResponseTemplate::new(200).set_body_json(if ray {
            json!({"submission_id":"driver"})
        } else {
            json!({"job_id":42})
        }))
        .expect(1)
        .mount(&server)
        .await;
    let mut command = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"));
    command.current_dir(dir.path()).args([
        "run",
        "a.out",
        "--follow",
        "--no-cache",
        "--executor",
        if ray { "ray" } else { "slurm" },
    ]);
    command.args([
        if ray { "--ray-address" } else { "--slurm-api" },
        &server.uri(),
    ]);
    if ray {
        command.arg("--ray-allow-pending");
    }
    (server, dir, command, poll.into())
}

fn status(ray: bool, complete: bool) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(if ray {
        json!({"status":if complete {"SUCCEEDED"} else {"RUNNING"}})
    } else {
        json!({"jobs":[{"job_id":42,"job_state":[if complete {"COMPLETED"} else {"RUNNING"}]}]})
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_commands_report_endpoint_loss_without_success() {
    for ray in [true, false] {
        let (server, dir, mut command, poll) = fixture(ray).await;
        command.arg("--timings");
        if !ray {
            command.arg("--json");
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        Mock::given(method("GET"))
            .and(path(poll))
            .respond_with(move |_: &wiremock::Request| {
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    status(ray, false)
                } else {
                    ResponseTemplate::new(503).set_body_string("endpoint gone")
                }
            })
            .mount(&server)
            .await;
        let output = tokio::task::spawn_blocking(move || {
            assert_cmd::Command::from_std(command)
                .timeout(Duration::from_secs(40))
                .assert()
                .code(1)
                .get_output()
                .clone()
        })
        .await
        .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let db = ox_state::db::StateDb::open(&dir.path().join(".oxymake/state.db")).unwrap();
        let sessions = db.session_statuses().unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(
            stderr.contains("state_db_finalize") && sessions[0].1 == "interrupted",
            "stopped follow must print timings and interrupt its session: stderr={stderr}, sessions={sessions:?}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Follow failed: endpoint"), "{stdout}");
        assert!(
            stdout.contains(&server.uri()) && stdout.contains("503"),
            "{stdout}"
        );
        assert!(
            stdout.contains("last error:") && stdout.contains("NOT cancelled"),
            "{stdout}"
        );
        assert!(!stdout.contains("Completed:"), "{stdout}");
        if !ray {
            let events: Vec<serde_json::Value> = stdout
                .lines()
                .map(|line| serde_json::from_str(line).expect("JSON output must be NDJSON"))
                .collect();
            let event = events
                .iter()
                .find(|event| event["event"] == "run_follow_stopped")
                .expect("stopped follow must emit its distinct terminal outcome");
            assert!(!events.iter().any(|event| event["event"] == "run_completed"));
            assert_eq!(event["reason"], "endpoint_unreachable");
            assert_eq!(event["exit_code"], 1);
            assert_eq!(event["failed"], 0);
        }
        // The injected controller tests establish the bound; this checks the
        // adapters use the production policy, including the healthy first poll.
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert!(
            !server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.method == "DELETE" || r.url.path().ends_with("/stop"))
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_commands_recover_and_keep_success_summary() {
    for ray in [true, false] {
        for transient in [false, true] {
            let (server, _dir, command, poll) = fixture(ray).await;
            let calls = Arc::new(AtomicUsize::new(0));
            let seen = calls.clone();
            Mock::given(method("GET"))
                .and(path(poll))
                .respond_with(move |_: &wiremock::Request| {
                    let call = seen.fetch_add(1, Ordering::SeqCst);
                    if transient && call < 2 {
                        ResponseTemplate::new(503)
                    } else {
                        status(ray, true)
                    }
                })
                .mount(&server)
                .await;
            let output = tokio::task::spawn_blocking(move || {
                assert_cmd::Command::from_std(command)
                    .timeout(Duration::from_secs(30))
                    .assert()
                    .success()
                    .get_output()
                    .clone()
            })
            .await
            .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stdout.contains("Completed: 1 succeeded, 0 failed, 0 skipped, 0 cancelled ("),
                "{stdout}"
            );
            assert!(
                stderr.contains("  Progress: 1/1 (1 succeeded, 0 failed)"),
                "{stderr}"
            );
            assert!(!stdout.contains("Follow failed"));
            assert_eq!(calls.load(Ordering::SeqCst), if transient { 3 } else { 1 });
        }
    }
}

#[cfg(unix)]
async fn interrupt_in_flight(ray: bool, cancel_fails: bool) {
    let (server, dir, mut command, poll) = fixture(ray).await;
    if ray {
        command.arg("--json");
        Mock::given(method("POST"))
            .and(path("/api/jobs/driver/stop"))
            .respond_with(ResponseTemplate::new(if cancel_fails { 503 } else { 200 }))
            .expect(1)
            .mount(&server)
            .await;
    }
    command.arg("--timings");
    let polling = Arc::new(tokio::sync::Notify::new());
    let seen_polling = polling.clone();
    let calls = AtomicUsize::new(0);
    Mock::given(method("GET"))
        .and(path(poll))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return status(ray, false);
            }
            seen_polling.notify_one();
            status(ray, false).set_delay(Duration::from_secs(30))
        })
        .mount(&server)
        .await;
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    if tokio::time::timeout(Duration::from_secs(20), polling.notified())
        .await
        .is_err()
    {
        child.kill().unwrap();
        panic!(
            "second poll never started: {:?}",
            child.wait_with_output().unwrap()
        );
    }
    assert!(
        std::process::Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(2) {
            child.kill().unwrap();
            panic!(
                "follow ignored first signal: {:?}",
                child.wait_with_output().unwrap()
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Follow interrupted"), "{stdout}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("state_db_finalize"));
    if ray {
        let events: Vec<serde_json::Value> = stdout
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let event = events
            .iter()
            .find(|e| e["event"] == "run_follow_stopped")
            .unwrap();
        assert_eq!(event["reason"], "interrupted");
        assert_eq!(event["exit_code"], 130);
        assert!(!events.iter().any(|e| e["event"] == "run_completed"));
        if cancel_fails {
            assert!(
                stdout.contains("failed to stop Ray driver") && stdout.contains("503"),
                "{stdout}"
            );
            assert!(
                !stdout.contains("Ray driver stopped") && !stdout.contains("NOT cancelled"),
                "{stdout}"
            );
        } else {
            assert!(stdout.contains("Ray driver stopped"), "{stdout}");
            assert!(!stdout.contains("NOT cancelled"), "{stdout}");
        }
    } else {
        assert!(
            stdout.contains("still be running") && stdout.contains("remote jobs are NOT cancelled"),
            "{stdout}"
        );
        assert!(
            stdout.contains("ox status") && stdout.contains("ox cancel"),
            "{stdout}"
        );
        assert!(
            !server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.method == "DELETE" || r.url.path().ends_with("/stop"))
        );
    }
    assert!(!stdout.contains("Completed:"), "{stdout}");
    let db = rusqlite::Connection::open(dir.path().join(".oxymake/state.db")).unwrap();
    let terminal: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE status IN ('failed', 'cancelled')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        terminal,
        if ray && !cancel_fails { 1 } else { 0 },
        "only a successful Ray driver stop may terminalize remote jobs"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ray_interrupt_in_flight() {
    interrupt_in_flight(true, false).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slurm_interrupt_in_flight() {
    interrupt_in_flight(false, false).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ray_interrupt_reports_cancellation_failure() {
    interrupt_in_flight(true, true).await;
}
