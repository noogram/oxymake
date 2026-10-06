use ox_format::parse::parse_workflow;
use std::{fs, path::Path};

fn reject(source: &str, expected: &[&str]) {
    let error = parse_workflow(source, Path::new("contract.toml"))
        .unwrap_err()
        .to_string();
    for fragment in expected {
        assert!(error.contains(fragment), "missing {fragment:?}: {error}");
    }
}

#[test]
fn version_contract_diagnostics() {
    for requirement in [
        None,
        Some("garbage"),
        Some("0.1"),
        Some("^0.7.0"),
        Some("<=0.7.0"),
        Some(">=99.0.0"),
        Some(">=0.7"),
        Some(">=0.7.0, <1.0.0"),
        Some(">=00.7.0"),
    ] {
        let declaration = requirement
            .map(|r| format!("ox_version = {r:?}\n"))
            .unwrap_or_default();
        reject(
            &format!("format_version = \"2\"\n{declaration}"),
            &[
                "contract.toml",
                "ox_version",
                requirement.unwrap_or("<missing>"),
                env!("CARGO_PKG_VERSION"),
            ],
        );
    }
    reject(
        "format_version = \"99\"",
        &["contract.toml", "format_version", "99"],
    );
    parse_workflow(
        &format!(
            "format_version = \"2\"\nox_version = \">={}\"",
            env!("CARGO_PKG_VERSION")
        ),
        Path::new("ok.toml"),
    )
    .unwrap();
}

#[test]
fn structural_keys_are_closed_dynamic_maps_are_open() {
    let header = "format_version = \"2\"\nox_version = \">=0.7.0\"\n";
    for (body, key) in [
        ("variables = {}", "variables"),
        ("[rule.a]\nshel = 'true'", "shel"),
        ("[rule.a]\nenv = 'python'", "expected a table"),
        ("[rule.a.log]\nstdot = 'x'", "stdot"),
        ("[rule.a]\noutput = [{path='x', typo=true}]", "typo"),
        ("[gate.a]\nbefor = []", "befor"),
        ("[profile.a]\njob = 1", "job"),
        ("[executor.slurm]\npartiton = 'x'", "partiton"),
        (
            "[rule.a]\nwhen = {op='not', condition={op='env_set', var='X', typo=1}}",
            "typo",
        ),
        ("[rule.a]\nerror_strategy = {retry=1, typo=1}", "typo"),
    ] {
        reject(&format!("{header}{body}"), &["contract.toml", key]);
    }
    parse_workflow(&format!("{header}\n[config]\ncustom = {{arbitrary='value'}}\n[env]\nCUSTOM = 'value'\n[rule.a]\nresources = {{custom_accelerator=1}}\ntags = {{custom='yes'}}\nparams = {{custom='value'}}\nenv = {{ RULE_CUSTOM = 'value' }}\n"), Path::new("open.toml")).unwrap();
}

#[test]
fn whole_include_graph_is_gated_before_external_config_reads() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("child.toml"),
        "format_version = '2'\nox_version = '>=99.0.0'",
    )
    .unwrap();
    let source = "format_version = '2'\nox_version = '>=0.7.0'\ninclude = ['child.toml']\n[config]\nitems = {source='must-not-read.csv', key='name'}";
    let error = parse_workflow(source, &dir.path().join("root.toml"))
        .unwrap_err()
        .to_string();
    for fragment in ["child.toml", ">=99.0.0", env!("CARGO_PKG_VERSION")] {
        assert!(error.contains(fragment), "{error}");
    }
    assert!(!error.contains("must-not-read"), "{error}");
}

#[test]
fn migration_detects_concurrent_edit_and_staging_failure_before_writes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root.toml");
    let child = dir.path().join("child.toml");
    let root_text = "ox_version='0.1'\ninclude=['child.toml']";
    let child_text = "ox_version='0.3'";
    fs::write(&root, root_text).unwrap();
    fs::write(&child, child_text).unwrap();
    let migration = ox_format::migrate::prepare(&root).unwrap();
    fs::write(&child, "# concurrent edit\nox_version='0.3'").unwrap();
    assert!(
        migration
            .write()
            .unwrap_err()
            .to_string()
            .contains("changed since migration review")
    );
    assert_eq!(fs::read_to_string(&root).unwrap(), root_text);
    fs::write(&child, child_text).unwrap();
    let original_permissions = fs::metadata(&child).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&child, permissions).unwrap();
    assert!(
        migration
            .write()
            .unwrap_err()
            .to_string()
            .contains("read-only")
    );
    assert_eq!(fs::read_to_string(&root).unwrap(), root_text);
    assert_eq!(fs::read_to_string(&child).unwrap(), child_text);
    fs::set_permissions(&child, original_permissions).unwrap();
}

