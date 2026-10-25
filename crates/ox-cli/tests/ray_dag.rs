//! HTTP contract tests; these do not simulate Ray scheduling.
use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ox() -> Command {
    Command::cargo_bin("ox").unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dash_f_file_submits_the_driver_written_beside_the_oxymakefile() {
    assert_submitted_driver_python(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ray_python_reaches_submitted_driver_entrypoint() {
    assert_submitted_driver_python(Some("/opt/ray env's/bin/python;false")).await;
}

async fn assert_submitted_driver_python(python: Option<&str>) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ray_version":"2.54.1"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"submission_id":"driver"})))
        .expect(1)
        .mount(&server)
        .await;

    let invocation_dir = tempfile::tempdir().unwrap();
    let project_dir = tempfile::Builder::new()
        .prefix("ray project's ")
        .tempdir()
        .unwrap();
    let oxymakefile = project_dir.path().join("Oxymakefile.toml");
    std::fs::write(
        &oxymakefile,
        "ox_version = \"0.1\"\n[rule.a]\noutput = [\"a.out\"]\nshell = \"touch a.out\"\n",
    )
    .unwrap();

    // A workflow selected through a symlink must submit the real staging path.
    #[cfg(unix)]
    let oxymakefile = {
        let link = invocation_dir.path().join("project-link");
        std::os::unix::fs::symlink(project_dir.path(), &link).unwrap();
        link.join("Oxymakefile.toml")
    };

    let mut command = ox();
    command.current_dir(invocation_dir.path()).args([
        "run",
        "a.out",
        "-f",
        oxymakefile.to_str().unwrap(),
        "--executor",
        "ray",
        "--ray-address",
        &server.uri(),
        "--ray-allow-pending",
        "--no-cache",
        "--metadata",
        "external.id=driver-test",
    ]);
    if let Some(python) = python {
        command.args(["--ray-python", python]);
    }
    command.assert().success();

    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.method == "POST" && request.url.path() == "/api/jobs/")
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(payload["metadata"]["external.id"], "driver-test");
    assert_eq!(payload["metadata"]["oxymake_dag_driver"], "true");
    assert!(payload["metadata"].get("oxymake_job_id").is_none());
    assert!(payload["metadata"].get("oxymake_rule").is_none());
    let run_id = payload["metadata"]["oxymake_run_id"].as_str().unwrap();
    let log = std::fs::read_to_string(
        project_dir
            .path()
            .join(format!(".oxymake/events/{run_id}.ndjson")),
    )
    .unwrap();
    let start: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(start["event"], "run_started");
    assert_eq!(start["metadata"]["external.id"], "driver-test");
    assert_eq!(run_id.len(), 40);
    assert_eq!(
        uuid::Uuid::parse_str(run_id.strip_prefix("run-").unwrap())
            .unwrap()
            .get_version_num(),
        4
    );
    let driver_path = project_dir
        .path()
        .canonicalize()
        .unwrap()
        .join(".oxymake/runs")
        .join(run_id)
        .join("oxymake_dag_driver.py");
    assert!(driver_path.is_absolute());
    assert!(
        driver_path.is_file(),
        "{driver_path:?} must exist when submitted"
    );
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(driver_path.parent().unwrap().join("meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["run_id"], run_id);
    let db = ox_state::db::StateDb::open(
        &project_dir
            .path()
            .canonicalize()
            .unwrap()
            .join(".oxymake/state.db"),
    )
    .unwrap();
    assert!(db.list_runs().unwrap().iter().any(|run| run.id == run_id));
    assert_eq!(
        payload["entrypoint"],
        format!(
            "{} '{}'",
            python
                .map(|p| format!("'{}'", p.replace('\'', "'\\''")))
                .unwrap_or_else(|| "python3".into()),
            driver_path.to_str().unwrap().replace('\'', "'\\''")
        )
    );
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
    ox().current_dir(dir.path())
        .args(["logs", "--failed"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Traceback: driver import exploded",
        ));
    let db = ox_state::db::StateDb::open(&dir.path().join(".oxymake/state.db")).unwrap();
    assert_eq!(db.job_status("a").unwrap().as_deref(), Some("failed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_explicit_run_never_contacts_or_enumerates_ray() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    recorded_run(dir.path(), &server.uri());

    let output = ox()
        .current_dir(dir.path())
        .args([
            "status",
            "--run",
            "missing-run",
            "--ray-address",
            &server.uri(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["error"]["code"], "run_not_found");
    assert_eq!(diagnostic["error"]["selector"], "missing-run");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_selected_driver_never_falls_back_to_ray_listing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/jobs/driver"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    recorded_run(dir.path(), &server.uri());

    let output = ox()
        .current_dir(dir.path())
        .args(["status", "--run", "run-1", "--json"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["error"]["code"], "run_remote_unavailable");
    assert_eq!(diagnostic["error"]["selector"], "run-1");
    assert!(
        diagnostic["error"]["reason"]
            .as_str()
            .unwrap()
            .contains("503")
    );
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| request.url.path() != "/api/jobs/")
    );
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
async fn empty_node_snapshot_warns_once_and_submits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ray_version":"2.40.0"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v0/nodes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result":true,"data":{"result":{"total":0,"num_after_truncation":0,"num_filtered":0,"result":[],"partial_failure_warning":null}}})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"submission_id":"driver"})))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Oxymakefile.toml"), "ox_version = \"0.1\"\n[rule.a]\noutput = [\"a.out\"]\nshell = \"touch a.out\"\nresources = {metal=1}\n").unwrap();
    let output = ox()
        .current_dir(dir.path())
        .args([
            "run",
            "a.out",
            "--executor",
            "ray",
            "--ray-address",
            &server.uri(),
            "--no-cache",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let warning = "resource feasibility could not be checked because no live node was visible";
    assert_eq!(stderr.matches(warning).count(), 1, "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_can_allow_pending_and_explicit_flag_wins_over_false() {
    for (profile_value, explicit_flag) in [(true, false), (false, true)] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/version"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ray_version":"2.40.0"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/jobs/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"submission_id":"driver"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Oxymakefile.toml"),
            format!("ox_version = \"0.1\"\n[profile.ray]\nexecutor = \"ray\"\nray_allow_pending = {profile_value}\n[rule.a]\noutput = [\"a.out\"]\nshell = \"touch a.out\"\nresources = {{metal=1}}\n"),
        ).unwrap();
        let mut command = ox();
        command.current_dir(dir.path()).args([
            "run",
            "a.out",
            "--profile",
            "ray",
            "--ray-address",
            &server.uri(),
            "--no-cache",
        ]);
        if explicit_flag {
            command.arg("--ray-allow-pending");
        }
        command.assert().success();
    }
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
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
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
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            panic!(
                "driver exited before being polled ({status}): {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        if start.elapsed() > Duration::from_secs(120) {
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
    // Observe the stop request before checking exit, with one hang guard
    // covering both cancellation and teardown rather than a latency budget.
    let start = Instant::now();
    loop {
        // Read exit first so a stop received between the two observations
        // cannot be mistaken for an exit without cancellation.
        let status = child.try_wait().unwrap();
        if server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.method == "POST" && r.url.path() == "/api/jobs/driver/stop")
        {
            break;
        }
        if let Some(status) = status {
            let output = child.wait_with_output().unwrap();
            panic!(
                "stop never received: driver exited ({status}): {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        if start.elapsed() > Duration::from_secs(120) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("stop never received");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(130));
            let output = child.wait_with_output().unwrap();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(text.contains("Ray driver stopped"), "{text}");
            assert!(!text.contains("NOT cancelled"), "{text}");
            assert!(!text.contains("Completed:"), "{text}");
            break;
        }
        if start.elapsed() > Duration::from_secs(120) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("follow ignored interruption");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
