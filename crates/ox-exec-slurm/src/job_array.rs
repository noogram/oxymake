//! Job array support for wildcard-expanded rules.
//!
//! When multiple `ConcreteJob`s share the same rule and differ only in wildcard
//! values, they can be submitted as a single SLURM job array (`--array=0-N`)
//! instead of N separate `sbatch` calls. This reduces scheduler overhead and
//! improves queue throughput on busy clusters.
//!
//! # How it works
//!
//! 1. Caller groups jobs by rule name using [`group_by_rule`].
//! 2. Each group becomes a [`JobArraySpec`] — one `sbatch` submission.
//! 3. A params file maps `SLURM_ARRAY_TASK_ID` → wildcard values + command.
//! 4. The generated script reads the params file at runtime and dispatches.
//!
//! Array task IDs in SLURM sacct output use the format `{parent}_{index}`,
//! e.g., `12345_0`, `12345_1`. The `parse_array_sacct` helpers handle this.

use std::collections::BTreeMap;

use ox_core::model::{ConcreteJob, JobId, RuleName};

/// Configuration for job array submission.
#[derive(Debug, Clone, Default)]
pub struct JobArrayConfig {
    /// Enable job array submission (default: false).
    pub enabled: bool,
    /// Maximum tasks per array (SLURM default limit is typically 1001).
    /// Arrays exceeding this are split into multiple submissions.
    pub max_array_size: Option<usize>,
    /// Maximum concurrently running array tasks (`--array=0-N%throttle`).
    /// Maps to the `%` suffix in the SLURM `--array` flag.
    pub max_concurrent: Option<usize>,
}

/// A group of jobs from the same rule, ready for array submission.
#[derive(Debug, Clone)]
pub struct JobArraySpec {
    /// The shared rule name.
    pub rule: RuleName,
    /// Ordered list of jobs — index position maps to `SLURM_ARRAY_TASK_ID`.
    pub jobs: Vec<ConcreteJob>,
}

impl JobArraySpec {
    /// Number of tasks in the array.
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    /// Whether the array is empty.
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Build the `--array` flag value (e.g., `0-4` or `0-4%2`).
    pub fn array_flag(&self, max_concurrent: Option<usize>) -> String {
        let last = self.jobs.len().saturating_sub(1);
        match max_concurrent {
            Some(throttle) if throttle > 0 => format!("0-{last}%{throttle}"),
            _ => format!("0-{last}"),
        }
    }

    /// Get the OxyMake job ID for a given array task index.
    pub fn job_id_for_task(&self, task_index: usize) -> Option<&JobId> {
        self.jobs.get(task_index).map(|j| &j.id)
    }
}

/// A single entry in the params file — one per array task.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArrayTaskParams {
    /// The 0-based array task index.
    pub index: usize,
    /// The OxyMake job ID for this task.
    pub job_id: String,
    /// Wildcard values for this task.
    pub wildcards: BTreeMap<String, String>,
    /// The resolved shell command for this task.
    pub command: String,
}

/// Tracking info for a submitted job array.
#[derive(Debug, Clone)]
pub struct JobArrayInfo {
    /// The parent SLURM job ID (the array itself).
    pub slurm_job_id: u32,
    /// Mapping from array task index → OxyMake job ID.
    pub task_job_ids: Vec<(usize, JobId)>,
    /// The staging directory for this array.
    pub staging_dir: std::path::PathBuf,
}

/// Group jobs by rule name for potential array submission.
///
/// Jobs are grouped when they share the same rule. Groups with only one job
/// are included (the caller decides whether to use array or single submission).
pub fn group_by_rule(jobs: &[ConcreteJob]) -> Vec<JobArraySpec> {
    let mut groups: BTreeMap<String, Vec<ConcreteJob>> = BTreeMap::new();
    for job in jobs {
        groups
            .entry(job.rule.as_str().to_string())
            .or_default()
            .push(job.clone());
    }
    groups
        .into_iter()
        .map(|(rule_name, jobs)| JobArraySpec {
            rule: RuleName::from(rule_name.as_str()),
            jobs,
        })
        .collect()
}

