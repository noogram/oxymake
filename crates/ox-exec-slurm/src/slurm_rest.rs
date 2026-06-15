//! HTTP client for the SLURM REST API (`slurmrestd`).
//!
//! Wraps the slurmrestd v0.0.40+ endpoints:
//! - `POST /slurm/v0.0.40/job/submit` — submit a job
//! - `GET  /slurm/v0.0.40/jobs` — list jobs
//! - `GET  /slurm/v0.0.40/job/{job_id}` — get job status
//! - `DELETE /slurm/v0.0.40/job/{job_id}` — cancel a job
//! - `GET  /slurm/v0.0.40/nodes` — list nodes

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::SlurmError;
use crate::slurm_cli::SacctRecord;

/// Default SLURM REST API version prefix.
/// SLURM 25.11.x uses v0.0.44 (v0.0.40 removed, v0.0.41 deprecated).
const API_VERSION: &str = "slurm/v0.0.44";

/// Job submission request body for the slurmrestd submit endpoint.
#[derive(Debug, Serialize)]
pub struct JobSubmitRequest {
    /// The job script content (bash script with #SBATCH directives stripped).
    pub script: String,
    /// Job description fields.
    pub job: JobDescription,
}

/// Job description fields for the submit request.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct JobDescription {
    /// Job name.
    pub name: String,
    /// Partition to submit to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<String>,
    /// Account for resource billing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Quality of service.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qos: Option<String>,
    /// Working directory for the job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_working_directory: Option<String>,
    /// Standard output file path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standard_output: Option<String>,
    /// Standard error file path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standard_error: Option<String>,
    /// Environment variables as "KEY=VALUE" strings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<Vec<String>>,
    /// Nodes to exclude from scheduling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excluded_nodes: Option<Vec<String>>,
    /// Job dependency specification (e.g., "afterok:123:456").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency: Option<String>,
    /// Job array specification (e.g., "0-4" or "0-4%2").
    /// When set, SLURM creates one task per array index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub array: Option<String>,
    /// Number of tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<u32>,
    /// CPUs per task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpus_per_task: Option<u32>,
    /// Memory per node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_per_node: Option<MemorySpec>,
    /// Time limit in minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_limit: Option<TimeLimit>,
}

/// Memory specification for the REST API.
#[derive(Debug, Serialize)]
pub struct MemorySpec {
    /// Memory amount in megabytes.
    pub number: u64,
    /// Whether the value is set.
    pub set: bool,
    /// Whether memory is infinite.
    pub infinite: bool,
}

/// Time limit for the REST API.
#[derive(Debug, Serialize)]
pub struct TimeLimit {
    /// Whether the value is set.
    pub set: bool,
    /// Whether time is infinite.
    pub infinite: bool,
    /// Time in minutes.
    pub number: u64,
}

/// Response from the job submission endpoint.
#[derive(Debug, Deserialize)]
pub struct JobSubmitResponse {
    /// The SLURM job ID assigned to the submitted job.
    pub job_id: u32,
    /// Errors from the API (empty on success).
    #[serde(default)]
    pub errors: Vec<SlurmApiError>,
}

/// A single job record from the jobs endpoint.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct JobInfo {
    /// SLURM job ID.
    pub job_id: u32,
    /// Current job state (e.g., "RUNNING", "COMPLETED").
    pub job_state: Vec<String>,
    /// Exit code information.
    #[serde(default)]
    pub exit_code: Option<ExitCodeInfo>,
    /// Node list.
    #[serde(default)]
    pub nodes: Option<String>,
    /// Parent array job ID (present for array job tasks).
    /// Deserialized for completeness but not read directly — we query
    /// `/job/{parent_id}` so all returned entries belong to the parent.
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) array_job_id: Option<ArrayTaskId>,
    /// Array task ID (present for array job tasks, e.g., `{"number": 3}`).
    #[serde(default)]
    pub(crate) array_task_id: Option<ArrayTaskId>,
}

/// Array task ID from the REST API.
#[derive(Debug, Deserialize)]
pub(crate) struct ArrayTaskId {
    /// The task index number.
    #[serde(default)]
    pub(crate) number: u32,
    /// Whether the value is set (false for non-array jobs).
    #[serde(default)]
    pub(crate) set: bool,
}

/// Exit code information from the REST API.
#[derive(Debug, Deserialize)]
pub struct ExitCodeInfo {
    /// The return code from the job.
    #[serde(default)]
    pub return_code: Option<ReturnCode>,
}

/// Return code details.
#[derive(Debug, Deserialize)]
pub struct ReturnCode {
    /// The numeric exit code.
    #[serde(default)]
    pub number: i32,
}

/// Wrapper for the jobs list response.
#[derive(Debug, Deserialize)]
pub struct JobsResponse {
    /// List of job records.
    pub jobs: Vec<JobInfo>,
    /// Errors from the API.
    #[serde(default)]
    pub errors: Vec<SlurmApiError>,
}

/// Wrapper for the nodes list response.
#[derive(Debug, Deserialize)]
pub struct NodesResponse {
    /// List of node records.
    pub nodes: Vec<NodeInfo>,
    /// Errors from the API.
    #[serde(default)]
    pub errors: Vec<SlurmApiError>,
}

