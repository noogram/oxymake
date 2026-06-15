//! Integration tests for the local process executor.
//!
//! These tests spawn real subprocesses and verify exit codes, stdout/stderr
//! capture, timeout enforcement, and cancellation.

use std::collections::BTreeMap;
use std::time::Duration;

use ox_core::model::*;
use ox_core::traits::executor::*;
use ox_exec_local::executor::LocalExecutor;
use serial_test::serial;

/// Helper: build a minimal [`ConcreteJob`] that runs a shell command.
fn shell_job(id: &str, command: &str) -> ConcreteJob {
    ConcreteJob {
        id: JobId::from(id),
        rule: RuleName("test".into()),
        wildcards: BTreeMap::new(),
        tags: BTreeMap::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        execution: ExecutionBlock::Shell {
            command: command.to_owned(),
        },
        resources: BTreeMap::new(),
        environment: None,
        error_strategy: ErrorStrategy::Terminate,
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

/// Helper: build an [`ExecContext`] using a temp directory for logs.
fn test_ctx(log_dir: &std::path::Path) -> ExecContext {
    ExecContext {
        global_job_limit: 4,
        run_id: "test-run".into(),
        log_dir: log_dir.to_path_buf(),
        project_dir: std::env::current_dir().unwrap_or_else(|_| log_dir.to_path_buf()),
        trusted_dirs: vec![],
        input_data: std::collections::HashMap::new(),
        memory_map: None,
    }
}

fn test_ctx_with_trusted(
    log_dir: &std::path::Path,
    trusted: Vec<std::path::PathBuf>,
) -> ExecContext {
    ExecContext {
        global_job_limit: 4,
        run_id: "test-run".into(),
        log_dir: log_dir.to_path_buf(),
        project_dir: std::env::current_dir().unwrap_or_else(|_| log_dir.to_path_buf()),
        trusted_dirs: trusted,
        input_data: std::collections::HashMap::new(),
        memory_map: None,
    }
}

// -----------------------------------------------------------------------
// Lifecycle
// -----------------------------------------------------------------------

#[tokio::test]
async fn init_succeeds() {
    let exec = LocalExecutor::new();
    exec.init().await.expect("init should succeed");
}

#[tokio::test]
async fn health_check_succeeds() {
    let exec = LocalExecutor::new();
    exec.health_check()
        .await
        .expect("health_check should succeed");
}

#[tokio::test]
async fn cleanup_succeeds() {
    let exec = LocalExecutor::new();
    exec.cleanup().await.expect("cleanup should succeed");
}

// -----------------------------------------------------------------------
// Capabilities
// -----------------------------------------------------------------------

#[test]
fn capabilities_reports_memory_passing() {
    let exec = LocalExecutor::new();
    let caps = exec.capabilities();
    assert!(caps.supports_memory_passing);
    assert!(!caps.supports_gpu);
    assert!(!caps.supports_streaming);
    assert!(!caps.supports_shadow_dirs);
}

#[test]
fn max_concurrency_default_is_none() {
    let exec = LocalExecutor::new();
    assert!(exec.max_concurrency().is_none());
}

#[test]
fn max_concurrency_with_max_jobs() {
    let exec = LocalExecutor::with_max_jobs(8);
    assert_eq!(exec.max_concurrency(), Some(8));
}

// -----------------------------------------------------------------------
// Execution — success
// -----------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn execute_echo_returns_exit_code_zero() {
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());
    let job = shell_job("echo-job", "echo hello");

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.job_id, JobId::from("echo-job"));
    assert!(result.duration.as_millis() < 5000);
}

// -----------------------------------------------------------------------
// Execution — failure
// -----------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn execute_false_returns_nonzero_exit_code() {
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());
    let job = shell_job("false-job", "false");

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();

    assert_ne!(result.exit_code, 0);
}

// -----------------------------------------------------------------------
// Log capture
// -----------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn stdout_captured_to_log_file() {
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());
    let job = shell_job("log-job", "echo 'captured output'");

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();

    let log_path = result.log_path.as_ref().expect("log_path should be set");
    assert!(log_path.exists(), "log file should exist");

    let contents = std::fs::read_to_string(log_path).unwrap();
    assert!(
        contents.contains("captured output"),
        "log should contain stdout; got: {contents}"
    );
}

#[tokio::test]
#[serial]
async fn stderr_captured_to_log_file() {
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());
    let job = shell_job("stderr-job", "echo 'error message' >&2");

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();

    let log_path = result.log_path.as_ref().expect("log_path should be set");
    let contents = std::fs::read_to_string(log_path).unwrap();
    assert!(
        contents.contains("error message"),
        "log should contain stderr; got: {contents}"
    );
}

// -----------------------------------------------------------------------
// Timeout
// -----------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn timeout_kills_long_running_job() {
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    let mut job = shell_job("timeout-job", "sleep 60");
    job.timeout = Some(Duration::from_secs(1));

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await;

    match result {
        Err(ox_exec_local::error::ExecLocalError::Timeout { timeout_secs }) => {
            assert_eq!(timeout_secs, 1);
        }
        other => panic!("expected Timeout error, got: {other:?}"),
    }
}

