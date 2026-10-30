//! Historical status must retain units after the same job IDs enter a new run.
use assert_cmd::Command;
use ox_state::db::{JobRecord, StateDb};
use serde_json::{Value, json};

fn status(project: &std::path::Path, run: Option<&str>) -> Value {
    let mut command = Command::cargo_bin("ox").unwrap();
    command
        .current_dir(project)
        .args(["status", "--json", "--units"]);
    if let Some(run) = run {
        command.args(["--run", run]);
    }
    let output = command.assert().success().get_output().stdout.clone();
    let mut value: Value = serde_json::from_slice(&output).unwrap();
    value["units"]
        .as_array_mut()
        .unwrap()
        .sort_by_key(|unit| unit["job_id"].as_str().unwrap().to_string());
    value
}

fn fixture() -> (tempfile::TempDir, StateDb) {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".oxymake")).unwrap();
    let db = StateDb::open(&project.path().join(".oxymake/state.db")).unwrap();
    db.begin_run("run-1", None, 2, None).unwrap();
    db.register_jobs(&jobs("run-1", &["j", "k"])).unwrap();
    let session = db.create_session(1, "", None).unwrap();
    db.claim_job("j", &session).unwrap();
    db.complete_job("j", &session, 0, "").unwrap();
    db.complete_session(&session).unwrap();
    db.skip_job("j").unwrap();
    db.exclude_job("k").unwrap();
    db.finalize_job_history(
        "run-1",
        "local",
        "",
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    (project, db)
}

fn jobs(run: &str, ids: &[&str]) -> Vec<JobRecord> {
    ids.iter()
        .map(|id| JobRecord {
            id: (*id).into(),
            rule_name: (*id).into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: Some(run.into()),
        })
        .collect()
}

#[test]
fn old_status_survives_partial_and_full_reregistration() {
    for moved in [vec!["j"], vec!["j", "k"]] {
        let (project, db) = fixture();
        let before = status(project.path(), Some("run-1"));
        db.begin_run("run-2", None, moved.len(), None).unwrap();
        db.register_jobs(&jobs("run-2", &moved)).unwrap();
        let after = status(project.path(), Some("run-1"));
        assert_eq!(after, before, "moved {moved:?}");
        assert_eq!(
            after["jobs"]["total"],
            after["units"].as_array().unwrap().len()
        );
        let second = status(project.path(), Some("run-2"));
        assert_eq!(second["jobs"]["total"], moved.len());
        assert_eq!(second["units"].as_array().unwrap().len(), moved.len());
    }
}

#[test]
fn current_status_units_snapshot() {
    let (project, _db) = fixture();
    let mut actual = status(project.path(), None);
    // Execution timestamps are wall-clock observations; normalize only those.
    for unit in actual["units"].as_array_mut().unwrap() {
        unit["ended_at"] = Value::Null;
        unit["started_at"] = Value::Null;
    }
    assert_eq!(
        actual,
        json!({
            "schema_version": 1, "run_id": "run-1", "executor": "local", "sessions": 0,
            "jobs": {"total":2, "completed":1, "cached":1, "skipped":1,
                "running":0, "orphaned":0, "failed":0, "pending":0, "cancelled":0},
            "running_jobs":[], "orphaned_jobs":[], "pending_jobs":[],
            "units":[
                {"job_id":"j", "rule":"j", "wildcards":{}, "outputs":[],
                 "state":"completed", "executor":"local", "endpoint":"", "submission_id":null,
                 "node":null, "started_at":null, "ended_at":null, "exit_code":0, "cache_hit":true},
                {"job_id":"k", "rule":"k", "wildcards":{}, "outputs":[],
                 "state":"skipped", "executor":"local", "endpoint":"", "submission_id":null,
                 "node":null, "started_at":null, "ended_at":null, "exit_code":null, "cache_hit":false}
            ]
        })
    );
}

#[test]
fn legacy_v12_units_expose_unknown_observations() {
    let (project, db) = fixture();
    drop(db);
    let path = project.path().join(".oxymake/state.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    // Reconstruct the v12 schema: these are the only v13 additions.
    conn.execute_batch(
        "DELETE FROM jobs;
        ALTER TABLE job_history DROP COLUMN status;
        ALTER TABLE job_history DROP COLUMN cached;
        ALTER TABLE job_history DROP COLUMN submission_id;
        UPDATE schema_version SET version=12;
        INSERT INTO job_history (run_id, job_id, rule_name, exit_code)
        VALUES ('run-1', 'failed', 'failed', 7);",
    )
    .unwrap();
    drop(conn);
    let status = status(project.path(), Some("run-1"));
    assert_eq!(status["jobs"]["total"], 3);
    let units = status["units"].as_array().unwrap();
    assert_eq!(units.len(), 3);
    for (id, state) in [("j", "completed"), ("k", "unknown"), ("failed", "failed")] {
        let unit = units.iter().find(|unit| unit["job_id"] == id).unwrap();
        assert_eq!(unit["state"], state);
        assert_eq!(unit.get("cache_hit"), Some(&Value::Null));
        assert_eq!(unit.get("submission_id"), Some(&Value::Null));
    }
}

#[test]
fn old_ray_results_do_not_write_into_later_run_jobs() {
    let (project, db) = fixture();
    db.begin_run("run-2", None, 1, None).unwrap();
    db.register_jobs(&jobs("run-2", &["j"])).unwrap();
    let conn = rusqlite::Connection::open(project.path().join(".oxymake/state.db")).unwrap();
    conn.execute(
        "UPDATE jobs SET status='pending', cached=0, exit_code=NULL WHERE run_id='run-2'",
        [],
    )
    .unwrap();
    let before = status(project.path(), Some("run-2"));
    let run = project.path().join(".oxymake/runs/run-1");
    std::fs::create_dir_all(run.join("tasks")).unwrap();
    // No remote submission or address: exercise disk reconciliation only.
    std::fs::write(run.join("meta.json"), r#"{"executor":"ray"}"#).unwrap();
    std::fs::write(
        run.join("results.json"),
        r#"{"j":{"status":"failed","exit_code":7}}"#,
    )
    .unwrap();
    std::fs::write(
        run.join("tasks/j.json"),
        serde_json::to_vec(&json!({
            "job_id":"j", "status":"failed", "exit_code":7,
            "started_at":1.0, "ended_at":2.0, "wall_time_ms":1000
        }))
        .unwrap(),
    )
    .unwrap();
    let old = status(project.path(), Some("run-1"));
    assert_eq!(old["jobs"]["total"], 2);
    assert_eq!(status(project.path(), Some("run-2")), before);
    assert_eq!(db.job_ids_for_run("run-1").unwrap(), ["k"]);
}
