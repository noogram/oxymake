//! Remote computation entries extend a fresh checkout's cache index.
use assert_cmd::Command;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const WORKFLOW: &str = r#"ox_version = "0.1"
[rule.produce]
input = ["input"]
output = ["a", "b"]
shell = "echo ran >> ran.log && cat input > a && echo second > b"
"#;

fn checkout(root: &Path) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("Oxymakefile.toml"), WORKFLOW).unwrap();
    fs::write(root.join("input"), "first\n").unwrap();
}
fn run(root: &Path, remote: &Path, extra: &[&str]) -> std::process::Output {
    let result = Command::cargo_bin("ox")
        .unwrap()
        .current_dir(root)
        .args(["run", "b", "--cache-remote"])
        .arg(remote)
        .args(extra)
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}
fn entry_file(remote: &Path) -> PathBuf {
    let files: Vec<_> = fs::read_dir(remote.join("entries"))
        .unwrap()
        .flat_map(|e| {
            fs::read_dir(e.unwrap().path())
                .unwrap()
                .map(|e| e.unwrap().path())
        })
        .collect();
    assert_eq!(files.len(), 1);
    files[0].clone()
}

#[test]
fn publishes_and_restores_verified_outputs_with_origin_marker() {
    let temp = tempfile::tempdir().unwrap();
    let remote = temp.path().join("remote");
    let a = temp.path().join("first");
    let b = temp.path().join("second");
    checkout(&a);
    checkout(&b);
    run(&a, &remote, &[]);
    let entry: Value = serde_json::from_slice(&fs::read(entry_file(&remote)).unwrap()).unwrap();
    assert_eq!(entry["kind"], "oxymake.remote-cache-entry");
    assert_eq!(entry["outputs"].as_array().unwrap().len(), 2);
    run(&b, &remote, &[]);
    assert!(!b.join("ran.log").exists(), "restored unit executed");
    assert_eq!(fs::read(b.join("a")).unwrap(), b"first\n");
    assert_eq!(fs::read(b.join("b")).unwrap(), b"second\n");
    let db = rusqlite::Connection::open(b.join(".oxymake/cache/cache.db")).unwrap();
    let markers: (i64, i64, String) = db
        .query_row(
            "SELECT restored, adopted, platform FROM cache_entries",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(markers, (1, 0, ox_cache::current_platform()));
}

#[test]
fn malformed_or_incompatible_entries_miss_with_one_warning_per_defect() {
    for defect in [
        "parse",
        "kind",
        "format_version",
        "cache_key",
        "job_spec_hash",
        "input_hashes",
        "platform_scope",
        "origin_platform",
        "paths",
        "duplicate",
        "hash",
        "oversize",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote");
        let a = temp.path().join("writer");
        let b = temp.path().join("reader");
        checkout(&a);
        checkout(&b);
        run(&a, &remote, &[]);
        let path = entry_file(&remote);
        let mut entry: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match defect {
            "kind" => entry["kind"] = "other".into(),
            "format_version" => entry["format_version"] = 99.into(),
            "cache_key" => entry["cache_key"] = "0".repeat(64).into(),
            "job_spec_hash" => entry["provenance"]["job_spec_hash"] = "other".into(),
            "input_hashes" => {
                entry["provenance"]["input_hashes"] =
                    serde_json::json!([["fabricated", "0".repeat(64)]])
            }
            "platform_scope" => entry["platform_scope"] = "any".into(),
            "origin_platform" => entry["origin_platform"] = "other/arch".into(),
            "paths" => entry["outputs"][0]["path"] = "../escape".into(),
            "duplicate" => {
                let out = entry["outputs"][0].clone();
                entry["outputs"].as_array_mut().unwrap().push(out);
            }
            "hash" => entry["outputs"][0]["content_hash"] = "invalid".into(),
            _ => (),
        }
        let bytes = match defect {
            "parse" => b"{".to_vec(),
            "oversize" => vec![b' '; 1024 * 1024 + 1],
            _ => serde_json::to_vec(&entry).unwrap(),
        };
        fs::write(path, bytes).unwrap();
        let result = run(&b, &remote, &[]);
        assert!(b.join("ran.log").exists(), "accepted {defect}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(
            stderr.matches("warning: remote cache entry").count(),
            1,
            "{defect}: {stderr}"
        );
        assert!(!temp.path().join("escape").exists());
    }
}

const CHAIN: &str = r#"ox_version = "0.1"
[rule.first]
input = ["input"]
output = ["first"]
shell = "echo first >> ran.log && cat input > first"
[rule.middle]
input = ["first"]
output = ["middle"]
shell = "echo middle >> ran.log && test -f allow && tr a-z A-Z < first > middle"
[rule.last]
input = ["middle"]
output = ["last"]
shell = "echo last >> ran.log && cat middle > last"
"#;

fn chain_checkout(root: &Path, allow: bool) {
    checkout(root);
    fs::write(root.join("Oxymakefile.toml"), CHAIN).unwrap();
    if allow {
        fs::write(root.join("allow"), "").unwrap();
    }
}
fn chain_run(root: &Path, remote: &Path) -> std::process::Output {
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(root)
        .args(["run", "last", "--jobs", "1", "--cache-remote"])
        .arg(remote)
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap()
}
fn entries(remote: &Path) -> Vec<Value> {
    fs::read_dir(remote.join("entries"))
        .unwrap()
        .flat_map(|e| {
            fs::read_dir(e.unwrap().path())
                .unwrap()
                .map(|e| e.unwrap().path())
        })
        .map(|p| serde_json::from_slice(&fs::read(p).unwrap()).unwrap())
        .collect()
}
fn restored_count(root: &Path) -> i64 {
    rusqlite::Connection::open(root.join(".oxymake/cache/cache.db"))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM cache_entries WHERE restored = 1 AND adopted = 0",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn disjoint_checkouts_continue_failed_run_and_reexecute_tampered_blob() {
    let temp = tempfile::tempdir().unwrap();
    let remote = temp.path().join("remote");
    let a = temp.path().join("checkout-a");
    let b = temp.path().join("elsewhere/checkout-b");
    chain_checkout(&a, false);
    chain_checkout(&b, true);
    let first = chain_run(&a, &remote);
    assert!(!first.status.success(), "middle unit should fail");
    assert_eq!(
        fs::read_to_string(a.join("ran.log")).unwrap(),
        "first\nmiddle\n"
    );
    assert_eq!(
        entries(&remote).len(),
        1,
        "only the recorded unit is published"
    );
    assert!(!a.join("last").exists());
    let second = chain_run(&b, &remote);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        fs::read_to_string(b.join("ran.log")).unwrap(),
        "middle\nlast\n"
    );
    assert_eq!(restored_count(&b), 1);
    assert_eq!(fs::read(b.join("first")).unwrap(), b"first\n");
    assert_eq!(fs::read(b.join("last")).unwrap(), b"FIRST\n");
    let published = entries(&remote);
    assert_eq!(published.len(), 3);
    let first_entry = published
        .iter()
        .find(|e| e["outputs"][0]["path"] == "first")
        .unwrap();
    let hash = first_entry["outputs"][0]["content_hash"].as_str().unwrap();
    fs::write(remote.join(&hash[..2]).join(hash), "tampered").unwrap();
    let c = temp.path().join("checkout-c");
    chain_checkout(&c, true);
    let third = chain_run(&c, &remote);
    assert!(
        third.status.success(),
        "{}",
        String::from_utf8_lossy(&third.stderr)
    );
    assert!(
        fs::read_to_string(c.join("ran.log"))
            .unwrap()
            .lines()
            .any(|unit| unit == "first")
    );
    assert_eq!(fs::read(c.join("first")).unwrap(), b"first\n");
    assert_eq!(fs::read(c.join("last")).unwrap(), b"FIRST\n");
    for entry in entries(&remote) {
        for out in entry["outputs"].as_array().unwrap() {
            let path = c.join(out["path"].as_str().unwrap());
            assert_eq!(
                ox_cache::hash_file(&path).unwrap().as_str(),
                out["content_hash"].as_str().unwrap()
            );
        }
    }
}

