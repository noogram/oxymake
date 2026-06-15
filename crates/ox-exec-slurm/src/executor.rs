//! SLURM executor — submits jobs to SLURM clusters via `sbatch` and polls
//! completion via `sacct`/`squeue`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use ox_core::event::EventBus;
use ox_core::model::{ConcreteJob, Event, JobId};
use ox_core::traits::executor::*;

use crate::error::SlurmError;
use crate::job_array::{JobArrayConfig, JobArrayInfo, JobArraySpec};
use crate::slurm_rest::SlurmRestClient;
use crate::status_parser;
use crate::{job_script, slurm_cli, slurm_rest};

/// Configuration for the SLURM executor.
#[derive(Debug, Clone)]
pub struct SlurmConfig {
    /// SLURM partition to submit to (default: cluster default).
    pub partition: Option<String>,
    /// SLURM account for resource accounting.
    pub account: Option<String>,
    /// Maximum concurrent submitted jobs (rate limiting).
    pub max_submit: Option<usize>,
    /// Base directory for job scripts and logs (must be on shared filesystem).
    pub staging_dir: PathBuf,
    /// Minimum polling interval for adaptive backoff.
    pub poll_interval_min: Duration,
    /// Maximum polling interval for adaptive backoff.
    pub poll_interval_max: Duration,
    /// Extra `sbatch` flags passed through verbatim.
    pub extra_flags: Vec<String>,
    /// SLURM QOS for job submissions.
    pub qos: Option<String>,
    /// Job array configuration for wildcard-expanded rules.
    pub job_array: JobArrayConfig,
    /// slurmrestd API URL. When set, the executor uses the REST API instead of
    /// CLI commands (sbatch/sacct/squeue). Example: `http://localhost:6820`.
    pub api_url: Option<String>,
    /// Shell command to execute to obtain a JWT token before REST submissions.
    /// The command's stdout (trimmed) is used as the token value.
    /// Example: `"scontrol token lifespan=3600"` or `"gcloud auth print-access-token"`.
    pub token_cmd: Option<String>,
}

impl Default for SlurmConfig {
    fn default() -> Self {
        Self {
            partition: None,
            account: None,
            max_submit: None,
            staging_dir: PathBuf::from("/tmp/oxymake-slurm"),
            poll_interval_min: Duration::from_secs(5),
            poll_interval_max: Duration::from_secs(60),
            extra_flags: vec![],
            qos: None,
            job_array: JobArrayConfig::default(),
            api_url: None,
            token_cmd: None,
        }
    }
}

/// Internal tracking info for a submitted SLURM job.
#[derive(Debug, Clone)]
struct SlurmJobInfo {
    slurm_job_id: u32,
    #[allow(dead_code)] // Used in production for log collection
    script_path: PathBuf,
    #[allow(dead_code)] // Used for timeout detection
    submitted_at: Instant,
}

/// Workspace state held between `prepare_workspace` and `finalize_workspace`.
#[derive(Debug)]
#[allow(dead_code)] // Fields used in finalize_workspace production impl
struct SlurmWorkspaceState {
    script_path: PathBuf,
    job_staging_dir: PathBuf,
}

/// SLURM executor — submits OxyMake jobs to a SLURM cluster.
///
/// # Usage
///
/// ```no_run
/// use ox_exec_slurm::{SlurmExecutor, SlurmConfig};
/// use ox_core::event::EventBus;
///
/// let config = SlurmConfig {
///     partition: Some("gpu".into()),
///     account: Some("my-lab".into()),
///     staging_dir: "/scratch/oxymake".into(),
///     ..SlurmConfig::default()
/// };
/// let executor = SlurmExecutor::new(config, EventBus::new());
/// ```
#[derive(Debug)]
pub struct SlurmExecutor {
    config: SlurmConfig,
    /// Maps OxyMake job IDs to SLURM job tracking info.
    running_jobs: Arc<Mutex<HashMap<String, SlurmJobInfo>>>,
    /// Nodes that failed during this run — excluded from future submissions.
    excluded_nodes: Arc<Mutex<HashSet<String>>>,
    /// Active job arrays: parent SLURM ID → array info.
    running_arrays: Arc<Mutex<HashMap<u32, JobArrayInfo>>>,
    /// Event bus for structured diagnostic output.
    event_bus: EventBus,
    /// Jobs to skip (cached) during DAG submission.
    /// Set via [`set_skip_jobs`] before calling [`submit_dag`].
    skip_jobs: Mutex<HashSet<JobId>>,
    /// REST API client (present when `config.api_url` is set).
    rest_client: Option<SlurmRestClient>,
}

