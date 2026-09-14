//! Rebuilt intermediates invalidate consumers only when their bytes change (#19).

use assert_cmd::Command;
use std::{fs, path::Path, time::Duration};
use tempfile::TempDir;

const RECIPE: &str = "cat seed.txt > mid.txt\n";

fn workflow() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("seed.txt"), "seed\n").unwrap();
    fs::write(dir.path().join("mid.sh"), RECIPE).unwrap();
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        r#"ox_version = "0.1"
[rule.mid]
input = ["seed.txt", "mid.sh"]
output = ["mid.txt"]
shell = "sh mid.sh"
[rule.final]
input = ["mid.txt"]
output = ["final.txt"]
shell = "cat mid.txt > final.txt; echo ran >> final-runs.txt"
"#,
    )
    .unwrap();
    dir
}

fn run(base: &Path, args: &[&str], success: bool) -> serde_json::Value {
    let output = Command::cargo_bin("ox")
        .unwrap()
        .current_dir(base)
        .args(["run", "final.txt", "--json"])
        .args(args)
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert_eq!(output.status.success(), success, "{output:?}");
    output
        .stdout
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .find(|event| event["event"] == "run_completed")
        .expect("run_completed")
}

fn counts(event: &serde_json::Value, succeeded: u64, skipped: u64) {
    assert_eq!(event["succeeded"], succeeded, "{event}");
    assert_eq!(event["skipped"], skipped, "{event}");
}

#[test]
fn deleted_intermediate_rebuilt_identically_skips_final() {
    // A, with both serial and parallel dispatch and both content-validating modes.
    for jobs in ["1", "4"] {
        for validation in ["hash", "mtime+hash"] {
            let dir = workflow();
            let base = dir.path();
            let args = ["-j", jobs, "--cache-validation", validation];
            counts(&run(base, &args, true), 2, 0);
            fs::remove_file(base.join("mid.txt")).unwrap();
            counts(&run(base, &args, true), 1, 1);
            assert_eq!(
                fs::read_to_string(base.join("final-runs.txt")).unwrap(),
                "ran\n"
            );
        }
    }
}

#[test]
fn identical_external_rewrite_keeps_entire_chain_cached() {
    // B: replacing the intermediate with identical bytes does not run its producer.
    let dir = workflow();
    let base = dir.path();
    counts(&run(base, &[], true), 2, 0);
    fs::copy(base.join("mid.txt"), base.join("replacement.txt")).unwrap();
    fs::rename(base.join("replacement.txt"), base.join("mid.txt")).unwrap();
    counts(&run(base, &[], true), 0, 2);
}

#[test]
fn failed_then_restored_recipe_skips_unchanged_final() {
    // C: failure cleanup removes mid.txt, but must retain its last successful hash.
    let dir = workflow();
    let base = dir.path();
    counts(&run(base, &[], true), 2, 0);
    fs::write(base.join("mid.sh"), "exit 1\n").unwrap();
    let failed = run(base, &[], false);
    assert_eq!(failed["failed"], 1);
    assert!(!base.join("mid.txt").exists());
    fs::write(base.join("mid.sh"), RECIPE).unwrap();
    counts(&run(base, &[], true), 1, 1);
    assert_eq!(
        fs::read_to_string(base.join("final-runs.txt")).unwrap(),
        "ran\n"
    );
}

#[test]
fn changed_intermediate_reruns_final_in_parallel() {
    let dir = workflow();
    let base = dir.path();
    counts(&run(base, &["-j", "4"], true), 2, 0);
    fs::write(base.join("seed.txt"), "changed\n").unwrap();
    counts(&run(base, &["-j", "4"], true), 2, 0);
    assert_eq!(
        fs::read_to_string(base.join("final.txt")).unwrap(),
        "changed\n"
    );
}

#[test]
fn unknown_comparisons_preserve_conservative_rebuilds() {
    for args in [vec!["--no-cache"], vec!["--cache-validation", "mtime"]] {
        let dir = workflow();
        let base = dir.path();
        counts(&run(base, &args, true), 2, 0);
        fs::remove_file(base.join("mid.txt")).unwrap();
        counts(&run(base, &args, true), 2, 0);
    }
}

#[test]
fn parallel_identical_producers_allow_fan_in_consumer_to_skip() {
    let dir = workflow();
    let base = dir.path();
    let workflow_path = base.join("Oxymakefile.toml");
    let mut spec = fs::read_to_string(&workflow_path).unwrap();
    spec = spec
        .replace(
            "input = [\"mid.txt\"]",
            "input = [\"mid.txt\", \"other.txt\"]",
        )
        .replace(
            "cat mid.txt > final.txt",
            "cat mid.txt other.txt > final.txt",
        );
    spec.push_str(
        "\n[rule.other]\noutput = [\"other.txt\"]\nshell = \"printf other > other.txt\"\n",
    );
    fs::write(workflow_path, spec).unwrap();
    counts(&run(base, &["-j", "4"], true), 3, 0);
    fs::remove_file(base.join("mid.txt")).unwrap();
    fs::remove_file(base.join("other.txt")).unwrap();
    counts(&run(base, &["-j", "4"], true), 2, 1);
    assert_eq!(
        fs::read_to_string(base.join("final-runs.txt")).unwrap(),
        "ran\n"
    );
}

