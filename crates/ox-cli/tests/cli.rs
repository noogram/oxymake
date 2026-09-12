//! Integration tests for the OxyMake CLI binary.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use std::fs;
use tempfile::TempDir;

/// Path to the simple fixture Oxymakefile.
fn simple_fixture() -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    format!("{}/../../tests/fixtures/simple/Oxymakefile.toml", manifest)
}

/// Create a Command for the `oxymake` binary.
fn ox() -> Command {
    Command::cargo_bin("oxymake").expect("binary should exist")
}

// ---------------------------------------------------------------------------
// Help and version
// ---------------------------------------------------------------------------

#[test]
fn help_shows_usage() {
    ox().arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("OxyMake"));
}

#[test]
fn version_shows_version() {
    ox().arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::contains(env!("CARGO_PKG_VERSION")));
}

/// A completed run records where each artefact was produced and whether its
/// rule opted into cross-platform reuse. The scope column must also make all
/// `any` entries directly enumerable for recall.
#[test]
fn run_records_cache_platform_provenance_and_enumerates_any_entries() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();
    let oxymakefile = base.join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.exact]
output = ["exact.txt"]
shell = "printf exact > exact.txt"

[rule.portable]
output = ["portable.txt"]
shell = "printf portable > portable.txt"
cache_platform = "any"
"#,
    )
    .unwrap();

    for target in ["exact.txt", "portable.txt"] {
        ox().args(["run", target, "-f", oxymakefile.to_str().unwrap()])
            .current_dir(base)
            .assert()
            .success();
    }

    let conn = rusqlite::Connection::open(base.join(".oxymake/cache/cache.db")).unwrap();
    let rows: Vec<(String, String)> = conn
        .prepare("SELECT platform, platform_scope FROM cache_entries ORDER BY platform_scope")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    let platform = ox_cache::current_platform();
    assert_eq!(
        rows,
        vec![(platform.clone(), "any".into()), (platform, "exact".into())]
    );

    let any_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cache_entries WHERE platform_scope = 'any'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(any_count, 1);
}

/// A transferred product becomes a trusted leaf even when the producer's raw
/// input is intentionally absent in the consuming tree.
#[test]
fn cache_export_import_continues_without_raw_inputs() {
    let producer = TempDir::new().unwrap();
    let consumer = TempDir::new().unwrap();
    let manifest = producer.path().join("portable-cache.json");
    let workflow = r#"ox_version = "0.1"

[rule.upstream]
input = ["raw.txt"]
output = ["product.txt"]
shell = "printf upstream-ran >> executions.log; cat raw.txt > product.txt"
cache_platform = "any"

[rule.downstream]
input = ["product.txt"]
output = ["final.txt"]
shell = "printf downstream-ran >> executions.log; cat product.txt > final.txt"
"#;
    for root in [producer.path(), consumer.path()] {
        fs::write(root.join("Oxymakefile.toml"), workflow).unwrap();
    }
    fs::write(producer.path().join("raw.txt"), "foreign result\n").unwrap();

    ox().args(["run", "product.txt"])
        .current_dir(producer.path())
        .assert()
        .success();
    ox().args([
        "cache-export",
        "product.txt",
        "--output",
        manifest.to_str().unwrap(),
    ])
    .current_dir(producer.path())
    .assert()
    .success();
    fs::copy(
        producer.path().join("product.txt"),
        consumer.path().join("product.txt"),
    )
    .unwrap();

    fs::write(consumer.path().join("product.txt"), "tampered\n").unwrap();
    ox().args(["cache-import", manifest.to_str().unwrap()])
        .current_dir(consumer.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains("output hash mismatch"));
    fs::copy(
        producer.path().join("product.txt"),
        consumer.path().join("product.txt"),
    )
    .unwrap();

    ox().args(["cache-import", manifest.to_str().unwrap()])
        .current_dir(consumer.path())
        .assert()
        .success();
    assert!(!consumer.path().join("raw.txt").exists());

    ox().args(["run", "final.txt"])
        .current_dir(consumer.path())
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(consumer.path().join("final.txt")).unwrap(),
        "foreign result\n"
    );
    assert_eq!(
        fs::read_to_string(consumer.path().join("executions.log")).unwrap(),
        "downstream-ran"
    );
}

#[test]
fn unsupported_future_adoption_manifest_reports_version_before_body_schema() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("future.json");
    fs::write(
        &manifest,
        r#"{
  "kind": "oxymake.cache-adoption-manifest",
  "format_version": 2,
  "producer_version": "0.4.0",
  "new_required_field": true
}"#,
    )
    .unwrap();

    ox().args(["cache-import", manifest.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains("format version 2"))
        .stderr(predicates::str::contains("produced by ox 0.4.0"))
        .stderr(predicates::str::contains("upgrade ox on this machine"))
        .stderr(predicates::str::contains("missing field").not());
}

#[test]
fn cache_import_rejects_the_wrong_manifest_kind_before_workflow_loading() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("other.json");
    fs::write(
        &manifest,
        r#"{
  "kind": "some.other.document",
  "format_version": 1,
  "producer_version": "0.3.0",
  "entries": []
}"#,
    )
    .unwrap();

    ox().args(["cache-import", manifest.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains("not an adoption manifest"));
}

/// A directory remote cache restores a missing output from the shared
/// content-addressed artifact store instead of re-executing the job.
#[test]
fn run_restores_missing_output_from_directory_remote_cache() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();
    let remote = base.join("shared-cache");
    let oxymakefile = base.join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.copy]
