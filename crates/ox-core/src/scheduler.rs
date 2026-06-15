//! # Topological Scheduler (Petri Net Executor)
//!
//! The scheduler is the core runtime loop of OxyMake. It takes an optimized
//! `JobGraph` and drives it to completion by:
//!
//! 1. Finding jobs whose upstream dependencies are all satisfied (succeeded or skipped).
//! 2. Dispatching ready jobs to an `Executor` with bounded parallelism via a semaphore.
//! 3. Handling completions — updating state, emitting events, and propagating failures
//!    according to each job's `ErrorStrategy`.
//! 4. Repeating until every job is succeeded, failed, skipped, or cancelled.
//!
//! ## Petri net interpretation
//!
//! The scheduler loop maps precisely to **Colored Petri Net** (CPN) semantics:
//!
//! | OxyMake concept | Petri net concept |
//! |-----------------|-------------------|
//! | `OutputRef` node | **Place** |
//! | `ConcreteJob` node | **Transition** |
//! | "Data exists" flag | **Token** |
//! | Job execution | **Transition firing** |
//! | `ready_frontier()` | **Enabled transitions** |
//! | `MaterializationSet` | **Colored tokens** (location = color) |
//! | Firing-mode selection | **Cheapest color** (memory > object store > disk) |
//! | Eviction guard | **Inhibitor arc** (do not remove last token if consumers pending) |
//!
//! The `JobGraph` is a bipartite directed graph — the standard **Levi expansion**
//! of the underlying directed hypergraph. Each `ConcreteJob` is a hyperarc:
//! `{inputs} → job → {outputs}`. The bipartite expansion preserves all structural
//! information at equivalent algorithmic complexity.
//!
//! Multi-materialization (Stage 2+) extends this to CPN semantics: tokens carry
//! a color encoding their physical location and access cost. When a downstream
//! transition fires, the scheduler selects the cheapest available color via
//! `MaterializationSet::cheapest`. Eviction safety maps to inhibitor arcs:
//! the `MaterializationSet::try_remove` method refuses to remove the last
//! token if pending transitions (consumers) still need to read from this place.
//!
//! ## Concurrency model
//!
//! The scheduler uses a `tokio::sync::Semaphore` to bound the number of concurrently
//! executing jobs to `SchedulerConfig::max_jobs`. Each dispatched job acquires a
//! permit before calling `Executor::execute`, and releases it on completion.
//!
//! ## Error strategies
//!
//! - **Terminate** (default): On failure, cancel all downstream jobs and stop
//!   dispatching new work. Already-running jobs are allowed to finish.
//! - **Ignore**: Treat the failure as success and continue.
//! - **Retry**: Re-queue the job as Pending if attempts remain, with configurable
//!   backoff (Constant, Linear, Exponential). Exhausted retries trigger terminal
//!   failure and downstream cancellation.
//! - **Finish**: Let currently running jobs complete, but do not start new ones.
//!
//! ## Example
//!
//! ```rust,no_run
//! use ox_core::scheduler::{SchedulerConfig, run_scheduler};
//! use ox_core::job_graph::JobGraph;
//! use ox_core::event::EventBus;
//! use ox_core::traits::executor::ExecContext;
//! use std::collections::HashMap;
//! use std::path::PathBuf;
//! use std::sync::Arc;
//!
//! # async fn example(graph: JobGraph, executor: impl ox_core::traits::executor::Executor + 'static) {
//! let config = SchedulerConfig::default();
//! let bus = EventBus::new();
//! let ctx = ExecContext {
//!     global_job_limit: config.max_jobs,
//!     run_id: "run-001".into(),
//!     log_dir: PathBuf::from("/tmp/logs"),
//!     project_dir: PathBuf::from("/tmp/project"),
//!     trusted_dirs: vec![],
//!     input_data: HashMap::new(),
//!     memory_map: None,
//! };
//! let result = run_scheduler(&graph, Arc::new(executor), &config, &bus, &ctx).await;
//! # }
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Notify, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::disk_writer::{DiskWriteRequest, DiskWriterHandle};
use crate::error::{ExecError, OxError};
use crate::event::EventBus;
use crate::job_graph::{JobGraph, output_ref_key};
use crate::model::*;
use crate::traits::benchmark::BenchmarkSink;
use crate::traits::cache::CacheCheck;
use crate::traits::executor::{ExecContext, Executor, JobResult, JobStatus};
use crate::traits::gate::{GateCheck, GateStatus};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How long the scheduler sleeps between gate-status polls when no jobs are
/// in-flight. Keeps CPU usage near zero while gates are pending, with at most
/// 500 ms latency after a gate is approved.
const GATE_POLL_INTERVAL: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the scheduler.
///
/// Controls concurrency limits, resource budgets, and error-handling behavior.
///
/// ```
/// use ox_core::scheduler::SchedulerConfig;
///
/// let config = SchedulerConfig::default();
/// assert_eq!(config.max_jobs, 1);
/// assert!(config.resource_budget.is_empty());
/// assert!(!config.keep_going);
/// assert_eq!(config.root_cause_threshold, 3);
/// ```
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Maximum concurrent jobs.
    pub max_jobs: usize,
    /// Resource budget (e.g., {"cpu": 16, "mem_mb": 32000}).
    /// Not enforced in v0.1 — concurrency is bounded only by `max_jobs`.
    pub resource_budget: BTreeMap<String, u64>,
    /// Whether to continue on independent branches after failure.
    pub keep_going: bool,
    /// Jobs to skip (e.g., because they are cached). These will be
    /// pre-marked as `Skipped` before the scheduling loop starts.
    pub skip_jobs: HashSet<JobId>,
    /// Jobs to force re-execute regardless of cache. These bypass the
    /// dynamic cache check in the scheduler, ensuring they always run
    /// even when the cache reports a hit (used by `--forcerun`).
    pub force_rerun: HashSet<JobId>,
    /// Why each non-skipped job needs to execute. Populated during the
    /// cache pre-scan in `run.rs` and propagated to `Event::JobStarted`.
    pub run_reasons: HashMap<JobId, RunReason>,
    /// Number of consecutive failures with the same stderr signature
    /// required to trigger root-cause detection and early abort.
    /// Set to 0 to disable. Default: 3.
    pub root_cause_threshold: usize,
    /// Maximum bytes of in-memory materializations before eviction triggers.
    /// When the total exceeds this budget, the scheduler evicts the largest
    /// evictable outputs (Belady optimal: `pending_consumers == 0`) until
    /// usage drops below the budget.
    ///
    /// `0` means unlimited (no eviction). Default: `0`.
    pub memory_budget_bytes: u64,
    /// Set of job IDs that lie on the critical path. Only these jobs'
    /// outputs are eligible for in-memory materialization. Populated by
    /// `CriticalPathPass` in `ox-plan`.
    ///
    /// When empty, all outputs are eligible (no gating).
    pub critical_path_jobs: HashSet<JobId>,
    /// Optional Ledger snapshot for **OX-7 resume** (re-derivability).
    ///
    /// When present, [`run_scheduler_with_cache`] constructs the initial
    /// `Frontier` via `Frontier::resume` instead of
    /// `Frontier::new`. Jobs marked terminal in the snapshot keep
    /// their recorded status and are not re-dispatched; the scheduler
    /// resumes from the implied decision point.
    ///
    /// When `None`, the scheduler starts a fresh run (status quo).
    pub resume_snapshot: Option<LedgerSnapshot>,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_jobs: 1,
            resource_budget: BTreeMap::new(),
            keep_going: false,
            skip_jobs: HashSet::new(),
            force_rerun: HashSet::new(),
            run_reasons: HashMap::new(),
            root_cause_threshold: 3,
            memory_budget_bytes: 0,
            critical_path_jobs: HashSet::new(),
            resume_snapshot: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

/// Result of a complete scheduler run.
///
/// ```
/// use ox_core::scheduler::SchedulerResult;
/// use std::time::Duration;
///
/// let result = SchedulerResult {
///     total_jobs: 10,
///     succeeded: 8,
///     failed: 1,
///     skipped: 1,
///     cancelled: 0,
///     duration: Duration::from_secs(42),
///     failed_details: vec![],
///     root_cause: None,
///     memory_stats: None,
/// };
/// assert_eq!(result.total_jobs, result.succeeded + result.failed + result.skipped + result.cancelled);
/// ```
#[derive(Debug, Clone)]
pub struct SchedulerResult {
    /// Total jobs in the graph.
    pub total_jobs: usize,
    /// Jobs that completed successfully.
    pub succeeded: usize,
    /// Jobs that failed.
    pub failed: usize,
    /// Jobs that were skipped (cached or guard-excluded).
    pub skipped: usize,
    /// Jobs cancelled due to upstream failure.
    pub cancelled: usize,
    /// Wall-clock duration of the entire run.
    pub duration: Duration,
    /// Details for the first failed jobs (job name + last stderr line).
    pub failed_details: Vec<FailedJobDetail>,
    /// If root-cause detection triggered, the shared error and matching job IDs.
    pub root_cause: Option<RootCause>,
    /// Stage 2 memory stats (only meaningful when `memory_budget_bytes > 0`).
    pub memory_stats: Option<MemoryStats>,
}

/// Stage 2 in-memory materialization statistics.
#[derive(Debug, Clone, Default)]
pub struct MemoryStats {
    /// Peak bytes held in memory during the run.
    pub peak_memory_bytes: u64,
    /// Configured memory budget (0 = unlimited).
    pub memory_budget_bytes: u64,
    /// Number of outputs evicted by `enforce_memory_budget`.
    pub eviction_count: usize,
    /// Total bytes reclaimed by eviction.
    pub eviction_bytes: u64,
}

/// Root-cause detection result: a common error across multiple failures.
#[derive(Debug, Clone)]
pub struct RootCause {
    /// The shared last stderr line.
    pub error_line: String,
    /// Job IDs that exhibited this root cause.
    pub job_ids: Vec<JobId>,
}

/// Summary of a single failed job for the completion report.
#[derive(Debug, Clone)]
pub struct FailedJobDetail {
    /// The job identifier.
    pub job_id: JobId,
    /// Last non-empty line of stderr (if available).
    pub last_stderr_line: Option<String>,
}

// ---------------------------------------------------------------------------
// Schedule status
// ---------------------------------------------------------------------------

/// Status of a job during scheduling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobLifecycle {
    /// Waiting for upstream dependencies.
    Pending,
    /// All dependencies met; eligible for dispatch.
    Ready,
    /// Currently executing.
    Running,
    /// Completed with exit code 0.
    Succeeded,
    /// Failed with an error message.
    Failed(String),
    /// Skipped (cached, guard-excluded, or pre-marked).
    Skipped,
    /// Cancelled because an upstream job failed.
    Cancelled,
}

impl JobLifecycle {
    /// Returns true if the job has reached a terminal state.
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobLifecycle::Succeeded
                | JobLifecycle::Failed(_)
                | JobLifecycle::Skipped
                | JobLifecycle::Cancelled
        )
    }
}

impl From<JobStatus> for JobLifecycle {
    fn from(status: JobStatus) -> Self {
        match status {
            JobStatus::Queued => JobLifecycle::Pending,
            JobStatus::Running => JobLifecycle::Running,
            JobStatus::Completed => JobLifecycle::Succeeded,
            JobStatus::Failed(msg) => JobLifecycle::Failed(msg),
            JobStatus::Cancelled => JobLifecycle::Cancelled,
        }
    }
}

// ---------------------------------------------------------------------------
// Ledger snapshot — input to Frontier::resume (OX-7)
// ---------------------------------------------------------------------------

/// Frozen view of job statuses recovered from the on-disk Ledger.
///
/// `LedgerSnapshot` is the bridge that lets `Frontier::resume` re-derive
/// the in-memory Frontier from `.oxymake/state.db` alone, without retaining any
/// hidden state that would be lost on process death. It is the load-bearing
/// data structure behind invariant **OX-7 (Re-derivability)**: the Frontier
/// `(ready_frontier, pending_upstream, statuses)` is a pure function of the
/// graph plus the terminal-status set recorded in this snapshot.
///
/// Only **terminal** statuses (`Succeeded`, `Skipped`, `Failed`, `Cancelled`)
/// should be populated here. Non-terminal statuses (`Pending`, `Ready`,
/// `Running`) are intentionally omitted — the in-memory Running state is by
/// definition transient and reverts to `Pending` on restart so the scheduler
/// can re-dispatch the job.
///
/// Implementations in `ox-state` populate this from the `jobs` table; downstream
/// consumers in `ox-core` accept it through `Frontier::resume` without
/// depending on the persistence layer.
#[derive(Debug, Clone, Default)]
pub struct LedgerSnapshot {
    /// Terminal statuses keyed by job ID. Jobs absent from this map are
    /// treated as `Pending` (never started, or started but not yet recorded).
    pub statuses: BTreeMap<JobId, JobLifecycle>,
}

impl LedgerSnapshot {
    /// Construct an empty snapshot — equivalent to a fresh run with no prior
    /// progress recorded. `resume` from an empty snapshot is observationally
    /// indistinguishable from a fresh `Frontier::new` (private constructor in
    /// this crate).
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a job's terminal status. Non-terminal statuses are silently
    /// rejected — only terminal entries belong in a Ledger snapshot, since
    /// `Running` state is not durable across process death (OX-7).
    pub fn insert(&mut self, job_id: JobId, status: JobLifecycle) {
        if status.is_terminal() {
            self.statuses.insert(job_id, status);
        }
    }

    /// Number of terminal-status entries in the snapshot.
    pub fn len(&self) -> usize {
        self.statuses.len()
    }

    /// True iff the snapshot has no terminal entries.
    pub fn is_empty(&self) -> bool {
        self.statuses.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// Run the scheduler on a `JobGraph`.
///
/// This is the core execution loop. It processes jobs in topological order,
/// dispatching ready jobs to the executor with bounded parallelism, and
/// handling errors according to each job's `ErrorStrategy`.
///
/// # Errors
///
/// Returns `OxError::Exec` if a job fails with `ErrorStrategy::Terminate`
/// (the default), after allowing in-flight jobs to complete.
pub async fn run_scheduler<E: Executor + 'static>(
    graph: &JobGraph,
    executor: Arc<E>,
    config: &SchedulerConfig,
    event_bus: &EventBus,
    ctx: &ExecContext,
) -> Result<SchedulerResult, OxError> {
    run_scheduler_with_cache(
        graph, executor, config, event_bus, ctx, None, None, None, None, None,
    )
    .await
}

/// Run the scheduler with an optional cache checker and gate checker.
///
/// When a `cache` is provided, the scheduler checks each job dynamically
/// as it becomes ready (all upstream dependencies satisfied, so input files
/// exist on disk). Cache hits are marked as [`JobLifecycle::Skipped`] and
/// never dispatched to the executor. Successful completions are recorded in
/// the cache so subsequent runs can skip them.
///
/// When a `gate_checker` is provided, the scheduler checks any gates that
/// block a job before dispatching it. If a gate is `Pending`, the job
/// remains in `Pending` state until the gate is approved via `ox gate approve`.
/// If a gate is `Rejected`, downstream jobs are cancelled.
///
/// When a `shutdown` signal is provided, the scheduler initiates graceful
/// shutdown when notified: it sends cancel (SIGTERM) to all in-flight jobs,
/// stops dispatching new work, and waits for running jobs to exit.  The
/// executor's `finalize_workspace` then cleans up partial outputs.
///
/// This is strictly more powerful than the static `skip_jobs` set in
/// `SchedulerConfig`, because it can check intermediate jobs whose inputs
/// are produced by upstream jobs within the same run.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    name = "scheduler.run",
    skip_all,
    fields(total_jobs = graph.job_count(), max_jobs = config.max_jobs),
)]
pub async fn run_scheduler_with_cache<E: Executor + 'static>(
    graph: &JobGraph,
    executor: Arc<E>,
    config: &SchedulerConfig,
    event_bus: &EventBus,
    ctx: &ExecContext,
    cache: Option<Arc<dyn CacheCheck>>,
    gate_checker: Option<Arc<dyn GateCheck>>,
    benchmark_sink: Option<Arc<dyn BenchmarkSink>>,
    shutdown: Option<Arc<Notify>>,
    disk_writer: Option<DiskWriterHandle>,
) -> Result<SchedulerResult, OxError> {
    let start = Instant::now();

    let total_jobs = graph.job_count();
    info!(target: "ox.scheduler", total_jobs, "scheduler.start");
    if total_jobs == 0 {
        event_bus.emit(Event::RunStarted {
            total_jobs: 0,
            to_run: 0,
            cached: 0,
        });
        event_bus.emit(Event::RunCompleted {
            total: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            cancelled: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        });
        return Ok(SchedulerResult {
            total_jobs: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            cancelled: 0,
            duration: start.elapsed(),
            failed_details: vec![],
            root_cause: None,
            memory_stats: None,
        });
    }

    // Initialize status for every job. Jobs that no longer appear in the
    // graph (removed by mark_skipped) won't have entries here — that's fine,
    // they're already gone from job_ids().
    let topo_order = graph.topological_order().map_err(|e| {
        OxError::Exec(ExecError::Executor {
            message: format!("topological sort failed: {e}"),
        })
    })?;

    let resource_budget = ResourceBudget::new(config.resource_budget.clone());
    let mut sched_state = match config.resume_snapshot.as_ref() {
        Some(snapshot) => Frontier::resume(
            &topo_order,
            graph,
            snapshot,
            config.memory_budget_bytes,
            disk_writer,
            resource_budget,
        ),
        None => Frontier::new(
            &topo_order,
            graph,
            config.memory_budget_bytes,
            disk_writer,
            resource_budget,
        ),
    };

    // Seed force_rerun from config (--forcerun CLI flag). These jobs bypass
    // the dynamic cache check even if their inputs haven't changed.
    for job_id in &config.force_rerun {
        sched_state.force_rerun.insert(job_id.clone());
    }

    // Pre-mark cached / skip-listed jobs as Skipped before the loop starts.
    // Promote their downstream dependents into the ready frontier.
    let mut skipped_pre = 0usize;
    let mut skipped_pre_ids = Vec::new();
    for job_id in &config.skip_jobs {
        if sched_state.statuses.contains_key(job_id) {
            sched_state.set_status(job_id.clone(), JobLifecycle::Skipped);
            sched_state.ready_frontier.remove(job_id);
            sched_state.promote_downstream(job_id, graph);
            skipped_pre += 1;
            skipped_pre_ids.push(job_id.clone());
        }
    }

    let state = Arc::new(Mutex::new(sched_state));
    let semaphore = Arc::new(Semaphore::new(config.max_jobs));

    event_bus.emit(Event::RunStarted {
        total_jobs,
        to_run: topo_order.len() - skipped_pre,
        cached: skipped_pre,
    });

    // Emit JobSkipped events for pre-marked cached jobs so reporters
    // can display them as instant completions in the progress bar.
    for job_id in skipped_pre_ids {
        event_bus.emit(Event::JobSkipped {
            job_id,
            reason: "cached".into(),
        });
    }

    // The main scheduling loop. Jobs are spawned into a JoinSet for true
    // parallel execution, bounded by the semaphore.
    let mut join_set: JoinSet<Result<CompletionMsg, OxError>> = JoinSet::new();
    let mut in_flight = HashSet::<JobId>::new();
    let mut terminate_requested = false;

    loop {
        // 1. Find newly ready jobs and dispatch them (unless terminating).
        let mut skipped_this_iter = false;
        if !terminate_requested {
            let ready = find_ready_jobs(&state, graph, &gate_checker, event_bus).await;
            for (idx, job_id) in ready.iter().enumerate() {
                let job = match graph.get_job(job_id) {
                    Some(j) => j.clone(),
                    None => {
                        continue;
                    }
                };

                // Dynamic cache check: now that all upstream deps are done,
                // input files exist on disk and we can compute a cache key.
                // Performed *outside* the state lock so we never hold the
                // mutex across the cache I/O. The force/skip resolution
                // happens inside the consolidated dispatch lock below.
                let cached = if let Some(ref cache) = cache {
                    // H7: the async disk writer may still be flushing this
                    // job's input files (outputs of upstream jobs). Hashing
                    // them mid-write yields a torn hash and a
                    // non-deterministic cache decision — wait for the
                    // writer's ack on those paths first.
                    let writer = { state.lock().await.disk_writer.clone() };
                    if let Some(writer) = writer {
                        let input_paths: Vec<std::path::PathBuf> = job
                            .inputs
                            .iter()
                            .filter_map(|i| match &i.reference {
                                OutputRef::File(p) => Some(p.clone()),
                                _ => None,
                            })
                            .collect();
                        if !input_paths.is_empty() {
                            writer.wait_for_paths(&input_paths).await;
                        }
                    }
                    cache.is_cached(&job).await
                } else {
                    false
                };

                // Consolidated dispatch lock: force-rerun read, cache-skip
                // resolution, budget check, semaphore acquire, resource
                // accounting, status transition, and input data collection
                // in a SINGLE lock acquisition. (Previously 5 acquisitions
                // — R5 bottleneck identified in next-steps-2026-04-06.md
                // Priority 1.)
                enum DispatchOutcome {
                    Stale,         // no longer Ready (cancelled since find_ready_jobs)
                    Deferred,      // budget doesn't fit, try next job
                    SemaphoreFull, // no permits, return remaining to frontier
                    Skipped,       // cache hit and not force-rerun
                    Go {
                        permit: tokio::sync::OwnedSemaphorePermit,
                        resource_guard: ResourceGuard,
                        input_data: HashMap<String, Arc<[u8]>>,
                        force: bool, // determines RunReason
                    },
                }
                let outcome = {
                    let mut s = state.lock().await;
                    let force = s.force_rerun.contains(job_id);

                    if !matches!(s.get_status(job_id), Some(JobLifecycle::Ready)) {
                        // H6 defense in depth: the job was cancelled (or
                        // otherwise moved off Ready) between find_ready_jobs
                        // and this dispatch lock — do not dispatch it.
                        DispatchOutcome::Stale
                    } else if cached && !force {
                        // 0. Cache hit (and no transitive force-rerun):
                        //    mark Skipped and propagate readiness downstream.
                        s.set_status(job_id.clone(), JobLifecycle::Skipped);
                        s.promote_downstream(job_id, graph);
                        DispatchOutcome::Skipped
                    } else if !s.resource_budget.fits(&job.resources) {
                        // 1. Resource budget gate — defer this job if it
                        //    doesn't fit; smaller jobs may still proceed.
                        s.ready_frontier.insert(job_id.clone());
                        s.set_status(job_id.clone(), JobLifecycle::Pending);
                        DispatchOutcome::Deferred
                    } else {
                        // 2. Semaphore gate — stop dispatching entirely
                        //    when max_jobs is saturated.
                        match semaphore.clone().try_acquire_owned() {
                            Err(_) => {
                                for remaining_id in &ready[idx..] {
                                    s.ready_frontier.insert(remaining_id.clone());
                                    s.set_status(remaining_id.clone(), JobLifecycle::Pending);
                                }
                                DispatchOutcome::SemaphoreFull
                            }
                            Ok(permit) => {
                                // 3. Commit: acquire resources, mark Running,
                                //    and collect in-memory inputs — all atomic
                                //    under the same lock. The resource guard
                                //    travels with the task and releases on
                                //    Drop, even if the task is aborted (H8).
                                let resource_guard = s.resource_budget.acquire(&job.resources);
                                s.set_status(job_id.clone(), JobLifecycle::Running);
                                let input_data = s.collect_input_data(&job);
                                DispatchOutcome::Go {
                                    permit,
                                    resource_guard,
                                    input_data,
                                    force,
                                }
                            }
                        }
                    }
                };

                let (permit, resource_guard, input_data, force) = match outcome {
                    DispatchOutcome::Stale => continue,
                    DispatchOutcome::Skipped => {
                        event_bus.emit(Event::JobSkipped {
                            job_id: job_id.clone(),
                            reason: "cached".into(),
                        });
                        debug!(
                            target: "ox.scheduler",
                            counter = "scheduler.job.skipped_cache_hit",
                            job_id = %job_id,
                            "cache_hit"
                        );
                        skipped_this_iter = true;
                        continue;
                    }
                    DispatchOutcome::Deferred => continue,
                    DispatchOutcome::SemaphoreFull => break,
                    DispatchOutcome::Go {
                        permit,
                        resource_guard,
                        input_data,
                        force,
                    } => (permit, resource_guard, input_data, force),
                };

                // Determine the run reason: if this job was force-rerun
                // due to an upstream rebuild, that takes priority.
                let reason = if force {
                    Some(RunReason::UpstreamRebuilt)
                } else {
                    config.run_reasons.get(job_id).cloned()
                };

                event_bus.emit(Event::JobStarted {
                    job_id: job_id.clone(),
                    executor: job.executor.clone().unwrap_or_else(|| "local".into()),
                    reason,
                });
                debug!(
                    target: "ox.scheduler",
                    counter = "scheduler.job.dispatched",
                    job_id = %job_id,
                    "dispatch"
                );

                // Spawn execution task with the pre-acquired permit.
                in_flight.insert(job_id.clone());
                let exec = executor.clone();
                let mut ctx_clone = ctx.clone();
                ctx_clone.input_data = input_data;
                let job_clone = job.clone();
                let jid = job_id.clone();

                join_set.spawn(async move {
                    let workspace = exec
                        .prepare_workspace(&job_clone, &ctx_clone)
                        .await
                        .map_err(|e| {
                            OxError::Exec(ExecError::Executor {
                                message: format!("prepare_workspace failed: {e}"),
                            })
                        })?;

                    let result = exec
                        .execute(&job_clone, &workspace, &ctx_clone)
                        .await
                        .map_err(|e| {
                            OxError::Exec(ExecError::Executor {
                                message: format!("execute failed: {e}"),
                            })
                        })?;

                    // Finalize the workspace (atomic output commit).
                    // On failure for a successful job, override the exit code
                    // so the scheduler treats the job as failed (prevents
                    // caching corrupt/partial outputs).
                    let result = match exec.finalize_workspace(workspace, &result).await {
                        Ok(()) => result,
                        Err(e) if result.exit_code == 0 => JobResult {
                            exit_code: 1,
                            stderr_tail: Some(format!("finalize_workspace: {e}")),
                            ..result
                        },
                        Err(_) => result, // Already failed — keep original.
                    };

                    // Stage 2: Populate the memory map for outputs that have
                    // no disk fallback (Never-policy). For outputs that ARE on
                    // disk (Auto/Always/Final), the page cache already serves as
                    // a transparent memory cache — reading into process memory
                    // would be a redundant memcpy that prepare_workspace would
                    // then write back to the same location.
                    //
                    // Key insight (Feynman/Torvalds analysis): for subprocess-
                    // based execution, the OS page cache IS the in-memory cache.
                    // Application-level caching only helps for in-process (call-
                    // mode) consumers that can use Arc<[u8]> directly.
                    if result.exit_code == 0 {
                        if let Some(ref mem_map) = ctx_clone.memory_map {
                            for output in &job_clone.outputs {
                                // Only read Never-policy outputs into memory —
                                // they have no disk representation and downstream
                                // jobs need the data from the memory map.
                                if matches!(output.materialize, MaterializePolicy::Never) {
                                    let key = output_ref_key(&output.reference);
                                    if let OutputRef::File(p) = &output.reference {
                                        if let Ok(data) = tokio::fs::read(p).await {
                                            mem_map.put(key, Arc::from(data));
                                        }
                                    }
                                }
                            }
                        }
                    }

                    drop(permit);
                    drop(resource_guard);

                    Ok(CompletionMsg {
                        job_id: jid,
                        job: job_clone,
                        result,
                    })
                });
            }
        }

        // 2. Check if we're done.
        {
            let s = state.lock().await;
            let all_terminal = s.statuses.values().all(|st| st.is_terminal());
            if all_terminal {
                break;
            }

            // If terminating and nothing in flight, cancel remaining pending jobs.
            if terminate_requested && in_flight.is_empty() {
                drop(s);
                cancel_remaining(&state, graph, event_bus).await;
                break;
            }

            // Safety: if no jobs are ready and none are in flight, we're stuck.
            // This shouldn't happen with a valid DAG, but guard against it.
            let has_pending = s
                .statuses
                .values()
                .any(|st| matches!(st, JobLifecycle::Pending | JobLifecycle::Ready));
            if !has_pending && in_flight.is_empty() {
                break;
            }
        }

        // 3. Wait for at least one completion if jobs are in flight,
        //    while also monitoring for a shutdown signal (SIGINT).
        if !in_flight.is_empty() {
            // Build a future that resolves when the shutdown signal fires.
            // If no shutdown signal was provided, this future never resolves.
            let shutdown_fut = async {
                match &shutdown {
                    Some(notify) => notify.notified().await,
                    None => std::future::pending().await,
                }
            };

            tokio::select! {
                biased;

                // Shutdown signal received — graceful termination.
                _ = shutdown_fut, if !terminate_requested => {
                    terminate_requested = true;
                    // Send SIGTERM to all in-flight children.  They will
                    // exit non-zero, and finalize_workspace (in the spawned
                    // tasks) will clean up partial outputs.
                    for job_id in &in_flight {
                        let _ = executor.cancel(job_id).await;
                    }
                    // Don't break — continue the loop to collect completions
                    // from the join_set so finalize_workspace can run.
                }

                // Normal completion path.
                join_result = join_set.join_next() => {
                    match join_result {
                        Some(Ok(Ok(msg))) => {
                            in_flight.remove(&msg.job_id);

                            let committed = handle_completion(
                                &msg,
                                &state,
                                graph,
                                config,
                                event_bus,
                                ctx,
                                &mut terminate_requested,
                                executor.as_ref(),
                            )
                            .await;

                            // Record successful jobs in the cache and write benchmarks.
                            // `committed` guards the B4 race: a job cancelled while
                            // running can still exit 0, but its result must not be
                            // recorded (it is derived from a failed upstream).
                            if committed && msg.result.exit_code == 0 {
                                if let Some(ref cache) = cache {
                                    cache.record(&msg.job).await;
                                }
                                if let Some(ref bench_path) = msg.job.benchmark {
                                    if let Some(ref sink) = benchmark_sink {
                                        sink.write_benchmark(std::path::Path::new(bench_path), &msg.result)
                                            .await;
                                    }
                                }
                            }
                        }
                        Some(Ok(Err(ox_err))) => {
                            // Fatal executor error — cancel all in-flight jobs via the
                            // executor before aborting tokio tasks. Without this,
                            // external processes (child PIDs, SLURM jobs) leak.
                            for job_id in &in_flight {
                                let _ = executor.cancel(job_id).await;
                            }
                            join_set.abort_all();
                            return Err(ox_err);
                        }
                        Some(Err(join_err)) => {
                            // Task panicked — cancel remaining in-flight jobs.
                            for job_id in &in_flight {
                                let _ = executor.cancel(job_id).await;
                            }
                            join_set.abort_all();
                            return Err(OxError::Exec(ExecError::Executor {
                                message: format!("spawned task panicked: {join_err}"),
                            }));
                        }
                        None => break,
                    }
                }
            }
        } else if skipped_this_iter {
            // Jobs were skipped (cached) this iteration, which promoted
            // downstream dependents into the ready frontier. Re-loop
            // immediately to process them instead of sleeping. This avoids
            // the 500ms-per-layer penalty that made fully-cached DAGs take
            // O(depth × 500ms) instead of O(depth × ~0ms).
            continue;
        } else {
            // No jobs in flight and nothing was skipped — sleep before
            // re-checking gate status. Using yield_now() here would
            // busy-poll at 100% CPU while gates are pending (ox-hm7).
            // A short sleep caps overhead while keeping gate-approval
            // latency under a second.
            tokio::time::sleep(GATE_POLL_INTERVAL).await;
        }
    }

    // Build final result.
    let s = state.lock().await;
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut cancelled = 0usize;

    for status in s.statuses.values() {
        match status {
            JobLifecycle::Succeeded => succeeded += 1,
            JobLifecycle::Failed(_) => failed += 1,
            JobLifecycle::Skipped => skipped += 1,
            JobLifecycle::Cancelled => cancelled += 1,
            _ => {}
        }
    }

    let duration = start.elapsed();

    event_bus.emit(Event::RunCompleted {
        total: total_jobs,
        succeeded,
        failed,
        skipped,
        cancelled,
        duration_ms: duration.as_millis() as u64,
    });

    let result = SchedulerResult {
        total_jobs,
        succeeded,
        failed,
        skipped,
        cancelled,
        duration,
        failed_details: s.failed_details.clone(),
        root_cause: s.root_cause.clone(),
        memory_stats: s.memory_stats(),
    };

    Ok(result)
}