/// A node record from the nodes endpoint.
#[derive(Debug, Deserialize)]
pub struct NodeInfo {
    /// Node hostname.
    pub name: String,
    /// Node state (e.g., "idle", "alloc", "down").
    #[serde(default)]
    pub state: Vec<String>,
}

/// An error returned by the slurmrestd API.
#[derive(Debug, Deserialize)]
pub struct SlurmApiError {
    /// Error message.
    #[serde(default)]
    pub error: String,
    /// Error number.
    #[serde(default)]
    pub error_number: Option<i32>,
}

impl std::fmt::Display for SlurmApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

/// HTTP client for the SLURM REST API (slurmrestd).
#[derive(Debug, Clone)]
pub struct SlurmRestClient {
    /// Base URL for slurmrestd (e.g., `http://localhost:6820`).
    base_url: String,
    /// The underlying HTTP client.
    client: reqwest::Client,
    /// SLURM user name for the `X-SLURM-USER-NAME` header.
    user_name: String,
    /// Optional JWT token for `X-SLURM-USER-TOKEN` header.
    user_token: Option<String>,
}

impl SlurmRestClient {
    /// Create a new SLURM REST client.
    ///
    /// `base_url` should be the slurmrestd address (e.g., `http://localhost:6820`).
    /// `user_name` is the SLURM user. If `user_token` is provided, JWT auth is used.
    pub fn new(base_url: String, user_name: String, user_token: Option<String>) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self {
            base_url,
            client,
            user_name,
            user_token,
        }
    }

    /// Add authentication headers to a request.
    fn auth_headers(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let builder = builder.header("X-SLURM-USER-NAME", &self.user_name);
        if let Some(ref token) = self.user_token {
            builder.header("X-SLURM-USER-TOKEN", token)
        } else {
            builder
        }
    }

    /// Check connectivity by querying the nodes endpoint.
    pub async fn check_available(&self) -> Result<String, SlurmError> {
        let url = format!("{}/{API_VERSION}/nodes", self.base_url);
        let resp = self
            .auth_headers(self.client.get(&url))
            .send()
            .await
            .map_err(|e| {
                SlurmError::ClusterUnreachable(format!("slurmrestd unreachable at {url}: {e}"))
            })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(SlurmError::ClusterUnreachable(format!(
                "slurmrestd returned HTTP {status}: {body}"
            )));
        }

        let nodes_resp: NodesResponse = resp.json().await.map_err(|e| {
            SlurmError::ParseError(format!("failed to parse slurmrestd nodes response: {e}"))
        })?;

        if !nodes_resp.errors.is_empty() {
            let errs: Vec<_> = nodes_resp.errors.iter().map(|e| e.to_string()).collect();
            return Err(SlurmError::ClusterUnreachable(errs.join("; ")));
        }

        let available = nodes_resp
            .nodes
            .iter()
            .filter(|n| n.state.iter().any(|s| s != "down" && s != "drain"))
            .count();
        Ok(format!(
            "slurmrestd ({API_VERSION}): {available} nodes available"
        ))
    }

    /// Check that at least one node is available.
    pub async fn health_check(&self) -> Result<(), SlurmError> {
        let url = format!("{}/{API_VERSION}/nodes", self.base_url);
        let resp = self
            .auth_headers(self.client.get(&url))
            .send()
            .await
            .map_err(|e| {
                SlurmError::ClusterUnreachable(format!("slurmrestd health check failed: {e}"))
            })?;

        let nodes_resp: NodesResponse = Self::check_response(resp).await?;

        let available = nodes_resp
            .nodes
            .iter()
            .filter(|n| n.state.iter().any(|s| s != "down" && s != "drain"))
            .count();

        if available == 0 {
            return Err(SlurmError::ClusterUnreachable(
                "no available nodes via slurmrestd".into(),
            ));
        }

        Ok(())
    }

    /// Submit a job via the REST API. Returns the SLURM job ID.
    pub async fn submit_job(&self, request: &JobSubmitRequest) -> Result<u32, SlurmError> {
        let url = format!("{}/{API_VERSION}/job/submit", self.base_url);
        let resp = self
            .auth_headers(self.client.post(&url))
            .json(request)
            .send()
            .await
            .map_err(|e| SlurmError::SubmitFailed(format!("POST {url}: {e}")))?;

        let submit_resp: JobSubmitResponse = Self::check_response(resp).await?;

        if !submit_resp.errors.is_empty() {
            let errs: Vec<_> = submit_resp.errors.iter().map(|e| e.to_string()).collect();
            return Err(SlurmError::SubmitFailed(errs.join("; ")));
        }

        Ok(submit_resp.job_id)
    }

    /// Submit a job with dependency constraints via the REST API.
    pub async fn submit_job_with_deps(
        &self,
        mut request: JobSubmitRequest,
        deps: &[u32],
    ) -> Result<u32, SlurmError> {
        if !deps.is_empty() {
            let dep_str: String = deps
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(":");
            request.job.dependency = Some(format!("afterok:{dep_str}"));
        }
        self.submit_job(&request).await
    }

    /// Get the status of a specific job. Returns `None` if not found.
    pub async fn get_job(&self, job_id: u32) -> Result<Option<JobInfo>, SlurmError> {
        let url = format!("{}/{API_VERSION}/job/{job_id}", self.base_url);
        let resp = self
            .auth_headers(self.client.get(&url))
            .send()
            .await
            .map_err(|e| SlurmError::ClusterUnreachable(format!("GET {url}: {e}")))?;

        let jobs_resp: JobsResponse = Self::check_response(resp).await?;
        Ok(jobs_resp.jobs.into_iter().next())
    }

    /// Get per-task status for a job array. Returns `(task_index, state)` pairs.
    ///
    /// Queries `/job/{parent_job_id}` for the specific parent array job and
    /// extracts per-task records identified by their `array_task_id` field.
    /// Tasks that haven't started yet may not appear in the response.
    pub async fn get_array_tasks(
        &self,
        parent_job_id: u32,
    ) -> Result<Vec<(usize, String)>, SlurmError> {
        let url = format!("{}/{API_VERSION}/job/{parent_job_id}", self.base_url);
        let resp = self
            .auth_headers(self.client.get(&url))
            .send()
            .await
            .map_err(|e| SlurmError::ClusterUnreachable(format!("GET {url}: {e}")))?;

        let jobs_resp: JobsResponse = Self::check_response(resp).await?;

        let mut results = Vec::new();
        for job in &jobs_resp.jobs {
            if let Some(ref task_id) = job.array_task_id {
                if task_id.set {
                    let state = job
                        .job_state
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "UNKNOWN".to_string());
                    results.push((task_id.number as usize, state));
                }
            }
        }

        Ok(results)
    }

    /// Convert a REST API job info into a SacctRecord-compatible format.
    pub fn job_info_to_sacct_record(info: &JobInfo) -> SacctRecord {
        let state = info
            .job_state
            .first()
            .cloned()
            .unwrap_or_else(|| "UNKNOWN".to_string());

        let exit_code = info
            .exit_code
            .as_ref()
            .and_then(|ec| ec.return_code.as_ref())
            .map(|rc| rc.number)
            .unwrap_or(-1);

        SacctRecord {
            job_id: info.job_id,
            state,
            exit_code,
            peak_memory_bytes: None, // REST API doesn't provide MaxRSS
            elapsed: Duration::ZERO, // Would need start/end time calculation
            node: info.nodes.clone().unwrap_or_default(),
        }
    }

    /// Cancel a job via the REST API.
    pub async fn cancel_job(&self, job_id: u32) -> Result<(), SlurmError> {
        let url = format!("{}/{API_VERSION}/job/{job_id}", self.base_url);
        let resp = self
            .auth_headers(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| SlurmError::ClusterUnreachable(format!("DELETE {url}: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            // Cancel on already-completed jobs is not an error.
            if status != 404 && !body.contains("Invalid job id") {
                return Err(SlurmError::SubmitFailed(format!(
                    "cancel job {job_id} failed (HTTP {status}): {body}"
                )));
            }
        }

        Ok(())
    }

    /// List all jobs on the cluster (for squeue-like fallback).
    pub async fn list_jobs(&self) -> Result<Vec<JobInfo>, SlurmError> {
        let url = format!("{}/{API_VERSION}/jobs", self.base_url);
        let resp = self
            .auth_headers(self.client.get(&url))
            .send()
            .await
            .map_err(|e| SlurmError::ClusterUnreachable(format!("GET {url}: {e}")))?;

        let jobs_resp: JobsResponse = Self::check_response(resp).await?;
        Ok(jobs_resp.jobs)
    }

    /// Check HTTP response status and deserialize the JSON body.
    async fn check_response<T: serde::de::DeserializeOwned>(
        resp: reqwest::Response,
    ) -> Result<T, SlurmError> {
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SlurmError::ApiError {
                status: status.as_u16(),
                body,
            });
        }
        resp.json::<T>().await.map_err(|e| {
            SlurmError::ParseError(format!("failed to parse slurmrestd response: {e}"))
        })
    }
}