input = ["input.txt"]
output = ["output.txt"]
shell = "cat {input} > {output}"
"#,
    )
    .unwrap();
    fs::write(base.join("input.txt"), "remote cache contents\n").unwrap();

    ox().args([
        "run",
        "--cache-remote",
        remote.to_str().unwrap(),
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(base)
    .assert()
    .success();
    assert!(base.join("output.txt").exists());

    fs::remove_file(base.join("output.txt")).unwrap();

    ox().args([
        "run",
        "--cache-remote",
        remote.to_str().unwrap(),
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(base)
    .assert()
    .success()
    .stdout(predicates::str::contains("1 of 1 job(s) up-to-date"));

    assert_eq!(
        fs::read_to_string(base.join("output.txt")).unwrap(),
        "remote cache contents\n"
    );
}

/// Identical workflow checkouts compute the same key and can restore through
/// the directory blob store when the local SQLite index is also available.
/// The copied index is intentional: DirectoryCache transports blobs only,
/// not the computation-key-to-output manifest.
#[test]
fn directory_remote_cache_hits_from_a_second_identical_checkout() {
    let dir = TempDir::new().unwrap();
    let first = dir.path().join("checkout-a");
    let second = dir.path().join("checkout-b");
    let remote = dir.path().join("shared-blobs");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();

    let first_workflow = first.join("Oxymakefile.toml");
    fs::write(
        &first_workflow,
        r#"ox_version = "0.1"

[rule.copy]
input = ["input.txt"]
output = ["output.txt"]
shell = "cat {input} > {output}"
"#,
    )
    .unwrap();
    fs::write(first.join("input.txt"), "portable cache key\n").unwrap();
    fs::copy(&first_workflow, second.join("Oxymakefile.toml")).unwrap();
    fs::copy(first.join("input.txt"), second.join("input.txt")).unwrap();

    ox().args([
        "run",
        "--cache-remote",
        remote.to_str().unwrap(),
        "-f",
        first_workflow.to_str().unwrap(),
    ])
    .current_dir(&first)
    .assert()
    .success()
    .stdout(predicates::str::contains("1 succeeded"));

    let first_cache = first.join(".oxymake/cache");
    let second_cache = second.join(".oxymake/cache");
    fs::create_dir_all(&second_cache).unwrap();
    for file in ["cache.db", "cache.db-wal", "cache.db-shm"] {
        let source = first_cache.join(file);
        if source.exists() {
            fs::copy(source, second_cache.join(file)).unwrap();
        }
    }

    ox().args([
        "run",
        "--cache-remote",
        remote.to_str().unwrap(),
        "-f",
        second.join("Oxymakefile.toml").to_str().unwrap(),
    ])
    .current_dir(&second)
    .assert()
    .success()
    .stdout(predicates::str::contains("1 of 1 job(s) up-to-date"));

    assert_eq!(
        fs::read_to_string(second.join("output.txt")).unwrap(),
        "portable cache key\n"
    );
}

/// The operator handbook must be reachable through both surfaces — `ox guide`
/// (runs the command) and `ox help guide` (clap long help) — and both must
/// carry the stable anchor string from the handbook text.
#[test]
fn guide_and_help_guide_both_print_handbook() {
    const ANCHOR: &str = "OxyMake — operator handbook";

    ox().arg("guide")
        .assert()
        .success()
        .stdout(predicates::str::contains(ANCHOR));

    ox().args(["help", "guide"])
        .assert()
        .success()
        .stdout(predicates::str::contains(ANCHOR));
}

// ---------------------------------------------------------------------------
// Lint
// ---------------------------------------------------------------------------

#[test]
fn lint_valid_oxymakefile() {
    ox().args(["lint", "-f", &simple_fixture()])
        .assert()
        .success()
        .stdout(predicates::str::contains("Oxymakefile is valid"));
}

#[test]
fn lint_invalid_toml() {
    let dir = TempDir::new().unwrap();
    let bad_file = dir.path().join("Oxymakefile.toml");
    fs::write(&bad_file, "this is not [[ valid toml").unwrap();

    ox().args(["lint", "-f", bad_file.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("error"));
}

#[test]
fn lint_missing_file() {
    ox().args(["lint", "-f", "/nonexistent/Oxymakefile.toml"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("cannot read"));
}

#[test]
fn lint_json_valid_outputs_json() {
    let output = ox()
        .args(["lint", "--json", "-f", &simple_fixture()])
        .output()
        .expect("command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON");
    assert_eq!(parsed["valid"], true);
    assert!(parsed["rule_count"].as_u64().unwrap() > 0);
    assert!(parsed["errors"].as_array().unwrap().is_empty());
    // stderr must be empty — no human text leaked
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.is_empty(),
        "stderr should be empty in JSON mode, got: {stderr}"
    );
}

#[test]
fn lint_json_missing_file_outputs_json() {
    let output = ox()
        .args(["lint", "--json", "-f", "/nonexistent/Oxymakefile.toml"])
        .output()
        .expect("command should run");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON");
    assert_eq!(parsed["valid"], false);
    assert!(!parsed["errors"].as_array().unwrap().is_empty());
}

/// A `[gate.*]` whose `before` names a rule that does not exist is a
/// validation error (issue #2, verification finding 2): a misspelled rule
/// name must not leave the rule it meant to guard unguarded. The message
/// names the gate and the unknown rule, in human and `--json` output.
#[test]
fn lint_rejects_gate_naming_unknown_rule() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("Oxymakefile.toml");
    fs::write(
        &file,
        r#"ox_version = "0.1"
format_version = "1"

[gate.approval]
after = []
before = ["typo_rule"]

[rule.guarded]
input = []
output = ["out.txt"]
shell = "echo RAN > out.txt"
"#,
    )
    .unwrap();

    ox().args(["lint", "-f", file.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "gate `approval` lists unknown rule `typo_rule` in `before`",
        ));

    let output = ox()
        .args(["lint", "--json", "-f", file.to_str().unwrap()])
        .output()
        .expect("command should run");
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON");
    assert_eq!(parsed["valid"], false);
    let errors = parsed["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0]
            .as_str()
            .unwrap()
            .contains("gate `approval` lists unknown rule `typo_rule` in `before`"),
        "{errors:?}"
    );
}

/// A well-formed gate produces no warning: the "enforcement is not wired"
/// stopgap warning was removed once gates started to block.
#[test]
fn lint_valid_gate_produces_no_warning() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("Oxymakefile.toml");
    fs::write(
        &file,
        r#"ox_version = "0.1"
format_version = "1"

[gate.approval]
after = []
before = ["guarded"]

[rule.guarded]
input = []
output = ["out.txt"]
shell = "echo RAN > out.txt"
"#,
    )
    .unwrap();

    let output = ox()
        .args(["lint", "--json", "-f", file.to_str().unwrap()])
        .output()
        .expect("command should run");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON");
    assert_eq!(parsed["valid"], true);
    assert!(parsed["warnings"].as_array().unwrap().is_empty());
    assert!(!stdout.contains("not wired"));
}

/// An Oxymakefile without any `[gate.*]` section produces no gate-related
/// warnings.
#[test]
fn lint_no_gate_warning_without_gates() {
    let output = ox()
        .args(["lint", "--json", "-f", &simple_fixture()])
        .output()
        .expect("command should run");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON");
    assert!(parsed["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn lint_json_invalid_toml_outputs_json() {
    let dir = TempDir::new().unwrap();
    let bad_file = dir.path().join("Oxymakefile.toml");
    fs::write(&bad_file, "this is not [[ valid toml").unwrap();

    let output = ox()
        .args(["lint", "--json", "-f", bad_file.to_str().unwrap()])
        .output()
        .expect("command should run");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON");
    assert_eq!(parsed["valid"], false);
    assert!(!parsed["errors"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

#[test]
fn plan_shows_job_count() {
    // The simple fixture has 3 samples (A, B, C) and a process rule,
    // but we need source files to exist for resolve to succeed.
    // Use --level=rules to just show the rule graph instead.
    ox().args(["plan", "--level=rules", "-f", &simple_fixture()])
        .assert()
        .success()
        .stdout(predicates::str::contains("rules"));
}

#[test]
fn plan_with_source_files() {
    let dir = TempDir::new().unwrap();

    // Create Oxymakefile.
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
samples = ["A", "B"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "cat {input} | sort > {output}"
"#,
    )
    .unwrap();

    // Create source data files.
    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "data_a").unwrap();
    fs::write(data_dir.join("B.csv"), "data_b").unwrap();

    ox().args(["plan", "-f", oxymakefile.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("2 jobs"));
}

fn write_two_rule_workflow(base: &std::path::Path) -> std::path::PathBuf {
    let oxymakefile = base.join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.a]
output = ["a.txt"]
shell = "printf 'constant\\n' > {output}"

[rule.b]
input = ["a.txt"]
output = ["b.txt"]
shell = "cat {input} > {output}"
"#,
    )
    .unwrap();
    oxymakefile
}

fn run_two_rule_workflow(base: &std::path::Path, oxymakefile: &std::path::Path) -> String {
    let output = ox()
        .args(["run", "b.txt", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .output()
        .expect("run should execute");
    assert!(output.status.success(), "run failed: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn planned_jobs(base: &std::path::Path, oxymakefile: &std::path::Path) -> Vec<serde_json::Value> {
    let output = ox()
        .args([
            "plan",
            "b.txt",
            "--json",
            "-f",
            oxymakefile.to_str().unwrap(),
        ])
        .current_dir(base)
        .output()
        .expect("plan should run");
    assert!(output.status.success(), "plan failed: {output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    json["jobs"].as_array().unwrap().clone()
}

struct RunObservation {
    started_jobs: Vec<String>,
    succeeded: u64,
    skipped: u64,
}

fn observe_run(base: &std::path::Path, oxymakefile: &std::path::Path) -> RunObservation {
    let output = ox()
        .args([
            "run",
            "b.txt",
            "--json",
            "-f",
            oxymakefile.to_str().unwrap(),
        ])
        .current_dir(base)
        .output()
        .expect("run should execute");
    assert!(output.status.success(), "run failed: {output:?}");
    let events: Vec<serde_json::Value> = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .collect();
    let started_jobs = events
        .iter()
        .filter(|event| event["event"] == "job_started")
        .map(|event| event["job_id"].as_str().unwrap().to_owned())
        .collect();
    let completed = events
        .iter()
        .find(|event| event["event"] == "run_completed")
        .unwrap();
    RunObservation {
        started_jobs,
        succeeded: completed["succeeded"].as_u64().unwrap(),
        skipped: completed["skipped"].as_u64().unwrap(),
    }
}

fn job_field(jobs: &[serde_json::Value], field: &str) -> Vec<String> {
    jobs.iter()
        .map(|job| job[field].as_str().unwrap().to_owned())
        .collect()
}

/// A missing intermediate must not make `plan` stop at an existing final output.
#[test]
fn plan_resolves_full_chain_when_intermediate_output_is_missing() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();
    let oxymakefile = write_two_rule_workflow(base);
    assert!(run_two_rule_workflow(base, &oxymakefile).contains("2 succeeded"));

    fs::remove_file(base.join("a.txt")).unwrap();
    let jobs = planned_jobs(base, &oxymakefile);
    assert_eq!(job_field(&jobs, "rule"), ["a", "b"]);
    assert_eq!(
        job_field(&jobs, "reason"),
        ["output missing: a.txt", "upstream rebuilt"]
    );

    let run = observe_run(base, &oxymakefile);
    assert_eq!(job_field(&jobs, "job_id"), run.started_jobs);
    assert_eq!((run.succeeded, run.skipped), (2, 0));
}

/// A missing final output selects only its producer when upstream is cached.
#[test]
fn plan_resolves_full_chain_when_final_output_is_missing() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();
    let oxymakefile = write_two_rule_workflow(base);
    assert!(run_two_rule_workflow(base, &oxymakefile).contains("2 succeeded"));

    fs::remove_file(base.join("b.txt")).unwrap();
    let jobs = planned_jobs(base, &oxymakefile);
    assert_eq!(job_field(&jobs, "rule"), ["b"]);
    assert_eq!(job_field(&jobs, "reason"), ["output missing: b.txt"]);

    let run = observe_run(base, &oxymakefile);
    assert_eq!(job_field(&jobs, "job_id"), run.started_jobs);
    assert_eq!((run.succeeded, run.skipped), (1, 1));
}

#[test]
fn plan_reports_no_jobs_when_everything_is_up_to_date() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();
    let oxymakefile = write_two_rule_workflow(base);
    assert!(run_two_rule_workflow(base, &oxymakefile).contains("2 succeeded"));

    let jobs = planned_jobs(base, &oxymakefile);
    assert!(jobs.is_empty());
    let run = observe_run(base, &oxymakefile);
    assert_eq!(job_field(&jobs, "job_id"), run.started_jobs);
    assert_eq!((run.succeeded, run.skipped), (0, 2));
}

/// `ox plan` on an Oxymakefile that carries `source_line` (as the translator
/// emits) and fails with `no rule produces output` must cite the original
/// Snakefile line in its error message — either directly (`Snakefile:N`)
/// or by pointing at the `.escalations.toml` sidecar that records the
/// dropped rule.
#[test]
fn plan_failure_on_translated_oxymakefile_cites_snakefile_line() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    // A workflow that requests `results/out.txt` but ships no rule that
    // produces it — mimicking a Snakemake rule the translator dropped.
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["results/out.txt"]

[rule.untranslated_stub]
source_line = 42
output = ["unrelated.txt"]
shell = "touch {output}"
"#,
    )
    .unwrap();

    let escalations = dir.path().join("Oxymakefile.toml.escalations.toml");
    fs::write(
        &escalations,
        r#"
[meta]
total_escalations = 1
tier_counts = { mechanical_deferred = 0, assisted = 0, human = 1 }

[[escalation]]
id = "esc-0001"
tier = "Human"
category = "SilentDrop"
severity = "Correctness"
rule_name = "produce_results"
construct = "rule"
source_line = 17
original_code = """
rule produce_results:
    output: "results/out.txt"
    shell: "touch {output}"
"""
"#,
    )
    .unwrap();

    let assert = ox()
        .args(["plan", "-f", oxymakefile.to_str().unwrap()])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("results/out.txt"),
        "expected the missing path in stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains("Snakefile:17") || stderr.contains("dropped by translation"),
        "expected a Snakefile line cite or a dropped-by-translation hint in stderr, got:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

#[test]
fn init_creates_oxymakefile() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("new_project");

    ox().args(["init", target.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("Initialized"));

    assert!(target.join("Oxymakefile.toml").exists());
    assert!(target.join(".oxymake").exists());
}

#[test]
fn init_refuses_overwrite_without_force() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Oxymakefile.toml"), "existing").unwrap();

    ox().args(["init", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("already exists"));
}

#[test]
fn init_force_overwrites() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Oxymakefile.toml"), "existing").unwrap();

    ox().args(["init", "--force", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("Oxymakefile.toml")).unwrap();
    assert!(content.contains("ox_version"));
}

// ---------------------------------------------------------------------------
// Run (dry-run mode)
// ---------------------------------------------------------------------------

#[test]
fn run_dry_run_with_source_files() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
samples = ["X"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "cat data/{sample}.csv > results/{sample}.txt"
"#,
    )
    .unwrap();

    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join("X.csv"), "x_data").unwrap();

    ox().args(["run", "--dry-run", "-f", oxymakefile.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("1 job(s) would execute"));
}

#[test]
fn run_dry_run_json_outputs_ndjson() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
samples = ["X"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "cat data/{sample}.csv > results/{sample}.txt"
"#,
    )
    .unwrap();

    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join("X.csv"), "x_data").unwrap();

    let output = ox()
        .args([
            "run",
            "--dry-run",
            "--json",
            "-f",
            oxymakefile.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).unwrap();
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line should be valid JSON"))
        .collect();

    // First line: dry_run_summary
    assert_eq!(lines[0]["event"], "dry_run_summary");
    assert_eq!(lines[0]["total_jobs"], 1);

    // Second line: dry_run_job
    assert_eq!(lines[1]["event"], "dry_run_job");
    assert_eq!(lines[1]["rule"], "process");
    assert!(lines[1]["outputs"].is_array());
    assert!(lines[1]["inputs"].is_array());
}

// ---------------------------------------------------------------------------
// Run (actual execution)
// ---------------------------------------------------------------------------

#[test]
fn run_executes_simple_workflow() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    let results_dir = dir.path().join("results");
    let data_dir = dir.path().join("data");

    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&results_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "zebra\napple\nbanana\n").unwrap();

    // Use absolute paths in the shell command so it works from any cwd.
    let data_path = data_dir.display();
    let results_path = results_dir.display();

    fs::write(
        &oxymakefile,
        format!(
            r#"ox_version = "0.1"

[config]
samples = ["A"]

[rule.all]
input = ["results/{{sample}}.txt"]

[rule.process]
input = ["data/{{sample}}.csv"]
output = ["results/{{sample}}.txt"]
shell = "sort {data_path}/{{sample}}.csv > {results_path}/{{sample}}.txt"
"#
        ),
    )
    .unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("1 succeeded"));

    // Verify the output was created with sorted content.
    let output = fs::read_to_string(results_dir.join("A.txt")).unwrap();
    assert!(output.contains("apple"));
}