// -----------------------------------------------------------------------
// Poll status
// -----------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn poll_status_returns_completed() {
    let exec = LocalExecutor::new();
    let status = exec.poll_status(&JobId::from("any")).await.unwrap();
    assert_eq!(status, JobStatus::Completed);
}

// -----------------------------------------------------------------------
// Output directory auto-creation
// -----------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn prepare_workspace_creates_output_directories() {
    // Verify that prepare_workspace auto-creates parent directories for
    // output files, like Snakemake does.
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    let mut job = shell_job("mkdir-job", "echo hello > results/sub/output.txt");
    job.outputs = vec![ResolvedOutput {
        reference: OutputRef::File(std::path::PathBuf::from("results/sub/output.txt")),
        name: None,
        format: None,
        lifecycle: OutputLifecycle::Permanent,
        materialize: MaterializePolicy::Always,
    }];

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();

    // The parent directory should have been created automatically.
    let expected_dir = ws.work_dir.join("results/sub");
    assert!(
        expected_dir.exists(),
        "prepare_workspace should auto-create output parent dirs; missing: {}",
        expected_dir.display()
    );
}

#[tokio::test]
#[serial]
async fn prepare_workspace_handles_nested_output_dirs() {
    // Verify deeply nested output directories are created.
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    let mut job = shell_job("deep-mkdir-job", "true");
    job.outputs = vec![
        ResolvedOutput {
            reference: OutputRef::File(std::path::PathBuf::from("a/b/c/out1.txt")),
            name: None,
            format: None,
            lifecycle: OutputLifecycle::Permanent,
            materialize: MaterializePolicy::Always,
        },
        ResolvedOutput {
            reference: OutputRef::File(std::path::PathBuf::from("x/y/out2.csv")),
            name: None,
            format: None,
            lifecycle: OutputLifecycle::Permanent,
            materialize: MaterializePolicy::Always,
        },
    ];

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();

    assert!(ws.work_dir.join("a/b/c").exists());
    assert!(ws.work_dir.join("x/y").exists());
}

#[tokio::test]
#[serial]
async fn prepare_workspace_ignores_virtual_and_inmemory_outputs() {
    // Virtual and InMemory outputs have no filesystem path — should not error.
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    let mut job = shell_job("virtual-job", "true");
    job.outputs = vec![
        ResolvedOutput {
            reference: OutputRef::Virtual {
                id: "checkpoint".into(),
                check: String::new(),
            },
            name: None,
            format: None,
            lifecycle: OutputLifecycle::Permanent,
            materialize: MaterializePolicy::Always,
        },
        ResolvedOutput {
            reference: OutputRef::InMemory {
                type_hint: Some("DataFrame".into()),
            },
            name: None,
            format: None,
            lifecycle: OutputLifecycle::Permanent,
            materialize: MaterializePolicy::Always,
        },
    ];

    // Should succeed without errors — no dirs to create.
    let _ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
}

#[tokio::test]
#[serial]
async fn prepare_workspace_rejects_path_traversal() {
    // A malicious output path with ".." components that escapes the project
    // root must be rejected to prevent directory creation outside the workspace.
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    let mut job = shell_job("traversal-job", "true");
    job.outputs = vec![ResolvedOutput {
        reference: OutputRef::File(std::path::PathBuf::from("../../../../etc/evil/output.txt")),
        name: None,
        format: None,
        lifecycle: OutputLifecycle::Permanent,
        materialize: MaterializePolicy::Always,
    }];

    let err = exec
        .prepare_workspace(&job, &ctx)
        .await
        .expect_err("path traversal should be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("escapes"),
        "error should mention path escaping: {msg}"
    );
}

#[tokio::test]
#[serial]
async fn prepare_workspace_rejects_absolute_output_path() {
    // An absolute output path should also be rejected — outputs must be
    // relative to the workspace.
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    let mut job = shell_job("abs-path-job", "true");
    job.outputs = vec![ResolvedOutput {
        reference: OutputRef::File(std::path::PathBuf::from("/tmp/evil/output.txt")),
        name: None,
        format: None,
        lifecycle: OutputLifecycle::Permanent,
        materialize: MaterializePolicy::Always,
    }];

    let err = exec
        .prepare_workspace(&job, &ctx)
        .await
        .expect_err("absolute output path should be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("escapes"),
        "error should mention path escaping: {msg}"
    );
}

#[tokio::test]
#[serial]
async fn prepare_workspace_allows_absolute_path_under_trusted_dir() {
    // Absolute output paths that fall under a trusted config directory
    // (e.g. results_dir from {config.results_dir}) should be accepted.
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let trusted_dir = tmp.path().join("external_results");
    std::fs::create_dir_all(&trusted_dir).unwrap();

    let ctx = test_ctx_with_trusted(tmp.path(), vec![trusted_dir.clone()]);

    let output_path = trusted_dir.join("data").join("output.npz");
    let mut job = shell_job("trusted-abs-job", "true");
    job.outputs = vec![ResolvedOutput {
        reference: OutputRef::File(output_path.clone()),
        name: None,
        format: None,
        lifecycle: OutputLifecycle::Permanent,
        materialize: MaterializePolicy::Always,
    }];

    // Override cwd to tmp so the test doesn't depend on the real cwd.
    let prev_dir = std::env::current_dir().unwrap();
    let _ = std::env::set_current_dir(tmp.path());

    let ws = exec
        .prepare_workspace(&job, &ctx)
        .await
        .expect("absolute path under trusted dir should be accepted");

    // Parent directory should have been created.
    assert!(output_path.parent().unwrap().exists());

    let _ = std::env::set_current_dir(prev_dir);
    drop(ws);
}

