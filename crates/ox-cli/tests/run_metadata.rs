use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use std::fs;

fn ox() -> Command {
    Command::cargo_bin("ox").unwrap()
}

#[test]
fn metadata_is_recorded_once_per_run_and_does_not_invalidate_cache() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        r#"
[rule.all]
input = ["b.txt"]
[rule.a]
output = ["a.txt"]
shell = "printf hello > a.txt"
[rule.b]
input = ["a.txt"]
output = ["b.txt"]
shell = "cat a.txt > b.txt"
"#,
    )
    .unwrap();
    for (index, label) in ["first", "second"].iter().enumerate() {
        let output = ox()
            .current_dir(dir.path())
            .args([
                "run",
                "--json",
                "--report-json",
                "report.ndjson",
                "--metadata",
                &format!("external.id={label}"),
                "--metadata",
                "text=é=a,b",
                "--metadata",
                "empty=",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let events: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let starts: Vec<_> = events
            .iter()
            .filter(|event| event["event"] == "run_started")
            .collect();
        assert_eq!(starts.len(), 1);
        let start = starts[0];
        let labels = json!({"external.id": label, "text": "é=a,b", "empty": ""});
        assert_eq!(start["metadata"], labels);
        assert_eq!(start["schema_version"], 1);
        assert!(
            events
                .iter()
                .filter(|event| event["event"] != "run_started")
                .all(|event| event.get("metadata").is_none())
        );
        if index == 1 {
            assert_eq!(start["to_run"], 0);
            assert_eq!(start["cached"], 2);
            assert!(!events.iter().any(|event| event["event"] == "job_started"));
        }
        let run_id = start["run_id"].as_str().unwrap();
        for path in [
            dir.path().join("report.ndjson"),
            dir.path().join(format!(".oxymake/events/{run_id}.ndjson")),
        ] {
            let log = fs::read_to_string(path).unwrap();
            let first: Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
            assert_eq!(first["metadata"], labels);
        }
    }
    // History must retain labels independently of disposable event logs.
    fs::remove_dir_all(dir.path().join(".oxymake/events")).unwrap();
    let output = ox()
        .current_dir(dir.path())
        .args(["history", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let runs: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(runs.len(), 2);
    for label in ["first", "second"] {
        assert!(
            runs.iter()
                .any(|run| run["metadata"]["external.id"] == label)
        );
    }
}

#[test]
fn metadata_rejects_invalid_arguments_before_loading_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let long_key = format!("{}=v", "a".repeat(65));
    let long_value = format!("key={}", "é".repeat(513));
    for (args, message) in [
        (vec!["oxymake_x=v"], "prefix 'oxymake_' is reserved"),
        (vec!["=v"], "key must match"),
        (vec!["Upper=v"], "key must match"),
        (vec!["a/b=v"], "key must match"),
        (vec!["key"], "must be KEY=VALUE"),
        (vec![long_key.as_str()], "key must match"),
        (vec![long_value.as_str()], "at most 1024 UTF-8 bytes"),
        (vec!["same=1", "same=2"], "duplicate metadata key 'same'"),
    ] {
        let mut command = ox();
        command.current_dir(dir.path()).arg("run");
        for arg in args {
            command.args(["--metadata", arg]);
        }
        command
            .assert()
            .code(2)
            .stderr(predicate::str::contains(message));
        assert!(!dir.path().join(".oxymake").exists());
    }
}

#[test]
fn metadata_accepts_key_and_utf8_byte_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        "[rule.a]\noutput = [\"a\"]\nshell = \"touch a\"\n",
    )
    .unwrap();
    let key = format!("0_.-{}", "a".repeat(60));
    let value = "é".repeat(512);
    ox().current_dir(dir.path())
        .args(["run", "a", "--metadata", &format!("{key}={value}")])
        .assert()
        .success();
}