// ---------------------------------------------------------------------------
// Resource budget
// ---------------------------------------------------------------------------

/// Tracks available and consumed resources for resource-aware scheduling.
///
/// The budget is initialized from `SchedulerConfig::resource_budget` which
/// declares the total capacity (e.g., `{"cpu": 8, "mem_mb": 32000}`). When a
/// job is dispatched, its [`ConcreteJob::resources`] are acquired (subtracted
/// from available). When the job completes, they are released (added back).
///
/// Jobs whose resource requirements exceed the remaining budget are held in
/// the ready frontier until enough resources are freed by completing jobs.
///
/// If the budget is empty (no resource constraints configured), all resource
/// checks are no-ops and dispatch is governed only by `max_jobs`.
///
/// The budget is a cheap-clone handle around shared state. Acquisition
/// returns a [`ResourceGuard`] that releases on `Drop` — the guard travels
/// with the executing task, so resources are returned even when the task is
/// aborted (`abort_all`) and never sends a completion message (H8).
#[derive(Debug, Clone)]
struct ResourceBudget {
    inner: Arc<std::sync::Mutex<ResourceBudgetInner>>,
}

#[derive(Debug)]
struct ResourceBudgetInner {
    /// Total capacity per resource key (from config).
    capacity: BTreeMap<String, u64>,
    /// Currently consumed per resource key (sum of running jobs' resources).
    in_use: BTreeMap<String, u64>,
}

impl ResourceBudget {
    fn new(capacity: BTreeMap<String, u64>) -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(ResourceBudgetInner {
                capacity,
                in_use: BTreeMap::new(),
            })),
        }
    }

    /// Check whether a job's resource requirements fit within the remaining
    /// budget. Returns `true` if the job can be dispatched.
    ///
    /// For each resource key declared by the job:
    /// - If the budget has a limit for that key, the job's requirement must fit
    ///   within `capacity - in_use`.
    /// - If the budget has no limit for that key, the resource is unconstrained.
    ///
    /// Resource values that cannot be converted to `u64` (unparseable strings)
    /// are treated as unconstrained (logged but not blocking).
    fn fits(&self, job_resources: &BTreeMap<String, ResourceValue>) -> bool {
        let inner = self.inner.lock().expect("resource budget lock poisoned");
        if inner.capacity.is_empty() {
            return true;
        }
        for (key, value) in job_resources {
            if let Some(&cap) = inner.capacity.get(key) {
                let required = match value.as_u64() {
                    Some(v) => v,
                    None => continue, // Unparseable → unconstrained.
                };
                let used = inner.in_use.get(key).copied().unwrap_or(0);
                if used.saturating_add(required) > cap {
                    return false;
                }
            }
        }
        true
    }

    /// Acquire resources for a dispatched job. The returned guard releases
    /// exactly what was acquired when dropped — tie its lifetime to the
    /// executing task so an abort cannot leak budget (H8).
    #[must_use]
    fn acquire(&self, job_resources: &BTreeMap<String, ResourceValue>) -> ResourceGuard {
        let mut acquired = Vec::new();
        {
            let mut inner = self.inner.lock().expect("resource budget lock poisoned");
            for (key, value) in job_resources {
                if inner.capacity.contains_key(key) {
                    if let Some(v) = value.as_u64() {
                        *inner.in_use.entry(key.clone()).or_insert(0) += v;
                        acquired.push((key.clone(), v));
                    }
                }
            }
        }
        ResourceGuard {
            budget: Arc::clone(&self.inner),
            acquired,
        }
    }
}

/// RAII guard for acquired budget resources. Releases on `Drop`, so the
/// budget is returned on every task exit path: normal completion, early
/// error return, panic, or `JoinSet::abort_all` (H8).
#[derive(Debug)]
struct ResourceGuard {
    budget: Arc<std::sync::Mutex<ResourceBudgetInner>>,
    /// Exactly what `acquire` added, as parsed (key, u64) pairs.
    acquired: Vec<(String, u64)>,
}

