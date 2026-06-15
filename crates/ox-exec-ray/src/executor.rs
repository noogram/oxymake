//! Ray executor — submits jobs to Ray clusters via the Ray Jobs API and polls
//! completion via HTTP.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use ox_core::model::{ConcreteJob, ExecutionBlock, JobId};
use ox_core::traits::executor::*;

use crate::autoscaler::{AutoscalerAdvisor, aggregate_cluster_resources};
use crate::call_mode;
use crate::error::RayError;
use crate::object_store;
use crate::ray_client::{JobSubmitRequest, RayClient, RayJobStatus};
use crate::resource_mapper;
use crate::runtime_env;

/// Configuration for the Ray executor.
#[derive(Debug, Clone)]
pub struct RayConfig {
    /// Ray dashboard address (default: `http://127.0.0.1:8265`).
    pub dashboard_address: String,
    /// Maximum concurrent submissions (rate limiting).
    pub max_submit: Option<usize>,
    /// Working directory on shared filesystem.
    pub working_dir: PathBuf,
    /// Minimum polling interval.
    pub poll_interval_min: Duration,
    /// Maximum polling interval.
    pub poll_interval_max: Duration,
    /// Extra entrypoint args appended to the job command.
    pub extra_args: Vec<String>,
    /// Optional metadata attached to all submitted jobs.
    pub metadata: HashMap<String, String>,
    /// Enable autoscaler-aware concurrency management.
    pub autoscaler_aware: bool,
    /// Minimum concurrency when autoscaler is active (floor).
    pub min_concurrency: usize,
}

impl Default for RayConfig {
    fn default() -> Self {
        Self {
            dashboard_address: "http://127.0.0.1:8265".to_string(),
            max_submit: None,
            working_dir: PathBuf::from("/tmp/oxymake-ray"),
            poll_interval_min: Duration::from_secs(2),
            poll_interval_max: Duration::from_secs(30),
            extra_args: vec![],
            metadata: HashMap::new(),
            autoscaler_aware: false,
            min_concurrency: 1,
        }
    }
}

/// Internal tracking info for a submitted Ray job.
#[derive(Debug, Clone)]
struct RayJobInfo {
    /// Ray submission ID returned by the Jobs API.
    submission_id: String,
    /// When the job was submitted (for timeout detection).
    #[allow(dead_code)]
    submitted_at: Instant,
}

/// Workspace state held between `prepare_workspace` and `finalize_workspace`.
#[derive(Debug)]
struct RayWorkspaceState {
    /// The staging directory for this job.
    #[allow(dead_code)]
    job_staging_dir: PathBuf,
    /// Path to the generated entrypoint script.
    #[allow(dead_code)]
    script_path: PathBuf,
}

/// Ray executor — submits OxyMake jobs to a Ray cluster via the Jobs API.
///
/// # Usage
///
/// ```no_run
/// use ox_exec_ray::{RayExecutor, RayConfig};
///
/// let config = RayConfig {
///     dashboard_address: "http://ray-head:8265".into(),
///     working_dir: "/shared/oxymake".into(),
///     ..RayConfig::default()
/// };
/// let executor = RayExecutor::new(config).expect("failed to create executor");
/// ```
#[derive(Debug)]
pub struct RayExecutor {
    config: RayConfig,
    /// HTTP client for the Ray Jobs API.
    client: RayClient,
    /// Maps OxyMake job IDs to Ray submission tracking info.
    running_jobs: Arc<Mutex<HashMap<String, RayJobInfo>>>,
    /// Autoscaler-aware concurrency advisor.
    autoscaler: AutoscalerAdvisor,
    /// Jobs to skip (cached) during DAG submission.
    /// Set via [`set_skip_jobs`] before calling [`submit_dag`].
    skip_jobs: Mutex<HashSet<JobId>>,
}

