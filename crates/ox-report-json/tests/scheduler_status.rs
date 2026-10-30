//! The terminal `status` of a run the scheduler settles without executing
//! anything: `all_cached` only when every skip was a cache hit.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ox_core::event::EventBus;
use ox_core::job_graph::{JobGraph, make_test_job};
use ox_core::model::*;
use ox_core::scheduler::{SchedulerConfig, run_scheduler};
use ox_core::traits::executor::*;
use ox_core::traits::reporter::Reporter;
use ox_report_json::reporter::JsonReporter;

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

    async fn init(&self) -> Result<(), NoopError> {
        Ok(())
    }
    async fn health_check(&self) -> Result<(), NoopError> {
        Ok(())
    }
    async fn cleanup(&self) -> Result<(), NoopError> {
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
    ) -> Result<Workspace, NoopError> {
        Ok(Workspace::new(PathBuf::from(".")))
    }
    async fn execute(
        &self,
        job: &ConcreteJob,
        _workspace: &Workspace,
        _ctx: &ExecContext,
    ) -> Result<JobResult, NoopError> {
        Ok(JobResult {
            job_id: job.id.clone(),
            exit_code: 0,
            duration: Duration::ZERO,
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
    ) -> Result<(), NoopError> {
        Ok(())
    }
    async fn cancel(&self, _job_id: &JobId) -> Result<(), NoopError> {
        Ok(())
    }
    async fn poll_status(&self, _job_id: &JobId) -> Result<JobStatus, NoopError> {
        Ok(JobStatus::Completed)
    }
    async fn submit_dag(
        &self,
        _graph: &JobGraph,
        _ctx: &ExecContext,
    ) -> Result<DagSubmission, NoopError> {
        Err(NoopError)
    }
}

/// Writer whose bytes stay readable after the reporter has consumed it.
#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Run the scheduler over `cached -> excluded` with both skip-listed, the
/// second marked `not_selected` when `exclude` holds, and return the
/// `status` of the reported `run_completed`.
async fn terminal_status(exclude: bool) -> String {
    let graph = JobGraph::build(vec![
        make_test_job("cached", &[], &["a.txt"]),
        make_test_job("excluded", &["a.txt"], &["b.txt"]),
    ])
    .unwrap();
    let config = SchedulerConfig {
        skip_jobs: HashSet::from([JobId::from("cached"), JobId::from("excluded")]),
        not_selected: if exclude {
            HashSet::from([JobId::from("excluded")])
        } else {
            HashSet::new()
        },
        ..Default::default()
    };
    let ctx = ExecContext {
        global_job_limit: 1,
        run_id: "run".into(),
        log_dir: PathBuf::from("logs"),
        project_dir: PathBuf::from("."),
        trusted_dirs: vec![],
        input_data: HashMap::new(),
        memory_map: None,
    };
    let bus = EventBus::new();
    let mut events = bus.subscribe();

    let result = run_scheduler(&graph, Arc::new(NoopExecutor), &config, &bus, &ctx)
        .await
        .unwrap();
    assert_eq!(result.succeeded, 0, "nothing is executed");

    let buffer = SharedBuffer::default();
    let reporter = JsonReporter::new(buffer.clone());
    while let Ok(event) = events.try_recv() {
        reporter.on_event(&event).await;
    }
    let output = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    let terminal: serde_json::Value = serde_json::from_str(output.lines().last().unwrap()).unwrap();
    assert_eq!(terminal["event"], "run_completed");
    terminal["status"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn excluded_jobs_make_a_run_that_executed_nothing_a_success() {
    assert_eq!(terminal_status(true).await, "success");
}

#[tokio::test]
async fn a_run_whose_every_skip_is_a_cache_hit_is_all_cached() {
    assert_eq!(terminal_status(false).await, "all_cached");
}