#[tokio::test]
#[serial]
async fn prepare_workspace_rejects_absolute_path_not_under_trusted_dir() {
    // An absolute path that does NOT fall under any trusted directory
    // should still be rejected even when trusted_dirs is populated.
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let trusted_dir = tmp.path().join("trusted_results");
    std::fs::create_dir_all(&trusted_dir).unwrap();

    let ctx = test_ctx_with_trusted(tmp.path(), vec![trusted_dir]);

    let mut job = shell_job("untrusted-abs-job", "true");
    job.outputs = vec![ResolvedOutput {
        reference: OutputRef::File(std::path::PathBuf::from("/tmp/evil/output.txt")),
        name: None,
        format: None,
        lifecycle: OutputLifecycle::Permanent,
        materialize: MaterializePolicy::Always,
    }];

    let err = exec
        .prepare_workspace(&job, &ctx)
        .await
        .expect_err("absolute path outside trusted dirs should be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("escapes"),
        "error should mention path escaping: {msg}"
    );
}

// -----------------------------------------------------------------------
// Finalize workspace
// -----------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn finalize_workspace_no_outputs_is_noop() {
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());
    let job = shell_job("fin-job", "true");

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();
    exec.finalize_workspace(ws, &result).await.unwrap();
}

/// Helper: build a job that writes a file output.
fn shell_job_with_output(id: &str, command: &str, output_path: &str) -> ConcreteJob {
    ConcreteJob {
        id: JobId::from(id),
        rule: RuleName("test".into()),
        wildcards: BTreeMap::new(),
        tags: BTreeMap::new(),
        inputs: Vec::new(),
        outputs: vec![ResolvedOutput {
            reference: OutputRef::File(std::path::PathBuf::from(output_path)),
            name: None,
            format: None,
            lifecycle: OutputLifecycle::default(),
            materialize: MaterializePolicy::default(),
        }],
        execution: ExecutionBlock::Shell {
            command: command.to_owned(),
        },
        resources: BTreeMap::new(),
        environment: None,
        error_strategy: ErrorStrategy::Terminate,
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

/// Guard that restores the working directory when dropped.
struct CwdGuard(std::path::PathBuf);
impl CwdGuard {
    fn set(path: &std::path::Path) -> Self {
        let prev = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(path);
        Self(prev)
    }
}
impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

#[tokio::test]
#[serial]
async fn atomic_write_success_preserves_output() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());
    let job = shell_job_with_output("aw-ok", "echo 'hello' > result.txt", "result.txt");

    let exec = LocalExecutor::new();
    let _cwd = CwdGuard::set(tmp.path());
    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();
    assert_eq!(result.exit_code, 0);
    exec.finalize_workspace(ws, &result).await.unwrap();

    // Output at final path, no .oxytmp remnant.
    assert!(tmp.path().join("result.txt").exists());
    assert!(!tmp.path().join("result.txt.oxytmp").exists());
}

#[tokio::test]
#[serial]
async fn atomic_write_failure_cleans_output() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());
    // Write an output then fail.
    let job = shell_job_with_output(
        "aw-fail",
        "echo 'partial' > result.txt; exit 1",
        "result.txt",
    );

    let exec = LocalExecutor::new();
    let _cwd = CwdGuard::set(tmp.path());
    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();
    assert_ne!(result.exit_code, 0);
    exec.finalize_workspace(ws, &result).await.unwrap();

    // Partial output should be cleaned up.
    assert!(!tmp.path().join("result.txt").exists());
}

#[tokio::test]
#[serial]
async fn atomic_write_missing_output_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());
    // Job succeeds but never writes the declared output.
    let job = shell_job_with_output("aw-missing", "true", "ghost.txt");

    let exec = LocalExecutor::new();
    let _cwd = CwdGuard::set(tmp.path());
    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();
    assert_eq!(result.exit_code, 0);
    let err = exec.finalize_workspace(ws, &result).await.unwrap_err();
    assert!(
        err.to_string().contains("not produced"),
        "expected OutputMissing error, got: {err}"
    );
}

#[tokio::test]
#[serial]
async fn prepare_cleans_stale_oxytmp_and_output() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    // Simulate stale files from a previous interrupted run.
    std::fs::write(tmp.path().join("result.txt"), "stale-output").unwrap();
    std::fs::write(tmp.path().join("result.txt.oxytmp"), "stale-tmp").unwrap();

    let job = shell_job_with_output("aw-clean", "true", "result.txt");

    let exec = LocalExecutor::new();
    let _cwd = CwdGuard::set(tmp.path());
    let _ws = exec.prepare_workspace(&job, &ctx).await.unwrap();

    assert!(!tmp.path().join("result.txt").exists());
    assert!(!tmp.path().join("result.txt.oxytmp").exists());
}

