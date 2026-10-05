//! Implementation of the `ox run` command.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
    ConcreteJob, ContentHash, Event, ExecutionBlock, GateId, JobId, OutputRef, ResourceValue,
    RunReason,
};
use ox_core::resolver::ResolveRequest;
use ox_core::resource::{TOKEN_SCALE, TokenAmount, normalize_resources};
use ox_core::scheduler::{self, FailedJobDetail, SchedulerConfig};
use ox_core::traits::benchmark::{self, BenchmarkSink};
use ox_core::traits::cache::{CacheCheck, OutputHashes};
use ox_core::traits::executor::{ExecContext, Executor, JobResult};
use ox_exec_local::executor::LocalExecutor;
use ox_exec_ray::{RayConfig, RayExecutor};
use ox_exec_slurm::executor::{SlurmConfig, SlurmExecutor};
use ox_plan::critical_path::CriticalPathPass;

use super::common;
use super::remote_follow::{self, FollowPolicy, FollowRequest, FollowStop};

/// Human-readable label and resolved text for a dry-run execution block.
///
/// Shell and inline-run blocks retain their exact resolved text. Script and
/// call blocks use a compact invocation notation because their executors add
/// runtime machinery that is not part of the workflow declaration.
fn dry_run_execution(execution: &ExecutionBlock) -> (String, String) {
    match execution {
        ExecutionBlock::Shell { command } => ("shell".into(), command.clone()),
        ExecutionBlock::Run { code, lang } => (format!("run, {lang}"), code.clone()),
        ExecutionBlock::Script { path, lang } => (
            "script".into(),
            format!("{} {}", lang.as_deref().unwrap_or("sh"), path.display()),
        ),
        ExecutionBlock::Call { function, lang } => ("call".into(), format!("{lang} {function}")),
    }
}

