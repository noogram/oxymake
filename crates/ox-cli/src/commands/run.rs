//! Implementation of the `ox run` command.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use tokio::sync::Mutex;

use ox_cache::{
    CacheHitStatus, CacheKeySpec, CacheStore, CacheValidation, compute_cache_key, current_platform,
    env_spec_content_hash, hash_file, workflow_relative_path,
};
use ox_cache_remote::{DirectoryCache, RemoteCache};
use ox_core::dag::RuleGraph;
use ox_core::disk_writer::spawn_disk_writer_confined;
use ox_core::event::EventBus;
use ox_core::hashing::{hash_kv_map, update_field, update_opt_field};
use ox_core::job_graph::JobGraph;
use ox_core::model::{
    ConcreteJob, ContentHash, Event, ExecutionBlock, JobId, OutputRef, RunReason,
};
use ox_core::resolver::{self, ResolveRequest};
use ox_core::scheduler::{self, FailedJobDetail, SchedulerConfig};
use ox_core::traits::benchmark::{self, BenchmarkSink};
use ox_core::traits::cache::CacheCheck;
use ox_core::traits::executor::{ExecContext, Executor, JobResult};
use ox_exec_local::executor::LocalExecutor;
use ox_exec_ray::{RayConfig, RayExecutor};
use ox_exec_slurm::executor::{SlurmConfig, SlurmExecutor};
use ox_plan::critical_path::CriticalPathPass;

use super::common;

/// Lightweight phase timer for `--timings` output.
struct PhaseTimer {
    enabled: bool,
    phases: Vec<(&'static str, std::time::Duration)>,
    lap: std::time::Instant,
}

impl PhaseTimer {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            phases: Vec::new(),
            lap: std::time::Instant::now(),
        }
    }

    fn mark(&mut self, name: &'static str) {
        if self.enabled {
            let elapsed = self.lap.elapsed();
            self.phases.push((name, elapsed));
            self.lap = std::time::Instant::now();
        }
    }

    fn print(&self) {
        if !self.enabled || self.phases.is_empty() {
            return;
        }
        let total: std::time::Duration = self.phases.iter().map(|(_, d)| *d).sum();
        eprintln!("\n--- Timings ---");
        for (name, dur) in &self.phases {
            eprintln!("  {:<30} {:>8.1}ms", name, dur.as_secs_f64() * 1000.0);
        }
        eprintln!("  {:<30} {:>8.1}ms", "TOTAL", total.as_secs_f64() * 1000.0);
    }
}

#[derive(clap::Args)]
#[command(after_long_help = "\
Exit codes:
  0  all requested jobs succeeded (or were already cached)
  1  at least one job failed, or a runtime error occurred
  2  command-line usage error

Machine output:
  --json emits NDJSON events on stdout (one JSON object per line);
  --report-json <path> writes the same stream to a file. Event types are
  listed under `ox subscribe --help`.")]
pub struct RunArgs {
    /// Target files or patterns to build
    pub targets: Vec<String>,

    /// Maximum concurrent jobs (must be at least 1)
    #[arg(
        short = 'j',
        long,
        default_value = "1",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
    )]
    pub jobs: usize,

    /// Filter by rule name (exact or /regex/)
    #[arg(long)]
    pub rule: Option<String>,

    /// Output NDJSON events
    #[arg(long)]
    pub json: bool,

    /// Write NDJSON events to this file (one JSON object per line)
    #[arg(long, value_name = "PATH")]
    pub report_json: Option<String>,

    /// Show what would run without executing
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Continue on independent branches after failure
    #[arg(short = 'k', long)]
    pub keep_going: bool,

    /// Annotate this run
    #[arg(long)]
    pub note: Option<String>,

    /// Override config or resource values
    #[arg(long = "set", value_name = "KEY=VALUE")]
    pub overrides: Vec<String>,

    /// Executor backend
    #[arg(long, default_value = "local")]
    pub executor: String,

    /// SLURM partition (only with --executor slurm)
    #[arg(long)]
    pub partition: Option<String>,

    /// SLURM account (only with --executor slurm)
    #[arg(long)]
    pub account: Option<String>,

    /// SLURM Quality of Service (only with --executor slurm)
    #[arg(long)]
    pub qos: Option<String>,

    /// slurmrestd API URL for REST mode (only with --executor slurm).
    ///
    /// When set, the SLURM executor uses the REST API instead of CLI commands
    /// (sbatch/sacct/squeue). Example: http://localhost:6820
    #[arg(long)]
    pub slurm_api: Option<String>,

    /// Ray dashboard address (only with --executor ray, default: http://127.0.0.1:8265)
    #[arg(long)]
    pub ray_address: Option<String>,

    /// Submit DAG to remote executor and stream progress (only with --executor ray)
    ///
    /// Without --follow, `ox run --executor ray` submits the DAG and returns
    /// immediately. With --follow, it submits and then polls until completion.
    #[arg(long)]
    pub follow: bool,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,

    /// Disable the content-addressable cache (re-execute everything)
    #[arg(long)]
    pub no_cache: bool,

    /// Cache validation strategy: mtime, mtime+hash (default), or hash
    ///
    /// `mtime` checks timestamps only and never verifies content — opt-in
    /// only, avoid on shared caches. `mtime+hash` re-hashes a file only when
    /// its timestamp or size changed. `hash` always verifies content.
    ///
    /// Resolution order (highest wins): this flag, the OX_CACHE_VALIDATION
    /// environment variable, `cache_validation` under `[config]` in the
    /// Oxymakefile, `cache_validation` in `~/.config/oxymake/config.toml`
    /// (or `$XDG_CONFIG_HOME/oxymake/config.toml`), then the built-in
    /// default `mtime+hash`.
    #[arg(long, value_name = "STRATEGY")]
    pub cache_validation: Option<String>,

    /// Shared directory for content-addressed cache artifacts.
    ///
    /// Existing local cache entries can restore missing outputs from this
    /// directory. Remote caches always use content-hash validation.
    #[arg(long, value_name = "DIR")]
    pub cache_remote: Option<PathBuf>,

    /// Verbose output (-v: job start/end/duration/exit codes, -vv: also show stdout/stderr)
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Run only up to this target (include target and all its dependencies)
    #[arg(long, value_name = "TARGET")]
    pub until: Option<String>,

    /// Omit this target and all its downstream dependents
    #[arg(long, value_name = "TARGET")]
    pub omit_from: Option<String>,

    /// Mark outputs as up-to-date without running (like make --touch)
    #[arg(long)]
    pub touch: bool,

    /// Force re-execution of jobs matching this rule, regardless of cache (repeatable, exact or /regex/)
    #[arg(long, value_name = "RULE")]
    pub forcerun: Vec<String>,

    /// Named profile to apply (defined in [profile.NAME] sections of the Oxymakefile)
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Print per-phase timing breakdown to stderr
    #[arg(long)]
    pub timings: bool,

    /// Open dashboard in browser after submitting DAG (Ray executor)
    #[arg(long)]
    pub open_dashboard: bool,

    /// Maximum bytes of in-memory materialization before eviction triggers.
    ///
    /// When set to a non-zero value, the scheduler keeps critical-path output
    /// data in memory (Stage 2 optimization) and evicts the largest outputs
    /// first when the budget is exceeded (Belady optimal eviction).
    ///
    /// Accepts human-readable sizes: `512M`, `1G`, `2G`, `0` (disabled).
    /// Default: `0` (disabled — all data flows through disk).
    #[arg(long, value_name = "SIZE", default_value = "0")]
    pub memory_budget: String,

    /// Enable warm Python workers for call-mode jobs (Stage 5).
    ///
    /// Modes:
    /// - `fork`: fork-after-import (state isolation, but JAX JIT cache lost)
    /// - `persistent`: same process reused (JIT cache persists, risk of state leak)
    ///
    /// Omit to disable warm workers (cold subprocess per job).
    #[arg(long, value_name = "MODE")]
    pub warm_workers: Option<String>,
}

// ---------------------------------------------------------------------------
// Cache helpers
// ---------------------------------------------------------------------------

/// Extract a string representation of the execution block for hashing.
/// Wait for the next shutdown signal: SIGINT (Ctrl+C) or SIGTERM
/// (`ox cancel`, `kill`). Both take the same graceful path (B8).
#[cfg(unix)]
async fn wait_for_shutdown_signal(sigterm: &mut Option<tokio::signal::unix::Signal>) {
    match sigterm {
        Some(st) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = st.recv() => {}
            }
        }
        None => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal(_sigterm: &mut ()) {
    let _ = tokio::signal::ctrl_c().await;
}

fn execution_source(job: &ConcreteJob) -> String {
    // Serialize the execution block deterministically via serde_json.
    // Falls back to Display if serialization fails.
    serde_json::to_string(&job.execution).unwrap_or_else(|_| job.execution.to_string())
}