/// Regression test (pre-pub blocker): translated Snakemake `run:` blocks
/// must actually execute, not just translate and dry-run.
///
/// Snakemake injects `input`, `output`, `params`, `wildcards`, `threads`,
/// and `log` as objects into the `run:` namespace. The executor must provide
/// an equivalent preamble — otherwise `input[0]` resolves to the Python
/// builtin `input` function and the first real run crashes with
/// `TypeError: 'builtin_function_or_method' object is not subscriptable`.
#[test]
fn run_executes_translated_snakemake_run_block() {
    let dir = TempDir::new().unwrap();

    let manifest = env!("CARGO_MANIFEST_DIR");
    let fixture = format!(
        "{}/../ox-translate/tests/fixtures/python/Snakefile",
        manifest
    );
    fs::copy(&fixture, dir.path().join("Snakefile")).unwrap();

    fs::create_dir_all(dir.path().join("data")).unwrap();
    fs::write(dir.path().join("data/sample1.csv"), "a,b\n1,2\n3,4\n").unwrap();

    // Command 1: translate the Snakefile.
    ox().args(["translate", "Snakefile", "-o", "Oxymakefile.toml"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Command 2: run it for real (the rule is wildcard-only, so name a
    // concrete target).
    ox().args(["run", "-f", "Oxymakefile.toml", "results/sample1.json"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("1 succeeded"));

    // The run: block counted the CSV rows and wrote them as JSON.
    let output = fs::read_to_string(dir.path().join("results/sample1.json")).unwrap();
    assert_eq!(output.trim(), r#"{"rows": 3}"#);
}

/// Regression test for ox-49z: state.db must be created even with --no-cache.
///
/// When cache is disabled, .oxymake/ was never created, causing StateDb::open
/// to fail silently. This left ox status/history broken after execution.
#[test]
fn run_creates_state_db_without_cache() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    let results_dir = dir.path().join("results");
    let data_dir = dir.path().join("data");

    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&results_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "zebra\napple\nbanana\n").unwrap();

    let data_path = data_dir.display();
    let results_path = results_dir.display();

    fs::write(
        &oxymakefile,
        format!(
            r#"ox_version = "0.1"

[config]
samples = ["A"]

[rule.all]
input = ["results/{{sample}}.txt"]

[rule.process]
input = ["data/{{sample}}.csv"]
output = ["results/{{sample}}.txt"]
shell = "sort {data_path}/{{sample}}.csv > {results_path}/{{sample}}.txt"
"#
        ),
    )
    .unwrap();

    ox().args(["run", "--no-cache", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("1 succeeded"));

    // state.db must exist even without cache — this is the ox-49z regression.
    assert!(
        dir.path().join(".oxymake/state.db").exists(),
        "state.db must be created regardless of cache setting"
    );
}

// ---------------------------------------------------------------------------
// Snapshot diff — identical snapshots
// ---------------------------------------------------------------------------

/// Regression test (ox-wed): `ox snapshot diff --json` on two identical snapshots
/// must emit a summary line with `{"type":"summary","unchanged":N,...}` so that
/// consumers can distinguish "identical" from "error" (previously produced zero
/// output lines and exit 0).
#[test]
fn snapshot_diff_identical_emits_summary_json() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    let results_dir = dir.path().join("results");
    let data_dir = dir.path().join("data");

    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&results_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "hello\n").unwrap();

    let data_path = data_dir.display();
    let results_path = results_dir.display();

    fs::write(
        &oxymakefile,
        format!(
            r#"ox_version = "0.1"

[config]
samples = ["A"]

[rule.all]
input = ["results/{{sample}}.txt"]

[rule.process]
input = ["data/{{sample}}.csv"]
output = ["results/{{sample}}.txt"]
shell = "cp {data_path}/{{sample}}.csv {results_path}/{{sample}}.txt"
"#
        ),
    )
    .unwrap();

    // Run the workflow to populate state.db
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    // Create two snapshots at the same state — they should be identical.
    ox().args([
        "snapshot",
        "create",
        "snap1",
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(dir.path())
    .assert()
    .success();

    ox().args([
        "snapshot",
        "create",
        "snap2",
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(dir.path())
    .assert()
    .success();

    // Diff the two identical snapshots with --json
    let output = ox()
        .args(["snapshot", "diff", "snap1", "snap2", "--json"])
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).unwrap();
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line should be valid JSON"))
        .collect();

    // Must have at least one line (the summary)
    assert!(
        !lines.is_empty(),
        "snapshot diff --json on identical snapshots must produce output"
    );

    // Find the summary line
    let summary = lines
        .iter()
        .find(|l| l["type"] == "summary")
        .expect("must contain a summary line");

    assert_eq!(summary["changed"], 0);
    assert_eq!(summary["added"], 0);
    assert_eq!(summary["removed"], 0);
    // The workflow has 1 job (process:A), so unchanged should be 1
    assert!(
        summary["unchanged"].as_u64().unwrap() > 0,
        "identical snapshots must report unchanged count"
    );
}

/// Verify text mode also reports unchanged count for identical snapshots.
#[test]
fn snapshot_diff_identical_reports_unchanged_text() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    let results_dir = dir.path().join("results");
    let data_dir = dir.path().join("data");

    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&results_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "hello\n").unwrap();

    let data_path = data_dir.display();
    let results_path = results_dir.display();

    fs::write(
        &oxymakefile,
        format!(
            r#"ox_version = "0.1"

[config]
samples = ["A"]

[rule.all]
input = ["results/{{sample}}.txt"]

[rule.process]
input = ["data/{{sample}}.csv"]
output = ["results/{{sample}}.txt"]
shell = "cp {data_path}/{{sample}}.csv {results_path}/{{sample}}.txt"
"#
        ),
    )
    .unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    ox().args([
        "snapshot",
        "create",
        "snap1",
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(dir.path())
    .assert()
    .success();

    ox().args([
        "snapshot",
        "create",
        "snap2",
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(dir.path())
    .assert()
    .success();

    // Text mode should report unchanged count
    ox().args(["snapshot", "diff", "snap1", "snap2"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("unchanged"));
}

// ---------------------------------------------------------------------------
// Invalidate
// ---------------------------------------------------------------------------

/// Regression test: `ox invalidate --rule` must match cache entries even when
/// the output files already exist on disk (i.e. after a successful `ox run`).
/// Previously, the resolver treated existing outputs as source files and
/// silently produced zero jobs, so invalidation found nothing to remove.
#[test]
fn invalidate_rule_matches_existing_outputs() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    let results_dir = dir.path().join("results");
    let data_dir = dir.path().join("data");

    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&results_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "data_a").unwrap();
    fs::write(data_dir.join("B.csv"), "data_b").unwrap();

    // Simulate completed outputs already on disk.
    fs::write(results_dir.join("A.txt"), "result_a").unwrap();
    fs::write(results_dir.join("B.txt"), "result_b").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
samples = ["A", "B"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "cp data/{sample}.csv results/{sample}.txt"
"#,
    )
    .unwrap();

    // Seed a cache manifest with entries keyed by the output paths.
    let oxymake_dir = dir.path().join(".oxymake");
    let cache_dir = oxymake_dir.join("cache");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(
        cache_dir.join("manifest.json"),
        r#"{
  "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": {
    "cache_key": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "output_hashes": { "results/A.txt": "1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a" },
    "output_mtimes": {},
    "completed_at": 1000
  },
  "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb": {
    "cache_key": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "output_hashes": { "results/B.txt": "1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b" },
    "output_mtimes": {},
    "completed_at": 1000
  }
}"#,
    )
    .unwrap();

    // Invalidate by rule name.
    ox().args([
        "invalidate",
        "--rule",
        "process",
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(dir.path())
    .assert()
    .success()
    .stdout(predicates::str::contains("Invalidated 2 cache entry(ies)"));
}

/// Regression test: `ox invalidate` must delete output files from disk so that
/// the mtime-based cache check (the default) correctly forces a rebuild.
/// Previously, invalidation only cleared the DB — the stateless mtime check
/// still saw existing outputs and skipped the rebuild.
#[test]
fn invalidate_deletes_output_files() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    let results_dir = dir.path().join("results");
    let data_dir = dir.path().join("data");

    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&results_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "data_a").unwrap();

    // Create an output file that looks like a completed run.
    let output_file = results_dir.join("A.txt");
    fs::write(&output_file, "result_a").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.process]
input = ["data/A.csv"]
output = ["results/A.txt"]
shell = "cp data/A.csv results/A.txt"
"#,
    )
    .unwrap();

    // Seed a cache manifest.
    let oxymake_dir = dir.path().join(".oxymake");
    let cache_dir = oxymake_dir.join("cache");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(
        cache_dir.join("manifest.json"),
        r#"{
  "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": {
    "cache_key": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "output_hashes": { "results/A.txt": "1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a" },
    "output_mtimes": {},
    "completed_at": 1000
  }
}"#,
    )
    .unwrap();

    // Invalidate by rule name — should remove cache AND delete output file.
    ox().args([
        "invalidate",
        "--rule",
        "process",
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(dir.path())
    .assert()
    .success()
    .stdout(predicates::str::contains("Deleted"));

    assert!(
        !output_file.exists(),
        "output file should be deleted after invalidation"
    );
}

/// Verify `--keep-outputs` preserves output files on disk.
#[test]
fn invalidate_keep_outputs_preserves_files() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    let results_dir = dir.path().join("results");
    let data_dir = dir.path().join("data");

    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&results_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "data_a").unwrap();

    let output_file = results_dir.join("A.txt");
    fs::write(&output_file, "result_a").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.process]
input = ["data/A.csv"]
output = ["results/A.txt"]
shell = "cp data/A.csv results/A.txt"
"#,
    )
    .unwrap();

    let oxymake_dir = dir.path().join(".oxymake");
    let cache_dir = oxymake_dir.join("cache");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(
        cache_dir.join("manifest.json"),
        r#"{
  "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": {
    "cache_key": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "output_hashes": { "results/A.txt": "1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a" },
    "output_mtimes": {},
    "completed_at": 1000
  }
}"#,
    )
    .unwrap();

    // Invalidate with --keep-outputs — output file should remain.
    ox().args([
        "invalidate",
        "--rule",
        "process",
        "--keep-outputs",
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(dir.path())
    .assert()
    .success()
    .stdout(predicates::str::contains("Invalidated"));

    assert!(
        output_file.exists(),
        "output file should be preserved with --keep-outputs"
    );
}

// ---------------------------------------------------------------------------
// Run --json (actual execution) — regression test for ox-6cm
// ---------------------------------------------------------------------------

/// Regression test for ox-6cm: `ox run --json` must produce only valid NDJSON
/// on stdout. Previously, a human-readable "Completed: N succeeded..." summary
/// line was appended after the NDJSON events, breaking JSON parsers.
#[test]
fn run_json_stdout_is_pure_ndjson() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    let results_dir = dir.path().join("results");
    let data_dir = dir.path().join("data");

    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&results_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "hello\n").unwrap();

    let data_path = data_dir.display();
    let results_path = results_dir.display();

    fs::write(
        &oxymakefile,
        format!(
            r#"ox_version = "0.1"

[config]
samples = ["A"]

[rule.all]
input = ["results/{{sample}}.txt"]

[rule.process]
input = ["data/{{sample}}.csv"]
output = ["results/{{sample}}.txt"]
shell = "cp {data_path}/{{sample}}.csv {results_path}/{{sample}}.txt"
"#
        ),
    )
    .unwrap();

    let output = ox()
        .args(["run", "--json", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).unwrap();
    assert!(!stdout.is_empty(), "stdout must not be empty in JSON mode");

    // Every non-empty line must be valid JSON — no human-readable summary.
    for (i, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("line {i} is not valid JSON: {e}\nline content: {line}"));
    }
}

