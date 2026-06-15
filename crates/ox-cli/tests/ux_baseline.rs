//! UX Baseline Scenarios (hq-3laln)
//!
//! These tests document current CLI behavior for key user-facing workflows.
//! Some may fail ("red") to establish a baseline before fixes.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use std::fs;
use tempfile::TempDir;

fn ox() -> Command {
    Command::cargo_bin("oxymake").expect("binary should exist")
}

// ===========================================================================
// Scenario 1: `ox test` — workflow validation via CLI
// ===========================================================================

/// `ox test` on a valid workflow should pass all checks.
#[test]
fn test_command_valid_workflow() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    ox().args(["test", "-f", oxymakefile.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("All checks passed"));
}

/// `ox test --json` on a valid workflow should produce NDJSON check results.
#[test]
fn test_command_json_valid_workflow() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    let output = ox()
        .args(["test", "--json", "-f", oxymakefile.to_str().unwrap()])
        .output()
        .expect("command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Every non-empty line must be valid JSON.
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed: serde_json::Value =
            serde_json::from_str(line).expect("each line should be valid JSON");
        assert!(
            parsed["check"].is_string(),
            "each line must have a 'check' field"
        );
        assert!(
            parsed["status"].is_string(),
            "each line must have a 'status' field"
        );
    }
}

/// `ox test` on a workflow with a cycle should report the cycle.
#[test]
fn test_command_detects_cycle() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
input = ["b.txt"]
output = ["a.txt"]
shell = "echo a > a.txt"

[rule.step_b]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo b > b.txt"
"#,
    )
    .unwrap();

    ox().args(["test", "-f", oxymakefile.to_str().unwrap()])
        .assert()
        .failure()
        .stdout(predicates::str::contains("FAIL"));
}

/// `ox test` on a missing file should fail with a clear error.
#[test]
fn test_command_missing_file() {
    ox().args(["test", "-f", "/nonexistent/Oxymakefile.toml"])
        .assert()
        .failure();
}

// ===========================================================================
// Scenario 2: `--rule` filtering
// ===========================================================================

/// `--rule fast_process` filters jobs to a single rule.
#[test]
fn rule_filter_selects_named_rule() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::write(base.join("data/A.csv"), "d").unwrap();
    fs::write(base.join("data/B.csv"), "d").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A", "B"]

[rule.fast_process]
input = ["data/{sample}.csv"]
output = ["out/{sample}_fast.txt"]
shell = "cp data/{sample}.csv out/{sample}_fast.txt"
tags = { type = "fast" }

[rule.slow_process]
input = ["data/{sample}.csv"]
output = ["out/{sample}_slow.txt"]
shell = "cp data/{sample}.csv out/{sample}_slow.txt"
tags = { type = "slow" }
"#,
    )
    .unwrap();

    // With --rule fast_process, only the two fast_process jobs should run.
    let output = ox()
        .args([
            "run",
            "--dry-run",
            "-f",
            oxymakefile.to_str().unwrap(),
            "--rule",
            "fast_process",
            "out/A_fast.txt",
            "out/B_fast.txt",
        ])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("2 job(s) would execute"),
        "Expected 2 jobs for --rule fast_process, got: {stdout}"
    );
}

// ===========================================================================
// Scenario 3: `ox history` after a run
// ===========================================================================

/// After `ox run`, `ox history` should list the completed run.
#[test]
fn history_after_run_shows_completed_run() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    // Run the workflow first.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    // History should show at least one run.
    ox().args(["history", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("run-"));
}

/// `ox history --json` should produce valid NDJSON after a run.
#[test]
fn history_json_after_run() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    let output = ox()
        .args(["history", "--json", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .output()
        .expect("command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "history --json should produce output");

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed: serde_json::Value =
            serde_json::from_str(line).expect("each line should be valid JSON");
        assert!(
            parsed["run_id"].is_string(),
            "each line must have a 'run_id' field"
        );
        assert!(
            parsed["total_jobs"].is_number(),
            "each line must have 'total_jobs'"
        );
    }
}

// ===========================================================================
// Scenario 4: Error UX — circular dependencies
// ===========================================================================

/// Circular dependencies in a workflow should produce a user-friendly error.
#[test]
fn run_circular_dependency_error() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
input = ["b.txt"]
output = ["a.txt"]
shell = "echo a > a.txt"

[rule.step_b]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo b > b.txt"
"#,
    )
    .unwrap();

    let output = ox()
        .args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    // Error message should mention the cycle or dependency issue.
    assert!(
        stderr.contains("cycle") || stderr.contains("circular") || stderr.contains("error"),
        "Circular dep error message should be user-friendly, got: {stderr}"
    );
}

