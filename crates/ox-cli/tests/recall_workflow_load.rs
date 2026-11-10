use assert_cmd::Command;
use std::{fs, time::Duration};
#[test]
fn remote_recall_loads_the_workflow_once() {
    let root = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("Oxymakefile.toml"),
        "ox_version=\"0.1\"\n[rule.first]\noutput=[\"first\"]\nshell=\"true\"\n",
    )
    .unwrap();
    let out = Command::cargo_bin("ox")
        .unwrap()
        .current_dir(root.path())
        .args(["invalidate", "--rule", "first", "--cache-remote"])
        .arg(remote.path())
        .timeout(Duration::from_secs(30))
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr);
    assert_eq!(stderr.matches("legacy schema 1").count(), 1, "{stderr}");
}
#[test]
fn remote_all_ignores_the_rule_filter_like_local_all() {
    let root = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("Oxymakefile.toml"),
        "ox_version=\"0.1\"\n[rule.first]\noutput=[\"first\"]\nshell=\"true\"\n",
    )
    .unwrap();
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(root.path())
        .args(["invalidate", "--all", "--rule", "nomatch", "--cache-remote"])
        .arg(remote.path())
        .timeout(Duration::from_secs(30))
        .assert()
        .success();
}