// ---------------------------------------------------------------------------
// Stub commands
// ---------------------------------------------------------------------------

#[test]
fn status_no_state() {
    let dir = TempDir::new().unwrap();
    ox().args(["status"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("No OxyMake state found"));
}

/// `ox status --json` must always emit valid JSON on stdout, even when no
/// state exists yet — an agent parsing stdout as JSON must not crash on a
/// human-readable sentence. Absence of state is reported as a structured
/// object, not an error.
#[test]
fn status_no_state_json_is_parseable() {
    let dir = TempDir::new().unwrap();
    let output = ox()
        .args(["status", "--json"])
        .current_dir(dir.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("status --json must emit valid JSON");
    assert_eq!(v["state"], "absent", "got: {stdout}");
    assert!(v["hint"].is_string(), "missing actionable hint: {stdout}");
}

/// Regression test for ox-pabj: `ox status` must sync results.json from the
/// Ray driver *before* reading job counts, so that the very first call shows
/// consistent, up-to-date state instead of stale pending counts.
#[test]
fn status_syncs_results_before_counting() {
    let dir = TempDir::new().unwrap();
    let oxdir = dir.path().join(".oxymake");
    fs::create_dir_all(&oxdir).unwrap();

    // 1. Create state.db with a run and one pending job.
    let db = ox_state::db::StateDb::open(&oxdir.join("state.db")).unwrap();
    db.begin_run("run-001", None, 1, None).unwrap();
    db.record_dag_submission("run-001", "ray", Some("http://127.0.0.1:19999"), 1)
        .unwrap();
    db.register_jobs(&[ox_state::db::JobRecord {
        id: "job-1".into(),
        rule_name: "build".into(),
        wildcards: "{}".into(),
        cache_key: None,
        run_id: Some("run-001".into()),
    }])
    .unwrap();
    // Verify the job starts as pending.
    let counts = db.job_counts_for_run("run-001").unwrap();
    assert_eq!(counts.pending, 1);
    assert_eq!(counts.completed, 0);
    drop(db);

    // 2. Create a run directory with meta.json (Ray executor) and
    //    results.json marking the job as completed.
    let run_dir = oxdir.join("runs/run-001");
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(
        run_dir.join("meta.json"),
        r#"{"executor":"ray","ray_address":"http://127.0.0.1:19999","ray_job_id":"test","active_jobs":1,"skipped_jobs":0,"total_jobs":1}"#,
    )
    .unwrap();
    fs::write(
        run_dir.join("results.json"),
        r#"{"job-1":{"status":"completed","exit_code":0}}"#,
    )
    .unwrap();

    // 3. Run `ox status` — it should show 1 completed (not 1 pending).
    //    The Ray API call will fail (no server), but the results.json sync
    //    should happen before counting.
    ox().current_dir(dir.path())
        .args(["status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("1 completed"))
        .stdout(predicates::str::contains("0 pending"));
}

/// Regression test for #5: a `running` row left behind by an interrupted
/// session must not be reported as live work.  The screen in the issue —
/// "Sessions: 0 active" printed next to "Running: 1 jobs in progress" — is
/// the contradiction this asserts is gone.
#[test]
fn status_reports_an_interrupted_sessions_row_as_orphaned_not_running() {
    use predicates::prelude::PredicateBooleanExt;
    let dir = TempDir::new().unwrap();
    let oxdir = dir.path().join(".oxymake");
    fs::create_dir_all(&oxdir).unwrap();

    let db = ox_state::db::StateDb::open(&oxdir.join("state.db")).unwrap();
    db.begin_run("run-72860", None, 2, None).unwrap();
    db.register_jobs(&[
        ox_state::db::JobRecord {
            id: "substrate_tests".into(),
            rule_name: "substrate_tests".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: Some("run-72860".into()),
        },
        ox_state::db::JobRecord {
            id: "repro_spheres".into(),
            rule_name: "repro_spheres".into(),
            wildcards: "{}".into(),
            cache_key: None,
            run_id: Some("run-72860".into()),
        },
    ])
    .unwrap();
    db.register_edges(&[("substrate_tests".into(), "repro_spheres".into())])
        .unwrap();

    // A session claims the job and is then interrupted, leaving the row
    // at 'running' with nobody executing it.
    let sid = db.create_session(424242, "localhost", None).unwrap();
    db.claim_job("substrate_tests", &sid).unwrap();
    db.interrupt_session(&sid).unwrap();
    assert!(db.active_sessions().unwrap().is_empty());
    // The declared row is still 'running' — the fix is on the read side.
    assert_eq!(
        db.job_status("substrate_tests").unwrap().as_deref(),
        Some("running")
    );
    drop(db);

    ox().current_dir(dir.path())
        .args(["status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Sessions: 0 active"))
        .stdout(predicates::str::contains("0 running"))
        .stdout(predicates::str::contains("1 orphaned"))
        .stdout(predicates::str::contains("substrate_tests"))
        .stdout(predicates::str::contains("owning session was interrupted"))
        // The upstream is named as orphaned on the pending line, not as work
        // in progress.
        .stdout(predicates::str::contains(
            "waiting for: substrate_tests (orphaned)",
        ))
        .stdout(predicates::str::contains("jobs in progress").not());

    // `ox status` is read-only: the row it just reported as orphaned is
    // still 'running' in the ledger — reclaiming stays with `ox run`.
    let db = ox_state::db::StateDb::open(&oxdir.join("state.db")).unwrap();
    assert_eq!(
        db.job_status("substrate_tests").unwrap().as_deref(),
        Some("running")
    );
    drop(db);

    let output = ox()
        .current_dir(dir.path())
        .args(["status", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("status --json must emit valid JSON");
    assert_eq!(json["jobs"]["running"], 0);
    assert_eq!(json["jobs"]["orphaned"], 1);
    assert_eq!(json["sessions"], 0);
    assert!(json["running_jobs"].as_array().unwrap().is_empty());
    let orphans = json["orphaned_jobs"].as_array().unwrap();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0]["reason"], "session_interrupted");
    assert_eq!(orphans[0]["declared_status"], "running");
    assert_eq!(
        json["pending_jobs"][0]["waiting_for_orphaned"][0],
        "substrate_tests"
    );
}

/// A job claimed by a session that is still alive stays `running`: the
/// derivation must not turn healthy work into an orphan.
#[test]
fn status_keeps_a_live_sessions_row_running() {
    use predicates::prelude::PredicateBooleanExt;
    let dir = TempDir::new().unwrap();
    let oxdir = dir.path().join(".oxymake");
    fs::create_dir_all(&oxdir).unwrap();

    let db = ox_state::db::StateDb::open(&oxdir.join("state.db")).unwrap();
    db.begin_run("run-1", None, 1, None).unwrap();
    db.register_jobs(&[ox_state::db::JobRecord {
        id: "job-1".into(),
        rule_name: "build".into(),
        wildcards: "{}".into(),
        cache_key: None,
        run_id: Some("run-1".into()),
    }])
    .unwrap();
    let sid = db
        .create_session(std::process::id(), "localhost", None)
        .unwrap();
    db.claim_job("job-1", &sid).unwrap();
    drop(db);

    ox().current_dir(dir.path())
        .args(["status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Sessions: 1 active"))
        .stdout(predicates::str::contains("1 running"))
        .stdout(predicates::str::contains("1 jobs in progress"))
        .stdout(predicates::str::contains("orphaned").not());
}

#[test]
fn cancel_no_state() {
    ox().args(["cancel"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("No OxyMake state found"));
}

#[test]
fn clean_no_oxymake_dir() {
    ox().args(["clean"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Nothing to clean"));
}

/// `ox clean --state` is the corruption escape hatch: it must delete
/// state.db (and its WAL/SHM sidecars) WITHOUT opening the database,
/// because a corrupt DB cannot be opened.
#[test]
fn clean_state_removes_corrupt_state_db() {
    let dir = tempfile::tempdir().unwrap();
    let oxdir = dir.path().join(".oxymake");
    std::fs::create_dir_all(&oxdir).unwrap();
    std::fs::write(oxdir.join("state.db"), b"garbage, not a sqlite database").unwrap();
    std::fs::write(oxdir.join("state.db-wal"), b"stale wal").unwrap();
    std::fs::write(oxdir.join("state.db-shm"), b"stale shm").unwrap();

    ox().args(["clean", "--state", "--yes"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("state.db"));

    assert!(!oxdir.join("state.db").exists(), "state.db must be removed");
    assert!(
        !oxdir.join("state.db-wal").exists(),
        "stale -wal sidecar must be removed with the main file"
    );
    assert!(
        !oxdir.join("state.db-shm").exists(),
        "stale -shm sidecar must be removed with the main file"
    );

    // The next open regenerates a fully usable DB at the latest schema.
    let db = ox_state::db::StateDb::open(&oxdir.join("state.db")).unwrap();
    assert_eq!(db.schema_version().unwrap(), 10);
}

/// A corrupt state.db hit by the normal clean path (which opens the DB)
/// must point the user at the `ox clean --state` escape hatch.
#[test]
fn clean_corrupt_db_mentions_escape_hatch() {
    let dir = tempfile::tempdir().unwrap();
    let oxdir = dir.path().join(".oxymake");
    std::fs::create_dir_all(&oxdir).unwrap();
    std::fs::write(oxdir.join("state.db"), b"garbage, not a sqlite database").unwrap();

    ox().args(["clean", "--state-only", "--yes"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains("ox clean --state"));
}

#[test]
fn clean_dry_run() {
    let dir = tempfile::tempdir().unwrap();
    let oxdir = dir.path().join(".oxymake");
    std::fs::create_dir_all(oxdir.join("cache")).unwrap();
    std::fs::create_dir_all(oxdir.join("logs")).unwrap();
    std::fs::write(oxdir.join("cache/manifest.json"), "{}").unwrap();
    std::fs::write(oxdir.join("logs/test.log"), "log data").unwrap();

    ox().args(["clean", "--dry-run"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("dry run"));
}

/// H17: `ox clean` must refuse to clear execution state while a live
/// (non-stale) session exists — wiping a running session's jobs makes
/// its audit trail read an empty table mid-flight. `--force` overrides.
#[test]
fn clean_refuses_when_live_session_exists() {
    let dir = tempfile::tempdir().unwrap();
    let oxdir = dir.path().join(".oxymake");
    std::fs::create_dir_all(&oxdir).unwrap();

    // A session with a fresh heartbeat = a run in flight.
    let db = ox_state::db::StateDb::open(&oxdir.join("state.db")).unwrap();
    let sid = db
        .create_session(std::process::id(), "localhost", None)
        .unwrap();
    let jobs = vec![ox_state::db::JobRecord {
        id: "j1".into(),
        rule_name: "build".into(),
        wildcards: "{}".into(),
        cache_key: None,
        run_id: None,
    }];
    db.register_jobs(&jobs).unwrap();
    assert!(db.claim_job("j1", &sid).unwrap());
    db.close().unwrap();

    // Without --force: refuse and leave state intact.
    ox().args(["clean", "--state-only", "--yes"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains("live session"));

    let db = ox_state::db::StateDb::open(&oxdir.join("state.db")).unwrap();
    assert_eq!(
        db.job_status("j1").unwrap().as_deref(),
        Some("running"),
        "live session's job must survive a refused clean"
    );
    db.close().unwrap();

    // With --force: proceed.
    ox().args(["clean", "--state-only", "--yes", "--force"])
        .current_dir(dir.path())
        .assert()
        .success();

    let db = ox_state::db::StateDb::open(&oxdir.join("state.db")).unwrap();
    assert_eq!(db.job_status("j1").unwrap(), None, "--force clears state");
}

/// Regression test for ox-jxdw: cache invalidation must cascade through the DAG.
///
/// Pipeline: step_a → step_b → step_c (linear chain).
/// 1. Run once — all 3 jobs execute.
/// 2. Run again — all 3 are cached (0 to run).
/// 3. Delete step_a's output, run again — step_a re-executes, which should
///    transitively invalidate step_b and step_c even though their outputs
///    still exist on disk from the first run.
#[test]
fn cache_invalidation_cascades_through_dag() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["c.txt"]

[rule.step_a]
output = ["a.txt"]
shell = "echo step_a > a.txt"

[rule.step_b]
input = ["a.txt"]
output = ["b.txt"]
shell = "cat a.txt > b.txt && echo step_b >> b.txt"

[rule.step_c]
input = ["b.txt"]
output = ["c.txt"]
shell = "cat b.txt > c.txt && echo step_c >> c.txt"
"#,
    )
    .unwrap();

    // Run 1: all 3 jobs should execute.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("3 succeeded"));

    // Verify outputs exist.
    assert!(base.join("a.txt").exists());
    assert!(base.join("b.txt").exists());
    assert!(base.join("c.txt").exists());

    // Run 2: all cached.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("up-to-date"));

    // Delete step_a's output to force re-execution.
    fs::remove_file(base.join("a.txt")).unwrap();

    // Run 3: step_a must re-execute, and its downstream (step_b, step_c)
    // must also re-execute even though b.txt and c.txt still exist on disk.
    let output = ox()
        .args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    // All 3 jobs should have re-executed (not just step_a).
    assert!(
        stdout.contains("3 succeeded"),
        "Expected all 3 jobs to re-execute after upstream invalidation, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Selective execution: --until, --omit-from, --touch, --forcerun
// ---------------------------------------------------------------------------

/// Helper: create a 3-step linear workflow (step_a → step_b → step_c) in a
/// temp dir. Returns (dir, oxymakefile_path).
fn create_three_step_workflow() -> (TempDir, String) {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["c.txt"]

[rule.step_a]
output = ["a.txt"]
shell = "echo step_a > a.txt"

[rule.step_b]
input = ["a.txt"]
output = ["b.txt"]
shell = "cat a.txt > b.txt && echo step_b >> b.txt"

[rule.step_c]
input = ["b.txt"]
output = ["c.txt"]
shell = "cat b.txt > c.txt && echo step_c >> c.txt"
"#,
    )
    .unwrap();

    let path = oxymakefile.to_str().unwrap().to_string();
    (dir, path)
}

#[test]
fn run_until_stops_at_target() {
    let (dir, oxymakefile) = create_three_step_workflow();
    let base = dir.path();

    // --until b.txt: should run step_a and step_b, but NOT step_c.
    let output = ox()
        .args(["run", "-f", &oxymakefile, "--until", "b.txt"])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("2 succeeded"),
        "Expected 2 jobs (step_a + step_b), got: {stdout}"
    );
    assert!(base.join("a.txt").exists(), "a.txt should exist");
    assert!(base.join("b.txt").exists(), "b.txt should exist");
    assert!(!base.join("c.txt").exists(), "c.txt should NOT exist");
}

#[test]
fn run_until_dry_run_filters_output() {
    let (dir, oxymakefile) = create_three_step_workflow();
    let base = dir.path();

    // --dry-run --until b.txt: should show 2 jobs, not 3.
    let output = ox()
        .args(["run", "--dry-run", "-f", &oxymakefile, "--until", "b.txt"])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("2 job(s) would execute"),
        "Expected 2 jobs in dry run, got: {stdout}"
    );
}

#[test]
fn run_omit_from_skips_target_and_downstream() {
    let (dir, oxymakefile) = create_three_step_workflow();
    let base = dir.path();

    // --omit-from b.txt: should run step_a only (b and c are excluded).
    let output = ox()
        .args(["run", "-f", &oxymakefile, "--omit-from", "b.txt"])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("1 succeeded"),
        "Expected 1 job (step_a only), got: {stdout}"
    );
    assert!(base.join("a.txt").exists(), "a.txt should exist");
    assert!(!base.join("b.txt").exists(), "b.txt should NOT exist");
    assert!(!base.join("c.txt").exists(), "c.txt should NOT exist");
}

#[test]
fn run_touch_creates_outputs_without_executing() {
    let (dir, oxymakefile) = create_three_step_workflow();
    let base = dir.path();

    // --touch: should create output files but not actually run shell commands.
    let output = ox()
        .args(["run", "-f", &oxymakefile, "--touch"])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("Touched 3 job output(s)"),
        "Expected 3 touched outputs, got: {stdout}"
    );
    // Files should exist but be empty (touched, not computed).
    assert!(base.join("a.txt").exists(), "a.txt should exist");
    assert!(base.join("b.txt").exists(), "b.txt should exist");
    assert!(base.join("c.txt").exists(), "c.txt should exist");
    assert_eq!(
        fs::read_to_string(base.join("a.txt")).unwrap(),
        "",
        "a.txt should be empty (touched, not executed)"
    );
}

#[test]
fn run_forcerun_bypasses_cache() {
    let (dir, oxymakefile) = create_three_step_workflow();
    let base = dir.path();

    // First run: all 3 jobs execute and get cached.
    ox().args(["run", "-f", &oxymakefile])
        .current_dir(base)
        .assert()
        .success();

    // Second run without forcerun: all cached.
    let output = ox()
        .args(["run", "-f", &oxymakefile])
        .current_dir(base)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("3 of 3 job(s) up-to-date"),
        "Expected all cached on second run, got: {stdout}"
    );

    // Third run with --forcerun step_b: step_b and step_c should re-execute.
    let output = ox()
        .args(["run", "-f", &oxymakefile, "--forcerun", "step_b"])
        .current_dir(base)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("2 succeeded"),
        "Expected 2 re-executed (step_b + step_c), got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Executor: ray
// ---------------------------------------------------------------------------

/// Regression test for ox-d86d: `--executor ray` should be recognised (not
/// "unknown executor"). It will fail because no Ray dashboard is running, but
/// the error must come from Ray init, not from the executor dispatcher.
#[test]
fn executor_ray_is_recognised() {
    let output = ox()
        .args([
            "run",
            "-f",
            &simple_fixture(),
            "--executor",
            "ray",
            "--ray-address",
            "http://127.0.0.1:19999",
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    assert!(
        !stderr.contains("unknown executor"),
        "ray executor should be wired, but got: {stderr}"
    );
}

// ===========================================================================
// Extended QA suite: wildcard resolution, config, multi-executor, cache,
// error recovery, concurrent execution (ox-xaog)
// ===========================================================================

// ---------------------------------------------------------------------------
// Wildcard resolution edge cases
// ---------------------------------------------------------------------------

/// Multiple wildcards with Cartesian product expansion.
#[test]
fn wildcard_cartesian_product_two_wildcards() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    let data_dir = base.join("data");
    let results_dir = base.join("results");
    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&results_dir).unwrap();

    // Create source files for 2 samples × 2 methods = 4 combinations.
    for s in &["X", "Y"] {
        for m in &["fast", "slow"] {
            fs::write(data_dir.join(format!("{s}_{m}.csv")), "data").unwrap();
        }
    }

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["X", "Y"]
method = ["fast", "slow"]

[rule.all]
input = ["results/{sample}_{method}.txt"]

[rule.process]
input = ["data/{sample}_{method}.csv"]
output = ["results/{sample}_{method}.txt"]
shell = "cp data/{sample}_{method}.csv results/{sample}_{method}.txt"
"#,
    )
    .unwrap();

    let output = ox()
        .args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("4 succeeded"),
        "Expected 4 jobs (2×2 product), got: {stdout}"
    );
}

/// Zip expansion mode — lists must be equal length, combined 1:1.
#[test]
fn wildcard_zip_expansion() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    let data_dir = base.join("data");
    let results_dir = base.join("results");
    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&results_dir).unwrap();

    // Zip: sample=["A","B","C"] × tag=["x","y","z"] → 3 jobs (not 9).
    fs::write(data_dir.join("A_x.csv"), "d").unwrap();
    fs::write(data_dir.join("B_y.csv"), "d").unwrap();
    fs::write(data_dir.join("C_z.csv"), "d").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A", "B", "C"]
tag = ["x", "y", "z"]

[rule.process]
input = ["data/{sample}_{tag}.csv"]
output = ["results/{sample}_{tag}.txt"]
expand = "zip"
shell = "cp data/{sample}_{tag}.csv results/{sample}_{tag}.txt"
"#,
    )
    .unwrap();

    // Run targeting all process outputs explicitly via the zip-expanded outputs.
    let output = ox()
        .args([
            "run",
            "-f",
            oxymakefile.to_str().unwrap(),
            "results/A_x.txt",
            "results/B_y.txt",
            "results/C_z.txt",
        ])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("3 succeeded"),
        "Expected 3 jobs (zip, not product), got: {stdout}"
    );

    // Verify only the zipped pairs exist, not cross-products.
    assert!(base.join("results/A_x.txt").exists());
    assert!(base.join("results/B_y.txt").exists());
    assert!(base.join("results/C_z.txt").exists());
    assert!(!base.join("results/A_y.txt").exists());
}

