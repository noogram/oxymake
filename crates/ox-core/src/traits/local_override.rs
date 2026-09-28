//! Per-job local routing for scheduler-driven runs.
//!
//! Both executors receive the same job and execution context. Files must
//! already be visible to both; this adapter does not transfer artifacts.
use std::collections::HashSet;

use super::executor::{
    DagSubmission, ExecContext, Executor, ExecutorCapabilities, JobResult, JobStatus, Workspace,
};
use crate::error::ExecError;
use crate::job_graph::JobGraph;
use crate::model::{ConcreteJob, JobId};

/// Route declared local jobs to a local executor and other jobs to the run's
/// executor. The route is fixed for the graph, including retries and shutdown.
#[derive(Debug)]
pub struct LocalOverrideExecutor<R, L> {
    remote: R,
    local: L,
    local_jobs: HashSet<JobId>,
}

impl<R: Executor, L: Executor> LocalOverrideExecutor<R, L> {
    /// Create an adapter for this graph. `local` must be a local executor.
    pub fn new(remote: R, local: L, graph: &JobGraph) -> Self {
        let local_jobs = graph
            .job_ids()
            .into_iter()
            .filter(|id| {
                graph
                    .get_job(id)
                    .is_some_and(|j| j.executor.as_deref() == Some("local"))
            })
            .cloned()
            .collect();
        Self {
            remote,
            local,
            local_jobs,
        }
    }
    fn is_local(&self, id: &JobId) -> bool {
        self.local_jobs.contains(id)
    }
}

/// Backend error preserved without adding another executor-error prefix.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct LocalOverrideError(Box<dyn std::error::Error + Send + Sync>);

fn error(e: impl std::error::Error + Send + Sync + 'static) -> LocalOverrideError {
    LocalOverrideError(Box::new(e))
}

impl<R: Executor, L: Executor> Executor for LocalOverrideExecutor<R, L> {
    type Error = LocalOverrideError;

