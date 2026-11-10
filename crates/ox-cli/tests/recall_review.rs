//! Regression cases from the remote-recall review.
use assert_cmd::Command;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const WILDCARDS: &str = r#"ox_version = "0.1"
[config]
samples = ["A", "B", "C"]
[rule.align]
input = ["raw/{sample}.txt"]
output = ["aligned/{sample}.txt"]
shell = "mkdir -p aligned && cat {input} > {output}"
[rule.summarize]
input = ["aligned/{sample}.txt"]
output = ["summ/{sample}.txt"]
shell = "mkdir -p summ && cat {input} > {output}"
"#;
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workflow");
    let remote = tmp.path().join("remote");
    fs::create_dir_all(root.join("raw")).unwrap();
    fs::create_dir(&remote).unwrap();
    fs::write(root.join("Oxymakefile.toml"), WILDCARDS).unwrap();
    for sample in ["A", "B", "C"] {
        fs::write(root.join(format!("raw/{sample}.txt")), sample).unwrap();
    }
    (tmp, root, remote)
}
fn invoke(root: &Path, args: &[&str], remote: Option<&Path>) -> std::process::Output {
    let mut cmd = Command::cargo_bin("ox").unwrap();
    cmd.current_dir(root)
        .args(args)
        .timeout(Duration::from_secs(30));
    if let Some(remote) = remote {
        cmd.arg("--cache-remote").arg(remote);
    }
    cmd.output().unwrap()
}
fn success(root: &Path, args: &[&str], remote: Option<&Path>) -> std::process::Output {
    let out = invoke(root, args, remote);
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn rows(root: &Path) -> i64 {
    rusqlite::Connection::open(root.join(".oxymake/cache/cache.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM cache_entries", [], |r| r.get(0))
        .unwrap()
}
fn entries(remote: &Path) -> Vec<PathBuf> {
    fs::read_dir(remote.join("entries"))
        .unwrap()
        .flat_map(|e| {
            fs::read_dir(e.unwrap().path())
                .unwrap()
                .map(|e| e.unwrap().path())
        })
        .collect()
}
#[test]
fn remote_output_cascade_preserves_local_selection_and_other_wildcard_entries() {
    for use_remote in [false, true] {
        let (_tmp, root, remote) = setup();
        success(
            &root,
            &["run", "summ/A.txt", "summ/B.txt", "summ/C.txt"],
            Some(&remote),
        );
        success(
            &root,
            &["invalidate", "--output", "aligned/A.txt", "--cascade"],
            use_remote.then_some(remote.as_path()),
        );
        assert_eq!(rows(&root), 5);
        assert!(!root.join("aligned/A.txt").exists());
        for p in [
            "aligned/B.txt",
            "aligned/C.txt",
            "summ/A.txt",
            "summ/B.txt",
            "summ/C.txt",
        ] {
            assert!(root.join(p).exists(), "removed {p}");
        }
        assert_eq!(entries(&remote).len(), if use_remote { 5 } else { 6 });
    }
}
#[test]
fn remote_all_does_not_delete_unrecorded_shared_outputs() {
    let (_tmp, root, remote) = setup();
    success(&root, &["run", "aligned/A.txt"], Some(&remote));
    for p in ["aligned/B.txt", "aligned/C.txt"] {
        fs::write(root.join(p), "other checkout").unwrap();
    }
    success(&root, &["invalidate", "--all"], Some(&remote));
    assert_eq!(rows(&root), 0);
    assert!(!root.join("aligned/A.txt").exists());
    for p in ["aligned/B.txt", "aligned/C.txt"] {
        assert_eq!(fs::read_to_string(root.join(p)).unwrap(), "other checkout");
    }
}

#[test]
fn local_empty_cache_preserves_outputs_and_legacy_success_message() {
    for args in [
        vec!["invalidate"],
        vec!["invalidate", "--rule", "align"],
        vec!["invalidate", "--all", "--rule", "nomatch", "--json"],
        vec!["invalidate", "--output", "aligned/A.txt", "--cascade"],
    ] {
        let (_tmp, root, _) = setup();
        fs::create_dir(root.join("aligned")).unwrap();
        fs::write(root.join("aligned/A.txt"), "unrecorded").unwrap();
        let out = success(&root, &args, None);
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "Cache is empty — nothing to invalidate.\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("aligned/A.txt")).unwrap(),
            "unrecorded"
        );
    }
}
#[test]
fn local_all_and_json_keep_the_legacy_output_contract() {
    for dry in [false, true] {
        let (_tmp, root, remote) = setup();
        success(&root, &["run", "aligned/A.txt"], Some(&remote));
        let mut args = vec!["invalidate", "--all", "--rule", "nomatch", "--json"];
        if dry {
            args.push("--dry-run");
        }
        let out = success(&root, &args, None);
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            if dry {
                "Would invalidate all 1 cache entries.\nWould delete 1 output file(s).\n"
            } else {
                "Invalidated all 1 cache entries.\nDeleted 1 output file(s).\n"
            }
        );
        assert_eq!(rows(&root), if dry { 1 } else { 0 });
    }
    let (_tmp, root, remote) = setup();
    success(&root, &["run", "aligned/A.txt"], Some(&remote));
    let out = success(
        &root,
        &["invalidate", "--output", "aligned/A.txt", "--json"],
        None,
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report,
        serde_json::json!({"invalidated_entries":1,"output_paths":["aligned/A.txt"],"deleted_files":1})
    );
}
#[test]
fn local_no_match_no_paths_and_dry_run_messages_are_preserved() {
    let (_tmp, root, remote) = setup();
    success(&root, &["run", "aligned/A.txt"], Some(&remote));
    let missing = invoke(&root, &["invalidate", "--rule", "nomatch"], None);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no rules match 'nomatch'"));
    let out = success(&root, &["invalidate", "--output", "missing"], None);
    assert_eq!(
        out.stdout,
        b"No cache entries matched the specified outputs.\n"
    );
    let out = success(
        &root,
        &[
            "invalidate",
            "--output",
            "aligned/A.txt",
            "--dry-run",
            "--json",
        ],
        None,
    );
    assert_eq!(out.stdout, b"Would invalidate cache entries referencing 1 output(s):\n  aligned/A.txt\nWould delete 1 output file(s).\n");
    fs::write(
        root.join("Oxymakefile.toml"),
        "ox_version = \"0.1\"\n[rule.empty]\nshell = \"true\"\n",
    )
    .unwrap();
    let out = success(&root, &["invalidate", "--rule", "empty"], None);
    assert_eq!(out.stdout, b"No output paths found to invalidate.\n");
}

