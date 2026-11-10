//! Implementation of `ox invalidate` — mark outputs as stale.
//!
//! Removes cache entries so that subsequent `ox run` re-executes the
//! corresponding jobs. Supports filtering by `--rule`, `--output`, or
//! clearing everything with `--all`.

use std::collections::{BTreeSet, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use ox_cache::CacheStore;
use ox_core::dag::RuleGraph;
use ox_core::model::{ContentHash, RuleName};
use ox_core::resolver::ResolveRequest;

use super::common;

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct InvalidateArgs {
    /// Filter by rule name (exact match or /regex/)
    #[arg(long)]
    pub rule: Option<String>,

    /// Specific output file paths to invalidate (repeatable)
    #[arg(long = "output", value_name = "PATH")]
    pub outputs: Vec<String>,

    /// Also invalidate all downstream rules
    #[arg(long)]
    pub cascade: bool,

    /// Invalidate all cache entries
    #[arg(long)]
    pub all: bool,

    /// Show what would be invalidated without modifying the cache
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Keep output files on disk (only clear cache entries).
    ///
    /// By default, `ox invalidate` deletes output files so that subsequent
    /// runs rebuild them regardless of cache validation mode.  With
    /// `--keep-outputs`, only cache metadata is removed (including remote
    /// computation entries when `--cache-remote` is supplied).
    #[arg(long)]
    pub keep_outputs: bool,

    /// Also remove computation entries from this remote directory (never blobs)
    #[arg(long, value_name = "DIR")]
    pub cache_remote: Option<PathBuf>,

    /// Output results as JSON
    #[arg(long)]
    pub json: bool,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,
}

/// Delete output files from disk, returning the count of files actually removed.
fn delete_output_files(paths: &[PathBuf]) -> usize {
    let mut deleted = 0;
    for p in paths {
        if p.exists() {
            if let Err(e) = std::fs::remove_file(p) {
                eprintln!("warning: could not delete {}: {e}", p.display());
            } else {
                deleted += 1;
            }
        }
    }
    deleted
}

/// An addressed entry can be recalled even if its payload is unsupported.
#[derive(serde::Serialize)]
struct RemoteEntry {
    cache_key: String,
    path: PathBuf,
    format_version: Option<u64>,
    newer_version: bool,
}

fn regular_entry_path(path: &Path) -> bool {
    let prefix = path.parent().expect("entry prefix");
    let entries = prefix.parent().expect("entry tree");
    for parent in [entries, prefix] {
        match std::fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.file_type().is_dir() => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
            _ => {
                eprintln!(
                    "warning: skipping remote entry {}: parent {} is not a regular directory",
                    path.display(),
                    parent.display()
                );
                return false;
            }
        }
    }
    // Revalidate a network filesystem's cached negative lookup only after
    // confirming that neither entry-tree directory is a symlink.
    let _ = std::fs::read_dir(prefix).map(|mut entries| entries.next());
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        _ => {
            eprintln!(
                "warning: skipping remote entry {}: not a regular file",
                path.display()
            );
            false
        }
    }
}

fn remote_entries(root: &Path, keys: &BTreeSet<String>) -> Vec<RemoteEntry> {
    let mut entries = Vec::new();
    for key in keys {
        let path = root
            .join("entries")
            .join(&key[..2])
            .join(format!("{key}.json"));
        if !regular_entry_path(&path) {
            continue;
        }
        // Read only bounded diagnostic metadata; recall never depends on parsing.
        let mut bytes = Vec::new();
        if let Ok(file) = std::fs::File::open(&path) {
            // Metadata is optional: a writable parent can permit recall even
            // when this producer cannot read the entry itself.
            let _ = file.take(1024 * 1024).read_to_end(&mut bytes);
        }
        let version = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|v| v.get("format_version").and_then(|v| v.as_u64()));
        entries.push(RemoteEntry {
            cache_key: key.clone(),
            path,
            format_version: version,
            newer_version: version.is_some_and(|v| v > 1),
        });
    }
    entries
}

