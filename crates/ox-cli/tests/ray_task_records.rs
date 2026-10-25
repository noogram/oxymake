//! Ray per-task records reach state, history and the event log (#58).
//!
//! `ox run --executor ray` submits to a mocked Jobs API; the test then runs the
//! driver it generated under a synchronous stand-in for the `ray` module, so
//! the recipes really execute and the records are real, but Ray's scheduler is
//! not exercised. `ox status` and `ox run --follow` synchronise the records.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// `.remote()` runs the task at once. `OX_STUB_KILL_AFTER=n` SIGKILLs the
/// driver before its (n+1)-th task, as `ray job stop` or an OOM kill would.
const SYNC_RAY_STUB: &str = r#"
import os, signal
_runs = 0
class _Context:
    def get_node_id(self): return 'stub-node'
def get_runtime_context(): return _Context()
def init(): pass
class _Ref:
    def __init__(self, value=None, error=None): self.value, self.error = value, error
class _Task:
    def __init__(self, fn): self.fn = fn
    def options(self, **_): return self
    def remote(self, *args):
        global _runs
        limit = os.environ.get('OX_STUB_KILL_AFTER')
        if limit is not None and _runs >= int(limit):
            os.kill(os.getpid(), signal.SIGKILL)
        _runs += 1
        try:
            return _Ref(value=self.fn(*args))
        except Exception as e:
            return _Ref(error=e)
def remote(fn): return _Task(fn)
def get(ref):
    if ref.error is not None:
        raise ref.error
    return ref.value
"#;

/// Two jobs, `light` then `heavy` (it consumes `light`'s output), holding
/// 32 MiB / 0.1 s CPU and 160 MiB / 0.6 s CPU.
const WORKFLOW: &str = r#"ox_version = "0.1"

[rule.light]
output = ["light.out"]
shell = '''python3 -c "import time; b = b'x' * (32 << 20); t = time.process_time(); [0 for _ in iter(lambda: time.process_time() - t < 0.1, False)]" && touch light.out'''

[rule.heavy]
input = ["light.out"]
output = ["heavy.out"]
shell = '''python3 -c "import time; b = b'x' * (160 << 20); t = time.process_time(); [0 for _ in iter(lambda: time.process_time() - t < 0.6, False)]" && touch heavy.out'''
"#;

const MIB: u64 = 1 << 20;

fn ox() -> Command {
    Command::cargo_bin("ox").unwrap()
}

async fn ray_api(driver_status: &str) -> MockServer {
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
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": driver_status})))
        .mount(&server)
        .await;
    server
}