impl RayExecutor {
    /// Create a new Ray executor with the given configuration.
    ///
    /// Returns an error if the HTTP client cannot be constructed (e.g. TLS
    /// backend unavailable).
    pub fn new(config: RayConfig) -> Result<Self, RayError> {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        let client = RayClient::new(config.dashboard_address.clone(), http_client);
        let autoscaler = AutoscalerAdvisor::new(config.max_submit, config.min_concurrency);
        Ok(Self {
            config,
            client,
            running_jobs: Arc::new(Mutex::new(HashMap::new())),
            autoscaler,
            skip_jobs: Mutex::new(HashSet::new()),
        })
    }

    /// Create a Ray executor with a custom HTTP client (for testing).
    #[cfg(any(test, feature = "ray-integration"))]
    pub fn with_client(config: RayConfig, client: RayClient) -> Self {
        let autoscaler = AutoscalerAdvisor::new(config.max_submit, config.min_concurrency);
        Self {
            config,
            client,
            running_jobs: Arc::new(Mutex::new(HashMap::new())),
            autoscaler,
            skip_jobs: Mutex::new(HashSet::new()),
        }
    }

    /// Access the underlying Ray API client (for dashboard metrics, etc.).
    pub fn client(&self) -> &RayClient {
        &self.client
    }

    /// Access the autoscaler advisor.
    pub fn autoscaler(&self) -> &AutoscalerAdvisor {
        &self.autoscaler
    }

    /// Set the jobs to skip (cached) during the next DAG submission.
    ///
    /// Call this before `submit_dag()` to ensure cached jobs are omitted
    /// from the generated Ray driver script.
    pub async fn set_skip_jobs(&self, jobs: HashSet<JobId>) {
        *self.skip_jobs.lock().await = jobs;
    }

    /// Refresh autoscaler state from the cluster.
    ///
    /// Called automatically during `init()` and can be called manually
    /// to update concurrency recommendations.
    pub async fn refresh_autoscaler(&self) -> Result<(), RayError> {
        if !self.config.autoscaler_aware {
            return Ok(());
        }
        let nodes_resp = self.client.get_nodes().await?;
        let resources = aggregate_cluster_resources(&nodes_resp.data);
        self.autoscaler.update(resources).await;
        Ok(())
    }

    /// Build the entrypoint command from an execution block.
    fn build_entrypoint(
        job: &ConcreteJob,
        project_dir: &std::path::Path,
    ) -> Result<String, RayError> {
        let shell = job.shell_executable.as_deref().unwrap_or("/bin/bash");

        match &job.execution {
            ExecutionBlock::Shell { command } => {
                // Wrap the shell command so it runs from the project directory.
                Ok(format!(
                    "{shell} -c 'cd {project_dir} && {command}'",
                    project_dir = project_dir.display(),
                    command = command.replace('\'', "'\\''"),
                ))
            }
            ExecutionBlock::Script { path, lang: _ } => Ok(format!(
                "{shell} -c 'cd {project_dir} && {shell} {script}'",
                project_dir = project_dir.display(),
                script = path.display(),
            )),
            ExecutionBlock::Run { code, lang } => {
                // For inline code, we need to write a temp script.
                // The script is written during prepare_workspace; here we
                // generate the command that runs it.
                let interpreter = match lang.as_str() {
                    "python" | "py" => "python3",
                    "r" | "R" => "Rscript",
                    "julia" | "jl" => "julia",
                    _ => shell,
                };
                // The actual script path is set during prepare_workspace.
                // This is a placeholder that gets overridden.
                Ok(format!(
                    "cd {project_dir} && {interpreter} -c {code}",
                    project_dir = project_dir.display(),
                    code = shell_quote(code),
                ))
            }
            ExecutionBlock::Call { .. } => {
                // Call-mode entrypoint is built during prepare_workspace
                // (the wrapper script must be written to disk first).
                // Return a placeholder that gets overridden in execute().
                Ok("__CALL_MODE_PLACEHOLDER__".to_string())
            }
        }
    }
}

