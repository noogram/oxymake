//! HTTP contract tests; these do not simulate Ray scheduling.
use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ox() -> Command {
    Command::cargo_bin("ox").unwrap()
}

fn recorded_run(dir: &std::path::Path, address: &str) {
    let root = dir.join(".oxymake");
    std::fs::create_dir_all(root.join("runs/run-1")).unwrap();
    let db = ox_state::db::StateDb::open(&root.join("state.db")).unwrap();
    db.begin_run("run-1", None, 1, None).unwrap();
    db.record_dag_submission("run-1", "ray", Some(address), 1)
        .unwrap();
    db.register_jobs(&[ox_state::db::JobRecord {
        id: "a".into(),
        rule_name: "a".into(),
        wildcards: "{}".into(),
        cache_key: None,
        run_id: Some("run-1".into()),
    }])
    .unwrap();
    db.set_executor_submission_id("a", "driver").unwrap();
    std::fs::write(root.join("runs/run-1/meta.json"), json!({"executor":"ray", "ray_address":address,"ray_job_id":"driver", "active_jobs":1,"total_jobs":1,"skipped_jobs":0}).to_string()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_crash_without_results_is_visible_in_status_and_logs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/jobs/driver"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status":"FAILED","message":"driver import exploded"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/jobs/driver/logs"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"logs":"Traceback: driver import exploded"})),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    recorded_run(dir.path(), &server.uri());
    // An older run's success must not mask this driver's crash.
    std::fs::create_dir_all(dir.path().join(".oxymake/runs/older")).unwrap();
    std::fs::write(
        dir.path().join(".oxymake/runs/older/results.json"),
        r#"{"a":{"status":"completed","exit_code":0}}"#,
    )
    .unwrap();
    // Logs must work even before a status refresh.
    ox().current_dir(dir.path())
        .args(["logs", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("driver import exploded"));
    ox().current_dir(dir.path())
        .args(["status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 failed"))
        .stdout(predicate::str::contains("0 pending"));
    let db = ox_state::db::StateDb::open(&dir.path().join(".oxymake/state.db")).unwrap();
    assert_eq!(db.job_status("a").unwrap().as_deref(), Some("failed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_pending_driver_calls_ray_stop() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/jobs/driver/stop"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"stopped":true})))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    recorded_run(dir.path(), &server.uri());
    ox().current_dir(dir.path())
        .args(["cancel", "--all"])
        .assert()
        .success();
}

#[test]
fn pending_flag_is_ray_only() {
    for backend in ["local", "slurm"] {
        ox().args(["run", "--ray-allow-pending", "--executor", backend])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "--ray-allow-pending requires --executor ray",
            ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follow_reports_failure_after_submission_ack() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ray_version":"2.40.0"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"submission_id":"driver"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/jobs/driver"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status":"FAILED","message":"driver startup failed"})),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Oxymakefile.toml"), "ox_version = \"0.1\"\n[rule.a]\noutput = [\"a.out\"]\nshell = \"touch a.out\"\nresources = {metal=1}\n").unwrap();
    ox().current_dir(dir.path())
        .args([
            "run",
            "a.out",
            "--executor",
            "ray",
            "--ray-address",
            &server.uri(),
            "--ray-allow-pending",
            "--follow",
            "--no-cache",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("driver startup failed"));
    assert!(!dir.path().join("a.out").exists());
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/api/v0/nodes")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_remote_stop_leaves_pending_jobs_retryable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/jobs/driver/stop"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    recorded_run(dir.path(), &server.uri());
    ox().current_dir(dir.path())
        .args(["cancel", "--all"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("local cancellation not recorded"));
    let db = ox_state::db::StateDb::open(&dir.path().join(".oxymake/state.db")).unwrap();
    assert_eq!(db.job_status("a").unwrap().as_deref(), Some("pending"));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_follow_stops_queued_driver() {
    use std::time::{Duration, Instant};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ray_version":"2.40.0"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"submission_id":"driver"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/jobs/driver"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"PENDING"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/jobs/driver/stop"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Oxymakefile.toml"), "ox_version = \"0.1\"\n[rule.a]\noutput = [\"a.out\"]\nshell = \"touch a.out\"\nresources = {metal=1}\n").unwrap();
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"))
        .current_dir(dir.path())
        .args([
            "run",
            "a.out",
            "--executor",
            "ray",
            "--ray-address",
            &server.uri(),
            "--ray-allow-pending",
            "--follow",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .any(|r| r.url.path() == "/api/jobs/driver")
    {
        if start.elapsed() > Duration::from_secs(12) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("driver never polled");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        std::process::Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(8) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("follow ignored interruption");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
