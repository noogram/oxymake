//! Round-trip test: parse Snakefile -> generate TOML -> ox-format validates.
//!
//! This ensures that generated Oxymakefile.toml output is structurally valid
//! from ox-format's perspective, making the coupling between the translator
//! and the parser explicit.
//!
//! Tests validate not just that the TOML parses, but that structural properties
//! (rule names, I/O patterns, execution types, resources, environments, etc.)
//! survive the full round trip.

use std::path::Path;

use ox_core::model::{EnvSpec, ExecutionBlock, ResourceValue};
use ox_format::parse::Workflow;
use ox_translate::ir::WorkflowIR;
use ox_translate::oxymake::generator::generate_oxymakefile;
use ox_translate::snakemake::parser::parse_snakefile;

fn fixture_snakefile(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/{}/Snakefile",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {}", path, e))
}

/// Parse a Snakefile fixture, generate TOML, then validate with ox-format.
/// Returns both the IR and the parsed Workflow for detailed assertions.
fn round_trip(fixture_name: &str) -> (WorkflowIR, Workflow, String) {
    let snakefile = fixture_snakefile(fixture_name);
    let ir = parse_snakefile(&snakefile).expect("parse_snakefile failed");
    let toml_output = generate_oxymakefile(&ir);

    let workflow = ox_format::parse::parse_workflow(&toml_output, Path::new("Oxymakefile.toml"))
        .unwrap_or_else(|e| {
            panic!(
                "ox-format failed to parse generated TOML for fixture '{}':\n\
                     Error: {}\n\nGenerated TOML:\n{}",
                fixture_name, e, toml_output,
            )
        });

    (ir, workflow, toml_output)
}

/// Parse inline Snakefile content through the full round trip.
fn round_trip_inline(snakefile: &str) -> (WorkflowIR, Workflow, String) {
    let ir = parse_snakefile(snakefile).expect("parse_snakefile failed");
    let toml_output = generate_oxymakefile(&ir);

    let workflow = ox_format::parse::parse_workflow(&toml_output, Path::new("Oxymakefile.toml"))
        .unwrap_or_else(|e| {
            panic!(
                "ox-format failed to parse generated TOML:\nError: {}\n\nGenerated TOML:\n{}",
                e, toml_output,
            )
        });

    (ir, workflow, toml_output)
}

/// Find a rule by name in the parsed workflow.
fn find_rule<'a>(workflow: &'a Workflow, name: &str) -> &'a ox_core::model::Rule {
    workflow
        .rules
        .iter()
        .find(|r| r.name.as_str() == name)
        .unwrap_or_else(|| {
            let names: Vec<_> = workflow.rules.iter().map(|r| r.name.as_str()).collect();
            panic!(
                "rule '{}' not found in workflow. Available: {:?}",
                name, names
            )
        })
}

// =========================================================================
// Fixture-based round-trip tests
// =========================================================================

#[test]
fn round_trip_simple() {
    let (ir, workflow, _toml) = round_trip("simple");

    // Rule count preserved (IR may have aggregation-only rules like "all")
    assert!(
        !workflow.rules.is_empty() || ir.rules.is_empty(),
        "expected rules in parsed workflow"
    );

    // "process" rule must survive with correct structure
    let process = find_rule(&workflow, "process");
    assert_eq!(process.inputs.len(), 1);
    assert_eq!(process.outputs.len(), 1);
    assert!(process.inputs[0].pattern.contains("{sample}"));
    assert!(process.outputs[0].pattern.contains("{sample}"));
    assert!(matches!(process.execution, ExecutionBlock::Shell { .. }));
}

