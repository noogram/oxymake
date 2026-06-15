//! Process spawning utilities for the local executor.
//!
//! This module provides [`spawn_shell`], which runs a shell command as an
//! async child process, captures stdout/stderr to a log file, enforces an
//! optional timeout, and returns the exit code, wall-clock duration, and
//! resource usage (peak memory, CPU time) via `getrusage(RUSAGE_CHILDREN)`.

use std::path::Path;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::error::ExecLocalError;

/// Maximum time to wait for stdout/stderr drain after the child process exits.
/// Once the child exits, its pipe file descriptors are closed and our readers
/// should reach EOF promptly.  This timeout is a safety net — if a grandchild
/// inherited the pipe FDs and is still alive, we don't block the orchestrator
/// forever.  Five seconds is generous for kernel pipe buffer drain.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// A single line of output from a child process.
#[derive(Debug, Clone)]
pub struct OutputLine {
    /// The text content (without trailing newline).
    pub line: String,
    /// `true` if the line came from stderr, `false` for stdout.
    pub is_stderr: bool,
}

/// The result of a completed (or timed-out) child process.
#[derive(Debug, Clone)]
pub struct ProcessResult {
    /// Exit code from the process (137 if killed by timeout).
    pub exit_code: i32,
    /// Wall-clock duration from spawn to termination.
    pub duration: Duration,
    /// Whether the process was killed because it exceeded its timeout.
    pub killed_by_timeout: bool,
    /// Peak resident set size in bytes (from `getrusage`, if available).
    pub peak_memory_bytes: Option<u64>,
    /// CPU time (user + system) from `getrusage`, if available.
    pub cpu_time: Option<Duration>,
}

/// Snapshot of `getrusage(RUSAGE_CHILDREN)` relevant fields.
#[cfg(unix)]
struct RusageSnapshot {
    user_time: Duration,
    system_time: Duration,
    max_rss_bytes: u64,
}

/// Read current `RUSAGE_CHILDREN` stats.
///
/// Returns `None` if the syscall fails (non-Unix platforms).
#[cfg(unix)]
fn snapshot_rusage_children() -> Option<RusageSnapshot> {
    use std::mem::MaybeUninit;
    let mut usage = MaybeUninit::<libc::rusage>::uninit();
    let ret = unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr()) };
    if ret != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };

    let user_time = Duration::new(
        usage.ru_utime.tv_sec as u64,
        usage.ru_utime.tv_usec as u32 * 1000,
    );
    let system_time = Duration::new(
        usage.ru_stime.tv_sec as u64,
        usage.ru_stime.tv_usec as u32 * 1000,
    );

    // On macOS, ru_maxrss is in bytes. On Linux, it's in kilobytes.
    let max_rss_bytes = if cfg!(target_os = "macos") {
        usage.ru_maxrss as u64
    } else {
        usage.ru_maxrss as u64 * 1024
    };

    Some(RusageSnapshot {
        user_time,
        system_time,
        max_rss_bytes,
    })
}

/// Spawn a shell command and capture its combined stdout/stderr to a log file.
///
/// The command is executed via `<shell> -c "<command>"` where `<shell>` defaults
/// to [`ox_core::model::DEFAULT_SHELL`] (`/bin/bash`).  If `timeout` is
/// `Some(d)`, the child is killed after `d` elapses and the returned
/// [`ProcessResult::killed_by_timeout`] flag is set.
///
/// # Arguments
///
/// * `command`  - The shell command string to execute.
/// * `work_dir` - Working directory for the child process.
/// * `log_path` - File path where combined stdout/stderr will be written.
/// * `timeout`  - Optional maximum wall-clock duration before the child is killed.
/// * `env_vars` - Additional environment variables to set in the child.
/// * `shell`    - Shell executable to use (default: [`ox_core::model::DEFAULT_SHELL`]).
///
/// # Errors
///
/// Returns [`ExecLocalError::SpawnFailed`] if the child cannot be created,
/// [`ExecLocalError::Io`] for log-file I/O failures.
pub async fn spawn_shell(
    command: &str,
    work_dir: &Path,
    log_path: &Path,
    timeout: Option<Duration>,
    env_vars: &[(String, String)],
    shell: &str,
) -> Result<ProcessResult, ExecLocalError> {
    spawn_shell_with_callback(
        command,
        work_dir,
        log_path,
        timeout,
        env_vars,
        shell,
        |_| {},
    )
    .await
}