/// Parameters for building a `JobSubmitRequest`.
pub struct SubmitParams<'a> {
    /// The full bash script content.
    pub script: &'a str,
    /// Job name for SLURM.
    pub job_name: &'a str,
    /// Executor configuration.
    pub config: &'a crate::executor::SlurmConfig,
    /// Standard output file path.
    pub stdout_path: &'a str,
    /// Standard error file path.
    pub stderr_path: &'a str,
    /// Nodes to exclude from scheduling.
    pub excluded_nodes: &'a [String],
    /// Working directory for the job.
    pub working_dir: &'a str,
    /// Environment variables to pass to the job.
    pub env_vars: &'a HashMap<String, String>,
}

/// Build the environment variable list for a SLURM job.
///
/// Starts with sensible defaults (PATH from the submitting process, HOME
/// from the process environment), then merges user-provided `env_vars`.
/// User vars override defaults with the same key, so e.g. passing
/// `HOME=/custom` replaces the default.
///
/// `working_dir` is used as a fallback for HOME only when the `HOME`
/// environment variable is not set in the submitting process.
fn build_environment(working_dir: &str, env_vars: &HashMap<String, String>) -> Vec<String> {
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert(
        "PATH".to_string(),
        "/usr/local/bin:/usr/bin:/bin".to_string(),
    );
    // Use the submitting process's HOME so that tools (pip, conda, ssh)
    // find their configs. Fall back to working_dir when HOME is unset.
    let home = std::env::var("HOME").unwrap_or_else(|_| working_dir.to_string());
    env.insert("HOME".to_string(), home);

    // User-provided vars override defaults.
    for (k, v) in env_vars {
        env.insert(k.clone(), v.clone());
    }

    env.into_iter().map(|(k, v)| format!("{k}={v}")).collect()
}

