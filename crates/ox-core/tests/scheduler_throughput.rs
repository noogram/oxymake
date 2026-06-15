//! Scheduler throughput benchmark — measures pure scheduling overhead
//! with no-op tasks.
//!
//! Run with:
//! ```sh
//! cargo test -p ox-core --test scheduler_throughput -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ox_core::event::EventBus;
use ox_core::job_graph::{JobGraph, make_test_job};
use ox_core::model::*;
use ox_core::scheduler::{SchedulerConfig, run_scheduler};
use ox_core::traits::executor::*;

// ---------------------------------------------------------------------------
// No-op executor: returns success immediately
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

// ---------------------------------------------------------------------------
// DAG builders
// ---------------------------------------------------------------------------

fn build_chain(n: usize) -> JobGraph {
    let mut jobs = Vec::with_capacity(n);
    for i in 0..n {
        let inputs: Vec<&str> = if i > 0 {
            vec![Box::leak(format!("out_{}.bin", i - 1).into_boxed_str())]
        } else {
            vec![]
        };
        jobs.push(make_test_job(
            Box::leak(format!("j_{i}").into_boxed_str()),
            &inputs,
            &[Box::leak(format!("out_{i}.bin").into_boxed_str())],
        ));
    }
    JobGraph::build(jobs).unwrap()
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

fn build_diamond(n_middle: usize) -> JobGraph {
    let mut jobs = Vec::with_capacity(n_middle + 2);
    jobs.push(make_test_job("root", &[], &["root.bin"]));

    let mut middle_outs = Vec::new();
    for i in 0..n_middle {
        let out: &str = Box::leak(format!("mid_{i}.bin").into_boxed_str());
        middle_outs.push(out);
        jobs.push(make_test_job(
            Box::leak(format!("mid_{i}").into_boxed_str()),
            &["root.bin"],
            &[out],
        ));
    }

    let refs: Vec<&str> = middle_outs.iter().copied().collect();
    jobs.push(make_test_job("sink", &refs, &["sink.bin"]));
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

// ---------------------------------------------------------------------------
// Throughput measurement
// ---------------------------------------------------------------------------

async fn measure(graph: &JobGraph, max_jobs: usize) -> (Duration, usize) {
    let executor = Arc::new(NoopExecutor);
    let config = SchedulerConfig {
        max_jobs,
        ..Default::default()
    };
    let bus = EventBus::new();
    let ctx = default_ctx();

    let start = Instant::now();
    let result = run_scheduler(graph, executor, &config, &bus, &ctx)
        .await
        .unwrap();
    let elapsed = start.elapsed();
    (elapsed, result.succeeded)
}

// ---------------------------------------------------------------------------
// Tests (run as benchmarks with --ignored --nocapture)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn throughput_chain() {
    println!("\n=== OxyMake Scheduler: Linear Chain ===");
    println!("{:>8} | {:>10} | {:>14}", "N", "Time", "Tasks/sec");
    println!("{:->8}-+-{:->10}-+-{:->14}", "", "", "");

    for &n in &[1_000, 10_000, 100_000] {
        let graph = build_chain(n);
        let (elapsed, succeeded) = measure(&graph, 1).await;
        let tps = succeeded as f64 / elapsed.as_secs_f64();
        println!(
            "{:>8} | {:>10.4}s | {:>12.0} t/s",
            n,
            elapsed.as_secs_f64(),
            tps
        );
        assert_eq!(succeeded, n);
    }
}

#[tokio::test]
#[ignore]
async fn throughput_wide() {
    println!("\n=== OxyMake Scheduler: Wide Parallel ===");
    println!("{:>8} | {:>10} | {:>14}", "N", "Time", "Tasks/sec");
    println!("{:->8}-+-{:->10}-+-{:->14}", "", "", "");

    for &n in &[1_000, 10_000, 100_000] {
        let graph = build_wide(n);
        let (elapsed, succeeded) = measure(&graph, n).await;
        let tps = succeeded as f64 / elapsed.as_secs_f64();
        println!(
            "{:>8} | {:>10.4}s | {:>12.0} t/s",
            n,
            elapsed.as_secs_f64(),
            tps
        );
        assert_eq!(succeeded, n);
    }
}

#[tokio::test]
#[ignore]
async fn throughput_diamond() {
    println!("\n=== OxyMake Scheduler: Diamond (1→N→1) ===");
    println!("{:>8} | {:>10} | {:>14}", "N", "Time", "Tasks/sec");
    println!("{:->8}-+-{:->10}-+-{:->14}", "", "", "");

    for &n in &[1_000, 10_000, 50_000] {
        let graph = build_diamond(n);
        let total = n + 2;
        let (elapsed, succeeded) = measure(&graph, n).await;
        let tps = succeeded as f64 / elapsed.as_secs_f64();
        println!(
            "{:>8} | {:>10.4}s | {:>12.0} t/s",
            total,
            elapsed.as_secs_f64(),
            tps
        );
        assert_eq!(succeeded, total);
    }
}