// ===========================================================================
// Scenario 5: Error UX — missing input files
// ===========================================================================

/// Running a workflow with missing source files should produce a clear error.
#[test]
fn run_missing_input_files_error() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "cp data/{sample}.csv results/{sample}.txt"
"#,
    )
    .unwrap();

    // No data/ directory or files — should fail with clear error.
    let output = ox()
        .args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    // Should mention the missing file or "no job found".
    assert!(
        stderr.contains("data/A.csv")
            || stderr.contains("no job found")
            || stderr.contains("missing")
            || stderr.contains("not found")
            || stderr.contains("error"),
        "Missing input error should be descriptive, got stderr: {stderr}"
    );
}

// ===========================================================================
// Scenario 6: `ox lock generate` + `ox lock verify`
// ===========================================================================

/// Lock generate creates a lockfile, lock verify passes on unchanged state.
#[test]
fn lock_generate_and_verify_roundtrip() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    // Generate lockfile.
    ox().args(["lock", "generate", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    // Lockfile should exist.
    assert!(
        base.join("ox.lock").exists(),
        "ox.lock should be created by lock generate"
    );

    // Verify should pass on unchanged state.
    ox().args(["lock", "verify", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();
}

/// Lock verify should fail after modifying the workflow.
#[test]
fn lock_verify_detects_drift() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    // Generate lockfile.
    ox().args(["lock", "generate", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    // Modify workflow after locking.
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo CHANGED > a.txt"

[rule.step_b]
output = ["b.txt"]
shell = "echo new_rule > b.txt"

[rule.all]
input = ["a.txt", "b.txt"]
"#,
    )
    .unwrap();

    // Verify should fail — workflow changed since lock.
    ox().args(["lock", "verify", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .failure();
}

// ===========================================================================
// Scenario 7: `ox run --json` event contract
// ===========================================================================

/// `ox run --json` events must have well-defined shapes for programmatic use.
#[test]
fn run_json_events_have_required_fields() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    let output = ox()
        .args(["run", "--json", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .output()
        .expect("command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    let events: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each line should be valid JSON"))
        .collect();

    assert!(!events.is_empty(), "should produce at least one event");

    // Every event must have an "event" field.
    for (i, ev) in events.iter().enumerate() {
        assert!(
            ev["event"].is_string(),
            "event {i} missing 'event' field: {ev}"
        );
    }

    // Should contain at least a job_started and job_completed event.
    let event_types: Vec<&str> = events
        .iter()
        .map(|e| e["event"].as_str().unwrap_or(""))
        .collect();

    assert!(
        event_types.contains(&"job_started") || event_types.contains(&"job_queued"),
        "Expected a job lifecycle event, got types: {event_types:?}"
    );
    assert!(
        event_types.contains(&"job_completed") || event_types.contains(&"run_completed"),
        "Expected a completion event, got types: {event_types:?}"
    );
}

// ===========================================================================
// Scenario 8: Partial re-execution after input change
// ===========================================================================

/// When an input file is modified, only affected jobs should re-execute
/// (not the entire DAG).
#[test]
fn partial_reexecution_after_input_change() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::write(base.join("data/A.csv"), "original_a").unwrap();
    fs::write(base.join("data/B.csv"), "original_b").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A", "B"]

[rule.all]
input = ["out/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["out/{sample}.txt"]
shell = "cp data/{sample}.csv out/{sample}.txt"
"#,
    )
    .unwrap();

    // First run: both jobs execute.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("2 succeeded"));

    // Modify only A's input — sleep to ensure mtime differs.
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(base.join("data/A.csv"), "modified_a").unwrap();

    // Second run: only A should re-execute, B should be cached.
    let output = ox()
        .args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("1 succeeded"),
        "Expected only 1 job to re-execute after single input change, got: {stdout}"
    );
}

// ===========================================================================
// Scenario 9: `ox run` with shell failure shows stderr
// ===========================================================================

/// When a job's shell command fails, the error output should include
/// the rule name and some indication of the failure.
#[test]
fn run_failure_shows_rule_and_error() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.broken_step]
output = ["a.txt"]
shell = "echo 'something went wrong' >&2 && exit 1"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    let output = ox()
        .args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .failure();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    // The failure summary should mention the rule name.
    assert!(
        stdout.contains("broken_step") || stdout.contains("1 failed"),
        "Failure output should mention rule name or failure count, got: {stdout}"
    );
}

// ===========================================================================
// Scenario 10: `ox plan --json` output contract
// ===========================================================================

/// `ox plan --json` should produce valid JSON showing the plan.
///
/// BASELINE RED: `ox plan --json` emits multi-line JSON (pretty-printed),
/// not NDJSON like other `--json` commands. Consumers expecting one-JSON-per-line
/// will break.
#[test]
#[ignore = "baseline-red: plan --json emits multi-line JSON, not NDJSON"]
fn plan_json_output_contract() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::write(base.join("data/A.csv"), "d").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "cp data/{sample}.csv results/{sample}.txt"
"#,
    )
    .unwrap();

    let output = ox()
        .args(["plan", "--json", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .output()
        .expect("command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should produce valid JSON or NDJSON.
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("plan --json line is not valid JSON: {e}\nline: {line}"));
    }
}