/// Collect the file-system paths from a job's outputs (only File outputs).
fn output_file_paths(job: &ConcreteJob) -> Vec<PathBuf> {
    job.outputs
        .iter()
        .filter_map(|o| match &o.reference {
            OutputRef::File(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn input_file_paths(job: &ConcreteJob) -> Vec<PathBuf> {
    job.inputs
        .iter()
        .filter_map(|i| match &i.reference {
            OutputRef::File(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

/// Components of a cache key computation, preserving intermediate hashes
/// for provenance tracking (Stage 2).
struct CacheKeyComponents {
    /// The final cache key (BLAKE3 of all components).
    cache_key: ContentHash,
    /// Content hashes of each input file, paired with their path.
    input_hashes: Vec<(String, String)>,
    /// BLAKE3 hash of the job specification (rule source + params + env).
    job_spec_hash: String,
}

/// Compute cache key for a job, returning components for provenance tracking.
///
/// When `store` is provided, input file hashes use the mtime-based fast path:
/// if a file's mtime+size haven't changed since the last run, the stored
/// BLAKE3 hash is reused without re-reading the file. This reduces cache-key
/// computation for large, unchanged inputs from full-file I/O to a single
/// `stat()` call.
///
/// See [`job_cache_key`] for the simple version that discards components.
fn job_cache_key_with_components(
    job: &ConcreteJob,
    mut store: Option<&mut CacheStore>,
) -> Option<CacheKeyComponents> {
    // OxyMake defines the workflow root as the execution directory, not the
    // parent of the file passed through `-f`. This is the sole root used when
    // converting input paths for portable cache keys.
    let execution_root = std::env::current_dir().ok()?;
    // Hash every content-tracked file as a (path, hash) pair, binding each
    // content hash to the path it was computed for.
    let hash_into = |pairs: &mut Vec<(String, ContentHash)>,
                     store: &mut Option<&mut CacheStore>,
                     p: &Path|
     -> Option<()> {
        if !p.exists() {
            // File doesn't exist yet (e.g. produced by an upstream job).
            // We can't compute a cache key until it is available.
            return None;
        }
        let h = match store {
            Some(s) => s.hash_input_cached(p).ok()?,
            None => hash_file(p).ok()?,
        };
        pairs.push((workflow_relative_path(p, &execution_root), h));
        Some(())
    };

    let mut input_pairs: Vec<(String, ContentHash)> = Vec::new();
    for inp in &job.inputs {
        if let OutputRef::File(p) = &inp.reference {
            hash_into(&mut input_pairs, &mut store, p)?;
        }
    }

    // Param files — their content is a cache dimension.
    for pf in &job.param_files {
        hash_into(&mut input_pairs, &mut store, pf)?;
    }

    // Script-mode jobs: the script file's *content* is a cache dimension
    // (audit B2) — the execution block only carries its path. Call mode
    // retains a residual exclusion: the referenced function's module
    // source is not content-tracked unless declared as an input.
    if let ExecutionBlock::Script { path, .. } = &job.execution {
        hash_into(&mut input_pairs, &mut store, path)?;
    }

    let rule_source = execution_source(job);

    // Params hash: framed key/value pairs of the resolved wildcards.
    let params_hash = if job.wildcards.is_empty() {
        None
    } else {
        Some(hash_kv_map(&job.wildcards))
    };

    // Env hash: content hash of the environment spec (audit H4) — hashes
    // the bytes of referenced spec files (requirements.txt, conda YAML,
    // nix expr), not just the literal spec.
    let env_hash = job.environment.as_ref().map(env_spec_content_hash);

    let shell_executable = job.shell_executable.as_deref();

    // Job spec hash: framed rule source + params + env + shell (audit H5).
    let mut spec_hasher = blake3::Hasher::new();
    update_field(&mut spec_hasher, "rule", rule_source.as_bytes());
    update_opt_field(
        &mut spec_hasher,
        "params",
        params_hash.as_deref().map(str::as_bytes),
    );
    update_opt_field(
        &mut spec_hasher,
        "env",
        env_hash.as_deref().map(str::as_bytes),
    );
    update_opt_field(
        &mut spec_hasher,
        "shell",
        shell_executable.map(str::as_bytes),
    );
    let job_spec_hash = spec_hasher.finalize().to_hex().to_string();

    let platform = current_platform();
    let cache_key = compute_cache_key(&CacheKeySpec {
        rule_source: &rule_source,
        inputs: &input_pairs,
        params_hash: params_hash.as_deref(),
        env_hash: env_hash.as_deref(),
        shell_executable,
        platform: &platform,
    });

    Some(CacheKeyComponents {
        cache_key,
        input_hashes: input_pairs
            .into_iter()
            .map(|(p, h)| (p, h.to_string()))
            .collect(),
        job_spec_hash,
    })
}

/// Compute cache key for a job by hashing its inputs, rule source, and env.
///
/// Thin wrapper over [`job_cache_key_with_components`] that discards the
/// provenance components.
fn job_cache_key(job: &ConcreteJob, store: Option<&mut CacheStore>) -> Option<ContentHash> {
    job_cache_key_with_components(job, store).map(|c| c.cache_key)
}

// ---------------------------------------------------------------------------
// CacheCheck implementation
// ---------------------------------------------------------------------------

/// Wraps a [`CacheStore`] to implement the [`CacheCheck`] trait for the
/// scheduler.  The `Mutex` ensures thread-safe access to the mutable
/// `CacheStore` (which is needed for `record` and `save`).
struct SchedulerCache {
    store: Mutex<CacheStore>,
    remote: Option<Arc<dyn RemoteCache>>,
}

/// Restore all outputs for a known local cache entry from a shared artifact
/// directory, then validate their hashes through `CacheStore`.
async fn restore_remote_outputs(
    remote: &dyn RemoteCache,
    store: &mut CacheStore,
    cache_key: &ContentHash,
    outputs: &[PathBuf],
) -> bool {
    let output_refs: Vec<&Path> = outputs.iter().map(|p| p.as_path()).collect();
    if store.is_cached(cache_key, &output_refs).unwrap_or(false) {
        return true;
    }
    let Some(entry) = store.get(cache_key) else {
        return false;
    };

    for output in outputs {
        let output_name = output.to_string_lossy();
        let Some(hash) = entry.output_hashes.get(output_name.as_ref()) else {
            return false;
        };
        match remote.fetch(hash, output).await {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                eprintln!(
                    "warning: remote cache fetch for {} failed: {e}",
                    output.display()
                );
                return false;
            }
        }
    }

    store.is_cached(cache_key, &output_refs).unwrap_or(false)
}

impl SchedulerCache {
    fn new(store: CacheStore, remote: Option<Arc<dyn RemoteCache>>) -> Self {
        Self {
            store: Mutex::new(store),
            remote,
        }
    }
}

impl CacheCheck for SchedulerCache {
    fn is_cached<'a>(
        &'a self,
        job: &'a ConcreteJob,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            let output_paths = output_file_paths(job);
            if output_paths.is_empty() {
                return false;
            }
            let output_refs: Vec<&Path> = output_paths.iter().map(|p| p.as_path()).collect();

            let mut store = self.store.lock().await;

            // Stateless mtime mode: pure filesystem comparison, no DB lookup.
            if store.validation() == CacheValidation::Mtime {
                let input_paths = input_file_paths(job);
                let input_refs: Vec<&Path> = input_paths.iter().map(|p| p.as_path()).collect();
                return match CacheStore::check_mtime_stateless(&input_refs, &output_refs) {
                    Ok(status) => status.is_hit(),
                    Err(_) => false,
                };
            }

            // DB-backed modes (MtimeHash, ContentHash): need cache key.
            let cache_key = match job_cache_key(job, Some(&mut *store)) {
                Some(k) => k,
                None => return false,
            };
            if let Some(remote) = &self.remote {
                if restore_remote_outputs(remote.as_ref(), &mut store, &cache_key, &output_paths)
                    .await
                {
                    return true;
                }
            }
            match store.check_cached(&cache_key, &output_refs) {
                Ok(status) => {
                    if let CacheHitStatus::Mismatch { ref path } = status {
                        eprintln!("cache: output hash mismatch for {}, re-executing", path,);
                    }
                    status.is_hit()
                }
                Err(_) => false,
            }
        })
    }

    fn record<'a>(&'a self, job: &'a ConcreteJob) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let output_paths = output_file_paths(job);
            if output_paths.is_empty() {
                return;
            }
            // All outputs must exist on disk.
            if !output_paths.iter().all(|p| p.exists()) {
                return;
            }
            // Compute cache key with components for provenance tracking.
            let mut store = self.store.lock().await;
            let components = match job_cache_key_with_components(job, Some(&mut *store)) {
                Some(c) => c,
                None => return,
            };

            // Build provenance from the cache key components.
            let provenance = ox_core::model::ArtifactProvenance {
                input_hashes: components.input_hashes,
                job_spec_hash: components.job_spec_hash,
                reproducibility: job.reproducibility,
            };

            let output_refs: Vec<&Path> = output_paths.iter().map(|p| p.as_path()).collect();
            if let Err(e) = store.record(components.cache_key, &output_refs, Some(&provenance)) {
                eprintln!("warning: failed to cache job {}: {e}", job.id.as_str());
                return;
            }

            if let Some(remote) = &self.remote {
                for output in &output_paths {
                    let Ok(hash) = hash_file(output) else {
                        eprintln!(
                            "warning: failed to hash output for remote cache: {}",
                            output.display()
                        );
                        continue;
                    };
                    if let Err(e) = remote.store(&hash, output).await {
                        eprintln!(
                            "warning: remote cache store for {} failed: {e}",
                            output.display()
                        );
                    }
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// BenchmarkSink implementation
// ---------------------------------------------------------------------------

/// Writes benchmark TSV files to disk.
struct FsBenchmarkSink;

impl BenchmarkSink for FsBenchmarkSink {
    fn write_benchmark<'a>(
        &'a self,
        path: &'a Path,
        result: &'a JobResult,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // Create parent directories.
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        eprintln!("benchmark: failed to create dir {}: {e}", parent.display());
                        return;
                    }
                }
            }

            let content = benchmark::format_benchmark_tsv(result);

            if let Err(e) = tokio::fs::write(path, content.as_bytes()).await {
                eprintln!("benchmark: failed to write {}: {e}", path.display());
            }
        })
    }
}

/// Find the job that produces the given target (file path or job ID).
fn find_target_job(job_graph: &JobGraph, target: &str) -> Result<JobId> {
    // Try matching by job ID directly.
    let target_id = JobId::from(target);
    if job_graph.get_job(&target_id).is_some() {
        return Ok(target_id);
    }

    // Otherwise, find the job that produces an output matching the target path.
    for job_id in job_graph.job_ids() {
        let job = job_graph
            .get_job(job_id)
            .expect("BUG: job_ids() returned an ID not present in the graph");
        for output in &job.outputs {
            let key = match &output.reference {
                OutputRef::File(p) => p.to_string_lossy().to_string(),
                OutputRef::Virtual { id, .. } => id.clone(),
                OutputRef::InMemory { type_hint } => {
                    type_hint.clone().unwrap_or_else(|| "<memory>".into())
                }
            };
            if key == target {
                return Ok(job_id.clone());
            }
        }
    }

    anyhow::bail!(
        "no job found that produces '{}'. Use `ox plan` to see available targets.",
        target
    )
}

/// Collect a job and all its transitive upstream dependencies.
fn upstream_closure(job_graph: &JobGraph, root: &JobId) -> HashSet<JobId> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(root.clone());
    queue.push_back(root.clone());
    while let Some(current) = queue.pop_front() {
        for upstream in job_graph.upstream(&current) {
            if visited.insert(upstream.clone()) {
                queue.push_back(upstream.clone());
            }
        }
    }
    visited
}

