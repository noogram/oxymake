//! Non-regular entry paths are cache misses, never blocking reads.
#![cfg(unix)]
use assert_cmd::Command;
use std::{fs, os::unix::fs::symlink, path::PathBuf, time::Duration};
fn check_rejected_entry(fifo: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let writer = tmp.path().join("writer");
    let reader = tmp.path().join("reader");
    let remote = tmp.path().join("remote");
    for root in [&writer, &reader] {
        fs::create_dir(root).unwrap();
        fs::write(root.join("Oxymakefile.toml"), "ox_version=\"0.1\"\n[rule.one]\noutput=[\"out\"]\nshell=\"echo ran >> ran.log && echo output > out\"\n").unwrap();
    }
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(&writer)
        .args(["run", "out", "--cache-remote"])
        .arg(&remote)
        .timeout(Duration::from_secs(30))
        .assert()
        .success();
    let prefix = fs::read_dir(remote.join("entries"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let entry = fs::read_dir(prefix)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let outside: PathBuf = tmp.path().join("outside.json");
    let original = fs::read(&entry).unwrap();
    fs::rename(&entry, &outside).unwrap();
    if fifo {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&entry)
                .status()
                .unwrap()
                .success()
        );
    } else {
        symlink(&outside, &entry).unwrap();
    }
    let out = Command::cargo_bin("ox")
        .unwrap()
        .current_dir(&reader)
        .args(["run", "out", "--cache-remote"])
        .arg(&remote)
        .timeout(Duration::from_secs(3))
        .assert()
        .success();
    assert!(
        reader.join("ran.log").exists(),
        "entry was restored instead of treated as a miss"
    );
    let stderr = String::from_utf8_lossy(&out.get_output().stderr);
    assert!(stderr.contains("warning: remote cache entry"), "{stderr}");
    assert_eq!(fs::read(outside).unwrap(), original);
    let restored: i64 = rusqlite::Connection::open(reader.join(".oxymake/cache/cache.db"))
        .unwrap()
        .query_row("SELECT restored FROM cache_entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(restored, 0);
}
#[test]
fn fifo_entry_misses_without_hanging_run() {
    check_rejected_entry(true);
}
#[test]
fn symlinked_entry_misses_with_a_warning() {
    check_rejected_entry(false);
}