/// Single sample — degenerate case where config has only one element.
#[test]
fn wildcard_single_sample() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::write(base.join("data/only.csv"), "d").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["only"]

[rule.all]
input = ["out/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["out/{sample}.txt"]
shell = "cp data/{sample}.csv out/{sample}.txt"
"#,
    )
    .unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("1 succeeded"));

    assert!(base.join("out/only.txt").exists());
}

/// Empty config list results in nothing to do.
#[test]
fn wildcard_empty_config_list() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = []

[rule.all]
input = ["out/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["out/{sample}.txt"]
shell = "cp data/{sample}.csv out/{sample}.txt"
"#,
    )
    .unwrap();

    // Empty config list means no targets — nothing to do.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("Nothing to do"));
}

// ---------------------------------------------------------------------------
// Config: profiles
// ---------------------------------------------------------------------------

/// Profile applies --no-cache flag from the Oxymakefile.
#[test]
fn profile_applies_no_cache() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[profile.fresh]
no_cache = true

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    // First run: executes.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("1 succeeded"));

    // Second run without profile: cached.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("up-to-date"));

    // Third run with --profile fresh: should re-execute (no cache).
    ox().args([
        "run",
        "-f",
        oxymakefile.to_str().unwrap(),
        "--profile",
        "fresh",
    ])
    .current_dir(base)
    .assert()
    .success()
    .stdout(predicates::str::contains("1 succeeded"));
}