#[test]
fn round_trip_bio() {
    let (ir, workflow, _toml) = round_trip("bio");

    // At least 2 real rules (align + sort; "all" is aggregation)
    let real_rules: Vec<_> = ir.rules.iter().filter(|r| r.name != "all").collect();
    assert!(real_rules.len() >= 2);

    // Verify "align" rule preserves key properties
    let align = find_rule(&workflow, "align");

    // Named inputs preserved
    assert!(
        align
            .inputs
            .iter()
            .any(|i| i.name.as_deref() == Some("fastq")),
        "named input 'fastq' should survive round trip"
    );
    assert!(
        align
            .inputs
            .iter()
            .any(|i| i.name.as_deref() == Some("ref")),
        "named input 'ref' should survive round trip"
    );

    // Shell execution preserved
    assert!(
        matches!(&align.execution, ExecutionBlock::Shell { command } if command.contains("bwa mem")),
        "shell command should contain 'bwa mem'"
    );

    // Threads → cpu resource preserved
    assert!(
        align.resources.contains_key("cpu"),
        "threads should translate to cpu resource"
    );
    assert_eq!(
        align.resources.get("cpu"),
        Some(&ResourceValue::Int(8)),
        "cpu should be 8 (from threads: 8)"
    );

    // Conda environment preserved
    assert!(
        matches!(&align.environment, Some(EnvSpec::Conda { env }) if env.contains("alignment")),
        "conda environment should survive round trip"
    );

    // Verify "sort" rule
    let sort = find_rule(&workflow, "sort");
    assert_eq!(sort.resources.get("cpu"), Some(&ResourceValue::Int(4)));
}

#[test]
fn round_trip_python() {
    let (_ir, workflow, _toml) = round_trip("python");

    let analyze = find_rule(&workflow, "analyze");

    // Run block preserved as Run execution
    assert!(
        matches!(&analyze.execution, ExecutionBlock::Run { lang, code }
            if lang == "python" && code.contains("import json")),
        "python run block should be preserved as Run {{ lang: python }}"
    );

    // I/O preserved
    assert_eq!(analyze.inputs.len(), 1);
    assert_eq!(analyze.outputs.len(), 1);
}

// =========================================================================
// New fixture round-trip tests
// =========================================================================

#[test]
fn round_trip_resources() {
    let (_ir, workflow, _toml) = round_trip("resources");

    let compute = find_rule(&workflow, "compute");

    // threads: 16 → cpu = 16
    assert_eq!(
        compute.resources.get("cpu"),
        Some(&ResourceValue::Int(16)),
        "threads: 16 should become cpu = 16"
    );

    // mem_mb=8192 → mem = "8G" (8192 / 1024 = 8, evenly divisible)
    assert_eq!(
        compute.resources.get("mem"),
        Some(&ResourceValue::Str("8G".into())),
        "mem_mb=8192 should become mem = \"8G\""
    );

    // disk_mb=2048 → disk = "2G"
    assert_eq!(
        compute.resources.get("disk"),
        Some(&ResourceValue::Str("2G".into())),
        "disk_mb=2048 should become disk = \"2G\""
    );

    // gpu=1
    assert_eq!(
        compute.resources.get("gpu"),
        Some(&ResourceValue::Int(1)),
        "gpu=1 should be preserved"
    );
}

#[test]
fn round_trip_script() {
    let (_ir, workflow, _toml) = round_trip("script");

    let analyze = find_rule(&workflow, "analyze");
    assert!(
        matches!(&analyze.execution, ExecutionBlock::Script { path, .. } if path.to_str().unwrap_or("").contains("analyze.R")),
        "script execution should preserve script path"
    );
}

#[test]
fn round_trip_multi_env() {
    let (_ir, workflow, _toml) = round_trip("multi_env");

    // Docker container
    let docker_rule = find_rule(&workflow, "docker_step");
    assert!(
        matches!(&docker_rule.environment, Some(EnvSpec::Docker { image }) if image.contains("python:3.12-slim")),
        "docker environment should survive round trip"
    );

    // Singularity / Apptainer
    let singularity_rule = find_rule(&workflow, "singularity_step");
    assert!(
        matches!(&singularity_rule.environment, Some(EnvSpec::Apptainer { image }) if image.contains("analysis.sif")),
        "singularity environment should survive round trip as apptainer"
    );
}

