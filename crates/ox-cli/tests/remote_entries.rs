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
