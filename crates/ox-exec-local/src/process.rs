//! Process spawning utilities for the local executor.
//!
//! This module provides [`spawn_shell`], which runs a shell command as an
//! async child process, captures stdout/stderr to a log file, enforces an
//! optional timeout, and returns the exit code and wall-clock duration.

use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::process::CommandExt;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::Command;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
    /// Peak resident set size in bytes, if measured for this child.
    ///
    /// On Linux/macOS, `wait4` reports the high-water mark of this child and
    /// its already-reaped descendants, not a simultaneous process-tree total.
    /// Background or daemonised work can escape this observation. Other
    /// platforms and persistent warm-worker dispatches leave this absent.
    pub peak_memory_bytes: Option<u64>,
    /// CPU time (user + system), if measured for this child.
    ///
    /// Taken from the same `wait4` result as RSS on Linux/macOS. Background
    /// or daemonised work can escape it. Persistent warm dispatches remain
    /// unmeasured; fork-mode warm usage is collected by the owning template.
    pub cpu_time: Option<Duration>,
}

/// Arrange for the child to inherit `fds` across `exec`.
///
/// Rust opens every file with `CLOEXEC`, so a plain spawn would close the
/// lock descriptors in the child. The duplicates are made in the child
/// only (after `fork`, before `exec`), so no other concurrently spawned
/// child — for another job of the same session — ever inherits them.
#[cfg(unix)]
fn inherit_descriptors(cmd: &mut Command, fds: &[i32]) {
    if fds.is_empty() {
        return;
    }
    let fds: Vec<i32> = fds.to_vec();
    // SAFETY: the closure runs in the forked child before `exec` and calls
    // only `dup(2)`, which is async-signal-safe; it touches no heap or
    // lock state of the parent beyond reading the moved `fds` vector, and
    // reports failure through the returned `io::Error` instead of panicking.
    unsafe {
        cmd.pre_exec(move || {
            for &fd in &fds {
                // `dup` clears FD_CLOEXEC on the new descriptor, which is
                // exactly what makes it survive `exec`.
                if libc::dup(fd) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
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
        &[],
        |_| {},
    )
    .await
}

/// Like [`spawn_shell`], but calls `on_spawn` with the child's PID immediately
/// after the process is created.  This allows the caller to track the PID for
/// cancellation before the process completes.
///
/// `inherit_fds` are open descriptors of the calling process that the child
/// must keep open: after `fork`, and only in the child, each is duplicated
/// without `CLOEXEC` so the child holds its own reference to the same open
/// file description across `exec`. The local executor passes its output-path
/// lock descriptors here, so an advisory `flock(2)` taken by the session
/// stays held while the job runs even if the session itself is killed
/// (issue #2, round-3 finding 1). Ignored on non-Unix targets.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_shell_with_callback(
    command: &str,
    work_dir: &Path,
    log_path: &Path,
    timeout: Option<Duration>,
    env_vars: &[(String, String)],
    shell: &str,
    inherit_fds: &[i32],
    on_spawn: impl FnOnce(u32),
) -> Result<ProcessResult, ExecLocalError> {
    let start = Instant::now();

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
    #[cfg(unix)]
    inherit_descriptors(&mut cmd, inherit_fds);

    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = spawn_child(&mut cmd).map_err(ExecLocalError::SpawnFailed)?;

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

    let (killed_by_timeout, observed) = wait_child(&mut child, timeout).await?;
    let status = observed.status;

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

    Ok(ProcessResult {
        exit_code,
        duration,
        killed_by_timeout,
        peak_memory_bytes: observed.peak_memory_bytes,
        cpu_time: observed.cpu_time,
    })
}

/// Like [`spawn_shell_with_callback`], but streams output lines through a
/// channel in real-time.  Each line from stdout/stderr is sent as an
/// [`OutputLine`] to the provided sender, enabling live progress display.
///
/// The log file is still written in the same format as [`spawn_shell`];
/// `inherit_fds` behaves as in [`spawn_shell_with_callback`].
#[allow(clippy::too_many_arguments)]
pub async fn spawn_shell_streaming(
    command: &str,
    work_dir: &Path,
    log_path: &Path,
    timeout: Option<Duration>,
    env_vars: &[(String, String)],
    shell: &str,
    inherit_fds: &[i32],
    on_spawn: impl FnOnce(u32),
    output_tx: mpsc::UnboundedSender<OutputLine>,
) -> Result<ProcessResult, ExecLocalError> {
    let start = Instant::now();

    let mut cmd = Command::new(shell);
    cmd.arg("-c")
        .arg(command)
        .current_dir(work_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(unix)]
    inherit_descriptors(&mut cmd, inherit_fds);

    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = spawn_child(&mut cmd).map_err(ExecLocalError::SpawnFailed)?;

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

    let (killed_by_timeout, observed) = wait_child(&mut child, timeout).await?;
    let status = observed.status;

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

    Ok(ProcessResult {
        exit_code,
        duration,
        killed_by_timeout,
        peak_memory_bytes: observed.peak_memory_bytes,
        cpu_time: observed.cpu_time,
    })
}

struct ChildExit {
    status: std::process::ExitStatus,
    peak_memory_bytes: Option<u64>,
    cpu_time: Option<Duration>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
use measured_child::ColdChild;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) use measured_child::platform_rss_bytes;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
type ColdChild = tokio::process::Child;

fn spawn_child(cmd: &mut Command) -> std::io::Result<ColdChild> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        ColdChild::spawn(cmd)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        cmd.spawn()
    }
}

