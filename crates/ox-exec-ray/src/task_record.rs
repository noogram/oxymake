//! Per-task records written by the native DAG driver.
//!
//! Each Ray task writes `runs/<run>/tasks/<job>.json` as soon as its command
//! exits, successfully or not: a temporary sibling is written, then renamed
//! over the final name, so a reader never sees a partial record. The driver's
//! end-of-run `results.json` holds only status and exit code and is lost when
//! the driver is killed; these records survive it, one per finished task.
//!
//! A record carries the `os.wait4` usage of the task's single child (CPU
//! user + system time, peak RSS normalised to bytes) and the node it ran on.
//! The child is waited for directly rather than read from `RUSAGE_CHILDREN`,
//! because Ray reuses worker processes and that counter accumulates across
//! every task a worker has run.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Directory, inside a run's staging directory, that holds the records.
pub const TASKS_DIR: &str = "tasks";

/// How a task ended, as seen by the task wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// The command exited with status 0.
    Completed,
    /// The command exited non-zero, was killed, or could not be started.
    Failed,
}

/// One task's record, as written by the driver.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TaskRecord {
    /// OxyMake job ID of the task.
    pub job_id: String,
    /// How the task ended.
    pub status: TaskStatus,
    /// Exit code; negative for a signal, `None` if the command never started.
    pub exit_code: Option<i32>,
    /// Start of the task's command, Unix seconds.
    pub started_at: f64,
    /// End of the task's command, Unix seconds.
    pub ended_at: f64,
    /// Wall-clock duration of the command in milliseconds.
    pub wall_time_ms: u64,
    /// User + system CPU time of the child and its reaped descendants.
    pub cpu_time_ms: Option<u64>,
    /// Peak resident set size of the child, in bytes.
    pub peak_rss_bytes: Option<u64>,
    /// Ray node ID (`ray.get_runtime_context().get_node_id()`).
    pub node_id: Option<String>,
    /// Host name of the node.
    pub hostname: Option<String>,
    /// Tail of the error output, for a failed task.
    #[serde(default)]
    pub error: Option<String>,
}

impl TaskRecord {
    /// Exit code to record for the job: the observed one, or 1 when a failed
    /// command reported none.
    pub fn exit_code_or_default(&self) -> i32 {
        match (self.status, self.exit_code) {
            (_, Some(code)) => code,
            (TaskStatus::Completed, None) => 0,
            (TaskStatus::Failed, None) => 1,
        }
    }
}

/// File name of a job's record. `%` and `/` are percent-escaped so that any
/// job ID maps to one file name and back; the driver applies the same rule.
pub fn record_file_name(job_id: &str) -> String {
    format!("{}.json", job_id.replace('%', "%25").replace('/', "%2F"))
}

/// Path of a job's record inside `tasks_dir`.
pub fn record_path(tasks_dir: &Path, job_id: &str) -> PathBuf {
    tasks_dir.join(record_file_name(job_id))
}

/// Read one job's record; `None` if the task has not finished (or its record
/// is unreadable, which the caller treats the same way).
pub fn read(tasks_dir: &Path, job_id: &str) -> Option<TaskRecord> {
    let content = std::fs::read(record_path(tasks_dir, job_id)).ok()?;
    serde_json::from_slice(&content).ok()
}

/// Read every record in `tasks_dir`, skipping temporaries and unreadable
/// files. A missing directory yields no records.
pub fn read_all(tasks_dir: &Path) -> Vec<TaskRecord> {
    let Ok(entries) = std::fs::read_dir(tasks_dir) else {
        return Vec::new();
    };
    let mut records: Vec<TaskRecord> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|content| serde_json::from_slice(&content).ok())
        .collect();
    records.sort_by(|a, b| a.job_id.cmp(&b.job_id));
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_injective_for_slashes_and_percents() {
        assert_eq!(record_file_name("a"), "a.json");
        assert_eq!(record_file_name("r-x/y"), "r-x%2Fy.json");
        assert_ne!(record_file_name("a/b"), record_file_name("a%2Fb"));
    }

    #[test]
    fn read_all_skips_temporaries_and_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let record = serde_json::json!({
            "job_id": "a/b", "status": "completed", "exit_code": 0,
            "started_at": 1.0, "ended_at": 2.5, "wall_time_ms": 1500,
            "cpu_time_ms": 7, "peak_rss_bytes": 4096,
            "node_id": "n1", "hostname": "h1"
        });
        std::fs::write(record_path(dir.path(), "a/b"), record.to_string()).unwrap();
        std::fs::write(dir.path().join("x.json.tmp-12"), "{").unwrap();
        std::fs::write(dir.path().join("broken.json"), "{").unwrap();

        let all = read_all(dir.path());
        assert_eq!(all.len(), 1);
        assert_eq!(read(dir.path(), "a/b").as_ref(), Some(&all[0]));
        assert_eq!(all[0].cpu_time_ms, Some(7));
        assert_eq!(all[0].exit_code_or_default(), 0);
        assert!(read(dir.path(), "missing").is_none());
        assert!(read_all(&dir.path().join("absent")).is_empty());
    }
}