// ---------------------------------------------------------------------------
// Config: --set overrides
// ---------------------------------------------------------------------------

/// `--set` overrides a config list — explicit target bypasses aggregation rule.
#[test]
fn set_overrides_config_list() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::write(base.join("data/Z.csv"), "z_data").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A", "B"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["out/{sample}.txt"]
shell = "cp data/{sample}.csv out/{sample}.txt"
"#,
    )
    .unwrap();

    // With --set sample=Z and explicit target: resolve using overridden config.
    let output = ox()
        .args([
            "run",
            "--dry-run",
            "-f",
            oxymakefile.to_str().unwrap(),
            "--set",
            "sample=Z",
            "out/Z.txt",
        ])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("1 job(s) would execute"),
        "Expected 1 job for explicit target with --set, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Multiple samples in a diamond DAG
// ---------------------------------------------------------------------------

/// Diamond DAG: two intermediate rules converge on a final target.
#[test]
fn diamond_dag_converges_correctly() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["merged.txt"]

[rule.source]
output = ["source.txt"]
shell = "echo source > source.txt"

[rule.left]
input = ["source.txt"]
output = ["left.txt"]
shell = "cat source.txt > left.txt && echo left >> left.txt"

[rule.right]
input = ["source.txt"]
output = ["right.txt"]
shell = "cat source.txt > right.txt && echo right >> right.txt"

[rule.merge]
input = ["left.txt", "right.txt"]
output = ["merged.txt"]
shell = "cat left.txt right.txt > merged.txt"
"#,
    )
    .unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("4 succeeded"));

    let content = fs::read_to_string(base.join("merged.txt")).unwrap();
    assert!(
        content.contains("left"),
        "merged.txt should contain left branch"
    );
    assert!(
        content.contains("right"),
        "merged.txt should contain right branch"
    );
}

// ---------------------------------------------------------------------------
// Cache behavior
// ---------------------------------------------------------------------------

/// Cache hit on second run: all jobs are cached and reported as up-to-date.
#[test]
fn cache_hit_second_run_all_cached() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::write(base.join("data/A.csv"), "data_a").unwrap();
    fs::write(base.join("data/B.csv"), "data_b").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A", "B"]

[rule.all]
input = ["out/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["out/{sample}.txt"]
shell = "cp data/{sample}.csv out/{sample}.txt"
"#,
    )
    .unwrap();

    // First run: 2 jobs execute.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("2 succeeded"));

    // Second run: all cached.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("up-to-date"));
}

// ---------------------------------------------------------------------------
// Rule-name filtering: --rule
// ---------------------------------------------------------------------------

/// `--rule` filter restricts execution to matching rule names.
#[test]
fn rule_name_filter() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::write(base.join("data/A.csv"), "d").unwrap();
    fs::write(base.join("data/B.csv"), "d").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A", "B"]

[rule.all]
input = ["out/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["out/{sample}.txt"]
shell = "cp data/{sample}.csv out/{sample}.txt"
"#,
    )
    .unwrap();

    // --rule process should show 2 jobs (both samples).
    let output = ox()
        .args([
            "run",
            "--dry-run",
            "-f",
            oxymakefile.to_str().unwrap(),
            "--rule",
            "process",
        ])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    assert!(
        stdout.contains("2 job(s) would execute"),
        "Expected 2 jobs for --rule process, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Cache validation strategies
// ---------------------------------------------------------------------------

/// `--cache-validation hash` forces hash-based validation.
#[test]
fn cache_validation_hash_strategy() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    // Run with hash validation.
    ox().args([
        "run",
        "-f",
        oxymakefile.to_str().unwrap(),
        "--cache-validation",
        "hash",
    ])
    .current_dir(base)
    .assert()
    .success()
    .stdout(predicates::str::contains("1 succeeded"));

    // Second run should cache even with hash validation.
    ox().args([
        "run",
        "-f",
        oxymakefile.to_str().unwrap(),
        "--cache-validation",
        "hash",
    ])
    .current_dir(base)
    .assert()
    .success()
    .stdout(predicates::str::contains("up-to-date"));
}

/// `--cache-validation mtime+hash` hybrid strategy.
#[test]
fn cache_validation_mtime_plus_hash() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    ox().args([
        "run",
        "-f",
        oxymakefile.to_str().unwrap(),
        "--cache-validation",
        "mtime+hash",
    ])
    .current_dir(base)
    .assert()
    .success()
    .stdout(predicates::str::contains("1 succeeded"));

    ox().args([
        "run",
        "-f",
        oxymakefile.to_str().unwrap(),
        "--cache-validation",
        "mtime+hash",
    ])
    .current_dir(base)
    .assert()
    .success()
    .stdout(predicates::str::contains("up-to-date"));
}

// ---------------------------------------------------------------------------
// Error recovery: --keep-going
// ---------------------------------------------------------------------------

/// `--keep-going` continues execution on independent branches after a failure.
#[test]
fn keep_going_continues_independent_branches() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    // Two independent branches: failing_branch and ok_branch.
    // With --keep-going, ok_branch should still execute.
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["fail.txt", "ok.txt"]

[rule.failing_step]
output = ["fail.txt"]
shell = "exit 1"

[rule.ok_step]
output = ["ok.txt"]
shell = "echo ok > ok.txt"
"#,
    )
    .unwrap();

    let output = ox()
        .args(["run", "-k", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .failure();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    // ok_step should still have run despite failing_step.
    assert!(
        stdout.contains("1 succeeded") || stdout.contains("1 failed"),
        "Expected keep-going to run independent branches, got: {stdout}"
    );
    assert!(
        base.join("ok.txt").exists(),
        "ok.txt should exist — ok_step should run despite failing_step"
    );
}

/// Without --keep-going, failure in one branch may prevent reporting.
#[test]
fn no_keep_going_fails_on_first_error() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "exit 1"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .failure()
        .stdout(predicates::str::contains("1 failed"));
}

// ---------------------------------------------------------------------------
// Concurrent execution: -j N
// ---------------------------------------------------------------------------

/// `-j 4` runs multiple independent jobs concurrently.
#[test]
fn concurrent_execution_j4() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    // Four independent jobs — with -j 4 they should all run concurrently.
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["a.txt", "b.txt", "c.txt", "d.txt"]

[rule.job_a]
output = ["a.txt"]
shell = "echo a > a.txt"

[rule.job_b]
output = ["b.txt"]
shell = "echo b > b.txt"

[rule.job_c]
output = ["c.txt"]
shell = "echo c > c.txt"

[rule.job_d]
output = ["d.txt"]
shell = "echo d > d.txt"
"#,
    )
    .unwrap();

    ox().args(["run", "-j", "4", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("4 succeeded"));

    for f in &["a.txt", "b.txt", "c.txt", "d.txt"] {
        assert!(base.join(f).exists(), "{f} should exist");
    }
}

/// `-j 2` limits concurrency to 2 on a 4-job independent DAG.
#[test]
fn concurrent_execution_j2_limits_parallelism() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["a.txt", "b.txt", "c.txt", "d.txt"]

[rule.job_a]
output = ["a.txt"]
shell = "echo a > a.txt"

[rule.job_b]
output = ["b.txt"]
shell = "echo b > b.txt"

[rule.job_c]
output = ["c.txt"]
shell = "echo c > c.txt"

[rule.job_d]
output = ["d.txt"]
shell = "echo d > d.txt"
"#,
    )
    .unwrap();

    // -j 2: still completes all 4, but serializes into 2 waves.
    ox().args(["run", "-j", "2", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("4 succeeded"));
}

// ---------------------------------------------------------------------------
// Explain command
// ---------------------------------------------------------------------------

/// `explain` shows the dependency chain for a target.
#[test]
fn explain_shows_dependency_chain() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::write(base.join("data/A.csv"), "d").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "cp data/{sample}.csv results/{sample}.txt"
"#,
    )
    .unwrap();

    ox().args([
        "explain",
        "results/A.txt",
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(base)
    .assert()
    .success()
    .stdout(predicates::str::contains("process"));
}

/// `explain --json` outputs JSON format.
#[test]
fn explain_json_output() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::write(base.join("data/A.csv"), "d").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "cp data/{sample}.csv results/{sample}.txt"
"#,
    )
    .unwrap();

    let output = ox()
        .args([
            "explain",
            "--json",
            "results/A.txt",
            "-f",
            oxymakefile.to_str().unwrap(),
        ])
        .current_dir(base)
        .output()
        .expect("command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("explain --json should produce valid JSON");
}

/// A missing intermediate must not truncate `explain` at an existing final output.
#[test]
fn explain_resolves_full_chain_when_intermediate_output_is_missing() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();
    let oxymakefile = write_two_rule_workflow(base);
    assert!(run_two_rule_workflow(base, &oxymakefile).contains("2 succeeded"));

    fs::remove_file(base.join("a.txt")).unwrap();
    let output = ox()
        .args([
            "explain",
            "b.txt",
            "--json",
            "-f",
            oxymakefile.to_str().unwrap(),
        ])
        .current_dir(base)
        .output()
        .expect("explain should run");
    assert!(output.status.success(), "explain failed: {output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let rules: Vec<_> = json["chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|job| job["rule"].as_str().unwrap())
        .collect();
    assert_eq!(rules, ["b", "a"]);

    let run = run_two_rule_workflow(base, &oxymakefile);
    assert!(run.contains("2 succeeded"), "unexpected run output: {run}");
}

// ---------------------------------------------------------------------------
// Query command
// ---------------------------------------------------------------------------

/// `query deps(X)` shows dependencies of a rule.
#[test]
fn query_deps_shows_dependencies() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    // Use a static DAG (no wildcards) so deps resolve deterministically.
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["c.txt"]

[rule.step_a]
output = ["a.txt"]
shell = "echo a > a.txt"

[rule.step_b]
input = ["a.txt"]
output = ["b.txt"]
shell = "cat a.txt > b.txt"

