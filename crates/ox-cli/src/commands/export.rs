//! Implementation of the `ox export` command.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};

use ox_cache::CacheStore;
use ox_core::model::{ArtifactProvenance, PlatformScope};

use ox_translate::export::{export_snakemake, export_wdl, generate_config_yaml, workflow_to_ir};

#[derive(clap::Args)]
pub struct ExportArgs {
    /// Targets to export, or the legacy translation format (snakemake/wdl)
    pub targets: Vec<String>,

    /// Write a versioned output-adoption manifest to this path
    #[arg(long, value_name = "PATH")]
    pub manifest: Option<String>,

    /// Path to the Oxymakefile to export (default: Oxymakefile.toml)
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,

    /// Write output to a file instead of stdout
    #[arg(short = 'o', long)]
    pub output: Option<String>,
}

pub(crate) const ADOPTION_MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AdoptionManifest {
    pub(crate) format_version: u32,
    pub(crate) entries: Vec<AdoptionEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AdoptionEntry {
    pub(crate) target: String,
    pub(crate) outputs: Vec<AdoptionOutput>,
    pub(crate) provenance: ArtifactProvenance,
    pub(crate) origin_platform: String,
    pub(crate) platform_scope: PlatformScope,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AdoptionOutput {
    pub(crate) path: String,
    pub(crate) content_hash: String,
}

pub fn cmd_export(args: ExportArgs) -> Result<()> {
    if let Some(manifest) = args.manifest {
        ensure!(
            args.output.is_none(),
            "--output cannot be used with --manifest"
        );
        return export_manifest(&args.targets, &manifest);
    }

    ensure!(
        args.targets.len() == 1,
        "specify snakemake or wdl, or use --manifest with one or more targets"
    );
    match args.targets[0].as_str() {
        "snakemake" => export_to_snakemake(args.file, args.output),
        "wdl" => export_to_wdl(args.file, args.output),
        other => bail!("unknown export format '{other}'; expected snakemake or wdl"),
    }
}

fn export_manifest(targets: &[String], destination: &str) -> Result<()> {
    ensure!(!targets.is_empty(), "specify at least one target to export");
    let cache =
        CacheStore::open(std::path::Path::new(".oxymake")).context("cannot open local cache")?;
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for target in targets {
        let entry = cache
            .entry_for_output(std::path::Path::new(target))
            .with_context(|| format!("target '{target}' has no local cache entry"))?;
        if !seen.insert(entry.cache_key.clone()) {
            continue;
        }
        let provenance = entry
            .provenance
            .context("cache entry predates artifact provenance; rebuild it before export")?;
        let origin_platform = entry
            .platform
            .context("cache entry has no producing platform; rebuild it before export")?;
        let platform_scope = entry
            .platform_scope
            .context("cache entry has no platform scope; rebuild it before export")?;
        let outputs = entry
            .output_hashes
            .into_iter()
            .map(|(path, content_hash)| AdoptionOutput {
                path,
                content_hash: content_hash.to_string(),
            })
            .collect();
        entries.push(AdoptionEntry {
            target: target.clone(),
            outputs,
            provenance,
            origin_platform,
            platform_scope,
        });
    }

    let manifest = AdoptionManifest {
        format_version: ADOPTION_MANIFEST_VERSION,
        entries,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    std::fs::write(destination, bytes)
        .with_context(|| format!("failed to write manifest {destination}"))?;
    println!(
        "Exported {} cache entry(ies) to {destination}.",
        manifest.entries.len()
    );
    Ok(())
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