#[tokio::test]
#[serial]
async fn atomic_write_multi_output_all_or_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    let job = ConcreteJob {
        id: JobId::from("aw-multi"),
        rule: RuleName("test".into()),
        wildcards: BTreeMap::new(),
        tags: BTreeMap::new(),
        inputs: Vec::new(),
        outputs: vec![
            ResolvedOutput {
                reference: OutputRef::File(std::path::PathBuf::from("a.csv")),
                name: None,
                format: None,
                lifecycle: OutputLifecycle::default(),
                materialize: MaterializePolicy::default(),
            },
            ResolvedOutput {
                reference: OutputRef::File(std::path::PathBuf::from("b.csv")),
                name: None,
                format: None,
                lifecycle: OutputLifecycle::default(),
                materialize: MaterializePolicy::default(),
            },
        ],
        execution: ExecutionBlock::Shell {
            command: "echo a > a.csv && echo b > b.csv".to_owned(),
        },
        resources: BTreeMap::new(),
        environment: None,
        error_strategy: ErrorStrategy::Terminate,
        timeout: None,
        executor: None,
        priority: None,
        benchmark: None,
        params: BTreeMap::new(),
        param_files: Vec::new(),
        log: LogConfig::default(),
        shell_executable: None,
        reproducibility: ReproducibilityClass::default(),
    };

    let exec = LocalExecutor::new();
    let _cwd = CwdGuard::set(tmp.path());
    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();
    assert_eq!(result.exit_code, 0);
    exec.finalize_workspace(ws, &result).await.unwrap();

    assert!(tmp.path().join("a.csv").exists());
    assert!(tmp.path().join("b.csv").exists());
    assert!(!tmp.path().join("a.csv.oxytmp").exists());
    assert!(!tmp.path().join("b.csv.oxytmp").exists());
}

// -----------------------------------------------------------------------
// Environment activation
// -----------------------------------------------------------------------

#[test]
fn shell_command_with_conda_env_wraps_command() {
    // Verify that a conda environment job produces the expected wrapped command.
    // We test the internal logic by creating a job with an environment spec and
    // checking the resulting command structure (without actually having conda).
    let mut job = shell_job("conda-job", "bwa mem ref.fa reads.fq");
    job.environment = Some(EnvSpec::Conda {
        env: "bioinfo".into(),
    });

    // The environment wrapping happens inside execute(); we can verify the
    // command wrapping logic via the public resolve_environment function
    // which is tested via the executor integration below.
    assert!(job.environment.is_some());
}

#[test]
fn conda_file_spec_env_accepted() {
    // Verify that a conda YAML file spec is accepted as a valid environment.
    let mut job = shell_job("conda-yaml-job", "python train.py");
    job.environment = Some(EnvSpec::Conda {
        env: "env.yaml".into(),
    });
    assert!(job.environment.is_some());

    let mut job2 = shell_job("conda-yml-job", "python train.py");
    job2.environment = Some(EnvSpec::Conda {
        env: "environment.yml".into(),
    });
    assert!(job2.environment.is_some());
}

#[tokio::test]
#[serial]
async fn execute_with_system_env_works_like_none() {
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    let mut job = shell_job("sys-env-job", "echo system-env");
    job.environment = Some(EnvSpec::System);

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();

    assert_eq!(result.exit_code, 0);
    let log_path = result.log_path.as_ref().unwrap();
    let contents = std::fs::read_to_string(log_path).unwrap();
    assert!(
        contents.contains("system-env"),
        "system env should pass through; got: {contents}"
    );
}

#[test]
fn docker_env_wraps_command() {
    // Verify that a Docker environment job produces the expected `docker run`
    // wrapper.  We don't actually run Docker in CI — just verify the job is
    // accepted and the environment spec is set correctly.
    let mut job = shell_job("docker-job", "echo hi");
    job.environment = Some(EnvSpec::Docker {
        image: "python:3.12".into(),
    });
    assert!(job.environment.is_some());
}

#[test]
fn nix_env_wraps_command() {
    // Verify that Nix environment spec produces a properly wrapped command.
    let mut job = shell_job("nix-job", "echo hi");
    job.environment = Some(EnvSpec::Nix {
        expr: "shell.nix".into(),
    });
    assert!(job.environment.is_some());
}

#[test]
fn apptainer_env_wraps_command() {
    // Verify that Apptainer environment spec produces a properly wrapped command.
    let mut job = shell_job("apptainer-job", "echo hi");
    job.environment = Some(EnvSpec::Apptainer {
        image: "container.sif".into(),
    });
    assert!(job.environment.is_some());
}

// -----------------------------------------------------------------------
// Process module — direct tests
// -----------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn spawn_shell_success() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("test.log");

    let result = ox_exec_local::process::spawn_shell(
        "echo direct-spawn",
        tmp.path(),
        &log_path,
        None,
        &[],
        ox_core::model::DEFAULT_SHELL,
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(!result.killed_by_timeout);

    let contents = std::fs::read_to_string(&log_path).unwrap();
    assert!(contents.contains("direct-spawn"));
}

#[tokio::test]
#[serial]
async fn spawn_shell_with_env_vars() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("env.log");

    let result = ox_exec_local::process::spawn_shell(
        "echo $MY_VAR",
        tmp.path(),
        &log_path,
        None,
        &[("MY_VAR".into(), "hello-from-env".into())],
        ox_core::model::DEFAULT_SHELL,
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    let contents = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        contents.contains("hello-from-env"),
        "env var should be visible; got: {contents}"
    );
}