fn run_args(server: &MockServer) -> Vec<String> {
    [
        "run",
        "heavy.out",
        "--executor",
        "ray",
        "--ray-allow-pending",
        "--no-cache",
        "--ray-address",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([server.uri()])
    .collect()
}

fn run_dir(project: &Path) -> PathBuf {
    let runs: Vec<PathBuf> = std::fs::read_dir(project.join(".oxymake/runs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(runs.len(), 1, "{runs:?}");
    runs.into_iter().next().unwrap()
}

/// Run the submitted driver as the Ray job would, with `ray` stubbed.
fn run_driver(project: &Path, run_dir: &Path, kill_after: Option<u32>) {
    let stub = tempfile::tempdir().unwrap();
    std::fs::write(stub.path().join("ray.py"), SYNC_RAY_STUB).unwrap();
    let mut driver = std::process::Command::new("python3");
    driver
        .arg(run_dir.join("oxymake_dag_driver.py"))
        .current_dir(project)
        .env("PYTHONPATH", stub.path());
    if let Some(n) = kill_after {
        driver.env("OX_STUB_KILL_AFTER", n.to_string());
    }
    let output = driver.output().unwrap();
    assert_eq!(
        output.status.success(),
        kill_after.is_none(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn hostname() -> String {
    let output = std::process::Command::new("python3")
        .args(["-c", "import socket; print(socket.gethostname())"])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn history(project: &Path, run_id: &str) -> Vec<Value> {
    let output = ox()
        .current_dir(project)
        .args(["history", "--run-id", run_id, "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn events(project: &Path, run_id: &str, name: &str) -> Vec<Value> {
    std::fs::read_to_string(project.join(format!(".oxymake/events/{run_id}.ndjson")))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["event"] == name)
        .collect()
}

fn job_status(project: &Path, job: &str) -> Option<String> {
    ox_state::db::StateDb::open(&project.join(".oxymake/state.db"))
        .unwrap()
        .job_status(job)
        .unwrap()
}

fn row<'a>(rows: &'a [Value], job: &str) -> &'a Value {
    rows.iter()
        .find(|row| row["job_id"] == job)
        .unwrap_or_else(|| panic!("no {job} in {rows:?}"))
}

/// Peak RSS and CPU time are distinct and plausible per job, in history and
/// in the event log, with the node the task ran on.
fn assert_usage(project: &Path, run_id: &str) {
    let rows = history(project, run_id);
    let (light, heavy) = (row(&rows, "light"), row(&rows, "heavy"));
    let mib = |row: &Value| row["peak_mem_mb"].as_u64().expect("peak RSS");
    let cpu = |row: &Value| row["cpu_time_ms"].as_u64().expect("CPU time");
    assert!((32..96).contains(&mib(light)), "{light}");
    assert!((160..320).contains(&mib(heavy)), "{heavy}");
    assert!(cpu(light) >= 100 && cpu(heavy) >= 600 && cpu(heavy) > cpu(light));
    let host = hostname();
    for row in [light, heavy] {
        assert_eq!(row["node"], host.as_str(), "{row}");
        assert_eq!(row["executor"], "ray", "{row}");
        assert!(
            row["wall_time_ms"].as_u64().unwrap() >= cpu(row) / 2,
            "{row}"
        );
    }

    let completed = events(project, run_id, "job_completed");
    assert_eq!(completed.len(), 2, "one event per job: {completed:?}");
    for (job, row) in [("light", light), ("heavy", heavy)] {
        let event = completed.iter().find(|e| e["job_id"] == job).unwrap();
        assert_eq!(event["cpu_time_ms"], row["cpu_time_ms"], "{event}");
        assert_eq!(event["node"], host.as_str(), "{event}");
        assert_eq!(event["submission_id"], "driver", "{event}");
        let bytes = event["peak_memory_bytes"].as_u64().unwrap();
        assert_eq!(bytes.div_ceil(MIB), mib(row), "{event}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_records_each_ray_task_usage_and_node_once() {
    let server = ray_api("SUCCEEDED").await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("Oxymakefile.toml"), WORKFLOW).unwrap();
    ox().current_dir(project.path())
        .args(run_args(&server))
        .assert()
        .success();
    let run_dir = run_dir(project.path());
    let run_id = run_dir.file_name().unwrap().to_str().unwrap().to_string();
    run_driver(project.path(), &run_dir, None);

    for _ in 0..2 {
        ox().current_dir(project.path())
            .arg("status")
            .assert()
            .success();
    }

    assert_eq!(
        job_status(project.path(), "light").as_deref(),
        Some("completed")
    );
    assert_eq!(
        job_status(project.path(), "heavy").as_deref(),
        Some("completed")
    );
    // A second synchronisation adds neither history rows nor events.
    assert_eq!(history(project.path(), &run_id).len(), 2);
    assert_usage(project.path(), &run_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_reads_the_record_a_killed_driver_left() {
    let server = ray_api("FAILED").await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("Oxymakefile.toml"), WORKFLOW).unwrap();
    ox().current_dir(project.path())
        .args(run_args(&server))
        .assert()
        .success();
    let run_dir = run_dir(project.path());
    let run_id = run_dir.file_name().unwrap().to_str().unwrap().to_string();
    // Killed after the first task: no results.json, one record.
    run_driver(project.path(), &run_dir, Some(1));
    assert!(!run_dir.join("results.json").exists());

    ox().current_dir(project.path())
        .arg("status")
        .assert()
        .success();

    assert_eq!(
        job_status(project.path(), "light").as_deref(),
        Some("completed")
    );
    assert_eq!(
        job_status(project.path(), "heavy").as_deref(),
        Some("failed")
    );
    let rows = history(project.path(), &run_id);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let light = row(&rows, "light");
    assert!(
        (32..96).contains(&light["peak_mem_mb"].as_u64().unwrap()),
        "{light}"
    );
    assert!(light["cpu_time_ms"].as_u64().unwrap() >= 100, "{light}");
    let completed = events(project.path(), &run_id, "job_completed");
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["job_id"], "light");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follow_records_each_ray_task_usage_and_node_once() {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("Oxymakefile.toml"), WORKFLOW).unwrap();
    // The driver is RUNNING until it has written results.json.
    let runs = project.path().join(".oxymake/runs");
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
        .respond_with(move |_: &wiremock::Request| {
            let done = std::fs::read_dir(&runs)
                .into_iter()
                .flatten()
                .any(|run| run.unwrap().path().join("results.json").exists());
            let status = if done { "SUCCEEDED" } else { "RUNNING" };
            ResponseTemplate::new(200).set_body_json(json!({ "status": status }))
        })
        .mount(&server)
        .await;

    let mut follow = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"));
    follow
        .current_dir(project.path())
        .args(run_args(&server))
        .arg("--follow")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let child = follow.spawn().unwrap();

    let driver_written = Instant::now();
    let run_dir = loop {
        let candidate = std::fs::read_dir(project.path().join(".oxymake/runs"))
            .into_iter()
            .flatten()
            .map(|run| run.unwrap().path())
            .find(|run| run.join("oxymake_dag_driver.py").is_file());
        if let Some(run) = candidate {
            break run;
        }
        assert!(driver_written.elapsed() < Duration::from_secs(30));
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let run_id = run_dir.file_name().unwrap().to_str().unwrap().to_string();
    run_driver(project.path(), &run_dir, None);

    let output = tokio::task::spawn_blocking(move || child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        job_status(project.path(), "heavy").as_deref(),
        Some("completed")
    );
    assert_usage(project.path(), &run_id);
    // A later status synchronisation finds nothing left to record.
    ox().current_dir(project.path())
        .arg("status")
        .assert()
        .success();
    assert_eq!(history(project.path(), &run_id).len(), 2);
    assert_eq!(events(project.path(), &run_id, "job_completed").len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_names_the_endpoint_submission_and_node_of_each_ray_unit() {
    let server = ray_api("SUCCEEDED").await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("Oxymakefile.toml"), WORKFLOW).unwrap();
    ox().current_dir(project.path())
        .args(run_args(&server))
        .assert()
        .success();
    let run_dir = run_dir(project.path());
    let run_id = run_dir.file_name().unwrap().to_str().unwrap().to_string();
    run_driver(project.path(), &run_dir, None);

    // `ox status` prints the snapshot, then the Ray driver's own document.
    let output = ox()
        .current_dir(project.path())
        .args([
            "status",
            "--json",
            "--units",
            "--ray-address",
            &server.uri(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let status: Value = serde_json::Deserializer::from_slice(&output.stdout)
        .into_iter::<Value>()
        .next()
        .unwrap()
        .unwrap();

    let host = hostname();
    let units = status["units"].as_array().unwrap();
    assert_eq!(units.len(), 2, "{units:?}");
    for unit in units {
        assert_eq!(unit["executor"], "ray", "{unit}");
        assert_eq!(unit["endpoint"], server.uri().as_str(), "{unit}");
        assert_eq!(unit["submission_id"], "driver", "{unit}");
        assert_eq!(unit["node"], host.as_str(), "{unit}");
        assert_eq!(unit["state"], "completed", "{unit}");
        assert_eq!(unit["exit_code"], 0, "{unit}");
    }

    let completed = events(project.path(), &run_id, "job_completed");
    assert_eq!(completed.len(), 2, "{completed:?}");
    for event in &completed {
        assert_eq!(event["endpoint"], server.uri().as_str(), "{event}");
        assert_eq!(event["submission_id"], "driver", "{event}");
        assert_eq!(event["node"], host.as_str(), "{event}");
    }
}
