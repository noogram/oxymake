//! Export OxyMake workflows to other formats.
//!
//! Supports exporting to Snakemake and WDL formats. The flow is:
//! 1. Parse `Oxymakefile.toml` → `Workflow` (via `ox-format`)
//! 2. Convert `Workflow` → `WorkflowIR`
//! 3. Generate target format from `WorkflowIR`

use std::path::Path;

use ox_core::model::{EnvSpec, ExecutionBlock, ResourceValue, Rule};
use ox_format::parse::{ConfigValue, Workflow};

use crate::ir::*;
use crate::snakemake::generator::generate_snakefile;
use crate::wdl::generator::generate_wdl;

/// Export an OxyMake workflow to Snakemake format.
///
/// Parses the Oxymakefile at the given path and returns a Snakefile string.
pub fn export_snakemake(oxymakefile: &Path) -> anyhow::Result<String> {
    let content = std::fs::read_to_string(oxymakefile)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {}", oxymakefile.display(), e))?;

    let workflow = ox_format::parse::parse_workflow(&content, oxymakefile)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {}", oxymakefile.display(), e))?;

    let ir = workflow_to_ir(&workflow);
    Ok(generate_snakefile(&ir))
}

/// Export an OxyMake workflow to WDL format.
///
/// Parses the Oxymakefile at the given path and returns a WDL string.
pub fn export_wdl(oxymakefile: &Path) -> anyhow::Result<String> {
    let content = std::fs::read_to_string(oxymakefile)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {}", oxymakefile.display(), e))?;

    let workflow = ox_format::parse::parse_workflow(&content, oxymakefile)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {}", oxymakefile.display(), e))?;

    let ir = workflow_to_ir(&workflow);
    Ok(generate_wdl(&ir))
}

/// Convert an ox-format `Workflow` to the translation IR.
pub fn workflow_to_ir(workflow: &Workflow) -> WorkflowIR {
    let mut diagnostics = Vec::new();

    // Convert config to IR config values and determine if we need a configfile
    let (config_values, config_file) = convert_config(&workflow.config);

    // Convert includes
    let includes: Vec<String> = workflow
        .includes
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    // Convert rules
    let rules: Vec<RuleIR> = workflow
        .rules
        .iter()
        .map(|r| convert_rule(r, &mut diagnostics))
        .collect();

    // Gates have no Snakemake equivalent — emit diagnostics
    for gate in &workflow.gates {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warning,
            message: format!(
                "Gate '{}' has no Snakemake equivalent (ignored): {}",
                gate.name, gate.message
            ),
            line: None,
        });
    }

    // Global environment
    if let Some(ref env) = workflow.global_environment {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Info,
            message: format!(
                "Global environment ({}) applied per-rule in Snakemake",
                env_spec_name(env)
            ),
            line: None,
        });
    }

    WorkflowIR {
        rules,
        config_file,
        includes,
        diagnostics,
        config_values,
        escalations: vec![],
        global_container: workflow.global_environment.as_ref().and_then(|e| match e {
            EnvSpec::Docker { image } => Some(format!("docker://{}", image)),
            EnvSpec::Apptainer { image } => Some(image.clone()),
            _ => None,
        }),
        global_report: None,
    }
}

fn env_spec_name(env: &EnvSpec) -> &'static str {
    match env {
        EnvSpec::System => "system",
        EnvSpec::Uv { .. } => "uv",
        EnvSpec::Conda { .. } => "conda",
        EnvSpec::Docker { .. } => "docker",
        EnvSpec::Nix { .. } => "nix",
        EnvSpec::Apptainer { .. } => "apptainer",
    }
}

/// Convert config section to IR config values.
/// Returns (config_values, optional configfile path).
fn convert_config(
    config: &std::collections::BTreeMap<String, ConfigValue>,
) -> (Vec<ConfigEntryIR>, Option<String>) {
    if config.is_empty() {
        return (vec![], None);
    }

    let mut entries = Vec::new();
    let mut needs_configfile = false;

    for (key, value) in config {
        match value {
            ConfigValue::List(items) => {
                entries.push(ConfigEntryIR {
                    key: key.clone(),
                    values: items.clone(),
                });
            }
            ConfigValue::Scalar(s) => {
                entries.push(ConfigEntryIR {
                    key: key.clone(),
                    values: vec![s.clone()],
                });
            }
            ConfigValue::FileSource { source, .. } => {
                // File sources need a configfile in Snakemake
                needs_configfile = true;
                entries.push(ConfigEntryIR {
                    key: key.clone(),
                    values: vec![format!("# from {}", source.display())],
                });
            }
        }
    }

    // If we have config values, generate a config.yaml for Snakemake
    let config_file = if needs_configfile || !entries.is_empty() {
        Some("config.yaml".to_string())
    } else {
        None
    };

    (entries, config_file)
}

