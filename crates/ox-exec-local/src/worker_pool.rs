//! # Warm Worker Pool
//!
//! Pre-warm Python workers using the fork-after-import pattern.
//! A template process imports all required libraries once, then `fork()`s
//! for each dispatch. The child inherits imported modules via COW pages,
//! executes the function, and exits — giving warm imports with process
//! isolation by construction.
//!
//! ## Architecture
//!
//! ```text
//! Scheduler → WorkerPool::dispatch()
//!                  │
//!                  ▼
//!            Template Process (imports done, waiting on stdin)
//!                  │
//!                  ├── fork() → Child 1: execute, write result, _exit(0)
//!                  ├── fork() → Child 2: execute, write result, _exit(0)
//!                  └── ...
//! ```
//!
//! ## Protocol
//!
//! JSON-line over stdin/stdout:
//! - Worker sends `{"status": "ready"}` after imports complete
//! - Parent sends `{"cmd": "exec", "module": "...", ...}` for each dispatch
//! - Worker sends `{"status": "ok"}` or `{"status": "error", "msg": "..."}`
//! - Parent sends `{"cmd": "shutdown"}` to terminate

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// A warm Python worker process.
struct WarmWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    #[allow(dead_code)]
    script_path: PathBuf,
}

/// Error from worker operations.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("worker spawn failed: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("worker did not become ready within timeout")]
    ReadyTimeout,
    #[error("worker sent invalid ready message: {0}")]
    BadReady(String),
    #[error("dispatch timeout")]
    DispatchTimeout,
    #[error("python error: {0}")]
    PythonError(String),
    #[error("worker I/O error: {0}")]
    Io(std::io::Error),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}

/// Pool of warm Python workers, keyed by environment name.
///
/// For v1: one worker per environment (lazy-initialized).
/// The pool never blocks — if no warm worker is available, the caller
/// falls back to cold subprocess spawn.
pub struct WorkerPool {
    /// One warm worker per environment key.
    workers: tokio::sync::Mutex<HashMap<String, WarmWorker>>,
    /// Directory for generated warmup scripts.
    work_dir: PathBuf,
    /// Execution mode for the dispatch loop.
    mode: crate::call_mode::WarmWorkerMode,
}