#[test]
fn remote_preflight_rejects_missing_and_non_directory_paths_before_mutation() {
    for is_file in [false, true] {
        let (_tmp, root, remote) = setup();
        success(&root, &["run", "aligned/A.txt"], Some(&remote));
        let entry = entries(&remote).pop().unwrap();
        let bytes = fs::read(&entry).unwrap();
        let bad = remote.join("invalid-root");
        if is_file {
            fs::write(&bad, "not a directory").unwrap();
        }
        let out = invoke(&root, &["invalidate", "--all"], Some(&bad));
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("existing directory"));
        assert_eq!(rows(&root), 1);
        assert_eq!(fs::read_to_string(root.join("aligned/A.txt")).unwrap(), "A");
        assert_eq!(fs::read(entry).unwrap(), bytes);
    }
    let (_tmp, root, remote) = setup();
    let out = invoke(
        &root,
        &["invalidate", "--rule", "align"],
        Some(&remote.join("absent")),
    );
    assert!(!out.status.success());
    assert!(!root.join(".oxymake").exists());
}
#[test]
fn remote_recall_reports_missing_keys_and_no_entry_matches() {
    let (_tmp, root, remote) = setup();
    // These jobs resolve, but their script bytes are absent, so no key exists.
    fs::write(root.join("Oxymakefile.toml"), "ox_version = \"0.1\"\n[rule.one]\noutput=[\"one\"]\nscript=\"missing.py\"\n[rule.two]\noutput=[\"two\"]\nscript=\"missing.py\"\n").unwrap();
    let out = success(&root, &["invalidate", "--all"], Some(&remote));
    let stderr = String::from_utf8_lossy(&out.stderr);
    for job in ["one", "two"] {
        assert!(
            stderr
                .lines()
                .any(|l| l.contains("cannot calculate current remote key") && l.contains(job)),
            "{stderr}"
        );
    }
    assert!(String::from_utf8_lossy(&out.stdout).contains("No remote entries found"));
    let out = success(&root, &["invalidate", "--all", "--json"], Some(&remote));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["remote_entries_found"], 0);
    // Missing raw inputs with present, unrecorded outputs can resolve as leaves.
    fs::write(root.join("Oxymakefile.toml"), WILDCARDS).unwrap();
    fs::remove_file(root.join("raw/A.txt")).unwrap();
    fs::create_dir(root.join("aligned")).unwrap();
    fs::write(root.join("aligned/A.txt"), "existing").unwrap();
    let out = success(
        &root,
        &["invalidate", "--output", "aligned/A.txt"],
        Some(&remote),
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot calculate current remote key"));
}

#[test]
fn remote_recall_uses_the_workflow_directory_and_honours_adopted_inputs() {
    let (tmp, root, remote) = setup();
    success(&root, &["run", "summ/A.txt"], Some(&remote));
    let db = rusqlite::Connection::open(root.join(".oxymake/cache/cache.db")).unwrap();
    db.execute("UPDATE cache_entries SET adopted=1 WHERE cache_key IN (SELECT cache_key FROM output_records WHERE path='aligned/A.txt')", []).unwrap();
    db.execute("DELETE FROM cache_entries WHERE cache_key IN (SELECT cache_key FROM output_records WHERE path='summ/A.txt')", []).unwrap();
    db.execute("DELETE FROM output_records WHERE path='summ/A.txt'", [])
        .unwrap();
    fs::remove_file(root.join("raw/A.txt")).unwrap();
    let target_entry = entries(&remote)
        .into_iter()
        .find(|p| {
            let entry: serde_json::Value = serde_json::from_slice(&fs::read(p).unwrap()).unwrap();
            entry["outputs"][0]["path"] == "summ/A.txt"
        })
        .unwrap();
    let file = root.join("Oxymakefile.toml");
    let out = success(
        tmp.path(),
        &[
            "invalidate",
            "--output",
            "summ/A.txt",
            "--keep-outputs",
            "-f",
            file.to_str().unwrap(),
        ],
        Some(&remote),
    );
    assert!(
        !target_entry.exists(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(rows(&root), 1, "adopted producer row must remain");
    assert!(
        !tmp.path().join(".oxymake").exists(),
        "opened cache in invocation directory"
    );
}

#[test]
fn non_hex_local_record_warns_without_aborting_other_recalls() {
    let (_tmp, root, remote) = setup();
    success(&root, &["run", "aligned/A.txt"], Some(&remote));
    let db = rusqlite::Connection::open(root.join(".oxymake/cache/cache.db")).unwrap();
    db.execute(
        "INSERT INTO cache_entries (cache_key, completed_at) VALUES ('legacy-key', 0)",
        [],
    )
    .unwrap();
    let out = success(&root, &["invalidate", "--all", "--json"], Some(&remote));
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid stored cache key"));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["invalidated_entries"], 2);
    assert_eq!(rows(&root), 0);
    assert!(entries(&remote).is_empty());
}