/// Collect a job and all its transitive downstream dependents.
fn downstream_closure(job_graph: &JobGraph, root: &JobId) -> HashSet<JobId> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(root.clone());
    queue.push_back(root.clone());
    while let Some(current) = queue.pop_front() {
        for downstream in job_graph.downstream(&current) {
            if visited.insert(downstream.clone()) {
                queue.push_back(downstream.clone());
            }
        }
    }
    visited
}

/// Apply profile values as defaults for CLI args.
///
/// Profile values only take effect when the CLI arg is at its default value.
/// Explicit CLI flags always take precedence over profile settings.
fn apply_profile_defaults(args: &mut RunArgs, profile: &ox_format::parse::Profile) {
    // jobs: clap default is 1; profile overrides only if still at default
    if args.jobs == 1 {
        if let Some(j) = profile.jobs {
            args.jobs = j;
        }
    }
    // cache_validation: None means not set on CLI
    if args.cache_validation.is_none() {
        args.cache_validation.clone_from(&profile.cache_validation);
    }
    // verbose: 0 means not set on CLI (count action)
    if args.verbose == 0 {
        if let Some(v) = profile.verbose {
            args.verbose = v;
        }
    }
    // executor: clap default is "local"
    if args.executor == "local" {
        if let Some(ref e) = profile.executor {
            args.executor.clone_from(e);
        }
    }
    // no_cache: false by default
    if !args.no_cache {
        if let Some(true) = profile.no_cache {
            args.no_cache = true;
        }
    }
    // keep_going: false by default
    if !args.keep_going {
        if let Some(true) = profile.keep_going {
            args.keep_going = true;
        }
    }
    // open_dashboard: false by default
    if !args.open_dashboard {
        if let Some(true) = profile.open_dashboard {
            args.open_dashboard = true;
        }
    }
    // SLURM options: None by default
    if args.partition.is_none() {
        args.partition.clone_from(&profile.partition);
    }
    if args.account.is_none() {
        args.account.clone_from(&profile.account);
    }
    if args.qos.is_none() {
        args.qos.clone_from(&profile.qos);
    }
}

/// Read `cache_validation` from the user-global config file.
///
/// Checks `$XDG_CONFIG_HOME/oxymake/config.toml` (or `~/.config/oxymake/config.toml`).
/// Returns `None` if the file doesn't exist or doesn't contain the key.
/// Read the global config TOML table.
///
/// Checks `$XDG_CONFIG_HOME/oxymake/config.toml` (or `~/.config/oxymake/config.toml`).
fn load_global_config() -> Option<toml::Table> {
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{home}/.config")
        });
    let path = PathBuf::from(config_dir).join("oxymake/config.toml");
    let content = std::fs::read_to_string(&path).ok()?;
    content.parse().ok()
}