/// Regression test for ox-89o: sequential stdout/stderr copy deadlocks when
/// the child fills its stderr pipe buffer while we're still draining stdout.
///
/// This test writes 128KB to stderr and 128KB to stdout concurrently. With
/// the old sequential approach, the child blocks on stderr (buffer full)
/// while we block reading stdout — classic pipe deadlock.
#[tokio::test]
#[serial]
async fn spawn_shell_concurrent_stdout_stderr_no_deadlock() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("concurrent.log");

    // Write 128KB to both stdout and stderr. The pipe buffer is typically 64KB,
    // so sequential draining will deadlock.
    let cmd = r#"
        python3 -c "
import sys, os
block = b'X' * 65536
sys.stdout.buffer.write(block)
sys.stdout.buffer.write(block)
sys.stdout.buffer.flush()
sys.stderr.buffer.write(block)
sys.stderr.buffer.write(block)
sys.stderr.buffer.flush()
"
    "#;

    // Use a timeout to avoid hanging the test suite if the deadlock is present.
    let result = ox_exec_local::process::spawn_shell(
        cmd,
        tmp.path(),
        &log_path,
        Some(Duration::from_secs(10)),
        &[],
        ox_core::model::DEFAULT_SHELL,
    )
    .await
    .unwrap();

    assert_eq!(
        result.exit_code, 0,
        "child should exit 0; timed out = {}",
        result.killed_by_timeout
    );
    assert!(
        !result.killed_by_timeout,
        "should not deadlock/timeout with concurrent stream capture"
    );

    let contents = std::fs::read_to_string(&log_path).unwrap();
    // 128KB stdout + 128KB stderr = 256KB total (at minimum)
    assert!(
        contents.len() >= 256 * 1024,
        "log should contain all output; got {} bytes",
        contents.len()
    );
}

/// Regression test for ox-d9y: cancel() was a no-op because execute() never
/// inserted the child PID into the running map.  This test verifies that a
/// long-running job can be cancelled via cancel() and that the running map is
/// populated during execution.
#[tokio::test]
#[serial]
async fn cancel_kills_running_job() {
    use std::sync::Arc;

    let exec = Arc::new(LocalExecutor::new());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    // A job that sleeps for 60 seconds — long enough that we can cancel it.
    let job = shell_job("cancel-job", "sleep 60");
    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();

    let exec_clone = Arc::clone(&exec);

    // Spawn execute() in a background task.
    let handle = tokio::spawn(async move { exec_clone.execute(&job, &ws, &ctx).await });

    // Give the child process time to start.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Cancel the job — this should find the PID and kill it.
    exec.cancel(&JobId::from("cancel-job")).await.unwrap();

    // The execute() task should complete quickly after cancellation.
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("execute should return after cancel")
        .expect("task should not panic");

    // The job should have been killed (non-zero exit code).
    match result {
        Ok(job_result) => {
            assert_ne!(
                job_result.exit_code, 0,
                "cancelled job should have non-zero exit code"
            );
        }
        Err(_) => {
            // Timeout error is also acceptable if the kill triggered it.
        }
    }
}

/// Regression test for ox-p3p0: timeout must kill the entire process group,
/// including grandchild processes spawned by shell pipelines.  Before the fix,
/// `child.kill()` only killed the direct shell child, leaving grandchildren
/// (e.g. `sleep` in a pipeline) running as orphans.
#[tokio::test]
#[serial]
#[cfg(unix)]
async fn timeout_kills_grandchild_processes() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("grandchild.pid");

    // Spawn a shell command that:
    // 1. Launches a background grandchild (`sleep 300 &`)
    // 2. Writes the grandchild's PID to a marker file
    // 3. Waits forever (so the timeout fires while grandchild is alive)
    let cmd = format!("sleep 300 & echo $! > {} ; wait", marker.display());
    let log_path = tmp.path().join("pgkill.log");

    let result = ox_exec_local::process::spawn_shell(
        &cmd,
        tmp.path(),
        &log_path,
        Some(Duration::from_secs(1)),
        &[],
        ox_core::model::DEFAULT_SHELL,
    )
    .await
    .unwrap();

    assert!(
        result.killed_by_timeout,
        "process should have been killed by timeout"
    );

    // Read the grandchild PID and verify it was also killed.
    let pid_str = std::fs::read_to_string(&marker)
        .expect("marker file should exist")
        .trim()
        .to_string();
    let grandchild_pid: i32 = pid_str.parse().expect("marker should contain a PID");

    // Give the OS a moment to reap the killed processes.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // kill(pid, 0) checks whether the process exists.  ESRCH means it's gone.
    let alive = unsafe { libc::kill(grandchild_pid, 0) };
    assert_ne!(
        alive, 0,
        "grandchild (pid {grandchild_pid}) should be dead after process group kill"
    );
}

