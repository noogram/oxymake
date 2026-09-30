use assert_cmd::Command;
use predicates::str::contains;
use std::fs;

fn ox() -> Command {
    Command::cargo_bin("ox").unwrap()
}

#[test]
fn migration_preview_write_and_ambiguous_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Oxymakefile.toml");
    let source = "# retained\nox_version = '0.1'\n[environment]\nuv = 'pyproject.toml'\n[rule.a]\noutput = ['a']\nshell = '''OMP_NUM_THREADS=2 printf 'hi' > a''' # exact bytes\n";
    fs::write(&path, source).unwrap();
    ox().current_dir(dir.path())
        .args(["migrate", "--to-format", "2"])
        .assert()
        .success()
        .stdout(contains("cache identity"))
        .stdout(contains("0.7.0"));
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    ox().current_dir(dir.path())
        .args(["migrate", "--to-format", "2", "--write"])
        .assert()
        .success();
    let migrated = fs::read_to_string(&path).unwrap();
    assert!(migrated.contains("shell = '''OMP_NUM_THREADS=2 printf 'hi' > a''' # exact bytes"));
    let before = ox_format::parse::parse_workflow(source, &path).unwrap();
    let after = ox_format::parse::parse_workflow(&migrated, &path).unwrap();
    assert_eq!(after.format_version, "2");
    assert_eq!(before.rules[0].execution, after.rules[0].execution);
    assert_eq!(before.rules[0].environment, after.rules[0].environment);
    for ambiguous in [
        "ox_version='9.9'",
        "ox_version='0.1'\nfuture_key=true",
        "[rule.a]\nenv='python'",
    ] {
        fs::write(&path, ambiguous).unwrap();
        ox().current_dir(dir.path())
            .args(["migrate", "--to-format", "2", "--write"])
            .assert()
            .failure();
        assert_eq!(fs::read_to_string(&path).unwrap(), ambiguous);
    }
}

#[test]
fn rejected_include_has_no_execution_or_cache_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("Oxymakefile.toml"), "format_version='2'\nox_version='>=0.7.0'\ninclude=['child.toml']\n[rule.a]\noutput=['a']\nshell='touch submitted; echo replaced > a'").unwrap();
    fs::write(
        dir.path().join("child.toml"),
        "format_version='2'\nox_version='>=99.0.0'",
    )
    .unwrap();
    fs::write(dir.path().join("a"), "original").unwrap();
    for executor in ["local", "ray", "slurm"] {
        ox().current_dir(dir.path())
            .args(["run", "a", "--executor", executor])
            .assert()
            .failure()
            .stderr(contains("child.toml"))
            .stderr(contains(">=99.0.0"));
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("a")).unwrap(),
        "original"
    );
    assert!(!dir.path().join("submitted").exists());
    assert!(!dir.path().join(".oxymake").exists());
}

#[test]
fn legacy_warns_for_each_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        "include=['child.toml']",
    )
    .unwrap();
    fs::write(dir.path().join("child.toml"), "ox_version='anything'").unwrap();
    ox().current_dir(dir.path())
        .arg("lint")
        .assert()
        .success()
        .stderr(contains("no binary requirement is enforced"))
        .stderr(contains("child.toml"));
}

#[test]
fn migration_is_coherent_across_includes_and_refuses_partial_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Oxymakefile.toml");
    let child = dir.path().join("child.toml");
    let root_text = "ox_version='0.1'\ninclude=['child.toml']\n";
    let good_child = "ox_version='0.3'\n[rule.a]\noutput=['a']\nshell='echo a > a'\nenvironment={docker='alpine'}\n";
    for bad_child in [
        "ox_version='0.2'",
        "format_version='3'\nox_version='0.3'",
        "ox_version='0.3'\ninclude=['Oxymakefile.toml']",
        "ox_version='0.3'\n[rule.a]\nenv={X='1'}",
    ] {
        fs::write(&root, root_text).unwrap();
        fs::write(&child, bad_child).unwrap();
        ox().current_dir(dir.path())
            .args(["migrate", "--to-format", "2", "--write"])
            .assert()
            .failure();
        assert_eq!(fs::read_to_string(&root).unwrap(), root_text);
        assert_eq!(fs::read_to_string(&child).unwrap(), bad_child);
    }
    fs::write(&root, root_text).unwrap();
    fs::write(&child, good_child).unwrap();
    ox().current_dir(dir.path())
        .args(["migrate", "--to-format", "2"])
        .assert()
        .success()
        .stdout(contains("child.toml"));
    assert_eq!(fs::read_to_string(&child).unwrap(), good_child);
    ox().current_dir(dir.path())
        .args(["migrate", "--to-format", "2", "--write"])
        .assert()
        .success();
    let migrated_root = fs::read_to_string(&root).unwrap();
    let migrated_child = fs::read_to_string(&child).unwrap();
    assert!(migrated_child.contains("shell='echo a > a'"));
    assert_eq!(
        ox_format::parse::parse_workflow(&migrated_root, &root)
            .unwrap()
            .rules
            .len(),
        1
    );
    // Already migrated and mixed supported graphs are safe and idempotent.
    ox().current_dir(dir.path())
        .args(["migrate", "--to-format", "2", "--write"])
        .assert()
        .success();
    assert_eq!(fs::read_to_string(&root).unwrap(), migrated_root);
    assert_eq!(fs::read_to_string(&child).unwrap(), migrated_child);
    fs::write(&root, root_text).unwrap();
    ox().current_dir(dir.path())
        .args(["migrate", "--to-format", "2", "--write"])
        .assert()
        .success();
    assert_eq!(fs::read_to_string(&child).unwrap(), migrated_child);
}

#[test]
fn rejected_requirement_precedes_cache_adoption() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Oxymakefile.toml"),
        "format_version='2'\nox_version='>=99.0.0'",
    )
    .unwrap();
    fs::write(dir.path().join("manifest.json"), r#"{"kind":"oxymake.cache-adoption-manifest","format_version":1,"producer_version":"0.7.0","entries":[]}"#).unwrap();
    ox().current_dir(dir.path())
        .args(["cache-import", "manifest.json"])
        .assert()
        .failure()
        .stderr(contains(">=99.0.0"));
    assert!(!dir.path().join(".oxymake").exists());
}
