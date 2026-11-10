//! Recall never opens special files or follows links in the entry tree.
#![cfg(unix)]
use assert_cmd::Command;
use std::{
    fs,
    os::unix::fs::{FileTypeExt, symlink},
    path::PathBuf,
    time::Duration,
};
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workflow");
    let remote = tmp.path().join("remote");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("Oxymakefile.toml"),
        "ox_version=\"0.1\"\n[rule.one]\noutput=[\"out\"]\nshell=\"echo output > out\"\n",
    )
    .unwrap();
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(&root)
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
    (tmp, root, remote, entry)
}
fn recall(root: &std::path::Path, remote: &std::path::Path) -> std::process::Output {
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(root)
        .args(["invalidate", "--all", "--keep-outputs", "--cache-remote"])
        .arg(remote)
        .timeout(Duration::from_secs(3))
        .assert()
        .success()
        .get_output()
        .clone()
}

#[test]
fn absent_entry_tree_prefix_or_file_is_skipped_without_a_warning() {
    for missing in ["tree", "prefix", "file"] {
        let (_tmp, root, remote, entry) = setup();
        fs::remove_file(&entry).unwrap();
        if missing != "file" {
            fs::remove_dir(entry.parent().unwrap()).unwrap();
        }
        if missing == "tree" {
            fs::remove_dir(remote.join("entries")).unwrap();
        }
        let output = recall(&root, &remote);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("warning: skipping remote entry"),
            "{missing}: {stderr}"
        );
    }
}

#[test]
fn inaccessible_entry_parents_are_skipped_with_a_warning() {
    use std::os::unix::fs::PermissionsExt;
    for blocked in ["tree", "prefix"] {
        let (_tmp, root, remote, entry) = setup();
        let directory = if blocked == "tree" {
            remote.join("entries")
        } else {
            entry.parent().unwrap().to_path_buf()
        };
        let probe = if blocked == "tree" {
            entry.parent().unwrap().to_path_buf()
        } else {
            entry.clone()
        };
        let permissions = fs::metadata(&directory).unwrap().permissions();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0)).unwrap();
        let denied = fs::symlink_metadata(probe)
            .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied);
        let output = Command::cargo_bin("ox")
            .unwrap()
            .current_dir(&root)
            .args(["invalidate", "--all", "--keep-outputs", "--cache-remote"])
            .arg(&remote)
            .timeout(Duration::from_secs(30))
            .output()
            .unwrap();
        fs::set_permissions(directory, permissions).unwrap();
        if !denied {
            eprintln!("permission-denial witness unavailable for a privileged user");
            continue;
        }
        assert!(
            output.status.success(),
            "{blocked}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("warning: skipping remote entry"),
            "{blocked}: {stderr}"
        );
        assert!(entry.is_file(), "inaccessible entry must remain untouched");
    }
}
#[test]
fn symlinked_prefix_is_not_followed() {
    let (tmp, root, remote, entry) = setup();
    let prefix = entry.parent().unwrap();
    let outside = tmp.path().join("outside");
    fs::rename(prefix, &outside).unwrap();
    symlink(&outside, prefix).unwrap();
    let original = outside.join(entry.file_name().unwrap());
    let bytes = fs::read(&original).unwrap();
    recall(&root, &remote);
    assert_eq!(fs::read(original).unwrap(), bytes);
    assert!(
        fs::symlink_metadata(prefix)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
#[test]
fn symlinked_entry_is_not_removed() {
    let (tmp, root, remote, entry) = setup();
    let outside = tmp.path().join("outside.json");
    fs::rename(&entry, &outside).unwrap();
    symlink(&outside, &entry).unwrap();
    recall(&root, &remote);
    assert!(
        fs::symlink_metadata(entry)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(outside.exists());
}
#[test]
fn fifo_entry_is_skipped_without_opening_or_blocking() {
    let (_tmp, root, remote, entry) = setup();
    fs::remove_file(&entry).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&entry)
            .status()
            .unwrap()
            .success()
    );
    recall(&root, &remote);
    assert!(fs::symlink_metadata(entry).unwrap().file_type().is_fifo());
}
