//! End-to-end gate enforcement (GitHub issue #2) and the cooperative claim
//! as the scheduling gate (GitHub issue #3).
//!
//! A `[gate.<name>]` whose `before` names a rule must hold that rule's job
//! until `ox gate approve <name>`; `ox gate reject <name>` must cancel it.
//! `ox run` blocks while the gate is pending, so the run is driven as a
//! child process and the gate is decided from a second `ox` invocation.
//!
//! Two approved sessions reaching the same job are the systematic way to
//! exercise the claim protocol (ADR-012): the session that loses the claim
//! must wait for its peer's result instead of executing — or failing — the
//! job, and reclaim it only once the peer's lease has expired.

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

/// A two-output rule guarded by a gate. Each execution writes its own shell
/// PID into both outputs; the write order is flipped by PID parity so that
/// two *concurrent* executions would interleave and leave a mixed set
/// (`a.txt != b.txt`). A `go` barrier file holds the shell so both sessions
/// are provably at the job before either may finish it, and every entry
/// into the shell is logged so executions can be counted.
const TWO_OUTPUT_OXYMAKEFILE: &str = r#"ox_version = "0.1"
format_version = "1"

[gate.approval]
after = []
before = ["guarded"]
message = "Should block until approved."

[rule.guarded]
input = []
output = ["a.txt", "b.txt"]
shell = "echo $$ >> executions.log; while [ ! -f go ]; do sleep 0.01; done; id=$$; if [ $((id % 2)) -eq 0 ]; then printf '%s\n' \"$id\" > a.txt; sleep 0.2; printf '%s\n' \"$id\" > b.txt; else printf '%s\n' \"$id\" > b.txt; sleep 0.2; printf '%s\n' \"$id\" > a.txt; fi"
"#;

/// What `ox run` prints while parking a job behind a peer session.
const WAITING_MSG: &str = "waiting for its result instead of running it here";
/// The fail-closed error of the output-path locks (#2), now defence in depth.
const CONCURRENT_MSG: &str = "already being executed by another session";

/// Start `ox run a.txt b.txt` in `dir`, logging to `run-<tag>.{out,err}`,
/// with the session lease set to `lease_secs` when given.
fn spawn_two_output_run(dir: &Path, tag: &str, lease_secs: Option<u64>) -> Child {
    let mut cmd = Command::new(ox_bin());
    cmd.args(["run", "a.txt", "b.txt"])
        .current_dir(dir)
        .stdout(Stdio::from(
            fs::File::create(dir.join(format!("run-{tag}.out"))).unwrap(),
        ))
        .stderr(Stdio::from(
            fs::File::create(dir.join(format!("run-{tag}.err"))).unwrap(),
        ));
    if let Some(secs) = lease_secs {
        cmd.env("OX_SESSION_LEASE_SECS", secs.to_string());
    }
    cmd.spawn().expect("spawn ox run")
}

/// `(stdout, stderr)` of the run tagged `tag`.
fn read_run(dir: &Path, tag: &str) -> (String, String) {
    (
        fs::read_to_string(dir.join(format!("run-{tag}.out"))).unwrap_or_default(),
        fs::read_to_string(dir.join(format!("run-{tag}.err"))).unwrap_or_default(),
    )
}

