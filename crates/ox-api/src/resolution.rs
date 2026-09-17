//! Shared source discovery and provenance-aware resolution for CLI, API and MCP.

use ox_cache::{CacheStore, hash_file};
use ox_core::resolver::Config;
use ox_core::wildcard::{CompiledPattern, Pattern, Segment};
use ox_format::parse::Workflow;
use std::path::{Path, PathBuf};

/// Directory against which workflow paths are interpreted.
pub fn workflow_directory(file: &Path) -> &Path {
    file.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Discover source files relative to the Oxymakefile's directory.
///
/// Rule outputs are excluded even when they exist on disk, so the resolver
/// cannot mistake generated files for sources and truncate the job graph.
/// Every resolution takes a fresh filesystem snapshot, so long-lived callers
/// observe changes inside subdirectories as well as at the workflow root.
///
/// One exception, when `honour_adopted` is set: an output that `ox
/// cache-import` adopted is the engine's explicit assertion that its verified
/// bytes may stand in for a producer whose source inputs do not exist in this
/// checkout. Such an output stays a resolver leaf — otherwise every command
/// would try to rebuild it from inputs this machine never had.
pub fn discover_source_files(
    oxymakefile_path: &Path,
    workflow: &Workflow,
    config: &Config,
    honour_adopted: bool,
) -> Vec<PathBuf> {
    let base = workflow_directory(oxymakefile_path);
    // Match actual paths as the resolver does: explicit targets can supply
    // wildcard values absent from config lists, so expand_pattern cannot
    // enumerate all generated outputs. Compile once, including constraints.
    let output_patterns: Vec<CompiledPattern> = workflow
        .rules
        .iter()
        .flat_map(|rule| {
            rule.outputs.iter().filter_map(|output| {
                let expanded = ox_format::targets::substitute_config_refs(
                    output.pattern.as_str(),
                    &config.scalars,
                );
                // Invalid patterns are reported by validation/resolution.
                let pattern = Pattern::parse(&expanded).ok()?;
                CompiledPattern::new(pattern, &rule.wildcard_constraints).ok()
            })
        })
        .collect();
    // A single scan finds candidate literal prefixes. Only those candidates
    // need full matching (including repeated-wildcard equality). Patterns
    // beginning with a wildcard have an empty prefix and remain candidates.
    let prefixes = regex::RegexSet::new(output_patterns.iter().map(|pattern| {
        let prefix = match pattern.pattern().segments().first() {
            Some(Segment::Literal(prefix)) => prefix.as_str(),
            _ => "",
        };
        format!("^{}", regex::escape(prefix))
    }))
    .ok();
    let cache = honour_adopted
        .then(|| CacheStore::open(&base.join(".oxymake")).ok())
        .flatten();
    let mut files = crate::discover::discover_existing_files_fresh(oxymakefile_path);
    files.retain(|path| {
        let text = path.to_string_lossy();
        let is_output = match &prefixes {
            Some(prefixes) => {
                let matches = prefixes.matches(&text);
                matches.matched_any()
                    && matches
                        .iter()
                        .any(|index| output_patterns[index].resolve(&text).is_some())
            }
            // A very large set can exceed the regex compiler's size limit.
            // Preserve correctness rather than silently accepting outputs.
            None => output_patterns
                .iter()
                .any(|pattern| pattern.resolve(&text).is_some()),
        };
        if !is_output {
            return true;
        }
        let Some(entry) = cache
            .as_ref()
            .and_then(|cache| cache.entry_for_output(path))
        else {
            return false;
        };
        let Some(provenance) = &entry.provenance else {
            return false;
        };
        entry.adopted
            && provenance
                .input_hashes
                .iter()
                .any(|(input, _)| !base.join(input).exists())
            && entry.output_hashes.iter().all(|(output, expected)| {
                hash_file(&base.join(output)).is_ok_and(|actual| actual == *expected)
            })
    });
    files
}

/// Keep cache provenance authoritative when resolving hand-maintained files.
/// Verified adopted leaves have already been admitted by discovery; all other
/// cached outputs must resolve their producer, even when its inputs are missing.
pub fn resolve(
    oxymakefile_path: &Path,
    rules: &[ox_core::model::Rule],
    request: &ox_core::resolver::ResolveRequest,
) -> Result<ox_core::resolver::ResolveResult, ox_core::error::DagError> {
    let base = workflow_directory(oxymakefile_path);
    let cache = CacheStore::open(&base.join(".oxymake")).ok();
    ox_core::resolver::resolve_with_source_fallback_at(rules, request, base, &|path| {
        // An absent cache opens as an empty store: unrecorded manual files may
        // be sources. A cache that cannot be opened fails closed because its
        // provenance is unknown. Known outputs always require their producer.
        cache
            .as_ref()
            .is_some_and(|cache| cache.entry_for_output(path).is_none())
    })
}
