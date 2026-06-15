//! OX-6 invariant: each job reaches at most one terminal status per run.
//!
//! These tests exercise the StateDb operations that drive jobs through
//! the lifecycle (claim → complete / fail / skip / cancel) and assert,
//! after every meaningful operation, that the audit trail does not
//! contain duplicate terminal rows for any `(run_id, job_id)` pair.
//!
//! Implements the defensive CI gate for the exactly-once terminal invariant.

use std::path::PathBuf;

use ox_state::db::{JobHistoryEntry, JobRecord, StateDb};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fresh_db() -> (TempDir, PathBuf, StateDb) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("state.db");
    let db = StateDb::open(&db_path).unwrap();
    (dir, db_path, db)
}

fn make_jobs(ids: &[&str], run_id: Option<&str>) -> Vec<JobRecord> {
    ids.iter()
        .map(|id| JobRecord {
            id: (*id).to_string(),
            rule_name: "build".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: run_id.map(|s| s.to_string()),
        })
        .collect()
}

/// Helper used by every integration scenario — asserts that the
/// canonical OX-6 SQL invariant holds.
fn assert_exactly_once_terminal(db: &StateDb) {
    let violations = db.terminal_status_violations().expect(
        "terminal_status_violations query should succeed against a healthy state.db schema",
    );
    assert!(
        violations.is_empty(),
        "OX-6 violated: job_history contains duplicate terminal rows: {:#?}",
        violations
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn empty_db_satisfies_invariant() {
    let (_dir, _path, db) = fresh_db();
    assert_exactly_once_terminal(&db);
}

#[test]
fn single_run_single_completion_satisfies_invariant() {
    let (_dir, _path, db) = fresh_db();
    let run_id = "run-1";
    db.begin_run(run_id, None, 1, None).unwrap();
    db.register_jobs(&make_jobs(&["j1"], Some(run_id))).unwrap();

    let session = db.create_session(123, "host", None).unwrap();
    assert!(db.claim_job("j1", &session).unwrap());
    assert!(db.complete_job("j1", &session, 0, "{}").unwrap());

    let count = db
        .finalize_job_history(run_id, "local", "host", &Default::default())
        .unwrap();
    assert_eq!(count, 1, "exactly one terminal history row should land");
    assert_exactly_once_terminal(&db);
}

#[test]
fn multiple_jobs_one_per_terminal_status_satisfies_invariant() {
    let (_dir, _path, db) = fresh_db();
    let run_id = "run-2";
    db.begin_run(run_id, None, 3, None).unwrap();
    db.register_jobs(&make_jobs(&["ok", "ko", "skip"], Some(run_id)))
        .unwrap();

    let session = db.create_session(456, "host", None).unwrap();
    db.claim_job("ok", &session).unwrap();
    db.complete_job("ok", &session, 0, "{}").unwrap();
    db.claim_job("ko", &session).unwrap();
    db.fail_job("ko", &session, 1).unwrap();
    db.skip_job("skip").unwrap();

    db.finalize_job_history(run_id, "local", "host", &Default::default())
        .unwrap();
    assert_exactly_once_terminal(&db);
}

#[test]
fn double_recording_is_caught_as_violation() {
    // Defensive test: deliberately corrupt the audit trail to confirm
    // the invariant query actually detects duplicates. If this test
    // ever fails, either `terminal_status_violations` regressed or the
    // schema lost its append-only semantics.
    let (_dir, _path, db) = fresh_db();
    let run_id = "run-3";
    db.begin_run(run_id, None, 1, None).unwrap();
    db.register_jobs(&make_jobs(&["dup"], Some(run_id)))
        .unwrap();

    let entry = JobHistoryEntry {
        run_id: run_id.into(),
        job_id: "dup".into(),
        rule_name: "build".into(),
        wildcards: Some("{}".into()),
        input_hashes: None,
        output_hashes: None,
        params_hash: None,
        env_hash: None,
        executor: Some("local".into()),
        hostname: Some("host".into()),
        started_at: Some(1),
        completed_at: Some(2),
        wall_time_ms: Some(1000),
        peak_mem_mb: None,
        exit_code: Some(0),
        reproducibility_class: None,
        artifact_provenance_json: None,
    };
    db.record_job_history(&entry).unwrap();
    db.record_job_history(&entry).unwrap(); // second record — violates OX-6.

    let violations = db.terminal_status_violations().unwrap();
    assert_eq!(violations.len(), 1, "the deliberate duplicate must surface");
    assert_eq!(violations[0].0, run_id);
    assert_eq!(violations[0].1, "dup");
    assert_eq!(violations[0].2, 2);
}

#[test]
fn cancellation_does_not_violate_invariant() {
    let (_dir, _path, db) = fresh_db();
    let run_id = "run-4";
    db.begin_run(run_id, None, 2, None).unwrap();
    db.register_jobs(&make_jobs(&["a", "b"], Some(run_id)))
        .unwrap();

    let session = db.create_session(789, "host", None).unwrap();
    db.claim_job("a", &session).unwrap();
    db.complete_job("a", &session, 0, "{}").unwrap();
    // Cancel b directly (pending → cancelled, no terminal history row
    // is recorded because finalize_job_history filters by run_id; a
    // cancelled job still gets one row).
    db.cancel_job_ids(&["b".into()]).unwrap();

    db.finalize_job_history(run_id, "local", "host", &Default::default())
        .unwrap();
    assert_exactly_once_terminal(&db);
}