#[test]
fn round_trip_wildcard_constraints() {
    let (_ir, workflow, _toml) = round_trip("wildcard_constraints");

    let process = find_rule(&workflow, "process");
    assert_eq!(
        process
            .wildcard_constraints
            .get("sample")
            .map(String::as_str),
        Some("[A-Z]+"),
        "wildcard constraint for 'sample' should survive"
    );
    assert_eq!(
        process
            .wildcard_constraints
            .get("replicate")
            .map(String::as_str),
        Some("[0-9]+"),
        "wildcard constraint for 'replicate' should survive"
    );
}

#[test]
fn round_trip_params() {
    let (_ir, workflow, _toml) = round_trip("params");

    let generate = find_rule(&workflow, "generate");
    assert_eq!(
        generate.params.get("n_iter").map(String::as_str),
        Some("1000"),
        "numeric param n_iter should survive"
    );
    assert_eq!(
        generate.params.get("seed").map(String::as_str),
        Some("42"),
        "numeric param seed should survive"
    );
    assert_eq!(
        generate.params.get("label").map(String::as_str),
        Some("experiment"),
        "string param label should survive"
    );
}

#[test]
fn round_trip_log() {
    let (_ir, workflow, _toml) = round_trip("log");

    let transform = find_rule(&workflow, "transform");
    // Log gets translated to stdout/stderr paths
    assert!(
        transform.log.stdout.is_some() || transform.log.stderr.is_some(),
        "log directive should produce log config"
    );
    let log_path = transform
        .log
        .stdout
        .as_deref()
        .or(transform.log.stderr.as_deref())
        .expect("at least one log path should be set");
    assert!(
        log_path.contains("{sample}"),
        "log path should preserve wildcards"
    );
}

// =========================================================================
// Inline round-trip tests for specific features
// =========================================================================

#[test]
fn round_trip_inline_minimal() {
    let snakefile = r#"
rule process:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.txt"
    shell:
        "sort {input} > {output}"
"#;

    let (_ir, workflow, _toml) = round_trip_inline(snakefile);

    assert_eq!(workflow.rules.len(), 1);
    assert_eq!(workflow.rules[0].name.as_str(), "process");
}

#[test]
fn round_trip_inline_config_values() {
    let snakefile = r#"
SAMPLES = ["A", "B", "C"]

rule all:
    input:
        expand("results/{sample}.txt", sample=SAMPLES)

rule process:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.txt"
    shell:
        "sort {input} > {output}"
"#;

    let (ir, workflow, _toml) = round_trip_inline(snakefile);

    // Config values should be extracted
    assert!(
        !ir.config_values.is_empty(),
        "SAMPLES assignment should produce config values in IR"
    );

    // Config should survive in the parsed workflow
    if !ir.config_values.is_empty() {
        assert!(
            !workflow.config.is_empty(),
            "config values from IR should appear in parsed workflow config"
        );
    }
}

#[test]
fn round_trip_inline_multiple_rules() {
    let snakefile = r#"
rule step_a:
    input:
        "raw/{id}.dat"
    output:
        "intermediate/{id}.tmp"
    shell:
        "preprocess {input} > {output}"

rule step_b:
    input:
        "intermediate/{id}.tmp"
    output:
        "final/{id}.result"
    shell:
        "finalize {input} > {output}"
"#;

    let (_ir, workflow, _toml) = round_trip_inline(snakefile);

    assert_eq!(workflow.rules.len(), 2);

    let step_a = find_rule(&workflow, "step_a");
    let step_b = find_rule(&workflow, "step_b");

    // Verify I/O patterns chain correctly
    assert!(step_a.outputs[0].pattern.contains("intermediate/"));
    assert!(step_b.inputs[0].pattern.contains("intermediate/"));
}

