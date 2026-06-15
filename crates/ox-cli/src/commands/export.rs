//! Implementation of the `ox export` command.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

use ox_translate::export::{export_snakemake, export_wdl, generate_config_yaml, workflow_to_ir};

#[derive(clap::Args)]
pub struct ExportArgs {
    /// Target format to export to
    #[arg(value_enum)]
    pub format: ExportFormat,

    /// Path to the Oxymakefile to export (default: Oxymakefile.toml)
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,

    /// Write output to a file instead of stdout
    #[arg(short = 'o', long)]
    pub output: Option<String>,
}

#[derive(clap::ValueEnum, Clone)]
pub enum ExportFormat {
    /// Export to Snakemake format
    Snakemake,
    /// Export to WDL (Workflow Description Language) format
    Wdl,
}

pub fn cmd_export(args: ExportArgs) -> Result<()> {
    match args.format {
        ExportFormat::Snakemake => export_to_snakemake(args.file, args.output),
        ExportFormat::Wdl => export_to_wdl(args.file, args.output),
    }
}

fn export_to_snakemake(oxymakefile_path: String, output: Option<String>) -> Result<()> {
    let path = PathBuf::from(&oxymakefile_path);
    let snakefile =
        export_snakemake(&path).with_context(|| format!("failed to export {}", path.display()))?;

    // Also generate config.yaml if needed
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let workflow = ox_format::parse::parse_workflow(&content, &path)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {}", path.display(), e))?;
    let ir = workflow_to_ir(&workflow);
    let config_yaml = generate_config_yaml(&ir);

    match &output {
        Some(out_path) => {
            std::fs::write(out_path, &snakefile)
                .with_context(|| format!("failed to write {out_path}"))?;
            eprintln!("wrote {} ({} rule(s))", out_path, ir.rules.len());

            // Write config.yaml alongside if we generated one
            if let Some(config_content) = config_yaml {
                let config_path = PathBuf::from(out_path)
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("config.yaml");
                let config_path_str = config_path.to_string_lossy().to_string();
                std::fs::write(&config_path, config_content)
                    .with_context(|| format!("failed to write {config_path_str}"))?;
                eprintln!("wrote {}", config_path_str);
            }
        }
        None => {
            std::io::stdout().write_all(snakefile.as_bytes())?;
        }
    }

    // Print diagnostics to stderr
    for diag in &ir.diagnostics {
        eprintln!("{:?}: {}", diag.level, diag.message);
    }

    Ok(())
}

fn export_to_wdl(oxymakefile_path: String, output: Option<String>) -> Result<()> {
    let path = PathBuf::from(&oxymakefile_path);
    let wdl_content =
        export_wdl(&path).with_context(|| format!("failed to export {}", path.display()))?;

    // Get diagnostics
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let workflow = ox_format::parse::parse_workflow(&content, &path)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {}", path.display(), e))?;
    let ir = workflow_to_ir(&workflow);

    match &output {
        Some(out_path) => {
            std::fs::write(out_path, &wdl_content)
                .with_context(|| format!("failed to write {out_path}"))?;
            eprintln!("wrote {} ({} rule(s))", out_path, ir.rules.len());
        }
        None => {
            std::io::stdout().write_all(wdl_content.as_bytes())?;
        }
    }

    // Print diagnostics to stderr
    for diag in &ir.diagnostics {
        eprintln!("{:?}: {}", diag.level, diag.message);
    }

    Ok(())
}
