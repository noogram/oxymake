//! Golden-file guard against doc/binary CLI-output drift.
//!
//! The OxyMake book quotes literal `ox` output in its getting-started and
//! reference pages (e.g. `docs/book/src/getting-started/output.md`,
//! `first-workflow.md`, `quickstart.md`, `reference/commands.md`). Those
//! transcripts were once *invented* and diverged from the real binary, so a
//! reader who ran the commands saw a different tool than the docs described.
//!
//! This test pins the load-bearing *format strings* the docs promise. If the
//! CLI output format changes, this test fails — forcing the docs to be updated
//! in the same change instead of silently drifting. It deliberately asserts on
//! stable substrings (not wall-clock timings, which vary run to run).
//!
//! The fixture mirrors the shell-based `quickstart.md` workflow: it has no
//! interpreter dependency, so it runs deterministically on any CI runner.

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use tempfile::TempDir;

/// A Command for the `ox` binary.
fn ox() -> Command {
    Command::cargo_bin("ox").expect("binary should exist")
}

/// Materialize the quickstart-style fixture in a fresh temp dir and return it.
fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(
        root.join("Oxymakefile.toml"),
        r#"ox_version = "0.1"

[config]
samples = ["A", "B"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "sort data/{sample}.csv > results/{sample}.txt"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("data")).unwrap();
    fs::write(root.join("data/A.csv"), "charlie,3\nalpha,1\nbravo,2\n").unwrap();
    fs::write(root.join("data/B.csv"), "zulu,26\nmike,13\n").unwrap();
    dir
}

/// `ox plan` must print the `Plan: N rules, N jobs, N source files` header and
/// the `[job-id] rule=<rule> -> [outputs]` job lines documented in
/// `getting-started/output.md` and `concepts/three-graphs.md`.
#[test]
fn plan_output_matches_docs() {
    let dir = fixture();
    ox().current_dir(dir.path())
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("Plan: 2 rules, 2 jobs, 2 source files"))
        .stdout(contains("rule=process -> [results/"));
}

/// `ox run` must end with the canonical summary line documented in
/// `getting-started/output.md`:
/// `Completed: N succeeded, N failed, N skipped, N cancelled (<elapsed>)`.
#[test]
fn run_summary_line_matches_docs() {
    let dir = fixture();
    ox().current_dir(dir.path())
        .arg("run")
        .assert()
        .success()
        .stdout(contains(
            "Completed: 2 succeeded, 0 failed, 0 skipped, 0 cancelled",
        ));
}

/// A second `ox run` must report every job as cached/skipped, exactly as the
/// caching sections of `first-workflow.md` and `output.md` show.
#[test]
fn run_cached_summary_matches_docs() {
    let dir = fixture();
    ox().current_dir(dir.path()).arg("run").assert().success();
    ox().current_dir(dir.path())
        .arg("run")
        .assert()
        .success()
        .stdout(contains("Cache: 2 of 2 job(s) up-to-date, skipping."))
        .stdout(contains(
            "Completed: 0 succeeded, 0 failed, 2 skipped, 0 cancelled",
        ));
}

/// `ox run --json` must emit the NDJSON event discriminants documented in the
/// "JSON Output (Agent Mode)" section of `output.md`. The doc once showed
/// invented field names (`"type":"run.started"`, `"id":...`); the real schema
/// uses `"event"` and `"job_id"`.
#[test]
fn run_json_events_match_docs() {
    let dir = fixture();
    ox().current_dir(dir.path())
        .args(["run", "--json"])
        .assert()
        .success()
        .stdout(contains(r#""event":"run_started""#))
        .stdout(contains(r#""event":"job_started""#))
        .stdout(contains(r#""event":"job_completed""#))
        .stdout(contains(r#""event":"run_completed""#))
        .stdout(contains(r#""job_id":"process-"#));
}

/// `ox --version` must report `ox 0.1.0` (the binary name, not `oxymake`), as
/// `installation.md` and `quickstart.md` state.
#[test]
fn version_string_matches_docs() {
    ox().arg("--version")
        .assert()
        .success()
        .stdout(contains("ox 0.1.0"));
}

/// Returns true if a `python` interpreter is on PATH (the flagship tutorial
/// uses `lang = "python"` run blocks).
fn python_available() -> bool {
    ["python3", "python"].iter().any(|bin| {
        std::process::Command::new(bin)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Regression guard for the flagship `first-workflow.md` tutorial. It once
/// shipped a Python `run` block written as `stats = {{ ... }}`; OxyMake's
/// interpolation does NOT unescape `{{`/`}}`, so the literal `{{...}}` became a
/// Python set-of-dict and the first `ox run` a newcomer ever issued crashed
/// with `TypeError: unhashable type: 'dict'`. This runs the corrected tutorial
/// end to end and asserts the documented `alice_stats.json` (`"mean": 85.0`) is
/// produced. Skipped when no Python interpreter is available.
#[test]
fn first_workflow_tutorial_runs_green() {
    if !python_available() {
        eprintln!("skipping first_workflow_tutorial_runs_green: no python on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(
        root.join("Oxymakefile.toml"),
        r#"ox_version = "0.1"

[config]
students = ["alice", "bob"]

[rule.stats]
input = ["data/{student}.csv"]
output = ["results/{student}_stats.json"]
lang = "python"
run = """
import csv, json

scores = []
with open("{input}") as f:
    for row in csv.DictReader(f):
        scores.append(int(row["score"]))

stats = {
    "student": "{wildcards.student}",
    "mean": sum(scores) / len(scores),
    "min": min(scores),
    "max": max(scores),
    "count": len(scores),
}

with open("{output}", "w") as f:
    json.dump(stats, f, indent=2)
"""

[rule.summary]
input = ["results/{student}_stats.json"]
output = ["results/summary.json"]
lang = "python"
run = """
import json, glob

all_stats = []
for path in sorted(glob.glob("results/*_stats.json")):
    with open(path) as f:
        all_stats.append(json.load(f))

with open("{output}", "w") as f:
    json.dump(all_stats, f, indent=2)
"""

[rule.all]
input = ["results/summary.json"]
"#,
    )
    .unwrap();
    fs::create_dir(root.join("data")).unwrap();
    fs::write(
        root.join("data/alice.csv"),
        "name,score\nAlice,85\nAlice,92\nAlice,78\n",
    )
    .unwrap();
    fs::write(
        root.join("data/bob.csv"),
        "name,score\nBob,91\nBob,88\nBob,95\n",
    )
    .unwrap();

    ox().current_dir(root)
        .arg("run")
        .assert()
        .success()
        .stdout(contains(
            "Completed: 3 succeeded, 0 failed, 0 skipped, 0 cancelled",
        ));

    let alice = fs::read_to_string(root.join("results/alice_stats.json")).unwrap();
    assert!(
        alice.contains("\"mean\": 85.0"),
        "alice_stats.json should contain the documented mean; got:\n{alice}"
    );
}

/// `ox init` must print the messages quoted in `installation.md`.
#[test]
fn init_output_matches_docs() {
    let dir = TempDir::new().unwrap();
    ox().current_dir(dir.path())
        .arg("init")
        .assert()
        .success()
        .stdout(contains("Initialized OxyMake project in"))
        .stdout(contains("Created: Oxymakefile.toml"));
}