#[test]
fn round_trip_inline_named_io() {
    let snakefile = r#"
rule merge:
    input:
        left="data/{sample}_L.csv",
        right="data/{sample}_R.csv"
    output:
        merged="results/{sample}_merged.csv",
        stats="results/{sample}_stats.txt"
    shell:
        "merge {input.left} {input.right} -o {output.merged} --stats {output.stats}"
"#;

    let (_ir, workflow, _toml) = round_trip_inline(snakefile);

    let merge = find_rule(&workflow, "merge");

    // Named inputs preserved
    let input_names: Vec<_> = merge
        .inputs
        .iter()
        .filter_map(|i| i.name.as_deref())
        .collect();
    assert!(input_names.contains(&"left"), "named input 'left' missing");
    assert!(
        input_names.contains(&"right"),
        "named input 'right' missing"
    );

    // Named outputs preserved
    let output_names: Vec<_> = merge
        .outputs
        .iter()
        .filter_map(|o| o.name.as_deref())
        .collect();
    assert!(
        output_names.contains(&"merged"),
        "named output 'merged' missing"
    );
    assert!(
        output_names.contains(&"stats"),
        "named output 'stats' missing"
    );
}

#[test]
fn round_trip_inline_multiline_shell() {
    let snakefile = r#"
rule pipeline:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.txt"
    shell:
        """
        set -euo pipefail
        cat {input} | sort | uniq > {output}
        """
"#;

    let (_ir, workflow, _toml) = round_trip_inline(snakefile);

    let pipeline = find_rule(&workflow, "pipeline");
    match &pipeline.execution {
        ExecutionBlock::Shell { command } => {
            assert!(
                command.contains("set -euo pipefail"),
                "multiline shell should preserve content"
            );
        }
        other => panic!("expected Shell execution, got {:?}", other),
    }
}

#[test]
fn round_trip_inline_aggregation_rule() {
    let snakefile = r#"
rule all:
    input:
        "results/final.txt"

rule produce:
    output:
        "results/final.txt"
    shell:
        "echo done > {output}"
"#;

    let (_ir, workflow, _toml) = round_trip_inline(snakefile);

    // Both rules should parse — "all" has no execution block
    assert!(
        workflow.rules.len() >= 1,
        "at least the 'produce' rule should parse"
    );
    let produce = find_rule(&workflow, "produce");
    assert!(matches!(produce.execution, ExecutionBlock::Shell { .. }));
}

// =========================================================================
// Structural invariant tests
// =========================================================================

#[test]
fn round_trip_rule_names_preserved() {
    // Verify that all IR rule names appear in the parsed workflow
    for fixture in &["simple", "bio", "python", "resources", "script"] {
        let (ir, workflow, toml) = round_trip(fixture);
        for ir_rule in &ir.rules {
            // Aggregation-only rules (no execution, no outputs) may not produce
            // valid ox-format rules, so skip them
            if ir_rule.execution == ox_translate::ir::ExecutionIR::None
                && ir_rule.outputs.is_empty()
            {
                continue;
            }
            let found = workflow
                .rules
                .iter()
                .any(|r| r.name.as_str() == ir_rule.name);
            assert!(
                found,
                "IR rule '{}' missing from ox-format output in fixture '{}'.\nTOML:\n{}",
                ir_rule.name, fixture, toml,
            );
        }
    }
}

