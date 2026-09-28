//! End-to-end coverage for local `ox run --resource-budget` admission.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::process::Command as ProcessCommand;
use tempfile::TempDir;

fn ox() -> Command {
    Command::cargo_bin("oxymake").expect("binary should exist")
}

fn write_workflow(dir: &TempDir, first_resources: &str, second_resources: &str) {
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        format!(
            r#"ox_version = "0.1"

[rule.a]
output = ["a.out"]
resources = {{ {first_resources} }}
shell = "sleep 0.05; touch a.out"

[rule.b]
output = ["b.out"]
resources = {{ {second_resources} }}
shell = "sleep 0.05; touch b.out"
"#,
        ),
    )
    .unwrap();
}

fn events(stdout: &[u8]) -> Vec<serde_json::Value> {
    stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect()
}

fn assert_serialized(events: &[serde_json::Value]) {
    let starts: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| event["event"] == "job_started")
        .map(|(index, _)| index)
        .collect();
    let completed: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| event["event"] == "job_completed")
        .map(|(index, _)| index)
        .collect();

    assert_eq!(starts.len(), 2, "expected both jobs to start: {events:?}");
    assert_eq!(
        completed.len(),
        2,
        "expected both jobs to complete: {events:?}"
    );
    assert!(
        completed[0] < starts[1],
        "the second job started before the first completed: {events:?}"
    );
}