/// Expand selected rules independently of the presence of their output files.
fn selected_paths(
    args: &InvalidateArgs,
    cache: Option<&CacheStore>,
    current_remote_keys: bool,
) -> Result<BTreeSet<PathBuf>> {
    // Remote recall never adds files or rows to local invalidation. In an
    // empty cache, main's local invalidation deletes nothing.
    if !current_remote_keys && args.cache_remote.is_some() && cache.is_none_or(CacheStore::is_empty)
    {
        return Ok(BTreeSet::new());
    }
    let mut paths: BTreeSet<PathBuf> = args.outputs.iter().map(PathBuf::from).collect();
    if args.all && !current_remote_keys {
        if let Some(cache) = cache {
            paths.extend(cache.all_output_paths());
        }
        return Ok(paths);
    }
    if args.rule.is_none() && !(args.all && current_remote_keys) {
        return Ok(paths);
    }
    let workflow = common::load_workflow(Path::new(&args.file))?;
    let config = common::workflow_config(&workflow);
    let mut rules: HashSet<RuleName> = HashSet::new();
    if let Some(filter) = &args.rule {
        let regex = if filter.starts_with('/') && filter.ends_with('/') && filter.len() >= 2 {
            Some(
                regex::Regex::new(&filter[1..filter.len() - 1])
                    .context("invalid regex in --rule")?,
            )
        } else {
            None
        };
        for rule in &workflow.rules {
            if regex.as_ref().map_or_else(
                || rule.name.as_str() == filter,
                |re| re.is_match(rule.name.as_str()),
            ) {
                rules.insert(rule.name.clone());
            }
        }
        if rules.is_empty() {
            bail!("no rules match '{filter}'");
        }
    }
    if args.all {
        rules.extend(workflow.rules.iter().map(|r| r.name.clone()));
    }
    if args.cascade {
        // As in local invalidation, cascade applies to --rule only. An
        // explicit --output never expands its producer's wildcard siblings.
        let graph =
            RuleGraph::build(workflow.rules.clone()).context("failed to build RuleGraph")?;
        let mut pending: Vec<_> = rules.iter().cloned().collect();
        while let Some(rule) = pending.pop() {
            for downstream in graph.downstream(&rule)? {
                if rules.insert(downstream.clone()) {
                    pending.push(downstream.clone());
                }
            }
        }
    }
    for rule in &workflow.rules {
        if rules.contains(&rule.name) {
            for output in &rule.outputs {
                let mut expanded = Vec::new();
                common::expand_pattern(output.pattern.as_str(), &config, &mut expanded);
                paths.extend(expanded.into_iter().map(PathBuf::from));
            }
        }
    }
    Ok(paths)
}

fn current_keys(
    args: &InvalidateArgs,
    paths: &BTreeSet<PathBuf>,
    cache: Option<&CacheStore>,
) -> Result<BTreeSet<String>> {
    current_keys_with(args, paths, cache, |job| {
        super::run::job_cache_key_with_components(job, None).map(|key| key.cache_key)
    })
}

fn current_keys_with(
    args: &InvalidateArgs,
    paths: &BTreeSet<PathBuf>,
    cache: Option<&CacheStore>,
    mut key_for: impl FnMut(&ox_core::model::ConcreteJob) -> Option<ContentHash>,
) -> Result<BTreeSet<String>> {
    let file = Path::new(&args.file);
    let workflow = common::load_workflow(file)?;
    let config = common::workflow_config(&workflow);
    // Honour adopted inputs just as a cache-enabled run does. With no cache
    // there are no adopted rows, and discovery must not create dry-run state.
    let existing_files = common::discover_source_files(file, &workflow, &config, cache.is_some());
    let request = ResolveRequest {
        targets: paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        config,
        existing_files,
    };
    let mut keys = BTreeSet::new();
    // Resolve the complete selection once; a dependency may occur under many
    // targets but its computation key must only be calculated once.
    match ox_core::resolver::resolve_with_source_fallback_at(
        &workflow.rules,
        &request,
        ox_api::resolution::workflow_directory(file),
        &|path| cache.is_none_or(|cache| cache.entry_for_output(path).is_none()),
    ) {
        Ok(result) => {
            let mut seen = HashSet::new();
            let mut resolved_paths = BTreeSet::new();
            for job in result.jobs {
                let outputs = super::run::output_file_paths(&job);
                if !outputs.iter().any(|p| paths.contains(p)) || !seen.insert(job.id.clone()) {
                    continue;
                }
                resolved_paths.extend(outputs);
                if let Some(key) = key_for(&job) {
                    keys.insert(key.to_string());
                } else {
                    eprintln!(
                        "warning: cannot calculate current remote key for job {}; required input content is unavailable",
                        job.id.as_str()
                    );
                }
            }
            for path in paths.difference(&resolved_paths) {
                eprintln!(
                    "warning: cannot calculate current remote key for {}; no producer job was resolved",
                    path.display()
                );
            }
        }
        Err(error) => {
            for path in paths {
                eprintln!(
                    "warning: cannot calculate current remote key for {}: {error}; stored local keys are still selected",
                    path.display()
                );
            }
        }
    }
    Ok(keys)
}