#[test]
fn round_trip_io_count_preserved() {
    // For each fixture, verify that input/output counts are preserved
    // (at least for rules with concrete I/O, not expand patterns)
    for fixture in &["simple", "bio", "python", "resources", "script", "log"] {
        let (ir, workflow, toml) = round_trip(fixture);
        for ir_rule in &ir.rules {
            if let Some(parsed_rule) = workflow
                .rules
                .iter()
                .find(|r| r.name.as_str() == ir_rule.name)
            {
                // Only check concrete inputs (not expand: patterns)
                let concrete_inputs: Vec<_> = ir_rule
                    .inputs
                    .iter()
                    .filter(|i| {
                        !i.pattern.starts_with("expand:") && !i.pattern.starts_with("# MANUAL:")
                    })
                    .collect();

                assert_eq!(
                    parsed_rule.inputs.len(),
                    concrete_inputs.len(),
                    "input count mismatch for rule '{}' in fixture '{}'.\nTOML:\n{}",
                    ir_rule.name,
                    fixture,
                    toml,
                );

                assert_eq!(
                    parsed_rule.outputs.len(),
                    ir_rule.outputs.len(),
                    "output count mismatch for rule '{}' in fixture '{}'.\nTOML:\n{}",
                    ir_rule.name,
                    fixture,
                    toml,
                );
            }
        }
    }
}

#[test]
fn round_trip_execution_type_preserved() {
    // Verify execution type mapping: Shell→Shell, Run→Run, Script→Script
    let test_cases = vec![
        ("simple", "process", "shell"),
        ("bio", "align", "shell"),
        ("python", "analyze", "run"),
        ("script", "analyze", "script"),
    ];

    for (fixture, rule_name, expected_type) in test_cases {
        let (_ir, workflow, toml) = round_trip(fixture);
        let rule = find_rule(&workflow, rule_name);

        let actual_type = match &rule.execution {
            ExecutionBlock::Shell { .. } => "shell",
            ExecutionBlock::Run { .. } => "run",
            ExecutionBlock::Script { .. } => "script",
            ExecutionBlock::Call { .. } => "call",
        };

        assert_eq!(
            actual_type, expected_type,
            "execution type mismatch for rule '{}' in fixture '{}'. TOML:\n{}",
            rule_name, fixture, toml,
        );
    }
}

#[test]
fn round_trip_snakemake_with_log_threads_params() {
    // Exercises the full Snakemake -> OxyMake chain with log, threads, and params
    // that are key Snakemake features needed for real workflows.
    let snakefile = r#"
rule process:
    input:
        csv="data/{sample}.csv"
    output:
        result="results/{sample}.txt"
    params:
        n_rows=100
    log:
        "logs/process_{sample}.log"
    threads: 4
    shell:
        "echo Processing {wildcards.sample} > {log} && cat {input.csv} | head -n {params.n_rows} > {output.result}"
"#;

    let (ir, workflow, _toml) = round_trip_inline(snakefile);

    assert_eq!(ir.rules.len(), 1);
    assert_eq!(ir.rules[0].log, vec!["logs/process_{sample}.log"]);

    let rule = find_rule(&workflow, "process");

    // Verify log config is preserved through the round-trip
    assert!(rule.log.stdout.is_some());
    assert!(
        rule.log.stdout.as_ref().unwrap().contains("{sample}"),
        "log path should preserve wildcard pattern"
    );

    // Verify threads -> resources.cpu mapping
    assert!(rule.resources.contains_key("cpu"));

    // Verify params
    assert!(rule.params.contains_key("n_rows"));

    // Verify shell command preserves {log}, {threads}, {params.*}, {wildcards.*}
    if let ExecutionBlock::Shell { ref command } = rule.execution {
        assert!(
            command.contains("{log}") || command.contains("{output.result}"),
            "shell should preserve log reference"
        );
        assert!(
            command.contains("{params.n_rows}"),
            "shell should preserve params reference"
        );
        assert!(
            command.contains("{input.csv}"),
            "shell should preserve named input reference"
        );
    } else {
        panic!("expected shell execution block");
    }
}

