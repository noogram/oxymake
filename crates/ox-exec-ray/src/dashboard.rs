//! Ray Dashboard metrics integration.
//!
//! Queries Ray Dashboard API endpoints for cluster metrics, job statistics,
//! and node health. Designed for integration with `ox-monitor-tui` for
//! real-time cluster monitoring.

use std::collections::HashMap;

use serde::Deserialize;
use serde::de::{self, Deserializer};

/// Job-level metrics from the Ray Dashboard.
#[derive(Debug, Clone, Default)]
pub struct JobMetrics {
    /// Number of jobs by status.
    pub status_counts: HashMap<String, usize>,
    /// Total jobs tracked.
    pub total_jobs: usize,
}

/// Cluster-wide metrics snapshot.
#[derive(Debug, Clone, Default)]
pub struct ClusterMetrics {
    /// Number of alive nodes.
    pub alive_nodes: usize,
    /// Total cluster CPU utilization (0.0 - 1.0).
    pub cpu_utilization: f64,
    /// Total cluster GPU utilization (0.0 - 1.0).
    pub gpu_utilization: f64,
    /// Total memory used bytes.
    pub memory_used_bytes: u64,
    /// Total memory total bytes.
    pub memory_total_bytes: u64,
    /// Per-job metrics.
    pub jobs: JobMetrics,
}

/// Response from the Ray jobs list endpoint.
///
/// Ray's `/api/jobs/` endpoint returns either a plain JSON array (v2.54+)
/// or an object with a `"data"` field (older versions). This type handles
/// both formats transparently.
#[derive(Debug)]
pub struct JobListResponse {
    /// List of job summaries.
    pub data: Vec<JobSummary>,
}

impl<'de> Deserialize<'de> for JobListResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match &value {
            serde_json::Value::Array(_) => {
                let data: Vec<JobSummary> =
                    serde_json::from_value(value).map_err(de::Error::custom)?;
                Ok(JobListResponse { data })
            }
            serde_json::Value::Object(_) => {
                #[derive(Deserialize)]
                struct Wrapper {
                    #[serde(default)]
                    data: Vec<JobSummary>,
                }
                let w: Wrapper = serde_json::from_value(value).map_err(de::Error::custom)?;
                Ok(JobListResponse { data: w.data })
            }
            _ => Err(de::Error::custom(
                "expected JSON array or object from Ray jobs API",
            )),
        }
    }
}

/// Summary of a single job from the jobs list.
#[derive(Debug, Deserialize)]
pub struct JobSummary {
    /// Ray submission ID.
    pub submission_id: String,
    /// Current job status.
    pub status: String,
    /// Job entrypoint command.
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// Job metadata.
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
}

/// Compute job-level metrics from a jobs list response.
pub fn compute_job_metrics(jobs: &[JobSummary]) -> JobMetrics {
    let mut status_counts: HashMap<String, usize> = HashMap::new();
    for job in jobs {
        *status_counts.entry(job.status.clone()).or_insert(0) += 1;
    }
    JobMetrics {
        total_jobs: jobs.len(),
        status_counts,
    }
}

/// Filter jobs to those submitted by OxyMake (have `oxymake_run_id` metadata).
pub fn filter_oxymake_jobs(jobs: &[JobSummary]) -> Vec<&JobSummary> {
    jobs.iter()
        .filter(|j| {
            j.metadata
                .as_ref()
                .is_some_and(|m| m.contains_key("oxymake_run_id"))
        })
        .collect()
}

/// Summary of OxyMake-specific job activity on the cluster.
#[derive(Debug, Clone, Default)]
pub struct OxymakeClusterSummary {
    /// Number of OxyMake jobs currently running.
    pub running: usize,
    /// Number of OxyMake jobs pending.
    pub pending: usize,
    /// Number of OxyMake jobs succeeded.
    pub succeeded: usize,
    /// Number of OxyMake jobs failed.
    pub failed: usize,
    /// Distinct run IDs with active jobs.
    pub active_runs: Vec<String>,
}