/// Parse an array job ID from sacct output.
///
/// SLURM array task IDs look like `12345_3` where 12345 is the parent job ID
/// and 3 is the array task index. Returns `(parent_id, Some(task_index))` for
/// array jobs, or `(job_id, None)` for regular jobs.
pub fn parse_array_job_id(raw: &str) -> Option<(u32, Option<usize>)> {
    if let Some((parent, task)) = raw.split_once('_') {
        let parent_id = parent.parse::<u32>().ok()?;
        let task_index = task.parse::<usize>().ok()?;
        Some((parent_id, Some(task_index)))
    } else {
        let job_id = raw.parse::<u32>().ok()?;
        Some((job_id, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::model::*;
    use std::collections::BTreeMap as StdBTreeMap;

    fn make_job(id: &str, rule: &str, wildcards: &[(&str, &str)]) -> ConcreteJob {
        let mut wc = StdBTreeMap::new();
        for (k, v) in wildcards {
            wc.insert(k.to_string(), v.to_string());
        }
        ConcreteJob {
            id: JobId::from(id),
            rule: RuleName::from(rule),
            wildcards: wc,
            tags: StdBTreeMap::new(),
            inputs: vec![],
            outputs: vec![],
            execution: ExecutionBlock::Shell {
                command: format!(
                    "process --sample={}",
                    wildcards.first().map(|(_, v)| *v).unwrap_or("x")
                ),
            },
            resources: StdBTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::Terminate,
            timeout: None,
            executor: Some("slurm".into()),
            priority: None,
            benchmark: None,
            params: StdBTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        }
    }

    #[test]
    fn group_by_rule_single_rule() {
        let jobs = vec![
            make_job("j-1", "align", &[("sample", "A")]),
            make_job("j-2", "align", &[("sample", "B")]),
            make_job("j-3", "align", &[("sample", "C")]),
        ];
        let groups = group_by_rule(&jobs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].rule.as_str(), "align");
        assert_eq!(groups[0].len(), 3);
    }

    #[test]
    fn group_by_rule_multiple_rules() {
        let jobs = vec![
            make_job("j-1", "align", &[("sample", "A")]),
            make_job("j-2", "count", &[("sample", "A")]),
            make_job("j-3", "align", &[("sample", "B")]),
        ];
        let groups = group_by_rule(&jobs);
        assert_eq!(groups.len(), 2);
        // BTreeMap ordering: "align" < "count"
        assert_eq!(groups[0].rule.as_str(), "align");
        assert_eq!(groups[0].len(), 2);
        assert_eq!(groups[1].rule.as_str(), "count");
        assert_eq!(groups[1].len(), 1);
    }

    #[test]
    fn array_flag_no_throttle() {
        let spec = JobArraySpec {
            rule: RuleName::from("align"),
            jobs: vec![
                make_job("j-1", "align", &[("sample", "A")]),
                make_job("j-2", "align", &[("sample", "B")]),
                make_job("j-3", "align", &[("sample", "C")]),
            ],
        };
        assert_eq!(spec.array_flag(None), "0-2");
    }

    #[test]
    fn array_flag_with_throttle() {
        let spec = JobArraySpec {
            rule: RuleName::from("align"),
            jobs: vec![
                make_job("j-1", "align", &[("sample", "A")]),
                make_job("j-2", "align", &[("sample", "B")]),
                make_job("j-3", "align", &[("sample", "C")]),
                make_job("j-4", "align", &[("sample", "D")]),
                make_job("j-5", "align", &[("sample", "E")]),
            ],
        };
        assert_eq!(spec.array_flag(Some(2)), "0-4%2");
    }

    #[test]
    fn job_id_for_task_valid() {
        let spec = JobArraySpec {
            rule: RuleName::from("align"),
            jobs: vec![
                make_job("j-1", "align", &[("sample", "A")]),
                make_job("j-2", "align", &[("sample", "B")]),
            ],
        };
        assert_eq!(spec.job_id_for_task(0).unwrap().as_str(), "j-1");
        assert_eq!(spec.job_id_for_task(1).unwrap().as_str(), "j-2");
        assert!(spec.job_id_for_task(2).is_none());
    }

    #[test]
    fn parse_array_job_id_regular() {
        assert_eq!(parse_array_job_id("12345"), Some((12345, None)));
    }

    #[test]
    fn parse_array_job_id_array_task() {
        assert_eq!(parse_array_job_id("12345_3"), Some((12345, Some(3))));
        assert_eq!(parse_array_job_id("99999_0"), Some((99999, Some(0))));
    }

    #[test]
    fn parse_array_job_id_invalid() {
        assert_eq!(parse_array_job_id("not_a_number"), None);
        assert_eq!(parse_array_job_id(""), None);
    }

    #[test]
    fn array_task_params_serialization() {
        let params = ArrayTaskParams {
            index: 0,
            job_id: "j-1".into(),
            wildcards: BTreeMap::from([("sample".into(), "A".into())]),
            command: "echo A".into(),
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: ArrayTaskParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.index, 0);
        assert_eq!(deserialized.job_id, "j-1");
        assert_eq!(deserialized.command, "echo A");
    }
}