fn resolve_global_config_cache_validation() -> Option<String> {
    let table = load_global_config()?;
    table
        .get("cache_validation")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Read `open_dashboard` from the user-global config file.
fn resolve_global_config_open_dashboard() -> Option<bool> {
    let table = load_global_config()?;
    table.get("open_dashboard").and_then(|v| v.as_bool())
}

pub fn cmd_run(mut args: RunArgs, theme: &ox_render::Theme) -> Result<()> {
    let mut timer = PhaseTimer::new(args.timings);
    let file_path = PathBuf::from(&args.file);
    if args.verbose >= 1 {
        eprintln!("Loading {}...", file_path.display());
    }
    let workflow = common::load_workflow(&file_path)?;

    timer.mark("parse");

    // Validate
    ox_format::validate::validate(&workflow).map_err(|errs| {
        let messages: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        anyhow::anyhow!("validation errors:\n  {}", messages.join("\n  "))
    })?;

    // Apply named profile if specified. Profile values act as defaults —
    // explicit CLI flags (--set, -j, etc.) take precedence.
    if let Some(ref profile_name) = args.profile {
        let profile = common::resolve_profile(&workflow, profile_name)?;
        apply_profile_defaults(&mut args, profile);
    }

    // Apply global config for open_dashboard (lowest precedence).
    if !args.open_dashboard {
        if let Some(true) = resolve_global_config_open_dashboard() {
            args.open_dashboard = true;
        }
    }

    // Build the RuleGraph for structural validation.
    let _rule_graph =
        RuleGraph::build(workflow.rules.clone()).context("failed to build RuleGraph")?;

    // Resolve: backward-chain from targets to concrete jobs.
    let mut config = common::workflow_config(&workflow);

    // Apply profile config overrides (before CLI --set, so --set wins).
    if let Some(ref profile_name) = args.profile {
        if let Some(profile) = workflow.profiles.get(profile_name) {
            common::apply_profile_config(&mut config, profile);
        }
    }

    common::apply_overrides(&mut config, &args.overrides);
    let targets = common::resolve_targets(&workflow, &args.targets);

    if targets.is_empty() {
        println!("Nothing to do (no targets specified and no default rule found).");
        return Ok(());
    }

    // Collect trusted directories from config scalars — absolute paths from
    // {config.*} substitution are user-declared and safe to write outside the
    // project root.
    let trusted_dirs: Vec<PathBuf> = config
        .scalars
        .values()
        .filter_map(|v| {
            let p = PathBuf::from(v);
            if p.is_absolute() { Some(p) } else { None }
        })
        .collect();

    // Gather existing source files — scan the working directory for files
    // that the resolver might need as source inputs.
    if args.verbose >= 2 {
        eprintln!("Scanning source files...");
    }
    let mut existing_files = common::discover_existing_files(&file_path);

    // Exclude rule output files from existing_files so the resolver always
    // produces the full job graph. Without this, the resolver sees existing
    // outputs and short-circuits (resolve_target returns early for files in
    // the `existing` set), producing 0 jobs.
    //
    // The cache pre-scan (below) handles skip logic for cached jobs. If we
    // let the resolver short-circuit, deleted/stale intermediate outputs
    // won't trigger downstream rebuilds (ox-jxdw).
    {
        let mut rule_outputs: HashSet<PathBuf> = HashSet::new();
        for rule in &workflow.rules {
            for output in &rule.outputs {
                let mut expanded = Vec::new();
                common::expand_pattern(output.pattern.as_str(), &config, &mut expanded);
                for path in expanded {
                    rule_outputs.insert(PathBuf::from(path));
                }
            }
        }
        existing_files.retain(|p| !rule_outputs.contains(p));
    }

    timer.mark("discover_files");

    let request = ResolveRequest {
        targets: targets.clone(),
        config,
        existing_files,
    };

    let mut resolve_result =
        resolver::resolve(&workflow.rules, &request).context("failed to resolve targets")?;

    // Filter by --rule if specified.
    if let Some(rule_filter) = &args.rule {
        let is_regex = rule_filter.starts_with('/') && rule_filter.ends_with('/');
        let jobs = if is_regex {
            let pattern = &rule_filter[1..rule_filter.len() - 1];
            let re = regex::Regex::new(pattern)?;
            resolve_result
                .jobs
                .into_iter()
                .filter(|j| re.is_match(j.rule.as_str()))
                .collect()
        } else {
            resolve_result
                .jobs
                .into_iter()
                .filter(|j| j.rule.as_str() == rule_filter)
                .collect()
        };
        resolve_result.jobs = jobs;
    }

    // Build the JobGraph.
    let job_graph = JobGraph::build(resolve_result.jobs).context("failed to build JobGraph")?;
    timer.mark("resolve_and_build");

    // -----------------------------------------------------------------------
    // Selective execution: --until and --omit-from
    // -----------------------------------------------------------------------
    // These compute skip sets early so that --dry-run also reflects filtering.
    let mut selective_skip: HashSet<JobId> = HashSet::new();

    if let Some(ref until_target) = args.until {
        let until_job = find_target_job(&job_graph, until_target)?;
        let keep = upstream_closure(&job_graph, &until_job);
        for job_id in job_graph.job_ids() {
            if !keep.contains(job_id) {
                selective_skip.insert(job_id.clone());
            }
        }
    }

    if let Some(ref omit_target) = args.omit_from {
        let omit_job = find_target_job(&job_graph, omit_target)?;
        let omitted = downstream_closure(&job_graph, &omit_job);
        for job_id in omitted {
            selective_skip.insert(job_id);
        }
    }

    let job_count = job_graph.job_count();

    if args.dry_run {
        let effective_count = job_count - selective_skip.len();
        if args.json {
            // Emit NDJSON: one summary line, then one line per job.
            let summary = serde_json::json!({
                "event": "dry_run_summary",
                "total_jobs": effective_count,
                "total_targets": targets.len(),
            });
            println!("{}", summary);
            if let Ok(topo) = job_graph.topological_order() {
                for job_id in topo {
                    if selective_skip.contains(job_id) {
                        continue;
                    }
                    if let Some(job) = job_graph.get_job(job_id) {
                        let outputs: Vec<String> = job
                            .outputs
                            .iter()
                            .map(|o| match &o.reference {
                                OutputRef::File(p) => p.display().to_string(),
                                OutputRef::Virtual { id, .. } => id.clone(),
                                OutputRef::InMemory { type_hint } => {
                                    type_hint.clone().unwrap_or_else(|| "<memory>".into())
                                }
                            })
                            .collect();
                        let inputs: Vec<String> = job
                            .inputs
                            .iter()
                            .map(|i| match &i.reference {
                                OutputRef::File(p) => p.display().to_string(),
                                OutputRef::Virtual { id, .. } => id.clone(),
                                OutputRef::InMemory { type_hint } => {
                                    type_hint.clone().unwrap_or_else(|| "<memory>".into())
                                }
                            })
                            .collect();
                        let job_event = serde_json::json!({
                            "event": "dry_run_job",
                            "job_id": job_id.as_str(),
                            "rule": job.rule.as_str(),
                            "outputs": outputs,
                            "inputs": inputs,
                        });
                        println!("{}", job_event);
                    }
                }
            }
        } else {
            println!(
                "Dry run: {} job(s) would execute for {} target(s)",
                effective_count,
                targets.len()
            );
            if let Ok(topo) = job_graph.topological_order() {
                for job_id in topo {
                    if selective_skip.contains(job_id) {
                        continue;
                    }
                    if let Some(job) = job_graph.get_job(job_id) {
                        let outputs: Vec<String> = job
                            .outputs
                            .iter()
                            .map(|o| match &o.reference {
                                OutputRef::File(p) => p.display().to_string(),
                                OutputRef::Virtual { id, .. } => id.clone(),
                                OutputRef::InMemory { type_hint } => {
                                    type_hint.clone().unwrap_or_else(|| "<memory>".into())
                                }
                            })
                            .collect();
                        println!(
                            "  [{}] rule={} outputs=[{}]",
                            job_id.as_str(),
                            job.rule.as_str(),
                            outputs.join(", ")
                        );
                    }
                }
            }
        }
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // --touch: mark outputs as up-to-date without running
    // -----------------------------------------------------------------------
    if args.touch {
        let oxymake_dir = PathBuf::from(".oxymake");
        let cache_validation = if let Some(ref cli_val) = args.cache_validation {
            cli_val
                .parse::<CacheValidation>()
                .map_err(|e| anyhow::anyhow!("{e}"))?
        } else {
            CacheValidation::default()
        };
        let mut cache_store = if args.no_cache {
            None
        } else {
            CacheStore::open_with(&oxymake_dir, cache_validation).ok()
        };

        let mut touched = 0usize;
        if let Ok(topo) = job_graph.topological_order() {
            for job_id in topo {
                if selective_skip.contains(job_id) {
                    continue;
                }
                if let Some(job) = job_graph.get_job(job_id) {
                    // Touch each file output: create parent dirs and update mtime.
                    for output in &job.outputs {
                        if let OutputRef::File(p) = &output.reference {
                            if let Some(parent) = p.parent() {
                                if !parent.as_os_str().is_empty() {
                                    std::fs::create_dir_all(parent).ok();
                                }
                            }
                            if p.exists() {
                                // Update mtime by opening for append (no content change).
                                let _ = std::fs::OpenOptions::new()
                                    .append(true)
                                    .open(p)
                                    .and_then(|f| f.set_modified(std::time::SystemTime::now()));
                            } else {
                                // Create an empty file.
                                std::fs::write(p, "").ok();
                            }
                        }
                    }

                    // Record in cache so the next run skips these jobs.
                    if let Some(ref mut store) = cache_store {
                        let output_paths = output_file_paths(job);
                        if !output_paths.is_empty() {
                            if let Some(components) =
                                job_cache_key_with_components(job, Some(&mut *store))
                            {
                                let provenance = ox_core::model::ArtifactProvenance {
                                    input_hashes: components.input_hashes,
                                    job_spec_hash: components.job_spec_hash,
                                    reproducibility: job.reproducibility,
                                };
                                let output_refs: Vec<&Path> =
                                    output_paths.iter().map(|p| p.as_path()).collect();
                                let _ = store.record(
                                    components.cache_key,
                                    &output_refs,
                                    Some(&provenance),
                                );
                            }
                        }
                    }
                    touched += 1;
                }
            }
        }

        // Save cache manifest.
        if let Some(ref store) = cache_store {
            if let Err(e) = store.save() {
                eprintln!("warning: failed to save cache manifest: {e}");
            }
        }

        println!("Touched {} job output(s).", touched);
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // Cache: determine which jobs can be skipped
    // -----------------------------------------------------------------------
    let oxymake_dir = PathBuf::from(".oxymake");

    // Resolve cache validation strategy (highest wins):
    //   1. CLI: --cache-validation=<strategy>
    //   2. Env: OX_CACHE_VALIDATION=<strategy>
    //   3. Oxymakefile.toml: [config] cache_validation = "<strategy>"
    //   4. User global: ~/.config/oxymake/config.toml
    //   5. Built-in default: mtime+hash (content-verifying; ADR-006 amendment)
    let cache_validation = if let Some(ref cli_val) = args.cache_validation {
        cli_val
            .parse::<CacheValidation>()
            .map_err(|e| anyhow::anyhow!("{e}"))?
    } else if let Ok(env_val) = std::env::var("OX_CACHE_VALIDATION") {
        env_val
            .parse::<CacheValidation>()
            .map_err(|e| anyhow::anyhow!("OX_CACHE_VALIDATION: {e}"))?
    } else if let Some(ox_format::parse::ConfigValue::Scalar(s)) =
        workflow.config.get("cache_validation")
    {
        s.parse::<CacheValidation>()
            .map_err(|e| anyhow::anyhow!("config cache_validation: {e}"))?
    } else if let Some(val) = resolve_global_config_cache_validation() {
        val.parse::<CacheValidation>()
            .map_err(|e| anyhow::anyhow!("global config cache_validation: {e}"))?
    } else {
        CacheValidation::default()
    };

    // A shared cache has no meaningful mtime relationship with this
    // workspace. DirectoryCache verifies each fetched artifact by content, so
    // retain that guarantee when validating the restored outputs locally.
    let remote_cache: Option<Arc<dyn RemoteCache>> = args
        .cache_remote
        .as_ref()
        .map(|dir| Arc::new(DirectoryCache::new(dir)) as Arc<dyn RemoteCache>);
    let cache_validation = if remote_cache.is_some() {
        CacheValidation::ContentHash
    } else {
        cache_validation
    };

    let mut cache_store = if args.no_cache {
        None
    } else {
        match CacheStore::open_with(&oxymake_dir, cache_validation) {
            Ok(store) => Some(store),
            Err(e) => {
                eprintln!("warning: cache unavailable ({e}), running without cache");
                None
            }
        }
    };

    let mut skip_jobs: HashSet<JobId> = HashSet::new();
    let mut run_reasons: HashMap<JobId, RunReason> = HashMap::new();

    // The cache pre-scan may restore outputs from a directory cache before
    // deciding which jobs are stale. Reuse this runtime for scheduling below.
    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;

    if args.verbose >= 1 {
        eprintln!("Checking {} job(s)...", job_count);
    }

    // Show a spinner during cache prescan for hash-based validation modes,
    // which can take several seconds while hashing output files.
    let prescan_spinner = if !args.no_cache
        && cache_store
            .as_ref()
            .is_some_and(|s| s.validation() != CacheValidation::Mtime)
        && std::io::IsTerminal::is_terminal(&std::io::stderr())
    {
        let spinner = indicatif::ProgressBar::new_spinner();
        spinner.set_style(
            indicatif::ProgressStyle::with_template("  {spinner:.yellow} {msg}")
                .unwrap_or_else(|_| indicatif::ProgressStyle::default_spinner())
                .tick_strings(&[
                    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}",
                    "\u{2826}", "\u{2827}", "\u{2807}", "\u{280f}",
                ]),
        );
        spinner.set_message(format!("Hashing outputs\u{2026} ({job_count} jobs)"));
        spinner.enable_steady_tick(std::time::Duration::from_millis(80));
        Some(spinner)
    } else {
        None
    };

    if args.no_cache {
        // All jobs run — mark each with CacheDisabled reason.
        for job_id in job_graph.job_ids() {
            run_reasons.insert(job_id.clone(), RunReason::CacheDisabled);
        }
    } else if let Some(ref mut store) = cache_store {
        // Track jobs that will re-execute (not cached). Any downstream job
        // must also re-execute even if its own outputs look valid on disk,
        // because its inputs may change once the upstream job runs. This is
        // transitive invalidation — the foundation of incremental builds.
        let mut stale_jobs: HashSet<JobId> = HashSet::new();
        if let Ok(topo) = job_graph.topological_order() {
            for job_id in topo {
                // If an upstream job is stale, this job is stale too —
                // its inputs will be regenerated and may differ.
                if stale_jobs.contains(job_id) {
                    // Propagate staleness to direct downstream dependents.
                    for dep in job_graph.downstream(job_id) {
                        stale_jobs.insert(dep.clone());
                    }
                    // Reason will be UpstreamRebuilt — set by the scheduler
                    // at emit time since it knows which jobs were force-rerun.
                    continue;
                }

                let mut is_hit = false;
                if let Some(job) = job_graph.get_job(job_id) {
                    let output_paths = output_file_paths(job);
                    let output_refs: Vec<&Path> =
                        output_paths.iter().map(|p| p.as_path()).collect();

                    if output_paths.is_empty() {
                        // No outputs — not cacheable.
                        run_reasons.insert(job_id.clone(), RunReason::NotCacheable);
                    } else if store.validation() == CacheValidation::Mtime {
                        // Stateless mtime mode: pure filesystem comparison.
                        let input_paths = input_file_paths(job);
                        let input_refs: Vec<&Path> =
                            input_paths.iter().map(|p| p.as_path()).collect();
                        match CacheStore::check_mtime_stateless(&input_refs, &output_refs) {
                            Ok(CacheHitStatus::Hit) => {
                                is_hit = true;
                                skip_jobs.insert(job_id.clone());
                            }
                            Ok(CacheHitStatus::Mismatch { ref path }) => {
                                run_reasons.insert(
                                    job_id.clone(),
                                    RunReason::OutputStale { path: path.clone() },
                                );
                            }
                            Ok(CacheHitStatus::OutputMissing { ref path }) => {
                                run_reasons.insert(
                                    job_id.clone(),
                                    RunReason::OutputMissing { path: path.clone() },
                                );
                            }
                            Ok(CacheHitStatus::Miss) => {
                                run_reasons.insert(job_id.clone(), RunReason::CacheMiss);
                            }
                            Err(_) => {
                                run_reasons.insert(job_id.clone(), RunReason::CacheMiss);
                            }
                        }
                    } else if let Some(cache_key) = job_cache_key(job, Some(&mut *store)) {
                        // DB-backed modes (MtimeHash, ContentHash).
                        let status = if let Some(remote) = &remote_cache {
                            if rt.block_on(restore_remote_outputs(
                                remote.as_ref(),
                                store,
                                &cache_key,
                                &output_paths,
                            )) {
                                CacheHitStatus::Hit
                            } else {
                                store
                                    .check_cached(&cache_key, &output_refs)
                                    .unwrap_or(CacheHitStatus::Miss)
                            }
                        } else {
                            store
                                .check_cached(&cache_key, &output_refs)
                                .unwrap_or(CacheHitStatus::Miss)
                        };
                        match status {
                            CacheHitStatus::Hit => {
                                is_hit = true;
                                skip_jobs.insert(job_id.clone());
                            }
                            CacheHitStatus::Mismatch { ref path } => {
                                run_reasons.insert(
                                    job_id.clone(),
                                    RunReason::OutputStale { path: path.clone() },
                                );
                            }
                            CacheHitStatus::OutputMissing { ref path } => {
                                run_reasons.insert(
                                    job_id.clone(),
                                    RunReason::OutputMissing { path: path.clone() },
                                );
                            }
                            CacheHitStatus::Miss => {
                                run_reasons.insert(job_id.clone(), RunReason::CacheMiss);
                            }
                        }
                    } else {
                        // No cache key — not cacheable.
                        run_reasons.insert(job_id.clone(), RunReason::NotCacheable);
                    }
                }

                // If this job is not cached, mark all downstream as stale.
                if !is_hit {
                    for dep in job_graph.downstream(job_id) {
                        stale_jobs.insert(dep.clone());
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // --forcerun: remove matching jobs (and downstream) from skip set
    // -----------------------------------------------------------------------
    let mut force_rerun: HashSet<JobId> = HashSet::new();
    if !args.forcerun.is_empty() {
        for pattern in &args.forcerun {
            let is_regex = pattern.starts_with('/') && pattern.ends_with('/');
            for job_id in job_graph.job_ids() {
                if let Some(job) = job_graph.get_job(job_id) {
                    let matches = if is_regex {
                        let re_str = &pattern[1..pattern.len() - 1];
                        regex::Regex::new(re_str)
                            .map(|re| re.is_match(job.rule.as_str()))
                            .unwrap_or(false)
                    } else {
                        job.rule.as_str() == pattern
                    };
                    if matches && !force_rerun.contains(job_id) {
                        // Force the matched job and all downstream.
                        let closure = downstream_closure(&job_graph, job_id);
                        for id in closure {
                            force_rerun.insert(id);
                        }
                    }
                }
            }
        }
        // Remove forced jobs from skip_jobs so they re-execute.
        for job_id in &force_rerun {
            skip_jobs.remove(job_id);
        }
    }

    // Merge selective_skip (--until / --omit-from) into skip_jobs.
    for job_id in &selective_skip {
        skip_jobs.insert(job_id.clone());
    }

    if let Some(spinner) = prescan_spinner {
        spinner.finish_and_clear();
    }

    timer.mark("cache_prescan");

    let cached_count = skip_jobs.len().saturating_sub(selective_skip.len());
    if cached_count > 0 {
        println!("Cache: {cached_count} of {job_count} job(s) up-to-date, skipping.");
    }
    if !selective_skip.is_empty() {
        println!(
            "Selective: {} job(s) excluded by --until/--omit-from.",
            selective_skip.len()
        );
    }

    // -----------------------------------------------------------------------
    // Fast path: all jobs cached — skip scheduler, state.db, and tokio
    // -----------------------------------------------------------------------
    let to_run = job_count - skip_jobs.len();
    if to_run == 0 {
        if !args.json {
            println!(
                "Completed: 0 succeeded, 0 failed, {} skipped, 0 cancelled (0.0s)",
                job_count
            );
        } else {
            let summary = serde_json::json!({
                "event": "run_completed",
                "total_jobs": job_count,
                "succeeded": 0,
                "failed": 0,
                "skipped": job_count,
                "cancelled": 0,
                "duration_ms": 0,
            });
            println!("{}", summary);
        }
        timer.mark("all_cached_exit");
        timer.print();
        return Ok(());
    }

    // Execute via the scheduler.
    let memory_budget_bytes =
        common::parse_human_size(&args.memory_budget).context("invalid --memory-budget value")?;

    // Compute the critical path so the scheduler can gate in-memory
    // materialization to critical-path jobs only (Stage 2 optimization).
    // When memory budget is zero, the set stays empty (all outputs eligible
    // but no in-memory materialization occurs since budget is disabled).
    let critical_path_jobs = if memory_budget_bytes > 0 {
        let cp = CriticalPathPass::new();
        cp.compute(&job_graph)
    } else {
        HashSet::new()
    };

    let scheduler_config = SchedulerConfig {
        max_jobs: args.jobs,
        keep_going: args.keep_going,
        skip_jobs,
        force_rerun,
        run_reasons,
        memory_budget_bytes,
        critical_path_jobs,
        ..Default::default()
    };

    let event_bus = EventBus::new();
    let project_dir = std::env::current_dir().context("failed to determine project directory")?;

    let ctx = ExecContext {
        global_job_limit: args.jobs,
        run_id: format!("run-{}", std::process::id()),
        log_dir: PathBuf::from(".oxymake/logs"),
        project_dir,
        trusted_dirs,
        input_data: std::collections::HashMap::new(),
        memory_map: Some(ox_core::memory_map::OutputMemoryMap::new()),
    };

    // Ensure .oxymake/ exists — CacheStore creates it when caching is
    // enabled, but with --no-cache (or any executor, including slurm) we
    // still need the directory for state.db.
    std::fs::create_dir_all(&oxymake_dir).ok();

    // Open state database for job persistence.
    let state_db_path = oxymake_dir.join("state.db");
    let state_db = match ox_state::db::StateDb::open(&state_db_path) {
        Ok(db) => Some(db),
        Err(e) => {
            eprintln!(
                "warning: state database unavailable ({e}), status/history will not be recorded"
            );
            None
        }
    };
    let session_id = if let Some(ref db) = state_db {
        let pid = std::process::id();
        db.create_session(pid, "localhost", None).ok()
    } else {
        None
    };

    timer.mark("state_db_open");

    // Register all jobs in state.db before execution.
    let run_id = ctx.run_id.clone();
    if let Some(ref db) = state_db {
        // Begin audit-trail run record BEFORE registering jobs, because
        // jobs.run_id references runs(id) via foreign key.
        let _ = db.begin_run(
            &run_id,
            None, // workflow_hash — not yet computed
            job_count,
            args.note.as_deref(),
        );

        let records: Vec<ox_state::db::JobRecord> = job_graph
            .topological_order()
            .unwrap_or_default()
            .iter()
            .filter_map(|job_id| {
                job_graph
                    .get_job(job_id)
                    .map(|job| ox_state::db::JobRecord {
                        id: job_id.as_str().to_string(),
                        rule_name: job.rule.as_str().to_string(),
                        wildcards: serde_json::to_string(&job.wildcards).unwrap_or_default(),
                        cache_key: None,
                        run_id: Some(run_id.clone()),
                    })
            })
            .collect();
        let _ = db.register_jobs(&records);

        // Persist job-to-job edges for DAG visualization.
        let edge_records: Vec<(String, String)> = job_graph
            .job_edges()
            .into_iter()
            .map(|(from, to)| (from.as_str().to_string(), to.as_str().to_string()))
            .collect();
        let _ = db.register_edges(&edge_records);
    }

    timer.mark("state_db_register");

    // Build the cache checker for the scheduler (dynamic cache checking).
    // Keep a typed reference for saving the manifest after the run.
    let scheduler_cache_impl: Option<Arc<SchedulerCache>> = cache_store
        .take()
        .map(|store| Arc::new(SchedulerCache::new(store, remote_cache)));
    let scheduler_cache: Option<Arc<dyn CacheCheck>> = scheduler_cache_impl
        .clone()
        .map(|sc| sc as Arc<dyn CacheCheck>);

    // Always write events to an NDJSON log file so `ox subscribe` can tail it.
    let events_dir = oxymake_dir.join("events");
    std::fs::create_dir_all(&events_dir).ok();
    let event_log_path = events_dir.join(format!("{}.ndjson", ctx.run_id));
    let event_log_file =
        std::fs::File::create(&event_log_path).context("failed to create event log file")?;

    // Create the --report-json file up front so an unwritable path fails
    // the run immediately, not after the work is done (H28).
    let report_json_file = match args.report_json {
        Some(ref path) => Some(
            std::fs::File::create(path)
                .with_context(|| format!("failed to create report file: {path}"))?,
        ),
        None => None,
    };

    // Collect per-job duration_ms from events for the audit trail (ox-mnbb).
    let job_durations: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));

    let result = rt.block_on(async {
        // Set up graceful shutdown: SIGINT (Ctrl+C) notifies the scheduler
        // to send SIGTERM to running children and stop dispatching new work.
        // A second Ctrl+C force-exits (exit code 130 = 128 + SIGINT).
        //
        // Without the second-signal handler, tokio's installed signal hook
        // swallows subsequent SIGINTs after the first ctrl_c() resolves,
        // making the process appear frozen (ox-2sek).
        let shutdown = Arc::new(tokio::sync::Notify::new());

        // Collect bridge JoinHandles so we can await them after the
        // scheduler completes, ensuring all in-flight events are drained
        // before the process exits (hq-28pdh).
        let mut bridge_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

        // Always persist events to the log file for `ox subscribe`.
        {
            let mut rx = event_bus.subscribe();
            let reporter = ox_report_json::reporter::JsonReporter::new(event_log_file);
            bridge_handles.push(tokio::spawn(async move {
                use ox_core::traits::reporter::Reporter;
                loop {
                    match rx.recv().await {
                        Ok(event) => reporter.on_event(&event).await,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("warning: event-log bridge lagged, dropped {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }));
        }

        // Subscribe a JsonReporter writing to the --report-json file.
        if let Some(report_file) = report_json_file {
            let mut rx = event_bus.subscribe();
            let reporter = ox_report_json::reporter::JsonReporter::new(report_file);
            bridge_handles.push(tokio::spawn(async move {
                use ox_core::traits::reporter::Reporter;
                loop {
                    match rx.recv().await {
                        Ok(event) => reporter.on_event(&event).await,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("warning: report-json bridge lagged, dropped {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }));
        }

        // Subscribe a JsonReporter when --json is passed.
        if args.json {
            let mut rx = event_bus.subscribe();
            let reporter = ox_report_json::reporter::JsonReporter::stdout();
            bridge_handles.push(tokio::spawn(async move {
                use ox_core::traits::reporter::Reporter;
                loop {
                    match rx.recv().await {
                        Ok(event) => reporter.on_event(&event).await,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("warning: json-stdout bridge lagged, dropped {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }));
        }

        // Subscribe a TermReporter when not in --json mode.
        // In non-TTY mode (piped, CI, cron) the reporter falls back to plain
        // text output without progress bars or spinners.
        // Keep a MultiProgress handle so the SIGINT handler can clear the bars.
        let (progress_multi, term_reporter) = if !args.json {
            let mut rx = event_bus.subscribe();
            let log_dir = if args.verbose >= 2 {
                Some(ctx.log_dir.clone())
            } else {
                None
            };
            let reporter = Arc::new(
                ox_report_term::reporter::TermReporter::with_verbosity_and_theme(
                    args.verbose,
                    log_dir,
                    theme.clone(),
                ),
            );
            let multi = reporter.multi();
            let reporter_clone = Arc::clone(&reporter);
            bridge_handles.push(tokio::spawn(async move {
                use ox_core::traits::reporter::Reporter;
                loop {
                    match rx.recv().await {
                        Ok(event) => reporter_clone.on_event(&event).await,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("warning: term-reporter bridge lagged, dropped {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }));
            (Some(multi), Some(reporter))
        } else {
            (None, None)
        };

        // Subscribe a state-db writer so the dashboard sees live updates.
        // Without this, state.db is only written after the scheduler
        // completes and the dashboard shows stale data (ox-g9ei).
        //
        // Also collect per-job duration_ms for the audit trail (ox-mnbb).
        // We capture the JoinHandle so we can await it after the scheduler
        // finishes — ensuring all events are flushed to DB before the
        // audit trail reads them (hq-9in00).
        if state_db.is_some() {
            let mut rx = event_bus.subscribe();
            let db_path = state_db_path.clone();
            let sid = session_id.clone().unwrap_or_default();
            let durations = Arc::clone(&job_durations);
            bridge_handles.push(tokio::spawn(async move {
                let db = match ox_state::db::StateDb::open(&db_path) {
                    Ok(db) => db,
                    Err(_) => return,
                };
                loop {
                    match rx.recv().await {
                        Ok(event) => match event {
                            Event::JobStarted { ref job_id, .. } => {
                                let _ = db.claim_job(job_id.as_str(), &sid);
                            }
                            Event::JobCompleted {
                                ref job_id,
                                duration_ms,
                                ..
                            } => {
                                // Claim first in case JobStarted was never received.
                                let _ = db.claim_job(job_id.as_str(), &sid);
                                let _ = db.complete_job(job_id.as_str(), &sid, 0, "");
                                durations
                                    .lock()
                                    .await
                                    .insert(job_id.as_str().to_string(), duration_ms);
                            }
                            Event::JobFailed {
                                ref job_id,
                                exit_code,
                                ..
                            } => {
                                let _ = db.claim_job(job_id.as_str(), &sid);
                                let _ = db.fail_job(job_id.as_str(), &sid, exit_code.unwrap_or(1));
                            }
                            Event::JobSkipped { ref job_id, .. } => {
                                let _ = db.skip_job(job_id.as_str());
                            }
                            Event::JobCancelled { ref job_id, .. } => {
                                let _ = db.cancel_job_ids(&[job_id.to_string()]);
                            }
                            _ => {}
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("warning: ledger EventSink lagged, dropped {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }));
        }

        // Spawn the shutdown-signal handler AFTER reporter setup so we can
        // clear progress bars on interrupt. The first SIGINT (Ctrl+C) *or*
        // SIGTERM (`ox cancel`, `kill`) triggers graceful shutdown — same
        // path for both, so cancelled children are reaped instead of
        // orphaned (B8). A second signal force-exits.
        {
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                #[cfg(unix)]
                let mut sigterm =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
                #[cfg(not(unix))]
                let mut sigterm = ();

                // First signal: graceful shutdown.
                wait_for_shutdown_signal(&mut sigterm).await;
                // Clear progress bars so the message is visible.
                if let Some(ref multi) = progress_multi {
                    let _ = multi.clear();
                }
                eprintln!("\nInterrupted — waiting for running jobs to exit…");
                shutdown.notify_waiters();

                // Second signal: force-exit. Without this, tokio's signal
                // hook swallows subsequent SIGINTs (ox-2sek).
                wait_for_shutdown_signal(&mut sigterm).await;
                eprintln!("\nForce exit.");
                std::process::exit(130);
            });
        }

        // Spawn the async disk writer when a memory budget is active.
        // The writer persists in-memory outputs to disk in the background,
        // ensuring cache correctness without blocking the critical path.
        // Targets are confined to the project workspace (H13): an
        // Oxymakefile is untrusted input and must not write outside it.
        let disk_writer_state = if memory_budget_bytes > 0 {
            let (handle, join) = spawn_disk_writer_confined(128, Some(ctx.project_dir.clone()));
            Some((handle, join))
        } else {
            None
        };
        let disk_writer_handle = disk_writer_state.as_ref().map(|(h, _)| h.clone());

        let sched_result = match args.executor.as_str() {
            "local" => {
                let mut executor = if args.jobs > 1 {
                    LocalExecutor::with_max_jobs(args.jobs)
                } else {
                    LocalExecutor::new()
                };
                if args.verbose >= 2 {
                    executor = executor.with_event_bus(event_bus.clone());
                }
                if let Some(ref mode) = args.warm_workers {
                    let project_dir = std::env::current_dir().unwrap_or_default();
                    let warm_mode = match mode.as_str() {
                        "fork" => ox_exec_local::call_mode::WarmWorkerMode::Fork,
                        "persistent" => ox_exec_local::call_mode::WarmWorkerMode::Persistent,
                        other => {
                            eprintln!(
                                "error: unknown --warm-workers mode: {other:?} (expected 'fork' or 'persistent')"
                            );
                            std::process::exit(1);
                        }
                    };
                    let pool = Arc::new(
                        ox_exec_local::worker_pool::WorkerPool::new_with_mode(
                            project_dir, warm_mode,
                        ),
                    );
                    executor = executor.with_worker_pool(pool);
                }
                let bench_sink: Option<Arc<dyn BenchmarkSink>> = Some(Arc::new(FsBenchmarkSink));
                scheduler::run_scheduler_with_cache(
                    &job_graph,
                    Arc::new(executor),
                    &scheduler_config,
                    &event_bus,
                    &ctx,
                    scheduler_cache.clone(),
                    None,
                    bench_sink,
                    Some(shutdown.clone()),
                    disk_writer_handle.clone(),
                )
                .await
            }
            "slurm" => {
                let slurm_toml = workflow.executor_config.slurm.as_ref();
                let slurm_config = SlurmConfig {
                    partition: args
                        .partition
                        .clone()
                        .or_else(|| slurm_toml.and_then(|s| s.partition.clone())),
                    account: args
                        .account
                        .clone()
                        .or_else(|| slurm_toml.and_then(|s| s.account.clone())),
                    qos: args
                        .qos
                        .clone()
                        .or_else(|| slurm_toml.and_then(|s| s.qos.clone())),
                    max_submit: Some(args.jobs),
                    api_url: args
                        .slurm_api
                        .clone()
                        .or_else(|| slurm_toml.and_then(|s| s.api_url.clone())),
                    token_cmd: slurm_toml.and_then(|s| s.token_cmd.clone()),
                    staging_dir: slurm_toml
                        .and_then(|s| s.staging_dir.as_ref())
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("/tmp/oxymake-slurm")),
                    extra_flags: slurm_toml
                        .map(|s| s.extra_flags.clone())
                        .unwrap_or_default(),
                    ..SlurmConfig::default()
                };
                let executor = SlurmExecutor::new(slurm_config, event_bus.clone());
                // Pre-flight: verify SLURM CLI tools are available before
                // scheduling any jobs. Without this, a missing `sbatch`
                // silently produces a "0 succeeded, 0 failed" result.
                executor.init().await.map_err(|e| {
                    ox_core::error::OxError::Exec(ox_core::error::ExecError::Executor {
                        message: format!("SLURM executor init failed: {e}"),
                    })
                })?;

                // Pass cached jobs to the SLURM executor so it omits them
                // from the DAG submission (their outputs already exist).
                executor
                    .set_skip_jobs(scheduler_config.skip_jobs.clone())
                    .await;

                // Mark cached jobs as skipped in state.db before submission.
                if let Some(ref db) = state_db {
                    for job_id in &scheduler_config.skip_jobs {
                        let _ = db.skip_job(job_id.as_str());
                    }
                }

                // DAG-level submission: submit uncached jobs via sbatch
                // with --dependency=afterok chains for the DAG edges.
                let dag_result = executor.submit_dag(&job_graph, &ctx).await.map_err(|e| {
                    ox_core::error::OxError::Exec(ox_core::error::ExecError::Executor {
                        message: format!("SLURM DAG submission failed: {e}"),
                    })
                })?;

                // Record DAG submission in state.db for `ox status` tracking.
                if let Some(ref db) = state_db {
                    let _ = db.record_dag_submission(
                        &dag_result.run_id,
                        "slurm",
                        None,
                        dag_result.total_jobs - dag_result.skipped,
                    );
                    for (job_id_str, submission_id) in &dag_result.job_submissions {
                        let _ = db.set_executor_submission_id(job_id_str, submission_id);
                    }
                }

                let active = dag_result.total_jobs - dag_result.skipped;
                eprintln!(
                    "DAG submitted to SLURM: {} active jobs ({} cached, {} total, run_id: {})",
                    active, dag_result.skipped, dag_result.total_jobs, dag_result.run_id
                );
                eprintln!(
                    "  {} root jobs submitted, {} jobs pending on dependencies",
                    dag_result.submitted, dag_result.pending
                );

                if args.follow {
                    // Poll until all jobs reach a terminal state.
                    eprintln!("Following execution progress…\n");
                    let follow_start = std::time::Instant::now();
                    let poll_interval = std::time::Duration::from_secs(5);
                    let mut completed = 0usize;
                    let mut failed = 0usize;

                    loop {
                        tokio::time::sleep(poll_interval).await;

                        let mut still_running = false;
                        for job_id_str in dag_result.job_submissions.keys() {
                            let jid = JobId::from(job_id_str.as_str());
                            match executor.poll_status(&jid).await {
                                Ok(ox_core::traits::executor::JobStatus::Completed) => {
                                    completed += 1;
                                }
                                Ok(ox_core::traits::executor::JobStatus::Failed(msg)) => {
                                    eprintln!("  FAILED: {} — {}", job_id_str, msg);
                                    failed += 1;
                                }
                                Ok(ox_core::traits::executor::JobStatus::Cancelled) => {
                                    failed += 1;
                                }
                                Ok(_) => {
                                    still_running = true;
                                }
                                Err(e) => {
                                    eprintln!("  Warning: failed to poll {}: {}", job_id_str, e);
                                    still_running = true;
                                }
                            }
                        }

                        let total = dag_result.total_jobs;
                        let done = completed + failed;
                        eprintln!(
                            "  Progress: {}/{} ({} succeeded, {} failed)",
                            done, total, completed, failed
                        );

                        if !still_running || done >= total {
                            break;
                        }

                        // Reset counters for next poll cycle (we re-poll all).
                        completed = 0;
                        failed = 0;
                    }

                    // Build a SchedulerResult from the final state.
                    let duration = follow_start.elapsed();
                    Ok(scheduler::SchedulerResult {
                        total_jobs: dag_result.total_jobs,
                        succeeded: completed,
                        failed,
                        skipped: dag_result.skipped,
                        cancelled: 0,
                        duration,
                        failed_details: vec![],
                        root_cause: None,
                        memory_stats: None,
                    })
                } else {
                    // Fire-and-forget: return immediately.
                    eprintln!("Use 'ox status' to check progress.");
                    Ok(scheduler::SchedulerResult {
                        total_jobs: dag_result.total_jobs,
                        succeeded: 0,
                        failed: 0,
                        skipped: dag_result.skipped,
                        cancelled: 0,
                        duration: std::time::Duration::ZERO,
                        failed_details: vec![],
                        root_cause: None,
                        memory_stats: None,
                    })
                }
            }
            "ray" => {
                let ray_config = RayConfig {
                    dashboard_address: args
                        .ray_address
                        .clone()
                        .unwrap_or_else(|| "http://127.0.0.1:8265".to_string()),
                    max_submit: Some(args.jobs),
                    working_dir: PathBuf::from(".oxymake/runs"),
                    ..RayConfig::default()
                };
                let executor = RayExecutor::new(ray_config).map_err(|e| {
                    ox_core::error::OxError::Exec(ox_core::error::ExecError::Executor {
                        message: format!("Ray executor creation failed: {e}"),
                    })
                })?;
                executor.init().await.map_err(|e| {
                    ox_core::error::OxError::Exec(ox_core::error::ExecError::Executor {
                        message: format!("Ray executor init failed: {e}"),
                    })
                })?;

                // Pass cached jobs to the Ray executor so it generates a
                // driver script containing only uncached jobs.
                executor
                    .set_skip_jobs(scheduler_config.skip_jobs.clone())
                    .await;

                // Mark cached jobs as skipped in state.db before submission.
                if let Some(ref db) = state_db {
                    for job_id in &scheduler_config.skip_jobs {
                        let _ = db.skip_job(job_id.as_str());
                    }
                }

                // DAG-level submission: submit only uncached jobs to Ray.
                let dag_result = executor.submit_dag(&job_graph, &ctx).await.map_err(|e| {
                    ox_core::error::OxError::Exec(ox_core::error::ExecError::Executor {
                        message: format!("Ray DAG submission failed: {e}"),
                    })
                })?;

                // Record DAG submission in state.db for `ox status` tracking.
                if let Some(ref db) = state_db {
                    let ray_addr = args
                        .ray_address
                        .as_deref()
                        .unwrap_or("http://127.0.0.1:8265");
                    let _ = db.record_dag_submission(
                        &dag_result.run_id,
                        "ray",
                        Some(ray_addr),
                        dag_result.total_jobs - dag_result.skipped,
                    );
                    for (job_id_str, submission_id) in &dag_result.job_submissions {
                        let _ = db.set_executor_submission_id(job_id_str, submission_id);
                    }
                }

                let active = dag_result.total_jobs - dag_result.skipped;
                eprintln!(
                    "DAG submitted to Ray: {} active jobs ({} cached, {} total, run_id: {})",
                    active, dag_result.skipped, dag_result.total_jobs, dag_result.run_id
                );
                let dashboard_url = args
                    .ray_address
                    .as_deref()
                    .unwrap_or("http://127.0.0.1:8265");
                // OSC 8 hyperlink: \e]8;;URL\e\\LABEL\e]8;;\e\\
                eprintln!(
                    "  Dashboard: \x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
                    dashboard_url, dashboard_url
                );

                if args.open_dashboard {
                    let _ = open::that(dashboard_url);
                }

                if args.follow {
                    // Poll until all jobs reach a terminal state.
                    eprintln!("Following execution progress…\n");
                    let follow_start = std::time::Instant::now();
                    let poll_interval = std::time::Duration::from_secs(3);
                    let mut completed = 0usize;
                    let mut failed = 0usize;

                    loop {
                        tokio::time::sleep(poll_interval).await;

                        let mut still_running = false;
                        for job_id_str in dag_result.job_submissions.keys() {
                            let jid = JobId::from(job_id_str.as_str());
                            match executor.poll_status(&jid).await {
                                Ok(ox_core::traits::executor::JobStatus::Completed) => {
                                    completed += 1;
                                }
                                Ok(ox_core::traits::executor::JobStatus::Failed(msg)) => {
                                    eprintln!("  FAILED: {} — {}", job_id_str, msg);
                                    failed += 1;
                                }
                                Ok(ox_core::traits::executor::JobStatus::Cancelled) => {
                                    failed += 1;
                                }
                                Ok(_) => {
                                    still_running = true;
                                }
                                Err(e) => {
                                    eprintln!("  Warning: failed to poll {}: {}", job_id_str, e);
                                    still_running = true;
                                }
                            }
                        }

                        let total = dag_result.total_jobs;
                        let done = completed + failed;
                        eprintln!(
                            "  Progress: {}/{} ({} succeeded, {} failed)",
                            done, total, completed, failed
                        );

                        if !still_running || done >= total {
                            break;
                        }

                        // Reset counters for next poll cycle (we re-poll all).
                        completed = 0;
                        failed = 0;
                    }

                    // Build a SchedulerResult from the final state.
                    let duration = follow_start.elapsed();
                    Ok(scheduler::SchedulerResult {
                        total_jobs: dag_result.total_jobs,
                        succeeded: completed,
                        failed,
                        skipped: dag_result.skipped,
                        cancelled: 0,
                        duration,
                        failed_details: vec![],
                        root_cause: None,
                        memory_stats: None,
                    })
                } else {
                    // Fire-and-forget: return immediately.
                    eprintln!("Use 'ox status' or 'ox run --follow' to track progress.");
                    Ok(scheduler::SchedulerResult {
                        total_jobs: dag_result.total_jobs,
                        succeeded: 0,
                        failed: 0,
                        skipped: dag_result.skipped,
                        cancelled: 0,
                        duration: std::time::Duration::ZERO,
                        failed_details: vec![],
                        root_cause: None,
                        memory_stats: None,
                    })
                }
            }
            other => Err(ox_core::error::OxError::Exec(
                ox_core::error::ExecError::Executor {
                    message: format!(
                        "unknown executor '{}': expected 'local', 'slurm', or 'ray'",
                        other
                    ),
                },
            )),
        };

        // Flush the async disk writer: drop ALL sender clones to close the
        // channel, then await the background task to drain remaining writes.
        // Both the original handle AND the clone we created for the scheduler
        // must be dropped — the channel closes only when all senders are gone.
        // A flush with failed writes means outputs are missing on disk; the
        // run must fail rather than report success (H14). The error is merged
        // into sched_result below, after the reporter summary.
        drop(disk_writer_handle);
        let disk_flush_result = match disk_writer_state {
            Some((handle, join)) => ox_core::disk_writer::flush_disk_writer(handle, join).await,
            None => Ok(()),
        };

        // Call reporter.finish() to print the run summary to stderr.
        // Skip for fire-and-forget remote executors — their "0 succeeded"
        // result is misleading; the real summary is printed later (ox-x2jm).
        let skip_reporter_finish =
            (args.executor == "ray" || args.executor == "slurm") && !args.follow;
        if !skip_reporter_finish {
            if let Some(ref reporter) = term_reporter {
                use ox_core::traits::reporter::{Reporter, RunSummary};
                let summary = match &sched_result {
                    Ok(r) => RunSummary {
                        total_jobs: r.total_jobs,
                        succeeded: r.succeeded,
                        failed: r.failed,
                        skipped: r.skipped + r.cancelled,
                        duration_ms: r.duration.as_millis() as u64,
                    },
                    Err(_) => RunSummary {
                        total_jobs: 0,
                        succeeded: 0,
                        failed: 0,
                        skipped: 0,
                        duration_ms: 0,
                    },
                };
                reporter.finish(&summary).await;
            }
        }

        // Drop the event bus sender so all bridge receivers see `Closed`
        // and drain their remaining events before we exit (hq-28pdh).
        drop(event_bus);

        // Await all bridge tasks to ensure in-flight events are fully
        // processed (written to disk, printed to terminal, etc.).
        for handle in bridge_handles {
            let _ = handle.await;
        }

        // Persistence failures override a successful scheduler result (H14):
        // outputs the scheduler believes exist never reached the disk.
        match (sched_result, disk_flush_result) {
            (Ok(_), Err(flush_err)) => Err(flush_err),
            (result, _) => result,
        }
    });

    timer.mark("scheduler");

    // The event-bus state-db writer has been awaited inside rt.block_on(),
    // so all claim/complete/fail/skip transitions are already flushed to DB.
    // We only need to:
    //   1. Mark any lingering "running" jobs as failed (crash / missing event).
    //   2. Record audit-trail history entries by reading final DB state (hq-9in00).
    let durations = job_durations.blocking_lock();
    if let Some(db) = &state_db {
        // Mark any jobs still in 'running' state as failed. This catches
        // jobs that crashed without emitting a JobFailed event.
        let had_failures = match &result {
            Ok(r) => r.failed > 0,
            Err(_) => true,
        };
        if had_failures {
            // Scoped to this run's session: in cooperative multi-session
            // mode another live session's running jobs are not ours to
            // terminalize (H16).
            if let Some(sid) = &session_id {
                if let Ok(running) = db.jobs_by_status("running") {
                    for job_id in &running {
                        let _ = db.fail_job(job_id.as_str(), sid, 1);
                    }
                }
            }
        }

        // Record audit-trail history entries from the post-flush DB state.
        // The jobs table already has rule_name, wildcards, status, timing,
        // and exit_code — no need to iterate the in-memory job graph.
        let _ = db.finalize_job_history(&run_id, &args.executor, "localhost", &durations);
    }

    // -----------------------------------------------------------------------
    // Cache: save manifest to disk
    // -----------------------------------------------------------------------
    // The scheduler records completed jobs via the CacheCheck trait. We just
    // need to flush the manifest to disk.
    if let Some(ref sc) = scheduler_cache_impl {
        let store = sc.store.blocking_lock();
        if let Err(e) = store.save() {
            eprintln!("warning: failed to save cache manifest: {e}");
        }
    }

    timer.mark("state_db_finalize");

    match result {
        Ok(sched_result) => {
            // Finalise the audit-trail run record.
            if let Some(ref db) = state_db {
                let _ = db.end_run(
                    &run_id,
                    sched_result.succeeded,
                    sched_result.failed,
                    sched_result.skipped,
                );
            }

            // Fire-and-forget remote executors: show "Submitted" not "Completed".
            let is_fire_and_forget =
                (args.executor == "ray" || args.executor == "slurm") && !args.follow;

            if is_fire_and_forget {
                let submitted = sched_result.total_jobs - sched_result.skipped;
                if args.json {
                    let summary = serde_json::json!({
                        "event": "run_submitted",
                        "executor": args.executor,
                        "submitted_jobs": submitted,
                        "cached_jobs": sched_result.skipped,
                        "total_jobs": sched_result.total_jobs,
                    });
                    println!("{}", summary);
                } else {
                    println!(
                        "Submitted: {} job(s) to {} ({} cached, {} total)",
                        submitted, args.executor, sched_result.skipped, sched_result.total_jobs,
                    );
                }
            } else if !args.json {
                println!(
                    "Completed: {} succeeded, {} failed, {} skipped, {} cancelled ({:.1}s)",
                    sched_result.succeeded,
                    sched_result.failed,
                    sched_result.skipped,
                    sched_result.cancelled,
                    sched_result.duration.as_secs_f64(),
                );
                if sched_result.failed > 0 {
                    print_failure_summary(
                        &sched_result.failed_details,
                        sched_result.failed,
                        &sched_result.root_cause,
                    );
                }
                // Print Stage 2 memory stats when a budget was active.
                if let Some(ref ms) = sched_result.memory_stats {
                    let peak_mb = ms.peak_memory_bytes as f64 / (1024.0 * 1024.0);
                    let budget_mb = ms.memory_budget_bytes as f64 / (1024.0 * 1024.0);
                    if ms.memory_budget_bytes > 0 {
                        eprintln!(
                            "  Memory: peak {:.1}M / {:.1}M budget ({} evictions, {:.1}M reclaimed)",
                            peak_mb,
                            budget_mb,
                            ms.eviction_count,
                            ms.eviction_bytes as f64 / (1024.0 * 1024.0),
                        );
                    } else if ms.peak_memory_bytes > 0 {
                        eprintln!("  Memory: peak {:.1}M (no budget)", peak_mb);
                    }
                }
            }
            timer.mark("summary");
            timer.print();
            if sched_result.failed > 0 {
                std::process::exit(1);
            }
            Ok(())
        }
        Err(e) => {
            // Finalise the run record even on error.
            if let Some(ref db) = state_db {
                let _ = db.end_run(&run_id, 0, 0, 0);
            }
            bail!("{e}");
        }
    }
}

/// Print a human-readable failure summary after a run with failures.
///
/// When root-cause detection triggered mid-run, shows the root cause once.
/// Otherwise shows up to 3 failed job names with their last stderr line.
/// When all failures share the same error, collapses them into a single
/// root-cause line.
fn print_failure_summary(
    details: &[FailedJobDetail],
    total_failed: usize,
    root_cause: &Option<scheduler::RootCause>,
) {
    if details.is_empty() {
        eprintln!("  Run 'ox logs --failed' for full details.");
        return;
    }

    // If root-cause detection fired mid-run, show it prominently.
    if let Some(rc) = root_cause {
        eprintln!(
            "  Detected common root cause across {} failures: {}",
            rc.job_ids.len(),
            rc.error_line,
        );
        eprintln!("  Run 'ox logs --failed' for full details.");
        return;
    }

    // Check if all failures share the same last stderr line.
    let first_line = details[0].last_stderr_line.as_deref();
    let all_same = first_line.is_some()
        && details
            .iter()
            .all(|d| d.last_stderr_line.as_deref() == first_line);

    if all_same && total_failed > 1 {
        eprintln!(
            "  All {} failures share the same root cause: {}",
            total_failed,
            first_line.unwrap(),
        );
    } else {
        let show_count = details.len().min(3);
        eprintln!(
            "  Failed jobs (showing {} of {}):",
            show_count, total_failed,
        );
        for detail in details.iter().take(3) {
            match &detail.last_stderr_line {
                Some(line) => eprintln!("    {}: {}", detail.job_id, line),
                None => eprintln!("    {}: (no stderr captured)", detail.job_id),
            }
        }
    }
    eprintln!("  Run 'ox logs --failed' for full details.");
}

#[cfg(test)]
mod cache_key_tests {
    use super::*;
    use ox_core::model::{EnvSpec, RuleName};

    fn make_job(execution: ExecutionBlock) -> ConcreteJob {
        ConcreteJob {
            id: JobId("job-1".into()),
            rule: RuleName("test".into()),
            wildcards: Default::default(),
            tags: Default::default(),
            inputs: vec![],
            outputs: vec![],
            execution,
            resources: Default::default(),
            environment: None,
            error_strategy: Default::default(),
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: Default::default(),
            param_files: Vec::new(),
            log: Default::default(),
            shell_executable: None,
            reproducibility: Default::default(),
        }
    }

    /// Audit B2 — editing a script file must change the cache key, even
    /// though the execution block (which only carries the path) is
    /// unchanged.
    #[test]
    fn script_content_changes_cache_key() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("script.py");
        std::fs::write(&script, "print('v1')").unwrap();

        let job = make_job(ExecutionBlock::Script {
            path: script.clone(),
            lang: Some("python".into()),
        });

        let k1 = job_cache_key(&job, None).expect("key with existing script");
        std::fs::write(&script, "print('v2')").unwrap();
        let k2 = job_cache_key(&job, None).expect("key after script edit");

        assert_ne!(k1, k2, "script content must enter the cache key");
    }

    /// A script-mode job whose script file is missing has no cache key
    /// (it can never be a cache hit).
    #[test]
    fn missing_script_yields_no_key() {
        let job = make_job(ExecutionBlock::Script {
            path: PathBuf::from("/nonexistent/script.py"),
            lang: None,
        });
        assert_eq!(job_cache_key(&job, None), None);
    }

    /// Audit H5 — the shell executable must enter the cache key: the same
    /// command under /bin/bash and /bin/zsh can behave differently.
    #[test]
    fn shell_executable_changes_cache_key() {
        let mut j1 = make_job(ExecutionBlock::Shell {
            command: "echo hello".into(),
        });
        let mut j2 = j1.clone();
        j1.shell_executable = None;
        j2.shell_executable = Some("/bin/zsh".into());

        let k1 = job_cache_key(&j1, None).unwrap();
        let k2 = job_cache_key(&j2, None).unwrap();
        assert_ne!(k1, k2, "shell executable must enter the cache key");
    }

    /// Audit H4 — editing the environment file's content (not its path)
    /// must change the cache key.
    #[test]
    fn env_file_content_changes_cache_key() {
        let dir = tempfile::tempdir().unwrap();
        let req = dir.path().join("requirements.txt");
        std::fs::write(&req, "numpy==1.0").unwrap();

        let mut job = make_job(ExecutionBlock::Shell {
            command: "python train.py".into(),
        });
        job.environment = Some(EnvSpec::Uv {
            requirements: Some(req.display().to_string()),
        });

        let k1 = job_cache_key(&job, None).unwrap();
        std::fs::write(&req, "numpy==2.0").unwrap();
        let k2 = job_cache_key(&job, None).unwrap();

        assert_ne!(k1, k2, "env file content must enter the cache key");
    }

    /// The script file appears in the provenance input pairs, bound to
    /// its path.
    #[test]
    fn script_appears_in_provenance_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("script.py");
        std::fs::write(&script, "print('hi')").unwrap();

        let job = make_job(ExecutionBlock::Script {
            path: script.clone(),
            lang: None,
        });
        let components = job_cache_key_with_components(&job, None).unwrap();
        let normalized_script = std::fs::canonicalize(&script)
            .unwrap()
            .display()
            .to_string();
        assert!(
            components
                .input_hashes
                .iter()
                .any(|(p, _)| p == &normalized_script),
            "script path must appear among provenance inputs"
        );
    }
}
