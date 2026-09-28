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

/// Issue #12 item 3: from the second run onwards every job is a cache hit,
/// but `jobs.cached` stayed 0 and `cache_key` stayed NULL — so the table
/// contradicted the console and could not report cache effectiveness.
#[test]
fn second_identical_run_records_the_cache_hit() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = write_two_rule_workflow(&dir);

    run(&dir, &oxymakefile);
    let second = run(&dir, &oxymakefile);
    assert!(
        second.contains("up-to-date"),
        "second run must be a full cache hit, got:\n{second}"
    );

    let conn = open_db(&dir);
    let rows: Vec<(String, String, i64, Option<String>)> = conn
        .prepare("SELECT rule_name, status, cached, cache_key FROM jobs ORDER BY rule_name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(rows.len(), 2, "expected one row per rule, got {rows:?}");
    for (rule, status, cached, cache_key) in &rows {
        assert_eq!(status, "completed", "rule {rule}");
        assert_eq!(*cached, 1, "rule {rule}: cache hit must be recorded");
        assert!(
            cache_key.as_deref().is_some_and(|k| !k.is_empty()),
            "rule {rule}: cache key must be recorded, got {cache_key:?}"
        );
    }
}

/// Issue #12 item 1: `job_history` declared provenance columns and wrote
/// `None` into every one of them, so the trail could not answer "were this
/// job's inputs identical to last time?".
#[test]
fn job_history_records_the_cache_key_components() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = write_two_rule_workflow(&dir);
    run(&dir, &oxymakefile);

    let conn = open_db(&dir);
    let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = conn
        .prepare(
            "SELECT rule_name, input_hashes, output_hashes, artifact_provenance_json
             FROM job_history ORDER BY rule_name",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2, "expected one row per rule, got {rows:?}");

    for (rule, input_hashes, output_hashes, artifact) in &rows {
        let outputs: serde_json::Value =
            serde_json::from_str(output_hashes.as_deref().unwrap_or_default())
                .unwrap_or_else(|_| panic!("rule {rule}: output_hashes must be JSON"));
        assert!(
            outputs.as_object().is_some_and(|m| !m.is_empty()),
            "rule {rule}: output hashes must be recorded, got {output_hashes:?}"
        );
        let artifact = artifact
            .as_deref()
            .unwrap_or_else(|| panic!("rule {rule}: artifact provenance must be recorded"));
        let artifact: serde_json::Value = serde_json::from_str(artifact).unwrap();
        assert!(
            artifact
                .get("job_spec_hash")
                .and_then(|v| v.as_str())
                .is_some(),
            "rule {rule}: artifact provenance must carry the job spec hash"
        );

        // Rule `a` has no inputs; rule `b` consumes a.txt.
        let inputs: serde_json::Value =
            serde_json::from_str(input_hashes.as_deref().unwrap_or_default())
                .unwrap_or_else(|_| panic!("rule {rule}: input_hashes must be JSON"));
        let inputs = inputs.as_array().expect("input hashes are a JSON array");
        if rule == "b" {
            assert_eq!(inputs.len(), 1, "rule b keys on its single input");
            assert_eq!(inputs[0][0].as_str(), Some("a.txt"));
        }
    }
}

/// The same data must be reachable through the reader, not only by opening
/// the SQLite file: `ox history <run> --json` is what a cache report reads.
#[test]
fn history_json_surfaces_the_provenance() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = write_two_rule_workflow(&dir);
    run(&dir, &oxymakefile);

    let run_id: String = open_db(&dir)
        .query_row(
            "SELECT id FROM runs ORDER BY started_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let out = ox()
        .args(["history", "--run-id", &run_id, "--json"])
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();

    let mut seen = 0;
    for line in out.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(
            v["artifact_provenance"].is_string(),
            "history --json must carry artifact_provenance: {line}"
        );
        assert!(
            v["hostname"]
                .as_str()
                .is_some_and(|h| h != "localhost" || h == ox_state::host::hostname()),
            "history --json must carry the resolved hostname: {line}"
        );
        assert!(v["output_hashes"].is_string(), "line: {line}");
        seen += 1;
    }
    assert_eq!(seen, 2, "expected one JSON line per job:\n{out}");
}

/// Issue #24: the history reader preserves measured/unavailable values from
/// StateDb. The actual CLI collection path is covered by slurm_history.rs.
#[test]
fn history_surfaces_measured_and_unavailable_peak_memory() {
    let dir = TempDir::new().unwrap();
    let state_dir = dir.path().join(".oxymake");
    fs::create_dir_all(&state_dir).unwrap();
    let db = ox_state::db::StateDb::open(&state_dir.join("state.db")).unwrap();
    let run_id = "run-memory";
    db.begin_run(run_id, None, 2, None).unwrap();
    db.register_jobs(&[
        ox_state::db::JobRecord {
            id: "measured".into(),
            rule_name: "measured_rule".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: Some(run_id.into()),
        },
        ox_state::db::JobRecord {
            id: "unavailable".into(),
            rule_name: "unavailable_rule".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: Some(run_id.into()),
        },
    ])
    .unwrap();
    let session = db.create_session(1, "slurm-host", None).unwrap();
    for job_id in ["measured", "unavailable"] {
        db.claim_job(job_id, &session).unwrap();
        db.complete_job(job_id, &session, 0, "").unwrap();
    }
    db.finalize_job_history(
        run_id,
        "slurm",
        "slurm-host",
        &Default::default(),
        &std::collections::HashMap::from([("measured".to_string(), 1u64)]),
        &Default::default(),
    )
    .unwrap();
    db.end_run(run_id, 2, 0, 0).unwrap();
    drop(db);

    let json = ox()
        .args(["history", "--run-id", run_id, "--json"])
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: Vec<serde_json::Value> = String::from_utf8(json)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let measured = rows.iter().find(|row| row["job_id"] == "measured").unwrap();
    let unavailable = rows
        .iter()
        .find(|row| row["job_id"] == "unavailable")
        .unwrap();
    assert_eq!(measured["peak_mem_mb"], 1);
    assert!(unavailable["peak_mem_mb"].is_null());

    let text = ox()
        .args(["history", "--run-id", run_id])
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(text).unwrap();
    let measured_fields: Vec<_> = text
        .lines()
        .find(|line| line.starts_with("measured "))
        .unwrap()
        .split_whitespace()
        .collect();
    let unavailable_fields: Vec<_> = text
        .lines()
        .find(|line| line.starts_with("unavailable "))
        .unwrap()
        .split_whitespace()
        .collect();
    assert_eq!(measured_fields[2], "slurm");
    assert_eq!(unavailable_fields[2], "slurm");
    assert_eq!(measured_fields[4], "1");
    assert_eq!(unavailable_fields[4], "-");
}

/// Issue #12 item 4: a run that aborts recorded `0/0/0`, so it was
/// indistinguishable from a run in which nothing happened.
#[test]
fn aborted_run_records_the_counts_it_knows() {
    let dir = TempDir::new().unwrap();
    // `b` fails, and `--keep-going` is off, so the run ends with an error
    // after `a` has succeeded.
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
shell = "exit 3"
"#,
    )
    .unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .failure();

    let conn = open_db(&dir);
    let (succeeded, failed, skipped): (i64, i64, i64) = conn
        .query_row(
            "SELECT succeeded, failed, skipped FROM runs ORDER BY started_at DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!(
        succeeded + failed + skipped > 0,
        "a run with recorded job outcomes must not report 0/0/0 \
         (succeeded={succeeded}, failed={failed}, skipped={skipped})"
    );
    assert_eq!(failed, 1, "the failing job must be counted");
}
