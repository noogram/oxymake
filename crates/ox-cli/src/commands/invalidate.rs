//! Implementation of `ox invalidate` — mark outputs as stale.
//!
//! Removes cache entries so that subsequent `ox run` re-executes the
//! corresponding jobs. Supports filtering by `--rule`, `--output`, or
//! clearing everything with `--all`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use ox_cache::CacheStore;
use ox_core::dag::RuleGraph;
use ox_core::model::RuleName;

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
    /// `--keep-outputs`, only the cache database entries are removed.
    #[arg(long)]
    pub keep_outputs: bool,

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

pub fn cmd_invalidate(args: InvalidateArgs) -> Result<()> {
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

    // Collect output paths to invalidate
    let mut paths_to_invalidate: BTreeSet<PathBuf> = BTreeSet::new();

    // --output: direct file paths
    for p in &args.outputs {
        let path = PathBuf::from(p);
        paths_to_invalidate.insert(path);
    }

    // --rule: resolve the workflow to find output file paths for matching rules
    if let Some(ref rule_filter) = args.rule {
        let file_path = PathBuf::from(&args.file);
        let workflow = common::load_workflow(&file_path)?;

        // Collect matching rule names
        let is_regex = rule_filter.starts_with('/') && rule_filter.ends_with('/');
        let matching_rules: Vec<RuleName> = if is_regex {
            let pattern = &rule_filter[1..rule_filter.len() - 1];
            let re = regex::Regex::new(pattern).context("invalid regex in --rule")?;
            workflow
                .rules
                .iter()
                .filter(|r| re.is_match(r.name.as_str()))
                .map(|r| r.name.clone())
                .collect()
        } else {
            workflow
                .rules
                .iter()
                .filter(|r| r.name.as_str() == rule_filter)
                .map(|r| r.name.clone())
                .collect()
        };

        if matching_rules.is_empty() {
            bail!("no rules match '{rule_filter}'");
        }

        // Cascade: also collect downstream rules
        let mut all_rules = matching_rules.clone();
        if args.cascade {
            let rule_graph =
                RuleGraph::build(workflow.rules.clone()).context("failed to build RuleGraph")?;
            let mut to_visit: Vec<RuleName> = matching_rules.clone();
            let mut visited: BTreeSet<String> = BTreeSet::new();
            for r in &matching_rules {
                visited.insert(r.as_str().to_string());
            }
            while let Some(current) = to_visit.pop() {
                for downstream in rule_graph.downstream(&current)? {
                    let name = downstream.as_str().to_string();
                    if visited.insert(name) {
                        all_rules.push(downstream.clone());
                        to_visit.push(downstream.clone());
                    }
                }
            }
        }

        // Expand the matched rules' output patterns directly into concrete
        // paths.  We intentionally bypass the resolver here: after a
        // successful `ox run`, the output files already exist on disk, so
        // the resolver would treat them as source files and never create
        // jobs for them — causing invalidation to silently find nothing.
        let config = common::workflow_config(&workflow);
        for rule_name in &all_rules {
            if let Some(rule) = workflow.rules.iter().find(|r| r.name == *rule_name) {
                for output in &rule.outputs {
                    let pattern_str = output.pattern.as_str();
                    if pattern_str.contains('{') {
                        let mut expanded = Vec::new();
                        common::expand_pattern(pattern_str, &config, &mut expanded);
                        for p in expanded {
                            paths_to_invalidate.insert(PathBuf::from(p));
                        }
                    } else {
                        paths_to_invalidate.insert(PathBuf::from(pattern_str));
                    }
                }
            }
        }
    }

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
