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
    let adopted_session = ox_api::SessionBuilder::new(base.join("Oxymakefile.toml"))
        .targets(["final/s2.txt"])
        .build()
        .unwrap();
    assert_eq!(adopted_session.job_graph.job_count(), 1);
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
            assert!(
                ox_api::SessionBuilder::new(base.join("Oxymakefile.toml"))
                    .targets(["final/s2.txt"])
                    .build()
                    .is_err()
            );
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

fn manual_fixture(input: &str, output: &str, source: &str, extra: &str) -> TempDir {
    let spec = format!(
        r#"ox_version = "0.1"
[config]
outdir = "data"
[rule.generate]
input = ["{input}"]
output = ["{output}"]
shell = "mkdir -p data && cat {{input}} > {{output}}"
{extra}
[rule.consume]
input = ["{source}"]
output = ["result.txt"]
shell = "cat {{input}} > {{output}}"
"#
    );
    let dir = fixture(&spec);
    let path = dir.path().join(source);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "hand maintained\n").unwrap();
    dir
}

fn assert_manual_source(dir: &TempDir, source: &str) {
    let planned = plan(dir.path(), &["result.txt"]);
    assert_eq!(planned["job_count"], 1, "{planned}");
    succeeded(&run(dir.path(), &["result.txt"]), 1);
    for path in [source, "result.txt"] {
        assert_eq!(
            fs::read_to_string(dir.path().join(path)).unwrap(),
            "hand maintained\n"
        );
    }
}

#[test]
fn handwritten_file_matching_wildcard_output_is_a_source() {
    let dir = manual_fixture("raw/{x}.csv", "data/{x}.csv", "data/manual.csv", "");
    assert_manual_source(&dir, "data/manual.csv");
}

#[test]
fn handwritten_file_matching_leading_wildcard_output_is_a_source() {
    let dir = manual_fixture("raw/{dir}.csv", "{dir}/data.csv", "cfg/data.csv", "");
    assert_manual_source(&dir, "cfg/data.csv");
}

#[test]
fn handwritten_file_matching_config_directory_output_is_a_source() {
    let dir = manual_fixture(
        "raw/{x}.csv",
        "{config.outdir}/{x}.csv",
        "data/manual.csv",
        "",
    );
    assert_manual_source(&dir, "data/manual.csv");
}

#[test]
fn producible_wildcard_output_overwrites_handwritten_file() {
    let dir = manual_fixture("raw/{x}.csv", "data/{x}.csv", "data/manual.csv", "");
    fs::create_dir(dir.path().join("raw")).unwrap();
    fs::write(dir.path().join("raw/manual.csv"), "generated\n").unwrap();
    assert_eq!(plan(dir.path(), &["result.txt"])["job_count"], 2);
    succeeded(&run(dir.path(), &["result.txt"]), 2);
    for path in ["data/manual.csv", "result.txt"] {
        assert_eq!(
            fs::read_to_string(dir.path().join(path)).unwrap(),
            "generated\n"
        );
    }
}

#[test]
fn wildcard_constraint_keeps_handwritten_file_a_source_even_with_raw_input() {
    let dir = manual_fixture(
        "raw/{x}.csv",
        "data/{x}.csv",
        "data/manual.csv",
        "[rule.generate.wildcard_constraints]\nx = \"sample[0-9]+\"",
    );
    fs::create_dir(dir.path().join("raw")).unwrap();
    fs::write(dir.path().join("raw/manual.csv"), "generated\n").unwrap();
    assert_manual_source(&dir, "data/manual.csv");
}

#[test]
fn source_fallback_does_not_hide_missing_intermediate_with_existing_final() {
    let dir = fixture(WORKFLOW);
    fs::create_dir(dir.path().join("final")).unwrap();
    fs::write(dir.path().join("final/s2.txt"), "old handwritten final\n").unwrap();
    assert_eq!(plan(dir.path(), &["final/s2.txt"])["job_count"], 2);
    succeeded(&run(dir.path(), &["final/s2.txt", "-j", "2"]), 2);
    assert_eq!(
        fs::read_to_string(dir.path().join("mid/s2.txt")).unwrap(),
        "b\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("final/s2.txt")).unwrap(),
        "B\n"
    );
}