pub fn cmd_invalidate(args: InvalidateArgs) -> Result<()> {
    if args.cache_remote.is_none() {
        return cmd_invalidate_local(args);
    }
    anyhow::ensure!(
        args.cache_remote.as_ref().is_some_and(|root| root.is_dir()),
        "--cache-remote must name an existing directory"
    );
    let original_dir = std::env::current_dir()?;
    let file = std::path::absolute(&args.file)?;
    let root = ox_api::resolution::workflow_directory(&file);
    let mut args = args;
    args.file = file.to_string_lossy().into_owned();
    args.cache_remote = args.cache_remote.map(std::path::absolute).transpose()?;
    // Cache, declared paths, and key inputs all use the chosen workflow's
    // directory. Restore the caller's directory even when recall fails.
    std::env::set_current_dir(root)?;
    let result = cmd_invalidate_remote(args);
    let restored =
        std::env::set_current_dir(original_dir).context("cannot restore invocation directory");
    result.and(restored)
}

fn cmd_invalidate_remote(args: InvalidateArgs) -> Result<()> {
    if !args.all && args.rule.is_none() && args.outputs.is_empty() {
        bail!("specify --rule <name>, --output <path>, or --all");
    }
    let cache_dir = Path::new(".oxymake");
    let mut cache = if cache_dir.join("cache/cache.db").exists()
        || cache_dir.join("cache/manifest.json").exists()
    {
        Some(CacheStore::open(cache_dir).context("cannot open local cache")?)
    } else {
        None
    };
    let paths = selected_paths(&args, cache.as_ref(), false)?;
    let refs: Vec<_> = paths.iter().map(|p| p.as_path()).collect();
    let local_keys = cache
        .as_ref()
        .map(|cache| cache.invalidation_keys(if args.all { None } else { Some(&refs) }))
        .transpose()?
        .unwrap_or_default();
    let mut keys: BTreeSet<String> = local_keys.iter().map(ContentHash::to_string).collect();
    let remote = if let Some(root) = &args.cache_remote {
        let current_paths = selected_paths(&args, cache.as_ref(), true)?;
        keys.extend(current_keys(&args, &current_paths, cache.as_ref())?);
        remote_entries(root, &keys)
    } else {
        Vec::new()
    };

    if !args.dry_run {
        // Recall before deleting inputs/outputs needed to compute current keys.
        // A deletion error is actionable, never reported as a successful recall.
        for entry in &remote {
            if !regular_entry_path(&entry.path) {
                continue;
            }
            match std::fs::remove_file(&entry.path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("cannot remove remote entry {}", entry.path.display())
                    });
                }
            }
        }
    }
    let removed = if args.dry_run {
        local_keys.len()
    } else {
        if let Some(cache) = cache.as_mut() {
            if args.all {
                cache.clear();
            } else {
                cache.invalidate(&refs);
            }
            cache.save().context("failed to save cache manifest")?;
        }
        local_keys.len()
    };
    let output_paths: Vec<_> = paths.iter().cloned().collect();
    let deleted = if args.dry_run || args.keep_outputs {
        0
    } else {
        delete_output_files(&output_paths)
    };
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "invalidated_entries": removed, "output_paths": output_paths,
                "deleted_files": deleted, "dry_run": args.dry_run,
                "remote_entries_found": remote.len(), "remote_entries": remote,
            })
        );
    } else {
        let action = if args.dry_run {
            "Would invalidate"
        } else {
            "Invalidated"
        };
        println!("{action} {removed} cache entry(ies).");
        for path in &paths {
            println!("  {}", path.display());
        }
        if args.dry_run && !args.keep_outputs {
            let count = paths.iter().filter(|p| p.exists()).count();
            if count > 0 {
                println!("Would delete {count} output file(s).");
            }
        } else if deleted > 0 {
            println!("Deleted {deleted} output file(s).");
        }
        if remote.is_empty() {
            println!("No remote entries found for the selected keys.");
        }
        for entry in &remote {
            let action = if args.dry_run {
                "Would remove"
            } else {
                "Removed"
            };
            let version = if entry.newer_version {
                format!(" (newer format version {})", entry.format_version.unwrap())
            } else {
                String::new()
            };
            println!("{action} remote entry {}{version}", entry.path.display());
        }
    }
    Ok(())
}