    fn executor_name(&self, job: &ConcreteJob) -> &str {
        if self.is_local(&job.id) {
            self.local.executor_name(job)
        } else {
            self.remote.executor_name(job)
        }
    }
    fn uses_resource_budget(&self, job: &ConcreteJob) -> bool {
        self.is_local(&job.id) && self.local.uses_resource_budget(job)
    }
    async fn init(&self) -> Result<(), Self::Error> {
        self.remote.init().await.map_err(error)?;
        self.local.init().await.map_err(error)
    }
    async fn health_check(&self) -> Result<(), Self::Error> {
        self.remote.health_check().await.map_err(error)?;
        self.local.health_check().await.map_err(error)
    }
    async fn cleanup(&self) -> Result<(), Self::Error> {
        let remote = self.remote.cleanup().await.map_err(error);
        let local = self.local.cleanup().await.map_err(error);
        remote.and(local)
    }
    fn capabilities(&self) -> ExecutorCapabilities {
        // These are graph-wide promises: neither backend's capabilities can
        // establish that memory/streams/shadow workspaces cross the host
        // boundary. Keep the conservative per-job path; each backend still
        // handles its own GPU and timeout requests. Arrays and DAG submission
        // would bypass routing, so they must remain disabled.
        ExecutorCapabilities::default()
    }
    fn max_concurrency(&self) -> Option<usize> {
        // A single global cap cannot express independent backend limits.
        // The stricter cap is conservative even if all jobs route to one side.
        match (self.remote.max_concurrency(), self.local.max_concurrency()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
    async fn prepare_workspace(
        &self,
        job: &ConcreteJob,
        ctx: &ExecContext,
    ) -> Result<Workspace, Self::Error> {
        if self.is_local(&job.id) {
            self.local.prepare_workspace(job, ctx).await.map_err(error)
        } else {
            self.remote.prepare_workspace(job, ctx).await.map_err(error)
        }
    }
    async fn execute(
        &self,
        job: &ConcreteJob,
        workspace: &Workspace,
        ctx: &ExecContext,
    ) -> Result<JobResult, Self::Error> {
        if self.is_local(&job.id) {
            self.local.execute(job, workspace, ctx).await.map_err(error)
        } else {
            self.remote
                .execute(job, workspace, ctx)
                .await
                .map_err(error)
        }
    }
    async fn finalize_workspace(
        &self,
        workspace: Workspace,
        result: &JobResult,
    ) -> Result<(), Self::Error> {
        if self.is_local(&result.job_id) {
            self.local
                .finalize_workspace(workspace, result)
                .await
                .map_err(error)
        } else {
            self.remote
                .finalize_workspace(workspace, result)
                .await
                .map_err(error)
        }
    }
    async fn cancel(&self, id: &JobId) -> Result<(), Self::Error> {
        if self.is_local(id) {
            self.local.cancel(id).await.map_err(error)
        } else {
            self.remote.cancel(id).await.map_err(error)
        }
    }
    async fn kill(&self, id: &JobId) -> Result<(), Self::Error> {
        if self.is_local(id) {
            self.local.kill(id).await.map_err(error)
        } else {
            self.remote.kill(id).await.map_err(error)
        }
    }
    async fn poll_status(&self, id: &JobId) -> Result<JobStatus, Self::Error> {
        if self.is_local(id) {
            self.local.poll_status(id).await.map_err(error)
        } else {
            self.remote.poll_status(id).await.map_err(error)
        }
    }
    async fn submit_dag(
        &self,
        _graph: &JobGraph,
        _ctx: &ExecContext,
    ) -> Result<DagSubmission, Self::Error> {
        Err(error(ExecError::Executor {
            message: "local overrides require per-job scheduler execution, not DAG submission"
                .into(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventBus;
    use crate::job_graph::make_test_job;
    use crate::model::{Backoff, ErrorStrategy, Event};
    use crate::scheduler::{SchedulerConfig, run_scheduler};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Debug)]
    struct Witness {
        name: &'static str,
        calls: Arc<Mutex<Vec<String>>>,
    }
    impl Witness {
        fn record(&self, action: &str, id: &JobId) {
            self.calls.lock().unwrap().push(format!("{action}:{id}"));
        }
    }
    impl Executor for Witness {
        type Error = ExecError;
        fn executor_name(&self, _: &ConcreteJob) -> &str {
            self.name
        }
        async fn init(&self) -> Result<(), ExecError> {
            Ok(())
        }
        async fn health_check(&self) -> Result<(), ExecError> {
            Ok(())
        }
        async fn cleanup(&self) -> Result<(), ExecError> {
            Ok(())
        }
        fn capabilities(&self) -> ExecutorCapabilities {
            ExecutorCapabilities::default()
        }
        fn max_concurrency(&self) -> Option<usize> {
            Some(if self.name == "local" { 2 } else { 4 })
        }
        async fn prepare_workspace(
            &self,
            job: &ConcreteJob,
            _: &ExecContext,
        ) -> Result<Workspace, ExecError> {
            self.record("prepare", &job.id);
            Ok(Workspace::with_state(".".into(), self.name.to_string()))
        }
        async fn execute(
            &self,
            job: &ConcreteJob,
            ws: &Workspace,
            _: &ExecContext,
        ) -> Result<JobResult, ExecError> {
            assert_eq!(ws.state::<String>().unwrap(), self.name);
            self.record("execute", &job.id);
            let attempts = self
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|s| *s == &format!("execute:{}", job.id))
                .count();
            Ok(JobResult {
                job_id: job.id.clone(),
                exit_code: i32::from(self.name == "local" && attempts == 1),
                duration: Duration::ZERO,
                peak_memory_bytes: None,
                cpu_time: None,
                log_path: None,
                stderr_tail: None,
            })
        }
        async fn finalize_workspace(
            &self,
            ws: Workspace,
            result: &JobResult,
        ) -> Result<(), ExecError> {
            assert_eq!(ws.into_state::<String>().unwrap(), self.name);
            self.record("finalize", &result.job_id);
            Ok(())
        }
        async fn cancel(&self, id: &JobId) -> Result<(), ExecError> {
            self.record("cancel", id);
            Ok(())
        }
        async fn kill(&self, id: &JobId) -> Result<(), ExecError> {
            self.record("kill", id);
            Ok(())
        }
        async fn poll_status(&self, id: &JobId) -> Result<JobStatus, ExecError> {
            self.record("poll", id);
            Ok(JobStatus::Completed)
        }
        async fn submit_dag(
            &self,
            _: &JobGraph,
            _: &ExecContext,
        ) -> Result<DagSubmission, ExecError> {
            panic!("must never submit mixed DAG")
        }
    }

    fn context() -> ExecContext {
        ExecContext {
            global_job_limit: 1,
            run_id: "routing-test".into(),
            log_dir: ".".into(),
            project_dir: ".".into(),
            trusted_dirs: vec![],
            input_data: Default::default(),
            memory_map: None,
        }
    }

    #[test]
    fn adapter_keeps_backend_limits_and_scopes_admission() {
        let mut local = make_test_job("local", &[], &["a"]);
        local.executor = Some("local".into());
        let remote = make_test_job("remote", &[], &["b"]);
        let graph = JobGraph::build(vec![local.clone(), remote.clone()]).unwrap();
        let router = LocalOverrideExecutor::new(
            Witness {
                name: "slurm",
                calls: Default::default(),
            },
            Witness {
                name: "local",
                calls: Default::default(),
            },
            &graph,
        );
        assert_eq!(router.max_concurrency(), Some(2));
        assert!(router.uses_resource_budget(&local));
        assert!(!router.uses_resource_budget(&remote));
        let caps = router.capabilities();
        assert!(!caps.supports_dag_submission);
        assert!(!caps.supports_job_arrays);
        assert!(!caps.supports_memory_passing);
    }

    #[test]
    fn adapter_preserves_error_display() {
        let original = ExecError::Executor {
            message: "job timed out".into(),
        };
        let expected = original.to_string();
        assert_eq!(error(original).to_string(), expected);
    }

    #[tokio::test(start_paused = true)]
    async fn routes_entire_lifecycle_and_retries_to_selected_backend() {
        let mut local = make_test_job("on_host", &[], &["a"]);
        local.executor = Some("local".into());
        local.error_strategy = ErrorStrategy::Retry {
            count: 2,
            backoff: Backoff::Constant,
        };
        let remote = make_test_job("on_cluster", &["a"], &["b"]);
        let local_id = local.id.clone();
        let remote_id = remote.id.clone();
        let graph = JobGraph::build(vec![local, remote]).unwrap();
        let local_calls = Arc::new(Mutex::new(Vec::new()));
        let remote_calls = Arc::new(Mutex::new(Vec::new()));
        let router = Arc::new(LocalOverrideExecutor::new(
            Witness {
                name: "slurm",
                calls: remote_calls.clone(),
            },
            Witness {
                name: "local",
                calls: local_calls.clone(),
            },
            &graph,
        ));
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let result = run_scheduler(
            &graph,
            router.clone(),
            &SchedulerConfig::default(),
            &bus,
            &context(),
        )
        .await
        .unwrap();
        assert_eq!(
            result.succeeded,
            2,
            "{result:?} local={:?} remote={:?}",
            local_calls.lock().unwrap(),
            remote_calls.lock().unwrap()
        );
        for id in [&local_id, &remote_id] {
            router.cancel(id).await.unwrap();
            router.kill(id).await.unwrap();
            router.poll_status(id).await.unwrap();
        }
        for (calls, id, attempts) in [(&local_calls, local_id, 2), (&remote_calls, remote_id, 1)] {
            let calls = calls.lock().unwrap();
            for action in ["prepare", "execute", "finalize"] {
                assert_eq!(
                    calls
                        .iter()
                        .filter(|s| *s == &format!("{action}:{id}"))
                        .count(),
                    attempts
                );
            }
            for action in ["cancel", "kill", "poll"] {
                assert!(calls.contains(&format!("{action}:{id}")));
            }
            assert_eq!(calls.len(), attempts * 3 + 3, "{calls:?}");
        }
        let mut names = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let Event::JobStarted { executor, .. } = event {
                names.push(executor);
            }
        }
        assert_eq!(names, ["local", "local", "slurm"]);
    }

    #[tokio::test]
    async fn bare_remote_scheduler_refuses_local_override() {
        let mut job = make_test_job("on_host", &[], &["a"]);
        job.executor = Some("local".into());
        let graph = JobGraph::build(vec![job]).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let remote = Arc::new(Witness {
            name: "slurm",
            calls: calls.clone(),
        });
        let err = run_scheduler(
            &graph,
            remote,
            &SchedulerConfig::default(),
            &EventBus::new(),
            &context(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("local routing"), "{err}");
        assert!(calls.lock().unwrap().is_empty());
    }
}