/// Compute OxyMake-specific cluster summary.
pub fn compute_oxymake_summary(jobs: &[JobSummary]) -> OxymakeClusterSummary {
    let ox_jobs = filter_oxymake_jobs(jobs);
    let mut summary = OxymakeClusterSummary::default();
    let mut run_ids = std::collections::HashSet::new();

    for job in &ox_jobs {
        match job.status.as_str() {
            "RUNNING" => summary.running += 1,
            "PENDING" => summary.pending += 1,
            "SUCCEEDED" => summary.succeeded += 1,
            "FAILED" => summary.failed += 1,
            _ => {}
        }
        if let Some(meta) = &job.metadata {
            if let Some(run_id) = meta.get("oxymake_run_id") {
                if job.status == "RUNNING" || job.status == "PENDING" {
                    run_ids.insert(run_id.clone());
                }
            }
        }
    }

    summary.active_runs = run_ids.into_iter().collect();
    summary.active_runs.sort();
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_job(id: &str, status: &str, is_oxymake: bool) -> JobSummary {
        let metadata = if is_oxymake {
            Some(HashMap::from([
                ("oxymake_run_id".into(), "run-1".into()),
                ("oxymake_job_id".into(), id.into()),
            ]))
        } else {
            None
        };
        JobSummary {
            submission_id: id.into(),
            status: status.into(),
            entrypoint: Some("echo hello".into()),
            metadata,
        }
    }

    #[test]
    fn test_compute_job_metrics() {
        let jobs = vec![
            make_job("j1", "RUNNING", true),
            make_job("j2", "RUNNING", true),
            make_job("j3", "SUCCEEDED", true),
            make_job("j4", "FAILED", false),
        ];
        let metrics = compute_job_metrics(&jobs);
        assert_eq!(metrics.total_jobs, 4);
        assert_eq!(metrics.status_counts["RUNNING"], 2);
        assert_eq!(metrics.status_counts["SUCCEEDED"], 1);
        assert_eq!(metrics.status_counts["FAILED"], 1);
    }

    #[test]
    fn test_filter_oxymake_jobs() {
        let jobs = vec![
            make_job("j1", "RUNNING", true),
            make_job("j2", "RUNNING", false),
            make_job("j3", "PENDING", true),
        ];
        let filtered = filter_oxymake_jobs(&jobs);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_oxymake_summary() {
        let jobs = vec![
            make_job("j1", "RUNNING", true),
            make_job("j2", "PENDING", true),
            make_job("j3", "SUCCEEDED", true),
            make_job("j4", "RUNNING", false),
        ];
        let summary = compute_oxymake_summary(&jobs);
        assert_eq!(summary.running, 1);
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.active_runs, vec!["run-1"]);
    }

    /// Ray v2.54+ returns a plain JSON array from `/api/jobs/`.
    #[test]
    fn test_job_list_response_plain_array() {
        let json = serde_json::json!([
            {"submission_id": "j1", "status": "RUNNING", "entrypoint": "echo hi"},
            {"submission_id": "j2", "status": "SUCCEEDED"}
        ]);
        let resp: JobListResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].submission_id, "j1");
        assert_eq!(resp.data[1].status, "SUCCEEDED");
    }

    /// Older Ray versions return `{"data": [...]}` from `/api/jobs/`.
    #[test]
    fn test_job_list_response_data_wrapper() {
        let json = serde_json::json!({
            "data": [
                {"submission_id": "j1", "status": "PENDING"}
            ]
        });
        let resp: JobListResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].submission_id, "j1");
    }

    /// Empty object (no `data` field) should yield an empty list.
    #[test]
    fn test_job_list_response_empty_object() {
        let json = serde_json::json!({});
        let resp: JobListResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.data.len(), 0);
    }

    /// Empty array should yield an empty list.
    #[test]
    fn test_job_list_response_empty_array() {
        let json = serde_json::json!([]);
        let resp: JobListResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.data.len(), 0);
    }
}