/// Like [`spawn_shell`], but calls `on_spawn` with the child's PID immediately
/// after the process is created.  This allows the caller to track the PID for
/// cancellation before the process completes.
pub async fn spawn_shell_with_callback(
    command: &str,
    work_dir: &Path,
    log_path: &Path,
    timeout: Option<Duration>,
    env_vars: &[(String, String)],
    shell: &str,
    on_spawn: impl FnOnce(u32),
) -> Result<ProcessResult, ExecLocalError> {
    let start = Instant::now();

    // Snapshot RUSAGE_CHILDREN before spawning so we can compute the delta.
    #[cfg(unix)]
    let before = snapshot_rusage_children();

    let mut cmd = Command::new(shell);
    cmd.arg("-c")
        .arg(command)
        .current_dir(work_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Place the child in its own process group so we can kill the entire
    // group (including grandchildren from shell pipelines) on timeout or
    // cancellation.  process_group(0) sets PGID = child PID.
    #[cfg(unix)]
    cmd.process_group(0);

    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = cmd.spawn().map_err(ExecLocalError::SpawnFailed)?;

    // Notify the caller of the child PID for cancellation tracking.
    if let Some(pid) = child.id() {
        on_spawn(pid);
    }

    // Take ownership of the child's stdout/stderr handles.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Drain stdout and stderr concurrently into in-memory buffers, then write
    // them to the log file sequentially.  The old approach copied them one at a
    // time through the log file, which deadlocks when the child fills one pipe
    // buffer while we're blocked reading the other (ox-89o).
    let stdout_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut stdout) = stdout {
            tokio::io::copy(&mut stdout, &mut buf).await?;
        }
        Ok::<Vec<u8>, std::io::Error>(buf)
    });

    let stderr_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut stderr) = stderr {
            tokio::io::copy(&mut stderr, &mut buf).await?;
        }
        Ok::<Vec<u8>, std::io::Error>(buf)
    });

    let log_path_owned = log_path.to_path_buf();
    let copy_handle = tokio::spawn(async move {
        let (stdout_result, stderr_result) = tokio::join!(stdout_handle, stderr_handle);
        let stdout_buf = stdout_result.map_err(std::io::Error::other)??;
        let stderr_buf = stderr_result.map_err(std::io::Error::other)??;

        let mut log_file = tokio::fs::File::create(&log_path_owned).await?;
        log_file.write_all(&stdout_buf).await?;
        if !stderr_buf.is_empty() {
            log_file.write_all(b"\n--- stderr ---\n").await?;
            log_file.write_all(&stderr_buf).await?;
        }
        log_file.flush().await?;
        Ok::<(), std::io::Error>(())
    });

    // Wait for the child, with an optional timeout.
    let (killed_by_timeout, status) = if let Some(dur) = timeout {
        match tokio::time::timeout(dur, child.wait()).await {
            Ok(result) => (false, result?),
            Err(_elapsed) => {
                // Timeout expired — kill the entire process group so
                // grandchildren (e.g. from shell pipelines) are also
                // terminated.
                #[cfg(unix)]
                {
                    if let Some(pid) = child.id() {
                        // Safety: killpg with a valid PGID is a standard
                        // POSIX syscall.  The process group was created by
                        // process_group(0) above.
                        unsafe {
                            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    child.kill().await.ok();
                }
                // Still wait so the OS can reap the zombie.
                let status = child.wait().await?;
                (true, status)
            }
        }
    } else {
        let status = child.wait().await?;
        (false, status)
    };

    // Wait for the pipe drain and log-copy task to finish.  The child has
    // already exited so its pipe ends are closed — our readers should reach
    // EOF quickly.  The timeout is a safety net for the rare case where a
    // grandchild inherited the pipe FDs and is still alive.
    match tokio::time::timeout(DRAIN_TIMEOUT, copy_handle).await {
        Ok(result) => {
            let _ = result;
        }
        Err(_elapsed) => {
            // Drain timed out — log data may be incomplete but the process
            // result is authoritative.  This is acceptable: the orchestrator
            // must not hang waiting for output from a rogue grandchild.
        }
    }

    let duration = start.elapsed();
    let exit_code = status
        .code()
        .unwrap_or(if killed_by_timeout { 137 } else { -1 });

    // Compute resource usage delta from RUSAGE_CHILDREN snapshots.
    #[cfg(unix)]
    let (peak_memory_bytes, cpu_time) = {
        let after = snapshot_rusage_children();
        match (before, after) {
            (Some(b), Some(a)) => {
                let cpu = (a.user_time + a.system_time).checked_sub(b.user_time + b.system_time);
                // max_rss is a high-water mark, not cumulative — the delta
                // approach doesn't work for it. Instead, if the after-snapshot
                // is larger than the before-snapshot, the child raised the
                // high-water mark and the difference is a lower bound on its
                // peak RSS. Otherwise, we report the raw after value as a
                // best-effort estimate (it may include prior children).
                let peak = if a.max_rss_bytes > b.max_rss_bytes {
                    Some(a.max_rss_bytes - b.max_rss_bytes)
                } else {
                    // Cannot isolate this child's contribution; report raw
                    // after value as upper bound.
                    Some(a.max_rss_bytes)
                };
                (peak, cpu)
            }
            _ => (None, None),
        }
    };

    #[cfg(not(unix))]
    let (peak_memory_bytes, cpu_time) = (None, None);

    Ok(ProcessResult {
        exit_code,
        duration,
        killed_by_timeout,
        peak_memory_bytes,
        cpu_time,
    })
}

/// Like [`spawn_shell_with_callback`], but streams output lines through a
/// channel in real-time.  Each line from stdout/stderr is sent as an
/// [`OutputLine`] to the provided sender, enabling live progress display.
///
/// The log file is still written in the same format as [`spawn_shell`].
#[allow(clippy::too_many_arguments)]
pub async fn spawn_shell_streaming(
    command: &str,
    work_dir: &Path,
    log_path: &Path,
    timeout: Option<Duration>,
    env_vars: &[(String, String)],
    shell: &str,
    on_spawn: impl FnOnce(u32),
    output_tx: mpsc::UnboundedSender<OutputLine>,
) -> Result<ProcessResult, ExecLocalError> {
    let start = Instant::now();

    #[cfg(unix)]
    let before = snapshot_rusage_children();

    let mut cmd = Command::new(shell);
    cmd.arg("-c")
        .arg(command)
        .current_dir(work_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    #[cfg(unix)]
    cmd.process_group(0);

    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = cmd.spawn().map_err(ExecLocalError::SpawnFailed)?;

    if let Some(pid) = child.id() {
        on_spawn(pid);
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Read stdout line-by-line, sending each line through the channel and
    // collecting into a buffer for the log file.
    let tx_out = output_tx.clone();
    let stdout_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(stdout) = stdout {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx_out.send(OutputLine {
                    line: line.clone(),
                    is_stderr: false,
                });
                buf.extend_from_slice(line.as_bytes());
                buf.push(b'\n');
            }
        }
        buf
    });

    let tx_err = output_tx;
    let stderr_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx_err.send(OutputLine {
                    line: line.clone(),
                    is_stderr: true,
                });
                buf.extend_from_slice(line.as_bytes());
                buf.push(b'\n');
            }
        }
        buf
    });

    // Write the log file from the collected buffers (same format as non-streaming).
    let log_path_owned = log_path.to_path_buf();
    let copy_handle = tokio::spawn(async move {
        let (stdout_result, stderr_result) = tokio::join!(stdout_handle, stderr_handle);
        let stdout_buf = stdout_result.map_err(std::io::Error::other)?;
        let stderr_buf = stderr_result.map_err(std::io::Error::other)?;

        let mut log_file = tokio::fs::File::create(&log_path_owned).await?;
        log_file.write_all(&stdout_buf).await?;
        if !stderr_buf.is_empty() {
            log_file.write_all(b"\n--- stderr ---\n").await?;
            log_file.write_all(&stderr_buf).await?;
        }
        log_file.flush().await?;
        Ok::<(), std::io::Error>(())
    });

    // Wait for the child, with an optional timeout.
    let (killed_by_timeout, status) = if let Some(dur) = timeout {
        match tokio::time::timeout(dur, child.wait()).await {
            Ok(result) => (false, result?),
            Err(_elapsed) => {
                #[cfg(unix)]
                {
                    if let Some(pid) = child.id() {
                        unsafe {
                            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    child.kill().await.ok();
                }
                let status = child.wait().await?;
                (true, status)
            }
        }
    } else {
        let status = child.wait().await?;
        (false, status)
    };

    // Wait for the pipe drain with a timeout (same rationale as
    // spawn_shell_with_callback — child has exited, drain should be fast).
    match tokio::time::timeout(DRAIN_TIMEOUT, copy_handle).await {
        Ok(result) => {
            let _ = result;
        }
        Err(_elapsed) => {
            // Drain timed out — streaming output may be incomplete but the
            // process result (from child.wait()) is authoritative.
        }
    }

    let duration = start.elapsed();
    let exit_code = status
        .code()
        .unwrap_or(if killed_by_timeout { 137 } else { -1 });

    #[cfg(unix)]
    let (peak_memory_bytes, cpu_time) = {
        let after = snapshot_rusage_children();
        match (before, after) {
            (Some(b), Some(a)) => {
                let cpu = (a.user_time + a.system_time).checked_sub(b.user_time + b.system_time);
                let peak = if a.max_rss_bytes > b.max_rss_bytes {
                    Some(a.max_rss_bytes - b.max_rss_bytes)
                } else {
                    Some(a.max_rss_bytes)
                };
                (peak, cpu)
            }
            _ => (None, None),
        }
    };

    #[cfg(not(unix))]
    let (peak_memory_bytes, cpu_time) = (None, None);

    Ok(ProcessResult {
        exit_code,
        duration,
        killed_by_timeout,
        peak_memory_bytes,
        cpu_time,
    })
}