/// Print execution text as a line-marked block. Splitting on `\n` preserves
/// blank lines (including a final one), while the marker makes embedded
/// indentation and multi-line commands unambiguous to a human reader.
fn print_dry_run_execution(execution: &ExecutionBlock) {
    let (label, text) = dry_run_execution(execution);
    println!("    execution ({label}):");
    for line in text.split('\n') {
        println!("      | {line}");
    }
}

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

    /// Per-run local resource capacity for job admission (repeatable; comma-separated).
    ///
    /// Uses the portable resource names and units, for example
    /// `--resource-budget cpu=6,mem_gb=32 --resource-budget metal=1`.
    /// This limits resources held by admitted jobs; it does not detect host
    /// capacity or coordinate with other `ox run` processes. In mixed SLURM
    /// runs it applies only to jobs routed locally. Pure remote runs reject it.
    #[arg(long, value_name = "KEY=VALUE", value_delimiter = ',')]
    pub resource_budget: Vec<String>,

    /// Filter by rule name (exact or /regex/)
    #[arg(long)]
    pub rule: Option<String>,

    /// Output NDJSON events
    #[arg(long)]
    pub json: bool,

    /// Write NDJSON events to this file (one JSON object per line)
    #[arg(long, value_name = "PATH")]
    pub report_json: Option<String>,

    /// Show jobs and expanded execution blocks without executing
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
    ///
    /// SLURM graphs with rule-level local overrides run through the scheduler
    /// and wait for completion. Ray DAG runs reject local overrides.
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

    /// Allow Ray tasks to wait for nodes with missing capacity (Ray only)
    #[arg(long)]
    pub ray_allow_pending: bool,

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
pub(super) fn output_file_paths(job: &ConcreteJob) -> Vec<PathBuf> {
    job.outputs
        .iter()
        .filter_map(|o| match &o.reference {
            OutputRef::File(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn input_file_paths(job: &ConcreteJob) -> Vec<PathBuf> {
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
pub(super) struct CacheKeyComponents {
    /// The final cache key (BLAKE3 of all components).
    pub(super) cache_key: ContentHash,
    /// Content hashes of each input file, paired with their path.
    pub(super) input_hashes: Vec<(String, String)>,
    /// BLAKE3 hash of the job specification (rule source + params + env).
    pub(super) job_spec_hash: String,
    /// Hash of the resolved wildcard bindings, `None` when the job has none.
    pub(super) params_hash: Option<String>,
    /// Content hash of the environment spec, `None` when undeclared.
    pub(super) env_hash: Option<String>,
}

fn job_spec_parts(job: &ConcreteJob) -> (String, Option<String>, Option<String>, String) {
    let rule_source = execution_source(job);
    let params_hash = (!job.wildcards.is_empty()).then(|| hash_kv_map(&job.wildcards));
    let env_hash = job.environment.as_ref().map(env_spec_content_hash);
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
        job.shell_executable.as_deref().map(str::as_bytes),
    );
    update_field(
        &mut spec_hasher,
        "clean_outputs",
        job.clean_outputs.to_string().as_bytes(),
    );
    (
        rule_source,
        params_hash,
        env_hash,
        spec_hasher.finalize().to_hex().to_string(),
    )
}

/// Reconstruct a local cache key from transferred provenance without reading
/// source inputs that may intentionally be absent on this machine.
pub(super) fn job_cache_key_from_provenance(
    job: &ConcreteJob,
    provenance: &ox_core::model::ArtifactProvenance,
) -> Result<ContentHash> {
    let (rule_source, params_hash, env_hash, job_spec_hash) = job_spec_parts(job);
    if job_spec_hash != provenance.job_spec_hash {
        bail!(
            "manifest job specification does not match rule '{}'",
            job.rule
        );
    }
    let inputs = provenance
        .input_hashes
        .iter()
        .map(|(path, hash)| {
            ContentHash::from_hex(hash.clone())
                .map(|hash| (path.clone(), hash))
                .with_context(|| format!("invalid input hash for {path}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let platform = current_platform();
    Ok(compute_cache_key(&CacheKeySpec {
        rule_source: &rule_source,
        inputs: &inputs,
        params_hash: params_hash.as_deref(),
        env_hash: env_hash.as_deref(),
        shell_executable: job.shell_executable.as_deref(),
        clean_outputs: job.clean_outputs,
        platform_scope: job.platform_scope,
        platform: &platform,
    }))
}

/// Compute cache key for a job, returning components for provenance tracking.
///
/// When `store` is provided, input file hashes use the mtime-based fast path:
/// if a file's mtime+size haven't changed since the last run, the stored
/// BLAKE3 hash is reused without re-reading the file. This reduces cache-key
/// computation for large, unchanged inputs from full-file I/O to a single
/// `stat()` call.
///
/// See `job_cache_key` (test-only) for the version that discards components.
pub(super) fn job_cache_key_with_components(
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

    let (rule_source, params_hash, env_hash, job_spec_hash) = job_spec_parts(job);
    let shell_executable = job.shell_executable.as_deref();

    let platform = current_platform();
    let cache_key = compute_cache_key(&CacheKeySpec {
        rule_source: &rule_source,
        inputs: &input_pairs,
        params_hash: params_hash.as_deref(),
        env_hash: env_hash.as_deref(),
        shell_executable,
        clean_outputs: job.clean_outputs,
        platform_scope: job.platform_scope,
        platform: &platform,
    });

    Some(CacheKeyComponents {
        cache_key,
        input_hashes: input_pairs
            .into_iter()
            .map(|(p, h)| (p, h.to_string()))
            .collect(),
        job_spec_hash,
        params_hash,
        env_hash,
    })
}

/// Compute cache key for a job by hashing its inputs, rule source, and env.
///
/// Thin wrapper over [`job_cache_key_with_components`] that discards the
/// provenance components.
#[cfg(test)]
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
    /// What each job was keyed on, collected as the cache layer computes
    /// it, so the run can write it into the audit trail afterwards (#12).
    /// Keyed by `JobId`; a job the cache layer never keyed is absent.
    provenance: Mutex<HashMap<String, ox_state::db::JobProvenance>>,
}

/// Build the audit-trail provenance for one job from the components its
/// cache key was computed from.
///
/// `output_hashes` is the cache entry's recorded output hashes when they
/// are known (after a `record`, or on a hit against a stored entry).
pub(super) fn job_provenance(
    job: &ConcreteJob,
    components: &CacheKeyComponents,
    output_hashes: Option<&std::collections::BTreeMap<String, ContentHash>>,
) -> ox_state::db::JobProvenance {
    let artifact = ox_core::model::ArtifactProvenance {
        input_hashes: components.input_hashes.clone(),
        job_spec_hash: components.job_spec_hash.clone(),
        reproducibility: job.reproducibility,
    };
    let output_hashes = output_hashes.and_then(|map| {
        let plain: std::collections::BTreeMap<&str, &str> =
            map.iter().map(|(p, h)| (p.as_str(), h.as_str())).collect();
        serde_json::to_string(&plain).ok()
    });
    ox_state::db::JobProvenance {
        cache_key: Some(components.cache_key.as_str().to_string()),
        input_hashes: serde_json::to_string(&components.input_hashes).ok(),
        output_hashes,
        params_hash: components.params_hash.clone(),
        env_hash: components.env_hash.clone(),
        reproducibility_class: Some(job.reproducibility.to_string()),
        artifact_provenance_json: serde_json::to_string(&artifact).ok(),
    }
}

/// Restore all outputs for a known local cache entry from a shared artifact
/// directory, then validate their hashes through `CacheStore`.
pub(super) async fn restore_remote_outputs(
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
            provenance: Mutex::new(HashMap::new()),
        }
    }

    /// Remember what a job was keyed on, for the audit trail.
    async fn note_provenance(&self, job: &ConcreteJob, prov: ox_state::db::JobProvenance) {
        self.provenance
            .lock()
            .await
            .insert(job.id.as_str().to_string(), prov);
    }
}

/// Lease under which a peer session counts as alive, from the environment.
fn resolve_lease_secs() -> u64 {
    std::env::var(ox_state::claim::LEASE_ENV_VAR)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(ox_state::claim::DEFAULT_LEASE_SECS)
}

/// Record a run in which every job was a cache hit.
///
/// The all-cached fast path never reaches the scheduler, so nothing else
/// writes to `state.db` for it.  Without this the state database could not
/// answer "did this run hit the cache?" — the question issue #12 is about.
///
/// Best-effort throughout: a state database that cannot be opened or
/// written is a degraded audit trail, never a failed run.
fn record_fully_cached_run(
    state_db_path: &Path,
    run_id: &str,
    job_graph: &JobGraph,
    executor: &str,
    note: Option<&str>,
    provenance: &HashMap<String, ox_state::db::JobProvenance>,
    cached: &HashSet<JobId>,
) {
    let Ok(db) = ox_state::db::StateDb::open(state_db_path) else {
        return;
    };
    let job_ids = job_graph.topological_order().unwrap_or_default();
    let records: Vec<ox_state::db::JobRecord> = job_ids
        .iter()
        .filter_map(|job_id| {
            job_graph
                .get_job(job_id)
                .map(|job| ox_state::db::JobRecord {
                    id: job_id.as_str().to_string(),
                    rule_name: job.rule.as_str().to_string(),
                    wildcards: serde_json::to_string(&job.wildcards).unwrap_or_default(),
                    cache_key: None,
                    run_id: Some(run_id.to_string()),
                })
        })
        .collect();

    // The run record comes first: jobs.run_id references runs(id).
    let _ = db.begin_run(run_id, None, records.len(), note);
    let _ = db.register_jobs(&records);
    // Rows a dead session left `running` / `failed` / `cancelled` are not
    // this run's verdict; a live peer's rows are left alone (ADR-012).
    let ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();
    let _ = db.reset_inactive_job_rows(&ids, resolve_lease_secs());

    // Only genuine cache hits are recorded as cached: a job excluded by
    // `--until` / `--omit-from` was not run *and* was not a hit, and
    // labelling it cached would overstate cache effectiveness.
    for job_id in job_ids.iter().filter(|id| cached.contains(id)) {
        let _ = db.skip_job(job_id.as_str());
    }
    let _ = db.record_job_cache_keys(run_id, provenance);
    let _ = db.finalize_job_history(
        run_id,
        executor,
        ox_state::host::hostname(),
        &HashMap::new(),
        &HashMap::new(),
        provenance,
    );
    let _ = db.end_run(run_id, 0, 0, ids.len());
}

fn cache_status_reason(status: CacheHitStatus) -> Result<(), RunReason> {
    match status {
        CacheHitStatus::Hit => Ok(()),
        CacheHitStatus::Miss => Err(RunReason::CacheMiss),
        CacheHitStatus::OutputMissing { path } => Err(RunReason::OutputMissing { path }),
        CacheHitStatus::Mismatch { path } => Err(RunReason::OutputStale { path }),
    }
}

impl CacheCheck for SchedulerCache {
    fn is_cached<'a>(
        &'a self,
        job: &'a ConcreteJob,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { self.check_with_reason(job).await.is_ok() })
    }

    fn check_with_reason<'a>(
        &'a self,
        job: &'a ConcreteJob,
    ) -> Pin<Box<dyn Future<Output = Result<(), RunReason>> + Send + 'a>> {
        Box::pin(async move {
            let output_paths = output_file_paths(job);
            if output_paths.is_empty() {
                return Err(RunReason::NotCacheable);
            }
            let output_refs: Vec<&Path> = output_paths.iter().map(|p| p.as_path()).collect();

            let mut store = self.store.lock().await;

            // Stateless mtime mode: pure filesystem comparison, no DB lookup.
            if store.validation() == CacheValidation::Mtime {
                let input_paths = input_file_paths(job);
                let input_refs: Vec<&Path> = input_paths.iter().map(|p| p.as_path()).collect();
                return match CacheStore::check_mtime_stateless(&input_refs, &output_refs) {
                    Ok(status) => cache_status_reason(status),
                    Err(_) => Err(RunReason::CacheMiss),
                };
            }

            // DB-backed modes (MtimeHash, ContentHash): need cache key.
            // Keep the components: on a hit they are the only record of
            // what the decision was keyed on (#12).
            let components = match job_cache_key_with_components(job, Some(&mut *store)) {
                Some(c) => c,
                None => return Err(RunReason::NotCacheable),
            };
            let cache_key = components.cache_key.clone();
            if let Some(remote) = &self.remote {
                if restore_remote_outputs(remote.as_ref(), &mut store, &cache_key, &output_paths)
                    .await
                {
                    let entry = store.get(&cache_key);
                    let prov =
                        job_provenance(job, &components, entry.as_ref().map(|e| &e.output_hashes));
                    drop(store);
                    self.note_provenance(job, prov).await;
                    return Ok(());
                }
            }
            let hit = match store.check_cached(&cache_key, &output_refs) {
                Ok(status) => {
                    if let CacheHitStatus::Mismatch { ref path } = status {
                        eprintln!("cache: output hash mismatch for {}, re-executing", path,);
                    }
                    cache_status_reason(status)
                }
                Err(_) => Err(RunReason::CacheMiss),
            };
            if hit.is_ok() {
                let entry = store.get(&cache_key);
                let prov =
                    job_provenance(job, &components, entry.as_ref().map(|e| &e.output_hashes));
                drop(store);
                self.note_provenance(job, prov).await;
            }
            hit
        })
    }

    fn recorded_output_hashes<'a>(
        &'a self,
        job: &'a ConcreteJob,
    ) -> Pin<Box<dyn Future<Output = Option<OutputHashes>> + Send + 'a>> {
        Box::pin(async move {
            if job.outputs.is_empty()
                || job
                    .outputs
                    .iter()
                    .any(|o| !matches!(o.reference, OutputRef::File(_)))
            {
                return None;
            }
            let mut store = self.store.lock().await;
            if store.validation() == CacheValidation::Mtime {
                return None;
            }
            let components = job_cache_key_with_components(job, Some(&mut *store))?;
            store
                .get(&components.cache_key)
                .map(|entry| entry.output_hashes)
        })
    }

    fn record<'a>(&'a self, job: &'a ConcreteJob) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move { self.record_with_hashes(job, &OutputHashes::new()).await })
    }

    fn record_with_hashes<'a>(
        &'a self,
        job: &'a ConcreteJob,
        hashes: &'a OutputHashes,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
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
                input_hashes: components.input_hashes.clone(),
                job_spec_hash: components.job_spec_hash.clone(),
                reproducibility: job.reproducibility,
            };

            let cache_key = components.cache_key.clone();
            let output_refs: Vec<&Path> = output_paths.iter().map(|p| p.as_path()).collect();
            if let Err(e) = store.record_with_hashes(
                cache_key.clone(),
                &output_refs,
                Some(&provenance),
                job.platform_scope,
                hashes,
            ) {
                eprintln!("warning: failed to cache job {}: {e}", job.id.as_str());
                return;
            }

            let entry = store.get(&cache_key);
            let prov = job_provenance(job, &components, entry.as_ref().map(|e| &e.output_hashes));
            self.note_provenance(job, prov).await;

            if let Some(remote) = &self.remote {
                for output in &output_paths {
                    let Some(hash) = entry.as_ref().and_then(|entry| {
                        entry.output_hashes.get(output.to_string_lossy().as_ref())
                    }) else {
                        continue;
                    };
                    if let Err(e) = remote.store(hash, output).await {
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
/// Attach the workflow's `[gate.<name>]` declarations to the job graph.
///
/// For every gate, each job whose rule is listed in the gate's `before`
/// gets a blocking gate node (see [`JobGraph::add_gate`]); the scheduler
/// then holds those jobs until the gate is approved. `after` adds no edge:
/// a gate is evaluated only once a guarded job's own inputs are ready, so
/// rules listed in `after` must be upstream of the `before` rules through
/// the DAG (which is the case whenever they produce the guarded inputs).
///
/// Returns the number of (gate, job) blocks attached.
fn attach_gates(job_graph: &mut JobGraph, gates: &[ox_format::parse::Gate]) -> usize {
    let mut blocks: Vec<(GateId, JobId)> = Vec::new();
    for gate in gates {
        let gate_id = GateId::from(gate.name.as_str());
        for job_id in job_graph.job_ids() {
            let Some(job) = job_graph.get_job(job_id) else {
                continue;
            };
            if gate.before.iter().any(|rule| rule == job.rule.as_str()) {
                blocks.push((gate_id.clone(), job_id.clone()));
            }
        }
    }
    for (gate_id, job_id) in &blocks {
        job_graph.add_gate(gate_id, job_id);
    }
    blocks.len()
}

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
    // ray_allow_pending: false by default; an explicit true flag wins over false.
    if !args.ray_allow_pending {
        if let Some(true) = profile.ray_allow_pending {
            args.ray_allow_pending = true;
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

/// Read `open_dashboard` from the user-global config file.
fn resolve_global_config_open_dashboard() -> Option<bool> {
    let table = common::load_global_config()?;
    table.get("open_dashboard").and_then(|v| v.as_bool())
}

fn validate_ray_flags(args: &RunArgs) -> Result<()> {
    if args.ray_allow_pending && args.executor != "ray" {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--ray-allow-pending requires --executor ray\n",
        )
        .into());
    }
    Ok(())
}

fn validate_resource_budget_flags(args: &RunArgs, has_local_jobs: bool) -> Result<()> {
    if !args.resource_budget.is_empty()
        && args.executor != "local"
        && !(args.executor == "slurm" && has_local_jobs)
    {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--resource-budget applies to the local executor only (including local overrides in SLURM runs); Ray and SLURM map a rule's declared resources onto their own backend requests\n",
        )
        .into());
    }
    Ok(())
}

fn budget_usage_error(message: String) -> anyhow::Error {
    clap::Error::raw(
        clap::error::ErrorKind::ValueValidation,
        format!("{message}\n"),
    )
    .into()
}

/// Parse CLI capacities through the same checked resource normalization used
/// for rule declarations and executor adapters.
///
/// `SchedulerConfig` expresses token capacities as whole counts, while C0
/// permits fractional *demands*. Memory has already been converted to bytes
/// by the normalizer before it reaches the scheduler.
fn parse_resource_budget(entries: &[String]) -> Result<BTreeMap<String, u64>> {
    let mut raw = BTreeMap::new();
    for entry in entries {
        let (key, value) = entry.split_once('=').ok_or_else(|| {
            budget_usage_error(format!(
                "invalid --resource-budget entry {entry:?}: expected KEY=VALUE"
            ))
        })?;
        if key.is_empty() || value.is_empty() {
            return Err(budget_usage_error(format!(
                "invalid --resource-budget entry {entry:?}: expected KEY=VALUE"
            )));
        }
        if raw
            .insert(key.to_owned(), ResourceValue::Str(value.to_owned()))
            .is_some()
        {
            return Err(budget_usage_error(format!(
                "invalid --resource-budget entry {entry:?}: resource {key:?} is declared more than once"
            )));
        }
    }

    let normalized = normalize_resources(&raw).map_err(|error| {
        let entry = match &error {
            ox_core::resource::ResourceError::Duplicate { second_key, .. } => raw
                .iter()
                .find(|(key, _)| *key == second_key)
                .map(|(key, value)| format!("{key}={value}")),
            ox_core::resource::ResourceError::InvalidValue { key, .. } => raw
                .get_key_value(key)
                .map(|(key, value)| format!("{key}={value}")),
            ox_core::resource::ResourceError::EmptyName => None,
        };
        match entry {
            Some(entry) => budget_usage_error(format!(
                "invalid --resource-budget entry {entry:?}: {error}"
            )),
            None => budget_usage_error(format!("invalid --resource-budget: {error}")),
        }
    })?;

    let mut capacity = BTreeMap::new();
    if let Some(cpu) = normalized.cpu {
        capacity.insert("cpu".into(), whole_token_capacity("cpu", cpu)?);
    }
    if let Some(gpu) = normalized.gpu {
        capacity.insert("gpu".into(), whole_token_capacity("gpu", gpu)?);
    }
    if let Some(memory_bytes) = normalized.memory_bytes {
        capacity.insert("memory".into(), memory_bytes);
    }
    for (name, amount) in normalized.custom {
        capacity.insert(
            format!("custom:{name}"),
            whole_token_capacity(&format!("custom:{name}"), amount)?,
        );
    }
    Ok(capacity)
}

fn whole_token_capacity(resource: &str, amount: TokenAmount) -> Result<u64> {
    let scaled = amount.ten_thousandths();
    if scaled % TOKEN_SCALE != 0 {
        return Err(budget_usage_error(format!(
            "resource budget capacity for {resource} must be a whole token"
        )));
    }
    Ok(scaled / TOKEN_SCALE)
}

fn local_executor(args: &RunArgs, event_bus: &EventBus) -> LocalExecutor {
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
        let pool = Arc::new(ox_exec_local::worker_pool::WorkerPool::new_with_mode(
            project_dir,
            warm_mode,
        ));
        executor = executor.with_worker_pool(pool);
    }
    executor
}

