//! Regressions for remote restoration and read-only run planning.
use assert_cmd::Command;
use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

fn run(root: &Path, target: &str, remote: Option<&Path>, extra: &[&str]) {
    let mut command = Command::cargo_bin("ox").unwrap();
    command.current_dir(root).args(["run", target]).args(extra);
    if let Some(remote) = remote {
        command.arg("--cache-remote").arg(remote);
    }
    command.timeout(Duration::from_secs(30)).assert().success();
}

#[test]
fn remote_restore_preserves_staging_shaped_inputs_and_siblings() {
    for prefix in ["", "out/"] {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote");
        for checkout in ["writer", "reader"] {
            let root = temp.path().join(checkout);
            let output_dir = root.join(prefix);
            fs::create_dir_all(&output_dir).unwrap();
            fs::write(root.join("Oxymakefile.toml"), format!("[rule.a]\ninput=['{prefix}result.remote.0123456789abcdef']\noutput=['{prefix}result']\nshell='echo ran > ran; cat {{input}} > {{output}}'\n")).unwrap();
            for name in [
                "result.remote.0123456789abcdef",
                "archive.remote.0123456789abcdef",
                "result.remote.0000000100000001",
            ] {
                let path = output_dir.join(name);
                fs::write(&path, "keep me").unwrap();
                fs::File::open(&path)
                    .unwrap()
                    .set_modified(SystemTime::now() - Duration::from_secs(7200))
                    .unwrap();
            }
            run(&root, &format!("{prefix}result"), Some(&remote), &[]);
            assert_eq!(fs::read(output_dir.join("result")).unwrap(), b"keep me");
            for name in [
                "result.remote.0123456789abcdef",
                "archive.remote.0123456789abcdef",
            ] {
                assert_eq!(
                    fs::read(output_dir.join(name)).unwrap(),
                    b"keep me",
                    "{checkout}: {name}"
                );
            }
            if checkout == "reader" {
                assert!(
                    !root.join("ran").exists(),
                    "reader executed instead of restoring"
                );
                assert!(
                    !output_dir.join("result.remote.0000000100000001").exists(),
                    "own old staging survived ({prefix})"
                );
            }
        }
    }
}

#[test]
fn remote_restore_preserves_parameter_script_and_workflow_sources() {
    for source in ["parameter", "script", "workflow"] {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote");
        let name = "result.remote.0123456789abcdef";
        for checkout in ["writer", "reader"] {
            let root = temp.path().join(checkout);
            fs::create_dir(&root).unwrap();
            let workflow_name = if source == "workflow" {
                name
            } else {
                "Oxymakefile.toml"
            };
            let command = "echo ran > ran; echo result > result";
            let execution = match source {
                "parameter" => format!("param_files=['{name}']\nshell='{command}'"),
                "script" => format!("script='{name}'\nlang='python3'"),
                _ => format!("shell='{command}'"),
            };
            fs::write(
                root.join(workflow_name),
                format!("[rule.a]\noutput=['result']\n{execution}\n"),
            )
            .unwrap();
            if source != "workflow" {
                let text = if source == "script" {
                    "from pathlib import Path\nPath('ran').write_text('ran')\nPath('result').write_text('result')\n"
                } else {
                    "parameters"
                };
                fs::write(root.join(name), text).unwrap();
            }
            fs::File::open(root.join(name))
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(7200))
                .unwrap();
            let before = fs::read(root.join(name)).unwrap();
            run(&root, "result", Some(&remote), &["-f", workflow_name]);
            assert_eq!(
                fs::read(root.join(name)).unwrap(),
                before,
                "{source}: {checkout}"
            );
            if checkout == "reader" {
                assert!(
                    !root.join("ran").exists(),
                    "{source} reader did not restore"
                );
            }
        }
    }
}
