//! Maps SLURM state strings to OxyMake `JobStatus`.

use ox_core::traits::executor::JobStatus;

/// Map a SLURM state string (from sacct or squeue) to an OxyMake `JobStatus`.
///
/// SLURM states reference:
/// <https://slurm.schedmd.com/sacct.html#SECTION_JOB-STATE-CODES>
pub fn slurm_state_to_job_status(state: &str) -> JobStatus {
    match state {
        // Pending states
        "PENDING" | "REQUEUED" | "SUSPENDED" | "CONFIGURING" => JobStatus::Queued,

        // Running states
        "RUNNING" | "COMPLETING" | "RESIZING" | "STAGE_OUT" => JobStatus::Running,

        // Success
        "COMPLETED" => JobStatus::Completed,

        // Failure states
        "FAILED" => JobStatus::Failed("job failed".into()),
        "OUT_OF_MEMORY" => JobStatus::Failed("out of memory (OOM)".into()),
        "TIMEOUT" => JobStatus::Failed("exceeded time limit".into()),
        "DEADLINE" => JobStatus::Failed("reached deadline".into()),
        "NODE_FAIL" => JobStatus::Failed("node failure".into()),
        "BOOT_FAIL" => JobStatus::Failed("node boot failure".into()),

        // Cancellation states
        "CANCELLED" | "PREEMPTED" | "REVOKED" => JobStatus::Cancelled,

        // Unknown — treat as running (conservative)
        _other => JobStatus::Running,
    }
}

/// For a terminal SLURM state, returns the reason the job must be treated
/// as failed regardless of the exit code sacct reports, or `None` when the
/// state is a genuine success (`COMPLETED`).
///
/// TIMEOUT ("0:15"), OUT_OF_MEMORY ("0:9") and PREEMPTED ("0:0") jobs
/// frequently carry exit code 0 — the exit code alone cannot gate success.
pub fn terminal_failure(state: &str) -> Option<String> {
    match slurm_state_to_job_status(state) {
        JobStatus::Completed => None,
        JobStatus::Failed(reason) => Some(reason),
        JobStatus::Cancelled => Some("cancelled or preempted".into()),
        // Non-terminal states carry no failure verdict; callers gate on
        // is_terminal() before asking.
        JobStatus::Queued | JobStatus::Running => None,
    }
}

/// Returns `true` if the SLURM state indicates a node failure, which should
/// trigger node exclusion for future submissions.
pub fn is_node_failure(state: &str) -> bool {
    matches!(state, "NODE_FAIL" | "BOOT_FAIL")
}

/// Returns `true` if the SLURM state is terminal (job will not change state).
pub fn is_terminal(state: &str) -> bool {
    matches!(
        state,
        "COMPLETED"
            | "FAILED"
            | "CANCELLED"
            | "TIMEOUT"
            | "OUT_OF_MEMORY"
            | "DEADLINE"
            | "NODE_FAIL"
            | "BOOT_FAIL"
            | "PREEMPTED"
            | "REVOKED"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_states() {
        assert_eq!(slurm_state_to_job_status("PENDING"), JobStatus::Queued);
        assert_eq!(slurm_state_to_job_status("REQUEUED"), JobStatus::Queued);
    }

    #[test]
    fn running_states() {
        assert_eq!(slurm_state_to_job_status("RUNNING"), JobStatus::Running);
        assert_eq!(slurm_state_to_job_status("COMPLETING"), JobStatus::Running);
    }

    #[test]
    fn completed() {
        assert_eq!(slurm_state_to_job_status("COMPLETED"), JobStatus::Completed);
    }

    #[test]
    fn failure_states() {
        assert!(matches!(
            slurm_state_to_job_status("FAILED"),
            JobStatus::Failed(_)
        ));
        assert!(matches!(
            slurm_state_to_job_status("OUT_OF_MEMORY"),
            JobStatus::Failed(_)
        ));
        assert!(matches!(
            slurm_state_to_job_status("TIMEOUT"),
            JobStatus::Failed(_)
        ));
        assert!(matches!(
            slurm_state_to_job_status("NODE_FAIL"),
            JobStatus::Failed(_)
        ));
    }

    #[test]
    fn cancelled_states() {
        assert_eq!(slurm_state_to_job_status("CANCELLED"), JobStatus::Cancelled);
        assert_eq!(slurm_state_to_job_status("PREEMPTED"), JobStatus::Cancelled);
    }

    #[test]
    fn terminal_detection() {
        assert!(is_terminal("COMPLETED"));
        assert!(is_terminal("FAILED"));
        assert!(is_terminal("CANCELLED"));
        assert!(!is_terminal("PENDING"));
        assert!(!is_terminal("RUNNING"));
    }

    /// B12: terminal states other than COMPLETED are failures even when
    /// sacct reports exit code 0.
    #[test]
    fn terminal_failure_gates_on_state_not_exit_code() {
        assert!(terminal_failure("COMPLETED").is_none());
        for state in [
            "FAILED",
            "TIMEOUT",
            "OUT_OF_MEMORY",
            "PREEMPTED",
            "CANCELLED",
            "NODE_FAIL",
            "BOOT_FAIL",
            "DEADLINE",
            "REVOKED",
        ] {
            assert!(
                terminal_failure(state).is_some(),
                "{state} must be a failure regardless of exit code"
            );
        }
    }

    #[test]
    fn node_failure_detection() {
        assert!(is_node_failure("NODE_FAIL"));
        assert!(is_node_failure("BOOT_FAIL"));
        assert!(!is_node_failure("FAILED"));
        assert!(!is_node_failure("COMPLETED"));
    }
}