/// Build a `JobSubmitRequest` from submit parameters.
///
/// This constructs the JSON body for the slurmrestd submit endpoint.
/// The script is the full bash script content (without #SBATCH directives,
/// as those are expressed via the `job` fields).
pub fn build_submit_request(params: &SubmitParams<'_>) -> JobSubmitRequest {
    let environment = build_environment(params.working_dir, params.env_vars);

    JobSubmitRequest {
        script: params.script.to_string(),
        job: JobDescription {
            name: params.job_name.to_string(),
            partition: params.config.partition.clone(),
            account: params.config.account.clone(),
            qos: params.config.qos.clone(),
            current_working_directory: Some(params.working_dir.to_string()),
            standard_output: Some(params.stdout_path.to_string()),
            standard_error: Some(params.stderr_path.to_string()),
            environment: Some(environment),
            excluded_nodes: if params.excluded_nodes.is_empty() {
                None
            } else {
                Some(params.excluded_nodes.to_vec())
            },
            dependency: None,
            array: None,
            tasks: None,
            cpus_per_task: None,
            memory_per_node: None,
            time_limit: None,
        },
    }
}

/// Parameters for building an array job submission request.
pub struct ArraySubmitParams<'a> {
    /// The full bash script content (array wrapper script).
    pub script: &'a str,
    /// Job name for SLURM.
    pub job_name: &'a str,
    /// Executor configuration.
    pub config: &'a crate::executor::SlurmConfig,
    /// Standard output file path (may contain `%a` for array task index).
    pub stdout_path: &'a str,
    /// Standard error file path (may contain `%a` for array task index).
    pub stderr_path: &'a str,
    /// Nodes to exclude from scheduling.
    pub excluded_nodes: &'a [String],
    /// Working directory for the job.
    pub working_dir: &'a str,
    /// Array specification (e.g., "0-4" or "0-4%2").
    pub array_spec: &'a str,
    /// Environment variables to pass to the job.
    pub env_vars: &'a HashMap<String, String>,
}

