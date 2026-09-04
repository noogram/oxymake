//! End-to-end gate enforcement (GitHub issue #2).
//!
//! A `[gate.<name>]` whose `before` names a rule must hold that rule's job
//! until `ox gate approve <name>`; `ox gate reject <name>` must cancel it.
//! `ox run` blocks while the gate is pending, so the run is driven as a
//! child process and the gate is decided from a second `ox` invocation.

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

const OXYMAKEFILE: &str = r#"ox_version = "0.1"
format_version = "1"

[gate.approval]
after = []
before = ["guarded"]
message = "Should block until approved."

[rule.guarded]
input = []
output = ["out.txt"]
shell = "echo RAN > out.txt"
"#;

const WAIT: Duration = Duration::from_secs(60);

fn ox_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("oxymake")
}

/// Start `ox run out.txt` in `dir`, logging to `run.out` / `run.err`.
fn spawn_run(dir: &Path) -> Child {
    Command::new(ox_bin())
        .args(["run", "out.txt"])
        .current_dir(dir)
        .stdout(Stdio::from(fs::File::create(dir.join("run.out")).unwrap()))
        .stderr(Stdio::from(fs::File::create(dir.join("run.err")).unwrap()))
        .spawn()
        .expect("spawn ox run")
}

/// `ox gate list --json` rows in `dir` (empty until state.db exists).
fn gate_rows(dir: &Path) -> Vec<serde_json::Value> {
    let out = Command::new(ox_bin())
        .args(["gate", "list", "--json"])
        .current_dir(dir)
        .output()
        .expect("run ox gate list");
    if !out.status.success() {
        return Vec::new();
    }
    serde_json::from_slice(&out.stdout).unwrap_or_default()
}

/// Poll until the gate `name` is listed with `status`, or fail after `WAIT`.
fn wait_for_gate(dir: &Path, child: &mut Child, name: &str, status: &str) -> serde_json::Value {
    let start = Instant::now();
    loop {
        if let Some(row) = gate_rows(dir)
            .into_iter()
            .find(|g| g["name"] == name && g["status"] == status)
        {
            return row;
        }
        if let Some(exit) = child.try_wait().unwrap() {
            panic!(
                "ox run exited ({exit}) before gate '{name}' became {status}\n--- stdout\n{}\n--- stderr\n{}",
                fs::read_to_string(dir.join("run.out")).unwrap_or_default(),
                fs::read_to_string(dir.join("run.err")).unwrap_or_default(),
            );
        }
        assert!(
            start.elapsed() < WAIT,
            "gate '{name}' never became {status}; rows: {:?}",
            gate_rows(dir)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Wait for the child to exit, killing it (and failing) after `WAIT`.
fn wait_for_exit(dir: &Path, child: &mut Child) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(exit) = child.try_wait().unwrap() {
            return exit;
        }
        if start.elapsed() > WAIT {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "ox run did not exit after the gate decision\n--- stdout\n{}\n--- stderr\n{}",
                fs::read_to_string(dir.join("run.out")).unwrap_or_default(),
                fs::read_to_string(dir.join("run.err")).unwrap_or_default(),
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Status of the (single) job of rule `rule` in state.db.
fn job_status(dir: &Path, rule: &str) -> Option<String> {
    let db = ox_state::db::StateDb::open(&dir.join(".oxymake/state.db")).unwrap();
    let jobs = db.all_jobs_detail().unwrap();
    let job = jobs.iter().find(|j| j.rule_name == rule)?;
    Some(job.status.clone())
}

/// Guard so a failing assertion never leaves a blocked `ox run` behind.
struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn gated_rule_waits_for_approval_by_name() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("Oxymakefile.toml"), OXYMAKEFILE).unwrap();

    let mut run = KillOnDrop(spawn_run(dir));

    // The gate is registered as pending under its `[gate.<name>]` key …
    let gate = wait_for_gate(dir, &mut run.0, "approval", "pending");
    assert!(
        gate["run_id"].as_str().is_some(),
        "gate is scoped to the run"
    );

    // … and the guarded job has not run.
    assert!(
        !dir.join("out.txt").exists(),
        "guarded rule ran before approval"
    );
    assert_eq!(
        job_status(dir, "guarded").as_deref(),
        Some("pending"),
        "guarded job must stay pending while the gate is pending"
    );
    // The run announces the gate to the operator.
    let stderr = fs::read_to_string(dir.join("run.err")).unwrap_or_default();
    assert!(
        stderr.contains("approval"),
        "GateReached not reported: {stderr}"
    );

    // Approve by NAME (the documented `ox gate approve qc_check` form).
    let approve = Command::new(ox_bin())
        .args(["gate", "approve", "approval", "--approver", "test"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        approve.status.success(),
        "approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );
    assert!(String::from_utf8_lossy(&approve.stdout).contains("'approval'"));

    // The blocked run resumes and completes.
    let exit = wait_for_exit(dir, &mut run.0);
    assert!(
        exit.success(),
        "run failed after approval: {}",
        fs::read_to_string(dir.join("run.err")).unwrap_or_default()
    );
    assert_eq!(
        fs::read_to_string(dir.join("out.txt")).unwrap().trim(),
        "RAN"
    );
    assert_eq!(job_status(dir, "guarded").as_deref(), Some("completed"));

    let gate = gate_rows(dir)
        .into_iter()
        .find(|g| g["name"] == "approval")
        .unwrap();
    assert_eq!(gate["status"], "approved");
    assert_eq!(gate["decided_by"], "test");

    // Approving again is refused: nothing is pending.
    let again = Command::new(ox_bin())
        .args(["gate", "approve", "approval"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("no pending gate named 'approval'"));
}

#[test]
fn rejected_gate_cancels_the_guarded_job() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("Oxymakefile.toml"), OXYMAKEFILE).unwrap();

    let mut run = KillOnDrop(spawn_run(dir));
    let gate = wait_for_gate(dir, &mut run.0, "approval", "pending");
    let id = gate["id"].as_i64().unwrap();

    // Reject by numeric id (the pre-existing form keeps working).
    let reject = Command::new(ox_bin())
        .args(["gate", "reject", &id.to_string(), "--reason", "not ready"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        reject.status.success(),
        "reject failed: {}",
        String::from_utf8_lossy(&reject.stderr)
    );

    // The run terminates without executing the guarded rule.
    let _exit = wait_for_exit(dir, &mut run.0);
    assert!(
        !dir.join("out.txt").exists(),
        "guarded rule ran despite rejection"
    );
    assert_eq!(job_status(dir, "guarded").as_deref(), Some("cancelled"));

    let stderr = fs::read_to_string(dir.join("run.err")).unwrap_or_default();
    let stdout = fs::read_to_string(dir.join("run.out")).unwrap_or_default();
    assert!(
        (stderr.clone() + &stdout).contains("cancelled"),
        "cancellation not reported\n{stdout}\n{stderr}"
    );
}

/// Gates are enforced by the scheduler; the SLURM/Ray DAG submission path
/// bypasses it, so a gated workflow must be refused there rather than run
/// unguarded.
#[test]
fn gated_workflow_is_refused_on_dag_submission_executors() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("Oxymakefile.toml"), OXYMAKEFILE).unwrap();

    for executor in ["slurm", "ray"] {
        let out = Command::new(ox_bin())
            .args(["run", "--executor", executor, "out.txt"])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "{executor}: gated run must be refused"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("gate") && stderr.contains("--executor local"),
            "{executor}: unexpected error: {stderr}"
        );
        assert!(!dir.join("out.txt").exists());
    }
}