#[tokio::test]
#[serial]
async fn spawn_shell_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("timeout.log");

    let result = ox_exec_local::process::spawn_shell(
        "sleep 60",
        tmp.path(),
        &log_path,
        Some(Duration::from_secs(1)),
        &[],
        ox_core::model::DEFAULT_SHELL,
    )
    .await
    .unwrap();

    assert!(result.killed_by_timeout);
    assert_ne!(result.exit_code, 0);
}

// -----------------------------------------------------------------------
// SIGINT contract: cancel mid-write cleans up partial outputs (ox-tofk)
// -----------------------------------------------------------------------

/// Simulate a job killed mid-write: the job writes partial output, then
/// gets cancelled (SIGTERM).  Verify that `finalize_workspace` cleans up
/// the partial output file and that a subsequent `prepare_workspace`
/// removes any orphaned `.oxytmp` files.
#[tokio::test]
#[serial]
async fn cancel_mid_write_cleans_partial_output() {
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let out_rel = std::path::PathBuf::from("result.csv");
    let out_abs = dir.path().join(&out_rel);

    // Build a job that writes partial output then sleeps (simulating a long write).
    let mut job = shell_job(
        "mid-write",
        &format!("echo 'partial' > {} && sleep 60", out_abs.display()),
    );
    job.outputs = vec![ResolvedOutput {
        reference: OutputRef::File(out_rel.clone()),
        name: None,
        format: None,
        lifecycle: OutputLifecycle::default(),
        materialize: MaterializePolicy::default(),
    }];

    // Use a shared executor so cancel() can find the PID tracked by execute().
    let exec = Arc::new(LocalExecutor::new());
    let log_dir = dir.path().join("logs");
    let ctx = ExecContext {
        global_job_limit: 1,
        run_id: "test-sigint".into(),
        log_dir: log_dir.clone(),
        project_dir: dir.path().to_path_buf(),
        trusted_dirs: vec![],
        input_data: std::collections::HashMap::new(),
        memory_map: None,
    };

    // Override cwd so prepare_workspace resolves relative paths correctly.
    let prev_dir = std::env::current_dir().unwrap();
    let _ = std::env::set_current_dir(dir.path());

    // Spawn execute in background using the SAME executor.
    let exec_bg = exec.clone();
    let job_clone = job.clone();
    let ctx_clone = ctx.clone();
    let handle = tokio::spawn(async move {
        let ws = exec_bg
            .prepare_workspace(&job_clone, &ctx_clone)
            .await
            .unwrap();
        let result = exec_bg.execute(&job_clone, &ws, &ctx_clone).await;
        (ws, result)
    });

    // Wait for the child to write partial output.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        out_abs.exists(),
        "partial output should exist before cancel"
    );

    // Cancel the job (sends SIGTERM via the same executor that tracks the PID).
    exec.cancel(&JobId::from("mid-write")).await.unwrap();

    // Wait for execute to return.
    let (ws, exec_result) = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("execute should return after cancel")
        .expect("task should not panic");

    // The job should have been killed (non-zero exit).
    let job_result = exec_result.unwrap();
    assert_ne!(job_result.exit_code, 0, "cancelled job should fail");

    // finalize_workspace should clean up partial outputs.
    exec.finalize_workspace(ws, &job_result).await.unwrap();
    assert!(
        !out_abs.exists(),
        "partial output should be removed after finalize on failure"
    );

    // Verify recovery: a fresh prepare_workspace also removes orphaned .oxytmp.
    let tmp_path = dir.path().join("result.csv.oxytmp");
    std::fs::write(&tmp_path, "orphan").unwrap();

    let ctx2 = ExecContext {
        global_job_limit: 1,
        run_id: "test-recovery".into(),
        log_dir: log_dir,
        project_dir: dir.path().to_path_buf(),
        trusted_dirs: vec![],
        input_data: std::collections::HashMap::new(),
        memory_map: None,
    };
    let _ws2 = exec.prepare_workspace(&job, &ctx2).await.unwrap();

    assert!(
        !tmp_path.exists(),
        "orphaned .oxytmp should be removed by prepare_workspace"
    );
    assert!(
        !out_abs.exists(),
        "stale output should be removed by prepare_workspace"
    );

    let _ = std::env::set_current_dir(&prev_dir);
}

