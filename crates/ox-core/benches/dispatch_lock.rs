//! Dispatch-lock throughput micro-benchmark.
//!
//! Measures end-to-end scheduler throughput at scales where the
//! per-job lock acquisition cost dominates (M7 — Lock Consolidation,
//! `docs/design/next-steps-2026-04-06.md` Priority 1).
//!
//! Before consolidation the dispatch loop took up to 5 separate
//! `state.lock().await` acquisitions per job (force-rerun read, cache
//! skip, budget check, set Running, collect_input_data). After M7 these
//! are folded into a single acquisition.
//!
//! Run with: `cargo bench -p ox-core --bench dispatch_lock`

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use ox_core::event::EventBus;
use ox_core::job_graph::{JobGraph, make_test_job};
use ox_core::model::*;
use ox_core::scheduler::{SchedulerConfig, run_scheduler};
use ox_core::traits::executor::*;

// ---------------------------------------------------------------------------
// No-op executor: returns success immediately so the scheduler hot path
// (find_ready_jobs + dispatch + completion) is the only thing being timed.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct NoopExecutor;

#[derive(Debug, thiserror::Error)]
#[error("noop")]
struct NoopError;

impl From<NoopError> for ox_core::error::OxError {
    fn from(_: NoopError) -> Self {
        ox_core::error::OxError::Exec(ox_core::error::ExecError::Executor {
            message: "noop".into(),
        })
    }
}

impl Executor for NoopExecutor {
    type Error = NoopError;
    fn init(&self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }
    fn health_check(&self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }
    fn cleanup(&self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }
    fn capabilities(&self) -> ExecutorCapabilities {
        ExecutorCapabilities::default()
    }
    fn max_concurrency(&self) -> Option<usize> {
        None
    }
    fn prepare_workspace(
        &self,
        _job: &ConcreteJob,
        _ctx: &ExecContext,
    ) -> impl Future<Output = Result<Workspace, Self::Error>> + Send {
        async { Ok(Workspace::new(PathBuf::from("."))) }
    }
    fn execute(
        &self,
        job: &ConcreteJob,
        _workspace: &Workspace,
        _ctx: &ExecContext,
    ) -> impl Future<Output = Result<JobResult, Self::Error>> + Send {
        let id = job.id.clone();
        async move {
            Ok(JobResult {
                job_id: id,
                exit_code: 0,
                duration: Duration::ZERO,
                peak_memory_bytes: None,
                cpu_time: None,
                log_path: None,
                stderr_tail: None,
            })
        }
    }
    fn finalize_workspace(
        &self,
        _workspace: Workspace,
        _result: &JobResult,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }
    fn cancel(&self, _job_id: &JobId) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }
    fn poll_status(
        &self,
        _job_id: &JobId,
    ) -> impl Future<Output = Result<JobStatus, Self::Error>> + Send {
        async { Ok(JobStatus::Completed) }
    }
    fn submit_dag(
        &self,
        _graph: &JobGraph,
        _ctx: &ExecContext,
    ) -> impl Future<Output = Result<DagSubmission, Self::Error>> + Send {
        async { Err(NoopError) }
    }
}

fn build_wide(n: usize) -> JobGraph {
    let mut jobs = Vec::with_capacity(n);
    for i in 0..n {
        jobs.push(make_test_job(
            Box::leak(format!("j_{i}").into_boxed_str()),
            &[],
            &[Box::leak(format!("out_{i}.bin").into_boxed_str())],
        ));
    }
    JobGraph::build(jobs).unwrap()
}

fn default_ctx() -> ExecContext {
    ExecContext {
        global_job_limit: 1_000_000,
        run_id: "bench".into(),
        log_dir: PathBuf::from("/tmp/ox_bench_logs"),
        project_dir: PathBuf::from("."),
        trusted_dirs: vec![],
        input_data: HashMap::new(),
        memory_map: None,
    }
}

/// Wide-parallel dispatch — exercises the per-job dispatch lock at
/// scale. The lock-consolidation gain is most visible here because
/// the dispatcher iterates over a flat frontier of N ready jobs and
/// performs one lock cycle per job.
fn bench_dispatch_wide(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("dispatch_wide");
    // Each iteration drives the full scheduler — keep the sample count
    // low so the suite finishes in tens of seconds rather than minutes.
    group.sample_size(10);

    for &n in &[1_000usize, 10_000] {
        group.bench_with_input(BenchmarkId::new("jobs", n), &n, |b, &n| {
            let graph = build_wide(n);
            b.iter(|| {
                rt.block_on(async {
                    let executor = Arc::new(NoopExecutor);
                    let config = SchedulerConfig {
                        max_jobs: n,
                        ..Default::default()
                    };
                    let bus = EventBus::new();
                    let ctx = default_ctx();
                    let result = run_scheduler(&graph, executor, &config, &bus, &ctx)
                        .await
                        .unwrap();
                    assert_eq!(result.succeeded, n);
                });
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_dispatch_wide);
criterion_main!(benches);
