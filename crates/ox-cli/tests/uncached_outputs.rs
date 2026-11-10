//! Existing outputs without a matching cache entry are reported before dispatch.
use assert_cmd::Command;
use std::{fs, time::Duration};
use tempfile::TempDir;

const WARNING: &str =
    "warning: 1 job(s) have all declared outputs present but no cache entry; they will be rebuilt";

fn workflow() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("out"), "existing").unwrap();
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        r#"ox_version = "0.1"
[rule.produce]
output = ["out"]
shell = "echo rebuilt > out"
"#,
    )
    .unwrap();
    dir
}

fn run(dir: &TempDir, args: &[&str]) -> String {
    let output = Command::cargo_bin("ox")
        .unwrap()
        .current_dir(dir.path())
        .arg("run")
        .args(args)
        .timeout(Duration::from_secs(30))
        .assert()
        .success();
    String::from_utf8(output.get_output().stderr.clone()).unwrap()
}

#[test]
fn warns_before_execution_and_stops_after_cache_entry_exists() {
    let dir = workflow();
    let stderr = run(&dir, &[]);
    assert!(stderr.contains(WARNING), "{stderr}");
    assert!(stderr.contains("produce"), "{stderr}");
    assert!(!run(&dir, &[]).contains("outputs present but no cache entry"));
    assert!(!run(&dir, &["--dry-run"]).contains("outputs present but no cache entry"));
}

#[test]
fn dry_run_warns_for_every_executor_without_changing_outputs_or_recording_a_run() {
    for executor in ["local", "ray", "slurm"] {
        let dir = workflow();
        let stderr = run(&dir, &["--dry-run", "--executor", executor]);
        assert!(stderr.contains(WARNING), "{executor}: {stderr}");
        assert!(stderr.contains("produce"), "{stderr}");
        assert_eq!(
            fs::read_to_string(dir.path().join("out")).unwrap(),
            "existing"
        );
        assert!(!dir.path().join(".oxymake/state.db").exists());
    }
}

#[test]
fn explicitly_forced_or_excluded_jobs_do_not_warn() {
    for args in [
        vec!["--forcerun", "produce"],
        vec!["--no-cache"],
        vec!["--omit-from", "produce"],
    ] {
        let dir = workflow();
        let mut dry_args = args.clone();
        dry_args.push("--dry-run");
        assert!(!run(&dir, &dry_args).contains("outputs present but no cache entry"));
        assert!(!run(&dir, &args).contains("outputs present but no cache entry"));
    }
}

#[test]
fn dry_run_uses_content_validation_with_a_remote_cache() {
    let dir = workflow();
    assert!(
        !run(&dir, &["--dry-run", "--cache-validation", "mtime"])
            .contains("outputs present but no cache entry")
    );
    let stderr = run(
        &dir,
        &[
            "--dry-run",
            "--cache-validation",
            "mtime",
            "--cache-remote",
            "remote",
        ],
    );
    assert!(
        stderr.contains("outputs present but no cache entry"),
        "{stderr}"
    );
    assert!(
        stderr.contains("remote entries are not read in dry-run"),
        "{stderr}"
    );
    assert!(
        stderr.contains("may be restored instead of executed"),
        "{stderr}"
    );
    assert!(!stderr.contains("they will be rebuilt"), "{stderr}");
    assert!(!dir.path().join("remote").exists());
}

#[test]
fn invalidated_cache_entry_warns_before_rebuild() {
    let dir = workflow();
    let cache_entries = || {
        rusqlite::Connection::open(dir.path().join(".oxymake/cache/cache.db"))
            .unwrap()
            .query_row("SELECT count(*) FROM cache_entries", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };
    run(&dir, &[]);
    assert_eq!(cache_entries(), 1);
    assert!(!run(&dir, &["--dry-run"]).contains("outputs present but no cache entry"));
    fs::write(dir.path().join("out"), "changed outside the workflow").unwrap();
    // A dry run retains the row, so this is stale, not unrecorded. The real
    // run still invalidates the mismatch before warning and rebuilding.
    assert!(!run(&dir, &["--dry-run"]).contains(WARNING));
    assert_eq!(cache_entries(), 1);
    assert!(run(&dir, &[]).contains(WARNING));
    assert!(!run(&dir, &[]).contains("outputs present but no cache entry"));
}

#[test]
fn warning_counts_only_complete_file_outputs_and_truncates_ids() {
    for (count, suffix) in [(10, ""), (12, ", ... (2 more)")] {
        let dir = TempDir::new().unwrap();
        let mut source = String::from("ox_version = \"0.1\"\n[rule.all]\ninput = [");
        for i in 0..count {
            source.push_str(&format!("\"out{i:02}\","));
        }
        source.push_str("\"missing\"]\n");
        for i in 0..count {
            source.push_str(&format!(
                "[rule.job{i:02}]\noutput = [\"out{i:02}\"]\nshell = \"true\"\n"
            ));
            fs::write(dir.path().join(format!("out{i:02}")), "existing").unwrap();
        }
        source
            .push_str("[rule.incomplete]\noutput = [\"present\", \"missing\"]\nshell = \"true\"\n");
        fs::write(dir.path().join("present"), "existing").unwrap();
        fs::write(dir.path().join("Oxymakefile.toml"), source).unwrap();
        let stderr = run(&dir, &["--dry-run"]);
        let warning = stderr
            .lines()
            .find(|l| l.contains("outputs present but no cache entry"))
            .expect(&stderr);
        let names = "job00, job01, job02, job03, job04, job05, job06, job07, job08, job09";
        assert_eq!(
            warning,
            format!(
                "warning: {count} job(s) have all declared outputs present but no cache entry; they will be rebuilt: {names}{suffix}"
            )
        );
    }
}