#[test]
fn restored_missing_output_with_same_metadata_is_rehashed() {
    let dir = workflow();
    let base = dir.path();
    fs::write(base.join("seed.txt"), "AAAA\n").unwrap();
    counts(&run(base, &[], true), 2, 0);
    // Exact issue #19 repro, including a separate planning process.
    Command::new("sh")
        .current_dir(base)
        .args([
            "-c",
            "echo BBBB > other.txt; touch -r mid.txt other.txt; rm -f mid.txt",
        ])
        .assert()
        .success();
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(base)
        .args(["plan", "final.txt"])
        .assert()
        .success();
    fs::rename(base.join("other.txt"), base.join("mid.txt")).unwrap();
    counts(&run(base, &[], true), 2, 0);
    assert_eq!(fs::read_to_string(base.join("mid.txt")).unwrap(), "AAAA\n");
    assert_eq!(
        fs::read_to_string(base.join("final.txt")).unwrap(),
        "AAAA\n"
    );
}

#[test]
fn identical_rebuild_reports_consumers_own_run_reason() {
    for cause in ["output_missing", "cache_miss", "output_stale"] {
        let dir = workflow();
        let base = dir.path();
        let spec = base.join("Oxymakefile.toml");
        fs::write(
            &spec,
            fs::read_to_string(&spec).unwrap().replace(
                "input = [\"mid.txt\"]",
                "input = [\"mid.txt\", \"extra.txt\"]",
            ),
        )
        .unwrap();
        fs::write(base.join("extra.txt"), "original").unwrap();
        counts(&run(base, &[], true), 2, 0);
        fs::remove_file(base.join("mid.txt")).unwrap();
        match cause {
            "output_missing" => fs::remove_file(base.join("final.txt")).unwrap(),
            "cache_miss" => fs::write(base.join("extra.txt"), "new input").unwrap(),
            _ => fs::write(base.join("final.txt"), "corrupt output").unwrap(),
        }
        let output = Command::cargo_bin("ox")
            .unwrap()
            .current_dir(base)
            .args(["run", "final.txt", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let started = output
            .stdout
            .split(|b| *b == b'\n')
            .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
            .find(|event| event["event"] == "job_started" && event["job_id"] == "final")
            .expect("consumer started");
        if cause == "cache_miss" {
            assert_eq!(started["reason"], cause, "{started}");
        } else {
            assert!(started["reason"].get(cause).is_some(), "{started}");
        }
    }
}

#[test]
fn changed_producer_key_forces_consumer_even_with_identical_bytes() {
    let dir = workflow();
    let base = dir.path();
    fs::write(base.join("mid.sh"), "printf constant > mid.txt\n").unwrap();
    counts(&run(base, &[], true), 2, 0);
    fs::write(base.join("seed.txt"), "changed dependency\n").unwrap();
    counts(&run(base, &[], true), 2, 0);
    assert_eq!(
        fs::read_to_string(base.join("final-runs.txt")).unwrap(),
        "ran\nran\n"
    );
}

#[test]
fn deferred_consumer_retains_observed_stale_reason() {
    let dir = workflow();
    let base = dir.path();
    fs::write(
        base.join("Oxymakefile.toml"),
        r#"ox_version = "0.1"
[rule.mid]
output = ["mid.txt"]
shell = "echo AAAA > mid.txt"
[rule.a]
priority = 100
input = ["mid.txt"]
output = ["a.txt"]
shell = "sleep 0.2; cat mid.txt > a.txt"
[rule.b]
input = ["mid.txt"]
output = ["b.txt"]
shell = "cat mid.txt > b.txt"
[rule.final]
input = ["a.txt", "b.txt"]
output = ["final.txt"]
shell = "cat a.txt b.txt > final.txt"
"#,
    )
    .unwrap();
    counts(&run(base, &["-j", "1"], true), 4, 0);
    fs::remove_file(base.join("mid.txt")).unwrap();
    fs::remove_file(base.join("a.txt")).unwrap();
    fs::write(base.join("b.txt"), "corrupt output").unwrap();
    let output = Command::cargo_bin("ox")
        .unwrap()
        .current_dir(base)
        .args(["run", "final.txt", "--json", "-j", "1"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let started = output
        .stdout
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .find(|event| event["event"] == "job_started" && event["job_id"] == "b")
        .expect("deferred consumer started");
    assert!(started["reason"].get("output_stale").is_some(), "{started}");
}
