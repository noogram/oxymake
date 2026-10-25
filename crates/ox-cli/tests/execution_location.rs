//! Where each unit ran: `endpoint`, `submission_id` and `node` in the run's
//! events and in `ox status --json --units` (#62).
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;

/// `hit` is built by a first run, so the second run takes it from the cache;
/// `fresh` runs; `broken` fails.
const WORKFLOW: &str = r#"ox_version = "0.1"

[rule.hit]
output = ["hit.out"]
shell = "echo hit > hit.out"

[rule.fresh]
input = ["hit.out"]
output = ["fresh.out"]
shell = "echo fresh > fresh.out"

[rule.slow]
output = ["slow.out"]
shell = "sleep 20; echo slow > slow.out"

[rule.broken]
input = ["hit.out"]
output = ["broken.out"]
shell = "exit 3"
"#;

fn ox(project: &Path) -> Command {
    let mut command = Command::cargo_bin("ox").unwrap();
    command.current_dir(project);
    command
}

fn local_host() -> String {
    ox_state::host::hostname().to_string()
}

fn events(project: &Path, name: &str) -> Vec<Value> {
    let mut logs: Vec<_> = std::fs::read_dir(project.join(".oxymake/events"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    logs.sort();
    std::fs::read_to_string(logs.last().unwrap())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["event"] == name)
        .collect()
}

/// Run IDs that have an event log.
fn run_ids(project: &Path) -> Vec<String> {
    std::fs::read_dir(project.join(".oxymake/events"))
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(".ndjson").map(str::to_string)
        })
        .collect()
}

fn project() -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("Oxymakefile.toml"), WORKFLOW).unwrap();
    project
}

#[test]
fn local_events_carry_the_host_and_null_submission() {
    let project = project();
    ox(project.path())
        .args(["run", "hit.out"])
        .assert()
        .success();

    let host = local_host();
    let started = events(project.path(), "job_started");
    let completed = events(project.path(), "job_completed");
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(completed.len(), 1, "{completed:?}");
    for event in started.iter().chain(&completed) {
        assert_eq!(event["endpoint"], host.as_str(), "{event}");
        // Present and null, not absent: consumers can rely on the key.
        assert_eq!(event.get("submission_id"), Some(&Value::Null), "{event}");
    }
    assert_eq!(started[0].get("node"), Some(&Value::Null), "{}", started[0]);
}

#[test]
fn status_units_gives_one_record_per_mixed_unit() {
    let project = project();
    ox(project.path())
        .args(["run", "hit.out"])
        .assert()
        .success();
    let first = run_ids(project.path());
    // Second run: `hit` is a cache hit, `fresh` succeeds, `broken` fails.
    ox(project.path())
        .args(["run", "fresh.out", "broken.out", "--keep-going"])
        .assert()
        .failure();

    // Runs started within the same second are not ordered by "latest".
    let second: Vec<String> = run_ids(project.path())
        .into_iter()
        .filter(|id| !first.contains(id))
        .collect();
    assert_eq!(second.len(), 1, "{second:?}");
    let output = ox(project.path())
        .args(["status", "--json", "--units", "--run", &second[0]])
        .output()
        .unwrap();
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["schema_version"], 1);
    let units = status["units"].as_array().unwrap();
    let unit = |rule: &str| {
        units
            .iter()
            .find(|unit| unit["rule"] == rule)
            .unwrap_or_else(|| panic!("no {rule} in {units:?}"))
    };

    let host = local_host();
    let (hit, fresh, broken) = (unit("hit"), unit("fresh"), unit("broken"));
    assert_eq!(hit["cache_hit"], true, "{hit}");
    assert_eq!(hit["state"], "completed", "{hit}");
    assert_eq!(fresh["cache_hit"], false, "{fresh}");
    assert_eq!(fresh["state"], "completed", "{fresh}");
    assert_eq!(fresh["exit_code"], 0, "{fresh}");
    assert_eq!(broken["state"], "failed", "{broken}");
    assert_eq!(broken["exit_code"], 3, "{broken}");
    for unit in [fresh, broken] {
        assert_eq!(unit["executor"], "local", "{unit}");
        assert_eq!(unit["endpoint"], host.as_str(), "{unit}");
        assert_eq!(unit["submission_id"], Value::Null, "{unit}");
        assert_eq!(unit["node"], Value::Null, "{unit}");
        assert!(unit["wildcards"].is_object(), "{unit}");
        assert!(unit["started_at"].as_u64().unwrap() <= unit["ended_at"].as_u64().unwrap());
    }
    for key in ["job_id", "outputs"] {
        assert!(fresh.get(key).is_some(), "{key} missing in {fresh}");
    }
}

#[test]
fn units_flag_needs_json() {
    let project = project();
    ox(project.path())
        .args(["status", "--units"])
        .assert()
        .failure();
}

fn history(project: &Path) -> Vec<Value> {
    let output = ox(project).args(["history", "--json"]).output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// `ox history --json` lists the runs of a workflow folder, a run still in
/// progress included, so no separate listing command is needed.
#[test]
fn history_lists_a_concurrent_run_still_in_progress() {
    let project = project();
    let mut slow = std::process::Command::new(assert_cmd::cargo::cargo_bin("ox"))
        .current_dir(project.path())
        .args(["run", "slow.out"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while history(project.path()).is_empty() {
        assert!(started.elapsed() < std::time::Duration::from_secs(15));
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    ox(project.path())
        .args(["run", "hit.out"])
        .assert()
        .success();

    let runs = history(project.path());
    let _ = slow.kill();
    let _ = slow.wait();
    assert_eq!(runs.len(), 2, "{runs:?}");
    let open: Vec<_> = runs
        .iter()
        .filter(|r| r["completed_at"].is_null())
        .collect();
    let done: Vec<_> = runs
        .iter()
        .filter(|r| !r["completed_at"].is_null())
        .collect();
    assert_eq!((open.len(), done.len()), (1, 1), "{runs:?}");
    assert_ne!(open[0]["run_id"], done[0]["run_id"]);
}