impl Drop for ResourceGuard {
    fn drop(&mut self) {
        if self.acquired.is_empty() {
            return;
        }
        // Don't panic in Drop on a poisoned lock — the process is already
        // unwinding in that case.
        if let Ok(mut inner) = self.budget.lock() {
            for (key, v) in &self.acquired {
                if let Some(used) = inner.in_use.get_mut(key) {
                    *used = used.saturating_sub(*v);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Internal state tracking for the scheduler.
///
/// In Petri net terms, this struct holds the **marking** (token distribution)
/// of the net. The `statuses` map tracks transition states, while `output_mats`
/// tracks the colored tokens at each place (output).
struct Frontier {
    statuses: BTreeMap<JobId, JobLifecycle>,
    /// Retry counts per job (how many times we've retried so far).
    retry_counts: BTreeMap<JobId, u32>,
    /// Collected failure details (job id + last stderr line) for the summary.
    failed_details: Vec<FailedJobDetail>,
    /// Recent failure stderr hashes (hash, job_id) in arrival order, for
    /// root-cause detection. When N consecutive entries share the same hash,
    /// the scheduler triggers a root-cause abort.
    recent_failure_hashes: Vec<(u64, JobId)>,
    /// Set once root-cause detection fires, to avoid re-triggering.
    root_cause: Option<RootCause>,
    /// Incrementally maintained set of jobs whose upstream dependencies are all
    /// satisfied (Succeeded, Skipped, or absent from the state map). Only jobs
    /// in this set are considered by `find_ready_jobs`, replacing the previous
    /// O(pending × deps) full rescan with O(completed_downstream) incremental
    /// updates.
    ready_frontier: HashSet<JobId>,
    /// Jobs that must bypass the dynamic cache check because an upstream job
    /// was re-executed in this run. This ensures transitive cache invalidation:
    /// if any ancestor ran, all descendants must also run (ox-jxdw).
    force_rerun: HashSet<JobId>,
    /// Per-output materialization tracking (Stage 2: in-memory data transport).
    ///
    /// Maps output keys (the string representation of an `OutputRef`) to their
    /// `MaterializationSet`. This is the runtime "colored token" state in the
    /// Petri net model — each entry tracks where the output's data physically
    /// lives and how many consumers still need to read it.
    ///
    /// Populated when a job completes successfully. The scheduler uses
    /// `MaterializationSet::cheapest` for firing-mode selection and
    /// `MaterializationSet::consumer_fired` to decrement reference counts
    /// for eviction eligibility.
    output_mats: HashMap<String, MaterializationSet>,
    /// Total bytes currently held in memory across all outputs.
    memory_used_bytes: u64,
    /// Peak bytes held in memory during the run (high-water mark).
    peak_memory_bytes: u64,
    /// Maximum bytes allowed in memory (0 = unlimited).
    memory_budget_bytes: u64,
    /// Number of outputs evicted by `enforce_memory_budget`.
    eviction_count: usize,
    /// Total bytes reclaimed by eviction.
    eviction_bytes: u64,
    /// Handle to the background async disk writer (Stage 2).
    ///
    /// When present, the scheduler can enqueue in-memory output data for
    /// background persistence to disk. This avoids blocking the critical
    /// path while still ensuring data is flushed for caching/reproducibility.
    disk_writer: Option<DiskWriterHandle>,
    /// Raw byte data for outputs that have an `InMemory` materialization.
    ///
    /// Maps output-ref keys to `Arc<[u8]>` so the data can be shared with
    /// downstream executor tasks via [`ExecContext::input_data`] without
    /// copying. Populated by [`register_materializations`] when an output
    /// is placed in memory; entries are removed when the in-memory
    /// materialization is evicted.
    memory_store: HashMap<String, Arc<[u8]>>,
    /// Resource budget tracker. Constrains dispatch so that the sum of
    /// running jobs' resource requirements never exceeds the configured
    /// capacity. Empty budget means no resource constraints.
    resource_budget: ResourceBudget,
    /// Number of unsatisfied upstream dependencies per job.
    ///
    /// Initialized from the DAG: each job starts with a count equal to
    /// its upstream dependency count. Decremented by `promote_downstream`
    /// when an upstream job completes. When the count reaches 0, the job
    /// is ready to execute and added to `ready_frontier`.
    ///
    /// This replaces the O(N) "scan all upstream" check with an O(1)
    /// counter decrement — critical for diamond topologies where a sink
    /// with 50K upstream dependencies would otherwise cause O(N²) work.
    pending_upstream: HashMap<JobId, usize>,
}

impl Frontier {
    fn new(
        job_ids: &[&JobId],
        graph: &JobGraph,
        memory_budget_bytes: u64,
        disk_writer: Option<DiskWriterHandle>,
        resource_budget: ResourceBudget,
    ) -> Self {
        let statuses: BTreeMap<JobId, JobLifecycle> = job_ids
            .iter()
            .map(|id| ((*id).clone(), JobLifecycle::Pending))
            .collect();

        // Initialize pending upstream counts and seed the ready frontier.
        let mut pending_upstream: HashMap<JobId, usize> = HashMap::new();
        let mut ready_frontier: HashSet<JobId> = HashSet::new();
        for id in job_ids {
            let count = graph.upstream(id).len();
            pending_upstream.insert((*id).clone(), count);
            if count == 0 {
                ready_frontier.insert((*id).clone());
            }
        }

        // Initialize materialization tracking for all outputs in the graph.
        // Each output's pending_consumers count is the number of downstream
        // jobs that consume it (in-degree of consumer edges in the bipartite
        // graph). This count decrements as consumers fire.
        let output_mats = graph.init_output_materializations();

        Self {
            statuses,
            retry_counts: BTreeMap::new(),
            failed_details: Vec::new(),
            recent_failure_hashes: Vec::new(),
            root_cause: None,
            ready_frontier,
            force_rerun: HashSet::new(),
            output_mats,
            memory_used_bytes: 0,
            peak_memory_bytes: 0,
            memory_budget_bytes,
            eviction_count: 0,
            eviction_bytes: 0,
            disk_writer,
            memory_store: HashMap::new(),
            resource_budget,
            pending_upstream,
        }
    }

    /// Reconstruct the in-memory Frontier from the on-disk Ledger.
    ///
    /// Invariant **OX-7 (Re-derivability)**: the triple
    /// `(statuses, ready_frontier, pending_upstream)` is a pure function of
    /// the graph plus the terminal-status set recorded in `snapshot`. After a
    /// process death, calling this on a freshly opened DB yields a state from
    /// which the scheduler can resume execution and produce a terminal-status
    /// set identical (modulo timing) to a clean single-process run.
    ///
    /// ## Semantics
    ///
    /// For each job in `job_ids`:
    /// - If the snapshot records a terminal status, the job adopts that
    ///   status. Running jobs are intentionally NOT carried over from a
    ///   prior process — they revert to `Pending` so the resumed scheduler
    ///   can re-dispatch them. This is the load-bearing simplification: the
    ///   Ledger only durably records terminal transitions, and any
    ///   `Running` row in the DB after a SIGKILL is treated as an aborted
    ///   attempt that must be retried.
    /// - `pending_upstream[j]` counts the upstream dependencies of `j`
    ///   whose snapshot status is NOT in `{Succeeded, Skipped}` (the
    ///   "satisfied" terminal states). Failed and Cancelled upstreams keep
    ///   their downstream blocked, mirroring the runtime invariant
    ///   enforced by [`Frontier::promote_downstream`].
    /// - `ready_frontier` contains every `Pending` job whose
    ///   `pending_upstream` count is zero — exactly the set
    ///   `Frontier::new` would have populated incrementally as
    ///   upstream jobs completed.
    ///
    /// ## Equivalence oracle
    ///
    /// Let `S₁` be the state produced by running the scheduler to a
    /// quiescent decision point. Let `S₂` be the state produced by
    /// constructing a `LedgerSnapshot` from `S₁.statuses` (terminal only)
    /// and calling `resume`. Then:
    /// ```text
    ///   S₁.pending_upstream == S₂.pending_upstream
    ///   S₁.ready_frontier   == S₂.ready_frontier
    ///   ∀ j with terminal status in S₁: S₁.statuses[j] == S₂.statuses[j]
    ///   ∀ j with non-terminal status in S₁: S₂.statuses[j] == Pending
    /// ```
    /// This equivalence is asserted by the `frontier_poison` test below
    /// and exercised end-to-end by the `crash_and_restart` integration
    /// test.
    fn resume(
        job_ids: &[&JobId],
        graph: &JobGraph,
        snapshot: &LedgerSnapshot,
        memory_budget_bytes: u64,
        disk_writer: Option<DiskWriterHandle>,
        resource_budget: ResourceBudget,
    ) -> Self {
        // Step 1: assign each job a status — snapshot wins for known terminal
        // jobs, otherwise Pending. Non-terminal entries in the snapshot are
        // ignored (Running cannot survive process death; see doc above).
        let statuses: BTreeMap<JobId, JobLifecycle> = job_ids
            .iter()
            .map(|id| {
                let status = match snapshot.statuses.get(*id) {
                    Some(s) if s.is_terminal() => s.clone(),
                    _ => JobLifecycle::Pending,
                };
                ((*id).clone(), status)
            })
            .collect();

        // Step 2: derive pending_upstream — count upstreams that are NOT in
        // a satisfied terminal state. This mirrors promote_downstream's
        // decrement-on-completion semantics, but re-derived from final
        // statuses rather than replayed event-by-event.
        let mut pending_upstream: HashMap<JobId, usize> = HashMap::new();
        let mut ready_frontier: HashSet<JobId> = HashSet::new();
        for id in job_ids {
            let count = graph
                .upstream(id)
                .iter()
                .filter(|up| {
                    !matches!(
                        statuses.get(*up),
                        Some(JobLifecycle::Succeeded) | Some(JobLifecycle::Skipped)
                    )
                })
                .count();
            pending_upstream.insert((*id).clone(), count);

            // Step 3: a job is in the ready frontier iff it is still Pending
            // and all its upstream dependencies are satisfied.
            if count == 0 && matches!(statuses.get(*id), Some(JobLifecycle::Pending)) {
                ready_frontier.insert((*id).clone());
            }
        }

        // Output materializations are not yet durable — they live entirely in
        // the executor's working tree on disk. A future Ledger evolution
        // (ADR-011 Stage 3) will rebuild output_mats from `output_hashes` rows;
        // for now we start with the same initialization as `new`, which is
        // correct because completed jobs' output data has already been
        // persisted to disk by the executor.
        let output_mats = graph.init_output_materializations();

        Self {
            statuses,
            retry_counts: BTreeMap::new(),
            failed_details: Vec::new(),
            recent_failure_hashes: Vec::new(),
            root_cause: None,
            ready_frontier,
            force_rerun: HashSet::new(),
            output_mats,
            memory_used_bytes: 0,
            peak_memory_bytes: 0,
            memory_budget_bytes,
            eviction_count: 0,
            eviction_bytes: 0,
            disk_writer,
            memory_store: HashMap::new(),
            resource_budget,
            pending_upstream,
        }
    }

    fn set_status(&mut self, id: JobId, status: JobLifecycle) {
        self.statuses.insert(id, status);
    }

    fn get_status(&self, id: &JobId) -> Option<&JobLifecycle> {
        self.statuses.get(id)
    }

    /// After a job reaches a "satisfied" terminal state (Succeeded or Skipped),
    /// check its downstream dependents and add any whose upstream dependencies
    /// are now all satisfied to the ready frontier. This replaces the O(pending
    /// × deps) full rescan with an O(downstream × upstream) incremental update.
    /// Promote downstream jobs that become ready after a job completes.
    ///
    /// Uses O(1) counter decrements instead of O(upstream_count) scans.
    /// This is critical for diamond topologies: a sink with 50K upstream
    /// dependencies previously caused O(N²) total work (scan all upstream
    /// on each completion). With counters, each completion is O(downstream_count).
    fn promote_downstream(&mut self, completed_id: &JobId, graph: &JobGraph) {
        for downstream_id in graph.downstream(completed_id) {
            // Only promote jobs that are still Pending.
            if !matches!(self.get_status(downstream_id), Some(JobLifecycle::Pending)) {
                continue;
            }

            // Decrement the pending upstream counter. When it reaches 0,
            // all dependencies are satisfied and the job is ready.
            if let Some(count) = self.pending_upstream.get_mut(downstream_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.ready_frontier.insert(downstream_id.clone());
                }
            }
        }
    }

    /// Register materializations for outputs produced by a completed job.
    ///
    /// This is the Petri net "token deposit" operation: when a transition
    /// (job) fires, it places tokens (materializations) on its output places
    /// (OutputRefs). The token color encodes the storage location (InMemory,
    /// OnDisk, ObjectStore), and the scheduler's firing-mode selection
    /// (`cheapest()`) picks the lowest-cost color for downstream transitions.
    ///
    /// ## Data flow (Stage 2)
    ///
    /// The executor's spawned task writes output data into `OutputMemoryMap`
    /// immediately after `finalize_workspace` (page-cache-hot read). This
    /// method drains that data into `memory_store` for downstream jobs,
    /// avoiding a redundant `std::fs::read` from the scheduler's lock-held
    /// path.
    ///
    /// ## In-memory eligibility
    ///
    /// An output is promoted to `InMemory` when:
    /// - The job is on the critical path AND a memory budget is active AND
    ///   the policy is not `Always` (which forces disk-only), OR
    /// - The policy is `Never` (memory-only, never persisted to disk).
    ///
    /// Returns a list of `DiskWriteRequest`s for outputs that should be
    /// persisted in the background (in-memory-primary outputs with policy
    /// != `Never`).
    fn register_materializations(
        &mut self,
        job: &ConcreteJob,
        on_critical_path: bool,
        memory_map: Option<&crate::memory_map::OutputMemoryMap>,
    ) -> Vec<DiskWriteRequest> {
        let mut disk_writes = Vec::new();

        for output in &job.outputs {
            let key = output_ref_key(&output.reference);
            if let Some(mat_set) = self.output_mats.get_mut(&key) {
                // Register disk materialization for file outputs — but NOT
                // for Never-policy outputs, which must stay memory-only.
                // Adding OnDisk to a Never output would cause cheapest()
                // to fall back to disk after eviction, reading a stale or
                // missing file.
                if let OutputRef::File(path) = &output.reference {
                    if !matches!(output.materialize, MaterializePolicy::Never) {
                        mat_set.add(Materialization::OnDisk {
                            path: path.clone(),
                            verified: false,
                        });
                    }

                    // Determine if this output should also live in memory.
                    //
                    // Two independent triggers (explicit parentheses):
                    // 1. Critical path + budget active + not Always → memory
                    // 2. Never policy → always memory (by definition: "never
                    //    persist to disk"). This bypasses budget gating because
                    //    Never outputs have no disk fallback. When budget=0
                    //    (disabled), enforce_memory_budget is a no-op, so
                    //    Never outputs accumulate without bound — this is
                    //    intentional (the user opted into memory-only storage).
                    let promote_to_memory = (on_critical_path
                        && self.memory_budget_bytes > 0
                        && !matches!(output.materialize, MaterializePolicy::Always))
                        || matches!(output.materialize, MaterializePolicy::Never);

                    if promote_to_memory {
                        // Drain data from OutputMemoryMap (populated by the
                        // executor's spawned task) instead of re-reading from
                        // disk. This avoids a redundant synchronous I/O call
                        // under the scheduler lock.
                        if let Some(mem_map) = memory_map {
                            if let Some(arc_data) = mem_map.remove(&key) {
                                // Zero-copy: OutputMemoryMap now uses Arc<[u8]>,
                                // same type as memory_store — no allocation needed.
                                let size = arc_data.len() as u64;

                                // Skip BLAKE3 hashing here — it is expensive on
                                // large outputs (2-3s for 6.5 GB) and unnecessary
                                // for the memory data flow. The eviction policy
                                // only needs size_bytes. The content hash for
                                // cache provenance is computed separately by
                                // job_cache_key_with_components at record time,
                                // just like the mtime-based cache validation
                                // avoids re-hashing on the disk path.
                                mat_set.set_size_bytes(size);

                                self.memory_store.insert(key.clone(), arc_data.clone());
                                mat_set.add(Materialization::InMemory { pinned: false });
                                self.add_memory_bytes(size);

                                // Enqueue background disk write if the policy
                                // allows persistence (everything except Never).
                                if self.disk_writer.is_some()
                                    && !matches!(output.materialize, MaterializePolicy::Never)
                                {
                                    disk_writes.push(DiskWriteRequest {
                                        job_id: job.id.clone(),
                                        target_path: path.clone(),
                                        data: arc_data,
                                    });
                                }
                                continue;
                            }
                        }

                        // Fallback: data not in OutputMemoryMap (e.g., no
                        // memory_map configured, or data wasn't populated).
                        //
                        // WARNING: This is a blocking std::fs::read under the
                        // scheduler mutex. It is only reached when no memory_map
                        // is available (legacy path — the CLI always creates a
                        // memory_map when memory_budget > 0). The file should be
                        // page-cache hot after finalize_workspace, making this
                        // effectively a memcpy. If this becomes a bottleneck,
                        // move to spawn_blocking or ensure memory_map is always
                        // configured when budget > 0.
                        debug_assert!(
                            memory_map.is_none(),
                            "blocking fs::read fallback reached with memory_map present — \
                             data should have been drained from OutputMemoryMap above"
                        );
                        if let Ok(data) = std::fs::read(path) {
                            let size = data.len() as u64;
                            mat_set.set_size_bytes(size);
                            self.memory_store.insert(key.clone(), Arc::from(data));
                            mat_set.add(Materialization::InMemory { pinned: false });
                            self.add_memory_bytes(size);
                        }
                        // If the file is not readable and no memory_map data
                        // exists, do NOT register a ghost InMemory materialization.
                        // A ghost (InMemory without data in memory_store) would
                        // cause collect_input_data to return nothing for this
                        // output, and the downstream job would fail with a
                        // missing input — especially bad for Never-policy outputs
                        // that have no disk fallback.
                    }
                } else {
                    // Non-file outputs (Virtual, InMemory): check OutputMemoryMap.
                    if let Some(mem_map) = memory_map {
                        if let Some(arc_data) = mem_map.remove(&key) {
                            let size = arc_data.len() as u64;
                            mat_set.set_size_bytes(size);
                            self.memory_store.insert(key.clone(), arc_data);
                            mat_set.add(Materialization::InMemory { pinned: false });
                            self.add_memory_bytes(size);
                        }
                    }
                }
            }
        }

        // Clean up OutputMemoryMap: remove any data for this job's outputs
        // that was NOT drained into memory_store (e.g., off-critical-path
        // outputs with Always policy). Without this cleanup, the spawned
        // task's unconditional put() would cause unbounded growth of
        // OutputMemoryMap for non-promoted outputs.
        if let Some(mem_map) = memory_map {
            for output in &job.outputs {
                let key = output_ref_key(&output.reference);
                let _ = mem_map.remove(&key);
            }
        }

        disk_writes
    }

    /// Decrement consumer reference counts for all inputs of a completed job.
    ///
    /// When a job fires (completes), it has consumed its inputs. Decrementing
    /// the reference count on each input's `MaterializationSet` allows eviction
    /// once all consumers have fired.
    fn decrement_input_consumers(&mut self, job: &ConcreteJob) {
        for input in &job.inputs {
            let key = output_ref_key(&input.reference);
            if let Some(mat_set) = self.output_mats.get_mut(&key) {
                mat_set.consumer_fired();
            }
        }
    }

    /// Build the `input_data` map for a job by collecting in-memory data from
    /// the memory store for each input that has an `InMemory` materialization
    /// as its cheapest option.
    fn collect_input_data(&self, job: &ConcreteJob) -> HashMap<String, Arc<[u8]>> {
        let mut input_data = HashMap::new();
        for input in &job.inputs {
            let key = output_ref_key(&input.reference);
            // Check if this input's cheapest materialization is InMemory.
            if let Some(mat_set) = self.output_mats.get(&key) {
                if let Some(Materialization::InMemory { .. }) = mat_set.cheapest() {
                    if let Some(data) = self.memory_store.get(&key) {
                        input_data.insert(key, Arc::clone(data));
                    }
                }
            }
        }
        input_data
    }

    /// Select the cheapest materialization for a given output (firing-mode selection).
    ///
    /// Returns `None` if the output has no materializations yet.
    #[cfg_attr(not(test), allow(dead_code))]
    fn cheapest_materialization(&self, output_key: &str) -> Option<&Materialization> {
        self.output_mats
            .get(output_key)
            .and_then(|ms| ms.cheapest())
    }

    /// Enqueue in-memory output data for background disk persistence.
    ///
    /// Called when a job produces output in memory and the output's
    /// `MaterializePolicy` allows disk persistence (not `Never`). The
    /// disk writer will atomically write the data to disk and the
    /// scheduler can later add an `OnDisk` materialization to the
    /// `MaterializationSet`.
    ///
    /// Returns `Ok(true)` if a write was enqueued, `Ok(false)` if no disk
    /// writer is configured (writes are silently skipped), or `Err` if the
    /// disk writer channel is closed.
    #[allow(dead_code)]
    async fn enqueue_disk_write(
        &self,
        job_id: &JobId,
        target_path: std::path::PathBuf,
        data: Arc<[u8]>,
    ) -> Result<bool, OxError> {
        match &self.disk_writer {
            Some(writer) => {
                writer
                    .write(DiskWriteRequest {
                        job_id: job_id.clone(),
                        target_path,
                        data,
                    })
                    .await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Collect output keys that are eligible for eviction (zero pending consumers).
    #[cfg_attr(not(test), allow(dead_code))]
    fn evictable_outputs(&self) -> Vec<String> {
        self.output_mats
            .iter()
            .filter(|(_, ms)| ms.is_evictable() && !ms.is_empty())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Register an in-memory materialization for an output, tracking its size
    /// against the memory budget.
    #[cfg_attr(not(test), allow(dead_code))]
    fn register_in_memory(&mut self, output_key: &str, size_bytes: u64) {
        if let Some(mat_set) = self.output_mats.get_mut(output_key) {
            mat_set.add(Materialization::InMemory { pinned: false });
            // Also add OnDisk so the output has a disk fallback for
            // eviction (enforce_memory_budget skips outputs without one).
            mat_set.add(Materialization::OnDisk {
                path: std::path::PathBuf::from(output_key),
                verified: false,
            });
            mat_set.set_size_bytes(size_bytes);
            self.add_memory_bytes(size_bytes);
        }
    }

    /// Track memory addition and update the peak watermark.
    fn add_memory_bytes(&mut self, bytes: u64) {
        self.memory_used_bytes += bytes;
        if self.memory_used_bytes > self.peak_memory_bytes {
            self.peak_memory_bytes = self.memory_used_bytes;
        }
    }

    /// Current total bytes held in memory.
    #[cfg_attr(not(test), allow(dead_code))]
    fn memory_used_bytes(&self) -> u64 {
        self.memory_used_bytes
    }

    /// Build the memory stats for the scheduler result.
    fn memory_stats(&self) -> Option<MemoryStats> {
        if self.memory_budget_bytes == 0 && self.peak_memory_bytes == 0 {
            return None;
        }
        Some(MemoryStats {
            peak_memory_bytes: self.peak_memory_bytes,
            memory_budget_bytes: self.memory_budget_bytes,
            eviction_count: self.eviction_count,
            eviction_bytes: self.eviction_bytes,
        })
    }

    /// Run Belady largest-first eviction until memory usage is within budget.
    ///
    /// Belady's algorithm is optimal for offline eviction: evict the item whose
    /// next use is furthest in the future. In a DAG scheduler, `pending_consumers
    /// == 0` means "never used again" — perfect future knowledge for free.
    ///
    /// Among evictable outputs, we evict the largest first to minimize the
    /// number of evictions needed to reclaim budget.
    ///
    /// Returns the number of outputs evicted.
    fn enforce_memory_budget(&mut self) -> usize {
        if self.memory_budget_bytes == 0 {
            return 0; // unlimited
        }

        let mut evicted = 0;

        while self.memory_used_bytes > self.memory_budget_bytes {
            // Find the largest evictable in-memory output that has a disk
            // fallback. Outputs without disk fallback (e.g., Never-policy)
            // are never evicted — eviction would destroy the only copy.
            let largest = self
                .output_mats
                .iter()
                .filter(|(_, ms)| ms.is_evictable() && ms.has_in_memory() && ms.has_disk_fallback())
                .max_by_key(|(_, ms)| ms.size_bytes())
                .map(|(k, ms)| (k.clone(), ms.size_bytes()));

            match largest {
                Some((key, size)) => {
                    if let Some(mat_set) = self.output_mats.get_mut(&key) {
                        if mat_set.evict_in_memory() {
                            self.memory_used_bytes = self.memory_used_bytes.saturating_sub(size);
                            self.memory_store.remove(&key);
                            self.eviction_count += 1;
                            self.eviction_bytes += size;
                            evicted += 1;
                        } else {
                            break; // eviction guard prevented removal
                        }
                    }
                }
                None => break, // nothing evictable
            }
        }

        evicted
    }
}

/// Message sent from a completed job back to the scheduler loop.
struct CompletionMsg {
    job_id: JobId,
    job: ConcreteJob,
    result: JobResult,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the last non-empty, non-whitespace line from a stderr tail.
fn extract_last_nonempty_line(stderr_tail: Option<&str>) -> Option<String> {
    stderr_tail.and_then(|tail| {
        tail.lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
    })
}

/// Hash a string to a u64 for fast comparison (not cryptographic).
fn hash_line(line: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish()
}

/// Check if the last `threshold` entries in the recent failure hashes all
/// share the same hash. Returns the matching job IDs if so.
fn check_root_cause(recent: &[(u64, JobId)], threshold: usize) -> Option<Vec<JobId>> {
    if threshold == 0 || recent.len() < threshold {
        return None;
    }
    let tail = &recent[recent.len() - threshold..];
    let target_hash = tail[0].0;
    if tail.iter().all(|(h, _)| *h == target_hash) {
        Some(tail.iter().map(|(_, id)| id.clone()).collect())
    } else {
        None
    }
}

/// Find jobs that are ready to execute: all upstream dependencies are
/// Succeeded or Skipped, and the job is still Pending, and all blocking
/// gates are approved.
///
/// Uses the incrementally maintained `ready_frontier` set instead of
/// rescanning all pending jobs, reducing per-tick cost from
/// O(pending × deps) to O(frontier_size).
///
/// Ready jobs are sorted by descending priority (higher runs first).
async fn find_ready_jobs(
    state: &Arc<Mutex<Frontier>>,
    graph: &JobGraph,
    gate_checker: &Option<Arc<dyn GateCheck>>,
    event_bus: &EventBus,
) -> Vec<JobId> {
    // Drain the ready frontier and flip ungated jobs to Ready in a SINGLE
    // lock acquisition (H6: the previous drain/flip split-lock left a window
    // where a job cancelled between the two acquisitions could be flipped
    // back to Ready and dispatched after Cancelled). Only jobs that are
    // still Pending are considered; others (cancelled, already running)
    // are silently dropped from the frontier.
    let mut ready = Vec::new();
    let gate_blocked = {
        let mut s = state.lock().await;
        let frontier: Vec<JobId> = s.ready_frontier.drain().collect();

        let mut gate_blocked_out = Vec::new();

        for job_id in frontier {
            // Only consider jobs that are still Pending.
            if !matches!(s.get_status(&job_id), Some(JobLifecycle::Pending)) {
                continue;
            }

            let blocking = graph.blocking_gates(&job_id);
            if blocking.is_empty() {
                s.set_status(job_id.clone(), JobLifecycle::Ready);
                ready.push(job_id);
            } else {
                gate_blocked_out.push((job_id, blocking.into_iter().cloned().collect::<Vec<_>>()));
            }
        }

        gate_blocked_out
    };

    // Check gate status for blocked jobs. The gate check awaits outside the
    // lock, so the job's status can change while it is in flight — every
    // status write below re-validates that the job is still Pending (H6).
    if let Some(checker) = gate_checker {
        for (job_id, gates) in gate_blocked {
            let mut all_approved = true;
            let mut any_rejected = false;

            for gate_id in &gates {
                match checker.check_gate(gate_id).await {
                    GateStatus::Approved | GateStatus::NotFound => {}
                    GateStatus::Pending => {
                        all_approved = false;
                        // Emit gate reached event (only on first encounter).
                        event_bus.emit(Event::GateReached {
                            gate_id: gate_id.clone(),
                            message: format!(
                                "Waiting for approval to proceed with {}",
                                job_id.as_str()
                            ),
                        });
                    }
                    GateStatus::Rejected => {
                        any_rejected = true;
                        all_approved = false;
                    }
                }
            }

            let mut s = state.lock().await;
            // Re-validate: a job cancelled during the gate check must not be
            // flipped to Ready (or Cancelled again) — drop it silently.
            if !matches!(s.get_status(&job_id), Some(JobLifecycle::Pending)) {
                continue;
            }
            if any_rejected {
                s.set_status(job_id.clone(), JobLifecycle::Cancelled);
                drop(s);
                event_bus.emit(Event::JobCancelled {
                    job_id,
                    reason: "gate rejected".into(),
                });
            } else if all_approved {
                for gate_id in &gates {
                    event_bus.emit(Event::GateApproved {
                        gate_id: gate_id.clone(),
                        approved_by: "gate-check".into(),
                    });
                }
                s.set_status(job_id.clone(), JobLifecycle::Ready);
                ready.push(job_id);
            } else {
                // Gate still pending — re-add to frontier for next poll.
                s.ready_frontier.insert(job_id);
            }
        }
    } else {
        // No gate checker — treat all gates as approved.
        let mut s = state.lock().await;
        for (job_id, _) in gate_blocked {
            if matches!(s.get_status(&job_id), Some(JobLifecycle::Pending)) {
                s.set_status(job_id.clone(), JobLifecycle::Ready);
                ready.push(job_id);
            }
        }
    }

    // Sort by priority: higher priority jobs run first.
    ready.sort_by(|a, b| {
        let pa = graph.get_job(a).and_then(|j| j.priority).unwrap_or(0);
        let pb = graph.get_job(b).and_then(|j| j.priority).unwrap_or(0);
        pb.cmp(&pa)
    });

    ready
}

/// Handle a job completion: update state, emit events, propagate failures.
///
/// Returns `true` if the job's result was committed as a success (status set
/// to `Succeeded`, materializations registered, downstream promoted). The
/// caller must only record the job in the cache when this returns `true` —
/// a job that was cancelled while running (B4) can complete with exit code 0,
/// but its result is derived from a failed upstream and must not be cached.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    name = "scheduler.handle_completion",
    skip_all,
    fields(job_id = %msg.job_id, exit_code = msg.result.exit_code),
)]
async fn handle_completion<E: Executor + ?Sized>(
    msg: &CompletionMsg,
    state: &Arc<Mutex<Frontier>>,
    graph: &JobGraph,
    config: &SchedulerConfig,
    event_bus: &EventBus,
    ctx: &ExecContext,
    terminate_requested: &mut bool,
    executor: &E,
) -> bool {
    let exit_code = msg.result.exit_code;
    let duration_ms = msg.result.duration.as_millis() as u64;
    if exit_code == 0 {
        debug!(
            target: "ox.scheduler",
            counter = "scheduler.job.completed",
            job_id = %msg.job_id,
            duration_ms,
            "completed"
        );
    } else {
        warn!(
            target: "ox.scheduler",
            counter = "scheduler.job.failed",
            job_id = %msg.job_id,
            exit_code,
            duration_ms,
            "failed"
        );
    }

    // Re-read the job's status under lock (B4). If the job was cancelled
    // while running (cancel_downstream flipped it after a sibling failure),
    // its completion — success or failure — must not overwrite the Cancelled
    // status, promote downstream, or re-enter the retry loop. The
    // JobCancelled event was already emitted. (The resource budget is
    // released by the ResourceGuard dropped at task end — H8.)
    {
        let s = state.lock().await;
        if matches!(s.get_status(&msg.job_id), Some(JobLifecycle::Cancelled)) {
            debug!(
                target: "ox.scheduler",
                counter = "scheduler.job.completed_after_cancel",
                job_id = %msg.job_id,
                exit_code,
                "completion discarded: job was cancelled while running"
            );
            return false;
        }
    }

    if exit_code == 0 {
        // Success.
        let outputs: Vec<String> = msg
            .job
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

        event_bus.emit(Event::JobCompleted {
            job_id: msg.job_id.clone(),
            duration_ms,
            outputs,
        });

        let on_critical_path =
            config.critical_path_jobs.is_empty() || config.critical_path_jobs.contains(&msg.job_id);
        let disk_writes = {
            let mut s = state.lock().await;
            s.set_status(msg.job_id.clone(), JobLifecycle::Succeeded);
            // Register materializations for outputs produced by this job.
            let writes =
                s.register_materializations(&msg.job, on_critical_path, ctx.memory_map.as_ref());
            // Decrement consumer reference counts for inputs consumed by this job.
            s.decrement_input_consumers(&msg.job);
            // Enforce memory budget — evict largest-first (Belady optimal).
            s.enforce_memory_budget();
            // This job actually executed — mark all downstream as needing
            // re-execution to ensure transitive cache invalidation (ox-jxdw).
            for dep in graph.downstream(&msg.job_id) {
                s.force_rerun.insert(dep.clone());
            }
            s.promote_downstream(&msg.job_id, graph);
            writes
        };
        // Enqueue background disk writes outside the scheduler lock.
        // The disk writer persists in-memory outputs asynchronously.
        if !disk_writes.is_empty() {
            let s = state.lock().await;
            if let Some(ref writer) = s.disk_writer {
                let writer = writer.clone();
                drop(s); // Release lock before async send.
                for req in disk_writes {
                    if let Err(e) = writer.write(req).await {
                        // Channel closed = disk writer task died (systemic failure).
                        // Log and continue — in-memory data is still available for
                        // downstream jobs, but the output won't be persisted to disk.
                        eprintln!(
                            "[memory-budget] disk-writer send failed for job {}: {e}",
                            msg.job_id
                        );
                    }
                }
            }
        }
        true
    } else {
        // Failure — check error strategy.
        let error_msg = format!("exit code {}", exit_code);

        match &msg.job.error_strategy {
            ErrorStrategy::Ignore => {
                // Treat as success.
                event_bus.emit(Event::JobCompleted {
                    job_id: msg.job_id.clone(),
                    duration_ms,
                    outputs: vec![],
                });
                let on_critical_path = config.critical_path_jobs.is_empty()
                    || config.critical_path_jobs.contains(&msg.job_id);
                let disk_writes = {
                    let mut s = state.lock().await;
                    s.set_status(msg.job_id.clone(), JobLifecycle::Succeeded);
                    // Register materializations + decrement consumers even on ignored failures.
                    let writes = s.register_materializations(
                        &msg.job,
                        on_critical_path,
                        ctx.memory_map.as_ref(),
                    );
                    s.decrement_input_consumers(&msg.job);
                    s.enforce_memory_budget();
                    for dep in graph.downstream(&msg.job_id) {
                        s.force_rerun.insert(dep.clone());
                    }
                    s.promote_downstream(&msg.job_id, graph);
                    writes
                };
                if !disk_writes.is_empty() {
                    let s = state.lock().await;
                    if let Some(ref writer) = s.disk_writer {
                        let writer = writer.clone();
                        drop(s);
                        for req in disk_writes {
                            if let Err(e) = writer.write(req).await {
                                eprintln!(
                                    "[memory-budget] disk-writer send failed for job {}: {e}",
                                    msg.job_id
                                );
                            }
                        }
                    }
                }
            }
            ErrorStrategy::Retry { count, backoff } => {
                // Check if we have retries remaining.
                let (current_attempt, should_retry) = {
                    let mut s = state.lock().await;
                    let attempt = s.retry_counts.entry(msg.job_id.clone()).or_insert(0);
                    *attempt += 1;
                    let a = *attempt;
                    let retry = a < *count;
                    if retry {
                        s.set_status(msg.job_id.clone(), JobLifecycle::Pending);
                        // Re-add to frontier — deps are still satisfied.
                        s.ready_frontier.insert(msg.job_id.clone());
                    } else {
                        s.set_status(msg.job_id.clone(), JobLifecycle::Failed(error_msg.clone()));
                    }
                    (a, retry)
                };

                if should_retry {
                    event_bus.emit(Event::JobFailed {
                        job_id: msg.job_id.clone(),
                        error_message: format!(
                            "{} (attempt {}/{}, retrying)",
                            error_msg, current_attempt, count
                        ),
                        exit_code: Some(exit_code),
                        stderr_tail: msg.result.stderr_tail.clone(),
                    });

                    // Backoff delay before retry.
                    let base_ms: u64 = 1000;
                    let delay_ms = match backoff {
                        Backoff::Constant => base_ms,
                        Backoff::Linear => base_ms * (current_attempt as u64),
                        Backoff::Exponential => base_ms * 2u64.saturating_pow(current_attempt - 1),
                    };
                    if delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                } else {
                    event_bus.emit(Event::JobFailed {
                        job_id: msg.job_id.clone(),
                        error_message: format!("{} (exhausted {} retries)", error_msg, count),
                        exit_code: Some(exit_code),
                        stderr_tail: msg.result.stderr_tail.clone(),
                    });

                    let last_line = extract_last_nonempty_line(msg.result.stderr_tail.as_deref());

                    let root_cause_triggered = {
                        let mut s = state.lock().await;
                        s.failed_details.push(FailedJobDetail {
                            job_id: msg.job_id.clone(),
                            last_stderr_line: last_line.clone(),
                        });

                        // Track for root-cause detection.
                        if let Some(ref line) = last_line {
                            s.recent_failure_hashes
                                .push((hash_line(line), msg.job_id.clone()));
                        }

                        // Check root-cause threshold.
                        if s.root_cause.is_none() && config.root_cause_threshold > 0 {
                            if let Some(ref line) = last_line {
                                if let Some(matching_ids) = check_root_cause(
                                    &s.recent_failure_hashes,
                                    config.root_cause_threshold,
                                ) {
                                    s.root_cause = Some(RootCause {
                                        error_line: line.clone(),
                                        job_ids: matching_ids.clone(),
                                    });
                                    Some((line.clone(), matching_ids))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    };

                    if !config.keep_going {
                        *terminate_requested = true;
                    }

                    // Emit root-cause event outside the lock.
                    if let Some((root_line, matching_ids)) = root_cause_triggered {
                        if !config.keep_going {
                            *terminate_requested = true;
                        }
                        event_bus.emit(Event::RootCauseDetected {
                            root_cause: root_line,
                            failure_count: matching_ids.len(),
                            job_ids: matching_ids,
                        });
                    }

                    cancel_downstream(&msg.job_id, state, graph, event_bus, executor).await;
                }
            }
            ErrorStrategy::Terminate | ErrorStrategy::Finish => {
                event_bus.emit(Event::JobFailed {
                    job_id: msg.job_id.clone(),
                    error_message: error_msg.clone(),
                    exit_code: Some(exit_code),
                    stderr_tail: msg.result.stderr_tail.clone(),
                });

                let last_line = extract_last_nonempty_line(msg.result.stderr_tail.as_deref());

                let mut s = state.lock().await;
                s.set_status(msg.job_id.clone(), JobLifecycle::Failed(error_msg));
                s.failed_details.push(FailedJobDetail {
                    job_id: msg.job_id.clone(),
                    last_stderr_line: last_line.clone(),
                });

                // Track for root-cause detection.
                if let Some(ref line) = last_line {
                    s.recent_failure_hashes
                        .push((hash_line(line), msg.job_id.clone()));
                }

                // Check root-cause threshold.
                let root_cause_triggered = if s.root_cause.is_none()
                    && config.root_cause_threshold > 0
                {
                    if let Some(ref line) = last_line {
                        if let Some(matching_ids) =
                            check_root_cause(&s.recent_failure_hashes, config.root_cause_threshold)
                        {
                            s.root_cause = Some(RootCause {
                                error_line: line.clone(),
                                job_ids: matching_ids.clone(),
                            });
                            Some((line.clone(), matching_ids))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                if !config.keep_going {
                    *terminate_requested = true;
                }
                drop(s);

                // Emit root-cause event outside the lock.
                if let Some((root_line, matching_ids)) = root_cause_triggered {
                    if !config.keep_going {
                        *terminate_requested = true;
                    }
                    event_bus.emit(Event::RootCauseDetected {
                        root_cause: root_line,
                        failure_count: matching_ids.len(),
                        job_ids: matching_ids,
                    });
                }

                cancel_downstream(&msg.job_id, state, graph, event_bus, executor).await;
            }
        }
        false
    }
}

/// Recursively cancel all downstream jobs of a failed job.
///
/// Handles three states for downstream jobs:
///
/// - `Pending` / `Ready`: simply mark as `Cancelled` (job was never dispatched).
/// - `Running`: mark as `Cancelled` AND request the executor to terminate the
///   running process. This prevents the silent-correctness race where job B is
///   already executing when upstream job A fails — without executor.cancel, B
///   continues to completion, writes outputs from stale inputs, and those
///   outputs may be cached on next run.
///
/// Note: the executor.cancel call is fire-and-forget (errors logged but not
/// propagated). The actual `Failed`/`Cancelled` status transition for the
/// running job happens via `handle_completion` when the child process exits;
/// here we only flip the schedule status so downstream propagation stops and
/// no further state writes happen on its behalf.
#[tracing::instrument(
    name = "scheduler.cancel_downstream",
    skip(state, graph, event_bus, executor),
    fields(failed_id = %failed_id),
)]
async fn cancel_downstream<E: Executor + ?Sized>(
    failed_id: &JobId,
    state: &Arc<Mutex<Frontier>>,
    graph: &JobGraph,
    event_bus: &EventBus,
    executor: &E,
) {
    let mut to_cancel: Vec<JobId> = graph.downstream(failed_id).into_iter().cloned().collect();
    let mut visited = HashSet::new();
    visited.insert(failed_id.clone());

    while let Some(id) = to_cancel.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let mut s = state.lock().await;
        let status_snapshot = s.get_status(&id).cloned();
        if let Some(status) = status_snapshot {
            match status {
                JobLifecycle::Pending | JobLifecycle::Ready => {
                    s.set_status(id.clone(), JobLifecycle::Cancelled);
                    s.ready_frontier.remove(&id);
                    drop(s);
                    event_bus.emit(Event::JobCancelled {
                        job_id: id.clone(),
                        reason: format!("upstream job {} failed", failed_id),
                    });
                    debug!(
                        target: "ox.scheduler",
                        counter = "scheduler.job.cancelled_pending",
                        job_id = %id,
                        upstream = %failed_id,
                        "cancel_pending"
                    );
                    let further = graph.downstream(&id);
                    to_cancel.extend(further.into_iter().cloned());
                }
                JobLifecycle::Running => {
                    // Critical correctness fix: mark Cancelled, then signal the
                    // executor. If we left this as Running, the in-flight task
                    // would commit its outputs on success and they would be
                    // derived from stale upstream data (the failed job).
                    s.set_status(id.clone(), JobLifecycle::Cancelled);
                    drop(s);
                    event_bus.emit(Event::JobCancelled {
                        job_id: id.clone(),
                        reason: format!("upstream job {} failed", failed_id),
                    });
                    if let Err(e) = executor.cancel(&id).await {
                        warn!(
                            target: "ox.scheduler",
                            job_id = %id,
                            error = %e,
                            "executor.cancel failed for running downstream job"
                        );
                    }
                    debug!(
                        target: "ox.scheduler",
                        counter = "scheduler.job.cancelled_running",
                        job_id = %id,
                        upstream = %failed_id,
                        "cancel_running"
                    );
                    // Still propagate to transitive downstream: a cancelled
                    // running job has no usable outputs, so downstream must
                    // also be cancelled.
                    let further = graph.downstream(&id);
                    to_cancel.extend(further.into_iter().cloned());
                }
                _ => {
                    // Already terminal (Succeeded, Failed, Skipped, Cancelled).
                    // Do not re-propagate — its downstream was either already
                    // handled or doesn't depend on this branch.
                }
            }
        }
    }
}

/// Cancel all remaining pending/ready jobs (called during termination).
async fn cancel_remaining(state: &Arc<Mutex<Frontier>>, _graph: &JobGraph, event_bus: &EventBus) {
    let mut s = state.lock().await;
    let to_cancel: Vec<JobId> = s
        .statuses
        .iter()
        .filter(|(_, st)| matches!(st, JobLifecycle::Pending | JobLifecycle::Ready))
        .map(|(id, _)| id.clone())
        .collect();

    for id in to_cancel {
        s.set_status(id.clone(), JobLifecycle::Cancelled);
        event_bus.emit(Event::JobCancelled {
            job_id: id,
            reason: "run terminated due to failure".into(),
        });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job_graph::make_test_job;
    use crate::traits::executor::{DagSubmission, ExecutorCapabilities, Workspace};
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    // -- Mock executor -------------------------------------------------------

    /// A mock executor for testing that returns configured exit codes per job.
    #[derive(Debug)]
    struct MockExecutor {
        /// Map of job_id -> exit_code. Jobs not in the map succeed (exit 0).
        results: BTreeMap<String, i32>,
        /// Optional delay per job execution.
        delay: Option<Duration>,
        /// Count of execute calls (for concurrency verification).
        call_count: AtomicUsize,
    }

    impl MockExecutor {
        fn new() -> Self {
            Self {
                results: BTreeMap::new(),
                delay: None,
                call_count: AtomicUsize::new(0),
            }
        }

        fn with_results(results: BTreeMap<String, i32>) -> Self {
            Self {
                results,
                delay: None,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("mock error: {0}")]
    struct MockError(String);

    impl Executor for MockExecutor {
        type Error = MockError;

        async fn init(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn health_check(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn cleanup(&self) -> Result<(), MockError> {
            Ok(())
        }
        fn capabilities(&self) -> ExecutorCapabilities {
            ExecutorCapabilities::default()
        }
        fn max_concurrency(&self) -> Option<usize> {
            None
        }
        async fn prepare_workspace(
            &self,
            _job: &ConcreteJob,
            _ctx: &ExecContext,
        ) -> Result<Workspace, MockError> {
            Ok(Workspace::new(PathBuf::from("/tmp/mock")))
        }
        async fn execute(
            &self,
            job: &ConcreteJob,
            _workspace: &Workspace,
            _ctx: &ExecContext,
        ) -> Result<JobResult, MockError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if let Some(d) = self.delay {
                tokio::time::sleep(d).await;
            }
            let exit_code = self.results.get(job.id.as_str()).copied().unwrap_or(0);
            Ok(JobResult {
                job_id: job.id.clone(),
                exit_code,
                duration: self.delay.unwrap_or(Duration::from_millis(1)),
                peak_memory_bytes: None,
                cpu_time: None,
                log_path: None,
                stderr_tail: None,
            })
        }
        async fn finalize_workspace(
            &self,
            _workspace: Workspace,
            _result: &JobResult,
        ) -> Result<(), MockError> {
            Ok(())
        }
        async fn cancel(&self, _job_id: &JobId) -> Result<(), MockError> {
            Ok(())
        }
        async fn poll_status(&self, _job_id: &JobId) -> Result<JobStatus, MockError> {
            Ok(JobStatus::Completed)
        }
        async fn submit_dag(
            &self,
            _graph: &crate::job_graph::JobGraph,
            _ctx: &ExecContext,
        ) -> Result<DagSubmission, MockError> {
            Err(MockError("DAG submission not supported by mock".into()))
        }
    }

    // -- Test helpers --------------------------------------------------------

    fn default_ctx() -> ExecContext {
        ExecContext {
            global_job_limit: 4,
            run_id: "test-run".into(),
            log_dir: PathBuf::from("/tmp/test-logs"),
            project_dir: PathBuf::from("/tmp/test-project"),
            trusted_dirs: vec![],
            input_data: HashMap::new(),
            memory_map: None,
        }
    }

    fn make_job(id: &str, rule: &str, inputs: Vec<&str>, outputs: Vec<&str>) -> ConcreteJob {
        ConcreteJob {
            id: JobId::from(id),
            rule: RuleName::from(rule),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: inputs
                .into_iter()
                .map(|p| ResolvedInput {
                    reference: OutputRef::File(PathBuf::from(p)),
                    name: None,
                    format: None,
                })
                .collect(),
            outputs: outputs
                .into_iter()
                .map(|p| ResolvedOutput {
                    reference: OutputRef::File(PathBuf::from(p)),
                    name: None,
                    format: None,
                    lifecycle: OutputLifecycle::default(),
                    materialize: MaterializePolicy::default(),
                })
                .collect(),
            execution: ExecutionBlock::Shell {
                command: "true".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::default(),
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        }
    }

    fn make_job_with_strategy(
        id: &str,
        rule: &str,
        inputs: Vec<&str>,
        outputs: Vec<&str>,
        strategy: ErrorStrategy,
    ) -> ConcreteJob {
        let mut job = make_job(id, rule, inputs, outputs);
        job.error_strategy = strategy;
        job
    }

    // -- Tests ---------------------------------------------------------------

    #[tokio::test]
    async fn empty_graph() {
        let graph = JobGraph::build(vec![]).unwrap();
        let executor = Arc::new(MockExecutor::new());
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.total_jobs, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.cancelled, 0);
    }

    #[tokio::test]
    async fn single_job_success() {
        let jobs = vec![make_job("j1", "build", vec![], vec!["out.txt"])];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::new());
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.total_jobs, 1);
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(executor.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn linear_chain_executes_in_order() {
        // A -> B -> C
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "rC", vec!["b.txt"], vec!["c.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::new());
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.succeeded, 3);
        assert_eq!(executor.call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn parallel_independent_jobs() {
        // Two independent jobs with no dependencies between them.
        let jobs = vec![
            make_job("X", "rX", vec![], vec!["x.txt"]),
            make_job("Y", "rY", vec![], vec!["y.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::new());
        let config = SchedulerConfig {
            max_jobs: 4,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.succeeded, 2);
        assert_eq!(executor.call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn job_failure_with_terminate_cancels_downstream() {
        // A(fail) -> B -> C
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "rC", vec!["b.txt"], vec!["c.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "A".into(),
            1,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert!(result.failed > 0);
    }

    // -------------------------------------------------------------------
    // Rank 2 — Cancel-during-running invariant
    //
    // Pin the end-state invariant: a job whose upstream has failed must
    // finish the run in `Cancelled` — never `Succeeded`, never `Failed` —
    // regardless of whether it was already dispatched at the moment its
    // upstream's failure surfaced (forgemaster §1.7 silent-incorrect-
    // behaviour mode). The focused unit regression on `cancel_downstream`
    // internals lives alongside the M2 fix itself (task-20260527-1811);
    // this companion gate guards the externally-observable contract so
    // that a future refactor of the dispatch lifecycle (speculative pre-
    // dispatch, retry-after-success flip, multi-attempt success/fail) can
    // not silently regress to "downstream succeeded on stale upstream".
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn upstream_failure_downstream_never_succeeded() {
        // A → B; A fails. The end state must classify B as Cancelled (not
        // Succeeded, not Failed) regardless of whether B was dispatched.
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "A".into(),
            1,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.failed, 1, "A failed");
        assert_eq!(
            result.cancelled, 1,
            "B must be Cancelled because its upstream A failed — never Succeeded"
        );
        assert_eq!(
            result.succeeded, 0,
            "no downstream of a failed upstream may end in Succeeded"
        );
    }

    /// Regression test for ox-6d6: when jobs fail with keep_going=false (default),
    /// the SchedulerResult must still contain accurate succeeded/failed/skipped
    /// counts so callers can record them (e.g. in `ox history --json`).
    #[tokio::test]
    async fn failed_run_returns_accurate_stats() {
        // A(ok) -> B(fail) -> C(cancelled), D(ok, independent)
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "rC", vec!["b.txt"], vec!["c.txt"]),
            make_job("D", "rD", vec![], vec!["d.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "B".into(),
            1,
        )])));
        let config = SchedulerConfig {
            keep_going: true,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        // A and D succeed, B fails, C is cancelled.
        assert_eq!(result.succeeded, 2, "A and D should succeed");
        assert_eq!(result.failed, 1, "B should fail");
        assert_eq!(result.cancelled, 1, "C should be cancelled");
        assert_eq!(result.total_jobs, 4);
    }

    /// Regression test for ox-6d6: even without keep_going, the result must
    /// carry the actual failure count (previously returned Err, losing stats).
    #[tokio::test]
    async fn failed_run_without_keep_going_returns_stats() {
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "A".into(),
            1,
        )])));
        // keep_going = false (default) — this was the buggy path.
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.failed, 1, "A should be counted as failed");
        assert_eq!(
            result.cancelled, 1,
            "B should be cancelled (downstream of A)"
        );
        assert!(
            result.succeeded + result.failed + result.skipped + result.cancelled
                == result.total_jobs,
            "all jobs must be accounted for"
        );
    }

    #[tokio::test]
    async fn job_failure_with_ignore_continues() {
        // A(fail, ignore) -> B
        let jobs = vec![
            make_job_with_strategy("A", "rA", vec![], vec!["a.txt"], ErrorStrategy::Ignore),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "A".into(),
            1,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        // A is treated as success (ignore), B runs normally.
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 0);
    }

    #[tokio::test]
    async fn keep_going_continues_independent_branches() {
        // A(fail) -> B,  C (independent)
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "rC", vec![], vec!["c.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "A".into(),
            1,
        )])));
        let config = SchedulerConfig {
            max_jobs: 1,
            keep_going: true,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        // A failed, B cancelled, C succeeded.
        assert_eq!(result.failed, 1);
        assert_eq!(result.cancelled, 1);
        assert_eq!(result.succeeded, 1);
    }

    #[tokio::test]
    async fn all_jobs_succeed_events_emitted() {
        let jobs = vec![make_job("j1", "r1", vec![], vec!["out.txt"])];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::new());
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let ctx = default_ctx();

        let _result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        // Collect events.
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Should have RunStarted, JobStarted, JobCompleted, RunCompleted.
        assert!(events.iter().any(|e| matches!(e, Event::RunStarted { .. })));
        assert!(events.iter().any(|e| matches!(e, Event::JobStarted { .. })));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::JobCompleted { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::RunCompleted { .. }))
        );
    }

    #[tokio::test]
    async fn scheduler_config_default() {
        let config = SchedulerConfig::default();
        assert_eq!(config.max_jobs, 1);
        assert!(config.resource_budget.is_empty());
        assert!(!config.keep_going);
    }

    #[tokio::test]
    async fn schedule_status_is_terminal() {
        assert!(!JobLifecycle::Pending.is_terminal());
        assert!(!JobLifecycle::Ready.is_terminal());
        assert!(!JobLifecycle::Running.is_terminal());
        assert!(JobLifecycle::Succeeded.is_terminal());
        assert!(JobLifecycle::Failed("err".into()).is_terminal());
        assert!(JobLifecycle::Skipped.is_terminal());
        assert!(JobLifecycle::Cancelled.is_terminal());
    }

    #[test]
    fn schedule_status_from_job_status() {
        assert_eq!(JobLifecycle::from(JobStatus::Queued), JobLifecycle::Pending);
        assert_eq!(
            JobLifecycle::from(JobStatus::Running),
            JobLifecycle::Running
        );
        assert_eq!(
            JobLifecycle::from(JobStatus::Completed),
            JobLifecycle::Succeeded
        );
        assert_eq!(
            JobLifecycle::from(JobStatus::Failed("oops".into())),
            JobLifecycle::Failed("oops".into())
        );
        assert_eq!(
            JobLifecycle::from(JobStatus::Cancelled),
            JobLifecycle::Cancelled
        );
    }

    // -- Fatal-abort mock executor (ox-dfno regression) ----------------------

    /// A mock executor where one job returns a fatal `Err` from `execute`,
    /// while other jobs sleep long enough to be in-flight when the abort fires.
    /// Records which job IDs were cancel()ed so the test can verify cleanup.
    #[derive(Debug)]
    struct FatalMockExecutor {
        /// Job ID that triggers a fatal executor error.
        fatal_job: String,
        /// IDs passed to cancel(), in call order.
        cancelled: std::sync::Mutex<Vec<String>>,
    }

    impl FatalMockExecutor {
        fn new(fatal_job: &str) -> Self {
            Self {
                fatal_job: fatal_job.into(),
                cancelled: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn cancelled_jobs(&self) -> Vec<String> {
            self.cancelled.lock().unwrap().clone()
        }
    }

    impl Executor for FatalMockExecutor {
        type Error = MockError;

        async fn init(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn health_check(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn cleanup(&self) -> Result<(), MockError> {
            Ok(())
        }
        fn capabilities(&self) -> ExecutorCapabilities {
            ExecutorCapabilities::default()
        }
        fn max_concurrency(&self) -> Option<usize> {
            None
        }
        async fn prepare_workspace(
            &self,
            _job: &ConcreteJob,
            _ctx: &ExecContext,
        ) -> Result<Workspace, MockError> {
            Ok(Workspace::new(PathBuf::from("/tmp/mock")))
        }
        async fn execute(
            &self,
            job: &ConcreteJob,
            _workspace: &Workspace,
            _ctx: &ExecContext,
        ) -> Result<JobResult, MockError> {
            if job.id.as_str() == self.fatal_job {
                // Return a fatal executor error (not a job failure — an Err).
                return Err(MockError("fatal executor error".into()));
            }
            // Non-fatal jobs sleep so they're in-flight during the abort.
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(JobResult {
                job_id: job.id.clone(),
                exit_code: 0,
                duration: Duration::from_millis(1),
                peak_memory_bytes: None,
                cpu_time: None,
                log_path: None,
                stderr_tail: None,
            })
        }
        async fn finalize_workspace(
            &self,
            _workspace: Workspace,
            _result: &JobResult,
        ) -> Result<(), MockError> {
            Ok(())
        }
        async fn cancel(&self, job_id: &JobId) -> Result<(), MockError> {
            self.cancelled.lock().unwrap().push(job_id.to_string());
            Ok(())
        }
        async fn poll_status(&self, _job_id: &JobId) -> Result<JobStatus, MockError> {
            Ok(JobStatus::Completed)
        }
        async fn submit_dag(
            &self,
            _graph: &crate::job_graph::JobGraph,
            _ctx: &ExecContext,
        ) -> Result<DagSubmission, MockError> {
            Err(MockError("DAG submission not supported by mock".into()))
        }
    }

    /// Regression test for ox-dfno: fatal executor error must call cancel() on
    /// all in-flight jobs before aborting tokio tasks. Without this, external
    /// processes (child PIDs, SLURM jobs) leak.
    #[tokio::test(start_paused = true)]
    async fn fatal_abort_cancels_in_flight_jobs() {
        // Two independent jobs: A (fatal) and B (sleeps forever).
        // Both dispatch concurrently with max_jobs=2.
        // When A returns Err, B should be cancel()ed before abort.
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec![], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(FatalMockExecutor::new("A"));
        let config = SchedulerConfig {
            max_jobs: 2,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx).await;

        // Should return the fatal error.
        assert!(result.is_err(), "fatal executor error should propagate");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("fatal executor error"),
            "error should contain executor message, got: {err_msg}"
        );

        // B should have been cancel()ed (it was in-flight when A failed).
        let cancelled = executor.cancelled_jobs();
        assert!(
            cancelled.contains(&"B".to_string()),
            "in-flight job B should be cancel()ed on fatal abort, got: {cancelled:?}"
        );
    }

    // -- cancel_downstream Running-state regression --------------------------

    /// Regression: `cancel_downstream` must terminate downstream jobs that are
    /// already `Running` — not just `Pending`/`Ready`. Without this, a
    /// downstream job whose upstream just failed could continue to completion,
    /// commit outputs derived from stale or partial inputs, and then be
    /// cached, silently propagating the failure into subsequent runs.
    ///
    /// We exercise the helper directly because reproducing the natural race
    /// requires very specific interleavings of the dispatch loop. Testing the
    /// invariant (Running ⇒ Cancelled + executor.cancel called) directly is
    /// both faster and more deterministic than driving the full scheduler.
    #[tokio::test]
    async fn cancel_downstream_terminates_running_jobs() {
        // Graph: A → B → C. A has just failed; B is in the Running state
        // (modeling the race), C is still Pending.
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "rC", vec!["b.txt"], vec!["c.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(FatalMockExecutor::new("__never_fires__"));

        let topo = graph.topological_order().unwrap();
        let mut sched_state =
            Frontier::new(&topo, &graph, 0, None, ResourceBudget::new(BTreeMap::new()));
        // Manually seed the state: A failed, B is Running, C is Pending.
        sched_state.set_status(JobId::from("A"), JobLifecycle::Failed("boom".into()));
        sched_state.set_status(JobId::from("B"), JobLifecycle::Running);
        // C stays at its default Pending.
        let state = Arc::new(Mutex::new(sched_state));

        let bus = EventBus::new();
        let mut sub = bus.subscribe();

        cancel_downstream(&JobId::from("A"), &state, &graph, &bus, executor.as_ref()).await;

        // (1) B must now be Cancelled in the schedule state.
        let s = state.lock().await;
        assert_eq!(
            s.get_status(&JobId::from("B")),
            Some(&JobLifecycle::Cancelled),
            "Running downstream B must be marked Cancelled"
        );
        // (2) C (transitively downstream) is also Cancelled.
        assert_eq!(
            s.get_status(&JobId::from("C")),
            Some(&JobLifecycle::Cancelled),
            "transitive downstream C must be cancelled too"
        );
        drop(s);

        // (3) executor.cancel() must have been called for the Running job B.
        let cancelled = executor.cancelled_jobs();
        assert!(
            cancelled.contains(&"B".to_string()),
            "executor.cancel must be invoked for Running downstream, got: {cancelled:?}"
        );

        // (4) JobCancelled event must have been emitted for B.
        let mut saw_b_cancelled = false;
        while let Ok(ev) = sub.try_recv() {
            if matches!(ev, Event::JobCancelled { ref job_id, .. } if job_id.as_str() == "B") {
                saw_b_cancelled = true;
            }
        }
        assert!(saw_b_cancelled, "Event::JobCancelled for B must be emitted");
    }

    // -- Retry-aware mock executor -------------------------------------------

    /// A mock executor that fails a configurable number of times per job before
    /// succeeding. Used to test retry-then-succeed scenarios.
    #[derive(Debug)]
    struct RetryMockExecutor {
        /// Number of times each job should fail before succeeding.
        /// Jobs not in the map always succeed.
        fail_count: BTreeMap<String, u32>,
        /// Track how many times each job has been executed.
        attempts: std::sync::Mutex<BTreeMap<String, u32>>,
    }

    impl RetryMockExecutor {
        /// Create an executor where `fail_count[job_id]` failures occur before
        /// the job starts succeeding.
        fn new(fail_count: BTreeMap<String, u32>) -> Self {
            Self {
                fail_count,
                attempts: std::sync::Mutex::new(BTreeMap::new()),
            }
        }

        fn total_attempts(&self, job_id: &str) -> u32 {
            self.attempts
                .lock()
                .unwrap()
                .get(job_id)
                .copied()
                .unwrap_or(0)
        }
    }

    impl Executor for RetryMockExecutor {
        type Error = MockError;

        async fn init(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn health_check(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn cleanup(&self) -> Result<(), MockError> {
            Ok(())
        }
        fn capabilities(&self) -> ExecutorCapabilities {
            ExecutorCapabilities::default()
        }
        fn max_concurrency(&self) -> Option<usize> {
            None
        }
        async fn prepare_workspace(
            &self,
            _job: &ConcreteJob,
            _ctx: &ExecContext,
        ) -> Result<Workspace, MockError> {
            Ok(Workspace::new(PathBuf::from("/tmp/mock")))
        }
        async fn execute(
            &self,
            job: &ConcreteJob,
            _workspace: &Workspace,
            _ctx: &ExecContext,
        ) -> Result<JobResult, MockError> {
            let attempt = {
                let mut attempts = self.attempts.lock().unwrap();
                let a = attempts.entry(job.id.to_string()).or_insert(0);
                *a += 1;
                *a
            };
            let max_failures = self.fail_count.get(job.id.as_str()).copied().unwrap_or(0);
            let exit_code = if attempt <= max_failures { 1 } else { 0 };
            Ok(JobResult {
                job_id: job.id.clone(),
                exit_code,
                duration: Duration::from_millis(1),
                peak_memory_bytes: None,
                cpu_time: None,
                log_path: None,
                stderr_tail: None,
            })
        }
        async fn finalize_workspace(
            &self,
            _workspace: Workspace,
            _result: &JobResult,
        ) -> Result<(), MockError> {
            Ok(())
        }
        async fn cancel(&self, _job_id: &JobId) -> Result<(), MockError> {
            Ok(())
        }
        async fn poll_status(&self, _job_id: &JobId) -> Result<JobStatus, MockError> {
            Ok(JobStatus::Completed)
        }
        async fn submit_dag(
            &self,
            _graph: &crate::job_graph::JobGraph,
            _ctx: &ExecContext,
        ) -> Result<DagSubmission, MockError> {
            Err(MockError("DAG submission not supported by mock".into()))
        }
    }

    // -- Retry integration tests ---------------------------------------------

    /// Collect all events from the broadcast receiver.
    fn drain_events(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[tokio::test(start_paused = true)]
    async fn retry_exhaustion_with_constant_backoff() {
        // Job fails all attempts → should exhaust retries and report failure.
        let jobs = vec![make_job_with_strategy(
            "flaky",
            "build",
            vec![],
            vec!["out.txt"],
            ErrorStrategy::Retry {
                count: 3,
                backoff: Backoff::Constant,
            },
        )];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "flaky".into(),
            1,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert!(result.failed > 0);

        let events = drain_events(&mut rx);
        let fail_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Event::JobFailed { .. }))
            .collect();

        // With count=3: attempts 1 and 2 trigger retries, attempt 3 exhausts.
        // So we expect 3 JobFailed events total (2 retry + 1 exhaustion).
        assert_eq!(fail_events.len(), 3);

        // Verify retry attempt messages.
        if let Event::JobFailed { error_message, .. } = &fail_events[0] {
            assert!(error_message.contains("attempt 1/3"));
            assert!(error_message.contains("retrying"));
        } else {
            panic!("expected JobFailed event");
        }

        if let Event::JobFailed { error_message, .. } = &fail_events[1] {
            assert!(error_message.contains("attempt 2/3"));
            assert!(error_message.contains("retrying"));
        } else {
            panic!("expected JobFailed event");
        }

        // Final event should indicate exhaustion.
        if let Event::JobFailed { error_message, .. } = &fail_events[2] {
            assert!(error_message.contains("exhausted 3 retries"));
        } else {
            panic!("expected JobFailed event");
        }

        // Total executions: 1 original + 2 retries = 3.
        assert_eq!(executor.call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_succeeds_on_second_attempt() {
        // Job fails once, then succeeds on retry.
        let jobs = vec![make_job_with_strategy(
            "flaky",
            "build",
            vec![],
            vec!["out.txt"],
            ErrorStrategy::Retry {
                count: 3,
                backoff: Backoff::Constant,
            },
        )];
        let graph = JobGraph::build(jobs).unwrap();
        // Fail 1 time, then succeed.
        let executor = Arc::new(RetryMockExecutor::new(BTreeMap::from([(
            "flaky".into(),
            1,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(executor.total_attempts("flaky"), 2); // 1 fail + 1 success

        let events = drain_events(&mut rx);
        // Should have exactly 1 JobFailed (the retry) and 1 JobCompleted.
        let fail_count = events
            .iter()
            .filter(|e| matches!(e, Event::JobFailed { .. }))
            .count();
        let complete_count = events
            .iter()
            .filter(|e| matches!(e, Event::JobCompleted { .. }))
            .count();
        assert_eq!(fail_count, 1);
        assert_eq!(complete_count, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_succeeds_on_last_attempt() {
        // Job fails count-1 times (2 failures with count=3), then succeeds.
        let jobs = vec![make_job_with_strategy(
            "flaky",
            "build",
            vec![],
            vec!["out.txt"],
            ErrorStrategy::Retry {
                count: 3,
                backoff: Backoff::Exponential,
            },
        )];
        let graph = JobGraph::build(jobs).unwrap();
        // Fail 2 times, succeed on 3rd execution.
        let executor = Arc::new(RetryMockExecutor::new(BTreeMap::from([(
            "flaky".into(),
            2,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(executor.total_attempts("flaky"), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_exponential_backoff_timing() {
        // Verify exponential backoff delays: 1s, 2s for count=3 (2 retries).
        let jobs = vec![make_job_with_strategy(
            "flaky",
            "build",
            vec![],
            vec!["out.txt"],
            ErrorStrategy::Retry {
                count: 3,
                backoff: Backoff::Exponential,
            },
        )];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "flaky".into(),
            1,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let start = tokio::time::Instant::now();
        let _result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        // Exponential: attempt 1 → 1000ms, attempt 2 → 2000ms.
        // Total backoff = 3000ms. With paused time, this is exact.
        assert!(
            elapsed >= Duration::from_millis(3000),
            "expected >=3000ms total backoff, got {:?}",
            elapsed
        );
        assert!(
            elapsed < Duration::from_millis(3500),
            "backoff took too long: {:?}",
            elapsed
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retry_linear_backoff_timing() {
        // Verify linear backoff delays: 1s, 2s for count=3 (2 retries).
        let jobs = vec![make_job_with_strategy(
            "flaky",
            "build",
            vec![],
            vec!["out.txt"],
            ErrorStrategy::Retry {
                count: 3,
                backoff: Backoff::Linear,
            },
        )];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "flaky".into(),
            1,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let start = tokio::time::Instant::now();
        let _result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        // Linear: attempt 1 → 1000ms, attempt 2 → 2000ms.
        // Total backoff = 3000ms.
        assert!(
            elapsed >= Duration::from_millis(3000),
            "expected >=3000ms total backoff, got {:?}",
            elapsed
        );
        assert!(
            elapsed < Duration::from_millis(3500),
            "backoff took too long: {:?}",
            elapsed
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retry_constant_backoff_timing() {
        // Verify constant backoff delays: 1s, 1s for count=3 (2 retries).
        let jobs = vec![make_job_with_strategy(
            "flaky",
            "build",
            vec![],
            vec!["out.txt"],
            ErrorStrategy::Retry {
                count: 3,
                backoff: Backoff::Constant,
            },
        )];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "flaky".into(),
            1,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let start = tokio::time::Instant::now();
        let _result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        // Constant: attempt 1 → 1000ms, attempt 2 → 1000ms.
        // Total backoff = 2000ms.
        assert!(
            elapsed >= Duration::from_millis(2000),
            "expected >=2000ms total backoff, got {:?}",
            elapsed
        );
        assert!(
            elapsed < Duration::from_millis(2500),
            "backoff took too long: {:?}",
            elapsed
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retry_exhaustion_cancels_downstream() {
        // A(retry, always fails) -> B
        // After A exhausts retries, B should be cancelled.
        let jobs = vec![
            make_job_with_strategy(
                "A",
                "rA",
                vec![],
                vec!["a.txt"],
                ErrorStrategy::Retry {
                    count: 2,
                    backoff: Backoff::Constant,
                },
            ),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "A".into(),
            1,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert!(result.failed > 0);

        let events = drain_events(&mut rx);
        // B should have been cancelled (never started).
        let cancelled = events
            .iter()
            .filter(|e| matches!(e, Event::JobCancelled { job_id, .. } if job_id.as_str() == "B"))
            .count();
        assert_eq!(cancelled, 1, "downstream job B should be cancelled");
    }

    #[tokio::test(start_paused = true)]
    async fn retry_with_keep_going_allows_independent_jobs() {
        // A(retry, fails) and C(independent) run in parallel.
        // A exhausts retries but C should still succeed.
        let jobs = vec![
            make_job_with_strategy(
                "A",
                "rA",
                vec![],
                vec!["a.txt"],
                ErrorStrategy::Retry {
                    count: 2,
                    backoff: Backoff::Constant,
                },
            ),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "rC", vec![], vec!["c.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "A".into(),
            1,
        )])));
        let config = SchedulerConfig {
            max_jobs: 4,
            keep_going: true,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        // A failed (exhausted), B cancelled, C succeeded.
        assert_eq!(result.failed, 1);
        assert_eq!(result.cancelled, 1);
        assert_eq!(result.succeeded, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_count_one_means_no_retries() {
        // count=1 means: attempt 1 → 1 < 1 = false → immediate exhaustion.
        // So the job fails on first attempt with no retry.
        let jobs = vec![make_job_with_strategy(
            "flaky",
            "build",
            vec![],
            vec!["out.txt"],
            ErrorStrategy::Retry {
                count: 1,
                backoff: Backoff::Constant,
            },
        )];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "flaky".into(),
            1,
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let ctx = default_ctx();

        let start = tokio::time::Instant::now();
        let _result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        // count=1: attempt 1, 1 < 1 = false → no retry, no backoff.
        assert!(
            elapsed < Duration::from_millis(500),
            "should have no backoff delay, got {:?}",
            elapsed
        );

        let events = drain_events(&mut rx);
        let fail_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Event::JobFailed { .. }))
            .collect();
        // Only 1 failure event (exhaustion, no retries).
        assert_eq!(fail_events.len(), 1);
        if let Event::JobFailed { error_message, .. } = &fail_events[0] {
            assert!(error_message.contains("exhausted 1 retries"));
        }

        assert_eq!(executor.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_multiple_independent_jobs_retry_independently() {
        // Two independent jobs both with retry strategies, both failing.
        // They should retry independently.
        let jobs = vec![
            make_job_with_strategy(
                "X",
                "rX",
                vec![],
                vec!["x.txt"],
                ErrorStrategy::Retry {
                    count: 2,
                    backoff: Backoff::Constant,
                },
            ),
            make_job_with_strategy(
                "Y",
                "rY",
                vec![],
                vec!["y.txt"],
                ErrorStrategy::Retry {
                    count: 2,
                    backoff: Backoff::Constant,
                },
            ),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([
            ("X".into(), 1),
            ("Y".into(), 1),
        ])));
        let config = SchedulerConfig {
            max_jobs: 4,
            keep_going: true,
            ..Default::default()
        };
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.failed, 2);

        let events = drain_events(&mut rx);
        let x_fails = events
            .iter()
            .filter(|e| matches!(e, Event::JobFailed { job_id, .. } if job_id.as_str() == "X"))
            .count();
        let y_fails = events
            .iter()
            .filter(|e| matches!(e, Event::JobFailed { job_id, .. } if job_id.as_str() == "Y"))
            .count();
        // Each job: 1 retry attempt + 1 exhaustion = 2 failure events.
        assert_eq!(x_fails, 2);
        assert_eq!(y_fails, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_success_then_downstream_runs() {
        // A(retry, fails once) -> B
        // A should retry and succeed, then B should run.
        let jobs = vec![
            make_job_with_strategy(
                "A",
                "rA",
                vec![],
                vec!["a.txt"],
                ErrorStrategy::Retry {
                    count: 3,
                    backoff: Backoff::Constant,
                },
            ),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        // A fails once, then succeeds. B always succeeds.
        let executor = Arc::new(RetryMockExecutor::new(BTreeMap::from([("A".into(), 1)])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.succeeded, 2); // Both A and B succeed.
        assert_eq!(result.failed, 0);
        assert_eq!(executor.total_attempts("A"), 2); // 1 fail + 1 success
        assert_eq!(executor.total_attempts("B"), 1);

        let events = drain_events(&mut rx);
        // B should have completed.
        let b_completed = events
            .iter()
            .any(|e| matches!(e, Event::JobCompleted { job_id, .. } if job_id.as_str() == "B"));
        assert!(
            b_completed,
            "downstream job B should complete after A's retry"
        );
    }

    // ---------------------------------------------------------------
    // Benchmark sink integration
    // ---------------------------------------------------------------

    /// A test sink that writes benchmark TSV to disk using the trait.
    struct FsTestSink;

    impl BenchmarkSink for FsTestSink {
        fn write_benchmark<'a>(
            &'a self,
            path: &'a std::path::Path,
            result: &'a JobResult,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                }
                let content = crate::traits::benchmark::format_benchmark_tsv(result);
                let _ = tokio::fs::write(path, content.as_bytes()).await;
            })
        }
    }

    #[tokio::test]
    async fn benchmark_tsv_written_on_success() {
        let bench_dir = std::env::temp_dir().join("oxymake_bench_test");
        let _ = std::fs::remove_dir_all(&bench_dir);

        let bench_path = bench_dir.join("timing.tsv");
        let mut job = make_job("bench-job", "r", vec![], vec![]);
        job.benchmark = Some(bench_path.display().to_string());

        let executor = Arc::new(MockExecutor::new());
        let bus = EventBus::new();

        let graph = JobGraph::build(vec![job]).unwrap();

        let config = SchedulerConfig::default();
        let ctx = default_ctx();

        let sink: Option<Arc<dyn BenchmarkSink>> = Some(Arc::new(FsTestSink));
        let result = run_scheduler_with_cache(
            &graph,
            executor.clone(),
            &config,
            &bus,
            &ctx,
            None,
            None,
            sink,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.succeeded, 1);

        // Verify the benchmark TSV was written.
        assert!(bench_path.exists(), "benchmark file should be created");

        let contents = std::fs::read_to_string(&bench_path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "should have header + 1 data row");
        assert_eq!(lines[0], "s\th:m:s\tmax_rss\tcpu_time");

        let fields: Vec<&str> = lines[1].split('\t').collect();
        assert_eq!(fields.len(), 4, "data row should have 4 columns");

        // Wall-clock seconds should be parseable as f64.
        let _wall_secs: f64 = fields[0].parse().expect("wall secs should be a number");

        // Clean up.
        let _ = std::fs::remove_dir_all(&bench_dir);
    }

    #[tokio::test]
    async fn benchmark_tsv_not_written_on_failure() {
        let bench_dir = std::env::temp_dir().join("oxymake_bench_fail_test");
        let _ = std::fs::remove_dir_all(&bench_dir);

        let bench_path = bench_dir.join("timing.tsv");
        let mut job = make_job("fail-bench", "r", vec![], vec![]);
        job.benchmark = Some(bench_path.display().to_string());

        let executor = Arc::new(MockExecutor::with_results(BTreeMap::from([(
            "fail-bench".into(),
            1,
        )])));
        let bus = EventBus::new();

        let graph = JobGraph::build(vec![job]).unwrap();

        let config = SchedulerConfig::default();
        let ctx = default_ctx();

        let sink: Option<Arc<dyn BenchmarkSink>> = Some(Arc::new(FsTestSink));
        let _ = run_scheduler_with_cache(
            &graph,
            executor.clone(),
            &config,
            &bus,
            &ctx,
            None,
            None,
            sink,
            None,
            None,
        )
        .await;

        // Benchmark should NOT be written for failed jobs.
        assert!(
            !bench_path.exists(),
            "benchmark file should not be created for failed jobs"
        );

        let _ = std::fs::remove_dir_all(&bench_dir);
    }

    // -- Gate-pending poll interval test (ox-hm7) ----------------------------

    /// A mock gate checker that stays Pending for a configured number of calls
    /// then returns Approved. Tracks the total number of `check_gate` calls.
    #[derive(Debug)]
    struct DelayedGateChecker {
        /// Number of check_gate calls that return Pending before switching to Approved.
        pending_count: AtomicUsize,
        /// Total check_gate calls observed.
        total_checks: AtomicUsize,
    }

    impl DelayedGateChecker {
        fn new(pending_for: usize) -> Self {
            Self {
                pending_count: AtomicUsize::new(pending_for),
                total_checks: AtomicUsize::new(0),
            }
        }
    }

    impl GateCheck for DelayedGateChecker {
        fn check_gate<'a>(
            &'a self,
            _gate_id: &'a GateId,
        ) -> Pin<Box<dyn Future<Output = GateStatus> + Send + 'a>> {
            Box::pin(async move {
                let call = self.total_checks.fetch_add(1, Ordering::SeqCst);
                let remaining = self.pending_count.load(Ordering::SeqCst);
                if call < remaining {
                    GateStatus::Pending
                } else {
                    GateStatus::Approved
                }
            })
        }

        fn register_gate<'a>(
            &'a self,
            _gate_id: &'a GateId,
            _run_id: Option<&'a str>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }
    }

    /// Regression test for ox-hm7: when gates are pending and no jobs are
    /// in-flight, the scheduler must sleep between poll iterations instead of
    /// busy-polling via `yield_now()`. With `start_paused = true`, tokio's
    /// auto-advance means each `sleep` instantly advances the clock. A busy-poll
    /// (yield_now) would complete in near-zero virtual time, while the fixed
    /// version advances at least `GATE_POLL_INTERVAL` per iteration.
    #[tokio::test(start_paused = true)]
    async fn gate_pending_does_not_busy_poll() {
        // Single job gated by a gate that stays Pending for 5 checks.
        let jobs = vec![make_job("gated", "build", vec![], vec!["out.txt"])];
        let mut graph = JobGraph::build(jobs).unwrap();
        let gate_id = GateId::from("approval-gate");
        graph.add_gate(&gate_id, &JobId::from("gated"));

        let gate_checker = Arc::new(DelayedGateChecker::new(5));
        let executor = Arc::new(MockExecutor::new());
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let start = tokio::time::Instant::now();
        let result = run_scheduler_with_cache(
            &graph,
            executor.clone(),
            &config,
            &bus,
            &ctx,
            None,
            Some(gate_checker.clone() as Arc<dyn GateCheck>),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let elapsed = start.elapsed();

        // The job should eventually succeed after the gate is approved.
        assert_eq!(result.succeeded, 1, "gated job should succeed");

        // With GATE_POLL_INTERVAL of 500ms and 5 pending checks, the scheduler
        // should have slept ~2500ms of virtual time. A busy-poll would complete
        // in near-zero time. We check for at least 2000ms to give margin.
        assert!(
            elapsed >= Duration::from_millis(2000),
            "scheduler should sleep between gate polls, but only {elapsed:?} elapsed \
             (busy-polling detected)"
        );
    }

    // -- Tests for failed_details in SchedulerResult --------------------------

    /// Mock executor that returns configurable stderr_tail on failure.
    #[derive(Debug)]
    struct StderrMockExecutor {
        /// Map of job_id -> (exit_code, stderr_tail).
        results: BTreeMap<String, (i32, Option<String>)>,
    }

    impl StderrMockExecutor {
        fn new(results: BTreeMap<String, (i32, Option<String>)>) -> Self {
            Self { results }
        }
    }

    impl Executor for StderrMockExecutor {
        type Error = MockError;

        async fn init(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn health_check(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn cleanup(&self) -> Result<(), MockError> {
            Ok(())
        }
        fn capabilities(&self) -> ExecutorCapabilities {
            ExecutorCapabilities::default()
        }
        fn max_concurrency(&self) -> Option<usize> {
            None
        }
        async fn prepare_workspace(
            &self,
            _job: &ConcreteJob,
            _ctx: &ExecContext,
        ) -> Result<Workspace, MockError> {
            Ok(Workspace::new(PathBuf::from("/tmp/mock")))
        }
        async fn execute(
            &self,
            job: &ConcreteJob,
            _workspace: &Workspace,
            _ctx: &ExecContext,
        ) -> Result<JobResult, MockError> {
            let (exit_code, stderr_tail) = self
                .results
                .get(job.id.as_str())
                .cloned()
                .unwrap_or((0, None));
            Ok(JobResult {
                job_id: job.id.clone(),
                exit_code,
                duration: Duration::from_millis(1),
                peak_memory_bytes: None,
                cpu_time: None,
                log_path: None,
                stderr_tail,
            })
        }
        async fn finalize_workspace(
            &self,
            _workspace: Workspace,
            _result: &JobResult,
        ) -> Result<(), MockError> {
            Ok(())
        }
        async fn cancel(&self, _job_id: &JobId) -> Result<(), MockError> {
            Ok(())
        }
        async fn poll_status(&self, _job_id: &JobId) -> Result<JobStatus, MockError> {
            Ok(JobStatus::Completed)
        }
        async fn submit_dag(
            &self,
            _graph: &crate::job_graph::JobGraph,
            _ctx: &ExecContext,
        ) -> Result<DagSubmission, MockError> {
            Err(MockError("DAG submission not supported by mock".into()))
        }
    }

    #[test]
    fn extract_last_nonempty_line_works() {
        assert_eq!(extract_last_nonempty_line(None), None);
        assert_eq!(extract_last_nonempty_line(Some("")), None);
        assert_eq!(extract_last_nonempty_line(Some("\n\n")), None);
        assert_eq!(
            extract_last_nonempty_line(Some("first\nsecond\n")),
            Some("second".into()),
        );
        assert_eq!(
            extract_last_nonempty_line(Some("  only line  ")),
            Some("only line".into()),
        );
        assert_eq!(
            extract_last_nonempty_line(Some("a\nb\n\n  \n")),
            Some("b".into()),
        );
    }

    #[tokio::test]
    async fn failed_details_populated_on_failure() {
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec![], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(StderrMockExecutor::new(BTreeMap::from([(
            "A".into(),
            (1, Some("line1\nIndexError: index out of bounds\n".into())),
        )])));
        let config = SchedulerConfig {
            keep_going: true,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor, &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.failed, 1);
        assert_eq!(result.failed_details.len(), 1);
        assert_eq!(result.failed_details[0].job_id.as_str(), "A");
        assert_eq!(
            result.failed_details[0].last_stderr_line.as_deref(),
            Some("IndexError: index out of bounds"),
        );
    }

    #[tokio::test]
    async fn failed_details_empty_when_no_failures() {
        let jobs = vec![make_job("A", "rA", vec![], vec!["a.txt"])];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::new());
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor, &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.failed, 0);
        assert!(result.failed_details.is_empty());
    }

    #[tokio::test]
    async fn failed_details_with_no_stderr() {
        let jobs = vec![make_job("A", "rA", vec![], vec!["a.txt"])];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(StderrMockExecutor::new(BTreeMap::from([(
            "A".into(),
            (1, None),
        )])));
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor, &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.failed, 1);
        assert_eq!(result.failed_details.len(), 1);
        assert!(result.failed_details[0].last_stderr_line.is_none());
    }

    // -- Root-cause detection unit tests ------------------------------------

    #[test]
    fn hash_line_deterministic() {
        let h1 = hash_line("FileNotFoundError: /data/input.csv");
        let h2 = hash_line("FileNotFoundError: /data/input.csv");
        let h3 = hash_line("PermissionError: /data/input.csv");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn check_root_cause_below_threshold() {
        let entries = vec![
            (hash_line("err"), JobId::from("j1")),
            (hash_line("err"), JobId::from("j2")),
        ];
        assert!(check_root_cause(&entries, 3).is_none());
    }

    #[test]
    fn check_root_cause_at_threshold() {
        let h = hash_line("err");
        let entries = vec![
            (h, JobId::from("j1")),
            (h, JobId::from("j2")),
            (h, JobId::from("j3")),
        ];
        let result = check_root_cause(&entries, 3);
        assert!(result.is_some());
        let ids = result.unwrap();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn check_root_cause_mixed_hashes() {
        let h1 = hash_line("err1");
        let h2 = hash_line("err2");
        let entries = vec![
            (h1, JobId::from("j1")),
            (h2, JobId::from("j2")),
            (h1, JobId::from("j3")),
        ];
        assert!(check_root_cause(&entries, 3).is_none());
    }

    #[test]
    fn check_root_cause_consecutive_at_tail() {
        let h1 = hash_line("err1");
        let h2 = hash_line("err2");
        let entries = vec![
            (h1, JobId::from("j1")),
            (h2, JobId::from("j2")),
            (h2, JobId::from("j3")),
            (h2, JobId::from("j4")),
        ];
        let result = check_root_cause(&entries, 3);
        assert!(result.is_some());
        let ids = result.unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], JobId::from("j2"));
    }

    #[test]
    fn check_root_cause_disabled() {
        let h = hash_line("err");
        let entries = vec![
            (h, JobId::from("j1")),
            (h, JobId::from("j2")),
            (h, JobId::from("j3")),
        ];
        assert!(check_root_cause(&entries, 0).is_none());
    }

    // -- Integration: root-cause detection with keep_going -------------------

    #[tokio::test]
    async fn root_cause_detected_with_keep_going() {
        // 3 independent jobs all fail with the same stderr signature.
        let same_error = "FileNotFoundError: /data/input.csv";
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec![], vec!["b.txt"]),
            make_job("C", "rC", vec![], vec!["c.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        let executor = Arc::new(StderrMockExecutor::new(BTreeMap::from([
            ("A".into(), (1, Some(same_error.into()))),
            ("B".into(), (1, Some(same_error.into()))),
            ("C".into(), (1, Some(same_error.into()))),
        ])));

        let config = SchedulerConfig {
            keep_going: true,
            root_cause_threshold: 3,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor, &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.failed, 3);
        assert!(result.root_cause.is_some());
        let rc = result.root_cause.unwrap();
        assert_eq!(rc.error_line, same_error);
        assert_eq!(rc.job_ids.len(), 3);
    }

    #[tokio::test]
    async fn no_root_cause_when_errors_differ() {
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec![], vec!["b.txt"]),
            make_job("C", "rC", vec![], vec!["c.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        let executor = Arc::new(StderrMockExecutor::new(BTreeMap::from([
            ("A".into(), (1, Some("error one".into()))),
            ("B".into(), (1, Some("error two".into()))),
            ("C".into(), (1, Some("error three".into()))),
        ])));

        let config = SchedulerConfig {
            keep_going: true,
            root_cause_threshold: 3,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor, &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.failed, 3);
        assert!(result.root_cause.is_none());
    }

    #[tokio::test]
    async fn root_cause_not_triggered_below_threshold() {
        // Only 2 jobs fail — below the default threshold of 3.
        let same_error = "FileNotFoundError: /data/input.csv";
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec![], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        let executor = Arc::new(StderrMockExecutor::new(BTreeMap::from([
            ("A".into(), (1, Some(same_error.into()))),
            ("B".into(), (1, Some(same_error.into()))),
        ])));

        let config = SchedulerConfig {
            keep_going: true,
            root_cause_threshold: 3,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor, &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.failed, 2);
        assert!(result.root_cause.is_none());
    }

    #[tokio::test]
    async fn run_started_reports_full_closure_with_cached_counts() {
        // 3 jobs: A -> B -> C. Mark A as cached via skip_jobs.
        // RunStarted should report total=3, to_run=2, cached=1.
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "rC", vec!["b.txt"], vec!["c.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::new());
        let config = SchedulerConfig {
            skip_jobs: HashSet::from([JobId::from("A")]),
            ..Default::default()
        };
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let ctx = default_ctx();

        let _result = run_scheduler(&graph, executor, &config, &bus, &ctx)
            .await
            .unwrap();

        // Find the RunStarted event and verify counts.
        let mut found = false;
        while let Ok(event) = rx.try_recv() {
            if let Event::RunStarted {
                total_jobs,
                to_run,
                cached,
            } = event
            {
                assert_eq!(total_jobs, 3, "total_jobs should be the full closure");
                assert_eq!(to_run, 2, "to_run should exclude cached jobs");
                assert_eq!(cached, 1, "cached should count skip_jobs");
                found = true;
            }
        }
        assert!(found, "RunStarted event should have been emitted");
    }
    // ---------------------------------------------------------------
    // Graceful shutdown (SIGINT contract)
    // ---------------------------------------------------------------

    /// Mock executor that blocks in execute() until cancelled, tracking
    /// which jobs were cancelled.
    #[derive(Debug)]
    struct ShutdownMockExecutor {
        /// Jobs that were cancelled via cancel().
        cancelled: Arc<Mutex<Vec<String>>>,
        /// Notified when execute() starts (test uses this to time the shutdown).
        started: Arc<Notify>,
        /// Notified when cancel() is called — unblocks execute().
        cancel_signal: Arc<Notify>,
    }

    impl ShutdownMockExecutor {
        fn new() -> Self {
            Self {
                cancelled: Arc::new(Mutex::new(Vec::new())),
                started: Arc::new(Notify::new()),
                cancel_signal: Arc::new(Notify::new()),
            }
        }
    }

    impl Executor for ShutdownMockExecutor {
        type Error = MockError;

        async fn init(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn health_check(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn cleanup(&self) -> Result<(), MockError> {
            Ok(())
        }
        fn capabilities(&self) -> ExecutorCapabilities {
            ExecutorCapabilities::default()
        }
        fn max_concurrency(&self) -> Option<usize> {
            Some(4)
        }
        async fn prepare_workspace(
            &self,
            _job: &ConcreteJob,
            _ctx: &ExecContext,
        ) -> Result<Workspace, MockError> {
            Ok(Workspace::new(PathBuf::from("/tmp/mock")))
        }
        async fn execute(
            &self,
            job: &ConcreteJob,
            _workspace: &Workspace,
            _ctx: &ExecContext,
        ) -> Result<JobResult, MockError> {
            self.started.notify_one();
            // Block until cancel() is called (simulates a long-running child
            // that exits on SIGTERM).
            self.cancel_signal.notified().await;
            Ok(JobResult {
                job_id: job.id.clone(),
                exit_code: 143, // SIGTERM exit code
                duration: Duration::from_millis(1),
                peak_memory_bytes: None,
                cpu_time: None,
                log_path: None,
                stderr_tail: None,
            })
        }
        async fn finalize_workspace(
            &self,
            _workspace: Workspace,
            _result: &JobResult,
        ) -> Result<(), MockError> {
            Ok(())
        }
        async fn cancel(&self, job_id: &JobId) -> Result<(), MockError> {
            self.cancelled.lock().await.push(job_id.to_string());
            self.cancel_signal.notify_waiters();
            Ok(())
        }
        async fn poll_status(&self, _job_id: &JobId) -> Result<JobStatus, MockError> {
            Ok(JobStatus::Completed)
        }
        async fn submit_dag(
            &self,
            _graph: &crate::job_graph::JobGraph,
            _ctx: &ExecContext,
        ) -> Result<DagSubmission, MockError> {
            Err(MockError("DAG submission not supported by mock".into()))
        }
    }

    #[tokio::test]
    async fn shutdown_signal_cancels_in_flight_jobs() {
        // Set up a single job that will block in execute().
        let job = make_job("slow", "r_slow", vec![], vec!["out.csv"]);
        let graph = JobGraph::build(vec![job]).unwrap();

        let executor = Arc::new(ShutdownMockExecutor::new());
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let shutdown = Arc::new(Notify::new());

        // Wait for the job to start, then fire the shutdown signal.
        let started = executor.started.clone();
        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            started.notified().await;
            // Small delay to ensure the scheduler is in the select! loop.
            tokio::time::sleep(Duration::from_millis(50)).await;
            shutdown_clone.notify_waiters();
        });

        let result = run_scheduler_with_cache(
            &graph,
            executor.clone(),
            &config,
            &bus,
            &ctx,
            None,
            None,
            None,
            Some(shutdown),
            None,
        )
        .await;

        // The scheduler should have cancelled the in-flight job.
        let cancelled = executor.cancelled.lock().await;
        assert!(
            cancelled.contains(&"slow".to_string()),
            "shutdown should cancel in-flight job 'slow', got: {cancelled:?}"
        );

        // The result should indicate the job was cancelled/failed, not succeeded.
        match result {
            Ok(r) => assert_eq!(
                r.succeeded, 0,
                "interrupted job should not count as succeeded"
            ),
            Err(_) => {} // Also acceptable — scheduler may return error on shutdown.
        }
    }

    #[tokio::test]
    async fn shutdown_prevents_new_dispatches() {
        // Job A depends on nothing, job B depends on A's output.
        // A will block (mock sleeps 60s), so B never becomes ready.
        // After shutdown, B should be cancelled without ever being dispatched.
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        let executor = Arc::new(ShutdownMockExecutor::new());
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();

        let shutdown = Arc::new(Notify::new());

        // Fire shutdown as soon as A starts executing.
        let started = executor.started.clone();
        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            started.notified().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            shutdown_clone.notify_waiters();
        });

        let result = run_scheduler_with_cache(
            &graph,
            executor.clone(),
            &config,
            &bus,
            &ctx,
            None,
            None,
            None,
            Some(shutdown),
            None,
        )
        .await;

        // Only A should have been executor-cancelled (B was never dispatched).
        let cancelled = executor.cancelled.lock().await;
        assert_eq!(
            cancelled.len(),
            1,
            "only 1 job (A) should be executor-cancelled, got: {cancelled:?}"
        );
        assert_eq!(cancelled[0], "A");

        // The result: A failed/cancelled, B cancelled by cancel_remaining.
        if let Ok(r) = result {
            assert_eq!(r.succeeded, 0, "no job should succeed after interrupt");
            assert!(
                r.cancelled > 0,
                "B should be cancelled (never dispatched), got: cancelled={}",
                r.cancelled,
            );
        }
    }

    #[tokio::test]
    async fn find_ready_jobs_snapshots_pending_ids() {
        // A -> B (B depends on A's output)
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let job_ids: Vec<&JobId> = graph.job_ids();
        let state = Arc::new(Mutex::new(Frontier::new(
            &job_ids,
            &graph,
            0,
            None,
            ResourceBudget::new(BTreeMap::new()),
        )));
        let bus = EventBus::new();

        // Initially both are Pending; A has no upstream so it should be ready.
        let ready = find_ready_jobs(&state, &graph, &None, &bus).await;
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].as_str(), "A");

        // Mark A as Succeeded — promote downstream into the frontier.
        {
            let mut s = state.lock().await;
            s.set_status(JobId::from("A"), JobLifecycle::Succeeded);
            s.promote_downstream(&JobId::from("A"), &graph);
        }
        let ready = find_ready_jobs(&state, &graph, &None, &bus).await;
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].as_str(), "B");

        // No more pending jobs — should return empty.
        let ready = find_ready_jobs(&state, &graph, &None, &bus).await;
        assert!(ready.is_empty());
    }

    // -- Concurrency-tracking executor for regression test -------------------

    /// Executor that tracks peak concurrency via atomic counters.
    #[derive(Debug)]
    struct ConcurrencyTrackingExecutor {
        current: AtomicUsize,
        peak: AtomicUsize,
        delay: Duration,
    }

    impl ConcurrencyTrackingExecutor {
        fn new(delay: Duration) -> Self {
            Self {
                current: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                delay,
            }
        }

        fn peak_concurrency(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }
    }

    impl Executor for ConcurrencyTrackingExecutor {
        type Error = MockError;

        async fn init(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn health_check(&self) -> Result<(), MockError> {
            Ok(())
        }
        async fn cleanup(&self) -> Result<(), MockError> {
            Ok(())
        }
        fn capabilities(&self) -> ExecutorCapabilities {
            ExecutorCapabilities::default()
        }
        fn max_concurrency(&self) -> Option<usize> {
            None
        }
        async fn prepare_workspace(
            &self,
            _job: &ConcreteJob,
            _ctx: &ExecContext,
        ) -> Result<Workspace, MockError> {
            Ok(Workspace::new(PathBuf::from("/tmp/mock")))
        }
        async fn execute(
            &self,
            job: &ConcreteJob,
            _workspace: &Workspace,
            _ctx: &ExecContext,
        ) -> Result<JobResult, MockError> {
            let prev = self.current.fetch_add(1, Ordering::SeqCst);
            let running = prev + 1;
            // Update peak if this is a new high-water mark.
            self.peak.fetch_max(running, Ordering::SeqCst);

            tokio::time::sleep(self.delay).await;

            self.current.fetch_sub(1, Ordering::SeqCst);

            Ok(JobResult {
                job_id: job.id.clone(),
                exit_code: 0,
                duration: self.delay,
                peak_memory_bytes: None,
                cpu_time: None,
                log_path: None,
                stderr_tail: None,
            })
        }
        async fn finalize_workspace(
            &self,
            _workspace: Workspace,
            _result: &JobResult,
        ) -> Result<(), MockError> {
            Ok(())
        }
        async fn cancel(&self, _job_id: &JobId) -> Result<(), MockError> {
            Ok(())
        }
        async fn poll_status(&self, _job_id: &JobId) -> Result<JobStatus, MockError> {
            Ok(JobStatus::Completed)
        }
        async fn submit_dag(
            &self,
            _graph: &crate::job_graph::JobGraph,
            _ctx: &ExecContext,
        ) -> Result<DagSubmission, MockError> {
            Err(MockError("DAG submission not supported by mock".into()))
        }
    }

    /// Regression test for ox-1aoz: scheduler must respect max_jobs concurrency
    /// limit. With 10 independent jobs and max_jobs=2, at most 2 should execute
    /// concurrently.
    #[tokio::test(start_paused = true)]
    async fn concurrency_limit_respected() {
        // 10 independent jobs — all ready immediately.
        let jobs: Vec<ConcreteJob> = (0..10)
            .map(|i| {
                make_job(
                    &format!("j{i}"),
                    "rule",
                    vec![],
                    vec![&format!("out{i}.txt")],
                )
            })
            .collect();
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(ConcurrencyTrackingExecutor::new(Duration::from_millis(100)));
        let config = SchedulerConfig {
            max_jobs: 2,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.succeeded, 10);
        assert!(
            executor.peak_concurrency() <= 2,
            "peak concurrency was {} but max_jobs is 2",
            executor.peak_concurrency(),
        );
    }

    /// With max_jobs=1 (the default), jobs must run sequentially.
    #[tokio::test(start_paused = true)]
    async fn concurrency_limit_one_is_sequential() {
        let jobs: Vec<ConcreteJob> = (0..5)
            .map(|i| {
                make_job(
                    &format!("j{i}"),
                    "rule",
                    vec![],
                    vec![&format!("out{i}.txt")],
                )
            })
            .collect();
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(ConcurrencyTrackingExecutor::new(Duration::from_millis(50)));
        let config = SchedulerConfig {
            max_jobs: 1,
            ..Default::default()
        };
        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.succeeded, 5);
        assert_eq!(
            executor.peak_concurrency(),
            1,
            "with max_jobs=1, peak concurrency must be exactly 1"
        );
    }

    // ---------------------------------------------------------------
    // Cache fast-path: fully-cached DAG must not sleep per layer
    // ---------------------------------------------------------------

    /// A mock cache that reports all jobs as cached.
    struct AllCachedMock;

    impl CacheCheck for AllCachedMock {
        fn is_cached<'a>(
            &'a self,
            _job: &'a ConcreteJob,
        ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
            Box::pin(async { true })
        }

        fn record<'a>(
            &'a self,
            _job: &'a ConcreteJob,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn fully_cached_dag_completes_without_sleep_penalty() {
        // Regression test for ox-wq9t: a fully-cached 5-layer chain should
        // complete in well under 500ms (the GATE_POLL_INTERVAL). Before the
        // fix, each layer incurred a 500ms sleep, making this take ~2.5s.
        let jobs = vec![
            make_job("A", "r", vec![], vec!["a.txt"]),
            make_job("B", "r", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "r", vec!["b.txt"], vec!["c.txt"]),
            make_job("D", "r", vec!["c.txt"], vec!["d.txt"]),
            make_job("E", "r", vec!["d.txt"], vec!["e.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::new());
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();
        let cache: Option<Arc<dyn CacheCheck>> = Some(Arc::new(AllCachedMock));

        let start = Instant::now();
        let result = run_scheduler_with_cache(
            &graph, executor, &config, &bus, &ctx, cache, None, None, None, None,
        )
        .await
        .unwrap();
        let elapsed = start.elapsed();

        assert_eq!(result.skipped, 5, "all 5 jobs should be skipped (cached)");
        assert_eq!(result.succeeded, 0, "no jobs should have executed");
        assert!(
            elapsed < Duration::from_millis(200),
            "fully-cached DAG took {:?}, expected <200ms (was sleeping per layer before fix)",
            elapsed
        );
    }

    // -- Memory budget + Belady eviction tests --------------------------------

    #[test]
    fn memory_budget_unlimited_no_eviction() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ])
        .unwrap();
        let topo = graph.topological_order().unwrap();
        let mut state = Frontier::new(&topo, &graph, 0, None, ResourceBudget::new(BTreeMap::new())); // unlimited

        // Register in-memory data.
        state.register_in_memory("a.txt", 1_000_000);
        state.register_in_memory("b.txt", 2_000_000);

        assert_eq!(state.memory_used_bytes(), 3_000_000);

        // Unlimited budget — no eviction.
        let evicted = state.enforce_memory_budget();
        assert_eq!(evicted, 0);
        assert_eq!(state.memory_used_bytes(), 3_000_000);
    }

    #[test]
    fn memory_budget_evicts_largest_first() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
            make_test_job("C", &["a.txt"], &["c.txt"]),
        ])
        .unwrap();
        let topo = graph.topological_order().unwrap();
        // Budget of 500 bytes.
        let mut state = Frontier::new(
            &topo,
            &graph,
            500,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        // Register in-memory data for outputs.
        state.register_in_memory("a.txt", 300);
        state.register_in_memory("b.txt", 200);
        state.register_in_memory("c.txt", 400);

        assert_eq!(state.memory_used_bytes(), 900);

        // Fire all consumers of "c.txt" (so it becomes evictable).
        if let Some(ms) = state.output_mats.get_mut("c.txt") {
            while ms.pending_consumers() > 0 {
                ms.consumer_fired();
            }
        }
        // Fire all consumers of "b.txt".
        if let Some(ms) = state.output_mats.get_mut("b.txt") {
            while ms.pending_consumers() > 0 {
                ms.consumer_fired();
            }
        }

        // Enforce budget (500). Currently at 900.
        // c.txt (400) is largest evictable → evict first (900 → 500).
        let evicted = state.enforce_memory_budget();
        assert_eq!(evicted, 1);
        assert_eq!(state.memory_used_bytes(), 500);

        // c.txt should no longer have in-memory.
        assert!(!state.output_mats["c.txt"].has_in_memory());
        // b.txt still has in-memory (not evicted, within budget).
        assert!(state.output_mats["b.txt"].has_in_memory());
    }

    #[test]
    fn memory_budget_respects_eviction_guard() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ])
        .unwrap();
        let topo = graph.topological_order().unwrap();
        let mut state = Frontier::new(
            &topo,
            &graph,
            100,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        // Register in-memory (a.txt still has pending consumers).
        state.register_in_memory("a.txt", 500);
        assert_eq!(state.memory_used_bytes(), 500);

        // a.txt has pending_consumers > 0 and only one materialization
        // (InMemory) — eviction guard should prevent removal.
        // But we also need to check: it has no other materialization,
        // so the guard kicks in only if it's the last one. Let's verify.
        let evicted = state.enforce_memory_budget();
        // a.txt is NOT evictable because pending_consumers > 0.
        assert_eq!(evicted, 0);
        assert_eq!(state.memory_used_bytes(), 500);
    }

    #[test]
    fn memory_budget_evicts_multiple_to_fit() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &[], &["b.txt"]),
            make_test_job("C", &[], &["c.txt"]),
        ])
        .unwrap();
        let topo = graph.topological_order().unwrap();
        // Budget of 100 bytes.
        let mut state = Frontier::new(
            &topo,
            &graph,
            100,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        state.register_in_memory("a.txt", 200);
        state.register_in_memory("b.txt", 300);
        state.register_in_memory("c.txt", 150);

        // All are leaf outputs with no consumers — immediately evictable.
        assert_eq!(state.memory_used_bytes(), 650);

        let evicted = state.enforce_memory_budget();
        // Should evict b.txt (300) first → 350, still over.
        // Then c.txt (200 was a.txt, 150 was c.txt) → evict a.txt (200) → 150, still over.
        // Then c.txt (150) → 0. Total 3 evictions.
        // Actually: 650 - 300 = 350 > 100, evict a.txt (200) → 150, still > 100,
        // evict c.txt (150) → 0.
        assert!(evicted >= 2);
        assert!(state.memory_used_bytes() <= 100);
    }

    #[test]
    fn register_in_memory_tracks_size() {
        let graph = JobGraph::build(vec![make_test_job("A", &[], &["a.txt"])]).unwrap();
        let topo = graph.topological_order().unwrap();
        let mut state = Frontier::new(&topo, &graph, 0, None, ResourceBudget::new(BTreeMap::new()));

        state.register_in_memory("a.txt", 42);

        assert_eq!(state.memory_used_bytes(), 42);
        assert!(state.output_mats["a.txt"].has_in_memory());
        assert_eq!(state.output_mats["a.txt"].size_bytes(), 42);
    }

    #[test]
    fn cheapest_materialization_prefers_memory() {
        let graph = JobGraph::build(vec![make_test_job("A", &[], &["a.txt"])]).unwrap();
        let topo = graph.topological_order().unwrap();
        let mut state = Frontier::new(&topo, &graph, 0, None, ResourceBudget::new(BTreeMap::new()));

        // Add disk materialization first.
        if let Some(ms) = state.output_mats.get_mut("a.txt") {
            ms.add(Materialization::OnDisk {
                path: PathBuf::from("a.txt"),
                verified: false,
            });
        }
        let cheapest = state.cheapest_materialization("a.txt").unwrap();
        assert!(matches!(cheapest, Materialization::OnDisk { .. }));

        // Add in-memory — should now be cheapest.
        state.register_in_memory("a.txt", 100);
        let cheapest = state.cheapest_materialization("a.txt").unwrap();
        assert!(matches!(cheapest, Materialization::InMemory { .. }));
    }

    #[test]
    fn evictable_outputs_tracks_consumer_state() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ])
        .unwrap();
        let topo = graph.topological_order().unwrap();
        let mut state = Frontier::new(&topo, &graph, 0, None, ResourceBudget::new(BTreeMap::new()));

        state.register_in_memory("a.txt", 100);

        // a.txt has 1 pending consumer (B) — not evictable.
        let evictable = state.evictable_outputs();
        assert!(!evictable.contains(&"a.txt".to_string()));

        // Simulate B consuming a.txt.
        if let Some(ms) = state.output_mats.get_mut("a.txt") {
            ms.consumer_fired();
        }

        // Now a.txt is evictable.
        let evictable = state.evictable_outputs();
        assert!(evictable.contains(&"a.txt".to_string()));
    }

    #[test]
    fn memory_store_populated_on_register() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ])
        .unwrap();
        let topo = graph.topological_order().unwrap();
        let mut state = Frontier::new(&topo, &graph, 0, None, ResourceBudget::new(BTreeMap::new()));

        // Manually insert data into memory_store (simulating what
        // register_materializations does when a file exists).
        let data: Arc<[u8]> = Arc::from(b"hello world" as &[u8]);
        state.memory_store.insert("a.txt".into(), data.clone());
        state.register_in_memory("a.txt", 11);

        assert!(state.memory_store.contains_key("a.txt"));
        assert_eq!(state.memory_store["a.txt"].len(), 11);
    }

    #[test]
    fn collect_input_data_returns_in_memory_inputs() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ])
        .unwrap();
        let topo = graph.topological_order().unwrap();
        let mut state = Frontier::new(&topo, &graph, 0, None, ResourceBudget::new(BTreeMap::new()));

        // Register in-memory materialization and store data.
        let data: Arc<[u8]> = Arc::from(b"test data" as &[u8]);
        state.memory_store.insert("a.txt".into(), data);
        state.register_in_memory("a.txt", 9);

        // Collect input data for job B (which consumes a.txt).
        let job_b = graph.get_job(&JobId::from("B")).unwrap();
        let input_data = state.collect_input_data(job_b);

        assert_eq!(input_data.len(), 1);
        assert_eq!(input_data["a.txt"].as_ref(), b"test data");
    }

    #[test]
    fn collect_input_data_empty_when_no_memory() {
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["a.txt"]),
            make_test_job("B", &["a.txt"], &["b.txt"]),
        ])
        .unwrap();
        let topo = graph.topological_order().unwrap();
        let state = Frontier::new(&topo, &graph, 0, None, ResourceBudget::new(BTreeMap::new()));

        // No in-memory materializations registered.
        let job_b = graph.get_job(&JobId::from("B")).unwrap();
        let input_data = state.collect_input_data(job_b);
        assert!(input_data.is_empty());
    }

