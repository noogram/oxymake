use assert_cmd::Command;
use serde_json::Value;
use std::{fs, time::Duration};
#[test]
fn remote_dry_run_json_describes_only_eligible_jobs_and_requires_the_flag() {
    let root = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    fs::write(root.path().join("Oxymakefile.toml"), "ox_version=\"0.1\"\n[rule.one]\noutput=[\"one\"]\nshell=\"true\"\n[rule.two]\noutput=[\"two\"]\nshell=\"true\"\n").unwrap();
    for (remote_enabled, extra, expected) in [
        (false, vec![], [false, false]),
        (true, vec![], [true, true]),
        (true, vec!["--forcerun", "/./"], [false, false]),
        (true, vec!["--forcerun", "one"], [false, true]),
        (true, vec!["--no-cache"], [false, false]),
    ] {
        let mut cmd = Command::cargo_bin("ox").unwrap();
        cmd.current_dir(root.path())
            .args(["run", "one", "two", "--dry-run", "--json"])
            .args(extra)
            .timeout(Duration::from_secs(30));
        if remote_enabled {
            cmd.arg("--cache-remote").arg(remote.path());
        }
        let out = cmd.assert().success();
        let events: Vec<Value> = String::from_utf8_lossy(&out.get_output().stdout)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let summary = &events[0];
        if !remote_enabled {
            for event in &events {
                assert!(event.get("remote_entries_checked").is_none());
                assert!(event.get("remote_restore_possible").is_none());
            }
        } else {
            assert_eq!(summary["remote_entries_checked"], false);
            assert_eq!(
                summary["remote_restore_possible"],
                expected.iter().any(|v| *v)
            );
            for (rule, eligible) in ["one", "two"].into_iter().zip(expected) {
                let job = events.iter().find(|e| e["rule"] == rule).unwrap();
                assert_eq!(job["remote_restore_possible"], eligible, "{job}");
            }
        }
    }
}