/// Build a `JobSubmitRequest` for a job array submission.
///
/// Like `build_submit_request` but includes the `array` field for SLURM to
/// create one task per array index.
pub fn build_array_submit_request(params: &ArraySubmitParams<'_>) -> JobSubmitRequest {
    let environment = build_environment(params.working_dir, params.env_vars);

    JobSubmitRequest {
        script: params.script.to_string(),
        job: JobDescription {
            name: params.job_name.to_string(),
            partition: params.config.partition.clone(),
            account: params.config.account.clone(),
            qos: params.config.qos.clone(),
            current_working_directory: Some(params.working_dir.to_string()),
            standard_output: Some(params.stdout_path.to_string()),
            standard_error: Some(params.stderr_path.to_string()),
            environment: Some(environment),
            excluded_nodes: if params.excluded_nodes.is_empty() {
                None
            } else {
                Some(params.excluded_nodes.to_vec())
            },
            dependency: None,
            array: Some(params.array_spec.to_string()),
            tasks: None,
            cpus_per_task: None,
            memory_per_node: None,
            time_limit: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn api_error_display() {
        let err = SlurmApiError {
            error: "job not found".into(),
            error_number: Some(1),
        };
        assert_eq!(err.to_string(), "job not found");
    }

    #[test]
    fn client_strips_trailing_slash() {
        let client = SlurmRestClient::new("http://localhost:6820/".into(), "testuser".into(), None);
        assert_eq!(client.base_url, "http://localhost:6820");
    }

    #[test]
    fn job_info_to_sacct_record_completed() {
        let info = JobInfo {
            job_id: 42,
            job_state: vec!["COMPLETED".into()],
            exit_code: Some(ExitCodeInfo {
                return_code: Some(ReturnCode { number: 0 }),
            }),
            nodes: Some("node01".into()),
            array_job_id: None,
            array_task_id: None,
        };
        let record = SlurmRestClient::job_info_to_sacct_record(&info);
        assert_eq!(record.job_id, 42);
        assert_eq!(record.state, "COMPLETED");
        assert_eq!(record.exit_code, 0);
        assert_eq!(record.node, "node01");
    }

    #[test]
    fn job_info_to_sacct_record_failed() {
        let info = JobInfo {
            job_id: 99,
            job_state: vec!["FAILED".into()],
            exit_code: Some(ExitCodeInfo {
                return_code: Some(ReturnCode { number: 1 }),
            }),
            nodes: None,
            array_job_id: None,
            array_task_id: None,
        };
        let record = SlurmRestClient::job_info_to_sacct_record(&info);
        assert_eq!(record.exit_code, 1);
        assert_eq!(record.node, "");
    }

    #[test]
    fn job_info_to_sacct_record_no_exit_code() {
        let info = JobInfo {
            job_id: 10,
            job_state: vec!["PENDING".into()],
            exit_code: None,
            nodes: None,
            array_job_id: None,
            array_task_id: None,
        };
        let record = SlurmRestClient::job_info_to_sacct_record(&info);
        assert_eq!(record.exit_code, -1);
    }

    #[test]
    fn build_submit_request_basic() {
        let config = crate::executor::SlurmConfig {
            partition: Some("gpu".into()),
            account: Some("lab".into()),
            ..crate::executor::SlurmConfig::default()
        };
        let mut env = HashMap::new();
        env.insert("PATH".into(), "/usr/bin".into());

        let req = build_submit_request(&SubmitParams {
            script: "#!/bin/bash\nhostname",
            job_name: "ox_test",
            config: &config,
            stdout_path: "/tmp/out.log",
            stderr_path: "/tmp/err.log",
            excluded_nodes: &[],
            working_dir: "/home/user/project",
            env_vars: &env,
        });

        assert_eq!(req.job.name, "ox_test");
        assert_eq!(req.job.partition, Some("gpu".into()));
        assert_eq!(req.job.account, Some("lab".into()));
        assert!(req.job.excluded_nodes.is_none());
        assert!(req.job.dependency.is_none());
        assert!(req.job.environment.is_some());
        assert_eq!(req.script, "#!/bin/bash\nhostname");
    }

    #[test]
    fn build_submit_request_with_excluded_nodes() {
        let config = crate::executor::SlurmConfig::default();
        let excluded = vec!["node01".into(), "node03".into()];
        let req = build_submit_request(&SubmitParams {
            script: "#!/bin/bash\necho hello",
            job_name: "ox_job",
            config: &config,
            stdout_path: "/tmp/out.log",
            stderr_path: "/tmp/err.log",
            excluded_nodes: &excluded,
            working_dir: "/home/user",
            env_vars: &HashMap::new(),
        });

        assert_eq!(
            req.job.excluded_nodes,
            Some(vec!["node01".into(), "node03".into()])
        );
        // Even with no user env_vars, default PATH/HOME are always injected
        let env = req.job.environment.expect("default env should be present");
        assert!(env.iter().any(|e| e.starts_with("PATH=")));
        assert!(env.iter().any(|e| e.starts_with("HOME=")));
    }

    #[test]
    fn deserialize_job_submit_response() {
        let json = r#"{"job_id": 12345, "errors": []}"#;
        let resp: JobSubmitResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.job_id, 12345);
        assert!(resp.errors.is_empty());
    }

    #[test]
    fn deserialize_jobs_response() {
        let json = r#"{
            "jobs": [
                {
                    "job_id": 42,
                    "job_state": ["RUNNING"],
                    "exit_code": null,
                    "nodes": "node01"
                }
            ],
            "errors": []
        }"#;
        let resp: JobsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.jobs.len(), 1);
        assert_eq!(resp.jobs[0].job_id, 42);
        assert_eq!(resp.jobs[0].job_state, vec!["RUNNING"]);
    }

    #[test]
    fn deserialize_nodes_response() {
        let json = r#"{
            "nodes": [
                {"name": "node01", "state": ["idle"]},
                {"name": "node02", "state": ["down"]}
            ],
            "errors": []
        }"#;
        let resp: NodesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.nodes.len(), 2);
        assert_eq!(resp.nodes[0].name, "node01");
    }

    // ── wiremock-based async tests ──────────────────────────────────

    /// Helper: create a client pointing at a wiremock server.
    fn mock_client(server: &MockServer) -> SlurmRestClient {
        SlurmRestClient::new(server.uri(), "testuser".into(), None)
    }

    #[tokio::test]
    async fn submit_job_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/{API_VERSION}/job/submit")))
            .and(header("X-SLURM-USER-NAME", "testuser"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"job_id": 5001, "errors": []})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let req = JobSubmitRequest {
            script: "#!/bin/bash\nhostname".into(),
            job: JobDescription {
                name: "test_job".into(),
                partition: None,
                account: None,
                qos: None,
                current_working_directory: None,
                standard_output: None,
                standard_error: None,
                environment: None,
                excluded_nodes: None,
                dependency: None,
                array: None,
                tasks: None,
                cpus_per_task: None,
                memory_per_node: None,
                time_limit: None,
            },
        };
        let job_id = client.submit_job(&req).await.unwrap();
        assert_eq!(job_id, 5001);
    }

    #[tokio::test]
    async fn submit_job_with_api_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/{API_VERSION}/job/submit")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "job_id": 0,
                "errors": [{"error": "Invalid partition", "error_number": 2}]
            })))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let req = JobSubmitRequest {
            script: "#!/bin/bash\necho hi".into(),
            job: JobDescription {
                name: "bad_job".into(),
                partition: Some("nonexistent".into()),
                account: None,
                qos: None,
                current_working_directory: None,
                standard_output: None,
                standard_error: None,
                environment: None,
                excluded_nodes: None,
                dependency: None,
                array: None,
                tasks: None,
                cpus_per_task: None,
                memory_per_node: None,
                time_limit: None,
            },
        };
        let err = client.submit_job(&req).await.unwrap_err();
        assert!(
            err.to_string().contains("Invalid partition"),
            "error should propagate API errors: {err}"
        );
    }

    #[tokio::test]
    async fn submit_job_with_deps_sets_dependency() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/{API_VERSION}/job/submit")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"job_id": 5002, "errors": []})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let req = JobSubmitRequest {
            script: "#!/bin/bash\necho dep".into(),
            job: JobDescription {
                name: "dep_job".into(),
                partition: None,
                account: None,
                qos: None,
                current_working_directory: None,
                standard_output: None,
                standard_error: None,
                environment: None,
                excluded_nodes: None,
                dependency: None,
                array: None,
                tasks: None,
                cpus_per_task: None,
                memory_per_node: None,
                time_limit: None,
            },
        };
        let job_id = client.submit_job_with_deps(req, &[100, 200]).await.unwrap();
        assert_eq!(job_id, 5002);
    }

    #[tokio::test]
    async fn get_job_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/job/42")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jobs": [{
                    "job_id": 42,
                    "job_state": ["COMPLETED"],
                    "exit_code": {"return_code": {"number": 0}},
                    "nodes": "c1"
                }],
                "errors": []
            })))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let info = client.get_job(42).await.unwrap().expect("job should exist");
        assert_eq!(info.job_id, 42);
        assert_eq!(info.job_state, vec!["COMPLETED"]);
    }

    #[tokio::test]
    async fn get_job_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/job/999")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jobs": [],
                "errors": []
            })))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let info = client.get_job(999).await.unwrap();
        assert!(info.is_none());
    }

    #[tokio::test]
    async fn cancel_job_success() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!("/{API_VERSION}/job/42")))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = mock_client(&server);
        client.cancel_job(42).await.unwrap();
    }

    #[tokio::test]
    async fn cancel_job_already_completed_404() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!("/{API_VERSION}/job/99")))
            .respond_with(ResponseTemplate::new(404).set_body_string("Invalid job id"))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        // Cancelling an already-completed job should not error.
        client.cancel_job(99).await.unwrap();
    }

    #[tokio::test]
    async fn list_jobs_returns_all() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/jobs")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jobs": [
                    {"job_id": 1, "job_state": ["RUNNING"], "nodes": "c1"},
                    {"job_id": 2, "job_state": ["PENDING"], "nodes": null}
                ],
                "errors": []
            })))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let jobs = client.list_jobs().await.unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].job_id, 1);
        assert_eq!(jobs[1].job_state, vec!["PENDING"]);
    }

    #[tokio::test]
    async fn check_available_counts_nodes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/nodes")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "nodes": [
                    {"name": "c1", "state": ["idle"]},
                    {"name": "c2", "state": ["alloc"]},
                    {"name": "c3", "state": ["down"]}
                ],
                "errors": []
            })))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let msg = client.check_available().await.unwrap();
        assert!(
            msg.contains("2 nodes available"),
            "should count 2 non-down nodes, got: {msg}"
        );
    }

    #[tokio::test]
    async fn health_check_fails_when_all_nodes_down() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/nodes")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "nodes": [
                    {"name": "c1", "state": ["down"]},
                    {"name": "c2", "state": ["drain"]}
                ],
                "errors": []
            })))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let err = client.health_check().await.unwrap_err();
        assert!(
            matches!(err, SlurmError::ClusterUnreachable(_)),
            "should be ClusterUnreachable: {err}"
        );
    }

    #[tokio::test]
    async fn health_check_succeeds_with_available_node() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/nodes")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "nodes": [{"name": "c1", "state": ["idle"]}],
                "errors": []
            })))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        client.health_check().await.unwrap();
    }

    #[tokio::test]
    async fn submit_job_http_500_returns_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/{API_VERSION}/job/submit")))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let req = JobSubmitRequest {
            script: "#!/bin/bash\necho fail".into(),
            job: JobDescription {
                name: "fail_job".into(),
                partition: None,
                account: None,
                qos: None,
                current_working_directory: None,
                standard_output: None,
                standard_error: None,
                environment: None,
                excluded_nodes: None,
                dependency: None,
                array: None,
                tasks: None,
                cpus_per_task: None,
                memory_per_node: None,
                time_limit: None,
            },
        };
        let err = client.submit_job(&req).await.unwrap_err();
        assert!(
            matches!(err, SlurmError::ApiError { status: 500, .. }),
            "expected ApiError(500), got: {err}"
        );
    }

    #[tokio::test]
    async fn auth_header_includes_jwt_when_set() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/nodes")))
            .and(header("X-SLURM-USER-NAME", "jdoe"))
            .and(header("X-SLURM-USER-TOKEN", "my-jwt-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "nodes": [{"name": "c1", "state": ["idle"]}],
                "errors": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = SlurmRestClient::new(server.uri(), "jdoe".into(), Some("my-jwt-token".into()));
        client.health_check().await.unwrap();
    }

    #[test]
    fn build_array_submit_request_sets_array_field() {
        let config = crate::executor::SlurmConfig {
            partition: Some("batch".into()),
            ..crate::executor::SlurmConfig::default()
        };
        let req = build_array_submit_request(&ArraySubmitParams {
            script: "#!/bin/bash\necho $SLURM_ARRAY_TASK_ID",
            job_name: "ox_array_align",
            config: &config,
            stdout_path: "/tmp/slurm-%j_%a.out",
            stderr_path: "/tmp/slurm-%j_%a.err",
            excluded_nodes: &[],
            working_dir: "/home/user/project",
            array_spec: "0-4%2",
            env_vars: &HashMap::new(),
        });

        assert_eq!(req.job.name, "ox_array_align");
        assert_eq!(req.job.array, Some("0-4%2".into()));
        assert_eq!(req.job.partition, Some("batch".into()));
        assert!(req.job.dependency.is_none());
        assert!(req.job.excluded_nodes.is_none());
    }

    #[test]
    fn build_array_submit_request_with_excluded_nodes() {
        let config = crate::executor::SlurmConfig::default();
        let excluded = vec!["node03".into()];
        let req = build_array_submit_request(&ArraySubmitParams {
            script: "#!/bin/bash\necho hi",
            job_name: "ox_array_count",
            config: &config,
            stdout_path: "/tmp/out.log",
            stderr_path: "/tmp/err.log",
            excluded_nodes: &excluded,
            working_dir: "/home/user",
            array_spec: "0-9",
            env_vars: &HashMap::new(),
        });

        assert_eq!(req.job.array, Some("0-9".into()));
        assert_eq!(req.job.excluded_nodes, Some(vec!["node03".into()]));
    }

    #[test]
    fn build_submit_request_uses_process_home_not_working_dir() {
        let config = crate::executor::SlurmConfig::default();
        let req = build_submit_request(&SubmitParams {
            script: "#!/bin/bash\necho $HOME",
            job_name: "ox_home_test",
            config: &config,
            stdout_path: "/tmp/out.log",
            stderr_path: "/tmp/err.log",
            excluded_nodes: &[],
            working_dir: "/home/researcher/project",
            env_vars: &HashMap::new(),
        });
        let env = req.job.environment.unwrap();
        let home = env.iter().find(|e| e.starts_with("HOME=")).unwrap();
        // HOME should come from the process environment, not working_dir.
        // On CI/HPC, working_dir is the project directory (e.g.
        // /home/researcher/project) which is NOT the user's home.
        let expected_home =
            std::env::var("HOME").unwrap_or_else(|_| "/home/researcher/project".into());
        assert_eq!(home, &format!("HOME={expected_home}"));
    }

    #[test]
    fn build_submit_request_env_vars_override_defaults() {
        let config = crate::executor::SlurmConfig::default();
        let mut env_vars = HashMap::new();
        env_vars.insert("HOME".into(), "/custom/home".into());
        env_vars.insert("MY_VAR".into(), "hello".into());
        let req = build_submit_request(&SubmitParams {
            script: "#!/bin/bash\necho $HOME",
            job_name: "ox_env_test",
            config: &config,
            stdout_path: "/tmp/out.log",
            stderr_path: "/tmp/err.log",
            excluded_nodes: &[],
            working_dir: "/home/user/project",
            env_vars: &env_vars,
        });
        let env = req.job.environment.unwrap();
        // User-provided HOME should override the default
        let homes: Vec<_> = env.iter().filter(|e| e.starts_with("HOME=")).collect();
        assert_eq!(homes.len(), 1, "HOME should appear exactly once");
        assert_eq!(homes[0], "HOME=/custom/home");
        assert!(env.iter().any(|e| e == "MY_VAR=hello"));
    }

    #[test]
    fn build_array_submit_request_uses_process_home_not_working_dir() {
        let config = crate::executor::SlurmConfig::default();
        let req = build_array_submit_request(&ArraySubmitParams {
            script: "#!/bin/bash\necho $HOME",
            job_name: "ox_array_home",
            config: &config,
            stdout_path: "/tmp/out.log",
            stderr_path: "/tmp/err.log",
            excluded_nodes: &[],
            working_dir: "/home/researcher/project",
            array_spec: "0-3",
            env_vars: &HashMap::new(),
        });
        let env = req.job.environment.unwrap();
        let home = env.iter().find(|e| e.starts_with("HOME=")).unwrap();
        let expected_home =
            std::env::var("HOME").unwrap_or_else(|_| "/home/researcher/project".into());
        assert_eq!(home, &format!("HOME={expected_home}"));
    }

    #[test]
    fn build_array_submit_request_merges_env_vars() {
        let config = crate::executor::SlurmConfig::default();
        let mut env_vars = HashMap::new();
        env_vars.insert("CUDA_VISIBLE_DEVICES".into(), "0,1".into());
        let req = build_array_submit_request(&ArraySubmitParams {
            script: "#!/bin/bash\necho hi",
            job_name: "ox_array_env",
            config: &config,
            stdout_path: "/tmp/out.log",
            stderr_path: "/tmp/err.log",
            excluded_nodes: &[],
            working_dir: "/home/user",
            array_spec: "0-2",
            env_vars: &env_vars,
        });
        let env = req.job.environment.unwrap();
        assert!(env.iter().any(|e| e == "CUDA_VISIBLE_DEVICES=0,1"));
        assert!(env.iter().any(|e| e.starts_with("PATH=")));
        assert!(env.iter().any(|e| e.starts_with("HOME=")));
    }

    #[test]
    fn deserialize_job_info_with_array_task_id() {
        let json = r#"{
            "jobs": [
                {
                    "job_id": 100,
                    "job_state": ["COMPLETED"],
                    "exit_code": {"return_code": {"number": 0}},
                    "nodes": "c1",
                    "array_task_id": {"number": 3, "set": true}
                }
            ],
            "errors": []
        }"#;
        let resp: JobsResponse = serde_json::from_str(json).unwrap();
        let job = &resp.jobs[0];
        assert_eq!(job.job_id, 100);
        let task_id = job.array_task_id.as_ref().unwrap();
        assert_eq!(task_id.number, 3);
        assert!(task_id.set);
    }

    #[test]
    fn deserialize_job_info_without_array_task_id() {
        let json = r#"{
            "jobs": [
                {
                    "job_id": 200,
                    "job_state": ["RUNNING"]
                }
            ],
            "errors": []
        }"#;
        let resp: JobsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.jobs[0].array_task_id.is_none());
    }

    #[tokio::test]
    async fn get_array_tasks_returns_per_task_status() {
        // get_array_tasks queries /job/{parent_job_id} and extracts per-task
        // records from the response.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/job/5000")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jobs": [
                    {
                        "job_id": 5001,
                        "job_state": ["COMPLETED"],
                        "array_job_id": {"number": 5000, "set": true},
                        "array_task_id": {"number": 0, "set": true}
                    },
                    {
                        "job_id": 5002,
                        "job_state": ["RUNNING"],
                        "array_job_id": {"number": 5000, "set": true},
                        "array_task_id": {"number": 1, "set": true}
                    },
                    {
                        "job_id": 5003,
                        "job_state": ["PENDING"],
                        "array_job_id": {"number": 5000, "set": true},
                        "array_task_id": {"number": 2, "set": true}
                    }
                ],
                "errors": []
            })))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let tasks = client.get_array_tasks(5000).await.unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0], (0, "COMPLETED".to_string()));
        assert_eq!(tasks[1], (1, "RUNNING".to_string()));
        assert_eq!(tasks[2], (2, "PENDING".to_string()));
    }

    #[tokio::test]
    async fn get_array_tasks_empty_when_no_tasks_started() {
        // When no tasks have started, the /job/{id} endpoint returns no task entries.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/job/6000")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jobs": [],
                "errors": []
            })))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let tasks = client.get_array_tasks(6000).await.unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn get_array_tasks_skips_entries_without_array_task_id() {
        // The /job/{id} response may include the parent record which has no
        // array_task_id. Verify we only return entries with array_task_id set.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/job/7000")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jobs": [
                    {
                        "job_id": 7001,
                        "job_state": ["COMPLETED"],
                        "array_job_id": {"number": 7000, "set": true},
                        "array_task_id": {"number": 0, "set": true}
                    },
                    {
                        "job_id": 7000,
                        "job_state": ["RUNNING"]
                    }
                ],
                "errors": []
            })))
            .mount(&server)
            .await;

        let client = mock_client(&server);
        let tasks = client.get_array_tasks(7000).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0], (0, "COMPLETED".to_string()));
    }

    #[tokio::test]
    async fn get_array_tasks_queries_specific_job_endpoint() {
        // get_array_tasks MUST query /job/{parent_job_id} (singular), not /jobs
        // (plural). The /jobs endpoint returns all jobs on the cluster and may
        // be paginated or rate-limited, causing silent empty results.
        let server = MockServer::start().await;

        // Mount a mock on the CORRECT endpoint: /job/{parent_job_id}
        Mock::given(method("GET"))
            .and(path(format!("/{API_VERSION}/job/5000")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jobs": [
                    {
                        "job_id": 5001,
                        "job_state": ["COMPLETED"],
                        "array_job_id": {"number": 5000, "set": true},
                        "array_task_id": {"number": 0, "set": true}
                    },
                    {
                        "job_id": 5002,
                        "job_state": ["RUNNING"],
                        "array_job_id": {"number": 5000, "set": true},
                        "array_task_id": {"number": 1, "set": true}
                    }
                ],
                "errors": []
            })))
            .mount(&server)
            .await;

        // Do NOT mount anything on /jobs — if the code hits /jobs it gets 404.
        let client = mock_client(&server);
        let tasks = client.get_array_tasks(5000).await.unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0], (0, "COMPLETED".to_string()));
        assert_eq!(tasks[1], (1, "RUNNING".to_string()));
    }
}
