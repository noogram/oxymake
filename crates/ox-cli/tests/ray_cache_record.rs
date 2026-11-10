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
/// `OX_STUB_HIDE=path` moves `path` to `path.hidden` just before the driver
/// writes a completed record: the record is readable while the output is not
/// yet visible, as on a network filesystem whose client has cached the
/// output's absence.
const SYNC_RAY_STUB: &str = r#"
import os, signal
_runs = 0
def _hide_on_record(scope):
    hidden = os.environ.get('OX_STUB_HIDE')
    original = scope.get('_write_record')
    if not hidden or original is None or getattr(original, 'hides', False):
        return
    def write(record):
        if record.get('status') == 'completed' and os.path.exists(hidden):
            os.replace(hidden, hidden + '.hidden')
        original(record)
    write.hides = True
    scope['_write_record'] = write
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
        _hide_on_record(self.fn.__globals__)
        try:
            resolved = [get(arg) if isinstance(arg, _Ref) else arg for arg in args]
            return _Ref(value=self.fn(*resolved))
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
            ResponseTemplate::new(200).set_body_json(
                json!({ "status": status, "message": "OTHER_TASK_TRACE: unrelated driver log" }),
            )
        })
        .mount(&server)
        .await;
    server
}

/// Run the submitted driver as the Ray job would, with `ray` stubbed.
fn run_driver(
    project: &Path,
    run_dir: &Path,
    kill_after: Option<u32>,
    hide: Option<&Path>,
    success: bool,
) {
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
    if let Some(path) = hide {
        driver.env("OX_STUB_HIDE", path);
    }
    let output = driver.output().unwrap();
    assert_eq!(
        output.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `ox run --follow` against `server`, with the driver run (and possibly
/// killed) by this test. Returns the follow process's output.
async fn follow_run(
    project: &Path,
    remote: &Path,
    targets: &[&str],
    kill_after: Option<u32>,
    terminal: &'static str,
) -> std::process::Output {
    follow_run_hiding(project, remote, targets, kill_after, terminal, None).await
}

/// [`follow_run`], with the driver hiding `hide` when it records the task
/// that produced it (see `OX_STUB_HIDE`).
async fn follow_run_hiding(
    project: &Path,
    remote: &Path,
    targets: &[&str],
    kill_after: Option<u32>,
    terminal: &'static str,
    hide: Option<&Path>,
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
        .args(targets)
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
    run_driver(project, &run_dir, kill_after, hide, terminal == "SUCCEEDED");
    done.store(true, Ordering::SeqCst);

    tokio::task::spawn_blocking(move || child.wait_with_output())
        .await
        .unwrap()
        .unwrap()
}

/// Once `ox` has logged `job`'s completion, wait `delay` and move the output
/// hidden by the driver back to `output`.
fn restore_after_completion(
    project: &Path,
    job: &'static str,
    output: PathBuf,
    delay: Duration,
) -> std::thread::JoinHandle<()> {
    let events = project.join(".oxymake/events");
    std::thread::spawn(move || {
        let started = Instant::now();
        let completed = || {
            std::fs::read_dir(&events)
                .into_iter()
                .flatten()
                .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path()).ok())
                .flat_map(|log| log.lines().map(str::to_string).collect::<Vec<_>>())
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
                .any(|event| event["event"] == "job_completed" && event["job_id"] == job)
        };
        while !completed() {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "{job} never completed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(delay);
        let mut hidden = output.clone().into_os_string();
        hidden.push(".hidden");
        std::fs::rename(hidden, output).unwrap();
    })
}

/// The jobs `ox plan` would run for `target`: none when it is cached.
fn planned_jobs(project: &Path, target: &str) -> serde_json::Value {
    let plan = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"))
        .current_dir(project)
        .args(["plan", target, "--json"])
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    plan["jobs"].clone()
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
        for entry in std::fs::read_dir(&dir).into_iter().flatten() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if dir == remote && path.file_name().is_some_and(|name| name == "entries") {
                    continue;
                }
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
    let first = follow_run(project.path(), remote.path(), &TARGETS, Some(2), "FAILED").await;
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

    // A second checkout restores the completed units from the remote entry.
    let fresh = tempfile::tempdir().unwrap();
    std::fs::create_dir(fresh.path().join("in")).unwrap();
    std::fs::write(fresh.path().join("in/s1.txt"), "a\n").unwrap();
    std::fs::write(fresh.path().join("in/s2.txt"), "b\n").unwrap();
    std::fs::write(fresh.path().join("Oxymakefile.toml"), WORKFLOW).unwrap();
    let restored = follow_run(fresh.path(), remote.path(), &TARGETS, None, "SUCCEEDED").await;
    let stderr = String::from_utf8_lossy(&restored.stderr);
    assert!(restored.status.success(), "{stderr}");
    assert!(
        stderr.contains("DAG submitted to Ray: 2 active jobs (2 cached, 4 total"),
        "{stderr}"
    );
    let fresh_ran = ran(fresh.path());
    assert_eq!(fresh_ran.len(), 2, "{fresh_ran:?}");
    let completed: BTreeSet<String> = completed.into_iter().collect();
    let expected_to_run: BTreeSet<String> =
        ["mid/s1.txt", "mid/s2.txt", "final/s1.txt", "final/s2.txt"]
            .into_iter()
            .filter(|output| !completed.contains(*output))
            .map(str::to_string)
            .collect();
    assert_eq!(
        fresh_ran.into_iter().collect::<BTreeSet<_>>(),
        expected_to_run
    );
    for (output, content) in [
        ("mid/s1.txt", "a\n"),
        ("mid/s2.txt", "b\n"),
        ("final/s1.txt", "A\n"),
        ("final/s2.txt", "B\n"),
    ] {
        assert_eq!(
            std::fs::read_to_string(fresh.path().join(output)).unwrap(),
            content
        );
    }

    // A same-checkout rerun now skips all four units from local cache state.
    let second = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"))
        .current_dir(project.path())
        .args(["run", "--executor", "ray", "--follow", "--cache-remote"])
        .arg(remote.path())
        .args(TARGETS)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "{stderr}");
    let all = ran(project.path());
    assert_eq!(
        all.len(),
        2,
        "each completed unit runs exactly once: {all:?}"
    );
    let distinct: BTreeSet<&String> = all.iter().collect();
    assert_eq!(distinct.len(), 2, "a cached unit ran again: {all:?}");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn absolute_output_outside_project_restores_from_entries() {
    let project = tempfile::tempdir().unwrap();
    let outputs = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let output = outputs.path().join("result.txt");
    let target = output.to_str().unwrap();
    let workflow = format!(
        r#"ox_version = "0.1"
[config]
results = "{}"
[rule.produce]
output = ["{{config.results}}/result.txt"]
shell = "echo ran >> ran.log && echo result > {{output}}"
"#,
        outputs.path().display()
    );
    std::fs::write(project.path().join("Oxymakefile.toml"), workflow).unwrap();

    let first = follow_run(project.path(), remote.path(), &[target], None, "SUCCEEDED").await;
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(first.status.success(), "{stderr}");
    assert_eq!(std::fs::read(&output).unwrap(), b"result\n");
    assert_eq!(
        remote_blobs(remote.path()),
        BTreeSet::from([b"result\n".to_vec()]),
        "{stderr}"
    );
    assert_eq!(ran(project.path()).len(), 1);
    let history = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"))
        .current_dir(project.path())
        .args(["history", "--json"])
        .output()
        .unwrap();
    assert!(
        history.status.success(),
        "{}",
        String::from_utf8_lossy(&history.stderr)
    );
    let rows: Vec<serde_json::Value> = String::from_utf8(history.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let details = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"))
        .current_dir(project.path())
        .args([
            "history",
            "--run-id",
            rows[0]["run_id"].as_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        details.status.success(),
        "{}",
        String::from_utf8_lossy(&details.stderr)
    );
    let rows: Vec<serde_json::Value> = String::from_utf8(details.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["job_id"], "produce", "{rows:?}");
    assert_eq!(rows[0]["exit_code"], 0, "{rows:?}");
    assert!(rows[0]["wall_time_ms"].is_number(), "{rows:?}");

    // Same local metadata: skip the unit without submitting a driver.
    let second = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"))
        .current_dir(project.path())
        .args([
            "run",
            target,
            "--executor",
            "ray",
            "--follow",
            "--cache-remote",
        ])
        .arg(remote.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "{stderr}");
    let events: Vec<serde_json::Value> = std::fs::read_dir(project.path().join(".oxymake/events"))
        .unwrap()
        .flat_map(|entry| {
            std::fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        events.iter().any(|event| {
            event["event"] == "job_skipped"
                && event["job_id"] == "produce"
                && event["reason"] == "cached"
        }),
        "{events:?}"
    );
    assert_eq!(ran(project.path()).len(), 1, "cached unit ran again");

    // Fresh local state restores the output from the remote computation entry.
    let old_state = tempfile::tempdir().unwrap();
    std::fs::remove_file(&output).unwrap();
    std::fs::rename(
        project.path().join(".oxymake"),
        old_state.path().join("state"),
    )
    .unwrap();
    let restored = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"))
        .current_dir(project.path())
        .args([
            "run",
            target,
            "--executor",
            "ray",
            "--follow",
            "--cache-remote",
        ])
        .arg(remote.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&restored.stderr);
    assert!(restored.status.success(), "{stderr}");
    assert_eq!(std::fs::read(&output).unwrap(), b"result\n");
    assert_eq!(ran(project.path()).len(), 1, "restored unit ran again");
    let db = rusqlite::Connection::open(project.path().join(".oxymake/cache/cache.db")).unwrap();
    let markers: (i64, i64) = db
        .query_row("SELECT restored, adopted FROM cache_entries", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(markers, (1, 0));
}

/// One unit writing a relative output.
const SINGLE_UNIT: &str = r#"ox_version = "0.1"
[rule.produce]
output = ["out/result.txt"]
shell = "echo ran >> ran.log && mkdir -p out && echo result > {output}"
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_visible_shortly_after_its_record_is_still_cached() {
    let project = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("Oxymakefile.toml"), SINGLE_UNIT).unwrap();
    let output = project.path().join("out/result.txt");

    // The output reappears 300 ms after `ox` has read the task's record.
    let restorer = restore_after_completion(
        project.path(),
        "produce",
        output.clone(),
        Duration::from_millis(300),
    );
    let run = follow_run_hiding(
        project.path(),
        remote.path(),
        &["out/result.txt"],
        None,
        "SUCCEEDED",
        Some(&output),
    )
    .await;
    restorer.join().unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(!stderr.contains("not visible"), "{stderr}");

    assert_eq!(
        remote_blobs(remote.path()),
        BTreeSet::from([b"result\n".to_vec()]),
        "{stderr}"
    );
    let jobs = planned_jobs(project.path(), "out/result.txt");
    assert_eq!(jobs, serde_json::json!([]), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_that_never_appears_is_named_in_a_warning_and_not_cached() {
    let project = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("Oxymakefile.toml"), SINGLE_UNIT).unwrap();
    let output = project.path().join("out/result.txt");

    let started = Instant::now();
    let run = follow_run_hiding(
        project.path(),
        remote.path(),
        &["out/result.txt"],
        None,
        "SUCCEEDED",
        Some(&output),
    )
    .await;
    let elapsed = started.elapsed();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.lines().any(|line| line.starts_with("warning:")
            && line.contains("not visible")
            && line.contains("out/result.txt")),
        "{stderr}"
    );
    // One follow round (3 s), the visibility bound (5 s), and slack.
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}\n{stderr}");
    assert!(remote_blobs(remote.path()).is_empty(), "{stderr}");

    // Restored after the run, the output still has no cache entry.
    let mut hidden = output.clone().into_os_string();
    hidden.push(".hidden");
    std::fs::rename(hidden, &output).unwrap();
    let jobs = planned_jobs(project.path(), "out/result.txt");
    assert_eq!(jobs[0]["reason"], "no cache entry", "{jobs}\n{stderr}");
}

/// An uncached upstream output must be rebuilt with the documented policy,
/// even when the run was requested through a downstream target (#65).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ray_rebuilds_existing_uncached_outputs_with_clean_outputs_policy() {
    for policy in ["always", "on-failure", "never"] {
        let project = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("Oxymakefile.toml"),
            format!(
                r#"ox_version = "0.1"
[rule.produce]
output = ["out.txt"]
clean_outputs = "{policy}"
shell = "test ! -e out.txt || exit 1; echo built >> attempts; echo complete > out.txt"
[rule.consume]
input = ["out.txt"]
output = ["final.txt"]
shell = "cat {{input}} > {{output}}"
"#
            ),
        )
        .unwrap();
        let first = follow_run(
            project.path(),
            remote.path(),
            &["out.txt"],
            None,
            "SUCCEEDED",
        )
        .await;
        assert!(
            first.status.success(),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(project.path().join("out.txt")).unwrap(),
            "complete\n"
        );
        let previous_mtime = std::fs::metadata(project.path().join("out.txt"))
            .unwrap()
            .modified()
            .unwrap();
        // Clear only cache metadata/state. Keep the output from the first run.
        std::fs::remove_dir_all(project.path().join(".oxymake")).unwrap();
        let succeeds = policy == "always";
        let empty_remote = tempfile::tempdir().unwrap();
        let second = follow_run(
            project.path(),
            empty_remote.path(),
            &["final.txt"],
            None,
            if succeeds { "SUCCEEDED" } else { "FAILED" },
        )
        .await;
        let stderr = String::from_utf8_lossy(&second.stderr);
        assert_eq!(second.status.success(), succeeds, "{policy}: {stderr}");
        assert_eq!(
            project.path().join("final.txt").exists(),
            succeeds,
            "{policy}: {stderr}"
        );
        assert!(project.path().join("out.txt").exists(), "{policy}");
        assert!(
            stderr.contains("1 job(s) have all declared outputs present but no cache entry"),
            "{stderr}"
        );
        if !succeeds {
            assert_eq!(
                std::fs::read_to_string(project.path().join("out.txt")).unwrap(),
                "complete\n"
            );
            assert_eq!(
                std::fs::metadata(project.path().join("out.txt"))
                    .unwrap()
                    .modified()
                    .unwrap(),
                previous_mtime
            );
        }
        assert_eq!(
            std::fs::read_to_string(project.path().join("attempts"))
                .unwrap()
                .lines()
                .count(),
            if succeeds { 2 } else { 1 }
        );
        if succeeds {
            assert_eq!(
                std::fs::read_to_string(project.path().join("final.txt")).unwrap(),
                "complete\n"
            );
        } else {
            assert!(stderr.contains("produce"), "{stderr}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_ray_attempts_keep_their_own_exit_codes_and_times_in_history() {
    let project = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let mut previous = None;
    for code in [7, 9] {
        std::fs::write(
            project.path().join("Oxymakefile.toml"),
            format!(
                r#"ox_version = "0.1"
[rule.fail]
output = ["out.txt"]
shell = "sleep 1.1; echo own_failure >&2; exit {code}"
"#
            ),
        )
        .unwrap();
        let output = follow_run(project.path(), remote.path(), &["out.txt"], None, "FAILED").await;
        assert!(!output.status.success());
        let db = rusqlite::Connection::open(project.path().join(".oxymake/state.db")).unwrap();
        let row: (String, i64, i64, i32) = db.query_row(
            "SELECT run_id, started_at, completed_at, exit_code FROM job_history WHERE job_id = 'fail' ORDER BY id DESC LIMIT 1",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).unwrap();
        let record: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                project
                    .path()
                    .join(".oxymake/runs")
                    .join(&row.0)
                    .join("tasks/fail.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(record["exit_code"], code);
        assert_eq!(row.3, code);
        assert_eq!(row.1, record["started_at"].as_f64().unwrap() as i64);
        assert_eq!(row.2, record["ended_at"].as_f64().unwrap() as i64);
        assert!(row.2 > row.1);
        if let Some((run, start, end, exit)) = previous {
            let old: (i64, i64, i32) = db.query_row(
                "SELECT started_at, completed_at, exit_code FROM job_history WHERE run_id = ? AND job_id = 'fail'",
                [run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
            assert_eq!(old, (start, end, exit));
            assert!(row.1 > end);
        }
        previous = Some(row);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follow_does_not_attribute_driver_trace_to_a_dependency_failed_task() {
    let project = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("Oxymakefile.toml"),
        r#"ox_version = "0.1"
[rule.produce]
output = ["out.txt"]
shell = "echo UPSTREAM_FAILURE >&2; exit 7"
[rule.consume]
input = ["out.txt"]
output = ["final.txt"]
shell = "echo ran > consumer-ran; cat {input} > {output}"
"#,
    )
    .unwrap();
    let output = follow_run(
        project.path(),
        remote.path(),
        &["final.txt"],
        None,
        "FAILED",
    )
    .await;
    assert!(!output.status.success());
    assert!(!project.path().join("consumer-ran").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("UPSTREAM_FAILURE"), "{stderr}");
    let failure = stderr
        .lines()
        .find(|line| line.contains("FAILED: consume"))
        .expect("dependent failure is reported");
    assert!(
        failure.contains("dependency") && failure.contains("produce"),
        "{stderr}"
    );
    assert!(
        !failure.contains("OTHER_TASK_TRACE") && !failure.contains("UPSTREAM_FAILURE"),
        "{stderr}"
    );
}