// Keep the established local-only reporting and mutation contract.
fn cmd_invalidate_local(args: InvalidateArgs) -> Result<()> {
    let oxymake_dir = PathBuf::from(".oxymake");
    let mut cache = CacheStore::open(&oxymake_dir)
        .context("cannot open cache (is .oxymake/cache/ present?)")?;

    if cache.is_empty() {
        println!("Cache is empty — nothing to invalidate.");
        return Ok(());
    }

    // --all: clear everything
    if args.all {
        let count = cache.len();
        let output_paths = if !args.keep_outputs {
            cache.all_output_paths()
        } else {
            Vec::new()
        };
        if args.dry_run {
            println!("Would invalidate all {count} cache entries.");
            if !output_paths.is_empty() {
                println!("Would delete {} output file(s).", output_paths.len());
            }
            return Ok(());
        }
        cache.clear();
        cache.save().context("failed to save cache manifest")?;
        let deleted = delete_output_files(&output_paths);
        println!("Invalidated all {count} cache entries.");
        if deleted > 0 {
            println!("Deleted {deleted} output file(s).");
        }
        return Ok(());
    }

    // Must specify at least --rule or --output
    if args.rule.is_none() && args.outputs.is_empty() {
        bail!("specify --rule <name>, --output <path>, or --all");
    }

    let paths_to_invalidate = selected_paths(&args, Some(&cache), false)?;

    if paths_to_invalidate.is_empty() {
        println!("No output paths found to invalidate.");
        return Ok(());
    }

    // Perform invalidation
    let path_refs: Vec<&std::path::Path> =
        paths_to_invalidate.iter().map(|p| p.as_path()).collect();

    if args.dry_run {
        println!(
            "Would invalidate cache entries referencing {} output(s):",
            path_refs.len()
        );
        for p in &paths_to_invalidate {
            println!("  {}", p.display());
        }
        if !args.keep_outputs {
            let existing: Vec<_> = paths_to_invalidate.iter().filter(|p| p.exists()).collect();
            if !existing.is_empty() {
                println!("Would delete {} output file(s).", existing.len());
            }
        }
        return Ok(());
    }

    let removed = cache.invalidate(&path_refs);
    cache.save().context("failed to save cache manifest")?;

    let deleted = if !args.keep_outputs {
        let paths_vec: Vec<PathBuf> = paths_to_invalidate.iter().cloned().collect();
        delete_output_files(&paths_vec)
    } else {
        0
    };

    if args.json {
        let invalidated: Vec<&str> = paths_to_invalidate
            .iter()
            .map(|p| p.to_str().unwrap_or(""))
            .collect();
        let obj = serde_json::json!({
            "invalidated_entries": removed,
            "output_paths": invalidated,
            "deleted_files": deleted,
        });
        println!("{}", serde_json::to_string(&obj).unwrap_or_default());
    } else if removed > 0 || deleted > 0 {
        if removed > 0 {
            println!("Invalidated {removed} cache entry(ies).");
        }
        if deleted > 0 {
            println!("Deleted {deleted} output file(s).");
        }
        for p in &paths_to_invalidate {
            println!("  {}", p.display());
        }
    } else {
        println!("No cache entries matched the specified outputs.");
    }

    Ok(())
}

#[cfg(test)]
mod review_tests {
    use super::*;

    #[test]
    fn selected_jobs_are_keyed_once_across_all_targets() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        std::fs::write(&first, "first").unwrap();
        let file = dir.path().join("Oxymakefile.toml");
        std::fs::write(
            &file,
            format!(
                r#"ox_version="0.1"
[rule.first]
output=["{}"]
shell="true"
[rule.second]
input=["{}"]
output=["{}"]
shell="true"
"#,
                first.display(),
                first.display(),
                second.display()
            ),
        )
        .unwrap();
        let args = InvalidateArgs {
            rule: None,
            outputs: vec![],
            cascade: false,
            all: true,
            dry_run: true,
            keep_outputs: true,
            cache_remote: Some(dir.path().into()),
            json: false,
            file: file.to_string_lossy().into_owned(),
        };
        let paths = BTreeSet::from([first, second]);
        let mut counts = std::collections::BTreeMap::new();
        current_keys_with(&args, &paths, None, |job| {
            *counts.entry(job.id.as_str().to_owned()).or_insert(0) += 1;
            Some(ContentHash::from(blake3::hash(job.id.as_str().as_bytes())))
        })
        .unwrap();
        assert_eq!(
            counts,
            std::collections::BTreeMap::from([("first".into(), 1), ("second".into(), 1)])
        );
    }
}