[rule.step_c]
input = ["b.txt"]
output = ["c.txt"]
shell = "cat b.txt > c.txt"
"#,
    )
    .unwrap();

    // Query deps of c.txt — should include b.txt (and transitively a.txt).
    ox().args(["query", "deps(c.txt)", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("b.txt"));
}

// ---------------------------------------------------------------------------
// DAG visualization
// ---------------------------------------------------------------------------

/// `dag` command produces output.
#[test]
fn dag_produces_output() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::write(base.join("data/A.csv"), "d").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "cp data/{sample}.csv results/{sample}.txt"
"#,
    )
    .unwrap();

    ox().args(["dag", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// Multiple rules in a chain with wildcards
// ---------------------------------------------------------------------------

/// Multi-step pipeline with wildcards flows correctly through the DAG.
#[test]
fn multi_step_wildcard_pipeline() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");
    fs::create_dir_all(base.join("raw")).unwrap();
    fs::write(base.join("raw/A.dat"), "raw_a").unwrap();
    fs::write(base.join("raw/B.dat"), "raw_b").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
sample = ["A", "B"]

[rule.all]
input = ["final/{sample}.out"]

[rule.clean]
input = ["raw/{sample}.dat"]
output = ["cleaned/{sample}.dat"]
shell = "mkdir -p cleaned && cp raw/{sample}.dat cleaned/{sample}.dat"

[rule.transform]
input = ["cleaned/{sample}.dat"]
output = ["final/{sample}.out"]
shell = "mkdir -p final && cp cleaned/{sample}.dat final/{sample}.out"
"#,
    )
    .unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success()
        .stdout(predicates::str::contains("4 succeeded"));

    assert!(base.join("final/A.out").exists());
    assert!(base.join("final/B.out").exists());
}

// ---------------------------------------------------------------------------
// Verbose output
// ---------------------------------------------------------------------------

