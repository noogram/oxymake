//! CLI schema contracts for status snapshots and history records.
use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn output(dir: &TempDir, args: &[&str]) -> Vec<Value> {
    let bytes = Command::cargo_bin("ox")
        .unwrap()
        .current_dir(dir.path())
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::Deserializer::from_slice(&bytes)
        .into_iter::<Value>()
        .map(Result::unwrap)
        .collect()
}

fn seed_run(dir: &TempDir) {
    std::fs::create_dir_all(dir.path().join(".oxymake")).unwrap();
    let db = ox_state::db::StateDb::open(&dir.path().join(".oxymake/state.db")).unwrap();
    db.begin_run("run-1", None, 0, None).unwrap();
}

#[test]
fn status_schema_version_covers_empty_current_selected_and_grouped() {
    let dir = TempDir::new().unwrap();
    let absent = output(&dir, &["status", "--json"]);
    assert_eq!(absent.len(), 1);
    assert_eq!(absent[0]["state"], "absent");
    assert_eq!(absent[0]["schema_version"].as_u64(), Some(1));
    seed_run(&dir);
    for args in [
        vec!["status", "--json"],
        vec!["status", "--json", "--run", "run-1"],
        vec!["status", "--json", "--group-by", "rule"],
        vec!["status", "--json", "--run", "run-1", "--group-by", "stage"],
    ] {
        let records = output(&dir, &args);
        assert_eq!(records.len(), 1, "{args:?}");
        assert_eq!(records[0]["schema_version"].as_u64(), Some(1), "{args:?}");
        assert!(records[0]["jobs"].is_object());
        if args.contains(&"--group-by") {
            assert!(records[0]["groups"].is_array());
        }
    }
}

#[test]
fn history_schema_version_covers_run_and_job_records() {
    let dir = TempDir::new().unwrap();
    seed_run(&dir);
    assert!(output(&dir, &["history", "--json", "--run-id", "run-1"]).is_empty());
    std::fs::write(
        dir.path().join("Oxymakefile.toml"),
        "ox_version = \"0.1\"\n[rule.a]\noutput = [\"a.out\"]\nshell = \"touch a.out\"\n",
    )
    .unwrap();
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(dir.path())
        .args(["run", "a.out"])
        .assert()
        .success();
    let runs = output(&dir, &["history", "--json"]);
    assert_eq!(runs.len(), 2);
    for run in &runs {
        assert_eq!(run["schema_version"].as_u64(), Some(1));
    }
    let run = runs.iter().find(|r| r["run_id"] != "run-1").unwrap();
    let jobs = output(
        &dir,
        &[
            "history",
            "--json",
            "--run-id",
            run["run_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["schema_version"].as_u64(), Some(1));
    assert!(jobs[0]["job_id"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_schema_version_covers_ray_driver_and_cluster_objects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/jobs/driver"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"RUNNING"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/jobs/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "submission_id":"driver", "status":"RUNNING",
            "metadata":{"oxymake_dag_driver":"true", "oxymake_run_id":"run-1"}
        }])))
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    seed_run(&dir);
    let meta_dir = dir.path().join(".oxymake/runs/run-1");
    std::fs::create_dir_all(&meta_dir).unwrap();
    std::fs::write(
        meta_dir.join("meta.json"),
        json!({
            "executor":"ray", "ray_address":server.uri(), "ray_job_id":"driver"
        })
        .to_string(),
    )
    .unwrap();
    for args in [
        vec!["status", "--json"],
        vec!["status", "--json", "--run", "run-1"],
    ] {
        let records = output(&dir, &args);
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r["ray_driver"].is_object()));
        for record in records {
            assert_eq!(record["schema_version"].as_u64(), Some(1));
        }
    }
    std::fs::remove_file(meta_dir.join("meta.json")).unwrap();
    let records = output(&dir, &["status", "--json", "--ray-address", &server.uri()]);
    assert_eq!(records.len(), 2);
    assert!(records.iter().any(|r| r["ray_dag_submissions"].is_array()));
    for record in records {
        assert_eq!(record["schema_version"].as_u64(), Some(1));
    }
}

#[test]
fn history_json_without_state_is_an_empty_stream() {
    let dir = TempDir::new().unwrap();
    assert!(output(&dir, &["history", "--json"]).is_empty());
}
