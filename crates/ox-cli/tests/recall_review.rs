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