#[test]
fn source_fallback_discards_speculative_jobs_and_allows_later_targets() {
    let dir = manual_fixture("raw/{x}.csv", "data/{x}.csv", "data/manual.csv", "");
    let file = dir.path().join("Oxymakefile.toml");
    let spec = fs::read_to_string(&file).unwrap().replace(
        "input = [\"raw/{x}.csv\"]",
        "input = [\"scratch/{x}.csv\", \"raw/{x}.csv\"]",
    ) + r#"
[rule.scratch]
input = ["in/s1.txt"]
output = ["scratch/{x}.csv"]
shell = "mkdir -p scratch && cat {input} > {output}"
"#;
    // Preserve the original nested fixture too: its raw producer's missing
    // source is now a hard error, rather than authorizing an ancestor fallback.
    let raw_rule = r#"
[rule.raw]
input = ["absent/{x}.csv"]
output = ["raw/{x}.csv"]
shell = "mkdir -p raw && cat {input} > {output}"
"#;
    fs::write(&file, format!("{spec}{raw_rule}")).unwrap();
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(dir.path())
        .args(["plan", "result.txt"])
        .timeout(Duration::from_secs(30))
        .assert()
        .failure()
        .stderr(predicates::str::contains("absent/manual.csv"));
    // With a directly unavailable raw input, all original rollback assertions
    // still apply, including resolving an abandoned scratch job as a later target.
    fs::write(file, spec).unwrap();
    assert_manual_source(&dir, "data/manual.csv");
    assert!(!dir.path().join("scratch/manual.csv").exists());
    // The abandoned producer must not leave scratch marked as already produced.
    assert_eq!(
        plan(dir.path(), &["result.txt", "scratch/manual.csv"])["job_count"],
        1
    );
}

#[test]
fn unavailable_cache_cannot_authorize_source_fallback() {
    let dir = manual_fixture("raw/{x}.csv", "data/{x}.csv", "data/manual.csv", "");
    // Without a readable cache we cannot establish that this file is unrecorded.
    fs::write(dir.path().join(".oxymake"), "not a cache directory").unwrap();
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(dir.path())
        .args(["query", "deps(result.txt)"])
        .timeout(Duration::from_secs(30))
        .assert()
        .failure()
        .stderr(predicates::str::contains("raw/manual.csv"));
}

#[test]
fn missing_shared_config_is_an_error_with_and_without_cache() {
    let spec = WORKFLOW
        .replace(
            "input = [\"mid/{sample}.txt\"]",
            "input = [\"mid/{sample}.txt\", \"cfg/settings.txt\"]",
        )
        .replace("tr a-z A-Z < {input}", "cat {input} | tr a-z A-Z");
    let dir = fixture(&spec);
    let base = dir.path();
    fs::create_dir(base.join("cfg")).unwrap();
    fs::write(base.join("cfg/settings.txt"), "settings\n").unwrap();
    ox(base, &["run", "final/s1.txt"]);
    fs::write(base.join("in/s1.txt"), "changed\n").unwrap();
    fs::remove_file(base.join("cfg/settings.txt")).unwrap();
    for remove_cache in [false, true] {
        if remove_cache {
            fs::remove_dir_all(base.join(".oxymake")).unwrap();
        }
        for command in ["plan", "run"] {
            Command::cargo_bin("ox")
                .unwrap()
                .current_dir(base)
                .args([command, "final/s1.txt"])
                .timeout(Duration::from_secs(30))
                .assert()
                .failure()
                .stderr(predicates::str::contains("cfg/settings.txt"));
        }
        assert_eq!(
            fs::read_to_string(base.join("final/s1.txt")).unwrap(),
            "A\nSETTINGS\n"
        );
    }
}