/// Resolve a JWT token for slurmrestd authentication.
///
/// Priority: `token_cmd` output > `SLURM_JWT` env var > `None`.
fn resolve_token(token_cmd: &Option<String>) -> Option<String> {
    if let Some(cmd) = token_cmd {
        match std::process::Command::new("sh").args(["-c", cmd]).output() {
            Ok(output) if output.status.success() => {
                let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !token.is_empty() {
                    return Some(token);
                }
                eprintln!("warning: token_cmd produced empty output, falling back to SLURM_JWT");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!(
                    "warning: token_cmd failed (exit {}): {stderr}",
                    output.status
                );
            }
            Err(e) => {
                eprintln!("warning: failed to execute token_cmd: {e}");
            }
        }
    }
    std::env::var("SLURM_JWT").ok()
}

impl SlurmExecutor {
    /// Create a new SLURM executor with the given configuration and event bus.
    ///
    /// If `config.api_url` is set, the executor creates a REST client for
    /// slurmrestd. Token resolution order:
    /// 1. `config.token_cmd` — executed to obtain a fresh JWT token
    /// 2. `SLURM_JWT` environment variable
    /// 3. No token (unauthenticated)
    pub fn new(config: SlurmConfig, event_bus: EventBus) -> Self {
        let rest_client = config.api_url.as_ref().map(|url| {
            let user = std::env::var("USER")
                .or_else(|_| std::env::var("SLURM_USER"))
                .unwrap_or_else(|_| "root".to_string());
            let token = resolve_token(&config.token_cmd);
            SlurmRestClient::new(url.clone(), user, token)
        });
        Self {
            config,
            running_jobs: Arc::new(Mutex::new(HashMap::new())),
            excluded_nodes: Arc::new(Mutex::new(HashSet::new())),
            running_arrays: Arc::new(Mutex::new(HashMap::new())),
            event_bus,
            skip_jobs: Mutex::new(HashSet::new()),
            rest_client,
        }
    }

    /// Whether this executor is using the REST API mode.
    pub fn is_rest_mode(&self) -> bool {
        self.rest_client.is_some()
    }

    /// Set the jobs to skip (cached) during the next DAG submission.
    ///
    /// Call this before `submit_dag()` to ensure cached jobs are omitted
    /// from the submission. Only uncached jobs will be submitted via `sbatch`.
    pub async fn set_skip_jobs(&self, jobs: HashSet<JobId>) {
        *self.skip_jobs.lock().await = jobs;
    }

    /// Emit a structured diagnostic message on the event bus.
    fn emit_message(&self, message: impl Into<String>) {
        self.event_bus.emit(Event::ExecutorMessage {
            executor: "slurm".into(),
            message: message.into(),
        });
    }

    /// Get the list of excluded nodes (for reporting at workflow end).
    pub async fn excluded_nodes(&self) -> Vec<String> {
        self.excluded_nodes.lock().await.iter().cloned().collect()
    }

    /// Whether job array mode is enabled and the spec qualifies.
    ///
    /// A spec qualifies if it has more than one job and doesn't exceed the
    /// configured maximum array size.
    pub fn should_use_array(&self, spec: &JobArraySpec) -> bool {
        if !self.config.job_array.enabled || spec.len() <= 1 {
            return false;
        }
        if let Some(max) = self.config.job_array.max_array_size {
            spec.len() <= max
        } else {
            true
        }
    }