#[test]
fn fresh_checkout_restores_absolute_output_under_declared_config_root() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    let outputs = temp.path().join("results");
    let remote = temp.path().join("remote");
    fs::create_dir_all(&outputs).unwrap();
    let workflow = format!(
        r#"ox_version = "0.1"
[config]
results = "{}"
[rule.produce]
input = ["input"]
output = ["{{config.results}}/result"]
shell = "echo ran >> ran.log && cat input > {{output}}"
"#,
        outputs.display()
    );
    for root in [&a, &b] {
        checkout(root);
        fs::write(root.join("Oxymakefile.toml"), &workflow).unwrap();
    }
    let output = outputs.join("result");
    let invoke = |root: &Path| {
        Command::cargo_bin("ox")
            .unwrap()
            .current_dir(root)
            .arg("run")
            .arg(&output)
            .arg("--cache-remote")
            .arg(&remote)
            .timeout(Duration::from_secs(30))
            .assert()
            .success()
    };
    invoke(&a);
    assert_eq!(
        entries(&remote)[0]["outputs"][0]["path"],
        output.to_str().unwrap()
    );
    fs::remove_file(&output).unwrap();
    invoke(&b);
    assert!(!b.join("ran.log").exists());
    assert_eq!(restored_count(&b), 1);
    assert_eq!(fs::read(&output).unwrap(), b"first\n");
}