#[test]
fn deeper_missing_source_is_not_a_manual_final_with_or_without_cache() {
    let dir = fixture(WORKFLOW);
    let base = dir.path();
    ox(base, &["run", "final/s1.txt"]);
    fs::remove_file(base.join("mid/s1.txt")).unwrap();
    fs::remove_file(base.join("in/s1.txt")).unwrap();
    for remove_cache in [false, true] {
        if remove_cache {
            fs::remove_dir_all(base.join(".oxymake")).unwrap();
        }
        Command::cargo_bin("ox")
            .unwrap()
            .current_dir(base)
            .args(["plan", "final/s1.txt"])
            .timeout(Duration::from_secs(30))
            .assert()
            .failure()
            .stderr(predicates::str::contains("in/s1.txt"));
    }
}

#[test]
fn manual_target_fallback_uses_workflow_directory() {
    let dir = fixture(WORKFLOW);
    let base = dir.path();
    fs::remove_dir_all(base.join("in")).unwrap();
    fs::create_dir(base.join("mid")).unwrap();
    fs::write(base.join("mid/s1.txt"), "manual\n").unwrap();
    fs::create_dir(base.join("sub")).unwrap();
    for args in [
        vec!["plan", "-f", "../Oxymakefile.toml", "mid/s1.txt", "--json"],
        vec!["run", "-f", "../Oxymakefile.toml", "mid/s1.txt", "--json"],
    ] {
        let output = ox(&base.join("sub"), &args);
        if args[0] == "plan" {
            assert_eq!(
                serde_json::from_str::<Value>(&output).unwrap()["job_count"],
                0
            );
        }
    }
    assert!(!base.join("sub/mid").exists());
}

#[test]
fn cached_target_cannot_become_manual_from_subdirectory() {
    let dir = fixture(WORKFLOW);
    let base = dir.path();
    ox(base, &["run", "mid/s1.txt"]);
    fs::remove_file(base.join("in/s1.txt")).unwrap();
    fs::create_dir(base.join("sub")).unwrap();
    fs::create_dir(base.join("sub/mid")).unwrap();
    fs::write(base.join("sub/mid/s1.txt"), "decoy\n").unwrap();
    Command::cargo_bin("ox")
        .unwrap()
        .current_dir(base.join("sub"))
        .args(["plan", "-f", "../Oxymakefile.toml", "mid/s1.txt"])
        .timeout(Duration::from_secs(30))
        .assert()
        .failure()
        .stderr(predicates::str::contains("in/s1.txt"));
}

#[test]
fn cached_workflow_plan_and_run_from_subdirectory_are_noops() {
    let dir = fixture(WORKFLOW);
    let base = dir.path();
    succeeded(&run(base, &["final/s1.txt"]), 2);
    fs::create_dir(base.join("sub")).unwrap();
    let output = ox(
        &base.join("sub"),
        &[
            "plan",
            "-f",
            "../Oxymakefile.toml",
            "final/s1.txt",
            "--json",
        ],
    );
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["job_count"],
        0
    );
    let output = ox(
        &base.join("sub"),
        &["run", "-f", "../Oxymakefile.toml", "final/s1.txt", "--json"],
    );
    let events: Vec<Value> = output
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    succeeded(&events, 0);
    assert!(!base.join("sub/mid").exists());
    // A real rebuild also executes against the same workflow root.
    fs::write(base.join("in/s1.txt"), "changed\n").unwrap();
    ox(
        &base.join("sub"),
        &["run", "-f", "../Oxymakefile.toml", "final/s1.txt"],
    );
    assert_eq!(
        fs::read_to_string(base.join("final/s1.txt")).unwrap(),
        "CHANGED\n"
    );
    assert!(!base.join("sub/mid").exists());
}