    /// Submit a job array to SLURM.
    ///
    /// Generates an array job script + params file, writes them to the staging
    /// directory, and submits via `sbatch`. Returns the parent SLURM job ID
    /// and the mapping from task indices to OxyMake job IDs.
    pub async fn submit_job_array(
        &self,
        spec: &JobArraySpec,
        ctx: &ExecContext,
    ) -> Result<JobArrayInfo, SlurmError> {
        let run_staging = self.config.staging_dir.join(&ctx.run_id);
        let array_staging = run_staging.join(format!("array_{}", spec.rule));
        tokio::fs::create_dir_all(&array_staging).await?;

        let excluded: Vec<String> = self.excluded_nodes.lock().await.iter().cloned().collect();
        let (script_content, params_content) = job_script::generate_array(
            spec,
            &self.config,
            &array_staging,
            &excluded,
            &ctx.project_dir,
            self.config.job_array.max_concurrent,
        )?;

        // Write script and params files.
        let script_path = array_staging.join("job_array.sh");
        let params_path = array_staging.join("array_params.jsonl");
        tokio::fs::write(&script_path, &script_content).await?;
        tokio::fs::write(&params_path, &params_content).await?;

        // Submit via REST or CLI.
        let slurm_job_id = if let Some(ref client) = self.rest_client {
            let out_path = array_staging.join("slurm-%j_%a.out");
            let err_path = array_staging.join("slurm-%j_%a.err");
            let array_flag = spec.array_flag(self.config.job_array.max_concurrent);
            let request = slurm_rest::build_array_submit_request(&slurm_rest::ArraySubmitParams {
                script: &script_content,
                job_name: &format!("ox_array_{}", spec.rule),
                config: &self.config,
                stdout_path: &out_path.display().to_string(),
                stderr_path: &err_path.display().to_string(),
                excluded_nodes: &excluded,
                working_dir: &ctx.project_dir.display().to_string(),
                array_spec: &array_flag,
                env_vars: &std::collections::HashMap::new(),
            });
            client.submit_job(&request).await?
        } else {
            slurm_cli::sbatch(&script_path).await?
        };
        self.emit_message(format!(
            "Submitted job array for rule '{}' as SLURM job {slurm_job_id} ({} tasks)",
            spec.rule.as_str(),
            spec.len()
        ));

        // Build task mapping.
        let task_job_ids: Vec<(usize, JobId)> = spec
            .jobs
            .iter()
            .enumerate()
            .map(|(i, j)| (i, j.id.clone()))
            .collect();

        let info = JobArrayInfo {
            slurm_job_id,
            task_job_ids: task_job_ids.clone(),
            staging_dir: array_staging,
        };

        // Track the array.
        {
            let mut arrays = self.running_arrays.lock().await;
            arrays.insert(slurm_job_id, info.clone());
        }

        // Also track individual tasks in running_jobs for poll_status compatibility.
        {
            let mut running = self.running_jobs.lock().await;
            for (_, job_id) in &task_job_ids {
                running.insert(
                    job_id.to_string(),
                    SlurmJobInfo {
                        slurm_job_id,
                        script_path: script_path.clone(),
                        submitted_at: Instant::now(),
                    },
                );
            }
        }

        Ok(info)
    }

    /// Poll status of all tasks in a job array via sacct.
    ///
    /// Returns a map from array task index → (OxyMake JobId, SLURM state string).
    /// Only includes tasks that have sacct records (pending tasks may not appear).
    pub async fn poll_array_status(
        &self,
        parent_job_id: u32,
    ) -> Result<HashMap<usize, (JobId, String)>, SlurmError> {
        let arrays = self.running_arrays.lock().await;
        let info = arrays
            .get(&parent_job_id)
            .ok_or(SlurmError::JobNotTracked {
                job_id: format!("array_{parent_job_id}"),
            })?
            .clone();
        drop(arrays);

        let mut results = HashMap::new();

        // Use REST or CLI to query per-task array status.
        let task_records = if let Some(ref client) = self.rest_client {
            client.get_array_tasks(parent_job_id).await?
        } else {
            slurm_cli::sacct_array(parent_job_id).await?
        };
        for (task_index, state) in task_records {
            if let Some((_, job_id)) = info.task_job_ids.iter().find(|(idx, _)| *idx == task_index)
            {
                results.insert(task_index, (job_id.clone(), state));
            }
        }

        Ok(results)
    }

    /// Cancel all tasks in a job array.
    pub async fn cancel_array(&self, parent_job_id: u32) -> Result<(), SlurmError> {
        if let Some(ref client) = self.rest_client {
            client.cancel_job(parent_job_id).await
        } else {
            slurm_cli::scancel(parent_job_id).await
        }
    }
}

