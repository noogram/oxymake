//! Issue #37: local execution must be witnessed, and attribution must agree.
#![cfg(unix)]
use assert_cmd::Command;
use predicates::prelude::*;
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
use tempfile::TempDir;

fn script(dir: &std::path::Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
fn events(bytes: &[u8]) -> Vec<serde_json::Value> {
    bytes
        .split(|b| *b == b'\n')
        .filter_map(|s| serde_json::from_slice(s).ok())
        .collect()
}
fn workflow(dir: &TempDir, value: &str) {
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        format!(
            r#"
[executor.slurm]
staging_dir = "{}"
[rule.local_work]
output = ["local.txt"]
executor = "{value}"
shell = "echo host-witness > local.txt"
[rule.remote_work]
input = ["local.txt"]
output = ["remote.txt"]
shell = "cp local.txt remote.txt"
"#,
            dir.path().join("staging").display()
        ),
    )
    .unwrap();
}
#[test]
fn mixed_slurm_routes_local_and_reports_actual_executor() {
    mixed_slurm(false);
}

#[test]
fn mixed_slurm_follow_also_uses_scheduler() {
    mixed_slurm(true);
}

fn mixed_slurm(follow: bool) {
    let dir = TempDir::new().unwrap();
    workflow(&dir, "local");
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    script(&bin, "sinfo", "echo 'slurm 24.11'");
    // A submission of local_work is a hard failure; the local executor must
    // have already produced its witness before remote_work is submitted.
    script(
        &bin,
        "sbatch",
        r#"
for arg do job_script="$arg"; done
case "$job_script" in *local_work*) exit 91;; esac
test "$(cat local.txt)" = host-witness || exit 92
echo submitted >> submissions
sh "$job_script" >&2 || exit 93
echo 42
"#,
    );
    script(&bin, "sacct", "echo '42|COMPLETED|0:0||00:00:01|mock-node'");
    script(&bin, "scancel", "exit 0");
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut command = Command::cargo_bin("ox").unwrap();
    command.args(["run", "remote.txt", "--executor", "slurm", "--json"]);
    if follow {
        command.arg("--follow");
    }
    let output = command
        .env("PATH", path.clone())
        .current_dir(dir.path())
        .timeout(Duration::from_secs(30))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        fs::read_to_string(dir.path().join("remote.txt"))
            .unwrap()
            .trim(),
        "host-witness"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("submissions"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let events = events(&output);
    let starts: Vec<_> = events
        .iter()
        .filter(|v| v["event"] == "job_started")
        .collect();
    assert_eq!(starts.len(), 2, "{events:?}");
    assert_eq!(starts[0]["executor"], "local");
    assert_eq!(starts[1]["executor"], "slurm");
    assert!(events.iter().any(|v| v["event"] == "run_completed"));
    let db = rusqlite::Connection::open(dir.path().join(".oxymake/state.db")).unwrap();
    let run_id: String = db
        .query_row("SELECT run_id FROM job_history LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let history = Command::cargo_bin("ox")
        .unwrap()
        .args(["history", "--run-id", &run_id, "--json"])
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows = self::events(&history);
    assert_eq!(rows.len(), 2, "{rows:?}");
    for row in rows {
        let expected = if row["rule_name"] == "local_work" {
            "local"
        } else {
            "slurm"
        };
        assert_eq!(row["executor"], expected, "{row}");
    }
    Command::cargo_bin("ox")
        .unwrap()
        .args(["history", "--run-id", &run_id])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(
            predicate::str::contains("EXECUTOR")
                .and(predicate::str::contains("local"))
                .and(predicate::str::contains("slurm")),
        );
    let warm = Command::cargo_bin("ox")
        .unwrap()
        .args(["run", "remote.txt", "--executor", "slurm", "--json"])
        .env("PATH", path)
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        !self::events(&warm)
            .iter()
            .any(|v| v["event"] == "job_started")
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("submissions"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}
#[test]
fn local_override_is_a_noop_in_local_run() {
    let dir = TempDir::new().unwrap();
    workflow(&dir, "local");
    let output = Command::cargo_bin("ox")
        .unwrap()
        .args(["run", "remote.txt", "--json"])
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let starts: Vec<_> = events(&output)
        .into_iter()
        .filter(|v| v["event"] == "job_started")
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(starts.iter().all(|v| v["executor"] == "local"));
}
#[test]
fn ray_dag_refuses_local_override_before_connecting() {
    let dir = TempDir::new().unwrap();
    workflow(&dir, "local");
    Command::cargo_bin("ox")
        .unwrap()
        .args([
            "run",
            "remote.txt",
            "--executor",
            "ray",
            "--ray-address",
            "http://127.0.0.1:1",
        ])
        .current_dir(dir.path())
        .timeout(Duration::from_secs(10))
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("local_work")
                .and(predicate::str::contains("DAG"))
                .and(predicate::str::contains("executor = \"local\"")),
        );
}
#[test]
fn parsing_commands_reject_other_rule_executors() {
    let dir = TempDir::new().unwrap();
    for value in ["ray", "slurm", "site-a", "locla"] {
        workflow(&dir, value);
        for command in [
            vec!["lint"],
            vec!["plan"],
            vec!["run"],
            vec!["query", "deps(remote_work)"],
            vec!["lock", "generate"],
            vec!["dag"],
            vec!["explain", "remote.txt"],
            vec!["export", "wdl"],
        ] {
            Command::cargo_bin("ox")
                .unwrap()
                .args(command)
                .current_dir(dir.path())
                .assert()
                .failure()
                .stderr(
                    predicate::str::contains("local_work")
                        .and(predicate::str::contains("local"))
                        .and(predicate::str::contains("ox run --executor")),
                );
        }
    }
}

#[test]
fn mixed_slurm_interrupt_cancels_local_process_on_this_host() {
    use std::process::{Command as ProcessCommand, Stdio};
    use std::time::Instant;
    let dir = TempDir::new().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    script(&bin, "sinfo", "echo 'slurm 24.11'");
    script(&bin, "sbatch", "touch unexpected-submit; exit 91");
    script(&bin, "scancel", "touch unexpected-cancel; exit 92");
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        r#"
[rule.slow]
output = ["out.txt"]
executor = "local"
shell = "touch started; sleep 30; touch out.txt"
"#,
    )
    .unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut child = ProcessCommand::new(env!("CARGO_BIN_EXE_ox"))
        .args([
            "run",
            "out.txt",
            "--executor",
            "slurm",
            "--json",
            "--no-cache",
        ])
        .env("PATH", path)
        .current_dir(dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !dir.path().join("started").exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "run exited before local job started"
        );
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("local job did not start");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        ProcessCommand::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("local cancellation did not finish");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(!dir.path().join("out.txt").exists());
    assert!(!dir.path().join("unexpected-submit").exists());
    assert!(!dir.path().join("unexpected-cancel").exists());
    assert!(
        events(&output.stdout)
            .iter()
            .any(|v| v["event"] == "job_started" && v["executor"] == "local")
    );
    let db = rusqlite::Connection::open(dir.path().join(".oxymake/state.db")).unwrap();
    let (status, executor): (String, String) = db
        .query_row(
            "SELECT j.status, h.executor FROM jobs j JOIN job_history h ON h.job_id = j.id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "cancelled");
    assert_eq!(executor, "local");
}