fn convert_rule(rule: &Rule, diagnostics: &mut Vec<Diagnostic>) -> RuleIR {
    // Convert inputs
    let inputs: Vec<PortIR> = rule
        .inputs
        .iter()
        .map(|inp| PortIR {
            name: inp.name.clone(),
            pattern: inp.pattern.to_string(),
            lifecycle: None,
        })
        .collect();

    // Convert outputs
    let outputs: Vec<PortIR> = rule
        .outputs
        .iter()
        .map(|out| {
            let lifecycle = match out.lifecycle {
                ox_core::model::OutputLifecycle::Permanent => None,
                ox_core::model::OutputLifecycle::Temporary => Some("temporary".to_string()),
                ox_core::model::OutputLifecycle::Protected => Some("protected".to_string()),
            };
            PortIR {
                name: out.name.clone(),
                pattern: out.pattern.to_string(),
                lifecycle,
            }
        })
        .collect();

    // Convert execution
    let execution = match &rule.execution {
        ExecutionBlock::Shell { command } => ExecutionIR::Shell(command.clone()),
        ExecutionBlock::Run { code, .. } => ExecutionIR::Run(code.clone()),
        ExecutionBlock::Script { path, .. } => {
            ExecutionIR::Script(path.to_string_lossy().to_string())
        }
        ExecutionBlock::Call { function, lang } => {
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warning,
                message: format!(
                    "Rule '{}': call execution ({}.{}) has no Snakemake equivalent, converted to run block",
                    rule.name, lang, function
                ),
                line: None,
            });
            ExecutionIR::Run(format!("# Call: {}.{}", lang, function))
        }
    };

    // Convert threads from resources
    let threads = rule.resources.get("cpu").map(|v| match v {
        ResourceValue::Int(n) => ThreadsIR::Literal(*n as u32),
        ResourceValue::Str(s) => {
            if let Ok(n) = s.parse::<u32>() {
                ThreadsIR::Literal(n)
            } else {
                ThreadsIR::Dynamic(s.clone())
            }
        }
        ResourceValue::Float(f) => ThreadsIR::Literal(f.into_inner() as u32),
    });

    // Convert resources (excluding cpu which becomes threads)
    let resources: Vec<ResourceIR> = rule
        .resources
        .iter()
        .filter(|(k, _)| k.as_str() != "cpu")
        .map(|(k, v)| ResourceIR {
            key: k.clone(),
            value: resource_value_to_string(v),
        })
        .collect();

    // Convert environment
    let environment = rule.environment.as_ref().and_then(|env| match env {
        EnvSpec::Conda { env: path } => Some(EnvironmentIR::Conda(path.clone())),
        EnvSpec::Docker { image } => Some(EnvironmentIR::Container(format!("docker://{}", image))),
        EnvSpec::Apptainer { image } => Some(EnvironmentIR::Singularity(image.clone())),
        other => {
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warning,
                message: format!(
                    "Rule '{}': {} environment has no Snakemake equivalent (ignored)",
                    rule.name,
                    env_spec_name(other)
                ),
                line: None,
            });
            None
        }
    });

    // Convert wildcard constraints
    let wildcard_constraints: Vec<(String, String)> = rule
        .wildcard_constraints
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Convert params
    let params: Vec<ParamIR> = rule
        .params
        .iter()
        .map(|(k, v)| ParamIR {
            name: k.clone(),
            value: v.clone(),
        })
        .collect();

    // Convert log
    let mut log = Vec::new();
    if let Some(ref stdout) = rule.log.stdout {
        log.push(stdout.clone());
    }
    if let Some(ref stderr) = rule.log.stderr {
        if rule.log.stdout.as_ref() != Some(stderr) {
            log.push(stderr.clone());
        }
    }

    // Convert expand mode
    let expand = match rule.expand_mode {
        ox_core::model::ExpandMode::Product => None, // default in Snakemake
        ox_core::model::ExpandMode::Zip => Some("zip".to_string()),
    };

    // Emit diagnostics for features without Snakemake equivalents
    if !rule.tags.is_empty() {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Info,
            message: format!(
                "Rule '{}': tags have no Snakemake equivalent (ignored)",
                rule.name
            ),
            line: None,
        });
    }

    if rule.when.is_some() {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warning,
            message: format!(
                "Rule '{}': 'when' guard has no Snakemake equivalent (ignored)",
                rule.name
            ),
            line: None,
        });
    }

    if rule.timeout.is_some() {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Info,
            message: format!(
                "Rule '{}': timeout has no Snakemake equivalent (ignored)",
                rule.name
            ),
            line: None,
        });
    }

    if rule.priority.is_some() {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Info,
            message: format!(
                "Rule '{}': priority has no Snakemake equivalent (ignored)",
                rule.name
            ),
            line: None,
        });
    }

    RuleIR {
        name: rule.name.to_string(),
        inputs,
        outputs,
        execution,
        threads,
        resources,
        environment,
        wildcard_constraints,
        params,
        log,
        source_line: None,
        expand,
    }
}

fn resource_value_to_string(v: &ResourceValue) -> String {
    match v {
        ResourceValue::Int(n) => n.to_string(),
        ResourceValue::Float(f) => f.to_string(),
        ResourceValue::Str(s) => s.clone(),
    }
}

// ---------------------------------------------------------------------------
// Config YAML generation
// ---------------------------------------------------------------------------

/// Generate a `config.yaml` string from the IR config values.
///
/// This is needed because Snakemake reads config from YAML files,
/// while OxyMake embeds config inline in the Oxymakefile.
pub fn generate_config_yaml(ir: &WorkflowIR) -> Option<String> {
    if ir.config_values.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str("# Generated by ox export snakemake\n");

    for entry in &ir.config_values {
        if entry.values.len() == 1 {
            let val = &entry.values[0];
            if val.starts_with('#') {
                out.push_str(&format!("{}: []  {}\n", entry.key, val));
            } else {
                out.push_str(&format!("{}: \"{}\"\n", entry.key, val));
            }
        } else {
            out.push_str(&format!("{}:\n", entry.key));
            for v in &entry.values {
                out.push_str(&format!("  - \"{}\"\n", v));
            }
        }
    }

    Some(out)
}
