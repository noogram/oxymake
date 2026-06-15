//! Job array emulation for wildcard expansion.
//!
//! Ray does not have native job arrays like SLURM. This module emulates
//! the pattern by submitting multiple individual jobs that share metadata
//! and naming conventions, allowing them to be tracked and managed as a
//! logical group.

use std::collections::HashMap;

use crate::ray_client::JobSubmitRequest;

/// Specification for a job array — a group of related jobs from wildcard expansion.
#[derive(Debug, Clone)]
pub struct JobArraySpec {
    /// Shared array name (typically the rule name).
    pub array_name: String,
    /// Run ID for all jobs in this array.
    pub run_id: String,
    /// Base entrypoint template (with `{index}` and `{value}` placeholders).
    pub entrypoint_template: String,
    /// The expanded values (one job per value).
    pub expansions: Vec<ArrayExpansion>,
    /// Resource requirements (shared across all jobs).
    pub num_cpus: Option<f64>,
    /// GPU requirements (shared across all jobs).
    pub num_gpus: Option<f64>,
    /// Custom resources (shared across all jobs).
    pub custom_resources: HashMap<String, f64>,
    /// Base runtime_env (shared across all jobs).
    pub runtime_env: Option<serde_json::Value>,
}

/// A single expansion within a job array.
#[derive(Debug, Clone)]
pub struct ArrayExpansion {
    /// Array index (0-based).
    pub index: usize,
    /// The OxyMake job ID for this expansion.
    pub job_id: String,
    /// Wildcard values that produced this expansion.
    pub wildcards: HashMap<String, String>,
    /// The resolved entrypoint command for this specific expansion.
    pub entrypoint: String,
}

/// An expanded job array ready for submission.
#[derive(Debug)]
pub struct ExpandedJobArray {
    /// Array metadata for tracking.
    pub array_name: String,
    /// Individual submission requests.
    pub requests: Vec<(String, JobSubmitRequest)>,
}

/// Expand a `JobArraySpec` into individual `JobSubmitRequest`s.
///
/// Each request gets metadata tags for array tracking:
/// - `oxymake_array_name`: the array group name
/// - `oxymake_array_index`: the 0-based index
/// - `oxymake_array_size`: total number of jobs
pub fn expand_job_array(spec: &JobArraySpec) -> ExpandedJobArray {
    let array_size = spec.expansions.len();
    let requests = spec
        .expansions
        .iter()
        .map(|exp| {
            let mut metadata = HashMap::new();
            metadata.insert("oxymake_array_name".to_string(), spec.array_name.clone());
            metadata.insert("oxymake_array_index".to_string(), exp.index.to_string());
            metadata.insert("oxymake_array_size".to_string(), array_size.to_string());
            metadata.insert("oxymake_job_id".to_string(), exp.job_id.clone());
            metadata.insert("oxymake_run_id".to_string(), spec.run_id.clone());

            // Add wildcard values as metadata.
            for (k, v) in &exp.wildcards {
                metadata.insert(format!("oxymake_wc_{k}"), v.clone());
            }

            let request = JobSubmitRequest {
                entrypoint: exp.entrypoint.clone(),
                submission_id: None,
                entrypoint_num_cpus: spec.num_cpus,
                entrypoint_num_gpus: spec.num_gpus,
                entrypoint_resources: if spec.custom_resources.is_empty() {
                    None
                } else {
                    Some(spec.custom_resources.clone())
                },
                runtime_env: spec.runtime_env.clone(),
                metadata: Some(metadata),
            };

            (exp.job_id.clone(), request)
        })
        .collect();

    ExpandedJobArray {
        array_name: spec.array_name.clone(),
        requests,
    }
}

/// Status summary for a job array.
#[derive(Debug, Clone, Default)]
pub struct JobArrayStatus {
    /// Array name.
    pub array_name: String,
    /// Total jobs in the array.
    pub total: usize,
    /// Number of completed (succeeded) jobs.
    pub succeeded: usize,
    /// Number of failed jobs.
    pub failed: usize,
    /// Number of running jobs.
    pub running: usize,
    /// Number of pending jobs.
    pub pending: usize,
    /// Number of stopped (cancelled) jobs.
    pub stopped: usize,
}

impl JobArrayStatus {
    /// Whether all jobs in the array have reached a terminal state.
    pub fn is_complete(&self) -> bool {
        self.succeeded + self.failed + self.stopped == self.total
    }

    /// Whether all jobs succeeded.
    pub fn all_succeeded(&self) -> bool {
        self.succeeded == self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_job_array() {
        let spec = JobArraySpec {
            array_name: "process_samples".into(),
            run_id: "run-42".into(),
            entrypoint_template: "python process.py --sample={value}".into(),
            expansions: vec![
                ArrayExpansion {
                    index: 0,
                    job_id: "job-0".into(),
                    wildcards: HashMap::from([("sample".into(), "A".into())]),
                    entrypoint: "python process.py --sample=A".into(),
                },
                ArrayExpansion {
                    index: 1,
                    job_id: "job-1".into(),
                    wildcards: HashMap::from([("sample".into(), "B".into())]),
                    entrypoint: "python process.py --sample=B".into(),
                },
                ArrayExpansion {
                    index: 2,
                    job_id: "job-2".into(),
                    wildcards: HashMap::from([("sample".into(), "C".into())]),
                    entrypoint: "python process.py --sample=C".into(),
                },
            ],
            num_cpus: Some(2.0),
            num_gpus: None,
            custom_resources: HashMap::new(),
            runtime_env: None,
        };

        let expanded = expand_job_array(&spec);
        assert_eq!(expanded.array_name, "process_samples");
        assert_eq!(expanded.requests.len(), 3);

        // Check first request.
        let (job_id, req) = &expanded.requests[0];
        assert_eq!(job_id, "job-0");
        assert_eq!(req.entrypoint, "python process.py --sample=A");
        assert_eq!(req.entrypoint_num_cpus, Some(2.0));

        let meta = req.metadata.as_ref().unwrap();
        assert_eq!(meta["oxymake_array_name"], "process_samples");
        assert_eq!(meta["oxymake_array_index"], "0");
        assert_eq!(meta["oxymake_array_size"], "3");
        assert_eq!(meta["oxymake_wc_sample"], "A");
    }

    #[test]
    fn test_array_status() {
        let status = JobArrayStatus {
            array_name: "test".into(),
            total: 5,
            succeeded: 3,
            failed: 1,
            running: 1,
            pending: 0,
            stopped: 0,
        };
        assert!(!status.is_complete());
        assert!(!status.all_succeeded());

        let done = JobArrayStatus {
            total: 3,
            succeeded: 3,
            ..Default::default()
        };
        assert!(done.is_complete());
        assert!(done.all_succeeded());
    }
}
