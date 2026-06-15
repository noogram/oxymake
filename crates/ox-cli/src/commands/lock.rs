//! Implementation of `ox lock` — generate and verify reproducibility lockfiles.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use ox_core::model::ContentHash;
use ox_lock::{DriftReport, verify_lockfile, write_lockfile};

use super::common::load_workflow;

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct LockArgs {
    #[command(subcommand)]
    pub action: LockAction,
}

#[derive(clap::Subcommand)]
pub enum LockAction {
    /// Generate an ox.lock file from the current workflow state
    Generate {
        /// Oxymakefile path
        #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
        file: String,

        /// Output lockfile path (default: ox.lock next to the Oxymakefile)
        #[arg(short = 'o', long)]
        output: Option<String>,

        /// Output structured JSON instead of human-readable text
        #[arg(long)]
        json: bool,
    },

    /// Verify the current state against an existing ox.lock
    Verify {
        /// Oxymakefile path
        #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
        file: String,

        /// Lockfile path (default: ox.lock next to the Oxymakefile)
        #[arg(short = 'l', long)]
        lockfile: Option<String>,

        /// Output structured JSON instead of human-readable text
        #[arg(long)]
        json: bool,
    },
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

pub fn cmd_lock(args: LockArgs) -> Result<()> {
    match args.action {
        LockAction::Generate { file, output, json } => {
            cmd_lock_generate(&file, output.as_deref(), json)
        }
        LockAction::Verify {
            file,
            lockfile,
            json,
        } => cmd_lock_verify(&file, lockfile.as_deref(), json),
    }
}

fn cmd_lock_generate(oxymakefile_path: &str, output: Option<&str>, json: bool) -> Result<()> {
    let path = Path::new(oxymakefile_path);
    let workflow = load_workflow(path)?;

    let oxymakefile_content =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let oxymakefile_hash = ContentHash::from(blake3::hash(oxymakefile_content.as_bytes()));

    let input_hashes = hash_input_files(path, &workflow.rules);

    let lockfile_path = match output {
        Some(p) => PathBuf::from(p),
        None => path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .join("ox.lock"),
    };

    let lockfile = write_lockfile(
        &lockfile_path,
        &oxymakefile_hash,
        &workflow.rules,
        &input_hashes,
    )?;

    if json {
        let out = serde_json::json!({
            "lockfile_path": lockfile_path.display().to_string(),
            "rules_locked": lockfile.rules.len(),
            "inputs_hashed": lockfile.inputs.len(),
            "environments": lockfile.environments.len(),
            "platform": {
                "os": lockfile.platform.os,
                "arch": lockfile.platform.arch,
            },
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "Lockfile written to {} ({} rules, {} inputs, {} environments)",
            lockfile_path.display(),
            lockfile.rules.len(),
            lockfile.inputs.len(),
            lockfile.environments.len(),
        );
    }

    Ok(())
}

fn cmd_lock_verify(oxymakefile_path: &str, lockfile: Option<&str>, json: bool) -> Result<()> {
    let path = Path::new(oxymakefile_path);
    let workflow = load_workflow(path)?;

    let oxymakefile_content =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let oxymakefile_hash = ContentHash::from(blake3::hash(oxymakefile_content.as_bytes()));

    let input_hashes = hash_input_files(path, &workflow.rules);

    let lockfile_path = match lockfile {
        Some(p) => PathBuf::from(p),
        None => path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .join("ox.lock"),
    };

    let report: DriftReport = verify_lockfile(
        &lockfile_path,
        &oxymakefile_hash,
        &workflow.rules,
        &input_hashes,
    )
    .with_context(|| format!("cannot verify lockfile: {}", lockfile_path.display()))?;

    if json {
        let mismatches: Vec<serde_json::Value> = report
            .drifts
            .iter()
            .map(|d| {
                serde_json::json!({
                    "kind": format!("{:?}", d.kind),
                    "message": d.message,
                })
            })
            .collect();
        let out = serde_json::json!({
            "matches": report.is_clean(),
            "mismatches": mismatches,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        if !report.is_clean() {
            std::process::exit(1);
        }
    } else if report.is_clean() {
        println!("No drift detected — lockfile is up to date.");
    } else {
        println!("Drift detected ({} change(s)):\n", report.len());
        for drift in &report.drifts {
            println!("  - {}", drift.message);
        }
        std::process::exit(1);
    }

    Ok(())
}

/// Hash input files referenced by rules, relative to the Oxymakefile directory.
fn hash_input_files(
    oxymakefile_path: &Path,
    rules: &[ox_core::model::Rule],
) -> BTreeMap<String, ContentHash> {
    let base_dir = oxymakefile_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut hashes = BTreeMap::new();

    for rule in rules {
        for input in &rule.inputs {
            let pattern = input.pattern.as_str();
            // Only hash concrete files (no wildcards).
            if pattern.contains('{') || pattern.contains('*') {
                continue;
            }
            let file_path = base_dir.join(pattern);
            if file_path.is_file() {
                if let Ok(data) = std::fs::read(&file_path) {
                    let hash = ContentHash::from(blake3::hash(&data));
                    hashes.insert(pattern.to_string(), hash);
                }
            }
        }
    }

    hashes
}