/// Shell-quote a string for safe embedding in a command.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl Executor for RayExecutor {
    type Error = RayError;

    async fn init(&self) -> Result<(), Self::Error> {
        // Verify Ray dashboard is reachable.
        let version = self.client.version().await?;
        tracing_log(&format!(
            "Ray executor initialized: Ray v{}",
            version.ray_version
        ));
        tracing_log(&format!("Dashboard: {}", self.config.dashboard_address));

        // Create working directory.
        tokio::fs::create_dir_all(&self.config.working_dir).await?;

        // Initialize autoscaler state if enabled.
        if self.config.autoscaler_aware {
            if let Err(e) = self.refresh_autoscaler().await {
                tracing_log(&format!(
                    "Warning: failed to initialize autoscaler state: {e}"
                ));
            }
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<(), Self::Error> {
        // Lightweight check that the Ray dashboard is reachable.
        let _version = self.client.version().await?;
        Ok(())
    }

    async fn cleanup(&self) -> Result<(), Self::Error> {
        // Stop all tracked running jobs.
        let running = self.running_jobs.lock().await;
        for info in running.values() {
            if let Err(e) = self.client.stop_job(&info.submission_id).await {
                tracing_log(&format!(
                    "Warning: failed to stop Ray job {}: {e}",
                    info.submission_id
                ));
            }
        }
        Ok(())
    }

    fn capabilities(&self) -> ExecutorCapabilities {
        ExecutorCapabilities {
            supports_gpu: true,
            supports_streaming: false,
            supports_shadow_dirs: false,
            supports_memory_passing: true,
            max_timeout: None,
            supports_job_arrays: true,
            supports_dag_submission: true,
        }
    }

    fn max_concurrency(&self) -> Option<usize> {
        // When autoscaler is not enabled, return the static config.
        // When enabled, the caller should use autoscaler().recommended_concurrency()
        // for dynamic values, but we still return the base as a ceiling.
        self.config.max_submit
    }

    async fn prepare_workspace(
        &self,
        job: &ConcreteJob,
        ctx: &ExecContext,
    ) -> Result<Workspace, Self::Error> {
        // Create per-job staging directory.
        let run_staging = self.config.working_dir.join(&ctx.run_id);
        let job_staging = run_staging.join(job.id.as_str());
        tokio::fs::create_dir_all(&job_staging).await?;

        // Generate the entrypoint script based on execution block type.
        let script_path = match &job.execution {
            ExecutionBlock::Run { code, lang } => {
                let interpreter = match lang.as_str() {
                    "python" | "py" => "python3",
                    "r" | "R" => "Rscript",
                    "julia" | "jl" => "julia",
                    _ => job.shell_executable.as_deref().unwrap_or("/bin/bash"),
                };
                let path = job_staging.join("entrypoint.sh");
                let script = format!("#!/usr/bin/env {interpreter}\n{code}\n");
                tokio::fs::write(&path, &script).await?;
                path
            }
            ExecutionBlock::Call { .. } => {
                // Generate a Python wrapper script for call-mode execution.
                let wrapper = call_mode::generate_wrapper(job)?;
                let path = job_staging.join("call_wrapper.py");
                tokio::fs::write(&path, &wrapper).await?;
                path
            }
            _ => job_staging.join("entrypoint.sh"),
        };

        Ok(Workspace::with_state(
            job_staging.clone(),
            RayWorkspaceState {
                job_staging_dir: job_staging,
                script_path,
            },
        ))
    }

    async fn execute(
        &self,
        job: &ConcreteJob,
        workspace: &Workspace,
        ctx: &ExecContext,
    ) -> Result<JobResult, Self::Error> {
        // Build the entrypoint command.
        let mut entrypoint = match &job.execution {
            ExecutionBlock::Call { .. } => {
                // Call-mode: run the generated Python wrapper script.
                let script_path = workspace.work_dir.join("call_wrapper.py");
                call_mode::wrapper_command(&script_path)
            }
            ExecutionBlock::Run { lang, .. } => {
                let interpreter = match lang.as_str() {
                    "python" | "py" => "python3",
                    "r" | "R" => "Rscript",
                    "julia" | "jl" => "julia",
                    _ => job.shell_executable.as_deref().unwrap_or("/bin/bash"),
                };
                let script_path = workspace.work_dir.join("entrypoint.sh");
                format!(
                    "cd {} && {} {}",
                    ctx.project_dir.display(),
                    interpreter,
                    script_path.display(),
                )
            }
            _ => Self::build_entrypoint(job, &ctx.project_dir)?,
        };

        // Append extra args if configured.
        if !self.config.extra_args.is_empty() {
            entrypoint.push(' ');
            entrypoint.push_str(&self.config.extra_args.join(" "));
        }

        // Map resources.
        let resources = resource_mapper::map_resources(&job.resources);

        // Build runtime_env from environment spec + memory resources.
        let env_runtime = job
            .environment
            .as_ref()
            .and_then(runtime_env::env_spec_to_runtime_env);

        let mem_runtime = resources.memory_bytes.map(runtime_env::memory_runtime_env);

        let merged_runtime = runtime_env::merge_runtime_env(env_runtime, mem_runtime);

        // For Nix/Apptainer, wrap the entrypoint command since Ray doesn't
        // natively support these environment types.
        if let Some(env) = &job.environment {
            match env {
                ox_core::model::EnvSpec::Nix { expr } => {
                    entrypoint = format!(
                        "nix develop {expr} --command {shell_quote}",
                        shell_quote = shell_quote(&entrypoint)
                    );
                }
                ox_core::model::EnvSpec::Apptainer { image } => {
                    entrypoint = format!("apptainer exec {image} {entrypoint}");
                }
                _ => {} // Handled via runtime_env above.
            }
        }

        // Build submission request.
        let mut metadata = self.config.metadata.clone();
        metadata.insert("oxymake_job_id".to_string(), job.id.to_string());
        metadata.insert("oxymake_rule".to_string(), job.rule.to_string());
        metadata.insert("oxymake_run_id".to_string(), ctx.run_id.clone());

        // Build runtime_env with environment variables for call-mode jobs.
        let runtime_env = if matches!(job.execution, ExecutionBlock::Call { .. }) {
            let mut env_vars = serde_json::Map::new();
            env_vars.insert(
                "OXYMAKE_WORKSPACE".to_string(),
                serde_json::Value::String(workspace.work_dir.display().to_string()),
            );

            // Object refs for InMemory inputs are passed via the job's params.
            // The scheduler populates these from the upstream job's object manifest.
            for (i, input) in job.inputs.iter().enumerate() {
                if let ox_core::model::OutputRef::InMemory { .. } = &input.reference {
                    let env_key = format!("OXYMAKE_OBJREF_{i}");
                    if let Some(val) = job.params.get(&env_key) {
                        env_vars.insert(env_key, serde_json::Value::String(val.clone()));
                    }
                }
            }

            let mut runtime = serde_json::Map::new();
            runtime.insert("env_vars".to_string(), serde_json::Value::Object(env_vars));
            Some(serde_json::Value::Object(runtime))
        } else {
            None
        };

        let request = JobSubmitRequest {
            entrypoint,
            submission_id: None,
            entrypoint_num_cpus: resources.num_cpus,
            entrypoint_num_gpus: resources.num_gpus,
            entrypoint_resources: if resources.custom.is_empty() {
                None
            } else {
                Some(resources.custom)
            },
            runtime_env: runtime_env.or(merged_runtime),
            metadata: Some(metadata),
        };

        // Submit to Ray.
        let submit_response = self.client.submit_job(&request).await?;
        let submission_id = submit_response.submission_id;
        tracing_log(&format!("Submitted {} as Ray job {submission_id}", job.id));

        // Track the job.
        {
            let mut running = self.running_jobs.lock().await;
            running.insert(
                job.id.to_string(),
                RayJobInfo {
                    submission_id: submission_id.clone(),
                    submitted_at: Instant::now(),
                },
            );
        }

        // Poll until terminal state with adaptive backoff.
        let start = Instant::now();
        let mut interval = self.config.poll_interval_min;
        loop {
            tokio::time::sleep(interval).await;

            // Opportunistically refresh autoscaler state during long polls.
            if self.config.autoscaler_aware && self.autoscaler.needs_refresh().await {
                let _ = self.refresh_autoscaler().await;
            }

            let details = self.client.get_job_details(&submission_id).await?;

            match details.status {
                RayJobStatus::Succeeded => {
                    let mut running = self.running_jobs.lock().await;
                    running.remove(job.id.as_str());

                    let log_path = ctx.log_dir.join(format!("{}.log", job.id));
                    return Ok(JobResult {
                        job_id: job.id.clone(),
                        exit_code: 0,
                        duration: start.elapsed(),
                        peak_memory_bytes: None,
                        cpu_time: None,
                        log_path: Some(log_path),
                        stderr_tail: None,
                    });
                }
                RayJobStatus::Failed => {
                    let mut running = self.running_jobs.lock().await;
                    running.remove(job.id.as_str());

                    let log_path = ctx.log_dir.join(format!("{}.log", job.id));
                    let stderr_tail = details.message.clone();
                    return Ok(JobResult {
                        job_id: job.id.clone(),
                        exit_code: 1,
                        duration: start.elapsed(),
                        peak_memory_bytes: None,
                        cpu_time: None,
                        log_path: Some(log_path),
                        stderr_tail,
                    });
                }
                RayJobStatus::Stopped => {
                    let mut running = self.running_jobs.lock().await;
                    running.remove(job.id.as_str());

                    return Ok(JobResult {
                        job_id: job.id.clone(),
                        exit_code: 137, // SIGKILL-like
                        duration: start.elapsed(),
                        peak_memory_bytes: None,
                        cpu_time: None,
                        log_path: None,
                        stderr_tail: Some("Job was stopped".into()),
                    });
                }
                RayJobStatus::Pending | RayJobStatus::Running => {
                    // Still running — adapt backoff.
                    if details.status == RayJobStatus::Running {
                        // Reset to min when we first see Running.
                        interval = self.config.poll_interval_min;
                    } else {
                        interval = (interval.mul_f64(1.5)).min(self.config.poll_interval_max);
                    }
                }
            }
        }
    }

    async fn finalize_workspace(
        &self,
        workspace: Workspace,
        result: &JobResult,
    ) -> Result<(), Self::Error> {
        // For successful call-mode jobs, read the object manifest.
        if result.exit_code == 0 {
            let manifest_path = workspace.work_dir.join(object_store::MANIFEST_FILENAME);
            if manifest_path.exists() {
                match object_store::read_manifest(&workspace.work_dir).await {
                    Ok(manifest) => {
                        tracing_log(&format!(
                            "Read object manifest with {} refs",
                            manifest.refs.len()
                        ));
                        // The manifest is available for the scheduler to read
                        // and pass object refs to downstream jobs. In a full
                        // integration, this would be returned via a channel or
                        // stored in the job result metadata.
                    }
                    Err(e) => {
                        tracing_log(&format!("Warning: failed to read object manifest: {e}"));
                    }
                }
            }
        }

        // Clean up the job staging directory.
        if let Err(e) = tokio::fs::remove_dir_all(&workspace.work_dir).await {
            tracing_log(&format!(
                "Warning: failed to clean staging dir {}: {e}",
                workspace.work_dir.display()
            ));
        }
        Ok(())
    }

    async fn cancel(&self, job_id: &JobId) -> Result<(), Self::Error> {
        let running = self.running_jobs.lock().await;
        if let Some(info) = running.get(job_id.as_str()) {
            self.client.stop_job(&info.submission_id).await?;
        }
        Ok(())
    }

    async fn poll_status(&self, job_id: &JobId) -> Result<JobStatus, Self::Error> {
        let running = self.running_jobs.lock().await;
        let info = running
            .get(job_id.as_str())
            .ok_or_else(|| RayError::JobNotTracked {
                job_id: job_id.to_string(),
            })?;
        let submission_id = info.submission_id.clone();
        drop(running);

        let details = self.client.get_job_details(&submission_id).await?;
        Ok(ray_status_to_job_status(&details.status))
    }

    async fn submit_dag(
        &self,
        graph: &ox_core::job_graph::JobGraph,
        ctx: &ExecContext,
    ) -> Result<DagSubmission, Self::Error> {
        use std::collections::HashMap;

        let topo_order = graph
            .topological_order()
            .map_err(|e| RayError::CallModeError(format!("topological sort failed: {e}")))?;

        let total_jobs = topo_order.len();

        // Read cached jobs to skip.
        let skip_jobs = self.skip_jobs.lock().await.clone();
        let skipped_count = topo_order
            .iter()
            .filter(|id| skip_jobs.contains(id))
            .count();
        let active_count = total_jobs - skipped_count;

        if active_count == 0 {
            return Ok(DagSubmission {
                run_id: ctx.run_id.clone(),
                total_jobs,
                submitted: 0,
                pending: 0,
                skipped: skipped_count,
                job_submissions: HashMap::new(),
            });
        }

        tracing_log(&format!(
            "Submitting DAG with {} active jobs ({} cached, {} total) as native Ray driver to {}",
            active_count, skipped_count, total_jobs, self.config.dashboard_address
        ));

        // Create a staging directory for the driver script and any
        // inline code / call-mode wrapper files it references.
        let run_staging = self.config.working_dir.join(&ctx.run_id);
        tokio::fs::create_dir_all(&run_staging).await?;

        // Generate a single Python driver script that encodes only the
        // uncached subgraph using @ray.remote tasks with ObjectRef
        // dependency chaining. Cached jobs are omitted — their outputs
        // already exist on disk.
        let driver_source =
            crate::driver_script::generate_driver(graph, ctx, &run_staging, &skip_jobs)?;

        let driver_path = run_staging.join("oxymake_dag_driver.py");
        tokio::fs::write(&driver_path, &driver_source).await?;

        // Write run metadata so `ox status` can detect the executor type
        // and poll Ray instead of relying solely on state.db.
        let meta = serde_json::json!({
            "executor": "ray",
            "ray_address": self.config.dashboard_address,
            "run_id": ctx.run_id,
            "total_jobs": total_jobs,
            "active_jobs": active_count,
            "skipped_jobs": skipped_count,
        });
        let meta_path = run_staging.join("meta.json");
        tokio::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).await?;

        tracing_log(&format!(
            "Generated DAG driver: {} ({} tasks)",
            driver_path.display(),
            active_count
        ));

        // Build metadata for the single driver job.
        let mut metadata = self.config.metadata.clone();
        metadata.insert("oxymake_run_id".to_string(), ctx.run_id.clone());
        metadata.insert("oxymake_dag_driver".to_string(), "true".to_string());
        metadata.insert(
            "oxymake_dag_total_jobs".to_string(),
            active_count.to_string(),
        );

        // Submit the driver as a single Ray job.
        let entrypoint = format!("python3 {}", driver_path.display());
        let request = JobSubmitRequest {
            entrypoint,
            submission_id: None,
            entrypoint_num_cpus: Some(1.0),
            entrypoint_num_gpus: None,
            entrypoint_resources: None,
            runtime_env: None,
            metadata: Some(metadata),
        };

        let submit_response = self.client.submit_job(&request).await?;
        let submission_id = submit_response.submission_id.clone();

        tracing_log(&format!(
            "DAG driver submitted as Ray job {submission_id} ({active_count} active tasks, {skipped_count} cached)"
        ));

        // Update meta.json with the Ray job submission ID.
        let meta = serde_json::json!({
            "executor": "ray",
            "ray_address": self.config.dashboard_address,
            "ray_job_id": submission_id,
            "run_id": ctx.run_id,
            "total_jobs": total_jobs,
            "active_jobs": active_count,
            "skipped_jobs": skipped_count,
        });
        let _ = tokio::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).await;

        // Track the driver job so poll_status() and cancel() work. The
        // driver is a single Ray job carrying the whole DAG, so every
        // uncached job ID is indexed to the same driver submission —
        // cancelling any job by its OxyMake ID cascades to `ray job stop`
        // on the driver (and thereby all its tasks).
        {
            let mut running = self.running_jobs.lock().await;
            running.insert(
                ctx.run_id.clone(),
                RayJobInfo {
                    submission_id: submission_id.clone(),
                    submitted_at: Instant::now(),
                },
            );
            for job_id in &topo_order {
                if !skip_jobs.contains(job_id) {
                    running.insert(
                        job_id.to_string(),
                        RayJobInfo {
                            submission_id: submission_id.clone(),
                            submitted_at: Instant::now(),
                        },
                    );
                }
            }
        }

        // Map: only uncached job IDs point to the driver submission ID.
        // Cached jobs are not submitted and have no submission ID.
        let mut job_submissions: HashMap<String, String> = HashMap::new();
        for job_id in &topo_order {
            if !skip_jobs.contains(job_id) {
                job_submissions.insert(job_id.to_string(), submission_id.clone());
            }
        }

        tracing_log(&format!(
            "DAG submission complete: 1 driver job for {} tasks (run_id: {})",
            active_count, ctx.run_id
        ));

        Ok(DagSubmission {
            run_id: ctx.run_id.clone(),
            total_jobs,
            submitted: 1,
            pending: active_count.saturating_sub(1),
            skipped: skipped_count,
            job_submissions,
        })
    }
}