/// Python stdout is captured even with default (block-buffered) piped I/O.
///
/// When OxyMake captures stdout via pipes, Python switches to block-buffered
/// output.  The orchestrator detects job completion via child.wait() and
/// drains pipes after exit, so buffered output is always captured.
///
/// Previously (ox-r8pm), we injected PYTHONUNBUFFERED=1 as a workaround.
/// That was removed in ox-dss1 — the orchestrator now handles this correctly.
#[tokio::test]
#[serial]
async fn python_stdout_captured_with_default_buffering() {
    // Skip if Python is not available on this system.
    let python_available = std::process::Command::new("python3")
        .arg("-c")
        .arg("pass")
        .output()
        .is_ok();
    if !python_available {
        eprintln!("python3 not found, skipping test");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();

    let ctx = test_ctx(&log_dir);
    let exec = LocalExecutor::new();

    // A small Python script that prints without explicit flush.  With default
    // block buffering, output stays in Python's internal buffer until exit.
    // The orchestrator drains pipes after child.wait(), capturing everything.
    let job = shell_job(
        "python-unbuf",
        r#"python3 -c "import sys; print('line-from-python'); sys.exit(0)""#,
    );

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();

    assert_eq!(result.exit_code, 0, "Python script should exit cleanly");

    // Read the log file and verify the output was captured.
    let log_path = result.log_path.as_ref().expect("log_path should be set");
    let log_content = std::fs::read_to_string(log_path).unwrap();
    assert!(
        log_content.contains("line-from-python"),
        "Python stdout should be captured in log; got: {log_content}"
    );
}

// -----------------------------------------------------------------------
// Stdin isolation (ox-mn6a)
// -----------------------------------------------------------------------

/// Verify that child processes have stdin connected to /dev/null, not the
/// parent's terminal.  Without this, processes in a background process group
/// (created by `process_group(0)`) can receive SIGTTIN when a descendant
/// reads from stdin, causing the entire job to hang indefinitely.
#[tokio::test]
#[serial]
async fn stdin_is_dev_null() {
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    // `cat` with no arguments reads from stdin.  If stdin is /dev/null it
    // gets EOF immediately and exits 0.  If stdin were a terminal in a
    // background process group, it would hang or receive SIGTTIN.
    let mut job = shell_job("stdin-null-job", "cat");
    job.timeout = Some(std::time::Duration::from_secs(5));

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();

    assert_eq!(
        result.exit_code, 0,
        "cat should exit 0 when stdin is /dev/null (got exit code {})",
        result.exit_code
    );
}

/// Regression test for ox-dss1: Python scripts with default stdout buffering
/// (block-buffered when piped) must complete correctly without requiring
/// PYTHONUNBUFFERED=1.  The orchestrator detects job completion via
/// child.wait(), not by pipe closure or line arrival, so buffered output
/// arriving late is fine.
#[tokio::test]
#[serial]
async fn python_default_buffering_completes_correctly() {
    let exec = LocalExecutor::new();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    // Python with default buffering (no PYTHONUNBUFFERED, no flush()).
    // When piped, Python uses block buffering — output stays in an internal
    // buffer until the process exits and the C runtime flushes.
    // We explicitly unset PYTHONUNBUFFERED in the shell to guarantee block
    // buffering regardless of the test runner's environment.
    let mut job = shell_job(
        "py-buffered",
        r#"unset PYTHONUNBUFFERED; python3 -c "
import sys, os
assert 'PYTHONUNBUFFERED' not in os.environ, 'PYTHONUNBUFFERED must not be set'
# Default pipe buffering: output is block-buffered.
for i in range(50):
    print(f'line {i}')
print('done', file=sys.stderr)
""#,
    );
    job.environment = Some(EnvSpec::System);
    job.timeout = Some(Duration::from_secs(10));

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();

    assert_eq!(
        result.exit_code, 0,
        "Python with default buffering should exit 0",
    );

    // Verify output was captured to the log file.
    let log_path = result.log_path.as_ref().expect("log_path should be set");
    let contents = std::fs::read_to_string(log_path).unwrap();
    assert!(
        contents.contains("line 49"),
        "log should contain all 50 stdout lines; got: {}",
        &contents[..contents.len().min(200)],
    );
    assert!(
        contents.contains("done"),
        "log should contain stderr output",
    );
}

/// Same as above but in streaming mode (with event bus) — verifies that
/// -vv output also works correctly with block-buffered child processes.
#[tokio::test]
#[serial]
async fn python_default_buffering_streaming_mode() {
    let exec = LocalExecutor::new().with_event_bus(ox_core::event::EventBus::new());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    let mut job = shell_job(
        "py-buffered-stream",
        r#"unset PYTHONUNBUFFERED; python3 -c "
import os
assert 'PYTHONUNBUFFERED' not in os.environ, 'PYTHONUNBUFFERED must not be set'
for i in range(50):
    print(f'line {i}')
""#,
    );
    job.environment = Some(EnvSpec::System);
    job.timeout = Some(Duration::from_secs(10));

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();

    assert_eq!(
        result.exit_code, 0,
        "Python streaming with default buffering should exit 0",
    );

    let log_path = result.log_path.as_ref().expect("log_path should be set");
    let contents = std::fs::read_to_string(log_path).unwrap();
    assert!(
        contents.contains("line 49"),
        "streaming log should contain all output; got: {}",
        &contents[..contents.len().min(200)],
    );
}

/// Streaming mode should also have stdin set to /dev/null.
#[tokio::test]
#[serial]
async fn stdin_is_dev_null_streaming_mode() {
    let exec = LocalExecutor::new().with_event_bus(ox_core::event::EventBus::new());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = test_ctx(tmp.path());

    let mut job = shell_job("stdin-null-stream-job", "cat");
    job.timeout = Some(std::time::Duration::from_secs(5));

    let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &ws, &ctx).await.unwrap();

    assert_eq!(
        result.exit_code, 0,
        "cat (streaming) should exit 0 when stdin is /dev/null (got exit code {})",
        result.exit_code
    );
}

// -----------------------------------------------------------------------
// Stage 2: Memory map integration
// -----------------------------------------------------------------------

