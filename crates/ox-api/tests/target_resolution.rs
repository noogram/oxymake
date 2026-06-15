//! Target resolution through the public API must match the CLI (H34).
//!
//! The same Oxymakefile that works via `ox run` must work via
//! `SessionBuilder` — including `{config.X}` substitution and wildcard
//! expansion in default targets (bug ox-7a98 was fixed in the CLI copy
//! only).

use ox_api::SessionBuilder;

#[test]
fn default_targets_substitute_config_refs() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("A.csv"), "x\n").unwrap();
    std::fs::write(data_dir.join("B.csv"), "y\n").unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    std::fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
results_dir = "out"
samples = ["A", "B"]

[rule.all]
input = ["{config.results_dir}/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["{config.results_dir}/{sample}.txt"]
shell = "cp data/{sample}.csv {config.results_dir}/{sample}.txt"
"#,
    )
    .unwrap();

    let session = SessionBuilder::new(&oxymakefile).build().unwrap();
    assert_eq!(
        session.job_graph.job_ids().len(),
        2,
        "one job per sample — config refs in the aggregation rule must resolve"
    );
}

#[test]
fn explicit_targets_substitute_config_refs() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("A.csv"), "x\n").unwrap();

    let oxymakefile = dir.path().join("Oxymakefile.toml");
    std::fs::write(
        &oxymakefile,
        r#"ox_version = "0.1"

[config]
results_dir = "out"

[rule.process]
input = ["data/{sample}.csv"]
output = ["{config.results_dir}/{sample}.txt"]
shell = "cp data/{sample}.csv {config.results_dir}/{sample}.txt"
"#,
    )
    .unwrap();

    let session = SessionBuilder::new(&oxymakefile)
        .targets(["{config.results_dir}/A.txt"])
        .build()
        .unwrap();
    assert_eq!(
        session.job_graph.job_ids().len(),
        1,
        "config refs in user-provided targets must resolve like the CLI"
    );
}