/// Convert a Ray job status to an OxyMake `JobStatus`.
fn ray_status_to_job_status(status: &RayJobStatus) -> JobStatus {
    match status {
        RayJobStatus::Pending => JobStatus::Queued,
        RayJobStatus::Running => JobStatus::Running,
        RayJobStatus::Succeeded => JobStatus::Completed,
        RayJobStatus::Failed => JobStatus::Failed("Ray job failed".into()),
        RayJobStatus::Stopped => JobStatus::Cancelled,
    }
}

/// Simple logging helper.
fn tracing_log(msg: &str) {
    eprintln!("[ox-exec-ray] {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::model::*;
    use std::collections::BTreeMap;

    /// Helper to create a minimal ConcreteJob for testing.
    fn test_job(id: &str, command: &str) -> ConcreteJob {
        ConcreteJob {
            id: JobId::from(id),
            rule: RuleName::from("test-rule"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![],
            outputs: vec![],
            execution: ExecutionBlock::Shell {
                command: command.to_string(),
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

    #[test]
    fn test_build_entrypoint_shell() {
        let job = test_job("job-1", "echo hello");
        let entrypoint =
            RayExecutor::build_entrypoint(&job, std::path::Path::new("/proj")).unwrap();
        assert!(entrypoint.contains("echo hello"));
        assert!(entrypoint.contains("/proj"));
    }

    #[test]
    fn test_build_entrypoint_call_returns_placeholder() {
        let mut job = test_job("job-2", "");
        job.execution = ExecutionBlock::Call {
            function: "my_mod:func".into(),
            lang: "python".into(),
        };
        let result = RayExecutor::build_entrypoint(&job, std::path::Path::new("/proj"));
        // Call blocks return a placeholder; the actual entrypoint is built during execute().
        assert!(result.is_ok());
        assert!(result.unwrap().contains("CALL_MODE_PLACEHOLDER"));
    }

    #[test]
    fn test_ray_status_mapping() {
        assert_eq!(
            ray_status_to_job_status(&RayJobStatus::Pending),
            JobStatus::Queued
        );
        assert_eq!(
            ray_status_to_job_status(&RayJobStatus::Running),
            JobStatus::Running
        );
        assert_eq!(
            ray_status_to_job_status(&RayJobStatus::Succeeded),
            JobStatus::Completed
        );
        assert_eq!(
            ray_status_to_job_status(&RayJobStatus::Stopped),
            JobStatus::Cancelled
        );
        assert!(matches!(
            ray_status_to_job_status(&RayJobStatus::Failed),
            JobStatus::Failed(_)
        ));
    }

    #[tokio::test]
    async fn test_ray_config_default() {
        let config = RayConfig::default();
        assert_eq!(config.dashboard_address, "http://127.0.0.1:8265");
        assert_eq!(config.poll_interval_min, Duration::from_secs(2));
        assert_eq!(config.poll_interval_max, Duration::from_secs(30));
    }

    #[test]
    fn test_new_returns_result() {
        let config = RayConfig::default();
        let result = RayExecutor::new(config);
        assert!(
            result.is_ok(),
            "RayExecutor::new should return Ok with default config"
        );
    }
}