// ===========================================================================
// Scenario 11: `ox clean` after a run removes artifacts
// ===========================================================================

/// After running, `ox clean` should remove the .oxymake directory.
///
/// BASELINE RED: `ox clean` does not remove `.oxymake/` by default.
/// It may require explicit confirmation or only removes specific subdirectories
/// (cache, logs) but not the state.db.
#[test]
#[ignore = "baseline-red: ox clean does not remove .oxymake dir"]
fn clean_removes_oxymake_dir() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    assert!(
        base.join(".oxymake").exists(),
        ".oxymake should exist after run"
    );

    ox().args(["clean"]).current_dir(base).assert().success();

    assert!(
        !base.join(".oxymake").exists(),
        ".oxymake should be removed after clean"
    );
}

// ===========================================================================
// Scenario 12: `ox status` after successful and failed runs
// ===========================================================================

/// `ox status` after a successful run shows completion info.
#[test]
fn status_after_successful_run() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    ox().current_dir(base)
        .args(["status"])
        .assert()
        .success()
        .stdout(
            predicates::str::contains("1 completed").or(predicates::str::contains("completed")),
        );
}

/// `ox status` after a failed run shows failure info.
#[test]
fn status_after_failed_run() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "exit 1"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    // Run should fail.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .failure();

    // Status should show the failed state.
    ox().current_dir(base)
        .args(["status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("1 failed").or(predicates::str::contains("failed")));
}

// ===========================================================================
// Scenario 13: `ox check-consistency` — invariant checking
// ===========================================================================

/// `ox check-consistency` on a valid workflow should report all invariants hold.
#[test]
fn check_consistency_valid_workflow() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    ox().args(["check-consistency", "-f", oxymakefile.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("All invariants hold"));
}

/// `ox check-consistency --json` emits NDJSON with invariant results.
#[test]
fn check_consistency_json_output() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    let output = ox()
        .args([
            "check-consistency",
            "--json",
            "-f",
            oxymakefile.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Each line should be valid JSON with "invariant" and "status" fields.
    for line in stdout.lines() {
        let v: serde_json::Value = serde_json::from_str(line).expect("each line should be JSON");
        assert!(v.get("invariant").is_some(), "missing 'invariant' field");
        assert!(v.get("status").is_some(), "missing 'status' field");
    }
}

/// `ox check-consistency` detects cyclic dependencies.
#[test]
fn check_consistency_detects_cycle() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.a]
input = ["b.txt"]
output = ["a.txt"]
shell = "echo a"

[rule.b]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo b"
"#,
    )
    .unwrap();

    ox().args(["check-consistency", "-f", oxymakefile.to_str().unwrap()])
        .assert()
        .failure()
        .stdout(predicates::str::contains("FAIL").and(predicates::str::contains("acyclic")));
}

/// `ox check-consistency` warns about shadow outputs (produced but never consumed).
#[test]
fn check_consistency_warns_shadow_outputs() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo a"

[rule.step_b]
output = ["b.txt"]
shell = "echo b"
"#,
    )
    .unwrap();

    // Both rules produce outputs that nothing consumes — warnings expected.
    ox().args(["check-consistency", "-f", oxymakefile.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("WARN").and(predicates::str::contains("shadow")));
}

/// `ox check-consistency` on a missing file should fail with parse error.
#[test]
fn check_consistency_missing_file() {
    ox().args(["check-consistency", "-f", "/nonexistent/Oxymakefile.toml"])
        .assert()
        .failure();
}
