//! Completed Ray tasks enter the cache, locally and in `--cache-remote` (#59).
//!
//! As in `ray_task_records.rs`, `ox run --executor ray --follow` submits to a
//! mocked Jobs API and the test runs the driver it generated under a
//! synchronous stand-in for the `ray` module: the recipes really execute and
//! the task records are real, but Ray's scheduler is not exercised.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
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

/// Four units in two stages; every output has distinct content, and each
/// execution appends its output path to `ran.log`.
const WORKFLOW: &str = r#"ox_version = "0.1"
[rule.mid]
input = ["in/{sample}.txt"]
output = ["mid/{sample}.txt"]
shell = "echo {output} >> ran.log && mkdir -p mid && cat {input} > {output}"
[rule.final]
input = ["mid/{sample}.txt"]
output = ["final/{sample}.txt"]
shell = "echo {output} >> ran.log && mkdir -p final && tr a-z A-Z < {input} > {output}"
"#;

const TARGETS: [&str; 2] = ["final/s1.txt", "final/s2.txt"];

/// A Jobs API whose driver reports `RUNNING` until `done` is set, then
/// `terminal`.
async fn ray_api(done: Arc<AtomicBool>, terminal: &'static str) -> MockServer {
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
            let status = if done.load(Ordering::SeqCst) {
                terminal
            } else {
                "RUNNING"
            };
            ResponseTemplate::new(200).set_body_json(json!({ "status": status }))
        })
        .mount(&server)
        .await;
    server
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

/// `ox run --follow` against `server`, with the driver run (and possibly
/// killed) by this test. Returns the follow process's output.
async fn follow_run(
    project: &Path,
    remote: &Path,
    kill_after: Option<u32>,
    terminal: &'static str,
) -> std::process::Output {
    let done = Arc::new(AtomicBool::new(false));
    let server = ray_api(done.clone(), terminal).await;
    let runs = project.join(".oxymake/runs");
    let earlier: BTreeSet<PathBuf> = std::fs::read_dir(&runs)
        .into_iter()
        .flatten()
        .map(|run| run.unwrap().path())
        .collect();

    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"))
        .current_dir(project)
        .arg("run")
        .args(TARGETS)
        .args(["--executor", "ray", "--ray-allow-pending", "--follow"])
        .arg("--cache-remote")
        .arg(remote)
        .args(["--ray-address", &server.uri()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let started = Instant::now();
    let run_dir = loop {
        let candidate = std::fs::read_dir(&runs)
            .into_iter()
            .flatten()
            .map(|run| run.unwrap().path())
            .find(|run| !earlier.contains(run) && run.join("oxymake_dag_driver.py").is_file());
        if let Some(run) = candidate {
            break run;
        }
        assert!(started.elapsed() < Duration::from_secs(30));
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    run_driver(project, &run_dir, kill_after);
    done.store(true, Ordering::SeqCst);

    tokio::task::spawn_blocking(move || child.wait_with_output())
        .await
        .unwrap()
        .unwrap()
}

/// Output paths in the order their recipes executed.
fn ran(project: &Path) -> Vec<String> {
    std::fs::read_to_string(project.join("ran.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Contents of every blob stored in the remote cache directory.
fn remote_blobs(remote: &Path) -> BTreeSet<Vec<u8>> {
    let mut blobs = BTreeSet::new();
    let mut dirs = vec![remote.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).into_iter().flatten() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                blobs.insert(std::fs::read(path).unwrap());
            }
        }
    }
    blobs
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_ray_driver_leaves_its_completed_units_cached_locally_and_remotely() {
    let project = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join("in")).unwrap();
    std::fs::write(project.path().join("in/s1.txt"), "a\n").unwrap();
    std::fs::write(project.path().join("in/s2.txt"), "b\n").unwrap();
    std::fs::write(project.path().join("Oxymakefile.toml"), WORKFLOW).unwrap();

    // The driver is killed after two of the four units.
    let first = follow_run(project.path(), remote.path(), Some(2), "FAILED").await;
    let completed = ran(project.path());
    assert_eq!(
        completed.len(),
        2,
        "{completed:?}\n{}",
        String::from_utf8_lossy(&first.stderr)
    );

    // The remote directory holds the blobs of exactly the completed units.
    let expected: BTreeSet<Vec<u8>> = completed
        .iter()
        .map(|output| std::fs::read(project.path().join(output)).unwrap())
        .collect();
    assert_eq!(
        remote_blobs(remote.path()),
        expected,
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    // Resubmission: the two completed units are cache hits, the other two run.
    let second = follow_run(project.path(), remote.path(), None, "SUCCEEDED").await;
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "{stderr}");
    assert!(
        stderr.contains("DAG submitted to Ray: 2 active jobs (2 cached, 4 total"),
        "{stderr}"
    );
    let all = ran(project.path());
    assert_eq!(all.len(), 4, "each unit runs exactly once: {all:?}");
    let distinct: BTreeSet<&String> = all.iter().collect();
    assert_eq!(distinct.len(), 4, "a cached unit ran again: {all:?}");
    for (output, content) in [
        ("mid/s1.txt", "a\n"),
        ("mid/s2.txt", "b\n"),
        ("final/s1.txt", "A\n"),
        ("final/s2.txt", "B\n"),
    ] {
        assert_eq!(
            std::fs::read_to_string(project.path().join(output)).unwrap(),
            content
        );
    }
}
