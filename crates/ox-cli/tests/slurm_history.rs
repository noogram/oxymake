//! Issue #24: exercise submission, accounting collection and SQLite finalization
//! through the real CLI. Each child has its own mock PATH; no global env edits.
#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

use assert_cmd::Command;
use tempfile::TempDir;

fn script(dir: &std::path::Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn follow(accounting: &str, status: &str, expected: Option<i64>, fallback: bool) {
    let dir = TempDir::new().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    script(&bin, "sinfo", "echo 'slurm 24.11'");
    script(&bin, "sbatch", "echo 42");
    script(&bin, "squeue", "echo COMPLETED");
    script(&bin, "scancel", "exit 0");
    script(
        &bin,
        "sacct",
        &if fallback {
            "exit 1".into()
        } else {
            format!("cat <<'ACCOUNTING'\n{accounting}\nACCOUNTING")
        },
    );
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        format!(
            r#"ox_version = "0.1"
[executor.slurm]
staging_dir = "{}"
[rule.work]
output = ["result.txt"]
shell = "echo done > result.txt"
"#,
            dir.path().join("staging").display()
        ),
    )
    .unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let result = Command::cargo_bin("ox")
        .unwrap()
        .args(["run", "--executor", "slurm", "--follow", "--no-cache"])
        .current_dir(dir.path())
        .env("PATH", path)
        .timeout(Duration::from_secs(30))
        .assert();
    if status == "completed" {
        result.success();
    } else {
        result.failure();
    }
    let db = rusqlite::Connection::open(dir.path().join(".oxymake/state.db")).unwrap();
    let (run_id, recorded_status, memory, elapsed): (String, String, Option<i64>, Option<i64>) = db
        .query_row(
            "SELECT h.run_id, j.status, h.peak_mem_mb, h.wall_time_ms FROM job_history h JOIN jobs j ON j.id = h.job_id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .expect("follow must finalize a history row");
    assert_eq!(recorded_status, status);
    assert_eq!(memory, expected);
    if status == "completed" {
        assert_eq!(elapsed, if fallback { None } else { Some(3000) });
    }
    let current_status: String = db
        .query_row("SELECT status FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(current_status, status);
    let output = Command::cargo_bin("ox")
        .unwrap()
        .args(["history", "--run-id", &run_id, "--json"])
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let row: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(row["peak_mem_mb"], serde_json::json!(expected));
    Command::cargo_bin("ox")
        .unwrap()
        .args(["history", "--run-id", &run_id])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("MEM MiB"));
}

#[test]
fn slurm_follow_collects_maximum_step_rss_with_allocation_status() {
    // Steps may precede the allocation; their state and elapsed are not authoritative.
    follow(
        "42.0|FAILED|1:0|1500K|00:00:01|c1\n42|COMPLETED|0:0||00:00:03|c1\n42.batch|COMPLETED|0:0|512K|00:00:02|c1\n42.extern|COMPLETED|0:0|0|00:00:03|c1",
        "completed",
        Some(2),
        false,
    );
}

#[test]
fn slurm_follow_preserves_sub_mib_measurement() {
    follow(
        "42|COMPLETED|0:0||00:00:03|c1\n42.batch|COMPLETED|0:0|1|00:00:03|c1",
        "completed",
        Some(1),
        false,
    );
}

#[test]
fn slurm_follow_zero_is_unavailable() {
    follow(
        "42|COMPLETED|0:0|0|00:00:03|c1\n42.batch|COMPLETED|0:0|0K|00:00:03|c1",
        "completed",
        None,
        false,
    );
}

#[test]
fn slurm_follow_missing_accounting_is_unavailable() {
    follow("42|COMPLETED|0:0||00:00:03|c1", "completed", None, false);
}

#[test]
fn slurm_follow_squeue_fallback_is_unavailable() {
    follow("", "completed", None, true);
}

#[test]
fn slurm_follow_failure_does_not_record_memory() {
    follow(
        "42|FAILED|7:0||00:00:03|c1\n42.batch|FAILED|7:0|2M|00:00:03|c1",
        "failed",
        None,
        false,
    );
}

#[test]
fn slurm_follow_cancellation_does_not_record_memory() {
    follow(
        "42|CANCELLED|0:15||00:00:03|c1\n42.batch|CANCELLED|0:15|2M|00:00:03|c1",
        "cancelled",
        None,
        false,
    );
}
