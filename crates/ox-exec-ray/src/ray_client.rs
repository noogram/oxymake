//! HTTP client for the Ray Jobs API.
//!
//! Wraps the Ray Jobs REST API endpoints:
//! - `POST /api/jobs/` — submit a new job
//! - `GET  /api/jobs/` — list all jobs
//! - `GET  /api/jobs/{id}` — get job details/status
//! - `POST /api/jobs/{id}/stop` — stop a running job
//! - `GET  /api/version` — check Ray dashboard version
//! - `GET  /api/nodes` — list cluster nodes (autoscaler awareness)
//! - `POST /api/placement_groups/` — create placement groups
//! - `GET  /api/placement_groups/{id}` — get placement group status

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::autoscaler::NodesResponse;
use crate::dashboard::JobListResponse;
use crate::error::RayError;
use crate::placement_group::{PlacementGroupConfig, PlacementGroupResponse, PlacementGroupStatus};

/// Submission request payload for the Ray Jobs API.
#[derive(Debug, Serialize)]
pub struct JobSubmitRequest {
    /// The shell command to execute as the job entrypoint.
    pub entrypoint: String,
    /// Optional submission ID (if not provided, Ray generates one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission_id: Option<String>,
    /// Resource requirements for the job's entrypoint process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint_num_cpus: Option<f64>,
    /// Memory requirement in bytes for the entrypoint process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint_num_gpus: Option<f64>,
    /// Additional resources for the entrypoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint_resources: Option<HashMap<String, f64>>,
    /// Environment variables to set for the job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_env: Option<serde_json::Value>,
    /// Metadata key-value pairs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

/// Response from a job submission.
#[derive(Debug, Deserialize)]
pub struct JobSubmitResponse {
    /// The unique submission ID for the job.
    pub submission_id: String,
}

/// Ray job status as returned by the Jobs API.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RayJobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Stopped,
}

/// Response from getting job details.
#[derive(Debug, Deserialize)]
pub struct JobDetailsResponse {
    /// Current status of the job.
    pub status: RayJobStatus,
    /// Human-readable status message.
    #[serde(default)]
    pub message: Option<String>,
    /// The entrypoint command.
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// Start time in milliseconds since epoch (if started).
    #[serde(default)]
    pub start_time: Option<u64>,
    /// End time in milliseconds since epoch (if finished).
    #[serde(default)]
    pub end_time: Option<u64>,
    /// Job metadata.
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
}

/// Response from the Ray version endpoint.
#[derive(Debug, Deserialize)]
pub struct VersionResponse {
    /// Ray version string.
    pub ray_version: String,
}

/// HTTP client for the Ray Jobs API.
#[derive(Debug, Clone)]
pub struct RayClient {
    /// Base URL for the Ray dashboard (e.g., `http://127.0.0.1:8265`).
    base_url: String,
    /// The underlying HTTP client.
    client: reqwest::Client,
}

impl RayClient {
    /// Create a new Ray API client.
    pub fn new(base_url: String, client: reqwest::Client) -> Self {
        // Strip trailing slash for consistent URL construction.
        let base_url = base_url.trim_end_matches('/').to_string();
        Self { base_url, client }
    }

    /// Check connectivity by querying the Ray version endpoint.
    pub async fn version(&self) -> Result<VersionResponse, RayError> {
        let url = format!("{}/api/version", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| RayError::ClusterUnreachable(format!("GET {url}: {e}")))?;
        Self::check_status(&url, resp).await
    }

    /// Submit a new job to the Ray cluster.
    pub async fn submit_job(
        &self,
        request: &JobSubmitRequest,
    ) -> Result<JobSubmitResponse, RayError> {
        let url = format!("{}/api/jobs/", self.base_url);
        let resp = self.client.post(&url).json(request).send().await?;
        Self::check_status(&url, resp).await
    }

    /// Get the details and status of a submitted job.
    pub async fn get_job_details(
        &self,
        submission_id: &str,
    ) -> Result<JobDetailsResponse, RayError> {
        let url = format!("{}/api/jobs/{}", self.base_url, submission_id);
        let resp = self.client.get(&url).send().await?;
        Self::check_status(&url, resp).await
    }

    /// Stop a running job.
    pub async fn stop_job(&self, submission_id: &str) -> Result<(), RayError> {
        let url = format!("{}/api/jobs/{}/stop", self.base_url, submission_id);
        let resp = self.client.post(&url).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(RayError::ApiStatus { status, body })
        }
    }

    /// List all jobs on the cluster.
    pub async fn list_jobs(&self) -> Result<JobListResponse, RayError> {
        let url = format!("{}/api/jobs/", self.base_url);
        let resp = self.client.get(&url).send().await?;
        Self::check_status(&url, resp).await
    }

    /// Get cluster node information (for autoscaler-aware concurrency).
    pub async fn get_nodes(&self) -> Result<NodesResponse, RayError> {
        let url = format!("{}/api/nodes", self.base_url);
        let resp = self.client.get(&url).send().await?;
        Self::check_status(&url, resp).await
    }

    /// Create a placement group.
    pub async fn create_placement_group(
        &self,
        config: &PlacementGroupConfig,
    ) -> Result<PlacementGroupResponse, RayError> {
        let url = format!("{}/api/placement_groups/", self.base_url);
        let body = config.to_ray_request();
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| RayError::PlacementGroup(format!("POST {url}: {e}")))?;
        Self::check_status(&url, resp).await
    }

    /// Get placement group status.
    pub async fn get_placement_group_status(
        &self,
        pg_id: &str,
    ) -> Result<PlacementGroupStatus, RayError> {
        let url = format!("{}/api/placement_groups/{}", self.base_url, pg_id);
        let resp = self.client.get(&url).send().await?;

        #[derive(serde::Deserialize)]
        struct PgStatusResponse {
            status: PlacementGroupStatus,
        }

        let status_resp: PgStatusResponse = Self::check_status(&url, resp).await?;
        Ok(status_resp.status)
    }

    /// Remove a placement group.
    pub async fn remove_placement_group(&self, pg_id: &str) -> Result<(), RayError> {
        let url = format!("{}/api/placement_groups/{}", self.base_url, pg_id);
        let resp = self.client.delete(&url).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(RayError::ApiStatus { status, body })
        }
    }

    /// Check HTTP response status and deserialize the JSON body.
    async fn check_status<T: serde::de::DeserializeOwned>(
        url: &str,
        resp: reqwest::Response,
    ) -> Result<T, RayError> {
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(RayError::ApiStatus {
                status: status.as_u16(),
                body,
            });
        }
        resp.json::<T>()
            .await
            .map_err(|e| RayError::ParseError(format!("failed to parse response from {url}: {e}")))
    }
}