pub fn cmd_run(mut args: RunArgs, theme: &ox_render::Theme) -> Result<()> {
    if args.profile.is_none() {
        validate_ray_flags(&args)?;
    }
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

    validate_ray_flags(&args)?;
    let resource_budget = parse_resource_budget(&args.resource_budget)?;

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
    // Rule outputs must never masquerade as source files. The cache pre-scan
    // below decides which jobs are up to date after resolution has built the
    // complete graph. Adopted outputs (ox cache-import) are the one exception:
    // their source inputs may not exist here, so they stay resolver leaves.
    let existing_files =
        common::discover_source_files(&file_path, &workflow, &config, !args.no_cache);

    timer.mark("discover_files");

    let request = ResolveRequest {
        targets: targets.clone(),
        config,
        existing_files,
    };

    let mut resolve_result = common::resolve(&file_path, &workflow.rules, &request)
        .context("failed to resolve targets")?;

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
    let mut job_graph = JobGraph::build(resolve_result.jobs).context("failed to build JobGraph")?;
    // Attach `[gate.<name>]` nodes: every job of a rule listed in a gate's
    // `before` is blocked until the gate is approved (issue #2).
    let gated_jobs = attach_gates(&mut job_graph, &workflow.gates);
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

    let selected_local_jobs: Vec<_> = job_graph
        .job_ids()
        .into_iter()
        .filter(|id| !selective_skip.contains(*id))
        .filter_map(|id| job_graph.get_job(id))
        .filter(|job| args.executor == "local" || job.executor.as_deref() == Some("local"))
        .cloned()
        .collect();
    validate_resource_budget_flags(&args, !selected_local_jobs.is_empty())?;

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
                            "execution": &job.execution,
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
                        print_dry_run_execution(&job.execution);
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
                                    job.platform_scope,
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

    // Admission is a property of the selected DAG, even when cache skips all work.
    ox_core::scheduler::validate_resource_budget(
        &JobGraph::build(selected_local_jobs)?,
        &resource_budget,
    )?;

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
    let cache_validation =
        common::resolve_cache_validation(args.cache_validation.as_deref(), &workflow)?;

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

    let remote_scan = remote_cache.as_deref().map(|remote| (remote, &rt));
    let execution = super::execution_plan::determine_execution(
        &job_graph,
        !args.no_cache,
        cache_validation,
        cache_store.as_mut(),
        remote_scan,
    );
    let mut skip_jobs = execution.skip_jobs;
    let run_reasons = execution.run_reasons;
    // What the shared prescan keyed each cache hit on, for the audit trail
    // (#12). A fully cached run computes nothing else, so this is the only
    // provenance it has.
    let prescan_provenance = execution.provenance;

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

    // Jobs skipped because the cache answered for them, as opposed to jobs
    // excluded by `--until` / `--omit-from`.
    let cache_hits: HashSet<JobId> = skip_jobs.difference(&selective_skip).cloned().collect();
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
        // A fully cached run used to leave no trace at all: it returned
        // here before state.db was even opened, so `jobs.cached` kept
        // whatever the run that executed the jobs had written (0), and
        // there was no `runs` row for the run that hit the cache (#12).
        // Recording costs one transaction plus one write per job, paid
        // only on the warm path it documents.
        std::fs::create_dir_all(&oxymake_dir).ok();
        record_fully_cached_run(
            &oxymake_dir.join("state.db"),
            &format!("run-{}", std::process::id()),
            &job_graph,
            &args.executor,
            args.note.as_deref(),
            &prescan_provenance,
            &cache_hits,
        );
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

    let local_override = job_graph
        .job_ids()
        .into_iter()
        .filter(|id| !skip_jobs.contains(*id))
        .filter_map(|id| job_graph.get_job(id))
        .find(|job| job.executor.as_deref() == Some("local"));
    if args.executor == "ray" {
        if let Some(job) = local_override {
            bail!(
                "rule '{}' declares executor = \"local\", which Ray DAG submission cannot honour on the submitting host; run these targets separately with ox run --executor local",
                job.rule
            );
        }
    }
    let mixed_slurm = args.executor == "slurm" && local_override.is_some();

    if mixed_slurm {
        eprintln!(
            "Notice: this run contains a local override and uses scheduler dispatch instead of DAG submission. \
             --jobs={} bounds concurrent jobs, including cluster submissions (default: 1). \
             The ox process must stay alive until the run completes.",
            args.jobs
        );
    }

    // Execute via the scheduler.
    //
    // Gates are enforced by the scheduler's GateCheck. The slurm and ray
    // branches below bypass the scheduler (`submit_dag`), so a gated
    // workflow would run unguarded there: refuse instead of silently
    // dropping the approval step.
    if gated_jobs > 0 && args.executor != "local" && !mixed_slurm {
        bail!(
            "this workflow declares {} gate(s) but `--executor {}` submits the DAG \
             without the scheduler, so gates cannot block jobs there; run with \
             `--executor local` or remove the [gate.*] tables",
            workflow.gates.len(),
            args.executor
        );
    }

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
        resource_budget,
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
    // Resolved once here and reused for the session row and for every
    // job_history row: the audit trail has to say which machine ran the
    // job, and both sites used to write the literal "localhost" (#12).
    let hostname = ox_state::host::hostname();
    let session_id = if let Some(ref db) = state_db {
        let pid = std::process::id();
        db.create_session(pid, hostname, None).ok()
    } else {
        None
    };
    // Lease of this session's claims: a session whose heartbeat is older
    // than this is dead to its peers and its running jobs are reclaimed.
    let lease_secs = resolve_lease_secs();

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

        // Rows left behind by sessions that are no longer live (yesterday's
        // run, a crashed peer) are reset so this run re-evaluates them;
        // rows owned by a live peer are kept for the claim protocol (ADR-012).
        let ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();
        if let Err(e) = db.reset_inactive_job_rows(&ids, lease_secs) {
            eprintln!("warning: could not reset stale job rows in state.db: {e}");
        }

        // Persist job-to-job edges for DAG visualization.
        let edge_records: Vec<(String, String)> = job_graph
            .job_edges()
            .into_iter()
            .map(|(from, to)| (from.as_str().to_string(), to.as_str().to_string()))
            .collect();
        let _ = db.register_edges(&edge_records);
    }

    timer.mark("state_db_register");

    // Gate checker: only when the workflow declares gates. Without gates
    // the graph has no gate nodes and the checker would never be consulted,
    // so `None` is equivalent and saves a second state.db connection. With
    // gates, a missing state.db is fatal: the gate ledger *is* the
    // enforcement (`ox gate approve` writes to it), so running without it
    // would execute guarded rules unapproved.
    let gate_checker: Option<Arc<dyn ox_core::traits::gate::GateCheck>> = if gated_jobs > 0 {
        if state_db.is_none() {
            bail!(
                "this workflow declares gates, which are enforced through .oxymake/state.db, \
                 but the state database could not be opened (see warning above)"
            );
        }
        let db = ox_state::db::StateDb::open(&state_db_path)
            .context("failed to open state database for gate enforcement")?;
        Some(Arc::new(ox_state::gate::StateGateChecker::new(
            db,
            Some(run_id.clone()),
        )))
    } else {
        None
    };

    // Job claimer: the cooperative claim protocol (ADR-012) as the scheduling
    // gate. Every job is claimed in state.db before dispatch; a claim lost to
    // a live peer makes this session wait for the peer's result instead of
    // executing the job a second time. Without a session there is nothing to
    // coordinate through, and the scheduler runs uncoordinated (the local
    // executor's output-path locks still fail closed).
    let job_claimer: Option<Arc<dyn ox_core::traits::claim::JobClaim>> = match &session_id {
        Some(sid) => match ox_state::db::StateDb::open(&state_db_path) {
            Ok(db) => Some(Arc::new(ox_state::claim::StateJobClaimer::new(
                db,
                sid.clone(),
                lease_secs,
            ))),
            Err(e) => {
                eprintln!(
                    "warning: state database unavailable for job claims ({e}); \
                     concurrent sessions are not coordinated"
                );
                None
            }
        },
        None => None,
    };

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
    let job_executors: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

    // Preserve only executor-scoped peak-memory observations. Missing values
    // stay absent so history can distinguish them from a measured zero (#24).
    let job_peak_memory_bytes: Arc<Mutex<HashMap<String, u64>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Set once the first shutdown signal is seen, so the post-scheduler
    // finalisation knows this run was interrupted rather than merely finished
    // with cancellations.
    let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let interrupted_outer = interrupted.clone();
    let mut remote_stop = None;
    let remote_following =
        args.follow && !mixed_slurm && matches!(args.executor.as_str(), "ray" | "slurm");
    let result = rt.block_on(async {
        let interrupted = interrupted_outer;
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

        // Heartbeat: keeps this session's claims alive for its peers. Stops
        // when the scheduler returns (aborted below); a session killed
        // outright stops heartbeating and its jobs are reclaimed by the
        // first peer that finds the heartbeat older than the lease.
        let heartbeat_handle = session_id.as_ref().map(|sid| {
            let sid = sid.clone();
            let db_path = state_db_path.clone();
            let period = ox_state::claim::heartbeat_interval(lease_secs);
            tokio::spawn(async move {
                let db = match ox_state::db::StateDb::open(&db_path) {
                    Ok(db) => db,
                    Err(_) => return,
                };
                let mut ticker = tokio::time::interval(period);
                ticker.tick().await; // first tick fires immediately; the session row is fresh
                loop {
                    ticker.tick().await;
                    let _ = db.heartbeat(&sid);
                }
            })
        });

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
            let executors = Arc::clone(&job_executors);
            let peak_memory = Arc::clone(&job_peak_memory_bytes);
            bridge_handles.push(tokio::spawn(async move {
                let db = match ox_state::db::StateDb::open(&db_path) {
                    Ok(db) => db,
                    Err(_) => return,
                };
                loop {
                    match rx.recv().await {
                        Ok(event) => match event {
                            Event::JobStarted {
                                ref job_id,
                                ref executor,
                                ..
                            } => {
                                executors
                                    .lock()
                                    .await
                                    .insert(job_id.to_string(), executor.clone());
                                let _ = db.claim_job(job_id.as_str(), &sid);
                            }
                            Event::JobCompleted {
                                ref job_id,
                                duration_ms,
                                peak_memory_bytes,
                                ..
                            } => {
                                // Claim first in case JobStarted was never received.
                                let _ = db.claim_job(job_id.as_str(), &sid);
                                let _ = db.complete_job(job_id.as_str(), &sid, 0, "");
                                durations
                                    .lock()
                                    .await
                                    .insert(job_id.as_str().to_string(), duration_ms);
                                if let Some(bytes) = peak_memory_bytes {
                                    peak_memory
                                        .lock()
                                        .await
                                        .insert(job_id.as_str().to_string(), bytes);
                                }
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
                                // Never cancel a row a live peer is executing:
                                // a job this session only waited for stays the
                                // peer's (ADR-012).
                                let _ = db.cancel_job_ids_for_session(&[job_id.to_string()], &sid);
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
            let interrupted_session = session_id.clone();
            let db_path = state_db_path.clone();
            let interrupted = interrupted.clone();
            tokio::spawn(async move {
                #[cfg(unix)]
                let mut sigterm =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
                #[cfg(not(unix))]
                let mut sigterm = ();

                // First signal: graceful shutdown.
                wait_for_shutdown_signal(&mut sigterm).await;
                interrupted.store(true, std::sync::atomic::Ordering::SeqCst);
                // Record the interruption before any job is terminalized, so
                // a peer waiting on one of our jobs reads the failure that
                // follows as an interrupted session's leftover (re-run it)
                // rather than a verdict to mirror (ADR-012).
                if let Some(sid) = &interrupted_session {
                    if let Ok(db) = ox_state::db::StateDb::open(&db_path) {
                        let _ = db.interrupt_session(sid);
                    }
                }
                // Clear progress bars so the message is visible.
                if let Some(ref multi) = progress_multi {
                    let _ = multi.clear();
                }
                if remote_following {
                    eprintln!("\nInterrupted — stopping remote follow…");
                } else {
                    eprintln!("\nInterrupted — waiting for running jobs to exit…");
                }
                shutdown.notify_waiters();

                // Second signal: force-exit. Without this, tokio's signal
                // hook swallows subsequent SIGINTs (ox-2sek).
                wait_for_shutdown_signal(&mut sigterm).await;
                // The ledger must not be left describing a state that no
                // longer exists (#4): before exiting, terminalize the rows
                // this session still holds as `running`. A handful of
                // synchronous SQLite statements — it must not await the
                // scheduler, which is exactly what the operator gave up on.
                // Scoped to our own session_id: a live peer's row is never
                // ours to terminalize (ADR-012). The session keeps the
                // `interrupted` status written on the first signal.
                if !remote_following && let Some(sid) = &interrupted_session {
                    if let Ok(db) = ox_state::db::StateDb::open(&db_path) {
                        if let Ok(ids) = db.running_job_ids_for_session(sid) {
                            let _ = db.cancel_job_ids_for_session(&ids, sid);
                        }
                    }
                }
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
                let executor = local_executor(&args, &event_bus);
                let bench_sink: Option<Arc<dyn BenchmarkSink>> = Some(Arc::new(FsBenchmarkSink));
                scheduler::run_scheduler_with_claims(
                    &job_graph,
                    Arc::new(executor),
                    &scheduler_config,
                    &event_bus,
                    &ctx,
                    scheduler_cache.clone(),
                    gate_checker.clone(),
                    bench_sink,
                    Some(shutdown.clone()),
                    disk_writer_handle.clone(),
                    job_claimer.clone(),
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
                let endpoint = slurm_config
                    .api_url
                    .clone()
                    .unwrap_or_else(|| "SLURM scheduler via local sacct/squeue".into());
                let executor = SlurmExecutor::new(slurm_config, event_bus.clone());
                // Pre-flight: verify SLURM CLI tools are available before
                // scheduling any jobs. Without this, a missing `sbatch`
                // silently produces a "0 succeeded, 0 failed" result.
                executor.init().await.map_err(|e| {
                    ox_core::error::OxError::Exec(ox_core::error::ExecError::Executor {
                        message: format!("SLURM executor init failed: {e}"),
                    })
                })?;

                if mixed_slurm {
                    let executor = ox_core::traits::local_override::LocalOverrideExecutor::new(
                        executor,
                        local_executor(&args, &event_bus),
                        &job_graph,
                    );
                    scheduler::run_scheduler_with_claims(
                        &job_graph,
                        Arc::new(executor),
                        &scheduler_config,
                        &event_bus,
                        &ctx,
                        scheduler_cache.clone(),
                        gate_checker.clone(),
                        Some(Arc::new(FsBenchmarkSink)),
                        Some(shutdown.clone()),
                        disk_writer_handle.clone(),
                        job_claimer.clone(),
                    )
                    .await
                } else {
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
                        eprintln!("Following execution progress…\n");
                        let (result, stop) = remote_follow::follow(
                            FollowRequest {
                                jobs: dag_result.job_submissions.keys().cloned().collect(),
                                total: dag_result.total_jobs,
                                skipped: dag_result.skipped,
                                endpoint: &endpoint,
                                policy: FollowPolicy::new(std::time::Duration::from_secs(5)),
                                interrupted: &interrupted,
                                shutdown: &shutdown,
                            },
                            |job_id_str| {
                                let executor = &executor;
                                let state_db = &state_db;
                                let session_id = &session_id;
                                let job_durations = &job_durations;
                                let job_peak_memory_bytes = &job_peak_memory_bytes;
                                async move {
                                    use ox_core::traits::executor::JobStatus;
                                    let jid = JobId::from(job_id_str.as_str());
                                    let (status, record) =
                                        executor.poll_status_with_record(&jid).await?;
                                    if let (Some(db), Some(sid)) = (&state_db, &session_id) {
                                        match &status {
                                            JobStatus::Completed
                                            | JobStatus::Failed(_)
                                            | JobStatus::Running => {
                                                let _ = db.claim_job(&job_id_str, sid);
                                            }
                                            _ => {}
                                        }
                                        match &status {
                                            JobStatus::Completed => {
                                                let _ = db.complete_job(&job_id_str, sid, 0, "");
                                            }
                                            JobStatus::Failed(_) => {
                                                let exit = record
                                                    .as_ref()
                                                    .map(|r| r.exit_code)
                                                    .filter(|c| *c != 0)
                                                    .unwrap_or(1);
                                                let _ = db.fail_job(&job_id_str, sid, exit);
                                            }
                                            JobStatus::Cancelled => {
                                                let _ = db.cancel_job_ids_for_session(
                                                    std::slice::from_ref(&job_id_str),
                                                    sid,
                                                );
                                            }
                                            _ => {}
                                        }
                                    }

                                    if matches!(status, JobStatus::Completed)
                                        && let Some(record) = record
                                    {
                                        job_durations.lock().await.insert(
                                            job_id_str.clone(),
                                            record.elapsed.as_millis() as u64,
                                        );
                                        if let Some(bytes) = record.peak_memory_bytes {
                                            job_peak_memory_bytes
                                                .lock()
                                                .await
                                                .insert(job_id_str, bytes);
                                        }
                                    }
                                    Ok::<_, ox_exec_slurm::error::SlurmError>(status)
                                }
                            },
                            |line| eprintln!("{line}"),
                        )
                        .await;
                        remote_stop = stop;
                        Ok(result)
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
            }
            "ray" => {
                let ray_config = RayConfig {
                    allow_pending: args.ray_allow_pending,
                    dashboard_address: args
                        .ray_address
                        .clone()
                        .unwrap_or_else(|| "http://127.0.0.1:8265".to_string()),
                    max_submit: Some(args.jobs),
                    // Native Ray DAG drivers are submitted by absolute path.
                    // This staging root follows the Oxymakefile, including
                    // when `ox run -f <path>` was invoked elsewhere.
                    working_dir: ctx.project_dir.join(".oxymake/runs"),
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
                    eprintln!("Following execution progress…\n");
                    // Preserve the existing follow block formatting.
                    #[rustfmt::skip]
                    let (result, stop) = remote_follow::follow(FollowRequest {
                        jobs: dag_result.job_submissions.keys().cloned().collect(),
                        total: dag_result.total_jobs, skipped: dag_result.skipped,
                        endpoint: dashboard_url,
                        policy: FollowPolicy::new(std::time::Duration::from_secs(3)),
                        interrupted: &interrupted, shutdown: &shutdown,
                    }, |job| {
                        let executor = &executor;
                        async move { executor.poll_status(&JobId::from(job.as_str())).await }
                    }, |line| eprintln!("{line}")).await;
                    // The controller drops an in-flight poll on the first signal.
                    // Restore Ray's driver cancellation before leaving follow;
                    // a failed stop must not claim or record cancellation.
                    remote_stop = if matches!(stop, Some(FollowStop::Interrupted)) {
                        let cancellation_error = executor
                            .cancel(&JobId::from(dag_result.run_id.as_str()))
                            .await
                            .err()
                            .map(|error| error.to_string());
                        if cancellation_error.is_none()
                            && let Some(db) = &state_db
                        {
                            let ids = dag_result
                                .job_submissions
                                .keys()
                                .cloned()
                                .collect::<Vec<_>>();
                            let _ = db.cancel_job_ids(&ids);
                        }
                        Some(FollowStop::RayInterrupted { cancellation_error })
                    } else {
                        stop
                    };
                    Ok(result)
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
            (args.executor == "ray" || args.executor == "slurm") && !args.follow && !mixed_slurm;
        if !skip_reporter_finish && remote_stop.is_none() {
            if let (Some(reporter), Ok(r)) = (&term_reporter, &sched_result) {
                use ox_core::traits::reporter::{Reporter, RunSummary};
                let summary = RunSummary {
                    total_jobs: r.total_jobs,
                    succeeded: r.succeeded,
                    failed: r.failed,
                    skipped: r.skipped,
                    cancelled: r.cancelled,
                    duration_ms: r.duration.as_millis() as u64,
                };
                reporter.finish(&summary).await;
            }
        }

        // The scheduler has returned: this session holds no live claims any
        // more, stop heartbeating.
        if let Some(handle) = heartbeat_handle {
            handle.abort();
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
    let peak_memory_bytes = job_peak_memory_bytes.blocking_lock();
    if let Some(db) = &state_db {
        // Mark any jobs still in 'running' state as failed. This catches
        // jobs that crashed without emitting a JobFailed event.
        let had_failures = match &result {
            Ok(r) => r.failed > 0,
            Err(_) => true,
        };
        let was_interrupted = interrupted.load(std::sync::atomic::Ordering::SeqCst);
        // Scoped to this run's session: in cooperative multi-session mode
        // another live session's running jobs are not ours to terminalize
        // (H16 / ADR-012).
        if remote_stop.is_none()
            && let Some(sid) = &session_id
        {
            if was_interrupted {
                // An interrupted run's leftovers are cancellations, not
                // failures: the scheduler asked these jobs to stop (#4).
                if let Ok(ids) = db.running_job_ids_for_session(sid) {
                    let _ = db.cancel_job_ids_for_session(&ids, sid);
                }
            } else if had_failures {
                if let Ok(running) = db.jobs_by_status("running") {
                    for job_id in &running {
                        let _ = db.fail_job(job_id.as_str(), sid, 1);
                    }
                }
            }
        }

        // Record audit-trail history entries from the post-flush DB state.
        // The jobs table already has rule_name, wildcards, status, timing,
        // and exit_code; the hashes the decision was keyed on exist only in
        // the cache layer, so they are threaded in from there (#12).
        // The prescan keyed the jobs it decided were cache hits; the
        // scheduler's cache layer keyed the rest as it ran them. Later wins.
        let mut cache_provenance = prescan_provenance.clone();
        if let Some(ref sc) = scheduler_cache_impl {
            cache_provenance.extend(sc.provenance.blocking_lock().clone());
        }
        let _ = db.record_job_cache_keys(&run_id, &cache_provenance);
        let _ = db.finalize_job_history_with_executors(
            &run_id,
            &args.executor,
            hostname,
            &durations,
            &peak_memory_bytes,
            &cache_provenance,
            &job_executors.blocking_lock(),
        );

        // A stopped follow did not complete, even without a signal. Close it
        // as interrupted; completed sessions also preserve prior interruption.
        // In either case, its terminal rows are then history to the next run,
        // which re-evaluates them instead of consuming them as a peer's.
        if let Some(sid) = &session_id {
            if remote_stop.is_some() {
                let _ = db.interrupt_session(sid);
            } else {
                let _ = db.complete_session(sid);
            }
        }
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
            if let Some(ref db) = state_db
                && (args.executor != "ray"
                    || args.follow
                    || sched_result.total_jobs == sched_result.skipped)
            {
                let _ = db.end_run(
                    &run_id,
                    sched_result.succeeded,
                    sched_result.failed,
                    sched_result.skipped,
                );
            }

            if let Some(stop) = &remote_stop {
                let message = stop.message();
                if args.json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "event": "run_follow_stopped", "executor": args.executor,
                            "reason": if stop.exit_code() == 130 { "interrupted" } else { "endpoint_unreachable" },
                            "message": message, "exit_code": stop.exit_code(),
                            "succeeded": sched_result.succeeded, "failed": sched_result.failed,
                            "skipped": sched_result.skipped,
                        })
                    );
                } else {
                    println!("{message}");
                    println!(
                        "Last known: {} succeeded, {} failed, {} skipped ({} total)",
                        sched_result.succeeded,
                        sched_result.failed,
                        sched_result.skipped,
                        sched_result.total_jobs
                    );
                }
                timer.print();
                std::process::exit(stop.exit_code());
            }

            // Fire-and-forget remote executors: show "Submitted" not "Completed".
            let is_fire_and_forget = (args.executor == "ray" || args.executor == "slurm")
                && !args.follow
                && !mixed_slurm;

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
            // An interrupted run did not complete its DAG, even when every
            // in-flight job was cancelled rather than failed: report the
            // conventional signal exit code instead of success.
            if interrupted.load(std::sync::atomic::Ordering::SeqCst) {
                std::process::exit(130);
            }
            Ok(())
        }
        Err(e) => {
            // Finalise the run record even on error. The scheduler returned
            // no result, but the jobs table was flushed above and is
            // authoritative, so the counts come from there instead of the
            // zeros that made an aborted run indistinguishable from an
            // empty one (#12). `runs` has no status column, so this is the
            // whole of the fix that the schema supports without a
            // migration.
            if let Some(ref db) = state_db {
                let (succeeded, failed, skipped) = db
                    .job_counts_for_run(&run_id)
                    .map(|c| (c.completed.saturating_sub(c.cached), c.failed, c.cached))
                    .unwrap_or((0, 0, 0));
                let _ = db.end_run(&run_id, succeeded, failed, skipped);
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
    use std::collections::BTreeMap;

    use super::*;
    use ox_core::model::{EnvSpec, ResourceValue, RuleName};

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
            clean_outputs: Default::default(),
            platform_scope: Default::default(),
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

    /// Resource declarations do not contribute to concrete job identity.
    #[test]
    fn resource_declarations_do_not_change_concrete_job_cache_key() {
        let mut job = make_job(ExecutionBlock::Shell {
            command: "echo resource-stable".into(),
        });
        job.resources = BTreeMap::from([
            ("cpus".into(), ResourceValue::Int(4)),
            ("mem_gb".into(), ResourceValue::Int(16)),
            ("metal".into(), ResourceValue::Float(0.5.into())),
        ]);

        // Compare distinct declarations on the same platform, not a golden hash.
        let mut other = job.clone();
        other.resources = BTreeMap::from([
            ("cpu".into(), ResourceValue::Int(1)),
            ("memory".into(), ResourceValue::Str("1GiB".into())),
        ]);
        assert_ne!(job.resources, other.resources);
        let before = job_cache_key(&other, None).unwrap();
        let raw = job.resources.clone();
        let normalized = ox_core::resource::normalize_resources(&job.resources)
            .expect("the declarations above are valid");
        assert!(normalized.cpu.is_some(), "normalization ran");
        let after = job_cache_key(&job, None).unwrap();
        assert_eq!(
            before.as_str(),
            after.as_str(),
            "resource declarations must not contribute to the concrete-job cache key"
        );
        assert_eq!(
            job.resources, raw,
            "normalization must leave the raw declarations untouched"
        );
    }

    #[test]
    fn threads_alias_fix_has_narrow_and_stable_cache_effect() {
        let cpu_before = make_job(ExecutionBlock::Shell {
            command: "echo --threads 2".into(),
        });
        let cpu_after = cpu_before.clone();
        assert_eq!(
            job_cache_key(&cpu_before, None),
            job_cache_key(&cpu_after, None),
            "the existing cpu spelling must preserve its cache identity"
        );

        let cpus_before = make_job(ExecutionBlock::Shell {
            command: "echo --threads {threads}".into(),
        });
        let cpus_after = make_job(ExecutionBlock::Shell {
            command: "echo --threads 2".into(),
        });
        let old_key = job_cache_key(&cpus_before, None);
        let new_key = job_cache_key(&cpus_after, None);
        assert_ne!(
            old_key, new_key,
            "fixing cpus interpolation must rerun once"
        );
        assert_eq!(
            new_key,
            job_cache_key(&cpus_after, None),
            "the corrected key must be stable across runs"
        );
    }

    #[test]
    fn clean_outputs_changes_cache_key_and_job_spec_hash() {
        use ox_core::model::CleanOutputs;
        let mut job = make_job(ExecutionBlock::Shell {
            command: "true".into(),
        });
        let mut keys = std::collections::HashSet::new();
        let mut specs = std::collections::HashSet::new();
        for policy in [
            CleanOutputs::Always,
            CleanOutputs::OnFailure,
            CleanOutputs::Never,
        ] {
            job.clean_outputs = policy;
            let components = job_cache_key_with_components(&job, None).unwrap();
            assert!(keys.insert(components.cache_key));
            assert!(specs.insert(components.job_spec_hash));
        }
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
            project: None,
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

#[cfg(test)]
mod resource_budget_tests {
    use std::collections::BTreeMap;

    use super::parse_resource_budget;

    #[test]
    fn resource_budget_uses_the_portable_alias_and_unit_contract() {
        let from_gib = parse_resource_budget(&["mem_gb=1".into()]).unwrap();
        let from_mib = parse_resource_budget(&["mem_mb=1024".into()]).unwrap();
        let from_bytes = parse_resource_budget(&["memory=1GiB".into()]).unwrap();

        let expected = BTreeMap::from([("memory".into(), 1_u64 << 30)]);
        assert_eq!(from_gib, expected);
        assert_eq!(from_mib, expected);
        assert_eq!(from_bytes, expected);
    }

    #[test]
    fn resource_budget_rejects_duplicate_aliases_across_flags() {
        let err = parse_resource_budget(&["cpu=6".into(), "cpus=6".into()]).unwrap_err();
        assert!(err.to_string().contains("cpu"));
        assert!(err.to_string().contains("more than once"));
    }

    #[test]
    fn resource_budget_names_the_malformed_entry() {
        let err = parse_resource_budget(&["memory=1XB".into()]).unwrap_err();
        assert!(err.to_string().contains("memory=1XB"));
    }

    #[test]
    fn resource_budget_requires_whole_token_capacities() {
        let err = parse_resource_budget(&["cpu=0.5".into()]).unwrap_err();
        assert!(err.to_string().contains("whole token"));
    }
}

#[cfg(test)]
mod resource_class_regressions {
    use std::path::Path;

    use ox_core::resolver::{Config, ResolveRequest, resolve};
    fn resolved_key(source: &str) -> ox_core::model::ContentHash {
        let workflow =
            ox_format::parse::parse_workflow(source, std::path::Path::new("test.toml")).unwrap();
        let job = resolve(
            &workflow.rules,
            &ResolveRequest {
                targets: vec!["out".into()],
                config: Config::default(),
                existing_files: vec![],
            },
        )
        .unwrap()
        .jobs
        .remove(0);
        super::job_cache_key_with_components(&job, None)
            .unwrap()
            .cache_key
    }
    #[test]
    fn resource_class_identity_is_the_resolved_execution_not_the_class_name() {
        let prefix = "format_version='2'\nox_version='>=0.7.0'\n";
        let inline = format!(
            "{prefix}[rule.a]\noutput=['out']\nshell='echo {{threads}} {{resources.mem}} > {{output}}'\nresources={{cpu=2,mem='4G'}}\n"
        );
        let named = format!(
            "{prefix}[rule.a]\noutput=['out']\nshell='echo {{threads}} {{resources.mem}} > {{output}}'\nresource_class='standard'\n[resource_classes.standard]\ncpu=2\nmem='4G'\n"
        );
        let renamed = named.replace("standard", "renamed");
        assert_eq!(resolved_key(&inline), resolved_key(&named));
        assert_eq!(resolved_key(&named), resolved_key(&renamed));
    }

    #[test]
    fn schema1_cpus_preserves_literal_threads_and_cache_key() {
        let source = "format_version='1'
[rule.a]
output=['out']
shell='echo t={threads}'
resources={cpus=7}
";
        let wf = ox_format::parse::parse_workflow(source, Path::new("legacy.toml")).unwrap();
        let job = resolve(
            &wf.rules,
            &ResolveRequest {
                targets: vec!["out".into()],
                config: Config::default(),
                existing_files: vec![],
            },
        )
        .unwrap()
        .jobs
        .remove(0);
        assert!(
            matches!(&job.execution, ox_core::model::ExecutionBlock::Shell {command} if command == "echo t={threads}")
        );
        assert_eq!(
            resolved_key(source),
            resolved_key(&source.replace("resources={cpus=7}", ""))
        );
    }

    #[test]
    fn class_alias_override_interpolates_inherited_spelling() {
        let source = "format_version='2'
ox_version='>=0.7.0'
[resource_classes.standard]
cpu=2
mem='4G'
[rule.a]
output=['out']
resource_class='standard'
resources={cpus=6}
shell='echo t={threads} c={resources.cpu} m={resources.mem}'
";
        let expected = "format_version='2'
ox_version='>=0.7.0'
[rule.a]
output=['out']
resources={cpu=6,mem='4G'}
shell='echo t=6 c=6 m=4G'
";
        assert_eq!(resolved_key(source), resolved_key(expected));
    }

    #[test]
    fn resource_class_cache_identity_survives_include_move_order_and_unused_values() {
        let prefix = "format_version='2'
ox_version='>=0.7.0'
";
        let class = "[resource_classes.standard]
cpu=2
mem='4G'
";
        let rule = "[rule.a]
output=['out']
shell='echo {threads} {resources.mem}'
resource_class='standard'
";
        let original = format!("{prefix}{class}{rule}");
        assert_eq!(
            resolved_key(&original),
            resolved_key(&format!("{prefix}{rule}{class}"))
        );
        let other = "[resource_classes.other]\ncpu=9\n";
        assert_eq!(
            resolved_key(&format!("{prefix}{class}{other}{rule}")),
            resolved_key(&format!("{prefix}{other}{class}{rule}"))
        );
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("classes.toml");
        std::fs::write(&child, format!("{prefix}{class}")).unwrap();
        let included = format!(
            "{prefix}include=[{:?}]
{rule}",
            child.to_str().unwrap()
        );
        assert_eq!(resolved_key(&original), resolved_key(&included));
        assert_ne!(
            resolved_key(&original),
            resolved_key(&original.replace("cpu=2", "cpu=3"))
        );
        let unused = original.replace("echo {threads} {resources.mem}", "echo constant");
        assert_eq!(
            resolved_key(&unused),
            resolved_key(&unused.replace("cpu=2", "cpu=3"))
        );
    }
}
