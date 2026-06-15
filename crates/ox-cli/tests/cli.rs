//! Integration tests for the OxyMake CLI binary.

use assert_cmd::Command;
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
    assert!(parsed["errors"].as_array().unwrap().len() > 0);
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
    assert!(parsed["errors"].as_array().unwrap().len() > 0);
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
    assert_eq!(db.schema_version().unwrap(), 9);
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
