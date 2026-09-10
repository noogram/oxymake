//! Regression tests for issue #12 — the state DB must be able to
//! substantiate a cache decision after the fact.
//!
//! Each test drives the real `ox run` binary over a two-rule workflow and
//! then reads `.oxymake/state.db` directly, because the defect is in what
//! is *recorded*, not in what is printed.

use std::fs;

use assert_cmd::Command;
use tempfile::TempDir;

fn ox() -> Command {
    Command::cargo_bin("oxymake").expect("binary should exist")
}

/// A two-rule chain: `a` produces `a.txt`, `b` consumes it and produces
/// `b.txt`.  Both are cacheable, so a second identical run is a full
/// cache hit.
fn write_two_rule_workflow(dir: &TempDir) -> std::path::PathBuf {
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["b.txt"]

[rule.a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.b]
input = ["a.txt"]
output = ["b.txt"]
shell = "cat a.txt > b.txt"
"#,
    )
    .unwrap();
    oxymakefile
}

fn run(dir: &TempDir, oxymakefile: &std::path::Path) -> String {
    let out = ox()
        .args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).unwrap()
}

fn open_db(dir: &TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.path().join(".oxymake/state.db")).unwrap()
}

/// Issue #12 item 2: `hostname` was the literal `"localhost"` in both
/// `job_history` and `sessions`, so the trail could not say which machine
/// a job ran on.
#[test]
fn audit_trail_records_the_real_hostname() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = write_two_rule_workflow(&dir);
    run(&dir, &oxymakefile);

    let expected = ox_state::host::hostname();
    let conn = open_db(&dir);

    let hosts: Vec<String> = conn
        .prepare("SELECT DISTINCT hostname FROM job_history")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        hosts,
        vec![expected.to_string()],
        "job_history must record the resolved host name"
    );

    let session_hosts: Vec<String> = conn
        .prepare("SELECT DISTINCT hostname FROM sessions")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        session_hosts,
        vec![expected.to_string()],
        "sessions must record the resolved host name"
    );
}