/// Poll until `run-<tag>.err` contains `needle`, failing after `WAIT` or if
/// the child exits first.
fn wait_for_stderr(dir: &Path, child: &mut Child, tag: &str, needle: &str) {
    let start = Instant::now();
    loop {
        if read_run(dir, tag).1.contains(needle) {
            return;
        }
        if let Some(exit) = child.try_wait().unwrap() {
            let (out, err) = read_run(dir, tag);
            panic!(
                "run {tag} exited ({exit}) before printing {needle:?}\n--- stdout\n{out}\n--- stderr\n{err}"
            );
        }
        assert!(
            start.elapsed() < WAIT,
            "run {tag} never printed {needle:?}:\n{}",
            read_run(dir, tag).1
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Approve every pending `approval` gate once `expected` of them exist.
fn approve_pending_gates(dir: &Path, children: &mut [(&str, &mut Child)], expected: usize) {
    let start = Instant::now();
    let ids: Vec<i64> = loop {
        let pending: Vec<i64> = gate_rows(dir)
            .into_iter()
            .filter(|g| g["name"] == "approval" && g["status"] == "pending")
            .filter_map(|g| g["id"].as_i64())
            .collect();
        if pending.len() == expected {
            break pending;
        }
        for (tag, child) in children.iter_mut() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "run {tag} exited before its gate became pending:\n{}",
                read_run(dir, tag).1
            );
        }
        assert!(
            start.elapsed() < WAIT,
            "{expected} pending gates never appeared"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    for id in &ids {
        let out = Command::new(ox_bin())
            .args(["gate", "approve", &id.to_string(), "--approver", "test"])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Issue #3, acceptance 1, on the #2 two-output reproducer: two `ox run`
/// approved for the same gated job. The claim protocol is the scheduling
/// gate, so the session that loses the claim never launches the job: it
/// waits for the owner's result and consumes it. Exactly one physical
/// execution happens, both sessions exit 0 and report the job done, no
/// session hits the output-path locks (`ConcurrentExecution`), and the
/// committed set is one execution's (`a.txt == b.txt`).
///
/// Without the dispatch-time claim both sessions execute, the loser fails
/// closed on the locks with exit status 1 and this test fails on the
/// exit-status and `CONCURRENT_MSG` assertions.
#[test]
fn two_approved_runs_share_one_execution_and_commit_one_set() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("Oxymakefile.toml"), TWO_OUTPUT_OXYMAKEFILE).unwrap();

    let mut a = KillOnDrop(spawn_two_output_run(dir, "a", None));
    let mut b = KillOnDrop(spawn_two_output_run(dir, "b", None));
    approve_pending_gates(dir, &mut [("a", &mut a.0), ("b", &mut b.0)], 2);

    // After approval exactly one session owns the claim and blocks on the
    // `go` barrier inside the job; the other reports that it is waiting.
    let start = Instant::now();
    let loser_tag = loop {
        let a_waits = read_run(dir, "a").1.contains(WAITING_MSG);
        let b_waits = read_run(dir, "b").1.contains(WAITING_MSG);
        assert!(!(a_waits && b_waits), "both sessions lost the claim");
        if a_waits {
            break "a";
        }
        if b_waits {
            break "b";
        }
        for (tag, child) in [("a", &mut a.0), ("b", &mut b.0)] {
            assert!(
                child.try_wait().unwrap().is_none(),
                "run {tag} exited before either session waited on the other:\n{}",
                read_run(dir, tag).1
            );
        }
        assert!(
            start.elapsed() < WAIT,
            "neither session waited for the other:\n--- a\n{}\n--- b\n{}",
            read_run(dir, "a").1,
            read_run(dir, "b").1
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let winner_tag = if loser_tag == "a" { "b" } else { "a" };

    // The job shell has been entered exactly once before the barrier.
    std::thread::sleep(Duration::from_millis(500));
    let executions = fs::read_to_string(dir.join("executions.log")).unwrap_or_default();
    assert_eq!(
        executions.lines().count(),
        1,
        "job shell entered {} times before release:\n{executions}",
        executions.lines().count()
    );

    // Release the winner's shell; both sessions must then finish.
    fs::write(dir.join("go"), "").unwrap();
    let a_exit = wait_for_exit(dir, &mut a.0);
    let b_exit = wait_for_exit(dir, &mut b.0);

    for (tag, exit) in [("a", a_exit), ("b", b_exit)] {
        let (out, err) = read_run(dir, tag);
        assert!(exit.success(), "run {tag} failed:\n{out}\n{err}");
        assert!(
            out.contains("1 succeeded"),
            "run {tag} did not report the job done:\n{out}"
        );
        assert!(
            !err.contains(CONCURRENT_MSG),
            "run {tag} hit the output locks; the claim did not gate dispatch:\n{err}"
        );
    }
    assert!(read_run(dir, loser_tag).1.contains(WAITING_MSG));
    assert!(!read_run(dir, winner_tag).1.contains(WAITING_MSG));

    // One physical execution, one coherent committed set — that execution's.
    let executions = fs::read_to_string(dir.join("executions.log")).unwrap();
    assert_eq!(executions.lines().count(), 1, "{executions}");
    let a_txt = fs::read_to_string(dir.join("a.txt")).unwrap();
    let b_txt = fs::read_to_string(dir.join("b.txt")).unwrap();
    assert_eq!(
        a_txt.trim(),
        b_txt.trim(),
        "committed set is mixed: a.txt={a_txt:?} b.txt={b_txt:?}"
    );
    assert_eq!(a_txt.trim(), executions.trim());
    assert!(!dir.join("a.txt.oxytmp").exists());
    assert!(!dir.join("b.txt.oxytmp").exists());
    assert_eq!(job_status(dir, "guarded").as_deref(), Some("completed"));
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

/// A gated two-output rule whose first execution records its shell PID in
/// `mid.pid` and then sleeps before writing `b.txt`; once `second` exists,
/// executions write the whole set quickly instead. Used to leave a job
/// shell orphaned by killing its `ox` session mid-job.
const ORPHAN_OXYMAKEFILE: &str = r#"ox_version = "0.1"
format_version = "1"

[gate.approval]
after = []
before = ["guarded"]

[rule.guarded]
input = []
output = ["a.txt", "b.txt"]
shell = "id=$$; if [ ! -f second ]; then printf 'killed-%s\n' \"$id\" > a.txt; printf '%s\n' \"$id\" > mid.pid; sleep 4; printf 'killed-%s\n' \"$id\" > b.txt; else printf 'replacement-%s\n' \"$id\" > a.txt; sleep 0.2; printf 'replacement-%s\n' \"$id\" > b.txt; fi"
"#;

/// Whether a process with `pid` still exists (`kill -0`).
fn process_alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Start `ox run a.txt b.txt` in `dir` under `lease_secs`, logging to
/// `run-<tag>.{out,err}`, and approve its gate once it is pending.
fn spawn_two_output_run_and_approve(dir: &Path, tag: &str, lease_secs: Option<u64>) -> KillOnDrop {
    let mut run = KillOnDrop(spawn_two_output_run(dir, tag, lease_secs));
    let gate = wait_for_gate(dir, &mut run.0, "approval", "pending");
    let id = gate["id"].as_i64().unwrap();
    let out = Command::new(ox_bin())
        .args(["gate", "approve", &id.to_string(), "--approver", "test"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "approve {tag}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    run
}

/// Start an approved run of `ORPHAN_OXYMAKEFILE` under `lease_secs`, wait
/// until its job shell is mid-job, `SIGKILL` the session and return the
/// orphaned shell's pid. Also drops the `second` marker so later runs take
/// the "replacement" branch of the rule.
fn kill_session_mid_job(dir: &Path, lease_secs: u64) -> String {
    let mut first = spawn_two_output_run_and_approve(dir, "killed", Some(lease_secs));
    let start = Instant::now();
    while !dir.join("mid.pid").exists() {
        assert!(
            first.0.try_wait().unwrap().is_none(),
            "first run exited before its job was mid-way:\n{}",
            read_run(dir, "killed").1
        );
        assert!(start.elapsed() < WAIT, "job never reached mid-job");
        std::thread::sleep(Duration::from_millis(50));
    }
    let shell_pid = fs::read_to_string(dir.join("mid.pid"))
        .unwrap()
        .trim()
        .to_string();
    first.0.kill().unwrap(); // SIGKILL on Unix
    let killed = first.0.wait().unwrap();
    assert!(!killed.success());
    assert!(
        process_alive(&shell_pid),
        "the job shell must have been orphaned by the kill for this test to mean anything"
    );
    fs::write(dir.join("second"), "").unwrap();
    shell_pid
}

/// `(id, status)` of the sessions recorded in state.db, oldest first.
fn sessions(dir: &Path) -> Vec<(String, String)> {
    ox_state::db::StateDb::open(&dir.join(".oxymake/state.db"))
        .unwrap()
        .session_statuses()
        .unwrap()
}

/// Issue #3, acceptance 2: the owner is `kill -9`ed mid-job. Its heartbeat
/// stops, so a replacement session first loses the claim and waits, then —
/// once the lease (8 s here) has expired — reclaims the job and executes it
/// itself. The orphaned job shell has exited by then (it sleeps 4 s), so the
/// output locks are free and the replacement commits an intact set.
///
/// Without the dispatch-time claim the replacement launches the job at
/// once, runs into the orphan's inherited locks and fails closed: this test
/// then fails on `WAITING_MSG` and on the exit status.
#[test]
fn killed_owner_is_reclaimed_after_lease_expiry_and_the_waiter_executes() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("Oxymakefile.toml"), ORPHAN_OXYMAKEFILE).unwrap();
    let lease = 8;

    let shell_pid = kill_session_mid_job(dir, lease);
    let killed_at = Instant::now();

    let mut waiter = spawn_two_output_run_and_approve(dir, "waiter", Some(lease));
    // The dead owner still reads as live until its lease expires: the
    // replacement waits rather than run (or fail) the job.
    wait_for_stderr(dir, &mut waiter.0, "waiter", WAITING_MSG);
    assert!(
        process_alive(&shell_pid),
        "the waiter must have parked while the orphan was still running"
    );

    let exit = wait_for_exit(dir, &mut waiter.0);
    let (out, err) = read_run(dir, "waiter");
    assert!(exit.success(), "{out}\n{err}");
    assert!(out.contains("1 succeeded"), "{out}");
    assert!(!err.contains(CONCURRENT_MSG), "{err}");
    assert!(
        killed_at.elapsed() >= Duration::from_secs(4),
        "the waiter finished before the orphan could have: the lease did not gate the reclaim"
    );
    assert!(!process_alive(&shell_pid));

    // The committed set is the waiter's, intact.
    let a = fs::read_to_string(dir.join("a.txt")).unwrap();
    let b = fs::read_to_string(dir.join("b.txt")).unwrap();
    assert!(a.starts_with("replacement-"), "a.txt={a:?}");
    assert_eq!(
        a.trim(),
        b.trim(),
        "committed set is mixed: a={a:?} b={b:?}"
    );
    assert!(!dir.join("a.txt.oxytmp").exists());
    assert!(!dir.join("b.txt.oxytmp").exists());

    // The ledger tells the story: the killed session was reclaimed
    // (`interrupted`), the waiter completed the job and closed its session.
    let sessions = sessions(dir);
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert_eq!(sessions[0].1, "interrupted", "{sessions:?}");
    assert_eq!(sessions[1].1, "completed", "{sessions:?}");
    assert_eq!(job_status(dir, "guarded").as_deref(), Some("completed"));
}

/// Round-3 finding 1 (#2), kept as defence in depth: with a lease shorter
/// than the orphaned job shell's life, the replacement reclaims while the
/// orphan is still alive and holding the inherited output locks, and must
/// fail closed naming the exited session — never commit next to the
/// orphan's writes. Once the orphan is gone, a later run re-evaluates the
/// job (the failed verdict belongs to a session that has exited) and
/// commits an intact set.
#[test]
fn reclaim_while_the_orphan_still_holds_the_locks_fails_closed() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("Oxymakefile.toml"), ORPHAN_OXYMAKEFILE).unwrap();

    let shell_pid = kill_session_mid_job(dir, 2);

    let mut early = spawn_two_output_run_and_approve(dir, "early", Some(2));
    let early_exit = wait_for_exit(dir, &mut early.0);
    let (early_out, early_err) = read_run(dir, "early");
    assert!(
        !early_exit.success(),
        "replacement succeeded while the orphaned job still held the locks:\n{early_out}\n{early_err}"
    );
    assert!(
        early_err.contains(CONCURRENT_MSG) && early_err.contains("which has exited"),
        "replacement must fail closed naming the dead session:\n{early_err}"
    );
    assert!(!early_out.contains("1 succeeded"), "{early_out}");
    assert!(
        fs::read_to_string(dir.join("a.txt"))
            .unwrap_or_default()
            .starts_with("killed-"),
        "the replacement must not have touched the orphan's paths"
    );

    // Wait past the orphan's delayed write (it exits right after it).
    let start = Instant::now();
    while process_alive(&shell_pid) {
        assert!(start.elapsed() < WAIT, "orphaned job shell never exited");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        fs::read_to_string(dir.join("b.txt")).unwrap().trim(),
        format!("killed-{shell_pid}"),
        "the orphan's delayed write landed (uncommitted by anyone)"
    );

    let mut late = spawn_two_output_run_and_approve(dir, "late", Some(2));
    let late_exit = wait_for_exit(dir, &mut late.0);
    let (late_out, late_err) = read_run(dir, "late");
    assert!(late_exit.success(), "{late_out}\n{late_err}");
    assert!(late_out.contains("1 succeeded"), "{late_out}");
    let a = fs::read_to_string(dir.join("a.txt")).unwrap();
    let b = fs::read_to_string(dir.join("b.txt")).unwrap();
    assert!(a.starts_with("replacement-"), "a.txt={a:?}");
    assert_eq!(
        a.trim(),
        b.trim(),
        "committed set is mixed: a={a:?} b={b:?}"
    );
    assert!(!dir.join("a.txt.oxytmp").exists());
    assert!(!dir.join("b.txt.oxytmp").exists());
}
