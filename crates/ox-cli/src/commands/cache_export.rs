//! Implementation of `ox cache-export`.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use ox_cache::CacheStore;
use ox_core::model::{ArtifactProvenance, PlatformScope};
use serde::{Deserialize, Serialize};

pub(crate) const MANIFEST_KIND: &str = "oxymake.cache-adoption-manifest";
pub(crate) const MAX_SUPPORTED_MANIFEST_VERSION: u32 = 1;

#[derive(clap::Args)]
pub struct CacheExportArgs {
    /// Cached output targets to describe
    #[arg(required = true)]
    pub targets: Vec<String>,

    /// Write the manifest to a file instead of stdout
    #[arg(short = 'o', long, value_name = "PATH")]
    pub output: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AdoptionManifest {
    pub(crate) kind: String,
    pub(crate) format_version: u32,
    pub(crate) producer_version: String,
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

pub fn cmd_cache_export(args: CacheExportArgs) -> Result<()> {
    let cache = CacheStore::open(Path::new(".oxymake")).context("cannot open local cache")?;
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for target in &args.targets {
        let entry = cache
            .entry_for_output(Path::new(target))
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
        ensure!(
            provenance.reproducibility != ox_core::model::ReproducibilityClass::NonReproducible,
            "refusing non-reproducible target '{target}'"
        );
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
        kind: MANIFEST_KIND.to_owned(),
        format_version: MAX_SUPPORTED_MANIFEST_VERSION,
        producer_version: env!("CARGO_PKG_VERSION").to_owned(),
        entries,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    match args.output {
        Some(destination) => {
            std::fs::write(&destination, bytes)
                .with_context(|| format!("failed to write manifest {destination}"))?;
            eprintln!(
                "exported {} cache entry(ies) to {destination}",
                manifest.entries.len()
            );
        }
        None => std::io::stdout().write_all(&bytes)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::model::ReproducibilityClass;

    #[test]
    fn manifest_enum_wire_spellings_are_pinned() {
        let entry = |scope, reproducibility| AdoptionEntry {
            target: "out".into(),
            outputs: Vec::new(),
            provenance: ArtifactProvenance {
                input_hashes: Vec::new(),
                job_spec_hash: "spec".into(),
                reproducibility,
            },
            origin_platform: "test/arch".into(),
            platform_scope: scope,
        };
        let manifest = AdoptionManifest {
            kind: MANIFEST_KIND.into(),
            format_version: MAX_SUPPORTED_MANIFEST_VERSION,
            producer_version: "0.3.0".into(),
            entries: vec![
                entry(PlatformScope::Exact, ReproducibilityClass::Deterministic),
                entry(PlatformScope::Any, ReproducibilityClass::SeedDeterministic),
                entry(PlatformScope::Any, ReproducibilityClass::Approximate),
                entry(PlatformScope::Exact, ReproducibilityClass::NonReproducible),
            ],
        };

        insta::assert_snapshot!(serde_json::to_string_pretty(&manifest).unwrap(), @r###"
        {
          "kind": "oxymake.cache-adoption-manifest",
          "format_version": 1,
          "producer_version": "0.3.0",
          "entries": [
            {
              "target": "out",
              "outputs": [],
              "provenance": {
                "input_hashes": [],
                "job_spec_hash": "spec",
                "reproducibility": "deterministic"
              },
              "origin_platform": "test/arch",
              "platform_scope": "exact"
            },
            {
              "target": "out",
              "outputs": [],
              "provenance": {
                "input_hashes": [],
                "job_spec_hash": "spec",
                "reproducibility": "seed_deterministic"
              },
              "origin_platform": "test/arch",
              "platform_scope": "any"
            },
            {
              "target": "out",
              "outputs": [],
              "provenance": {
                "input_hashes": [],
                "job_spec_hash": "spec",
                "reproducibility": "approximate"
              },
              "origin_platform": "test/arch",
              "platform_scope": "any"
            },
            {
              "target": "out",
              "outputs": [],
              "provenance": {
                "input_hashes": [],
                "job_spec_hash": "spec",
                "reproducibility": "non_reproducible"
              },
              "origin_platform": "test/arch",
              "platform_scope": "exact"
            }
          ]
        }
        "###);
    }
}