async fn observe_child(child: &mut ColdChild) -> std::io::Result<ChildExit> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        child.wait().await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(ChildExit {
            status: child.wait().await?,
            peak_memory_bytes: None,
            cpu_time: None,
        })
    }
}

/// Shared by buffered and streaming cold launches. A timeout cancels only the
/// async wait, never the reaper ownership, then collects the killed child's usage.
async fn wait_child(
    child: &mut ColdChild,
    timeout: Option<Duration>,
) -> std::io::Result<(bool, ChildExit)> {
    if let Some(dur) = timeout {
        match tokio::time::timeout(dur, observe_child(child)).await {
            Ok(result) => return Ok((false, result?)),
            Err(_) => {
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    // SAFETY: this unreaped child owns the process group created
                    // at spawn; no other code in this module can reap it.
                    unsafe {
                        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                    }
                }
                #[cfg(not(unix))]
                child.kill().await.ok();
                return Ok((true, observe_child(child).await?));
            }
        }
    }
    Ok((false, observe_child(child).await?))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod measured_child {
    use super::{ChildExit, Command, Duration};
    use std::io;
    use std::os::unix::process::ExitStatusExt;
    use std::task::Poll;
    use tokio::signal::unix::{Signal, SignalKind, signal};

    /// Sole reaper for a cold child. `std::process` spawns but never waits;
    /// Tokio owns only the nonblocking pipes, never a process handle.
    pub(super) struct ColdChild {
        pid: Option<libc::pid_t>,
        exited: Signal,
        pub stdout: Option<tokio::process::ChildStdout>,
        pub stderr: Option<tokio::process::ChildStderr>,
    }

    impl ColdChild {
        pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
            // Subscribe before spawning: even an immediate exit must wake us.
            // This listener only notifies; wait4 below remains the sole reaper.
            let exited = signal(SignalKind::child())?;
            let mut process = cmd.spawn()?;
            let mut child = Self {
                pid: Some(process.id() as libc::pid_t),
                exited,
                stdout: None,
                stderr: None,
            };
            // Install reaper ownership before any fallible pipe conversion.
            child.stdout = process
                .stdout
                .take()
                .map(tokio::process::ChildStdout::from_std)
                .transpose()?;
            child.stderr = process
                .stderr
                .take()
                .map(tokio::process::ChildStderr::from_std)
                .transpose()?;
            // Dropping std::process::Child neither kills nor reaps it.
            Ok(child)
        }

        pub fn id(&self) -> Option<u32> {
            self.pid.map(|pid| pid as u32)
        }

        pub async fn wait(&mut self) -> io::Result<ChildExit> {
            let pid = self
                .pid
                .ok_or_else(|| io::Error::other("child already reaped"))?;
            std::future::poll_fn(|cx| {
                loop {
                    // Register the waker BEFORE checking wait4, as Tokio's reaper
                    // does, so an exit between the check and Pending cannot be lost.
                    // Signals may coalesce or belong to another child; always check
                    // this PID, and re-register after consuming a notification.
                    let registered = self.exited.poll_recv(cx).is_pending();
                    match reap(pid, libc::WNOHANG) {
                        Ok(Some(result)) => {
                            self.pid = None;
                            return Poll::Ready(Ok(result));
                        }
                        Ok(None) => {
                            if registered {
                                return Poll::Pending;
                            }
                        }
                        Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                        Err(err) => {
                            if err.raw_os_error() == Some(libc::ECHILD) {
                                self.pid = None;
                            }
                            return Poll::Ready(Err(err));
                        }
                    }
                }
            })
            .await
        }
    }

    impl Drop for ColdChild {
        fn drop(&mut self) {
            if let Some(pid) = self.pid.take() {
                // An aborted async caller must not leave a zombie. Transfer
                // ownership (do not duplicate it) to a detached cleanup thread.
                // Unlike spawn_blocking, it cannot hold Tokio shutdown open
                // while a surviving child runs. Like Tokio Child drop, it
                // does not kill the child.
                let cleanup = std::thread::Builder::new()
                    .name("ox-child-reaper".into())
                    .spawn(move || {
                        loop {
                            match reap(pid, 0) {
                                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                                _ => break,
                            }
                        }
                    });
                if let Err(error) = cleanup {
                    tracing::warn!(pid, %error, "could not start abandoned child reaper");
                }
            }
        }
    }

    fn reap(pid: libc::pid_t, options: i32) -> io::Result<Option<ChildExit>> {
        let mut status = 0;
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: pid names our unreaped child. Both output pointers are valid;
        // usage is read only when wait4 reports that this child was reaped.
        let result = unsafe { libc::wait4(pid, &mut status, options, usage.as_mut_ptr()) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if result == 0 {
            return Ok(None);
        }
        // SAFETY: successful wait4 initialized the rusage output above.
        let usage = unsafe { usage.assume_init() };
        Ok(Some(ChildExit {
            status: std::process::ExitStatus::from_raw(status),
            peak_memory_bytes: platform_rss_bytes(i128::from(usage.ru_maxrss)),
            cpu_time: timeval_duration(usage.ru_utime)
                .and_then(|user| user.checked_add(timeval_duration(usage.ru_stime)?)),
        }))
    }

    enum RssUnit {
        #[cfg(any(target_os = "macos", test))]
        Bytes,
        #[cfg(any(target_os = "linux", test))]
        Kibibytes,
    }

    fn rss_bytes(raw: libc::c_long, unit: RssUnit) -> Option<u64> {
        let raw = u64::try_from(raw).ok()?;
        match unit {
            #[cfg(any(target_os = "macos", test))]
            RssUnit::Bytes => Some(raw),
            #[cfg(any(target_os = "linux", test))]
            RssUnit::Kibibytes => raw.checked_mul(1024),
        }
    }

    /// Convert this platform's `ru_maxrss` unit to bytes.
    ///
    /// Both the native cold-child collector and Python's `os.wait4` warm-fork
    /// collector report the same platform-defined raw value, so they must pass
    /// through this single conversion boundary.
    pub(crate) fn platform_rss_bytes(raw: i128) -> Option<u64> {
        let raw = libc::c_long::try_from(raw).ok()?;
        #[cfg(target_os = "macos")]
        let unit = RssUnit::Bytes;
        #[cfg(target_os = "linux")]
        let unit = RssUnit::Kibibytes;
        rss_bytes(raw, unit)
    }

    fn timeval_duration(value: libc::timeval) -> Option<Duration> {
        let seconds = u64::try_from(value.tv_sec).ok()?;
        let micros = u32::try_from(value.tv_usec).ok()?;
        if micros >= 1_000_000 {
            return None;
        }
        Some(Duration::new(seconds, micros * 1000))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test(start_paused = true)]
        async fn child_exit_wakes_without_advancing_a_poll_timer() {
            use std::future::Future;
            use std::io::Write;
            use std::os::fd::OwnedFd;
            use std::os::unix::net::UnixStream;

            let (reader, mut writer) = UnixStream::pair().unwrap();
            let mut command = Command::new("/bin/bash");
            command.args(["-c", "read line; exit 0"]);
            command.stdin(OwnedFd::from(reader));
            let mut child = ColdChild::spawn(&mut command).unwrap();
            let mut waiting = Box::pin(child.wait());
            // The child cannot exit until we release stdin. First establish a
            // pending wait, so even a slow host cannot skip the old poll sleep.
            std::future::poll_fn(|cx| {
                assert!(waiting.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            writer.write_all(b"go\n").unwrap();
            let start = tokio::time::Instant::now();
            assert!(waiting.await.unwrap().status.success());
            // Paused Tokio time auto-advances to pending timers. SIGCHLD should
            // finish this wait without any clock advance, regardless of the
            // real shell startup time. Restoring the polling loop fails here.
            assert_eq!(start.elapsed(), Duration::ZERO);
        }

        #[tokio::test]
        async fn exit_before_wait_is_reaped_once() {
            let mut command = Command::new("/bin/bash");
            command.args(["-c", "exit 7"]);
            let mut child = ColdChild::spawn(&mut command).unwrap();
            // Observe exit without reaping, independently of signal delivery.
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
                    // SAFETY: info is valid writable storage. WNOWAIT preserves
                    // this owned child's status and usage for the sole reaper.
                    let result = unsafe {
                        libc::waitid(
                            libc::P_PID,
                            child.id().unwrap() as libc::id_t,
                            info.as_mut_ptr(),
                            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                        )
                    };
                    assert_eq!(result, 0);
                    // SAFETY: waitid succeeded and initialized the zeroed info.
                    if unsafe { info.assume_init().si_pid() } != 0 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
            let result = tokio::time::timeout(Duration::from_secs(1), child.wait())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.status.code(), Some(7));
            assert!(result.peak_memory_bytes.is_some());
            assert!(result.cpu_time.is_some());
            assert!(child.id().is_none());
            assert!(child.wait().await.is_err());
        }

        #[test]
        fn rss_units_and_missing_are_distinct_from_zero() {
            assert_eq!(rss_bytes(65_536, RssUnit::Bytes), Some(65_536));
            assert_eq!(rss_bytes(65_536, RssUnit::Kibibytes), Some(67_108_864));
            for unit in [RssUnit::Bytes, RssUnit::Kibibytes] {
                assert_eq!(rss_bytes(0, unit), Some(0));
            }
            assert_eq!(rss_bytes(-1, RssUnit::Bytes), None);
            assert_eq!(rss_bytes(-1, RssUnit::Kibibytes), None);
            #[cfg(target_pointer_width = "64")]
            assert_eq!(rss_bytes(libc::c_long::MAX, RssUnit::Kibibytes), None);

            #[cfg(target_os = "macos")]
            assert_eq!(platform_rss_bytes(65_536), Some(65_536));
            #[cfg(target_os = "linux")]
            assert_eq!(platform_rss_bytes(65_536), Some(67_108_864));
        }

        #[test]
        fn cpu_timeval_preserves_zero_and_rejects_invalid_values() {
            assert_eq!(
                timeval_duration(libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0
                }),
                Some(Duration::ZERO)
            );
            assert_eq!(
                timeval_duration(libc::timeval {
                    tv_sec: 2,
                    tv_usec: 500_000
                }),
                Some(Duration::from_millis(2500))
            );
            assert_eq!(
                timeval_duration(libc::timeval {
                    tv_sec: -1,
                    tv_usec: 0
                }),
                None
            );
            assert_eq!(
                timeval_duration(libc::timeval {
                    tv_sec: 0,
                    tv_usec: 1_000_000
                }),
                None
            );
        }
    }
}
