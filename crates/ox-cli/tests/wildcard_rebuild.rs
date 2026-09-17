//! Existing wildcard outputs must not cut off missing intermediates (#22).

use assert_cmd::Command;
use serde_json::Value;
use std::{fs, path::Path, time::Duration};
use tempfile::TempDir;

const WORKFLOW: &str = r#"ox_version = "0.1"
[rule.mid]
input = ["in/{sample}.txt"]
output = ["mid/{sample}.txt"]
shell = "mkdir -p mid && cat {input} > {output}"
[rule.final]
input = ["mid/{sample}.txt"]
output = ["final/{sample}.txt"]
shell = "mkdir -p final && tr a-z A-Z < {input} > {output}"
"#;

fn fixture(spec: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("in")).unwrap();
    fs::write(dir.path().join("in/s1.txt"), "a\n").unwrap();
    fs::write(dir.path().join("in/s2.txt"), "b\n").unwrap();
    fs::write(dir.path().join("Oxymakefile.toml"), spec).unwrap();
    dir
}

fn ox(base: &Path, args: &[&str]) -> String {
    let output = Command::cargo_bin("ox")
        .unwrap()
        .current_dir(base)
        .env("OX_CACHE_VALIDATION", "hash")
        .args(args)
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn plan(base: &Path, targets: &[&str]) -> Value {
    let mut args = vec!["plan", "--json"];
    args.extend_from_slice(targets);
    serde_json::from_str(&ox(base, &args)).unwrap()
}

fn run(base: &Path, targets: &[&str]) -> Vec<Value> {
    let mut args = vec!["run", "--json"];
    args.extend_from_slice(targets);
    ox(base, &args)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn succeeded(events: &[Value], expected: u64) {
    let completed = events
        .iter()
        .find(|e| e["event"] == "run_completed")
        .unwrap();
    assert_eq!(completed["succeeded"], expected, "{completed}");
}

fn assert_rebuild(base: &Path, targets: &[&str], planned_outputs: &[&str]) {
    let planned = plan(base, targets);
    let jobs = planned["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), planned_outputs.len(), "{planned}");
    for output in planned_outputs {
        assert!(
            jobs.iter().any(|job| job["outputs"]
                .as_array()
                .unwrap()
                .contains(&Value::from(*output))),
            "missing {output}: {planned}"
        );
    }
    let events = run(base, targets);
    succeeded(&events, 1);
    // Plan is an upper bound: byte-identical reconstruction can skip consumers (#19).
    for event in events.iter().filter(|e| e["event"] == "job_started") {
        assert!(
            jobs.iter().any(|job| job["job_id"] == event["job_id"]),
            "{event}: {planned}"
        );
    }
    assert_eq!(
        fs::read_to_string(base.join(planned_outputs[0])).unwrap(),
        "b\n"
    );
    assert_eq!(plan(base, targets)["job_count"], 0);
}

#[test]
fn deleted_wildcard_intermediate_with_existing_final_targets() {
    // Exact issue #22: explicit targets, two samples, no config wildcard lists.
    let dir = fixture(WORKFLOW);
    let base = dir.path();
    let targets = ["final/s1.txt", "final/s2.txt"];
    succeeded(&run(base, &targets), 4);
    fs::remove_file(base.join("mid/s2.txt")).unwrap();
    let text = ox(base, &["plan", "final/s1.txt", "final/s2.txt"]);
    assert!(text.contains("mid/s2.txt"), "{text}");
    assert_rebuild(base, &targets, &["mid/s2.txt", "final/s2.txt"]);
    assert_eq!(
        fs::read_to_string(base.join("final/s2.txt")).unwrap(),
        "B\n"
    );
}

#[test]
fn deleted_concrete_intermediate_control() {
    let dir = fixture(&WORKFLOW.replace("{sample}", "s2"));
    let base = dir.path();
    succeeded(&run(base, &["final/s2.txt"]), 2);
    fs::remove_file(base.join("mid/s2.txt")).unwrap();
    assert_rebuild(base, &["final/s2.txt"], &["mid/s2.txt", "final/s2.txt"]);
}

#[test]
fn deleted_intermediate_in_mixed_wildcard_and_concrete_chain() {
    let (mid, final_rule) = WORKFLOW.split_once("[rule.final]").unwrap();
    for spec in [
        format!("{mid}[rule.final]{}", final_rule.replace("{sample}", "s2")),
        format!("{}[rule.final]{final_rule}", mid.replace("{sample}", "s2")),
    ] {
        let dir = fixture(&spec);
        let base = dir.path();
        succeeded(&run(base, &["final/s2.txt"]), 2);
        fs::remove_file(base.join("mid/s2.txt")).unwrap();
        assert_rebuild(base, &["final/s2.txt"], &["mid/s2.txt", "final/s2.txt"]);
    }
}

#[test]
fn explicit_wildcard_target_outside_config_list_with_config_directory() {
    let spec = WORKFLOW
        .replace(
            "[rule.mid]",
            "[config]\nsamples = [\"s1\"]\nout_dir = \"final\"\n[rule.mid]",
        )
        .replace(
            "output = [\"final/{sample}.txt\"]",
            "output = [\"{config.out_dir}/{sample}.txt\"]",
        );
    let dir = fixture(&spec);
    let base = dir.path();
    succeeded(&run(base, &["final/s2.txt"]), 2);
    fs::remove_file(base.join("mid/s2.txt")).unwrap();
    assert_rebuild(base, &["final/s2.txt"], &["mid/s2.txt", "final/s2.txt"]);
}

#[test]
fn missing_wildcard_intermediate_propagates_through_two_consumers() {
    let spec = WORKFLOW.replace(
        "input = [\"mid/{sample}.txt\"]",
        "input = [\"next/{sample}.txt\"]",
    ) + r#"
[rule.next]
input = ["mid/{sample}.txt"]
output = ["next/{sample}.txt"]
shell = "mkdir -p next && cat {input} > {output}"
"#;
    let dir = fixture(&spec);
    let base = dir.path();
    let targets = ["final/s1.txt", "final/s2.txt"];
    succeeded(&run(base, &targets), 6);
    fs::remove_file(base.join("mid/s2.txt")).unwrap();
    assert_rebuild(
        base,
        &targets,
        &["mid/s2.txt", "next/s2.txt", "final/s2.txt"],
    );
}

#[test]
fn ordinary_cached_wildcard_output_is_not_an_adopted_leaf() {
    let dir = fixture(WORKFLOW);
    let base = dir.path();
    ox(base, &["run", "mid/s2.txt"]);
    fs::remove_file(base.join("in/s2.txt")).unwrap();
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(base)
        .args(["plan", "final/s2.txt"])
        .timeout(Duration::from_secs(30))
        .assert()
        .failure()
        .stderr(predicates::str::contains("in/s2.txt"));
}

#[test]
fn adopted_wildcard_leaf_remains_a_source_only_while_verified_and_inputs_missing() {
    let spec = WORKFLOW.replace("[rule.mid]", "[rule.mid]\ncache_platform = \"any\"");
    let producer = fixture(&spec);
    ox(producer.path(), &["run", "mid/s2.txt"]);
    ox(
        producer.path(),
        &["cache-export", "mid/s2.txt", "-o", "adoption.json"],
    );
    let consumer = fixture(&spec);
    let base = consumer.path();
    fs::remove_dir_all(base.join("in")).unwrap();
    fs::create_dir(base.join("mid")).unwrap();
    fs::copy(producer.path().join("mid/s2.txt"), base.join("mid/s2.txt")).unwrap();
    fs::copy(
        producer.path().join("adoption.json"),
        base.join("adoption.json"),
    )
    .unwrap();
    ox(base, &["cache-import", "adoption.json"]);
    assert_eq!(plan(base, &["final/s2.txt"])["job_count"], 1);
    succeeded(&run(base, &["final/s2.txt"]), 1);
    assert_eq!(plan(base, &["final/s2.txt"])["job_count"], 0);

    // No-cache must refuse the adopted shortcut. So must changed output bytes,
    // even though the adopted producer's raw input is still unavailable.
    for args in [
        vec!["run", "final/s2.txt", "--no-cache"],
        vec!["plan", "final/s2.txt"],
    ] {
        if args[0] == "plan" {
            fs::write(base.join("mid/s2.txt"), "tampered\n").unwrap();
        }
        Command::cargo_bin("ox")
            .unwrap()
            .current_dir(base)
            .args(args)
            .timeout(Duration::from_secs(30))
            .assert()
            .failure()
            .stderr(predicates::str::contains("in/s2.txt"));
    }
    fs::write(base.join("mid/s2.txt"), "b\n").unwrap();

    // Once the inputs return, provenance must no longer cut off the producer.
    fs::create_dir(base.join("in")).unwrap();
    fs::write(base.join("in/s2.txt"), "changed\n").unwrap();
    assert_eq!(plan(base, &["final/s2.txt"])["job_count"], 2);
    succeeded(&run(base, &["final/s2.txt"]), 2);
    assert_eq!(
        fs::read_to_string(base.join("final/s2.txt")).unwrap(),
        "CHANGED\n"
    );
}