impl Executor for SlurmExecutor {
    type Error = SlurmError;

    async fn init(&self) -> Result<(), Self::Error> {
        if let Some(ref client) = self.rest_client {
            // REST mode: verify slurmrestd is reachable.
            let version_info = client.check_available().await?;
            self.emit_message(format!("SLURM executor initialized (REST): {version_info}"));
        } else {
            // CLI mode: verify SLURM CLI is available.
            let version = slurm_cli::check_slurm_available().await?;
            self.emit_message(format!("SLURM executor initialized: {version}"));
        }

        // Create staging directory.
        tokio::fs::create_dir_all(&self.config.staging_dir).await?;
        Ok(())
    }

    async fn health_check(&self) -> Result<(), Self::Error> {
        if let Some(ref client) = self.rest_client {
            // REST mode: check nodes via API.
            client.health_check().await
        } else {
            // CLI mode: check that at least one node is available.
            let output = tokio::process::Command::new("sinfo")
                .args(["-N", "-h"])
                .output()
                .await
                .map_err(|e| SlurmError::ClusterUnreachable(format!("sinfo failed: {e}")))?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.trim().is_empty() {
                return Err(SlurmError::ClusterUnreachable("no nodes available".into()));
            }
            Ok(())
        }
    }

    async fn cleanup(&self) -> Result<(), Self::Error> {
        // Cancel all tracked individual jobs.
        let running = self.running_jobs.lock().await;
        for info in running.values() {
            if let Some(ref client) = self.rest_client {
                let _ = client.cancel_job(info.slurm_job_id).await;
            } else {
                let _ = slurm_cli::scancel(info.slurm_job_id).await;
            }
        }
        drop(running);

        // Cancel all tracked job arrays.
        let arrays = self.running_arrays.lock().await;
        for parent_id in arrays.keys() {
            if let Some(ref client) = self.rest_client {
                let _ = client.cancel_job(*parent_id).await;
            } else {
                let _ = slurm_cli::scancel(*parent_id).await;
            }
        }
        drop(arrays);

        // Report excluded nodes.
        let excluded = self.excluded_nodes.lock().await;
        if !excluded.is_empty() {
            let nodes: Vec<_> = excluded.iter().collect();
            self.emit_message(format!(
                "SLURM executor: {} nodes were excluded during this run: {}",
                nodes.len(),
                nodes
                    .iter()
                    .map(|n| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        Ok(())
    }

    fn capabilities(&self) -> ExecutorCapabilities {
        ExecutorCapabilities {
            supports_gpu: true,
            supports_streaming: false,
            supports_shadow_dirs: false,
            supports_memory_passing: false,
            max_timeout: None,
            supports_job_arrays: self.config.job_array.enabled,
            supports_dag_submission: true,
        }
    }

    fn max_concurrency(&self) -> Option<usize> {
        self.config.max_submit
    }

    async fn prepare_workspace(
        &self,
        job: &ConcreteJob,
        ctx: &ExecContext,
    ) -> Result<Workspace, Self::Error> {
        // Create per-job staging directory.
        let run_staging = self.config.staging_dir.join(&ctx.run_id);
        let job_staging = run_staging.join(job.id.as_str());
        tokio::fs::create_dir_all(&job_staging).await?;

        // Generate the sbatch script.
        let excluded: Vec<String> = self.excluded_nodes.lock().await.iter().cloned().collect();
        let script_content =
            job_script::generate(job, &self.config, &run_staging, &excluded, &ctx.project_dir)?;

        let script_path = job_staging.join("job.sh");
        tokio::fs::write(&script_path, &script_content).await?;

        Ok(Workspace::with_state(
            job_staging.clone(),
            SlurmWorkspaceState {
                script_path,
                job_staging_dir: job_staging,
            },
        ))
    }

    async fn execute(
        &self,
        job: &ConcreteJob,
        workspace: &Workspace,
        ctx: &ExecContext,
    ) -> Result<JobResult, Self::Error> {
        let script_path = workspace.work_dir.join("job.sh");

        // Submit the job (REST or CLI).
        let slurm_job_id = if let Some(ref client) = self.rest_client {
            let script_content = tokio::fs::read_to_string(&script_path).await?;
            let run_staging = self.config.staging_dir.join(&ctx.run_id);
            let out_path = run_staging.join(format!("{}/slurm-%j.out", job.id));
            let err_path = run_staging.join(format!("{}/slurm-%j.err", job.id));
            let excluded: Vec<String> = self.excluded_nodes.lock().await.iter().cloned().collect();
            let request = slurm_rest::build_submit_request(&slurm_rest::SubmitParams {
                script: &script_content,
                job_name: &format!("ox_{}_{}", job.rule, job.id),
                config: &self.config,
                stdout_path: &out_path.display().to_string(),
                stderr_path: &err_path.display().to_string(),
                excluded_nodes: &excluded,
                working_dir: &ctx.project_dir.display().to_string(),
                env_vars: &std::collections::HashMap::new(),
            });
            client.submit_job(&request).await?
        } else {
            slurm_cli::sbatch(&script_path).await?
        };
        self.emit_message(format!("Submitted {} as SLURM job {slurm_job_id}", job.id));

        // Track the job.
        {
            let mut running = self.running_jobs.lock().await;
            running.insert(
                job.id.to_string(),
                SlurmJobInfo {
                    slurm_job_id,
                    script_path: script_path.clone(),
                    submitted_at: Instant::now(),
                },
            );
        }

        // Poll until terminal state with adaptive backoff.
        let mut interval = self.config.poll_interval_min;
        loop {
            tokio::time::sleep(interval).await;

            let record = if let Some(ref client) = self.rest_client {
                // REST mode: poll via the REST API.
                match client.get_job(slurm_job_id).await {
                    Ok(Some(info)) => Some(SlurmRestClient::job_info_to_sacct_record(&info)),
                    Ok(None) => None,
                    Err(_) => None,
                }
            } else {
                // CLI mode: try sacct first.
                let records = slurm_cli::sacct(&[slurm_job_id]).await;
                match records {
                    Ok(ref recs) if !recs.is_empty() => Some(recs[0].clone()),
                    _ => None,
                }
            };

            if let Some(record) = record {
                if status_parser::is_terminal(&record.state) {
                    // Handle node failure — add to exclusion set.
                    if status_parser::is_node_failure(&record.state) && !record.node.is_empty() {
                        let mut excluded = self.excluded_nodes.lock().await;
                        excluded.insert(record.node.clone());
                        self.emit_message(format!(
                            "Node {} failed for SLURM job {slurm_job_id} — excluded",
                            record.node
                        ));
                    }

                    // Remove from tracking.
                    {
                        let mut running = self.running_jobs.lock().await;
                        running.remove(job.id.as_str());
                    }

                    let log_path = ctx.log_dir.join(format!("{}.log", job.id));

                    return Ok(record_to_job_result(&job.id, &record, log_path));
                }
                // Job still running — reset backoff on state change.
                interval = self.config.poll_interval_min;
            } else if self.rest_client.is_some() {
                // REST mode: no result means job may not be visible yet.
                interval = (interval.mul_f64(1.5)).min(self.config.poll_interval_max);
            } else {
                // CLI mode: sacct failed or empty — try squeue fallback.
                match slurm_cli::squeue(slurm_job_id).await {
                    Ok(Some(_state)) => {
                        // Job is still in queue — increase backoff.
                        interval = (interval.mul_f64(1.5)).min(self.config.poll_interval_max);
                    }
                    Ok(None) => {
                        // Not in squeue AND not in sacct — job may have completed
                        // between our queries. Wait one more cycle and retry sacct.
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        let retry = slurm_cli::sacct(&[slurm_job_id]).await;
                        if let Ok(recs) = retry {
                            if let Some(record) = recs.into_iter().next() {
                                let mut running = self.running_jobs.lock().await;
                                running.remove(job.id.as_str());

                                let log_path = ctx.log_dir.join(format!("{}.log", job.id));
                                return Ok(record_to_job_result(&job.id, &record, log_path));
                            }
                        }
                        // Truly lost — report as failed.
                        let mut running = self.running_jobs.lock().await;
                        running.remove(job.id.as_str());
                        return Err(SlurmError::JobNotFound { slurm_job_id });
                    }
                    Err(_) => {
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
        // Determine the target log directory from the result's log_path.
        let log_dir = result
            .log_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());

        if let Some(log_dir) = log_dir {
            tokio::fs::create_dir_all(&log_dir).await?;

            // Collect SLURM stdout/stderr logs from the staging directory.
            let staging = &workspace.work_dir;
            let mut entries = tokio::fs::read_dir(staging).await?;
            while let Some(entry) = entries.next_entry().await? {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("slurm-")
                    && (name_str.ends_with(".out") || name_str.ends_with(".err"))
                {
                    let dest = log_dir.join(&name);
                    if let Err(e) = tokio::fs::copy(entry.path(), &dest).await {
                        self.emit_message(format!("Warning: failed to copy {}: {e}", name_str));
                    }
                }
            }
        }

        // Clean up the job staging directory.
        if let Err(e) = tokio::fs::remove_dir_all(&workspace.work_dir).await {
            self.emit_message(format!(
                "Warning: failed to clean staging dir {}: {e}",
                workspace.work_dir.display()
            ));
        }

        Ok(())
    }

    async fn cancel(&self, job_id: &JobId) -> Result<(), Self::Error> {
        let running = self.running_jobs.lock().await;
        if let Some(info) = running.get(job_id.as_str()) {
            if let Some(ref client) = self.rest_client {
                client.cancel_job(info.slurm_job_id).await?;
            } else {
                slurm_cli::scancel(info.slurm_job_id).await?;
            }
        }
        Ok(())
    }

    async fn poll_status(&self, job_id: &JobId) -> Result<JobStatus, Self::Error> {
        let running = self.running_jobs.lock().await;
        let info = running
            .get(job_id.as_str())
            .ok_or_else(|| SlurmError::JobNotTracked {
                job_id: job_id.to_string(),
            })?;
        let slurm_job_id = info.slurm_job_id;
        drop(running); // Release lock before I/O.

        if let Some(ref client) = self.rest_client {
            // REST mode: poll via the REST API.
            match client.get_job(slurm_job_id).await? {
                Some(info) => {
                    let record = SlurmRestClient::job_info_to_sacct_record(&info);
                    Ok(status_parser::slurm_state_to_job_status(&record.state))
                }
                None => Err(SlurmError::JobNotFound { slurm_job_id }),
            }
        } else {
            // CLI mode: try sacct first.
            if let Ok(records) = slurm_cli::sacct(&[slurm_job_id]).await {
                if let Some(record) = records.into_iter().next() {
                    return Ok(status_parser::slurm_state_to_job_status(&record.state));
                }
            }

            // Fallback to squeue.
            match slurm_cli::squeue(slurm_job_id).await? {
                Some(state) => Ok(status_parser::slurm_state_to_job_status(&state)),
                None => Err(SlurmError::JobNotFound { slurm_job_id }),
            }
        }
    }

    async fn submit_dag(
        &self,
        graph: &ox_core::job_graph::JobGraph,
        ctx: &ExecContext,
    ) -> Result<DagSubmission, Self::Error> {
        let topo_order = graph
            .topological_order()
            .map_err(|e| SlurmError::SubmitFailed(format!("topological sort failed: {e}")))?;

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

        let mode_label = if self.rest_client.is_some() {
            "slurmrestd"
        } else {
            "sbatch --dependency chains"
        };
        self.emit_message(format!(
            "Submitting DAG with {} active jobs ({} cached, {} total) via {mode_label}",
            active_count, skipped_count, total_jobs
        ));

        // Create staging directory for this run.
        let run_staging = self.config.staging_dir.join(&ctx.run_id);
        tokio::fs::create_dir_all(&run_staging).await?;

        // Maps OxyMake job ID → SLURM job ID (populated as we submit).
        let mut ox_to_slurm: HashMap<String, u32> = HashMap::new();
        let mut job_submissions: HashMap<String, String> = HashMap::new();
        let mut submitted_roots = 0usize;
        let mut pending_deps = 0usize;

        let excluded: Vec<String> = self.excluded_nodes.lock().await.iter().cloned().collect();

        // Submit in topological order so upstream SLURM IDs are known
        // before downstream jobs reference them via --dependency.
        for job_id in &topo_order {
            if skip_jobs.contains(job_id) {
                continue;
            }

            let job = graph.get_job(job_id).ok_or_else(|| {
                SlurmError::SubmitFailed(format!("job {} not found in graph", job_id))
            })?;

            // Generate the sbatch script for this job.
            let job_staging = run_staging.join(job_id.as_str());
            tokio::fs::create_dir_all(&job_staging).await?;

            let script_content =
                job_script::generate(job, &self.config, &run_staging, &excluded, &ctx.project_dir)?;
            let script_path = job_staging.join("job.sh");
            tokio::fs::write(&script_path, &script_content).await?;

            // Collect SLURM IDs of upstream dependencies that were actually
            // submitted (not cached). Cached upstream jobs are already done —
            // their outputs exist on disk — so we don't need to wait for them.
            let upstream = graph.upstream(job_id);
            let dep_slurm_ids: Vec<u32> = upstream
                .iter()
                .filter(|uid| !skip_jobs.contains(uid))
                .filter_map(|uid| ox_to_slurm.get(uid.as_str()).copied())
                .collect();

            // Submit via sbatch (CLI) or REST API with dependency chain.
            let slurm_job_id = if let Some(ref client) = self.rest_client {
                let out_path = run_staging.join(format!("{}/slurm-%j.out", job_id));
                let err_path = run_staging.join(format!("{}/slurm-%j.err", job_id));
                let request = slurm_rest::build_submit_request(&slurm_rest::SubmitParams {
                    script: &script_content,
                    job_name: &format!("ox_{}_{}", job.rule, job_id),
                    config: &self.config,
                    stdout_path: &out_path.display().to_string(),
                    stderr_path: &err_path.display().to_string(),
                    excluded_nodes: &excluded,
                    working_dir: &ctx.project_dir.display().to_string(),
                    env_vars: &std::collections::HashMap::new(),
                });
                client.submit_job_with_deps(request, &dep_slurm_ids).await?
            } else {
                slurm_cli::sbatch_with_deps(&script_path, &dep_slurm_ids).await?
            };

            if dep_slurm_ids.is_empty() {
                submitted_roots += 1;
            } else {
                pending_deps += 1;
            }

            self.emit_message(format!(
                "Submitted {} as SLURM job {slurm_job_id}{}",
                job_id,
                if dep_slurm_ids.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (depends on {})",
                        dep_slurm_ids
                            .iter()
                            .map(|id| id.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                }
            ));

            ox_to_slurm.insert(job_id.to_string(), slurm_job_id);
            job_submissions.insert(job_id.to_string(), slurm_job_id.to_string());

            // Track in running_jobs for poll_status() / cancel().
            {
                let mut running = self.running_jobs.lock().await;
                running.insert(
                    job_id.to_string(),
                    SlurmJobInfo {
                        slurm_job_id,
                        script_path,
                        submitted_at: Instant::now(),
                    },
                );
            }
        }

        // Write run metadata so `ox status` can detect the executor type.
        let meta = serde_json::json!({
            "executor": "slurm",
            "version": 1,
            "run_id": ctx.run_id,
            "total_jobs": total_jobs,
            "active_jobs": active_count,
            "skipped_jobs": skipped_count,
            "job_mapping": &job_submissions,
        });
        let meta_path = run_staging.join("meta.json");
        let _ = tokio::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).await;

        self.emit_message(format!(
            "DAG submission complete: {} jobs submitted ({} roots, {} with dependencies)",
            active_count, submitted_roots, pending_deps
        ));

        Ok(DagSubmission {
            run_id: ctx.run_id.clone(),
            total_jobs,
            submitted: submitted_roots,
            pending: pending_deps,
            skipped: skipped_count,
            job_submissions,
        })
    }
}

/// Build a [`JobResult`] from a terminal sacct record.
///
/// The terminal state is consulted BEFORE the exit code: any state other
/// than `COMPLETED` is a failure even when sacct reports exit code 0
/// (TIMEOUT → "0:15", OOM → "0:9", PREEMPTED → "0:0"). SLURM jobs have no
/// atomic-write protocol, so classifying a killed job as successful would
/// cache its partial output as a valid artifact.
fn record_to_job_result(
    job_id: &JobId,
    record: &slurm_cli::SacctRecord,
    log_path: PathBuf,
) -> JobResult {
    let failure = status_parser::terminal_failure(&record.state);
    let exit_code = match failure {
        Some(_) if record.exit_code == 0 => 1,
        _ => record.exit_code,
    };
    JobResult {
        job_id: job_id.clone(),
        exit_code,
        duration: record.elapsed,
        peak_memory_bytes: record.peak_memory_bytes,
        cpu_time: None,
        log_path: Some(log_path),
        stderr_tail: failure.map(|f| format!("SLURM terminal state {}: {f}", record.state)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slurm_cli::SacctRecord;

    fn record(state: &str, exit_code: i32) -> SacctRecord {
        SacctRecord {
            job_id: 42,
            state: state.into(),
            exit_code,
            peak_memory_bytes: Some(1024),
            elapsed: Duration::from_secs(3),
            node: "c1".into(),
        }
    }

    /// B12: a job killed by SLURM (TIMEOUT, OOM, PREEMPTED, CANCELLED)
    /// frequently reports exit code 0 in sacct ("0:15", "0:9", "0:0").
    /// Classifying success on the exit code alone caches the partial
    /// output of a killed job as a valid artifact. The terminal state
    /// must gate success BEFORE the exit code is consulted.
    #[test]
    fn killed_states_with_exit_zero_are_failures() {
        for state in [
            "TIMEOUT",
            "OUT_OF_MEMORY",
            "PREEMPTED",
            "CANCELLED",
            "NODE_FAIL",
        ] {
            let result = record_to_job_result(
                &JobId::from("j-1"),
                &record(state, 0),
                PathBuf::from("/logs/j-1.log"),
            );
            assert_ne!(
                result.exit_code, 0,
                "state {state} with sacct exit 0 must not be classified as success"
            );
            let tail = result.stderr_tail.expect("failure reason populated");
            assert!(
                tail.contains(state),
                "stderr_tail should name the state: {tail}"
            );
        }
    }

    #[test]
    fn completed_with_exit_zero_is_success() {
        let result = record_to_job_result(
            &JobId::from("j-2"),
            &record("COMPLETED", 0),
            PathBuf::from("/logs/j-2.log"),
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.stderr_tail.is_none());
        assert_eq!(result.duration, Duration::from_secs(3));
        assert_eq!(result.peak_memory_bytes, Some(1024));
    }

    #[test]
    fn failed_state_preserves_real_exit_code() {
        let result = record_to_job_result(
            &JobId::from("j-3"),
            &record("FAILED", 7),
            PathBuf::from("/logs/j-3.log"),
        );
        assert_eq!(result.exit_code, 7);
    }

    #[test]
    fn default_config() {
        let config = SlurmConfig::default();
        assert!(config.partition.is_none());
        assert_eq!(config.poll_interval_min, Duration::from_secs(5));
        assert_eq!(config.poll_interval_max, Duration::from_secs(60));
    }

    #[test]
    fn executor_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SlurmExecutor>();
    }

    #[test]
    fn capabilities_support_gpu() {
        let executor = SlurmExecutor::new(SlurmConfig::default(), EventBus::new());
        assert!(executor.capabilities().supports_gpu);
        assert!(!executor.capabilities().supports_memory_passing);
    }

    /// Regression test for ox-pfu: init() must fail when SLURM CLI tools
    /// (sinfo, sbatch) are not available on the system. Without this check,
    /// `ox run --executor slurm` silently succeeds with 0 jobs.
    #[tokio::test]
    async fn init_fails_when_slurm_not_available() {
        // On CI / dev machines without SLURM installed, init() must return
        // an error (ClusterUnreachable) rather than silently succeeding.
        let executor = SlurmExecutor::new(SlurmConfig::default(), EventBus::new());
        let result = executor.init().await;
        // If SLURM happens to be installed, init() succeeds — that's fine.
        // But if it's NOT installed (the common case), it must be an error.
        if std::process::Command::new("sinfo")
            .arg("--version")
            .output()
            .is_err()
        {
            assert!(
                result.is_err(),
                "init() should fail when sinfo is not found"
            );
            let err = result.unwrap_err();
            assert!(
                matches!(err, SlurmError::ClusterUnreachable(_)),
                "expected ClusterUnreachable, got: {err}"
            );
        }
    }
}
