//! Implementation of `ox cache-import`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use ox_cache::{CacheStore, current_platform, hash_file};
use ox_core::model::{OutputRef, PlatformScope, ReproducibilityClass};
use ox_core::resolver::{self, ResolveRequest};
use serde::Deserialize;

use super::cache_export::{
    AdoptionEntry, AdoptionManifest, MANIFEST_KIND, MAX_SUPPORTED_MANIFEST_VERSION,
};
use super::common;
use super::run::job_cache_key_from_provenance;

#[derive(clap::Args)]
pub struct CacheImportArgs {
    /// Adoption manifest produced by `ox cache-export`
    pub manifest: String,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,
}

#[derive(Deserialize)]
struct ManifestProbe {
    kind: String,
    format_version: u32,
}

#[derive(Deserialize)]
struct ProducerProbe {
    producer_version: Option<String>,
}

pub fn cmd_cache_import(args: CacheImportArgs) -> Result<()> {
    let bytes = std::fs::read(&args.manifest)
        .with_context(|| format!("failed to read manifest {}", args.manifest))?;
    let probe: ManifestProbe = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse manifest {}", args.manifest))?;
    ensure!(
        probe.kind == MANIFEST_KIND,
        "this is not an adoption manifest (expected kind '{MANIFEST_KIND}')"
    );
    let producer = serde_json::from_slice::<ProducerProbe>(&bytes)
        .ok()
        .and_then(|probe| probe.producer_version)
        .unwrap_or_else(|| "an unknown version".into());
    ensure!(
        probe.format_version <= MAX_SUPPORTED_MANIFEST_VERSION,
        "manifest format version {} was produced by ox {}, but this ox {} supports through \
         version {}; upgrade ox on this machine",
        probe.format_version,
        producer,
        env!("CARGO_PKG_VERSION"),
        MAX_SUPPORTED_MANIFEST_VERSION
    );
    let manifest: AdoptionManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse manifest {}", args.manifest))?;

    let file_path = PathBuf::from(&args.file);
    let workflow = common::load_workflow(&file_path)?;
    let config = common::workflow_config(&workflow);
    let mut cache = CacheStore::open(Path::new(".oxymake")).context("cannot open local cache")?;

    // Validate every entry before mutating cache.db, so a bad multi-entry
    // manifest cannot leave a partially adopted set.
    let mut prepared = Vec::new();
    for entry in &manifest.entries {
        prepared.push(prepare_entry(entry, &workflow.rules, &config)?);
    }

    for (entry, cache_key, output_paths) in prepared {
        let refs: Vec<&Path> = output_paths.iter().map(PathBuf::as_path).collect();
        cache.record_adopted(
            cache_key,
            &refs,
            &entry.provenance,
            &entry.origin_platform,
            entry.platform_scope,
        )?;
    }
    cache.save().context("failed to save cache")?;
    println!("Imported {} cache entry(ies).", manifest.entries.len());
    Ok(())
}

fn prepare_entry<'a>(
    entry: &'a AdoptionEntry,
    rules: &[ox_core::model::Rule],
    config: &ox_core::resolver::Config,
) -> Result<(&'a AdoptionEntry, ox_core::model::ContentHash, Vec<PathBuf>)> {
    ensure!(
        entry.provenance.reproducibility != ReproducibilityClass::NonReproducible,
        "refusing non-reproducible target '{}'",
        entry.target
    );
    if entry.platform_scope == PlatformScope::Exact {
        ensure!(
            entry.origin_platform == current_platform(),
            "target '{}' was produced on {} with exact platform scope (local platform is {})",
            entry.target,
            entry.origin_platform,
            current_platform()
        );
    }

    let existing_files = entry
        .provenance
        .input_hashes
        .iter()
        .map(|(path, _)| PathBuf::from(path))
        .collect();
    let result = resolver::resolve(
        rules,
        &ResolveRequest {
            targets: vec![entry.target.clone()],
            config: config.clone(),
            existing_files,
        },
    )
    .with_context(|| format!("failed to resolve imported target '{}'", entry.target))?;
    let job = result
        .jobs
        .iter()
        .find(|job| {
            job.outputs.iter().any(|output| {
                matches!(&output.reference, OutputRef::File(path) if path == Path::new(&entry.target))
            })
        })
        .with_context(|| format!("manifest target '{}' is not produced by a rule", entry.target))?;

    ensure!(
        job.platform_scope == entry.platform_scope,
        "manifest platform scope for '{}' does not match the local rule",
        entry.target
    );
    ensure!(
        job.reproducibility != ReproducibilityClass::NonReproducible,
        "local rule for '{}' is non-reproducible",
        entry.target
    );

    let expected_paths: BTreeSet<_> = job
        .outputs
        .iter()
        .filter_map(|output| match &output.reference {
            OutputRef::File(path) => Some(path.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let manifest_paths: BTreeSet<_> = entry.outputs.iter().map(|out| out.path.clone()).collect();
    ensure!(
        expected_paths == manifest_paths,
        "manifest output set for '{}' does not match the local rule",
        entry.target
    );

    let mut output_paths = Vec::new();
    for output in &entry.outputs {
        let path = PathBuf::from(&output.path);
        ensure!(
            path.exists(),
            "required output '{}' is missing",
            path.display()
        );
        let actual = hash_file(&path)
            .with_context(|| format!("failed to hash output '{}'", path.display()))?;
        if actual.as_str() != output.content_hash {
            bail!(
                "output hash mismatch for '{}': manifest {}, local {}",
                path.display(),
                output.content_hash,
                actual
            );
        }
        output_paths.push(path);
    }

    let cache_key = job_cache_key_from_provenance(job, &entry.provenance)?;
    Ok((entry, cache_key, output_paths))
}