#[test]
fn absent_resource_budget_preserves_unconstrained_dispatch_and_budget_rejects_impossible_jobs() {
    let unconstrained = TempDir::new().unwrap();
    write_workflow(&unconstrained, "cpu = 999", "cpu = 999");
    let output = ox()
        .args(["run", "a.out", "b.out", "--no-cache", "--json", "-j", "2"])
        .current_dir(unconstrained.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let unconstrained_events = events(&output.stdout);
    let first_complete = unconstrained_events
        .iter()
        .position(|event| event["event"] == "job_completed")
        .unwrap();
    let starts_before_completion = unconstrained_events[..first_complete]
        .iter()
        .filter(|event| event["event"] == "job_started")
        .count();
    assert_eq!(starts_before_completion, 2, "{unconstrained_events:?}");

    let constrained = TempDir::new().unwrap();
    write_workflow(&constrained, "cpu = 999", "cpu = 999");
    ox().args([
        "run",
        "a.out",
        "b.out",
        "--no-cache",
        "-j",
        "2",
        "--resource-budget",
        "cpu=6",
    ])
    .current_dir(constrained.path())
    .assert()
    .failure()
    .stderr(predicate::str::contains("job "))
    .stderr(predicate::str::contains("requests cpu"))
    .stderr(predicate::str::contains("more than the whole budget"));
    assert!(!constrained.path().join("a.out").exists());
    assert!(!constrained.path().join("b.out").exists());
}

#[test]
fn resource_budget_serializes_admission_without_changing_the_job_limit() {
    let resource_limited = TempDir::new().unwrap();
    write_workflow(&resource_limited, "cpu = 4", "cpu = 4");
    let output = ox()
        .args([
            "run",
            "a.out",
            "b.out",
            "--no-cache",
            "--json",
            "-j",
            "2",
            "--resource-budget",
            "cpu=6",
        ])
        .current_dir(resource_limited.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_serialized(&events(&output.stdout));

    let job_limited = TempDir::new().unwrap();
    write_workflow(&job_limited, "cpu = 4", "cpu = 4");
    let output = ox()
        .args([
            "run",
            "a.out",
            "b.out",
            "--no-cache",
            "--json",
            "-j",
            "1",
            "--resource-budget",
            "cpu=8",
        ])
        .current_dir(job_limited.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_serialized(&events(&output.stdout));
}

#[test]
fn undeclared_resources_consume_a_job_slot_but_not_a_resource_reservation() {
    let dir = TempDir::new().unwrap();
    write_workflow(&dir, "", "cpu = 6");
    let output = ox()
        .args([
            "run",
            "a.out",
            "b.out",
            "--no-cache",
            "--json",
            "-j",
            "2",
            "--resource-budget",
            "cpu=6",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let observed = events(&output.stdout);
    let first_complete = observed
        .iter()
        .position(|event| event["event"] == "job_completed")
        .unwrap();
    assert_eq!(
        observed[..first_complete]
            .iter()
            .filter(|event| event["event"] == "job_started")
            .count(),
        2,
        "a rule without a declaration must not reserve cpu: {observed:?}"
    );
}

#[test]
fn resource_budget_is_rejected_for_remote_executors() {
    let dir = TempDir::new().unwrap();
    write_workflow(&dir, "cpu = 1", "cpu = 1");
    for executor in ["ray", "slurm"] {
        ox().args([
            "run",
            "a.out",
            "--executor",
            executor,
            "--resource-budget",
            "cpu=6",
        ])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--resource-budget applies to the local executor only",
        ));
    }
}

#[test]
fn independent_processes_each_receive_their_own_resource_budget() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        r#"ox_version = "0.1"

[rule.a]
output = ["a.out"]
resources = { cpu = 6 }
shell = "touch a.started; for i in $(seq 1 500); do [ -f b.started ] && break; sleep 0.01; done; test -f b.started; touch a.out"

[rule.b]
output = ["b.out"]
resources = { cpu = 6 }
shell = "touch b.started; for i in $(seq 1 500); do [ -f a.started ] && break; sleep 0.01; done; test -f a.started; touch b.out"
"#,
    )
    .unwrap();

    let binary = assert_cmd::cargo::cargo_bin("oxymake");
    let first = ProcessCommand::new(&binary)
        .args(["run", "a.out", "--no-cache", "--resource-budget", "cpu=6"])
        .current_dir(dir.path())
        .spawn()
        .unwrap();
    let second = ProcessCommand::new(&binary)
        .args(["run", "b.out", "--no-cache", "--resource-budget", "cpu=6"])
        .current_dir(dir.path())
        .spawn()
        .unwrap();

    assert!(first.wait_with_output().unwrap().status.success());
    assert!(second.wait_with_output().unwrap().status.success());
    assert!(dir.path().join("a.out").exists());
    assert!(dir.path().join("b.out").exists());
}

#[test]
fn budget_usage_errors_have_exit_two_and_newline() {
    let dir = TempDir::new().unwrap();
    write_workflow(&dir, "cpu = 1", "cpu = 1");
    for flags in [
        vec!["--resource-budget", "cpu"],
        vec!["--resource-budget", "cpu="],
        vec!["--resource-budget", "=1"],
        vec!["--resource-budget", "cpu=1,cpu=2"],
        vec!["--resource-budget", "cpu=1,cpus=1"],
        vec!["--resource-budget", "memory=1XB"],
        vec!["--resource-budget", "gpu=0.5"],
        vec!["--resource-budget", "cpu=1", "--executor", "ray"],
        vec!["--ray-allow-pending"],
    ] {
        let output = ox()
            .args(["run", "a.out"])
            .args(&flags)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{flags:?}: {output:?}");
        assert!(output.stderr.ends_with(b"\n"), "{flags:?}: {output:?}");
    }
}

#[test]
fn impossible_declarations_fail_even_when_everything_is_cached() {
    let dir = TempDir::new().unwrap();
    write_workflow(&dir, "cpu = 999", "cpu = 999");
    ox().args(["run", "a.out", "b.out"])
        .current_dir(dir.path())
        .assert()
        .success();
    // Prove the fixture really takes the fully cached path without a budget.
    ox().args(["run", "a.out", "b.out"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("2 job(s) up-to-date"));
    ox().args(["run", "a.out", "b.out", "--resource-budget", "cpu=6"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("more than the whole budget"));
}

#[test]
fn admission_failure_has_no_success_summary() {
    let dir = TempDir::new().unwrap();
    write_workflow(&dir, "cpu = 999", "cpu = 999");
    let output = ox()
        .args(["run", "a.out", "--resource-budget", "cpu=6"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined.contains("Completed"), "{combined}");
}

#[test]
fn admission_memory_error_preserves_declared_unit() {
    let dir = TempDir::new().unwrap();
    write_workflow(&dir, "mem_mb = 100", "");
    ox().args(["run", "a.out", "--resource-budget", "mem_mb=50"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("mem_mb = 100 (104857600 bytes)"));
}

#[test]
fn failed_logs_continue_past_missing_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join(".oxymake");
    fs::create_dir_all(root.join("logs")).unwrap();
    let db = ox_state::db::StateDb::open(&root.join("state.db")).unwrap();
    let session = db
        .create_session(std::process::id(), "localhost", None)
        .unwrap();
    for id in ["a_missing", "b_present"] {
        db.register_jobs(&[ox_state::db::JobRecord {
            id: id.into(),
            rule_name: id.into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: None,
        }])
        .unwrap();
        assert!(db.claim_job(id, &session).unwrap());
        assert!(db.fail_job(id, &session, 1).unwrap());
    }
    fs::write(
        root.join("logs/b_present.log"),
        "surviving failure detail\n",
    )
    .unwrap();
    ox().args(["logs", "--failed"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("==> a_missing <=="))
        .stdout(predicate::str::contains("No log available"))
        .stdout(predicate::str::contains("==> b_present <=="))
        .stdout(predicate::str::contains("surviving failure detail"));
}