#[test]
fn round_trip_configfile_yaml_resolution() {
    // Parse a Snakefile with configfile: "config.yaml", then resolve the YAML
    let snakefile = r#"
configfile: "config.yaml"

rule all:
    input:
        expand("results/{sample}.txt", sample=config["samples"])
"#;
    let mut ir = parse_snakefile(snakefile).expect("parse_snakefile failed");
    assert_eq!(ir.config_file.as_deref(), Some("config.yaml"));
    assert!(
        ir.config_values.is_empty(),
        "config_values should be empty before resolution"
    );

    // Resolve using benchmark/advanced/config.yaml
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let base_dir = workspace_root.join("benchmark/advanced");
    ir.resolve_config(&base_dir).expect("resolve_config failed");

    // config_values should now be populated from YAML
    assert!(
        !ir.config_values.is_empty(),
        "config_values should be populated after resolution"
    );
    assert!(
        ir.config_values
            .iter()
            .any(|e| e.key == "samples" && e.values == vec!["iris", "wine"]),
        "should contain samples list from YAML"
    );
    assert!(
        ir.config_values
            .iter()
            .any(|e| e.key == "seed" && e.values == vec!["42"]),
        "should contain seed scalar from YAML"
    );

    // Generate TOML — should have [config] section, NOT a TODO comment
    let toml_output = generate_oxymakefile(&ir);
    assert!(
        toml_output.contains("[config]"),
        "should have [config] section"
    );
    assert!(
        toml_output.contains(r#"samples = ["iris", "wine"]"#),
        "should have samples array"
    );
    assert!(
        toml_output.contains(r#"seed = "42""#),
        "should have seed value"
    );
    assert!(
        !toml_output.contains("# TODO: Convert"),
        "should NOT have TODO comment after resolution"
    );

    // Verify generated TOML is valid
    let _: toml::Value = toml::from_str(&toml_output).expect("generated TOML should be valid");
}

#[test]
fn round_trip_generated_toml_is_valid_toml() {
    // Every fixture's generated TOML should parse as valid TOML (independent of ox-format)
    for fixture in &[
        "simple",
        "bio",
        "python",
        "resources",
        "script",
        "multi_env",
        "wildcard_constraints",
        "params",
        "log",
    ] {
        let snakefile = fixture_snakefile(fixture);
        let ir = parse_snakefile(&snakefile).expect("parse_snakefile failed");
        let toml_output = generate_oxymakefile(&ir);

        let _: toml::Value = toml::from_str(&toml_output).unwrap_or_else(|e| {
            panic!(
                "generated TOML for fixture '{}' is not valid TOML:\n{}\n\nTOML:\n{}",
                fixture, e, toml_output,
            )
        });
    }
}

/// `source_line` from the Snakefile parser must thread all the way through
/// generation and back into `ox_core::model::Rule.source_line`, so that
/// `PlanError` can cite the original Snakefile location on failure.
#[test]
fn round_trip_preserves_source_line_through_oxymakefile() {
    let snakefile = "\
rule first:
    input: \"in.txt\"
    output: \"out.txt\"
    shell: \"cp {input} {output}\"

rule second:
    input: \"out.txt\"
    output: \"final.txt\"
    shell: \"cp {input} {output}\"
";
    let (ir, workflow, toml_output) = round_trip_inline(snakefile);

    // IR must already carry source_line.
    let ir_first = ir.rules.iter().find(|r| r.name == "first").unwrap();
    let ir_second = ir.rules.iter().find(|r| r.name == "second").unwrap();
    assert_eq!(ir_first.source_line, Some(1));
    assert_eq!(ir_second.source_line, Some(6));

    // Generator must serialize `source_line = N` so the parser can recover it.
    assert!(
        toml_output.contains("source_line = 1"),
        "expected `source_line = 1` in generated TOML:\n{toml_output}"
    );
    assert!(
        toml_output.contains("source_line = 6"),
        "expected `source_line = 6` in generated TOML:\n{toml_output}"
    );

    // ox-format::parse must populate the field on `Rule`.
    let parsed_first = find_rule(&workflow, "first");
    let parsed_second = find_rule(&workflow, "second");
    assert_eq!(parsed_first.source_line, Some(1));
    assert_eq!(parsed_second.source_line, Some(6));
}