impl std::fmt::Debug for WorkerPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerPool")
            .field("work_dir", &self.work_dir)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl WorkerPool {
    /// Create a new empty pool with fork mode (default).
    pub fn new(work_dir: PathBuf) -> Self {
        Self {
            workers: tokio::sync::Mutex::new(HashMap::new()),
            work_dir,
            mode: crate::call_mode::WarmWorkerMode::Fork,
        }
    }

    /// Create a new pool with explicit mode selection.
    pub fn new_with_mode(work_dir: PathBuf, mode: crate::call_mode::WarmWorkerMode) -> Self {
        Self {
            workers: tokio::sync::Mutex::new(HashMap::new()),
            work_dir,
            mode,
        }
    }

    /// The warm worker mode this pool uses.
    pub fn mode(&self) -> crate::call_mode::WarmWorkerMode {
        self.mode
    }

    /// Ensure a warm worker exists for the given environment key.
    /// If one already exists, this is a no-op.
    ///
    /// `argv` is the full command to spawn (e.g., `["uv", "run", ..., "python3", "worker.py"]`).
    /// `warmup_script` is the Python source for the warm worker.
    pub async fn ensure_warm(
        &self,
        env_key: &str,
        argv: &[String],
        warmup_script: &str,
    ) -> Result<(), WorkerError> {
        let mut workers = self.workers.lock().await;
        if workers.contains_key(env_key) {
            return Ok(());
        }

        // Write the warmup script. The last element of argv is the script path.
        let script_path = PathBuf::from(argv.last().expect("argv must have at least one element"));
        tokio::fs::write(&script_path, warmup_script)
            .await
            .map_err(WorkerError::Spawn)?;

        // Spawn the template process.
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(&self.work_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit()) // forward to parent stderr
            .kill_on_drop(true); // prevent zombies

        // Place the template in its own process group so that killing it
        // (timeout, cancel) also kills the fork-after-import children it
        // spawned — otherwise a grandchild mid-execution keeps writing its
        // outputs after the dispatch was abandoned. process_group(0) sets
        // PGID = template PID; forked children inherit it.
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(WorkerError::Spawn)?;
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout piped"));

        let mut worker = WarmWorker {
            child,
            stdin,
            stdout,
            script_path: script_path.clone(),
        };

        // Wait for "ready" with timeout (imports can take seconds for JAX).
        let ready =
            tokio::time::timeout(Duration::from_secs(60), read_json_line(&mut worker.stdout))
                .await
                .map_err(|_| WorkerError::ReadyTimeout)?
                .map_err(WorkerError::Io)?;

        if ready.get("status").and_then(|v| v.as_str()) != Some("ready") {
            return Err(WorkerError::BadReady(ready.to_string()));
        }

        workers.insert(env_key.to_string(), worker);
        Ok(())
    }

    /// Dispatch a job to a warm worker. Returns Ok(()) if the function
    /// executed successfully, Err otherwise. On error, the worker may
    /// be in an undefined state — the caller should fall back to cold spawn.
    pub async fn dispatch(
        &self,
        env_key: &str,
        payload: &serde_json::Value,
        timeout: Duration,
    ) -> Result<(), WorkerError> {
        let mut workers = self.workers.lock().await;
        let worker = match workers.get_mut(env_key) {
            Some(w) => w,
            None => return Err(WorkerError::PythonError("no warm worker for env".into())),
        };

        // Send dispatch command.
        let line = serde_json::to_string(payload).unwrap() + "\n";
        tokio::time::timeout(timeout, worker.stdin.write_all(line.as_bytes()))
            .await
            .map_err(|_| WorkerError::DispatchTimeout)?
            .map_err(WorkerError::Io)?;
        worker.stdin.flush().await.map_err(WorkerError::Io)?;

        // Read response (the template forks, child executes, parent reads child result).
        let response = match tokio::time::timeout(timeout, read_json_line(&mut worker.stdout)).await
        {
            Ok(Ok(val)) => val,
            Ok(Err(e)) => return Err(WorkerError::Io(e)),
            Err(_) => {
                // Timeout — kill the worker, it's in an undefined state.
                kill_worker_group(&worker.child);
                workers.remove(env_key);
                return Err(WorkerError::DispatchTimeout);
            }
        };

        match response.get("status").and_then(|v| v.as_str()) {
            Some("ok") => Ok(()),
            Some("error") => {
                let msg = response["msg"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_string();
                Err(WorkerError::PythonError(msg))
            }
            _ => Err(WorkerError::InvalidResponse(response.to_string())),
        }
    }

    /// Forcefully kill the warm worker for `env_key` (template process
    /// group, including any in-flight forked child) and evict it from the
    /// pool. Returns `true` if a worker was found and killed.
    ///
    /// Used by `cancel()`: warm dispatches execute inside the template's
    /// forked child, so cancelling the job means killing the group.
    pub async fn kill_env(&self, env_key: &str) -> bool {
        let mut workers = self.workers.lock().await;
        match workers.remove(env_key) {
            Some(worker) => {
                kill_worker_group(&worker.child);
                true
            }
            None => false,
        }
    }

    /// Shutdown all workers gracefully.
    pub async fn shutdown(&self) {
        let mut workers = self.workers.lock().await;
        for (_, mut worker) in workers.drain() {
            let _ = worker.stdin.write_all(b"{\"cmd\":\"shutdown\"}\n").await;
            let _ = worker.child.wait().await;
            // Clean up script file.
            let _ = tokio::fs::remove_file(&worker.script_path).await;
        }
    }
}

/// SIGKILL the worker's entire process group (template + forked children).
///
/// The template was spawned with `process_group(0)`, so its PID doubles as
/// the PGID and every fork-after-import child inherits it. Killing only the
/// template PID would leave a grandchild mid-execution writing its outputs.
fn kill_worker_group(child: &Child) {
    if let Some(id) = child.id() {
        #[cfg(unix)]
        unsafe {
            libc::killpg(id as libc::pid_t, libc::SIGKILL);
        }
        #[cfg(not(unix))]
        unsafe {
            libc::kill(id as i32, libc::SIGKILL);
        }
    }
}

/// Read one JSON line from the worker's stdout.
async fn read_json_line(reader: &mut BufReader<ChildStdout>) -> std::io::Result<serde_json::Value> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "worker closed stdout",
        ));
    }
    serde_json::from_str(&line).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