#[test]
fn legacy_retains_permissive_keys_and_informational_version() {
    let source = "ox_version='99.99'\nfuture_key=true\n[rule.a]\nenv='ignored'\nresources={cpu=2, accelerator='custom'}";
    let workflow = parse_workflow(source, Path::new("legacy.toml")).unwrap();
    assert_eq!(workflow.format_version, "1");
    assert_eq!(workflow.ox_version.as_deref(), Some("99.99"));
    assert_eq!(workflow.rules[0].resources.len(), 2);
    assert!(workflow.warnings[0].contains("legacy.toml"));
    assert!(workflow.warnings[0].contains("no binary requirement is enforced"));
}

#[test]
fn schema1_repository_fixture_corpus_is_unchanged() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    for (name, expected_rules) in [("simple", 2), ("genomics", 4)] {
        let path = fixtures.join(name).join("Oxymakefile.toml");
        let source = fs::read_to_string(&path).unwrap();
        let workflow = parse_workflow(&source, &path).unwrap();
        assert_eq!(workflow.format_version, "1");
        assert_eq!(workflow.rules.len(), expected_rules);
        assert!(workflow.warnings[0].contains("no binary requirement is enforced"));
        assert_eq!(fs::read_to_string(path).unwrap(), source);
    }
}

#[cfg(unix)]
#[test]
fn included_symlink_contexts_are_all_gated_before_config() {
    let dir = tempfile::tempdir().unwrap();
    for folder in ["first", "second"] {
        fs::create_dir(dir.path().join(folder)).unwrap();
    }
    let shared = dir.path().join("first/shared.toml");
    fs::write(
        &shared,
        "format_version='2'\nox_version='>=0.7.0'\ninclude=['leaf.toml']",
    )
    .unwrap();
    std::os::unix::fs::symlink(&shared, dir.path().join("second/shared.toml")).unwrap();
    fs::write(
        dir.path().join("first/leaf.toml"),
        "format_version='2'\nox_version='>=0.7.0'",
    )
    .unwrap();
    fs::write(
        dir.path().join("second/leaf.toml"),
        "format_version='2'\nox_version='>=99.0.0'",
    )
    .unwrap();
    let source = "include=['first/shared.toml','second/shared.toml']\n[config]\nx={source='must-not-read.csv', key='x'}";
    let error = parse_workflow(source, &dir.path().join("root.toml"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("second/leaf.toml") && error.contains(">=99.0.0"),
        "{error}"
    );
}

#[test]
fn migration_preserves_all_execution_forms_and_multiline_command_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("root.toml");
    let body = r#"
[environment]
uv = 'pyproject.toml'
[rule.shell]
output = ['shell.out']
shell = '''X=2 echo 'literal' \
  && echo "quoted" > shell.out
'''
[rule.run]
output = ['run.out']
lang = 'python'
run = """
print(\"hello\")
"""
[rule.script]
output = ['script.out']
script = 'scripts/job.py'
[rule.call]
output = ['call.out']
call = 'module:function'
"#;
    fs::write(&path, format!("ox_version='0.1'\n{body}")).unwrap();
    ox_format::migrate::prepare(&path).unwrap().write().unwrap();
    assert!(fs::read_to_string(&path).unwrap().ends_with(body));
}

#[test]
fn migration_report_states_cache_and_document_formatting_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("root.toml");
    fs::write(
        &path,
        "ox_version='0.1'\r\n\r\n[rule.a]\r\noutput=['a']\r\nshell='true'\r\n",
    )
    .unwrap();

    let migration = ox_format::migrate::prepare(&path).unwrap();
    let report = migration.report();
    assert!(
        report.contains("Migration preserves cache identity."),
        "{report}"
    );
    assert!(
        report.contains(
            "Any future execution-semantics change will state its cache consequences in that release's notes."
        ),
        "{report}"
    );
    assert!(
        report.contains("Document formatting, including line endings, may be normalized."),
        "{report}"
    );
    assert!(!report.contains("first run after"), "{report}");
}
