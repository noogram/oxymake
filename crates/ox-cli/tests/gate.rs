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
    // The progress summary counts the rejected job as cancelled, not as
    // skipped (verification finding 7); the final line agrees.
    assert!(
        stderr.contains("1 cancelled"),
        "progress summary must say `1 cancelled`\n{stderr}"
    );
    assert!(
        !stderr.contains("1 skipped"),
        "progress summary must not report the rejected job as skipped\n{stderr}"
    );
    assert!(
        stdout.contains("0 skipped, 1 cancelled"),
        "final summary line\n{stdout}"
    );
    // Gate reject confirmation uses an explicit past-tense verb (finding 6).
    assert!(
        String::from_utf8_lossy(&reject.stdout).contains("rejected by"),
        "reject confirmation: {}",
        String::from_utf8_lossy(&reject.stdout)
    );
}

/// Verification finding 1: when the gate record cannot be written to
/// state.db while reads still work, the guarded rule must not run. The
/// write failure is injected with a `BEFORE INSERT` trigger on `gates`;
/// once the trigger is dropped the run's retried registration succeeds,
/// the gate becomes pending and approval completes the run.
#[test]
fn gate_registration_failure_keeps_the_guarded_job_blocked() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();

    // Initialise a healthy state.db with an ungated run.
    fs::write(
        dir.join("Oxymakefile.toml"),
        r#"ox_version = "0.1"
format_version = "1"

[rule.init]
input = []
output = ["init.txt"]
shell = "echo INIT > init.txt"
"#,
    )
    .unwrap();
    let init = Command::new(ox_bin())
        .args(["run", "init.txt"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );

    let db_path = dir.join(".oxymake/state.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER deny_gate_insert BEFORE INSERT ON gates
             BEGIN SELECT RAISE(ABORT, 'deny gate insert'); END;",
        )
        .unwrap();
    }

    fs::write(dir.join("Oxymakefile.toml"), OXYMAKEFILE).unwrap();
    let mut run = KillOnDrop(spawn_run(dir));

    // Several gate polls (500 ms each) later the run is still blocked: no
    // output, no gate row, job pending, process alive.
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        run.0.try_wait().unwrap().is_none(),
        "ox run exited while gate registration was failing\n--- stdout\n{}\n--- stderr\n{}",
        fs::read_to_string(dir.join("run.out")).unwrap_or_default(),
        fs::read_to_string(dir.join("run.err")).unwrap_or_default(),
    );
    assert!(
        !dir.join("out.txt").exists(),
        "guarded rule ran although its gate could not be registered"
    );
    assert!(
        gate_rows(dir).is_empty(),
        "no gate row can exist: inserts are denied"
    );
    assert_eq!(job_status(dir, "guarded").as_deref(), Some("pending"));
    let stderr = fs::read_to_string(dir.join("run.err")).unwrap_or_default();
    assert!(
        stderr.contains("could not be registered"),
        "the blocked-on-registration condition is reported: {stderr}"
    );

    // Lift the write failure: the retried registration succeeds.
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("DROP TRIGGER deny_gate_insert;")
            .unwrap();
    }
    wait_for_gate(dir, &mut run.0, "approval", "pending");
    assert!(!dir.join("out.txt").exists());

    let approve = Command::new(ox_bin())
        .args(["gate", "approve", "approval", "--approver", "test"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        approve.status.success(),
        "{}",
        String::from_utf8_lossy(&approve.stderr)
    );

    let exit = wait_for_exit(dir, &mut run.0);
    assert!(exit.success());
    assert_eq!(
        fs::read_to_string(dir.join("out.txt")).unwrap().trim(),
        "RAN"
    );
    assert_eq!(job_status(dir, "guarded").as_deref(), Some("completed"));
}

/// A gate whose `before` names a rule that does not exist refuses to run
/// (verification finding 2): with the typo, no job would be attached to the
/// gate and the rule the author meant to guard would run unapproved.
#[test]
fn run_refuses_gate_naming_unknown_rule() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(
        dir.join("Oxymakefile.toml"),
        OXYMAKEFILE.replace("before = [\"guarded\"]", "before = [\"typo_rule\"]"),
    )
    .unwrap();

    let out = Command::new(ox_bin())
        .args(["run", "out.txt"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "run with a misspelled gate rule must be refused"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("gate `approval` lists unknown rule `typo_rule` in `before`"),
        "unexpected error: {stderr}"
    );
    assert!(
        !dir.join("out.txt").exists(),
        "guarded rule ran despite the refusal"
    );
    assert!(gate_rows(dir).is_empty());
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