#[test]
fn dry_run_with_fresh_state_does_not_read_entries_or_fetch_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    let remote = temp.path().join("remote");
    checkout(&a);
    checkout(&b);
    run(&a, &remote, &[]);
    let path = entry_file(&remote);
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, b"invalid").unwrap();
    let dry = run(&b, &remote, &["--dry-run"]);
    assert!(!String::from_utf8_lossy(&dry.stderr).contains("warning: remote cache entry"));
    assert!(!b.join("a").exists());
    assert!(!b.join("b").exists());
    // Resolution may initialize an empty local cache, but must not adopt an entry.
    let db = rusqlite::Connection::open(b.join(".oxymake/cache/cache.db")).unwrap();
    let rows: i64 = db
        .query_row("SELECT count(*) FROM cache_entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    assert!(!b.join("ran.log").exists());
    assert_eq!(fs::read(&path).unwrap(), b"invalid");
    fs::write(path, bytes).unwrap();
    run(&b, &remote, &[]);
    assert!(!b.join("ran.log").exists());
}

#[test]
fn remote_entries_cover_every_locally_cached_reproducibility_class() {
    for class in [
        "deterministic",
        "seed_deterministic",
        "approximate",
        "non_reproducible",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote");
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        for root in [&a, &b] {
            checkout(root);
            fs::write(
                root.join("Oxymakefile.toml"),
                WORKFLOW.replace(
                    "[rule.produce]",
                    &format!("[rule.produce]\nreproducibility = \"{class}\""),
                ),
            )
            .unwrap();
        }
        run(&a, &remote, &[]);
        run(&b, &remote, &[]);
        assert!(!b.join("ran.log").exists(), "{class}");
        assert_eq!(entries(&remote)[0]["provenance"]["reproducibility"], class);
    }
}

#[test]
fn restored_provenance_keeps_local_non_reproducible_export_refusal() {
    let temp = tempfile::tempdir().unwrap();
    let remote = temp.path().join("remote");
    let writer = temp.path().join("writer");
    let reader = temp.path().join("reader");
    checkout(&writer);
    checkout(&reader);
    run(&writer, &remote, &[]);
    fs::write(
        reader.join("Oxymakefile.toml"),
        WORKFLOW.replace(
            "[rule.produce]",
            "[rule.produce]\nreproducibility = \"non_reproducible\"",
        ),
    )
    .unwrap();
    run(&reader, &remote, &[]);
    assert!(!reader.join("ran.log").exists(), "unit should restore");
    assert_eq!(restored_count(&reader), 1);
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(&reader)
        .args(["cache-export", "b"])
        .timeout(Duration::from_secs(30))
        .assert()
        .failure()
        .stderr(predicates::str::contains("refusing non-reproducible"));
}
