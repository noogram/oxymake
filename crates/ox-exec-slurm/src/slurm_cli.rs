//! Wrapper functions for SLURM CLI commands (sbatch, sacct, squeue, scancel, sinfo).
//!
//! Each function spawns the corresponding SLURM command as a subprocess and
//! parses its output. In tests, these call mock scripts prepended to `$PATH`.

use crate::error::SlurmError;

/// Submit a job script via `sbatch --parsable` and return the SLURM job ID.
pub async fn sbatch(script_path: &std::path::Path) -> Result<u32, SlurmError> {
    let output = tokio::process::Command::new("sbatch")
        .arg("--parsable")
        .arg(script_path)
        .output()
        .await
        .map_err(|e| SlurmError::ClusterUnreachable(format!("sbatch not found: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SlurmError::SubmitFailed(stderr.trim().to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // sbatch --parsable output: "12345" or "12345;cluster_name"
    let id_str = stdout.trim().split(';').next().unwrap_or("");
    id_str
        .parse::<u32>()
        .map_err(|_| SlurmError::ParseError(format!("invalid sbatch output: {stdout}")))
}

/// Query job status via `sacct`. Returns parsed records for each job ID.
///
/// Uses `--parsable2` format (pipe-delimited, no trailing delimiter).
pub async fn sacct(job_ids: &[u32]) -> Result<Vec<SacctRecord>, SlurmError> {
    if job_ids.is_empty() {
        return Ok(vec![]);
    }

    let ids: String = job_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let output = tokio::process::Command::new("sacct")
        .args([
            "-j",
            &ids,
            "--parsable2",
            "--noheader",
            "-o",
            "JobID,State,ExitCode,MaxRSS,Elapsed,NodeList",
        ])
        .output()
        .await
        .map_err(|e| SlurmError::ClusterUnreachable(format!("sacct not found: {e}")))?;

    if !output.status.success() {
        // sacct may not be available — caller should fall back to squeue.
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SlurmError::ParseError(format!("sacct failed: {stderr}")));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut records = Vec::new();

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() < 6 {
            continue;
        }

        // Skip job step records (e.g., "12345.batch") — only want the main job.
        if fields[0].contains('.') {
            continue;
        }

        let job_id = fields[0]
            .parse::<u32>()
            .map_err(|_| SlurmError::ParseError(format!("invalid job ID: {}", fields[0])))?;

        let exit_code = parse_exit_code(fields[2]);
        let peak_memory_bytes = parse_max_rss(fields[3]);
        let elapsed = parse_elapsed(fields[4]);

        records.push(SacctRecord {
            job_id,
            state: fields[1].to_string(),
            exit_code,
            peak_memory_bytes,
            elapsed,
            node: fields[5].to_string(),
        });
    }

    Ok(records)
}

/// Query per-task status for a job array via `sacct`.
///
/// Returns a vec of `(task_index, state_string)` for each array task.
/// Array task IDs in sacct appear as `{parent}_{index}` in the JobID column.
pub async fn sacct_array(parent_job_id: u32) -> Result<Vec<(usize, String)>, SlurmError> {
    let output = tokio::process::Command::new("sacct")
        .args([
            "-j",
            &parent_job_id.to_string(),
            "--parsable2",
            "--noheader",
            "-o",
            "JobID,State",
        ])
        .output()
        .await
        .map_err(|e| SlurmError::ClusterUnreachable(format!("sacct not found: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SlurmError::ParseError(format!("sacct failed: {stderr}")));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() < 2 {
            continue;
        }

        // Skip job step records (e.g., "12345_0.batch").
        if fields[0].contains('.') {
            continue;
        }

        // Parse array task ID: "12345_3" → task_index = 3
        if let Some((_parent, task_str)) = fields[0].split_once('_') {
            if let Ok(task_index) = task_str.parse::<usize>() {
                results.push((task_index, fields[1].to_string()));
            }
        }
    }

    Ok(results)
}

/// Query job status via `squeue` (fallback when sacct is unavailable).
/// Returns the SLURM state string for the job, or `None` if not found.
pub async fn squeue(job_id: u32) -> Result<Option<String>, SlurmError> {
    let output = tokio::process::Command::new("squeue")
        .args(["-j", &job_id.to_string(), "-h", "-o", "%T"])
        .output()
        .await
        .map_err(|e| SlurmError::ClusterUnreachable(format!("squeue not found: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let state = stdout.trim();
    if state.is_empty() {
        Ok(None) // Job not in queue (completed or unknown)
    } else {
        Ok(Some(state.to_string()))
    }
}

/// Submit a job script via `sbatch --parsable` with dependency constraints.
///
/// `deps` is a list of SLURM job IDs that must complete successfully
/// (`afterok`) before this job is eligible to run. If `deps` is empty,
/// behaves identically to [`sbatch`].
pub async fn sbatch_with_deps(
    script_path: &std::path::Path,
    deps: &[u32],
) -> Result<u32, SlurmError> {
    let mut cmd = tokio::process::Command::new("sbatch");
    cmd.arg("--parsable");

    if !deps.is_empty() {
        let dep_str: String = deps
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(":");
        cmd.arg(format!("--dependency=afterok:{dep_str}"));
    }

    cmd.arg(script_path);

    let output = cmd
        .output()
        .await
        .map_err(|e| SlurmError::ClusterUnreachable(format!("sbatch not found: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SlurmError::SubmitFailed(stderr.trim().to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // sbatch --parsable output: "12345" or "12345;cluster_name"
    let id_str = stdout.trim().split(';').next().unwrap_or("");
    id_str
        .parse::<u32>()
        .map_err(|_| SlurmError::ParseError(format!("invalid sbatch output: {stdout}")))
}

/// Cancel a job via `scancel`.
pub async fn scancel(job_id: u32) -> Result<(), SlurmError> {
    let output = tokio::process::Command::new("scancel")
        .arg(job_id.to_string())
        .output()
        .await
        .map_err(|e| SlurmError::ClusterUnreachable(format!("scancel not found: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // scancel for already-completed jobs is not an error.
        if !stderr.contains("Invalid job id") {
            return Err(SlurmError::ParseError(format!("scancel failed: {stderr}")));
        }
    }

    Ok(())
}

/// Verify SLURM is reachable by running `sinfo --version`.
pub async fn check_slurm_available() -> Result<String, SlurmError> {
    let output = tokio::process::Command::new("sinfo")
        .arg("--version")
        .output()
        .await
        .map_err(|e| SlurmError::ClusterUnreachable(format!("sinfo not found: {e}")))?;

    if !output.status.success() {
        return Err(SlurmError::ClusterUnreachable(
            "sinfo --version failed".into(),
        ));
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(version)
}

/// A parsed record from `sacct` output.
#[derive(Debug, Clone)]
pub struct SacctRecord {
    pub job_id: u32,
    pub state: String,
    pub exit_code: i32,
    pub peak_memory_bytes: Option<u64>,
    pub elapsed: std::time::Duration,
    pub node: String,
}

/// Parse sacct exit code format "exit:signal" → exit code integer.
fn parse_exit_code(s: &str) -> i32 {
    s.split(':')
        .next()
        .and_then(|c| c.parse::<i32>().ok())
        .unwrap_or(-1)
}

/// Parse sacct MaxRSS format (e.g., "1024K", "512M", "2G") → bytes.
fn parse_max_rss(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let s = s.trim();
    let (num_str, suffix) = if let Some(stripped) = s.strip_suffix('K') {
        (stripped, 1024u64)
    } else if let Some(stripped) = s.strip_suffix('M') {
        (stripped, 1024 * 1024)
    } else if let Some(stripped) = s.strip_suffix('G') {
        (stripped, 1024 * 1024 * 1024)
    } else {
        (s, 1u64) // Assume bytes
    };
    num_str.parse::<u64>().ok().map(|n| n * suffix)
}

/// Parse sacct Elapsed format "HH:MM:SS" → Duration.
fn parse_elapsed(s: &str) -> std::time::Duration {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.as_slice() {
        [h, m, s] => {
            let hours = h.parse::<u64>().unwrap_or(0);
            let mins = m.parse::<u64>().unwrap_or(0);
            let secs = s.parse::<u64>().unwrap_or(0);
            std::time::Duration::from_secs(hours * 3600 + mins * 60 + secs)
        }
        [m, s] => {
            let mins = m.parse::<u64>().unwrap_or(0);
            let secs = s.parse::<u64>().unwrap_or(0);
            std::time::Duration::from_secs(mins * 60 + secs)
        }
        _ => std::time::Duration::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exit_code_success() {
        assert_eq!(parse_exit_code("0:0"), 0);
    }

    #[test]
    fn parse_exit_code_failure() {
        assert_eq!(parse_exit_code("1:0"), 1);
        assert_eq!(parse_exit_code("137:9"), 137);
    }

    #[test]
    fn parse_max_rss_kilobytes() {
        assert_eq!(parse_max_rss("1024K"), Some(1024 * 1024));
    }

    #[test]
    fn parse_max_rss_megabytes() {
        assert_eq!(parse_max_rss("512M"), Some(512 * 1024 * 1024));
    }

    #[test]
    fn parse_max_rss_empty() {
        assert_eq!(parse_max_rss(""), None);
    }

    #[test]
    fn parse_elapsed_hhmmss() {
        let d = parse_elapsed("01:30:15");
        assert_eq!(d, std::time::Duration::from_secs(5415));
    }

    #[test]
    fn parse_elapsed_mmss() {
        let d = parse_elapsed("05:30");
        assert_eq!(d, std::time::Duration::from_secs(330));
    }
}