    #[test]
    fn memory_store_cleaned_on_eviction() {
        let graph = JobGraph::build(vec![make_test_job("A", &[], &["a.txt"])]).unwrap();
        let topo = graph.topological_order().unwrap();
        let mut state = Frontier::new(
            &topo,
            &graph,
            10,
            None,
            ResourceBudget::new(BTreeMap::new()),
        ); // budget: 10 bytes

        // Register data larger than budget.
        let data: Arc<[u8]> = Arc::from(b"large data here!!" as &[u8]);
        state.memory_store.insert("a.txt".into(), data);
        state.register_in_memory("a.txt", 17);

        assert!(state.memory_store.contains_key("a.txt"));
        assert_eq!(state.memory_used_bytes(), 17);

        // a.txt is a leaf with no consumers — immediately evictable.
        let evicted = state.enforce_memory_budget();
        assert_eq!(evicted, 1);
        assert!(!state.memory_store.contains_key("a.txt"));
        assert_eq!(state.memory_used_bytes(), 0);
    }

    // -- Stage 2: Memory map tests ------------------------------------------

    #[tokio::test]
    async fn memory_map_populated_on_job_success() {
        // A→B pipeline: A produces out.txt, B reads it.
        let jobs = vec![
            make_job("a", "produce", vec![], vec!["out.txt"]),
            make_job("b", "consume", vec!["out.txt"], vec!["final.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let executor = Arc::new(MockExecutor::new());
        let config = SchedulerConfig::default();
        let bus = EventBus::new();

        // Create a ctx WITH a memory map.
        let mem_map = crate::memory_map::OutputMemoryMap::new();
        let ctx = ExecContext {
            memory_map: Some(mem_map.clone()),
            ..default_ctx()
        };

        let result = run_scheduler(&graph, executor.clone(), &config, &bus, &ctx)
            .await
            .unwrap();
        assert_eq!(result.succeeded, 2);

        // NOTE: This test verifies the scheduler runs to completion with a
        // memory_map configured. The map population (spawned task reads from
        // disk) does not fire because MockExecutor doesn't create real files.
        // The actual data-flow path is tested by the unit tests below:
        // - register_materializations_drains_from_memory_map
        // - register_materializations_computes_artifact_meta
        // For end-to-end testing with real files, see stage2-lab-test-plan.md.
        assert!(ctx.memory_map.is_some());
        assert_eq!(
            mem_map.len(),
            0,
            "mock executor produces no files to populate"
        );
    }

    #[test]
    fn register_materializations_adds_in_memory_when_map_present() {
        use crate::model::MaterializePolicy;

        let mut jobs = vec![make_job("j1", "rule", vec![], vec!["out.parquet"])];
        // Use Auto policy so the output is eligible for in-memory promotion.
        jobs[0].outputs[0].materialize = MaterializePolicy::Auto;

        let graph = JobGraph::build(jobs.clone()).unwrap();
        let job_ids = graph.job_ids();
        let mut state = Frontier::new(
            &job_ids,
            &graph,
            1024, // memory budget active
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        // Set up memory map with the output data.
        let mem_map = crate::memory_map::OutputMemoryMap::new();
        mem_map.put("out.parquet".into(), Arc::from(vec![1u8, 2, 3, 4]));

        // Register with on_critical_path=true and budget > 0.
        let _disk_writes = state.register_materializations(&jobs[0], true, Some(&mem_map));

        // The output should have both OnDisk and InMemory materializations.
        let key = output_ref_key(&OutputRef::File(PathBuf::from("out.parquet")));
        let mat_set = state.output_mats.get(&key).unwrap();
        assert!(mat_set.is_available());
        assert_eq!(mat_set.len(), 2, "should have OnDisk + InMemory");

        // Cheapest should be InMemory (cost 100µs vs 200ms for disk).
        let cheapest = mat_set.cheapest().unwrap();
        assert!(
            matches!(cheapest, Materialization::InMemory { .. }),
            "cheapest should be InMemory, got {cheapest:?}"
        );
    }

    #[test]
    fn register_materializations_disk_only_without_map() {
        let jobs = vec![make_job("j1", "rule", vec![], vec!["out.parquet"])];
        let graph = JobGraph::build(jobs.clone()).unwrap();

        let job_ids = graph.job_ids();
        let mut state = Frontier::new(
            &job_ids,
            &graph,
            0,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        // Register without memory map — should only have OnDisk.
        let _disk_writes = state.register_materializations(&jobs[0], false, None);

        let key = output_ref_key(&OutputRef::File(PathBuf::from("out.parquet")));
        let mat_set = state.output_mats.get(&key).unwrap();
        assert_eq!(mat_set.len(), 1, "should only have OnDisk");
        assert!(matches!(
            mat_set.cheapest().unwrap(),
            Materialization::OnDisk { .. }
        ));
    }

    #[test]
    fn register_materializations_drains_from_memory_map() {
        // Verify that register_materializations drains data from
        // OutputMemoryMap into memory_store (Stage 2 optimization:
        // no redundant disk read).
        use crate::model::MaterializePolicy;

        let mut jobs = vec![make_job("j1", "rule", vec![], vec!["out.bin"])];
        // Use Auto policy (eligible for in-memory promotion).
        // Always policy forces disk-only; Auto allows memory promotion.
        jobs[0].outputs[0].materialize = MaterializePolicy::Auto;

        let graph = JobGraph::build(jobs.clone()).unwrap();
        let job_ids = graph.job_ids();
        let mut state = Frontier::new(
            &job_ids,
            &graph,
            1024, // memory budget active
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        let mem_map = crate::memory_map::OutputMemoryMap::new();
        let test_data = vec![10, 20, 30, 40, 50];
        mem_map.put("out.bin".into(), Arc::from(test_data.clone()));

        let disk_writes = state.register_materializations(&jobs[0], true, Some(&mem_map));

        // Data should be drained from OutputMemoryMap.
        assert!(
            mem_map.get("out.bin").is_none(),
            "data should be drained from map"
        );

        // Data should be in memory_store.
        let stored = state.memory_store.get("out.bin").unwrap();
        assert_eq!(stored.as_ref(), &test_data[..]);

        // Memory usage should be tracked.
        assert_eq!(state.memory_used_bytes, test_data.len() as u64);

        // No disk write requests without a disk writer.
        assert!(disk_writes.is_empty());
    }

    #[test]
    fn register_materializations_never_without_data_no_ghost() {
        // Verify that MaterializePolicy::Never does NOT register a ghost
        // InMemory materialization when no data is available (no memory_map,
        // file doesn't exist on disk). A ghost would cause collect_input_data
        // to return nothing, silently breaking downstream jobs.
        use crate::model::MaterializePolicy;

        let mut jobs = vec![make_job("j1", "rule", vec![], vec!["never.bin"])];
        jobs[0].outputs[0].materialize = MaterializePolicy::Never;

        let graph = JobGraph::build(jobs.clone()).unwrap();
        let job_ids = graph.job_ids();
        let mut state = Frontier::new(
            &job_ids,
            &graph,
            0,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        let _disk_writes = state.register_materializations(&jobs[0], false, None);

        let key = output_ref_key(&OutputRef::File(PathBuf::from("never.bin")));
        let mat_set = state.output_mats.get(&key).unwrap();
        // Without data, InMemory should NOT be registered (no ghost).
        // The output still gets OnDisk (for the file path reference).
        assert!(
            !mat_set.has_in_memory(),
            "Never without data should not create ghost InMemory"
        );
    }

    #[test]
    fn register_materializations_never_with_memory_map_promotes() {
        // Verify that MaterializePolicy::Never DOES promote to InMemory
        // when data is available in the memory map. This is the intended
        // behavior — Never means "keep in memory, don't persist to disk".
        use crate::model::MaterializePolicy;

        let mut jobs = vec![make_job("j1", "rule", vec![], vec!["never.bin"])];
        jobs[0].outputs[0].materialize = MaterializePolicy::Never;

        let graph = JobGraph::build(jobs.clone()).unwrap();
        let job_ids = graph.job_ids();
        let mut state = Frontier::new(
            &job_ids,
            &graph,
            0,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        let mem_map = crate::memory_map::OutputMemoryMap::new();
        mem_map.put("never.bin".into(), Arc::from(vec![1u8, 2, 3]));

        let _disk_writes = state.register_materializations(&jobs[0], false, Some(&mem_map));

        let key = output_ref_key(&OutputRef::File(PathBuf::from("never.bin")));
        let mat_set = state.output_mats.get(&key).unwrap();
        assert!(
            mat_set.has_in_memory(),
            "Never with data should promote to InMemory"
        );
        assert!(state.memory_store.contains_key("never.bin"));
    }

    #[test]
    fn never_policy_no_ondisk_materialization() {
        // Never-policy outputs should NOT get OnDisk materialization.
        // Adding OnDisk would cause cheapest() to fall back to disk
        // after eviction, reading a stale or missing file.
        use crate::model::MaterializePolicy;

        let mut jobs = vec![make_job("j1", "rule", vec![], vec!["never.bin"])];
        jobs[0].outputs[0].materialize = MaterializePolicy::Never;

        let graph = JobGraph::build(jobs.clone()).unwrap();
        let job_ids = graph.job_ids();
        let mut state = Frontier::new(
            &job_ids,
            &graph,
            1024,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        let mem_map = crate::memory_map::OutputMemoryMap::new();
        mem_map.put("never.bin".into(), Arc::from(vec![1u8, 2, 3]));

        let _ = state.register_materializations(&jobs[0], true, Some(&mem_map));

        let key = output_ref_key(&OutputRef::File(PathBuf::from("never.bin")));
        let mat_set = state.output_mats.get(&key).unwrap();

        assert!(mat_set.has_in_memory(), "Never should have InMemory");
        assert!(!mat_set.has_disk_fallback(), "Never should NOT have OnDisk");
    }

    #[test]
    fn eviction_skips_outputs_without_disk_fallback() {
        // Outputs with no disk fallback (e.g., Never-policy) should
        // never be evicted, even when the memory budget is exceeded.
        // Evicting them would destroy the only copy of the data.
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["mem_only.bin"]),
            make_test_job("B", &[], &["has_disk.bin"]),
        ])
        .unwrap();
        let topo = graph.topological_order().unwrap();
        let mut state = Frontier::new(
            &topo,
            &graph,
            100, // tiny budget to force eviction
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        // mem_only.bin: InMemory ONLY (no disk fallback, like Never policy)
        let data_a: Arc<[u8]> = Arc::from(vec![0u8; 80]);
        state.memory_store.insert("mem_only.bin".into(), data_a);
        if let Some(ms) = state.output_mats.get_mut("mem_only.bin") {
            ms.add(Materialization::InMemory { pinned: false });
            ms.set_artifact_meta(ArtifactMeta::new([0u8; 32], 80));
            ms.consumer_fired(); // make evictable
        }
        state.memory_used_bytes += 80;

        // has_disk.bin: InMemory + OnDisk (has disk fallback)
        let data_b: Arc<[u8]> = Arc::from(vec![1u8; 80]);
        state.memory_store.insert("has_disk.bin".into(), data_b);
        if let Some(ms) = state.output_mats.get_mut("has_disk.bin") {
            ms.add(Materialization::InMemory { pinned: false });
            ms.add(Materialization::OnDisk {
                path: PathBuf::from("has_disk.bin"),
                verified: false,
            });
            ms.set_artifact_meta(ArtifactMeta::new([1u8; 32], 80));
            ms.consumer_fired(); // make evictable
        }
        state.memory_used_bytes += 80;

        // Total: 160 bytes, budget: 100. Must evict.
        let evicted = state.enforce_memory_budget();
        assert_eq!(evicted, 1, "should evict exactly one output");

        // has_disk.bin should be evicted (has disk fallback).
        assert!(
            !state.memory_store.contains_key("has_disk.bin"),
            "output with disk fallback should be evicted"
        );
        // mem_only.bin should survive (no disk fallback).
        assert!(
            state.memory_store.contains_key("mem_only.bin"),
            "output without disk fallback must survive eviction"
        );
    }

    #[tokio::test]
    async fn register_materializations_returns_disk_write_requests() {
        use crate::model::MaterializePolicy;

        // Verify that disk write requests are returned when a DiskWriter
        // is configured and the output is promoted to memory.
        let mut jobs = vec![make_job("j1", "rule", vec![], vec!["out.bin"])];
        jobs[0].outputs[0].materialize = MaterializePolicy::Auto;

        let graph = JobGraph::build(jobs.clone()).unwrap();
        let job_ids = graph.job_ids();

        let (handle, _join) = crate::disk_writer::spawn_disk_writer(16);
        let mut state = Frontier::new(
            &job_ids,
            &graph,
            1024,
            Some(handle),
            ResourceBudget::new(BTreeMap::new()),
        );

        let mem_map = crate::memory_map::OutputMemoryMap::new();
        mem_map.put("out.bin".into(), Arc::from(vec![1u8, 2, 3]));

        let disk_writes = state.register_materializations(&jobs[0], true, Some(&mem_map));

        assert_eq!(disk_writes.len(), 1, "should have 1 disk write request");
        assert_eq!(disk_writes[0].target_path, PathBuf::from("out.bin"));
        assert_eq!(disk_writes[0].data.as_ref(), &[1, 2, 3]);
    }

    #[test]
    fn register_materializations_tracks_size_without_hashing() {
        // Verify that register_materializations tracks size_bytes for
        // eviction ordering WITHOUT computing BLAKE3 — hashing is
        // expensive on large outputs and unnecessary for the memory
        // data flow. The cache provenance hash is computed separately
        // by job_cache_key_with_components at record time.
        use crate::model::MaterializePolicy;

        let mut jobs = vec![make_job("j1", "rule", vec![], vec!["out.bin"])];
        jobs[0].outputs[0].materialize = MaterializePolicy::Auto;

        let graph = JobGraph::build(jobs.clone()).unwrap();
        let job_ids = graph.job_ids();
        let mut state = Frontier::new(
            &job_ids,
            &graph,
            1024,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        let test_data = vec![42u8; 100];
        let mem_map = crate::memory_map::OutputMemoryMap::new();
        mem_map.put("out.bin".into(), Arc::from(test_data.clone()));

        let _ = state.register_materializations(&jobs[0], true, Some(&mem_map));

        let key = output_ref_key(&OutputRef::File(PathBuf::from("out.bin")));
        let mat_set = state.output_mats.get(&key).unwrap();

        // Size should be tracked for eviction.
        assert_eq!(mat_set.size_bytes(), 100);

        // ArtifactMeta is NOT set (no BLAKE3 hash computed).
        // This is intentional — the hash is computed lazily at cache
        // record time, not on the hot path.
        assert!(
            mat_set.artifact_meta().is_none(),
            "ArtifactMeta should NOT be set (BLAKE3 skipped for performance)"
        );
    }

    #[test]
    fn artifact_meta_size_drives_eviction_ordering() {
        // Verify that Belady eviction uses ArtifactMeta.size_bytes for
        // ordering: among evictable outputs, the largest is evicted first.
        let graph = JobGraph::build(vec![
            make_test_job("A", &[], &["small.bin"]),
            make_test_job("B", &[], &["large.bin"]),
        ])
        .unwrap();
        let topo = graph.topological_order().unwrap();
        let mut state = Frontier::new(
            &topo,
            &graph,
            200, // budget: 200 bytes
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        // Register small output (50 bytes) with ArtifactMeta.
        let small_data: Arc<[u8]> = Arc::from(vec![0u8; 50].as_slice());
        state
            .memory_store
            .insert("small.bin".into(), small_data.clone());
        state.register_in_memory("small.bin", 50);
        if let Some(ms) = state.output_mats.get_mut("small.bin") {
            let hash = blake3::hash(&small_data);
            ms.set_artifact_meta(ArtifactMeta::new(*hash.as_bytes(), 50));
            // Mark as leaf (no consumers) so it's evictable.
            ms.consumer_fired(); // 0 consumers for root
        }

        // Register large output (180 bytes) with ArtifactMeta.
        let large_data: Arc<[u8]> = Arc::from(vec![1u8; 180].as_slice());
        state
            .memory_store
            .insert("large.bin".into(), large_data.clone());
        state.register_in_memory("large.bin", 180);
        if let Some(ms) = state.output_mats.get_mut("large.bin") {
            let hash = blake3::hash(&large_data);
            ms.set_artifact_meta(ArtifactMeta::new(*hash.as_bytes(), 180));
            ms.consumer_fired();
        }

        // Total: 50 + 180 = 230 bytes, budget = 200. Over budget.
        assert_eq!(state.memory_used_bytes(), 230);

        let evicted = state.enforce_memory_budget();
        assert_eq!(evicted, 1, "should evict exactly 1 output");

        // The larger output (180 bytes) should be evicted first.
        assert!(
            !state.memory_store.contains_key("large.bin"),
            "large output should be evicted"
        );
        assert!(
            state.memory_store.contains_key("small.bin"),
            "small output should remain"
        );
        assert_eq!(state.memory_used_bytes(), 50);
    }

    // -- Resource budget unit tests -------------------------------------------

    #[test]
    fn resource_budget_empty_always_fits() {
        let budget = ResourceBudget::new(BTreeMap::new());
        let mut resources = BTreeMap::new();
        resources.insert("cpu".into(), ResourceValue::Int(100));
        assert!(budget.fits(&resources));
    }

    #[test]
    fn resource_budget_fits_within_capacity() {
        let mut cap = BTreeMap::new();
        cap.insert("cpu".into(), 8);
        cap.insert("mem_mb".into(), 32000);
        let budget = ResourceBudget::new(cap);

        let mut resources = BTreeMap::new();
        resources.insert("cpu".into(), ResourceValue::Int(4));
        resources.insert("mem_mb".into(), ResourceValue::Int(16000));
        assert!(budget.fits(&resources));
    }

    #[test]
    fn resource_budget_exceeds_capacity() {
        let mut cap = BTreeMap::new();
        cap.insert("cpu".into(), 8);
        let budget = ResourceBudget::new(cap);

        let mut resources = BTreeMap::new();
        resources.insert("cpu".into(), ResourceValue::Int(9));
        assert!(!budget.fits(&resources));
    }

    #[test]
    fn resource_budget_acquire_and_release() {
        let mut cap = BTreeMap::new();
        cap.insert("cpu".into(), 8);
        let budget = ResourceBudget::new(cap);

        let mut r1 = BTreeMap::new();
        r1.insert("cpu".into(), ResourceValue::Int(4));

        assert!(budget.fits(&r1));
        let g1 = budget.acquire(&r1);
        assert!(budget.fits(&r1)); // 4+4 = 8 == capacity, fits exactly.
        let g2 = budget.acquire(&r1); // Now at 8/8.

        let mut r2 = BTreeMap::new();
        r2.insert("cpu".into(), ResourceValue::Int(1));
        assert!(!budget.fits(&r2)); // 8+1 > 8.

        drop(g1); // Release 4 CPUs → 4/8 in use.
        assert!(budget.fits(&r2)); // 4+1 = 5 <= 8.
        drop(g2);
    }

    #[test]
    fn resource_budget_unconstrained_keys_ignored() {
        let mut cap = BTreeMap::new();
        cap.insert("cpu".into(), 4);
        let budget = ResourceBudget::new(cap);

        // Job requests "gpu" which has no budget limit — should pass.
        let mut resources = BTreeMap::new();
        resources.insert("cpu".into(), ResourceValue::Int(2));
        resources.insert("gpu".into(), ResourceValue::Int(4));
        assert!(budget.fits(&resources));
    }

    #[test]
    fn resource_budget_string_values_parsed() {
        let mut cap = BTreeMap::new();
        cap.insert("mem".into(), 32_000_000_000); // 32G
        let budget = ResourceBudget::new(cap);

        let mut resources = BTreeMap::new();
        resources.insert("mem".into(), ResourceValue::Str("16G".into()));
        assert!(budget.fits(&resources));

        let mut too_much = BTreeMap::new();
        too_much.insert("mem".into(), ResourceValue::Str("33G".into()));
        assert!(!budget.fits(&too_much));
    }

    // -- Resource budget integration test (full scheduler) --------------------

    /// Two independent jobs each request 4 CPUs with a budget of 6.
    /// With max_jobs=4, both could run concurrently without resource limits,
    /// but the resource budget should force sequential execution (4+4=8 > 6).
    #[tokio::test]
    async fn scheduler_respects_resource_budget() {
        use std::sync::atomic::AtomicUsize;

        // Track peak concurrency via an atomic counter.
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        #[derive(Debug)]
        struct ConcurrencyTracker {
            running: Arc<AtomicUsize>,
            peak: Arc<AtomicUsize>,
        }

        impl Executor for ConcurrencyTracker {
            type Error = MockError;
            async fn init(&self) -> Result<(), MockError> {
                Ok(())
            }
            async fn health_check(&self) -> Result<(), MockError> {
                Ok(())
            }
            async fn cleanup(&self) -> Result<(), MockError> {
                Ok(())
            }
            fn capabilities(&self) -> ExecutorCapabilities {
                ExecutorCapabilities::default()
            }
            fn max_concurrency(&self) -> Option<usize> {
                None
            }
            async fn prepare_workspace(
                &self,
                _job: &ConcreteJob,
                _ctx: &ExecContext,
            ) -> Result<Workspace, MockError> {
                Ok(Workspace::new(PathBuf::from("/tmp/mock")))
            }
            async fn execute(
                &self,
                job: &ConcreteJob,
                _workspace: &Workspace,
                _ctx: &ExecContext,
            ) -> Result<JobResult, MockError> {
                let prev = self.running.fetch_add(1, Ordering::SeqCst);
                // Update peak.
                let _ = self.peak.fetch_max(prev + 1, Ordering::SeqCst);
                // Simulate some work so both jobs overlap if allowed.
                tokio::time::sleep(Duration::from_millis(50)).await;
                self.running.fetch_sub(1, Ordering::SeqCst);
                Ok(JobResult {
                    job_id: job.id.clone(),
                    exit_code: 0,
                    duration: Duration::from_millis(50),
                    peak_memory_bytes: None,
                    cpu_time: None,
                    log_path: None,
                    stderr_tail: None,
                })
            }
            async fn finalize_workspace(
                &self,
                _workspace: Workspace,
                _result: &JobResult,
            ) -> Result<(), MockError> {
                Ok(())
            }
            async fn cancel(&self, _job_id: &JobId) -> Result<(), MockError> {
                Ok(())
            }
            async fn poll_status(&self, _job_id: &JobId) -> Result<JobStatus, MockError> {
                Ok(JobStatus::Completed)
            }
            async fn submit_dag(
                &self,
                _graph: &crate::job_graph::JobGraph,
                _ctx: &ExecContext,
            ) -> Result<DagSubmission, MockError> {
                Err(MockError("not supported".into()))
            }
        }

        // Two independent jobs, each requesting 4 CPUs.
        let mut job_a = make_job("A", "rule_a", vec![], vec!["a.txt"]);
        job_a.resources.insert("cpu".into(), ResourceValue::Int(4));
        let mut job_b = make_job("B", "rule_b", vec![], vec!["b.txt"]);
        job_b.resources.insert("cpu".into(), ResourceValue::Int(4));

        let graph = JobGraph::build(vec![job_a, job_b]).unwrap();
        let executor = Arc::new(ConcurrencyTracker {
            running: running.clone(),
            peak: peak.clone(),
        });

        // Budget: 6 CPUs, max_jobs: 4. Without resource budget, both would run
        // concurrently. With it, 4+4=8 > 6 means only one at a time.
        let mut config = SchedulerConfig::default();
        config.max_jobs = 4;
        config.resource_budget.insert("cpu".into(), 6);

        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor, &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 0);
        // Peak concurrency must be 1 because 4+4 > 6.
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "resource budget should limit concurrency to 1"
        );
    }

    /// Jobs that fit within resource budget can run concurrently.
    #[tokio::test]
    async fn scheduler_allows_concurrent_when_budget_permits() {
        use std::sync::atomic::AtomicUsize;

        let peak = Arc::new(AtomicUsize::new(0));
        let running = Arc::new(AtomicUsize::new(0));

        #[derive(Debug)]
        struct ConcTracker {
            running: Arc<AtomicUsize>,
            peak: Arc<AtomicUsize>,
        }

        impl Executor for ConcTracker {
            type Error = MockError;
            async fn init(&self) -> Result<(), MockError> {
                Ok(())
            }
            async fn health_check(&self) -> Result<(), MockError> {
                Ok(())
            }
            async fn cleanup(&self) -> Result<(), MockError> {
                Ok(())
            }
            fn capabilities(&self) -> ExecutorCapabilities {
                ExecutorCapabilities::default()
            }
            fn max_concurrency(&self) -> Option<usize> {
                None
            }
            async fn prepare_workspace(
                &self,
                _j: &ConcreteJob,
                _c: &ExecContext,
            ) -> Result<Workspace, MockError> {
                Ok(Workspace::new(PathBuf::from("/tmp/mock")))
            }
            async fn execute(
                &self,
                job: &ConcreteJob,
                _w: &Workspace,
                _c: &ExecContext,
            ) -> Result<JobResult, MockError> {
                let prev = self.running.fetch_add(1, Ordering::SeqCst);
                let _ = self.peak.fetch_max(prev + 1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                self.running.fetch_sub(1, Ordering::SeqCst);
                Ok(JobResult {
                    job_id: job.id.clone(),
                    exit_code: 0,
                    duration: Duration::from_millis(50),
                    peak_memory_bytes: None,
                    cpu_time: None,
                    log_path: None,
                    stderr_tail: None,
                })
            }
            async fn finalize_workspace(
                &self,
                _w: Workspace,
                _r: &JobResult,
            ) -> Result<(), MockError> {
                Ok(())
            }
            async fn cancel(&self, _j: &JobId) -> Result<(), MockError> {
                Ok(())
            }
            async fn poll_status(&self, _j: &JobId) -> Result<JobStatus, MockError> {
                Ok(JobStatus::Completed)
            }
            async fn submit_dag(
                &self,
                _g: &crate::job_graph::JobGraph,
                _c: &ExecContext,
            ) -> Result<DagSubmission, MockError> {
                Err(MockError("not supported".into()))
            }
        }

        // Two jobs each requesting 4 CPUs, budget = 8 (enough for both).
        let mut job_a = make_job("A", "rule_a", vec![], vec!["a.txt"]);
        job_a.resources.insert("cpu".into(), ResourceValue::Int(4));
        let mut job_b = make_job("B", "rule_b", vec![], vec!["b.txt"]);
        job_b.resources.insert("cpu".into(), ResourceValue::Int(4));

        let graph = JobGraph::build(vec![job_a, job_b]).unwrap();
        let executor = Arc::new(ConcTracker {
            running: running.clone(),
            peak: peak.clone(),
        });

        let mut config = SchedulerConfig::default();
        config.max_jobs = 4;
        config.resource_budget.insert("cpu".into(), 8);

        let bus = EventBus::new();
        let ctx = default_ctx();

        let result = run_scheduler(&graph, executor, &config, &bus, &ctx)
            .await
            .unwrap();

        assert_eq!(result.succeeded, 2);
        // Peak should be 2 since both fit: 4+4 = 8 = capacity.
        assert_eq!(
            peak.load(Ordering::SeqCst),
            2,
            "both jobs should run concurrently"
        );
    }

    // -- OX-7 Re-derivability tests -----------------------------------------
    //
    // Invariant: the Frontier (statuses, ready_frontier, pending_upstream) is
    // a pure function of the graph plus the Ledger's terminal-status set.
    // These tests assert that re-deriving the Frontier from a snapshot —
    // imitating a SIGKILL-then-restart — yields the same decision point.

    /// `frontier_poison` — drop Frontier mid-run, rebuild from the
    /// Ledger snapshot alone, and verify the resumed state matches the
    /// original on every observable Frontier field.
    ///
    /// Topology (diamond):
    /// ```text
    ///       A
    ///      / \
    ///     B   C
    ///      \ /
    ///       D
    /// ```
    /// Partial run: A and B reach terminal states. C is mid-flight (Running
    /// in old_state). D has never been touched (Pending). After resume:
    /// - A keeps its Succeeded status (from snapshot).
    /// - B keeps its Skipped status (from snapshot).
    /// - C reverts to Pending (Running is not durable across process death).
    /// - D stays Pending.
    /// - C joins the ready_frontier (B is Skipped → satisfied → C unblocked).
    #[test]
    fn frontier_poison_diamond_rederives_frontier() {
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "rC", vec!["a.txt"], vec!["c.txt"]),
            make_job("D", "rD", vec!["b.txt", "c.txt"], vec!["d.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let topo = graph.topological_order().unwrap();

        // Build the original ("pre-crash") state and drive it forward by
        // hand: A Succeeded, B Skipped (cache hit), C Running (mid-flight),
        // D untouched.
        let mut old_state =
            Frontier::new(&topo, &graph, 0, None, ResourceBudget::new(BTreeMap::new()));
        old_state.set_status(JobId::from("A"), JobLifecycle::Succeeded);
        old_state.ready_frontier.remove(&JobId::from("A"));
        old_state.promote_downstream(&JobId::from("A"), &graph);

        old_state.set_status(JobId::from("B"), JobLifecycle::Skipped);
        old_state.ready_frontier.remove(&JobId::from("B"));
        old_state.promote_downstream(&JobId::from("B"), &graph);

        // C started running but the process is about to die — Running is
        // intentionally NOT recorded in the snapshot.
        old_state.set_status(JobId::from("C"), JobLifecycle::Running);
        old_state.ready_frontier.remove(&JobId::from("C"));

        // --- Imitate the SIGKILL: the in-memory struct vanishes; only
        // terminal statuses survive in the Ledger snapshot. ---
        let mut snapshot = LedgerSnapshot::new();
        for (id, status) in &old_state.statuses {
            if status.is_terminal() {
                snapshot.insert(id.clone(), status.clone());
            }
        }

        // C is Running in old_state → not terminal → not in snapshot.
        assert!(!snapshot.statuses.contains_key(&JobId::from("C")));
        assert_eq!(snapshot.len(), 2); // A + B

        drop(old_state);

        // --- Restart: rebuild Frontier from the snapshot alone. ---
        let new_state = Frontier::resume(
            &topo,
            &graph,
            &snapshot,
            0,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        // Statuses: terminal entries from snapshot are preserved; others
        // revert to Pending (Running is not durable).
        assert_eq!(
            new_state.statuses.get(&JobId::from("A")),
            Some(&JobLifecycle::Succeeded)
        );
        assert_eq!(
            new_state.statuses.get(&JobId::from("B")),
            Some(&JobLifecycle::Skipped)
        );
        assert_eq!(
            new_state.statuses.get(&JobId::from("C")),
            Some(&JobLifecycle::Pending),
            "C was Running pre-crash; resume must revert it to Pending so it \
             can be re-dispatched."
        );
        assert_eq!(
            new_state.statuses.get(&JobId::from("D")),
            Some(&JobLifecycle::Pending)
        );

        // pending_upstream: A=0 (root, but already Succeeded — irrelevant
        // since it's not Pending). B=0 (Skipped, no longer Pending). C=0
        // because A is Succeeded (satisfied). D=1 because C is still
        // unsatisfied (Pending) — B contributed 0 to D's count.
        assert_eq!(new_state.pending_upstream[&JobId::from("A")], 0);
        assert_eq!(new_state.pending_upstream[&JobId::from("B")], 0);
        assert_eq!(new_state.pending_upstream[&JobId::from("C")], 0);
        assert_eq!(
            new_state.pending_upstream[&JobId::from("D")],
            1,
            "D has two upstreams (B, C); B is Skipped (satisfied) but C is \
             Pending (unsatisfied) → count = 1"
        );

        // ready_frontier: only C — A/B are no longer Pending, D still
        // blocked by C.
        let expected_frontier: HashSet<JobId> = [JobId::from("C")].into_iter().collect();
        assert_eq!(
            new_state.ready_frontier, expected_frontier,
            "After resume, C is the only ready job — exactly what a fresh \
             scheduler would conclude from the same Ledger contents."
        );
    }

    /// `resume_from_empty_snapshot_equals_new` — re-derivability degenerates
    /// to fresh construction when the Ledger has no terminal entries.
    /// Establishes the base case of the OX-7 equivalence: `resume(∅) ≡ new`.
    #[test]
    fn resume_from_empty_snapshot_equals_new() {
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "rC", vec!["a.txt"], vec!["c.txt"]),
            make_job("D", "rD", vec!["b.txt", "c.txt"], vec!["d.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let topo = graph.topological_order().unwrap();

        let fresh = Frontier::new(&topo, &graph, 0, None, ResourceBudget::new(BTreeMap::new()));
        let resumed = Frontier::resume(
            &topo,
            &graph,
            &LedgerSnapshot::new(),
            0,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        assert_eq!(fresh.statuses, resumed.statuses);
        assert_eq!(fresh.pending_upstream, resumed.pending_upstream);
        assert_eq!(fresh.ready_frontier, resumed.ready_frontier);
    }

    /// `resume_propagates_blocked_downstream_after_failure` — a failed
    /// upstream keeps its downstream blocked. Exercises the load-bearing
    /// branch of `pending_upstream`: only `Succeeded` and `Skipped` count as
    /// satisfied; `Failed` and `Cancelled` do not.
    #[test]
    fn resume_keeps_downstream_blocked_when_upstream_failed() {
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let topo = graph.topological_order().unwrap();

        let mut snapshot = LedgerSnapshot::new();
        snapshot.insert(JobId::from("A"), JobLifecycle::Failed("boom".into()));

        let state = Frontier::resume(
            &topo,
            &graph,
            &snapshot,
            0,
            None,
            ResourceBudget::new(BTreeMap::new()),
        );

        assert!(matches!(
            state.statuses.get(&JobId::from("A")),
            Some(JobLifecycle::Failed(_))
        ));
        assert_eq!(
            state.statuses.get(&JobId::from("B")),
            Some(&JobLifecycle::Pending)
        );
        // B's upstream (A) is Failed → not satisfied → B remains blocked.
        assert_eq!(state.pending_upstream[&JobId::from("B")], 1);
        assert!(
            !state.ready_frontier.contains(&JobId::from("B")),
            "downstream of a failed upstream must NOT be dispatched"
        );
    }

    /// `crash_and_restart_yields_same_terminal_status_set` — end-to-end
    /// re-derivability. Drives the scheduler to partial completion, simulates
    /// a SIGKILL by capturing a LedgerSnapshot from the partial run, then
    /// re-invokes `run_scheduler` with the snapshot and asserts:
    ///
    /// 1. **No re-execution** — jobs marked terminal in the snapshot are
    ///    not dispatched to the executor a second time (call_count check).
    /// 2. **Same terminal status set** — combining the partial run's
    ///    terminal statuses with the resumed run's completions reproduces
    ///    the baseline single-process run's terminal set, modulo timing.
    ///
    /// Topology: linear chain `A → B → C → D → E → F` (6 jobs). The
    /// "crash" snapshot includes A, B, C as Succeeded; the resumed run
    /// must execute exactly D, E, F.
    ///
    /// This test stands in for the binary-level subprocess SIGKILL test
    /// described in the OX-7 audit proposal: at the library level it
    /// exercises the same invariant (Frontier re-derived from Ledger)
    /// without needing a real `ox run` subprocess. The binary wiring
    /// (which feeds `LedgerSnapshot` from `ox-state`'s `jobs` table into
    /// `SchedulerConfig::resume_snapshot`) is the next sequencing step
    /// and is exercised by the `crash.sh` chaos harness once it lands.
    #[tokio::test]
    async fn crash_and_restart_yields_same_terminal_status_set() {
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
            make_job("C", "rC", vec!["b.txt"], vec!["c.txt"]),
            make_job("D", "rD", vec!["c.txt"], vec!["d.txt"]),
            make_job("E", "rE", vec!["d.txt"], vec!["e.txt"]),
            make_job("F", "rF", vec!["e.txt"], vec!["f.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();

        // --- Baseline: clean single-process run. ---
        let baseline_executor = Arc::new(MockExecutor::new());
        let bus = EventBus::new();
        let ctx = default_ctx();
        let baseline = run_scheduler(
            &graph,
            baseline_executor.clone(),
            &SchedulerConfig::default(),
            &bus,
            &ctx,
        )
        .await
        .unwrap();

        assert_eq!(baseline.succeeded, 6);
        assert_eq!(baseline.failed, 0);
        assert_eq!(baseline.cancelled, 0);
        assert_eq!(
            baseline_executor.call_count.load(Ordering::SeqCst),
            6,
            "baseline must dispatch every job exactly once"
        );

        // --- Crash imitation: build a LedgerSnapshot as if the process had
        // been SIGKILLed after the first three jobs completed. ---
        let mut snapshot = LedgerSnapshot::new();
        snapshot.insert(JobId::from("A"), JobLifecycle::Succeeded);
        snapshot.insert(JobId::from("B"), JobLifecycle::Succeeded);
        snapshot.insert(JobId::from("C"), JobLifecycle::Succeeded);

        // --- Resume: a fresh scheduler invocation with the snapshot. ---
        let resumed_executor = Arc::new(MockExecutor::new());
        let bus2 = EventBus::new();
        let config_resume = SchedulerConfig {
            resume_snapshot: Some(snapshot),
            ..Default::default()
        };
        let resumed = run_scheduler(
            &graph,
            resumed_executor.clone(),
            &config_resume,
            &bus2,
            &ctx,
        )
        .await
        .unwrap();

        // The resumed run sees three jobs already terminal from the
        // snapshot. They count as Skipped in the result (their terminal
        // status was carried over, not re-executed). The remaining three
        // (D, E, F) succeed normally.
        assert_eq!(
            resumed.total_jobs, 6,
            "graph job count is unchanged across the crash"
        );
        assert_eq!(
            resumed.cancelled, 0,
            "no cancellations after a clean resume"
        );
        assert_eq!(resumed.failed, 0, "no failures after a clean resume");

        // Load-bearing invariant: the carried-over snapshot statuses
        // count toward the terminal totals.
        let carried_over = resumed.succeeded + resumed.skipped;
        assert_eq!(
            carried_over, 6,
            "every job reaches a satisfied terminal state across the \
             two runs combined (A/B/C from snapshot + D/E/F from resume)"
        );

        // No-re-execution invariant: the resumed scheduler must NOT have
        // dispatched A, B, or C to the executor — they were carried over
        // from the snapshot.
        assert_eq!(
            resumed_executor.call_count.load(Ordering::SeqCst),
            3,
            "resumed run must dispatch only the jobs that were NOT in the \
             snapshot (D, E, F). Re-executing carried-over jobs would \
             violate OX-7 by destroying the Ledger's record of completion."
        );

        // Terminal status set equivalence (modulo timing): both runs
        // arrive at a state where every job is in a satisfied terminal
        // status. Hash these sets and compare.
        let baseline_terminal_count =
            baseline.succeeded + baseline.skipped + baseline.failed + baseline.cancelled;
        let resumed_terminal_count =
            resumed.succeeded + resumed.skipped + resumed.failed + resumed.cancelled;
        assert_eq!(
            baseline_terminal_count, resumed_terminal_count,
            "OX-7: terminal status set must be invariant across crash + restart"
        );
        assert_eq!(baseline_terminal_count, 6);
    }

    /// `ledger_snapshot_rejects_non_terminal_inserts` — the snapshot is a
    /// terminal-only contract. `Running`, `Ready`, `Pending` are silently
    /// dropped because they are not durable across process death (OX-7).
    #[test]
    fn ledger_snapshot_rejects_non_terminal_inserts() {
        let mut s = LedgerSnapshot::new();
        s.insert(JobId::from("a"), JobLifecycle::Pending);
        s.insert(JobId::from("b"), JobLifecycle::Ready);
        s.insert(JobId::from("c"), JobLifecycle::Running);
        assert!(s.is_empty());

        s.insert(JobId::from("d"), JobLifecycle::Succeeded);
        s.insert(JobId::from("e"), JobLifecycle::Skipped);
        s.insert(JobId::from("f"), JobLifecycle::Failed("nope".into()));
        s.insert(JobId::from("g"), JobLifecycle::Cancelled);
        assert_eq!(s.len(), 4);
    }

    // -- B4: cancel/completion race ------------------------------------------

    fn completion_msg_for(graph: &JobGraph, id: &str, exit_code: i32) -> CompletionMsg {
        let job_id = JobId::from(id);
        CompletionMsg {
            job_id: job_id.clone(),
            job: graph.get_job(&job_id).unwrap().clone(),
            result: JobResult {
                job_id,
                exit_code,
                duration: Duration::from_millis(1),
                peak_memory_bytes: None,
                cpu_time: None,
                log_path: None,
                stderr_tail: None,
            },
        }
    }

    /// B4: a job marked Cancelled (by cancel_downstream) whose process
    /// finishes successfully before the kill lands must NOT be rewritten
    /// Succeeded, and its downstream must NOT be promoted. The TLA+ spec
    /// (CancelPropagation) models this race; the code must preserve it.
    #[tokio::test]
    async fn cancelled_job_success_completion_stays_cancelled() {
        let jobs = vec![
            make_job("A", "rA", vec![], vec!["a.txt"]),
            make_job("B", "rB", vec!["a.txt"], vec!["b.txt"]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let job_ids: Vec<&JobId> = graph.job_ids();
        let state = Arc::new(Mutex::new(Frontier::new(
            &job_ids,
            &graph,
            0,
            None,
            ResourceBudget::new(BTreeMap::new()),
        )));
        {
            // Simulate cancel_downstream having flipped A while it was running.
            let mut s = state.lock().await;
            s.set_status(JobId::from("A"), JobLifecycle::Cancelled);
        }

        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();
        let executor = MockExecutor::new();
        let mut terminate = false;

        let msg = completion_msg_for(&graph, "A", 0);
        let committed = handle_completion(
            &msg,
            &state,
            &graph,
            &config,
            &bus,
            &ctx,
            &mut terminate,
            &executor,
        )
        .await;

        assert!(!committed, "cancelled job result must not be committed");
        let s = state.lock().await;
        assert!(
            matches!(
                s.get_status(&JobId::from("A")),
                Some(JobLifecycle::Cancelled)
            ),
            "status must remain Cancelled, got {:?}",
            s.get_status(&JobId::from("A"))
        );
        assert!(
            !s.ready_frontier.contains(&JobId::from("B")),
            "downstream of a cancelled job must not be promoted"
        );
    }

    /// B4 (failure side): a cancelled job that exits non-zero (the expected
    /// SIGTERM outcome) with a Retry strategy must NOT be resurrected to
    /// Pending — it stays Cancelled.
    #[tokio::test]
    async fn cancelled_job_failure_completion_is_not_retried() {
        let jobs = vec![make_job_with_strategy(
            "A",
            "rA",
            vec![],
            vec!["a.txt"],
            ErrorStrategy::Retry {
                count: 3,
                backoff: Backoff::Constant,
            },
        )];
        let graph = JobGraph::build(jobs).unwrap();
        let job_ids: Vec<&JobId> = graph.job_ids();
        let state = Arc::new(Mutex::new(Frontier::new(
            &job_ids,
            &graph,
            0,
            None,
            ResourceBudget::new(BTreeMap::new()),
        )));
        {
            // Simulate the job having been dispatched (drained from the
            // frontier) and then cancelled while running.
            let mut s = state.lock().await;
            s.ready_frontier.remove(&JobId::from("A"));
            s.set_status(JobId::from("A"), JobLifecycle::Cancelled);
        }

        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let ctx = default_ctx();
        let executor = MockExecutor::new();
        let mut terminate = false;

        let msg = completion_msg_for(&graph, "A", 137);
        let committed = handle_completion(
            &msg,
            &state,
            &graph,
            &config,
            &bus,
            &ctx,
            &mut terminate,
            &executor,
        )
        .await;

        assert!(!committed);
        let s = state.lock().await;
        assert!(
            matches!(
                s.get_status(&JobId::from("A")),
                Some(JobLifecycle::Cancelled)
            ),
            "cancelled job must not re-enter the retry loop, got {:?}",
            s.get_status(&JobId::from("A"))
        );
        assert!(!s.ready_frontier.contains(&JobId::from("A")));
    }

    // -- H8: resource budget must not leak on task abort ----------------------

    /// H8: the budget acquired at dispatch must be released when the
    /// executing task is aborted (abort_all on fatal error / panic).
    /// With release tied to the completion message, an aborted task never
    /// sends one and the budget leaks forever — every later job that needs
    /// the resources is re-deferred and the run hangs silently.
    #[tokio::test]
    async fn resource_budget_released_when_task_aborted() {
        let mut cap = BTreeMap::new();
        cap.insert("cpu".to_string(), 2u64);
        let budget = ResourceBudget::new(cap);

        let mut resources = BTreeMap::new();
        resources.insert("cpu".to_string(), ResourceValue::Int(2));

        assert!(budget.fits(&resources));
        let guard = budget.acquire(&resources);
        assert!(!budget.fits(&resources), "budget fully consumed");

        // Move the guard into a task that never completes, then abort it —
        // models join_set.abort_all() killing in-flight jobs.
        let task = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        // Let the task start so the guard is owned by the spawned future.
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;

        assert!(
            budget.fits(&resources),
            "budget must be released when the owning task is aborted"
        );
    }

    /// H8: dropping the guard releases exactly what was acquired.
    #[test]
    fn resource_guard_drop_releases_budget() {
        let mut cap = BTreeMap::new();
        cap.insert("cpu".to_string(), 8u64);
        let budget = ResourceBudget::new(cap);

        let mut r1 = BTreeMap::new();
        r1.insert("cpu".to_string(), ResourceValue::Int(8));

        let guard = budget.acquire(&r1);
        assert!(!budget.fits(&r1));
        drop(guard);
        assert!(budget.fits(&r1));
    }

    // -- H6: TOCTOU between gate check and Ready flip -------------------------

    /// Gate checker that cancels a job in the scheduler state while the gate
    /// check is in flight — models cancel_downstream racing the gated
    /// readiness path between the frontier drain and the Ready flip.
    struct CancellingGateChecker {
        state: Arc<Mutex<Frontier>>,
        target: JobId,
    }

    impl GateCheck for CancellingGateChecker {
        fn check_gate<'a>(
            &'a self,
            _gate_id: &'a GateId,
        ) -> Pin<Box<dyn Future<Output = GateStatus> + Send + 'a>> {
            Box::pin(async move {
                let mut s = self.state.lock().await;
                s.set_status(self.target.clone(), JobLifecycle::Cancelled);
                drop(s);
                GateStatus::Approved
            })
        }

        fn register_gate<'a>(
            &'a self,
            _gate_id: &'a GateId,
            _run_id: Option<&'a str>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }
    }

    /// H6: a job cancelled between the frontier drain and the gated Ready
    /// flip must not be returned as ready (it would be dispatched after
    /// Cancelled). The gated path must re-validate Pending before flipping.
    #[tokio::test]
    async fn job_cancelled_during_gate_check_is_not_dispatched() {
        let jobs = vec![make_job("gated", "build", vec![], vec!["out.txt"])];
        let mut graph = JobGraph::build(jobs).unwrap();
        let gate_id = GateId::from("approval-gate");
        graph.add_gate(&gate_id, &JobId::from("gated"));

        let job_ids: Vec<&JobId> = graph.job_ids();
        let state = Arc::new(Mutex::new(Frontier::new(
            &job_ids,
            &graph,
            0,
            None,
            ResourceBudget::new(BTreeMap::new()),
        )));
        let bus = EventBus::new();

        let checker: Option<Arc<dyn GateCheck>> = Some(Arc::new(CancellingGateChecker {
            state: state.clone(),
            target: JobId::from("gated"),
        }));

        let ready = find_ready_jobs(&state, &graph, &checker, &bus).await;
        assert!(
            ready.is_empty(),
            "job cancelled during gate check must not be returned as ready, got {ready:?}"
        );
        let s = state.lock().await;
        assert!(
            matches!(
                s.get_status(&JobId::from("gated")),
                Some(JobLifecycle::Cancelled)
            ),
            "status must remain Cancelled, got {:?}",
            s.get_status(&JobId::from("gated"))
        );
    }

    // -- Property-based tests (proptest) ------------------------------------

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for a random memory budget.
        fn arb_budget() -> impl Strategy<Value = u64> {
            1..500_000u64
        }

        /// Build a linear chain DAG with N outputs, each with the given sizes.
        /// Returns (graph, output_keys).
        fn build_linear_dag(sizes: &[u64]) -> (JobGraph, Vec<String>) {
            let n = sizes.len();
            let mut jobs = Vec::new();
            let mut output_keys = Vec::new();

            for i in 0..n {
                let out = format!("out_{i}.bin");
                output_keys.push(out.clone());
                let inputs: Vec<&str> = if i > 0 {
                    vec![Box::leak(format!("out_{}.bin", i - 1).into_boxed_str())]
                } else {
                    vec![]
                };
                let job = make_test_job(
                    Box::leak(format!("job_{i}").into_boxed_str()),
                    &inputs,
                    &[Box::leak(out.into_boxed_str())],
                );
                jobs.push(job);
            }

            let graph = JobGraph::build(jobs).unwrap();
            (graph, output_keys)
        }

        proptest! {
            /// P0: memory_used_bytes always equals the sum of sizes of
            /// outputs currently in memory_store.
            #[test]
            fn budget_accounting_invariant(
                n in 2..10usize,
                budget in arb_budget(),
            ) {
                // Build a linear chain of N jobs.
                let sizes: Vec<u64> = (0..n).map(|i| ((i + 1) * 1000) as u64).collect();
                let (graph, keys) = build_linear_dag(&sizes);
                let topo = graph.topological_order().unwrap();
                let mut state = Frontier::new(
                    &topo, &graph, budget, None,
                    ResourceBudget::new(BTreeMap::new()),
                );

                // Simulate: register each output, consume inputs, evict.
                for (i, key) in keys.iter().enumerate() {
                    let data: Arc<[u8]> = Arc::from(vec![0u8; sizes[i] as usize]);
                    state.memory_store.insert(key.clone(), data);
                    state.register_in_memory(key, sizes[i]);

                    // Fire consumers of previous output (if any).
                    if i > 0 {
                        let prev = &keys[i - 1];
                        if let Some(ms) = state.output_mats.get_mut(prev) {
                            ms.consumer_fired();
                        }
                    }

                    state.enforce_memory_budget();

                    // INVARIANT: accounting matches reality.
                    let actual: u64 = state.memory_store.values()
                        .map(|v| v.len() as u64).sum();
                    prop_assert_eq!(
                        state.memory_used_bytes(), actual,
                        "accounting drift after registering output"
                    );
                }
            }

            /// P0: No output with pending_consumers > 0 is ever evicted.
            #[test]
            fn eviction_safety(
                n in 2..10usize,
                budget in 1..5000u64,
            ) {
                let sizes: Vec<u64> = (0..n).map(|i| ((i + 1) * 1000) as u64).collect();
                let (graph, keys) = build_linear_dag(&sizes);
                let topo = graph.topological_order().unwrap();
                let mut state = Frontier::new(
                    &topo, &graph, budget, None,
                    ResourceBudget::new(BTreeMap::new()),
                );

                // Register all outputs with data.
                for (i, key) in keys.iter().enumerate() {
                    let data: Arc<[u8]> = Arc::from(vec![0u8; sizes[i] as usize]);
                    state.memory_store.insert(key.clone(), data);
                    state.register_in_memory(key, sizes[i]);
                }

                // Fire consumers one by one, running eviction each time.
                for i in 0..n.saturating_sub(1) {
                    if let Some(ms) = state.output_mats.get_mut(&keys[i]) {
                        ms.consumer_fired();
                    }
                    state.enforce_memory_budget();

                    // INVARIANT: no output with pending consumers was evicted.
                    for (j, key) in keys.iter().enumerate() {
                        if let Some(ms) = state.output_mats.get(key) {
                            if ms.pending_consumers() > 0 && !ms.has_in_memory() {
                                prop_assert!(
                                    false,
                                    "output {j} ({key}) evicted with {} pending consumers",
                                    ms.pending_consumers()
                                );
                            }
                        }
                    }
                }
            }

            /// P0: Evicted outputs always have a disk fallback.
            #[test]
            fn eviction_preserves_disk_fallback(
                n in 2..8usize,
                budget in 1..5000u64,
            ) {
                let sizes: Vec<u64> = (0..n).map(|i| ((i + 1) * 1000) as u64).collect();
                let (graph, keys) = build_linear_dag(&sizes);
                let topo = graph.topological_order().unwrap();
                let mut state = Frontier::new(
                    &topo, &graph, budget, None,
                    ResourceBudget::new(BTreeMap::new()),
                );

                // Register all outputs with data + OnDisk.
                for (i, key) in keys.iter().enumerate() {
                    let data: Arc<[u8]> = Arc::from(vec![0u8; sizes[i] as usize]);
                    state.memory_store.insert(key.clone(), data);
                    state.register_in_memory(key, sizes[i]);
                    // register_in_memory already adds OnDisk
                }

                // Also add one output with InMemory ONLY (no disk fallback).
                // This simulates a Never-policy output.
                let never_key = &keys[0];
                if let Some(ms) = state.output_mats.get_mut(never_key) {
                    // Remove the OnDisk that register_in_memory added
                    ms.try_remove(&Materialization::OnDisk {
                        path: PathBuf::from(never_key),
                        verified: false,
                    });
                }

                // Fire all consumers.
                for i in 0..n.saturating_sub(1) {
                    if let Some(ms) = state.output_mats.get_mut(&keys[i]) {
                        ms.consumer_fired();
                    }
                }

                state.enforce_memory_budget();

                // INVARIANT: the output without disk fallback was NOT evicted.
                prop_assert!(
                    state.memory_store.contains_key(never_key),
                    "output without disk fallback should survive eviction"
                );
            }
        }
    }
}
