//! Recall is addressed by workflow keys and never removes content blobs.
use assert_cmd::Command;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const WORKFLOW: &str = r#"ox_version = "0.1"
[rule.first]
input = ["input"]
output = ["first"]
shell = "echo first >> ran.log && cat input > first"
[rule.second]
input = ["first"]
output = ["second"]
shell = "echo second >> ran.log && cat first > second"
"#;
fn checkout(root: &Path) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("Oxymakefile.toml"), WORKFLOW).unwrap();
    fs::write(root.join("input"), "data\n").unwrap();
}
fn ox(root: &Path, args: &[&str], remote: &Path) -> std::process::Output {
    let out = Command::cargo_bin("ox")
        .unwrap()
        .current_dir(root)
        .args(args)
        .arg("--cache-remote")
        .arg(remote)
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    if root.exists() {
        for e in fs::read_dir(root).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                files.extend(snapshot(&path));
            } else {
                files.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    files
}
fn entry_for(remote: &Path, output: &str) -> PathBuf {
    snapshot(&remote.join("entries"))
        .into_iter()
        .find_map(|(path, bytes)| {
            let e: Value = serde_json::from_slice(&bytes).unwrap();
            e["outputs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["path"] == output)
                .then_some(path)
        })
        .unwrap()
}
fn rows(root: &Path) -> i64 {
    rusqlite::Connection::open(root.join(".oxymake/cache/cache.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM cache_entries", [], |r| r.get(0))
        .unwrap()
}
#[test]
fn recall_from_writer_or_fresh_checkout_reexecutes_only_selected_rule() {
    for fresh in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let remote = tmp.path().join("remote");
        checkout(&a);
        checkout(&b);
        // Independent units: rebuilding a producer can force its consumers
        // under the scheduler's existing content-propagation policy.
        for root in [&a, &b] {
            fs::write(
                root.join("Oxymakefile.toml"),
                WORKFLOW
                    .replace("input = [\"first\"]", "input = [\"input\"]")
                    .replace("cat first > second", "cat input > second"),
            )
            .unwrap();
        }
        ox(&a, &["run", "first", "second"], &remote);
        let recalled = entry_for(&remote, "first");
        let retained = entry_for(&remote, "second");
        let blobs: BTreeMap<_, _> = snapshot(&remote)
            .into_iter()
            .filter(|(p, _)| !p.starts_with(remote.join("entries")))
            .collect();
        ox(
            if fresh { &b } else { &a },
            &["invalidate", "--rule", "first"],
            &remote,
        );
        assert!(!recalled.exists());
        assert!(retained.exists());
        assert_eq!(
            snapshot(&remote)
                .into_iter()
                .filter(|(p, _)| !p.starts_with(remote.join("entries")))
                .collect::<BTreeMap<_, _>>(),
            blobs
        );
        if !fresh {
            assert_eq!(rows(&a), 1);
        }
        ox(&b, &["run", "first", "second"], &remote);
        assert_eq!(fs::read_to_string(b.join("ran.log")).unwrap(), "first\n");
        assert_eq!(fs::read_to_string(b.join("second")).unwrap(), "data\n");
    }
}
#[test]
fn recall_dry_run_json_keeps_rows_outputs_entries_and_blobs() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let remote = tmp.path().join("remote");
    checkout(&a);
    ox(&a, &["run", "second"], &remote);
    let before = snapshot(&remote);
    let outputs = [
        fs::read(a.join("first")).unwrap(),
        fs::read(a.join("second")).unwrap(),
    ];
    let out = ox(&a, &["invalidate", "--all", "--dry-run", "--json"], &remote);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["remote_entries"].as_array().unwrap().len(), 2);
    assert_eq!(report["dry_run"], true);
    assert_eq!(snapshot(&remote), before);
    assert_eq!(rows(&a), 2);
    assert_eq!(
        [
            fs::read(a.join("first")).unwrap(),
            fs::read(a.join("second")).unwrap()
        ],
        outputs
    );
}
#[test]
fn recall_all_from_fresh_state_preserves_other_workflows_and_blobs() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let remote = tmp.path().join("remote");
    checkout(&a);
    checkout(&b);
    ox(&a, &["run", "second"], &remote);
    // Make the downstream key calculable without a local row.
    fs::write(b.join("first"), "data\n").unwrap();
    let foreign = remote
        .join("entries/ff")
        .join(format!("{}.json", "f".repeat(64)));
    fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    fs::write(&foreign, b"foreign workflow").unwrap();
    let before = snapshot(&remote);
    ox(&b, &["invalidate", "--all", "--keep-outputs"], &remote);
    assert!(b.join("first").exists());
    assert_eq!(
        snapshot(&remote.join("entries")),
        BTreeMap::from([(foreign, b"foreign workflow".to_vec())])
    );
    for (path, bytes) in before {
        if !path.starts_with(remote.join("entries")) {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}
#[test]
fn recall_removes_every_historical_key_even_with_missing_raw_inputs() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let remote = tmp.path().join("remote");
    checkout(&a);
    ox(&a, &["run", "first"], &remote);
    fs::write(a.join("input"), "changed input\n").unwrap();
    ox(&a, &["run", "first"], &remote);
    assert_eq!(snapshot(&remote.join("entries")).len(), 2);
    fs::remove_file(a.join("input")).unwrap();
    ox(
        &a,
        &["invalidate", "--output", "first", "--keep-outputs"],
        &remote,
    );
    assert!(snapshot(&remote.join("entries")).is_empty());
    assert_eq!(rows(&a), 0);
    assert!(a.join("first").exists());
}
#[test]
fn recall_reports_current_and_newer_versions_at_the_metadata_size_limit() {
    for (version, newer) in [(1, false), (99, true)] {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let remote = tmp.path().join("remote");
        checkout(&a);
        ox(&a, &["run", "first"], &remote);
        let path = entry_for(&remote, "first");
        let mut document = serde_json::json!({"kind": "oxymake.remote-cache-entry", "format_version": version, "padding": ""});
        let overhead = serde_json::to_vec(&document).unwrap().len();
        document["padding"] = "x".repeat(1024 * 1024 - overhead).into();
        let bytes = serde_json::to_vec(&document).unwrap();
        assert_eq!(bytes.len(), 1024 * 1024);
        fs::write(&path, bytes).unwrap();
        let dry = ox(&a, &["invalidate", "--rule", "first", "--dry-run"], &remote);
        assert_eq!(
            String::from_utf8_lossy(&dry.stdout).contains("newer format version"),
            newer
        );
        assert!(path.exists());
        let out = ox(
            &a,
            &["invalidate", "--rule", "first", "--json", "--keep-outputs"],
            &remote,
        );
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["remote_entries"][0]["format_version"], version);
        assert_eq!(report["remote_entries"][0]["newer_version"], newer);
        assert!(!path.exists());
        assert!(a.join("first").exists());
    }
}
#[test]
fn recall_cascade_applies_only_to_rule_without_local_rows() {
    for select in [["--output", "first"], ["--rule", "/^first$/"]] {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let remote = tmp.path().join("remote");
        checkout(&a);
        checkout(&b);
        ox(&a, &["run", "second"], &remote);
        fs::write(b.join("first"), "data\n").unwrap();
        ox(
            &b,
            &["invalidate", select[0], select[1], "--cascade"],
            &remote,
        );
        assert_eq!(
            snapshot(&remote.join("entries")).len(),
            if select[0] == "--rule" { 0 } else { 1 }
        );
    }
}
#[test]
fn remote_run_dry_run_qualifies_execution_when_outputs_already_present() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let remote = tmp.path().join("remote");
    checkout(&a);
    checkout(&b);
    let outputs = tmp.path().join("shared-results");
    fs::create_dir_all(&outputs).unwrap();
    let workflow = format!(
        r#"ox_version = "0.1"
[config]
results = "{}"
[rule.first]
input = ["input"]
output = ["{{config.results}}/first"]
shell = "echo first >> ran.log && cat input > {{output}}"
[rule.second]
input = ["input"]
output = ["{{config.results}}/second"]
shell = "echo second >> ran.log && cat input > {{output}}"
"#,
        outputs.display()
    );
    for root in [&a, &b] {
        fs::write(root.join("Oxymakefile.toml"), &workflow).unwrap();
    }
    let first = outputs.join("first");
    let second = outputs.join("second");
    let targets = [first.to_str().unwrap(), second.to_str().unwrap()];
    ox(&a, &["run", targets[0], targets[1]], &remote);
    let before = snapshot(&remote);
    let out = ox(&b, &["run", targets[0], targets[1], "--dry-run"], &remote);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stderr.contains("they will be rebuilt"), "{stderr}");
    assert!(stderr.contains("may be restored"), "{stderr}");
    assert!(stdout.contains("remote entries are not read"), "{stdout}");
    assert!(stdout.contains("may be restored"), "{stdout}");
    assert_eq!(snapshot(&remote), before);
    assert!(!b.join("ran.log").exists());
    let out = ox(
        &b,
        &["run", targets[0], targets[1], "--dry-run", "--json"],
        &remote,
    );
    let summary: Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).lines().next().unwrap()).unwrap();
    assert_eq!(summary["remote_entries_checked"], false);
    assert_eq!(summary["remote_restore_possible"], true);
}

#[test]
fn recall_dry_run_from_fresh_state_creates_no_local_state() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let remote = tmp.path().join("remote");
    checkout(&a);
    checkout(&b);
    ox(&a, &["run", "first"], &remote);
    let before = snapshot(&remote);
    let out = ox(
        &b,
        &["invalidate", "--rule", "first", "--dry-run", "--json"],
        &remote,
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["remote_entries"].as_array().unwrap().len(), 1);
    assert!(
        !b.join(".oxymake").exists(),
        "read-only recall initialized local state"
    );
    assert_eq!(snapshot(&remote), before);
}

#[test]
fn recall_unions_recorded_keys_with_a_different_current_key() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let remote = tmp.path().join("remote");
    checkout(&a);
    checkout(&b);
    ox(&a, &["run", "first"], &remote);
    fs::write(b.join("input"), "new data\n").unwrap();
    ox(&b, &["run", "first"], &remote);
    fs::write(a.join("input"), "new data\n").unwrap();
    assert_eq!(snapshot(&remote.join("entries")).len(), 2);
    ox(&a, &["invalidate", "--rule", "first"], &remote);
    assert!(snapshot(&remote.join("entries")).is_empty());
}