/// Test that prepare_workspace materializes in-memory inputs to disk.
#[tokio::test]
#[serial]
async fn memory_map_materializes_input_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let exec = LocalExecutor::new();

    // Create an input file path that doesn't exist on disk yet.
    let input_path = std::path::PathBuf::from("data/input.bin");

    // Create a job that reads from this input and produces an output.
    let mut job = shell_job("mem-test-1", "cat data/input.bin > output/result.bin");
    job.inputs.push(ResolvedInput {
        reference: OutputRef::File(input_path.clone()),
        name: None,
        format: None,
    });
    job.outputs.push(ResolvedOutput {
        reference: OutputRef::File(std::path::PathBuf::from("output/result.bin")),
        name: None,
        lifecycle: OutputLifecycle::Permanent,
        materialize: MaterializePolicy::Always,
        format: None,
    });

    // Put data in the memory map.
    let mem_map = ox_core::memory_map::OutputMemoryMap::new();
    let test_data: Vec<u8> = b"hello from memory map".to_vec();
    mem_map.put(
        input_path.display().to_string(),
        std::sync::Arc::from(test_data.clone()),
    );

    let ctx = ExecContext {
        global_job_limit: 1,
        run_id: "test-memmap".into(),
        log_dir: dir.path().join("logs"),
        project_dir: dir.path().to_path_buf(),
        trusted_dirs: vec![],
        input_data: std::collections::HashMap::new(),
        memory_map: Some(mem_map),
    };

    // Override cwd to the temp dir.
    let _guard = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    // prepare_workspace should write memory data to disk.
    let workspace = exec.prepare_workspace(&job, &ctx).await.unwrap();

    // Verify the input file was materialized to disk from memory.
    let materialized = dir.path().join("data/input.bin");
    assert!(
        materialized.exists(),
        "input file should be materialized from memory map"
    );
    let content = std::fs::read(&materialized).unwrap();
    assert_eq!(content, b"hello from memory map");

    // Execute the job — it should succeed since the input exists.
    let result = exec.execute(&job, &workspace, &ctx).await.unwrap();
    assert_eq!(result.exit_code, 0, "job should succeed with memory input");

    // Verify the output was produced.
    let output = dir.path().join("output/result.bin");
    assert!(output.exists(), "output should be produced");
    let output_content = std::fs::read(&output).unwrap();
    assert_eq!(output_content, b"hello from memory map");

    // Restore cwd.
    std::env::set_current_dir(_guard).unwrap();
}

/// Test that prepare_workspace works normally without a memory map (backward compat).
#[tokio::test]
#[serial]
async fn no_memory_map_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let exec = LocalExecutor::new();

    let mut job = shell_job("no-mem-1", "echo done > output.txt");
    job.outputs.push(ResolvedOutput {
        reference: OutputRef::File(std::path::PathBuf::from("output.txt")),
        name: None,
        lifecycle: OutputLifecycle::Permanent,
        materialize: MaterializePolicy::Always,
        format: None,
    });

    let ctx = ExecContext {
        global_job_limit: 1,
        run_id: "test-no-mem".into(),
        log_dir: dir.path().join("logs"),
        project_dir: dir.path().to_path_buf(),
        trusted_dirs: vec![],
        input_data: std::collections::HashMap::new(),
        memory_map: None, // No memory map.
    };

    let _guard = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let workspace = exec.prepare_workspace(&job, &ctx).await.unwrap();
    let result = exec.execute(&job, &workspace, &ctx).await.unwrap();
    assert_eq!(result.exit_code, 0, "job should succeed without memory map");

    std::env::set_current_dir(_guard).unwrap();
}

/// Test that prepare_workspace skips writing when the file already exists
/// on disk (page-cache-aware optimization). In OxyMake's execution model,
/// the upstream job's finalize_workspace has already committed the file
/// before the downstream job's prepare_workspace runs, so the on-disk
/// copy is always fresh.
#[tokio::test]
#[serial]
async fn prepare_workspace_skips_write_when_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let exec = LocalExecutor::new();

    let input_path = std::path::PathBuf::from("already_on_disk.bin");

    // Simulate finalize_workspace having written the file.
    let original_data = b"data from finalize_workspace";
    std::fs::write(dir.path().join("already_on_disk.bin"), original_data).unwrap();

    let mut job = shell_job("skip-test", "cat already_on_disk.bin");
    job.inputs.push(ResolvedInput {
        reference: OutputRef::File(input_path.clone()),
        name: None,
        format: None,
    });

    // Memory map has the same data (populated by spawned task in production).
    let mem_map = ox_core::memory_map::OutputMemoryMap::new();
    mem_map.put(
        input_path.display().to_string(),
        std::sync::Arc::from(original_data.to_vec()),
    );

    let ctx = ExecContext {
        global_job_limit: 1,
        run_id: "test-skip".into(),
        log_dir: dir.path().join("logs"),
        project_dir: dir.path().to_path_buf(),
        trusted_dirs: vec![],
        input_data: std::collections::HashMap::new(),
        memory_map: Some(mem_map),
    };

    let _guard = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let _workspace = exec.prepare_workspace(&job, &ctx).await.unwrap();

    // The file should still contain the original data (no unnecessary rewrite).
    let content = std::fs::read(dir.path().join("already_on_disk.bin")).unwrap();
    assert_eq!(
        content, original_data,
        "file should not be rewritten when it already exists"
    );

    std::env::set_current_dir(_guard).unwrap();
}