/// `-v` flag enables verbose output showing job details.
#[test]
fn verbose_output_shows_job_details() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    let output = ox()
        .args(["run", "-v", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    // Verbose mode should show job execution details on stderr.
    assert!(
        stderr.contains("step_a") || stderr.contains("exit"),
        "Verbose mode should show job details on stderr, got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Timings
// ---------------------------------------------------------------------------

/// `--timings` flag shows per-phase timing breakdown.
#[test]
fn timings_flag_shows_breakdown() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.step_a]
output = ["a.txt"]
shell = "echo hello > a.txt"

[rule.all]
input = ["a.txt"]
"#,
    )
    .unwrap();

    let output = ox()
        .args(["run", "--timings", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    // --timings should produce timing information on stderr.
    assert!(
        stderr.contains("ms") || stderr.contains("µs") || stderr.contains("s"),
        "Timings should show duration info on stderr, got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Forcerun with regex pattern
// ---------------------------------------------------------------------------

/// `--forcerun /regex/` matches rule names by regex.
#[test]
fn forcerun_regex_pattern() {
    let dir = TempDir::new().unwrap();
    let base = dir.path();

    let oxymakefile = base.join("Oxymakefile.toml");

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["c.txt"]

[rule.step_a]
output = ["a.txt"]
shell = "echo step_a > a.txt"

[rule.step_b]
input = ["a.txt"]
output = ["b.txt"]
shell = "cat a.txt > b.txt && echo step_b >> b.txt"

[rule.step_c]
input = ["b.txt"]
output = ["c.txt"]
shell = "cat b.txt > c.txt && echo step_c >> c.txt"
"#,
    )
    .unwrap();

    // First run.
    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(base)
        .assert()
        .success();

    // Forcerun with regex matching step_a and step_b.
    let output = ox()
        .args([
            "run",
            "-f",
            oxymakefile.to_str().unwrap(),
            "--forcerun",
            "/step_[ab]/",
        ])
        .current_dir(base)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    // step_a, step_b forced + step_c cascades = 3 re-executed.
    assert!(
        stdout.contains("3 succeeded"),
        "Expected 3 re-executed with regex forcerun, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// translate — default surface (no -o) writes files and prints summary
// ---------------------------------------------------------------------------

#[test]
fn translate_default_writes_translated_file_next_to_input() {
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("Snakefile");
    fs::write(
        &input,
        r#"rule process:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.txt"
    shell:
        "sort {input} > {output}"
"#,
    )
    .unwrap();

    ox().args(["translate", input.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicates::str::contains("translated: 1 rules"))
        .stderr(predicates::str::contains("includes: 0 files NOT followed"));

    let expected = tmp.path().join("Snakefile.translated.toml");
    assert!(
        expected.exists(),
        "expected {} to exist",
        expected.display()
    );
    let body = fs::read_to_string(&expected).unwrap();
    assert!(body.contains("[rule.process]"), "body: {body}");

    let esc = tmp
        .path()
        .join("Snakefile.translated.toml.escalations.toml");
    assert!(
        !esc.exists(),
        "no escalations expected — {} should not exist",
        esc.display()
    );
}

#[test]
fn translate_default_exits_non_zero_when_escalations_present() {
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("Snakefile");
    fs::write(
        &input,
        r#"report: "report.html"

rule process:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.txt"
    shell:
        "sort {input} > {output}"
"#,
    )
    .unwrap();

    let output = ox()
        .args(["translate", input.to_str().unwrap()])
        .assert()
        .code(2);

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    assert!(stderr.contains("translated: 1 rules"), "stderr: {stderr}");
    assert!(
        stderr.contains("escalation"),
        "expected escalation hint in stderr, got: {stderr}"
    );

    let esc = tmp
        .path()
        .join("Snakefile.translated.toml.escalations.toml");
    assert!(
        esc.exists(),
        "escalation file should be written even on non-zero exit"
    );
}

#[test]
fn translate_counts_unsupported_top_level_constructs() {
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("Snakefile");
    fs::write(
        &input,
        r#"from os import path
import sys

rule process:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.txt"
    shell:
        "sort {input} > {output}"
"#,
    )
    .unwrap();

    let output = ox()
        .args(["translate", input.to_str().unwrap()])
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    assert!(
        stderr.contains("dropped: 2 unsupported top-level constructs"),
        "stderr: {stderr}"
    );
}

#[test]
fn translate_counts_includes_not_followed() {
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("Snakefile");
    fs::write(
        &input,
        r#"include: "common.smk"
include: "utils.smk"

rule process:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.txt"
    shell:
        "sort {input} > {output}"
"#,
    )
    .unwrap();

    let output = ox()
        .args(["translate", input.to_str().unwrap()])
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    assert!(
        stderr.contains("includes: 2 files NOT followed"),
        "stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// translate — unsupported constructs and --lossy
// ---------------------------------------------------------------------------

#[test]
fn translate_checkpoint_fails_without_lossy() {
    let dir = TempDir::new().unwrap();
    let snakefile = dir.path().join("Snakefile");
    fs::write(
        &snakefile,
        "\ncheckpoint split:\n    output:\n        \"x.txt\"\n    shell:\n        \"echo\"\n",
    )
    .unwrap();

    ox().args(["translate", snakefile.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "unsupported construct 'checkpoint'",
        ))
        .stderr(predicates::str::contains("use --lossy to opt out"));

    // No output file should be written when translation is rejected.
    let translated = dir.path().join("Snakefile.translated.toml");
    assert!(
        !translated.exists(),
        "rejected Snakefile should not produce {}",
        translated.display()
    );
}

#[test]
fn translate_checkpoint_succeeds_with_lossy() {
    let dir = TempDir::new().unwrap();
    let snakefile = dir.path().join("Snakefile");
    fs::write(
        &snakefile,
        "\ncheckpoint split:\n    output:\n        \"x.txt\"\n    shell:\n        \"echo\"\n",
    )
    .unwrap();

    ox().args(["translate", "--lossy", snakefile.to_str().unwrap()])
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// run — -j 0 must be rejected at parse time (H27)
// ---------------------------------------------------------------------------

#[test]
fn run_rejects_zero_jobs() {
    // `-j 0` used to be accepted and hung forever: the scheduler's
    // semaphore never issued a single permit. It must be a clap-level
    // parse error, before any scheduling starts.
    ox().args(["run", "-j", "0"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("invalid value '0'"));
}

// ---------------------------------------------------------------------------
// run --report-json (H28 — documented stable in STATUS.md, must exist)
// ---------------------------------------------------------------------------

#[test]
fn run_report_json_writes_ndjson_file() {
    let dir = TempDir::new().unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "hello\n").unwrap();

    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
samples = ["A"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "mkdir -p results && cp data/{sample}.csv results/{sample}.txt"
"#,
    )
    .unwrap();

    let report = dir.path().join("report.ndjson");
    ox().args([
        "run",
        "--report-json",
        report.to_str().unwrap(),
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(dir.path())
    .assert()
    .success();

    let content = fs::read_to_string(&report).expect("--report-json must write the file");
    assert!(!content.is_empty(), "report file must not be empty");
    for line in content.lines() {
        let v: serde_json::Value =
            serde_json::from_str(line).expect("each line must be valid JSON");
        assert!(v.get("event").is_some(), "each event has a discriminator");
    }
    assert!(
        content.lines().any(|l| l.contains("run_started")),
        "report must contain run_started"
    );
}

#[test]
fn run_report_json_unwritable_path_fails_cleanly() {
    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["out.txt"]

[rule.make]
output = ["out.txt"]
shell = "touch out.txt"
"#,
    )
    .unwrap();

    ox().args([
        "run",
        "--report-json",
        "/nonexistent-dir/report.ndjson",
        "-f",
        oxymakefile.to_str().unwrap(),
    ])
    .current_dir(dir.path())
    .assert()
    .failure()
    .stderr(predicates::str::contains("failed to create report file"));
}

// ---------------------------------------------------------------------------
// include expansion end-to-end (H29)
// ---------------------------------------------------------------------------

#[test]
fn run_executes_rule_from_included_file() {
    let dir = TempDir::new().unwrap();

    fs::write(
        dir.path().join("lib.toml"),
        r#"
[rule.copy]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "mkdir -p results && cp data/{sample}.csv results/{sample}.txt"
"#,
    )
    .unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"
include = ["lib.toml"]

[config]
samples = ["A"]

[rule.all]
input = ["results/{sample}.txt"]
"#,
    )
    .unwrap();

    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join("A.csv"), "hello\n").unwrap();

    ox().args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(
        dir.path().join("results/A.txt").exists(),
        "rule from included file must execute"
    );
}

// ---------------------------------------------------------------------------
// SIGTERM graceful shutdown (B8 — `ox cancel` sends SIGTERM to `ox run`)
// ---------------------------------------------------------------------------

/// `ox run` must handle SIGTERM on the same graceful path as SIGINT:
/// print the interrupt message, cancel children, and exit — instead of
/// dying on the default disposition and orphaning job subprocesses (B8).
#[test]
#[cfg(unix)]
fn run_handles_sigterm_gracefully() {
    use std::process::{Command as StdCommand, Stdio};
    use std::time::{Duration, Instant};

    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["out.txt"]

[rule.slow]
output = ["out.txt"]
shell = "touch started.txt && sleep 30 && touch out.txt"
"#,
    )
    .unwrap();

    let mut child = StdCommand::new(env!("CARGO_BIN_EXE_oxymake"))
        .args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ox run");

    // Wait until the slow job has actually started (sentinel file). The
    // signal handler is installed before jobs dispatch, so this guarantees
    // SIGTERM arrives after installation rather than during startup.
    let sentinel = dir.path().join("started.txt");
    let start_deadline = Instant::now() + Duration::from_secs(30);
    while !sentinel.exists() {
        if let Some(status) = child.try_wait().expect("try_wait") {
            panic!("ox run exited before the job started: {status:?}");
        }
        if Instant::now() > start_deadline {
            let _ = child.kill();
            panic!("slow job did not start within 30 s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Send SIGTERM — what `ox cancel` does.
    StdCommand::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("send SIGTERM");

    // The process must exit on its own well before the 30 s job would end.
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("ox run did not exit within 15 s of SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    // Graceful path: the interrupt message was printed (the default
    // SIGTERM disposition kills the process before any output).
    let mut stderr = String::new();
    use std::io::Read;
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        stderr.contains("Interrupted"),
        "SIGTERM must take the graceful shutdown path; stderr: {stderr:?}, status: {status:?}"
    );
}

// ---------------------------------------------------------------------------
// Bounded interruption (#4 — the ledger never keeps a `running` row behind)
// ---------------------------------------------------------------------------

/// Read `(id, status)` for every job row of `session_id` in a state DB.
#[cfg(unix)]
fn jobs_of_session(db_path: &std::path::Path, session_id: &str) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(db_path).expect("open state.db");
    let mut stmt = conn
        .prepare("SELECT id, status FROM jobs WHERE session_id = ?1")
        .expect("prepare");
    let rows = stmt
        .query_map([session_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .expect("query");
    rows.map(|r| r.expect("row")).collect()
}

/// The single session recorded in a state DB (this fixture only ever runs one).
#[cfg(unix)]
fn only_session(db_path: &std::path::Path) -> (String, String) {
    let conn = rusqlite::Connection::open(db_path).expect("open state.db");
    let mut stmt = conn
        .prepare("SELECT id, status FROM sessions")
        .expect("prepare");
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .map(|r| r.expect("row"))
        .collect();
    assert_eq!(rows.len(), 1, "fixture must record exactly one session");
    rows.into_iter().next().unwrap()
}

/// Spawn `ox run` on a job that ignores SIGTERM and wait until it is live.
#[cfg(unix)]
fn spawn_sigterm_deaf_run(
    dir: &std::path::Path,
    grace_secs: &str,
) -> (std::process::Child, std::path::PathBuf) {
    use std::process::{Command as StdCommand, Stdio};
    use std::time::{Duration, Instant};

    let oxymakefile = dir.join("Oxymakefile.toml");
    // The trap makes the job's shell deaf to SIGTERM: it records the signal
    // in `term-seen.txt` and keeps looping; the inner sleeps die on the group
    // signal but the shell (and the process group) stays alive, so only
    // SIGKILL ends it. `job.pid` lets the test check the process is gone.
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["out.txt"]

[rule.deaf]
output = ["out.txt"]
shell = "echo $$ > job.pid; trap 'touch term-seen.txt' TERM; touch started.txt; while true; do sleep 0.2; done"
"#,
    )
    .unwrap();

    let child = StdCommand::new(env!("CARGO_BIN_EXE_oxymake"))
        .args(["run", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir)
        .env("OX_SHUTDOWN_GRACE_SECS", grace_secs)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ox run");

    let sentinel = dir.join("started.txt");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !sentinel.exists() {
        if Instant::now() > deadline {
            panic!("deaf job did not start within 30 s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // The signal handler is installed before dispatch; give the scheduler a
    // moment to reach its select! loop.
    std::thread::sleep(Duration::from_millis(200));

    (child, dir.join(".oxymake/state.db"))
}

/// `kill -0`: true while a process with this PID exists (zombies included,
/// which is why callers poll — the parent `ox run` is gone and init reaps).
#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(unix)]
fn wait_for_exit(child: &mut std::process::Child, secs: u64) -> std::process::ExitStatus {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("ox run did not exit within {secs} s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A single SIGINT on a job that ignores SIGTERM must still terminate within
/// the shutdown grace period: the child is SIGKILLed, its row lands on
/// `cancelled`, the session stays `interrupted`, and nothing is left
/// `running` (#4, hole 2).
#[test]
#[cfg(unix)]
fn interrupt_escalates_to_sigkill_and_leaves_no_running_row() {
    use std::process::Command as StdCommand;

    use std::time::{Duration, Instant};

    let dir = TempDir::new().unwrap();
    let (mut child, db_path) = spawn_sigterm_deaf_run(dir.path(), "2");
    let job_pid: u32 = fs::read_to_string(dir.path().join("job.pid"))
        .expect("job.pid")
        .trim()
        .parse()
        .expect("job pid");

    let signalled_at = Instant::now();
    StdCommand::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send SIGINT");

    // Grace is 2 s; allow generous margin for CI scheduling.
    wait_for_exit(&mut child, 40);
    let elapsed = signalled_at.elapsed();

    // The run must have waited out the grace period before killing: exiting
    // earlier means the child was never escalated and is leaked. (A lower
    // bound with a little slack for timer granularity.)
    assert!(
        elapsed >= Duration::from_millis(1800),
        "ox run exited {elapsed:?} after SIGINT, before the 2 s grace elapsed — no escalation happened"
    );
    // SIGTERM was delivered first (the trap ran), then SIGKILL ended the shell.
    assert!(
        dir.path().join("term-seen.txt").exists(),
        "the job never saw SIGTERM: graceful cancellation did not reach it"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_alive(job_pid) {
        assert!(
            Instant::now() < deadline,
            "job process {job_pid} is still alive after ox run exited: it was not SIGKILLed"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let (session_id, session_status) = only_session(&db_path);
    assert_eq!(
        session_status, "interrupted",
        "a signalled session must stay 'interrupted'"
    );

    let jobs = jobs_of_session(&db_path, &session_id);
    assert!(
        !jobs.iter().any(|(_, st)| st == "running"),
        "no row of the interrupted session may stay 'running': {jobs:?}"
    );
    assert!(
        jobs.iter()
            .any(|(id, st)| id == "deaf" && st == "cancelled"),
        "the SIGKILLed job must be recorded 'cancelled': {jobs:?}"
    );
}

/// Two quick signals force-exit with 130 — and the force-exit path must
/// terminalize this session's rows before it calls `process::exit` (#4,
/// hole 1).
#[test]
#[cfg(unix)]
fn double_interrupt_force_exits_without_leaving_running_rows() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command as StdCommand;
    use std::time::Duration;

    let dir = TempDir::new().unwrap();
    // A long grace keeps the run parked in graceful shutdown, so the second
    // signal is what ends it.
    let (mut child, db_path) = spawn_sigterm_deaf_run(dir.path(), "600");

    let pid = child.id().to_string();
    StdCommand::new("kill")
        .args(["-INT", &pid])
        .status()
        .expect("send first SIGINT");
    std::thread::sleep(Duration::from_millis(500));
    StdCommand::new("kill")
        .args(["-INT", &pid])
        .status()
        .expect("send second SIGINT");

    let status = wait_for_exit(&mut child, 30);
    assert_eq!(
        status.code(),
        Some(130),
        "a second signal must force-exit with 130 (signal: {:?})",
        status.signal()
    );

    let (session_id, session_status) = only_session(&db_path);
    assert_eq!(session_status, "interrupted");

    let jobs = jobs_of_session(&db_path, &session_id);
    assert!(
        !jobs.iter().any(|(_, st)| st == "running"),
        "force-exit must not leave a 'running' row behind: {jobs:?}"
    );

    // The orphaned job's process group is not this test's business (the
    // force-exit path is deliberately ledger-only), but leave nothing behind.
    if let Ok(pid) = fs::read_to_string(dir.path().join("job.pid")) {
        let _ = StdCommand::new("kill").args(["-KILL", pid.trim()]).status();
    }
}

/// After a job fails (no `--keep-going`) the run stops dispatching and lets
/// the jobs already running finish. A Ctrl+C in that phase must still cancel
/// them with the bounded shutdown — it used to be ignored until the force-exit,
/// leaving a SIGTERM-deaf sibling alive and its row `running` (#4, QA round 2).
#[test]
#[cfg(unix)]
fn interrupt_during_post_failure_wait_cancels_running_siblings() {
    use std::process::{Command as StdCommand, Stdio};
    use std::time::{Duration, Instant};

    let dir = TempDir::new().unwrap();
    let oxymakefile = dir.path().join("Oxymakefile.toml");
    fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[rule.all]
input = ["fail.txt", "deaf.txt"]

[rule.fail]
output = ["fail.txt"]
shell = "touch fail-started.txt; sleep 1; exit 1"

[rule.deaf]
output = ["deaf.txt"]
shell = "echo $$ > job.pid; trap 'touch term-seen.txt' TERM; touch started.txt; while true; do sleep 0.2; done"
"#,
    )
    .unwrap();

    let mut child = StdCommand::new(env!("CARGO_BIN_EXE_oxymake"))
        .args(["run", "-j", "2", "-f", oxymakefile.to_str().unwrap()])
        .current_dir(dir.path())
        .env("OX_SHUTDOWN_GRACE_SECS", "2")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ox run");

    let db_path = dir.path().join(".oxymake/state.db");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !dir.path().join("started.txt").exists() {
        assert!(
            Instant::now() < deadline,
            "deaf job did not start within 30 s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let job_pid: u32 = fs::read_to_string(dir.path().join("job.pid"))
        .expect("job.pid")
        .trim()
        .parse()
        .expect("job pid");

    // Wait until `fail` has failed and the run is in its post-failure wait.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let conn = rusqlite::Connection::open(&db_path).expect("open state.db");
        let failed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE id = 'fail' AND status = 'failed'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if failed == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fail job did not fail within 30 s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "the run must still be waiting on the running sibling"
    );

    let signalled_at = Instant::now();
    StdCommand::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send SIGINT");
    let status = wait_for_exit(&mut child, 40);
    let elapsed = signalled_at.elapsed();

    // A job did fail: that verdict outranks the interruption in the exit code.
    assert_eq!(status.code(), Some(1), "a run with a failed job exits 1");
    assert!(
        elapsed < Duration::from_secs(20),
        "SIGINT during the post-failure wait was ignored: run took {elapsed:?} to exit"
    );
    assert!(
        dir.path().join("term-seen.txt").exists(),
        "the running sibling never saw SIGTERM"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_alive(job_pid) {
        assert!(
            Instant::now() < deadline,
            "sibling {job_pid} is still alive after ox run exited"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let (session_id, session_status) = only_session(&db_path);
    assert_eq!(session_status, "interrupted");
    let jobs = jobs_of_session(&db_path, &session_id);
    assert!(
        jobs.iter()
            .any(|(id, st)| id == "deaf" && st == "cancelled"),
        "the interrupted sibling must be recorded 'cancelled': {jobs:?}"
    );
    assert!(
        !jobs.iter().any(|(_, st)| st == "running"),
        "no row may stay 'running': {jobs:?}"
    );
}
