//! Local executor — runs jobs as child processes on the host machine.
//!
//! [`LocalExecutor`] implements the `Executor` trait from `ox-core` by
//! spawning shell commands through `/bin/bash` (configurable) using
//! [`tokio::process::Command`].
//!
//! ## Execution model
//!
//! Each job is a single child process.  The executor captures stdout/stderr
//! to a log file under the run's log directory, measures wall-clock time,
//! and enforces per-job timeouts by killing the child after the deadline.
//!
//! ## Concurrency
//!
//! - **Sequential** (default): `max_jobs = None` — one job at a time.
//! - **Parallel**: `max_jobs = Some(n)` — up to `n` concurrent jobs.  The
//!   *scheduler* is responsible for limiting parallelism; the executor just
//!   advertises its capacity via [`Executor::max_concurrency`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use ox_core::event::EventBus;
use ox_core::model::{ConcreteJob, EnvSpec, Event, ExecutionBlock, JobId, OutputRef, OutputStream};
use ox_core::traits::executor::*;

use crate::error::ExecLocalError;
use crate::process::{self, ProcessResult};
use tracing::{debug, warn};

/// Suffix appended to output files during the atomic staging phase.
const ATOMIC_TEMP_SUFFIX: &str = ".oxytmp";

/// State carried through the workspace for atomic output finalization.
///
/// Stored in [`Workspace::_private`] via [`Workspace::with_state`] and
/// recovered in `finalize_workspace` via [`Workspace::into_state`].
#[derive(Debug)]
struct AtomicOutputState {
    /// Absolute paths to declared file outputs.
    output_files: Vec<PathBuf>,
}

/// Local executor that runs jobs as subprocesses on the current machine.
///
/// # Examples
///
/// ```no_run
/// use ox_exec_local::executor::LocalExecutor;
///
/// // Sequential executor (one job at a time).
/// let exec = LocalExecutor::new();
///
/// // Parallel executor (up to 4 concurrent jobs).
/// let exec = LocalExecutor::with_max_jobs(4);
/// ```
#[derive(Debug)]
pub struct LocalExecutor {
    /// Maximum concurrent jobs (`None` means 1 — sequential).
    max_jobs: Option<usize>,
    /// Map of running job IDs to their child-process handles, used for
    /// cancellation.  The `u32` is the OS PID.
    running: Arc<Mutex<HashMap<String, u32>>>,
    /// Warm call-mode dispatches in flight: job ID → worker-pool env key.
    /// Warm jobs have no PID of their own (they run inside the template's
    /// forked child), so `cancel()` routes them to the pool's group kill
    /// instead of the PID map.
    warm_running: Arc<Mutex<HashMap<String, String>>>,
    /// Job IDs whose warm dispatch was killed by `cancel()`. Consulted by
    /// `execute()` so a cancelled dispatch returns `Cancelled` instead of
    /// falling back to cold spawn (which would re-run the job).
    warm_cancelled: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Event bus for streaming job output (used when verbosity >= 2).
    event_bus: Option<EventBus>,
    /// Warm worker pool for call-mode jobs (Stage 5: fork-after-import).
    /// When present, call-mode jobs try warm dispatch before falling back
    /// to cold subprocess spawn.
    worker_pool: Option<Arc<crate::worker_pool::WorkerPool>>,
}

impl LocalExecutor {
    /// Create a new sequential local executor (one job at a time).
    pub fn new() -> Self {
        Self {
            max_jobs: None,
            running: Arc::new(Mutex::new(HashMap::new())),
            warm_running: Arc::new(Mutex::new(HashMap::new())),
            warm_cancelled: Arc::new(Mutex::new(std::collections::HashSet::new())),
            event_bus: None,
            worker_pool: None,
        }
    }

    /// Create a local executor that supports up to `max_jobs` concurrent jobs.
    pub fn with_max_jobs(max_jobs: usize) -> Self {
        Self {
            max_jobs: Some(max_jobs),
            running: Arc::new(Mutex::new(HashMap::new())),
            warm_running: Arc::new(Mutex::new(HashMap::new())),
            warm_cancelled: Arc::new(Mutex::new(std::collections::HashSet::new())),
            event_bus: None,
            worker_pool: None,
        }
    }

    /// Attach a warm worker pool for call-mode job acceleration.
    ///
    /// When set, call-mode jobs attempt warm dispatch (fork-after-import)
    /// before falling back to cold subprocess spawn.
    pub fn with_worker_pool(mut self, pool: Arc<crate::worker_pool::WorkerPool>) -> Self {
        self.worker_pool = Some(pool);
        self
    }

    /// Attach an event bus for real-time output streaming.
    ///
    /// When set, the executor emits [`Event::JobOutput`] events for each
    /// line of stdout/stderr during job execution.  Used by `-vv` mode.
    pub fn with_event_bus(mut self, event_bus: EventBus) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Extract the shell command string from a job's execution block.
    ///
    /// For Call mode, returns `None` — the caller must use
    /// [`call_mode::generate_wrapper`] instead.
    fn shell_command(job: &ConcreteJob) -> Result<Option<String>, ExecLocalError> {
        match &job.execution {
            ExecutionBlock::Shell { command } => Ok(Some(command.clone())),
            ExecutionBlock::Run { code, lang } => {
                // Execute inline code via the language interpreter. For Python
                // `run:` blocks (the Snakemake-translation path), inject a
                // preamble defining `input`, `output`, `params`, `wildcards`,
                // `threads`, `log`, and `resources` so the user code sees the
                // same injected objects Snakemake provides.
                let full = if is_python_lang(lang) {
                    format!("{}\n{code}", python_run_preamble(job))
                } else {
                    code.clone()
                };
                Ok(Some(format!("{lang} -c {}", shell_escape(&full))))
            }
            ExecutionBlock::Script { path, lang } => {
                let interpreter = lang.as_deref().unwrap_or("sh");
                Ok(Some(format!("{interpreter} {}", path.display())))
            }
            ExecutionBlock::Call { .. } => Ok(None),
        }
    }
}

impl Default for LocalExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Lexically normalize a path by resolving `.` and `..` components without
/// touching the filesystem.  This is needed because the target directory may
/// not exist yet, so `std::fs::canonicalize` would fail.
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// Minimally escape a string for safe embedding in a shell single-quote context.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Returns `true` if the `run:` block's language is Python (the only language
/// for which we inject a Snakemake-compatible preamble).
fn is_python_lang(lang: &str) -> bool {
    let l = lang.trim();
    l == "python" || l == "python3" || l.starts_with("python")
}

/// Render a Python string literal that safely embeds `s` (handles quotes,
/// backslashes, newlines). JSON string syntax is a subset of Python's, so a
/// JSON-encoded string is a valid Python `str` literal.
fn py_str(s: &str) -> String {
    // serde_json always emits a double-quoted, fully-escaped string.
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// File path of an input/output reference, if it is a concrete file.
fn ref_file_str(reference: &OutputRef) -> Option<String> {
    match reference {
        OutputRef::File(p) => Some(p.display().to_string()),
        _ => None,
    }
}

/// Build the Snakemake-compatible Python preamble for a `run:` block.
///
/// Defines `input`, `output`, `log` (list-like, with attribute access for
/// named entries and a space-joined `str()`), `params`, `wildcards`,
/// `resources` (dict-like with attribute access), and `threads` (int) so the
/// translated Snakemake `run:` body executes against the same namespace it
/// expects.
fn python_run_preamble(job: &ConcreteJob) -> String {
    // Positional file lists.
    let input_paths: Vec<String> = job
        .inputs
        .iter()
        .filter_map(|i| ref_file_str(&i.reference))
        .map(|s| py_str(&s))
        .collect();
    let output_paths: Vec<String> = job
        .outputs
        .iter()
        .filter_map(|o| ref_file_str(&o.reference))
        .map(|s| py_str(&s))
        .collect();

    // Named entries.
    let input_named: Vec<String> = job
        .inputs
        .iter()
        .filter_map(|i| {
            let name = i.name.as_ref()?;
            let path = ref_file_str(&i.reference)?;
            Some(format!("{}: {}", py_str(name), py_str(&path)))
        })
        .collect();
    let output_named: Vec<String> = job
        .outputs
        .iter()
        .filter_map(|o| {
            let name = o.name.as_ref()?;
            let path = ref_file_str(&o.reference)?;
            Some(format!("{}: {}", py_str(name), py_str(&path)))
        })
        .collect();

    // Log paths (stdout + stderr, in that order).
    let mut log_paths: Vec<String> = Vec::new();
    if let Some(ref s) = job.log.stdout {
        log_paths.push(py_str(s));
    }
    if let Some(ref s) = job.log.stderr {
        log_paths.push(py_str(s));
    }

    // params / wildcards as string-keyed dicts.
    let params: Vec<String> = job
        .params
        .iter()
        .map(|(k, v)| format!("{}: {}", py_str(k), py_str(v)))
        .collect();
    let wildcards: Vec<String> = job
        .wildcards
        .iter()
        .map(|(k, v)| format!("{}: {}", py_str(k), py_str(v)))
        .collect();

    // resources as a dict; threads derived from `cpu` (default 1).
    let resources: Vec<String> = job
        .resources
        .iter()
        .map(|(k, v)| {
            let val = match v {
                ox_core::model::ResourceValue::Int(n) => n.to_string(),
                ox_core::model::ResourceValue::Float(f) => f.0.to_string(),
                ox_core::model::ResourceValue::Str(s) => py_str(s),
            };
            format!("{}: {}", py_str(k), val)
        })
        .collect();
    let threads = job
        .resources
        .get("cpu")
        .and_then(|v| v.as_u64())
        .filter(|n| *n > 0)
        .unwrap_or(1);

    format!(
        r#"class _OxyIO(list):
    def __init__(self, items, names):
        list.__init__(self, items)
        self._names = names
        for _k, _v in names.items():
            setattr(self, _k, _v)
    def __str__(self):
        return " ".join(str(_x) for _x in self)
    def get(self, _k, _d=None):
        return self._names.get(_k, _d)
class _OxyNS(dict):
    def __init__(self, d):
        dict.__init__(self, d)
        for _k, _v in d.items():
            setattr(self, _k, _v)
    def __getattr__(self, _k):
        try:
            return self[_k]
        except KeyError:
            raise AttributeError(_k)
input = _OxyIO([{input_list}], {{{input_named}}})
output = _OxyIO([{output_list}], {{{output_named}}})
log = _OxyIO([{log_list}], {{}})
params = _OxyNS({{{params}}})
wildcards = _OxyNS({{{wildcards}}})
resources = _OxyNS({{{resources}}})
threads = {threads}"#,
        input_list = input_paths.join(", "),
        input_named = input_named.join(", "),
        output_list = output_paths.join(", "),
        output_named = output_named.join(", "),
        log_list = log_paths.join(", "),
        params = params.join(", "),
        wildcards = wildcards.join(", "),
        resources = resources.join(", "),
        threads = threads,
    )
}

/// Compute the `.oxytmp` staging path for an output file.
fn temp_path(path: &std::path::Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(ATOMIC_TEMP_SUFFIX);
    PathBuf::from(s)
}

/// Returns `true` if the conda `env` value looks like a YAML file spec
/// (ends with `.yaml` or `.yml`) rather than a named environment.
fn is_conda_file_spec(env: &str) -> bool {
    env.ends_with(".yaml") || env.ends_with(".yml")
}

/// Resolve a job's environment specification into a command wrapper.
///
/// Returns the (possibly wrapped) command string and any extra environment
/// variables that should be injected into the child process.
///
/// - `None` / `System` — pass through unchanged.
/// - `Conda { env }` — wrap with `conda run -n <env>` for named envs, or
///   `conda env create -f <file>` + `conda run` for YAML file specs.
/// - `Docker { image }` — wrap with `docker run --rm <image>`.
/// - `Uv { requirements }` — wrap with `uv run [-r <req>]`.
/// - `Nix { expr }` — wrap with `nix develop <expr> -c`.
/// - `Apptainer { image }` — wrap with `apptainer exec <image>`.
fn resolve_environment(
    command: &str,
    env: &Option<EnvSpec>,
    shell: &str,
) -> Result<(String, Vec<(String, String)>), ExecLocalError> {
    match env {
        None | Some(EnvSpec::System) => Ok((command.to_string(), vec![])),
        Some(EnvSpec::Conda { env: env_val }) => {
            if is_conda_file_spec(env_val) {
                // File-based: create the environment from the YAML spec if it
                // doesn't already exist, then run the command inside it.
                // We derive a stable env name from the file stem so repeated
                // runs reuse the same environment.
                let env_name = std::path::Path::new(env_val)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("oxymake_env");
                let wrapped = format!(
                    "conda env create -f {} -n {} --yes 2>/dev/null || \
                     conda env update -f {} -n {} --prune; \
                     conda run --no-banner -n {} -- {} -c {}",
                    shell_escape(env_val),
                    shell_escape(env_name),
                    shell_escape(env_val),
                    shell_escape(env_name),
                    shell_escape(env_name),
                    shell_escape(shell),
                    shell_escape(command),
                );
                Ok((wrapped, vec![]))
            } else {
                // Named environment: activate directly via `conda run`.
                let wrapped = format!(
                    "conda run --no-banner -n {} -- {} -c {}",
                    shell_escape(env_val),
                    shell_escape(shell),
                    shell_escape(command),
                );
                Ok((wrapped, vec![]))
            }
        }
        Some(EnvSpec::Docker { image }) => {
            // Wrap the command inside a Docker container.  `--rm` ensures
            // the container is cleaned up after exit.
            let wrapped = format!(
                "docker run --rm {} {} -c {}",
                shell_escape(image),
                shell_escape(shell),
                shell_escape(command),
            );
            Ok((wrapped, vec![]))
        }
        Some(EnvSpec::Uv { requirements }) => {
            // Wrap the command with `uv run` so it executes inside a
            // uv-managed Python environment.  If a requirements file is
            // specified, pass it via `-r` so uv installs dependencies
            // before running.
            let req_flag = match requirements {
                Some(req) => format!(" -r {}", shell_escape(req)),
                None => String::new(),
            };
            let wrapped = format!(
                "uv run{req_flag} -- {} -c {}",
                shell_escape(shell),
                shell_escape(command)
            );
            Ok((wrapped, vec![]))
        }
        Some(EnvSpec::Nix { expr }) => {
            // Wrap the command with `nix develop` so it executes inside a
            // Nix-defined development environment.  The `-c` flag passes
            // the command to run inside the devshell.
            let wrapped = format!(
                "nix develop {} -c {} -c {}",
                shell_escape(expr),
                shell_escape(shell),
                shell_escape(command),
            );
            Ok((wrapped, vec![]))
        }
        Some(EnvSpec::Apptainer { image }) => {
            // Wrap the command inside an Apptainer (formerly Singularity)
            // container.  `exec` runs the command and exits.
            let wrapped = format!(
                "apptainer exec {} {} -c {}",
                shell_escape(image),
                shell_escape(shell),
                shell_escape(command),
            );
            Ok((wrapped, vec![]))
        }
    }
}

/// Resolve `InMemory` inputs to temporary files for call-mode execution.
///
/// When a call-mode job has `OutputRef::InMemory` inputs, the data must be
/// materialized to temp files so the generated Python wrapper can read it.
/// Returns `Some(cloned_job)` with InMemory inputs replaced by File refs,
/// or `None` if no InMemory inputs exist (no modification needed).
/// Build the command argv for spawning a warm worker process.
///
/// For `uv` environments, produces `["uv", "run", ..., "python3", "script.py"]`.
/// For no environment, just `["python3", "script.py"]`.
fn warm_worker_argv(job: &ConcreteJob, script_path: &std::path::Path) -> Vec<String> {
    use ox_core::model::EnvSpec;
    let script = script_path.display().to_string();
    match &job.environment {
        Some(EnvSpec::Uv {
            requirements: Some(req),
        }) => vec![
            "uv".into(),
            "run".into(),
            "-r".into(),
            req.clone(),
            "--".into(),
            "python3".into(),
            script,
        ],
        Some(EnvSpec::Uv { requirements: None }) => vec![
            "uv".into(),
            "run".into(),
            "--".into(),
            "python3".into(),
            script,
        ],
        _ => vec!["python3".into(), script],
    }
}

/// Resolve in-memory inputs to temporary files for call-mode execution.
///
/// Call-mode jobs generate a Python wrapper script that reads inputs from
/// disk. This function materializes in-memory data to temp files and
/// rewrites the job's input references to point at them.
///
/// Handles two cases:
/// - `OutputRef::InMemory` — data keyed by type_hint in the memory map
/// - `OutputRef::File` where the file doesn't exist on disk but data is
///   available in the memory map (Stage 2: upstream kept data in memory)
async fn resolve_memory_inputs(
    job: &ConcreteJob,
    work_dir: &std::path::Path,
    memory_map: Option<&ox_core::memory_map::OutputMemoryMap>,
) -> Result<Option<ConcreteJob>, ExecLocalError> {
    let mem_map = match memory_map {
        Some(m) => m,
        None => {
            // No memory map — check if any InMemory inputs exist (which
            // would be an error without a map).
            if job
                .inputs
                .iter()
                .any(|i| matches!(i.reference, OutputRef::InMemory { .. }))
            {
                return Err(ExecLocalError::UnsupportedExecution(
                    "job has InMemory inputs but no memory map is available".into(),
                ));
            }
            return Ok(None);
        }
    };

    let mut resolved = job.clone();
    let mut changed = false;

    for (i, input) in resolved.inputs.iter_mut().enumerate() {
        match &input.reference {
            OutputRef::InMemory { type_hint } => {
                let key = match type_hint {
                    Some(hint) => hint.clone(),
                    None => format!("{}:input:{i}", job.id),
                };

                let data = mem_map.get(&key).ok_or_else(|| {
                    ExecLocalError::UnsupportedExecution(format!(
                        "InMemory input {key:?} not found in memory map"
                    ))
                })?;

                let ext = input.format.as_deref().unwrap_or("bin");
                let temp_path = work_dir.join(format!(".oxymake_mem_{}_{i}.{ext}", job.id));
                tokio::fs::write(&temp_path, &*data).await?;

                input.reference = OutputRef::File(temp_path);
                changed = true;
            }
            // If the file doesn't exist on disk but we have data in the
            // memory map, write it to a temp file. This handles the
            // Stage 2 case where the upstream job's output was kept in
            // memory and the async disk writer hasn't flushed yet.
            OutputRef::File(p) if !p.exists() => {
                let key = ox_core::job_graph::output_ref_key(&input.reference);
                if let Some(data) = mem_map.get(&key) {
                    let ext = input
                        .format
                        .as_deref()
                        .or_else(|| p.extension().and_then(|e| e.to_str()))
                        .unwrap_or("bin");
                    let temp_path = work_dir.join(format!(".oxymake_mem_{}_{i}.{ext}", job.id));
                    tokio::fs::write(&temp_path, &*data).await?;
                    input.reference = OutputRef::File(temp_path);
                    changed = true;
                }
            }
            _ => {}
        }
    }

    Ok(if changed { Some(resolved) } else { None })
}

/// Read the last `n` lines from a log file, returning `None` if the file
/// cannot be read or is empty.
async fn read_log_tail(path: &std::path::Path, n: usize) -> Option<String> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    let tail = lines[start..].join("\n");
    if tail.is_empty() { None } else { Some(tail) }
}

impl Executor for LocalExecutor {
    type Error = ExecLocalError;

    /// Initialize the executor.  No-op for local execution.
    async fn init(&self) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Verify we can spawn a child process by running `true` under the default shell.
    async fn health_check(&self) -> Result<(), Self::Error> {
        let output = tokio::process::Command::new(ox_core::model::DEFAULT_SHELL)
            .arg("-c")
            .arg("true")
            .output()
            .await
            .map_err(ExecLocalError::SpawnFailed)?;

        if !output.status.success() {
            return Err(ExecLocalError::SpawnFailed(std::io::Error::other(format!(
                "health-check command `{} -c true` returned non-zero",
                ox_core::model::DEFAULT_SHELL
            ))));
        }
        Ok(())
    }

    /// Clean up executor resources.  No-op for local execution.
    async fn cleanup(&self) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Advertise local executor capabilities.
    ///
    /// The local executor supports memory passing (same-machine, same-FS)
    /// but not GPU scheduling, streaming, or shadow directories.
    fn capabilities(&self) -> ExecutorCapabilities {
        ExecutorCapabilities {
            supports_memory_passing: true,
            ..ExecutorCapabilities::default()
        }
    }

    /// Return the maximum concurrency this executor supports.
    fn max_concurrency(&self) -> Option<usize> {
        self.max_jobs
    }

    /// Prepare a workspace for job execution.
    ///
    /// Uses the current working directory.  Ensures output directories exist
    /// (like Snakemake's automatic directory creation).  Removes stale outputs
    /// and `.oxytmp` staging files to prevent corrupt cache entries from
    /// previous failed runs.
    async fn prepare_workspace(
        &self,
        job: &ConcreteJob,
        ctx: &ExecContext,
    ) -> Result<Workspace, Self::Error> {
        let work_dir = std::env::current_dir()?;
        let mut output_files = Vec::new();

        // Create output directories before execution.
        for output in &job.outputs {
            if let OutputRef::File(p) = &output.reference {
                let normalized = if p.is_absolute() {
                    // Absolute paths are allowed only if they fall under a
                    // trusted config directory (e.g. {config.results_dir}).
                    let norm = normalize_path(p);
                    let trusted = ctx.trusted_dirs.iter().any(|dir| {
                        let dir_norm = normalize_path(dir);
                        norm.starts_with(&dir_norm)
                    });
                    if !trusted {
                        return Err(ExecLocalError::OutputPathEscapesRoot {
                            path: p.display().to_string(),
                            root: work_dir.display().to_string(),
                        });
                    }
                    norm
                } else {
                    // Validate that the joined path stays within the work
                    // directory by normalizing ".." components lexically (the
                    // directory may not exist yet, so canonicalize() would
                    // fail).
                    let joined = work_dir.join(p);
                    let norm = normalize_path(&joined);
                    if !norm.starts_with(&work_dir) {
                        return Err(ExecLocalError::OutputPathEscapesRoot {
                            path: p.display().to_string(),
                            root: work_dir.display().to_string(),
                        });
                    }
                    norm
                };

                if let Some(parent) = normalized.parent() {
                    if !parent.as_os_str().is_empty() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                }

                // Remove stale .oxytmp files from a previous interrupted run.
                let tmp_path = temp_path(&normalized);
                let _ = tokio::fs::remove_file(&tmp_path).await;

                // Remove existing output files so stale data from a previous
                // failed run cannot masquerade as a valid cache entry.
                let _ = tokio::fs::remove_file(&normalized).await;

                output_files.push(normalized);
            }
        }

        // Stage 2: Materialize in-memory inputs to disk before execution.
        //
        // Shell jobs need real files on disk — there is no way to pass raw
        // bytes to `bash -c "..."`. So we write in-memory data to the
        // expected file path, avoiding the latency of the upstream job's
        // async disk write (the data is already in process memory).
        //
        // Priority: `input_data` (scheduler's memory_store, already
        // Arc<[u8]>) first, then `memory_map` (OutputMemoryMap, populated
        // by the spawned executor task) as fallback.
        for input in &job.inputs {
            if let OutputRef::File(p) = &input.reference {
                let key = ox_core::job_graph::output_ref_key(&input.reference);

                // Check input_data first (scheduler's memory_store), then
                // memory_map (OutputMemoryMap) as fallback. Both use Arc<[u8]>
                // — we hold the Arc while writing, no cloning needed.
                let from_input_data = ctx.input_data.get(&key);
                let from_memory_map;
                let bytes: Option<&[u8]> = if let Some(arc) = from_input_data {
                    Some(arc.as_ref())
                } else if let Some(ref mem_map) = ctx.memory_map {
                    from_memory_map = mem_map.get(&key);
                    from_memory_map.as_deref()
                } else {
                    None
                };

                // Page-cache-aware materialization: if the file already
                // exists on disk, the subprocess can read it directly from
                // the OS page cache (hot after finalize_workspace). Only
                // write from memory when the file does NOT exist — e.g.,
                // Never-policy outputs or outputs from a memory-only path.
                //
                // This avoids the disk→memory→disk round-trip that was
                // causing +7.6% overhead on the PHF pipeline (6.5 GB of
                // unnecessary memcpy).
                let resolved = if p.is_absolute() {
                    p.clone()
                } else {
                    work_dir.join(p)
                };

                let file_exists = tokio::fs::metadata(&resolved).await.is_ok();

                if !file_exists {
                    if let Some(bytes) = bytes {
                        if let Some(parent) = resolved.parent() {
                            if !parent.as_os_str().is_empty() {
                                tokio::fs::create_dir_all(parent).await?;
                            }
                        }
                        tokio::fs::write(&resolved, bytes).await?;
                    }
                }
            }
        }

        let state = AtomicOutputState { output_files };
        Ok(Workspace::with_state(work_dir, state))
    }

    /// Execute a job by spawning a child process.
    ///
    /// The command is derived from the job's [`ExecutionBlock`]:
    /// - `Shell` — run directly via the job's shell (default `/bin/bash`)
    /// - `Run`   — pass inline code to the language interpreter
    /// - `Script`— invoke the script file with the interpreter
    /// - `Call`  — unsupported (returns an error)
    ///
    /// stdout/stderr are captured to `{ctx.log_dir}/{job_id}.log`.
    #[tracing::instrument(
        name = "executor.local.execute",
        skip_all,
        fields(job_id = %job.id, executor = "local"),
    )]
    async fn execute(
        &self,
        job: &ConcreteJob,
        workspace: &Workspace,
        ctx: &ExecContext,
    ) -> Result<JobResult, Self::Error> {
        debug!(
            target: "ox.executor",
            counter = "executor.local.spawn",
            job_id = %job.id,
            "spawn"
        );
        let log_path = ctx.log_dir.join(format!("{}.log", job.id));
        let work_dir = workspace.work_dir.clone();

        // Ensure the log directory exists.
        tokio::fs::create_dir_all(&ctx.log_dir).await?;

        // Stage 2: if any inputs have in-memory data (from
        // ExecContext::input_data), write them to temp files so the
        // subprocess can read them. Build a mapping from output-ref keys
        // to the temp file paths for call-mode wrapper generation.
        let mut mem_temp_files: Vec<PathBuf> = Vec::new();
        let mut resolved_job;
        let job_ref = if !ctx.input_data.is_empty() {
            resolved_job = job.clone();
            for input in &mut resolved_job.inputs {
                let key = ox_core::job_graph::output_ref_key(&input.reference);
                if let Some(data) = ctx.input_data.get(&key) {
                    // Write in-memory data to a temp file.
                    let temp_path =
                        work_dir.join(format!(".oxymake_mem_{}.dat", key.replace(['/', '.'], "_")));
                    tokio::fs::write(&temp_path, data.as_ref()).await?;
                    mem_temp_files.push(temp_path.clone());
                    // Replace the input reference with the temp file path,
                    // preserving the original format hint so codecs resolve.
                    if input.format.is_none() {
                        // Infer format from original file path if available.
                        if let OutputRef::File(p) = &input.reference {
                            input.format = p
                                .extension()
                                .and_then(|e| e.to_str())
                                .map(|s| s.to_string());
                        }
                    }
                    input.reference = OutputRef::File(temp_path);
                }
            }
            &resolved_job
        } else {
            job
        };

        // Determine the shell command — Call mode generates a wrapper script.
        let (raw_command, _wrapper_path) = match Self::shell_command(job_ref)? {
            Some(cmd) => (cmd, None),
            None => {
                // Call mode: resolve InMemory inputs to temp files so the
                // wrapper script can read them. Use `job_ref` (not `job`)
                // so that any input_data rewrites from above are preserved —
                // the wrapper sees temp file paths instead of the original
                // (possibly non-existent) disk paths.
                let resolved_job =
                    resolve_memory_inputs(job_ref, &work_dir, ctx.memory_map.as_ref()).await?;
                let wrapper_job = resolved_job.as_ref().unwrap_or(job_ref);

                // Stage 5: Try warm dispatch (fork-after-import) before cold spawn.
                if let Some(ref pool) = self.worker_pool {
                    // Derive a stable environment key for worker pooling.
                    let env_key = match &wrapper_job.environment {
                        Some(ox_core::model::EnvSpec::Uv {
                            requirements: Some(r),
                        }) => {
                            format!("uv_{}", r.replace(['/', '.'], "_"))
                        }
                        Some(ox_core::model::EnvSpec::Uv { requirements: None }) => {
                            "uv_default".to_string()
                        }
                        Some(other) => format!("{other:?}")
                            .chars()
                            .filter(|c| c.is_alphanumeric() || *c == '_')
                            .collect(),
                        None => "system".to_string(),
                    };
                    let warmup_script = crate::call_mode::generate_warmup_script_with_mode(
                        wrapper_job,
                        pool.mode(),
                    )?;

                    // Build argv: the script path is inside the pool's work_dir.
                    let script_path = work_dir.join(format!(".oxymake_warm_{env_key}.py"));
                    let argv = warm_worker_argv(wrapper_job, &script_path);

                    match pool.ensure_warm(&env_key, &argv, &warmup_script).await {
                        Err(e) => {
                            eprintln!("[warm-workers] failed to warm env {env_key}: {e}");
                        }
                        Ok(()) => {
                            let payload = crate::call_mode::build_dispatch_payload(wrapper_job)?;
                            let timeout = wrapper_job
                                .timeout
                                .unwrap_or(std::time::Duration::from_secs(300));

                            // Track the dispatch so cancel() can route this
                            // job to the pool (warm jobs have no PID in the
                            // `running` map).
                            self.warm_running
                                .lock()
                                .insert(job.id.to_string(), env_key.clone());
                            let dispatch_start = std::time::Instant::now();
                            let dispatch_result = pool.dispatch(&env_key, &payload, timeout).await;
                            self.warm_running.lock().remove(job.id.as_str());
                            match dispatch_result {
                                Ok(()) => {
                                    // Completed before any cancel took effect —
                                    // drop a stale cancel marker if one raced in.
                                    self.warm_cancelled.lock().remove(job.id.as_str());
                                    // Warm dispatch succeeded — skip cold path.
                                    for temp in &mem_temp_files {
                                        let _ = tokio::fs::remove_file(temp).await;
                                    }
                                    return Ok(JobResult {
                                        job_id: job.id.clone(),
                                        exit_code: 0,
                                        duration: dispatch_start.elapsed(),
                                        peak_memory_bytes: None,
                                        cpu_time: None,
                                        log_path: None,
                                        stderr_tail: None,
                                    });
                                }
                                Err(e) => {
                                    // If the dispatch died because cancel()
                                    // killed the worker, surface the
                                    // cancellation — falling back to cold
                                    // spawn would re-run a cancelled job.
                                    if self.warm_cancelled.lock().remove(job.id.as_str()) {
                                        return Err(ExecLocalError::Cancelled);
                                    }
                                    // Warm dispatch failed — fall through to cold spawn.
                                    eprintln!(
                                        "[warm-workers] dispatch failed for {}: {e}, falling back to cold spawn",
                                        job.id
                                    );
                                }
                            }
                        } // Ok(()) arm
                    } // match ensure_warm
                }

                // Cold path: generate a Python wrapper script.
                let script = crate::call_mode::generate_wrapper(wrapper_job)?;
                let script_path = work_dir.join(format!(".oxymake_call_{}.py", wrapper_job.id));
                tokio::fs::write(&script_path, &script).await?;
                let cmd = crate::call_mode::wrapper_command(&script_path);
                (cmd, Some(script_path))
            }
        };

        let shell = job
            .shell_executable
            .as_deref()
            .unwrap_or(ox_core::model::DEFAULT_SHELL);
        let (command, env_vars) = resolve_environment(&raw_command, &job.environment, shell)?;

        // Log the resolved command so users can reproduce manually.
        if let Some(ref event_bus) = self.event_bus {
            let mut repro = format!("cd {} && ", work_dir.display());
            for (k, v) in &env_vars {
                repro.push_str(&format!("{}={} ", k, shell_escape(v)));
            }
            repro.push_str(&format!("{} -c {}", shell, shell_escape(&command)));
            event_bus.emit(Event::ExecutorMessage {
                executor: "local".into(),
                message: format!("[{}] {}", job.id, repro),
            });
        }

        // Spawn the process and track its PID for cancellation.
        let job_id_str = job.id.to_string();
        let running = Arc::clone(&self.running);
        let result: ProcessResult = if let Some(ref event_bus) = self.event_bus {
            // Streaming mode: emit JobOutput events for each output line.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<process::OutputLine>();
            let bus = event_bus.clone();
            let jid = job.id.clone();

            // Spawn a task that forwards output lines to the event bus.
            let forward_handle = tokio::spawn(async move {
                while let Some(output_line) = rx.recv().await {
                    let stream = if output_line.is_stderr {
                        OutputStream::Stderr
                    } else {
                        OutputStream::Stdout
                    };
                    bus.emit(Event::JobOutput {
                        job_id: jid.clone(),
                        line: output_line.line,
                        stream,
                    });
                }
            });

            let job_id_clone = job_id_str.clone();
            let r = process::spawn_shell_streaming(
                &command,
                &work_dir,
                &log_path,
                job.timeout,
                &env_vars,
                shell,
                |pid| {
                    running.lock().insert(job_id_clone, pid);
                },
                tx,
            )
            .await?;

            // Wait for all output to be forwarded.
            let _ = forward_handle.await;
            r
        } else {
            // Non-streaming mode: buffer output and write to log.
            let job_id_clone = job_id_str.clone();
            process::spawn_shell_with_callback(
                &command,
                &work_dir,
                &log_path,
                job.timeout,
                &env_vars,
                shell,
                |pid| {
                    running.lock().insert(job_id_clone, pid);
                },
            )
            .await?
        };

        // Clean up the wrapper script and in-memory temp files (best-effort).
        if let Some(ref wrapper) = _wrapper_path {
            let _ = tokio::fs::remove_file(wrapper).await;
        }
        for temp in &mem_temp_files {
            let _ = tokio::fs::remove_file(temp).await;
        }

        // Remove from the running map (if it was tracked).
        {
            self.running.lock().remove(job.id.as_str());
        }

        if result.killed_by_timeout {
            return Err(ExecLocalError::Timeout {
                timeout_secs: job.timeout.map(|d| d.as_secs()).unwrap_or(0),
            });
        }

        // On failure, capture the last 20 lines of the log as stderr_tail.
        let stderr_tail = if result.exit_code != 0 {
            read_log_tail(&log_path, 20).await
        } else {
            None
        };

        Ok(JobResult {
            job_id: job.id.clone(),
            exit_code: result.exit_code,
            duration: result.duration,
            peak_memory_bytes: result.peak_memory_bytes,
            cpu_time: result.cpu_time,
            log_path: Some(log_path),
            stderr_tail,
        })
    }

    /// Finalize the workspace after execution.
    ///
    /// Implements the atomic write protocol:
    /// - **On failure** (exit != 0): delete all declared output files to
    ///   prevent partial/corrupt data from being cached.
    /// - **On success** (exit == 0): verify all outputs exist, then
    ///   atomically commit them via a two-phase rename:
    ///   1. Stage: rename each `output` → `output.oxytmp`
    ///   2. Commit: rename each `output.oxytmp` → `output`
    ///
    ///   This ensures multi-output rules are all-or-nothing: either every
    ///   output is visible at its final path or none are.
    async fn finalize_workspace(
        &self,
        workspace: Workspace,
        result: &JobResult,
    ) -> Result<(), Self::Error> {
        let state: AtomicOutputState = match workspace.into_state() {
            Some(s) => s,
            None => return Ok(()), // No file outputs to finalize.
        };

        if state.output_files.is_empty() {
            return Ok(());
        }

        if result.exit_code != 0 {
            // Failure: clean up any partial outputs.
            for path in &state.output_files {
                let _ = tokio::fs::remove_file(path).await;
                let _ = tokio::fs::remove_file(temp_path(path)).await;
            }
            return Ok(());
        }

        // Success: verify all outputs exist.
        for path in &state.output_files {
            if !path.exists() {
                // Clean up any outputs that WERE produced.
                for p in &state.output_files {
                    let _ = tokio::fs::remove_file(p).await;
                }
                return Err(ExecLocalError::OutputMissing {
                    path: path.display().to_string(),
                });
            }
        }

        // Phase 1 — Stage: move all outputs to .oxytmp.
        // After this phase, no output is visible at its final path.
        let mut staged: Vec<&PathBuf> = Vec::with_capacity(state.output_files.len());
        for path in &state.output_files {
            let tmp = temp_path(path);
            if let Err(e) = tokio::fs::rename(path, &tmp).await {
                // Rollback: move already-staged files back.
                for staged_path in &staged {
                    let _ = tokio::fs::rename(&temp_path(staged_path), staged_path.as_path()).await;
                }
                return Err(ExecLocalError::AtomicWriteFailed {
                    path: path.display().to_string(),
                    reason: format!("stage rename failed: {e}"),
                });
            }
            staged.push(path);
        }

        // Phase 2 — Commit: move all .oxytmp to final paths.
        // Each individual rename(2) is atomic on POSIX (same filesystem).
        for path in &state.output_files {
            let tmp = temp_path(path);
            if let Err(e) = tokio::fs::rename(&tmp, path).await {
                // Best-effort cleanup of remaining .oxytmp files.
                for p in &state.output_files {
                    let _ = tokio::fs::remove_file(&temp_path(p)).await;
                    let _ = tokio::fs::remove_file(p).await;
                }
                return Err(ExecLocalError::AtomicWriteFailed {
                    path: path.display().to_string(),
                    reason: format!("commit rename failed: {e}"),
                });
            }
        }

        Ok(())
    }

    /// Cancel a running job by sending SIGTERM to its entire process group.
    ///
    /// Uses `libc::killpg` directly instead of forking a `kill` process,
    /// avoiding the overhead and lock-contention issues of spawning a child
    /// while holding the running-jobs mutex.
    #[tracing::instrument(
        name = "executor.local.cancel",
        skip_all,
        fields(job_id = %job_id),
    )]
    async fn cancel(&self, job_id: &JobId) -> Result<(), Self::Error> {
        let pid = self.running.lock().get(job_id.as_str()).copied();
        if let Some(pid) = pid {
            debug!(
                target: "ox.executor",
                counter = "executor.local.cancel",
                job_id = %job_id,
                pid,
                "SIGTERM"
            );
            #[cfg(unix)]
            {
                // Safety: killpg with a valid PGID is a standard POSIX
                // syscall.  The process group was created by
                // process_group(0) in spawn_shell_with_callback.
                unsafe {
                    libc::killpg(pid as libc::pid_t, libc::SIGTERM);
                }
            }
            #[cfg(not(unix))]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .status();
            }
            return Ok(());
        }

        // Warm call-mode dispatches have no PID of their own — they run
        // inside the worker pool's forked child. Mark the job cancelled
        // (so execute() surfaces Cancelled instead of falling back to a
        // cold re-run) and kill the worker's process group via the pool.
        let env_key = self.warm_running.lock().get(job_id.as_str()).cloned();
        if let Some(env_key) = env_key {
            debug!(
                target: "ox.executor",
                counter = "executor.local.cancel",
                job_id = %job_id,
                env_key = %env_key,
                "killing warm worker group"
            );
            self.warm_cancelled.lock().insert(job_id.to_string());
            if let Some(ref pool) = self.worker_pool {
                pool.kill_env(&env_key).await;
            }
            return Ok(());
        }

        warn!(
            target: "ox.executor",
            job_id = %job_id,
            "cancel called but job not in running map"
        );
        Ok(())
    }

    /// Poll the status of a job.
    ///
    /// For the local executor, jobs are executed synchronously within
    /// `execute`, so polling always returns [`JobStatus::Completed`].
    /// A real polling implementation would be needed for async submission
    /// backends (SLURM, K8s).
    async fn poll_status(&self, _job_id: &JobId) -> Result<JobStatus, Self::Error> {
        // Local jobs block in execute(), so by the time anyone polls they
        // are already done.
        Ok(JobStatus::Completed)
    }

    async fn submit_dag(
        &self,
        _graph: &ox_core::job_graph::JobGraph,
        _ctx: &ExecContext,
    ) -> Result<DagSubmission, Self::Error> {
        Err(ExecLocalError::UnsupportedExecution(
            "DAG-level submission is not supported by the local executor; \
             use the scheduler loop instead"
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    // The cwd-serialization tests below hold `CWD_LOCK` across an `.await`
    // (the unit under test runs while cwd is overridden). That is the intended
    // mutual exclusion, not the executor-blocking hazard the lint guards
    // against — these are synchronous-style tests on a test runtime.
    #![allow(clippy::await_holding_lock)]

    use super::*;

    /// `std::env::set_current_dir` mutates *process-global* state, so the tests
    /// that change the working directory must not run concurrently with each
    /// other (cargo runs tests on parallel threads within one process). They
    /// serialize on this lock. `unwrap_or_else(into_inner)` tolerates a poisoned
    /// lock so one panicking cwd test does not cascade-fail the rest.
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// H23: warm call-mode dispatches execute inside the worker pool's
    /// forked child and never enter the PID-keyed `running` map — so
    /// `cancel()` was a silent no-op and Ctrl-C left Python jobs running.
    /// A cancel for a warm-tracked job must kill the worker's process
    /// group via the pool and mark the job so `execute()` does not fall
    /// back to cold spawn.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_routes_warm_dispatch_to_pool() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let work_dir = dir.path().to_path_buf();

        // Fake warm worker: announces ready, then stalls on stdin.
        let script_path = work_dir.join("fake_worker.sh");
        std::fs::write(
            &script_path,
            "#!/bin/sh\nprintf '{\"status\":\"ready\"}\\n'\nwhile read -r line; do :; done\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        let dummy_warmup = work_dir.join("unused_warmup.sh");
        let argv = vec![
            "/bin/sh".to_string(),
            script_path.to_string_lossy().into_owned(),
            dummy_warmup.to_string_lossy().into_owned(),
        ];

        let pool = Arc::new(crate::worker_pool::WorkerPool::new(work_dir));
        pool.ensure_warm("env-w", &argv, "# unused\n")
            .await
            .unwrap();

        let executor = LocalExecutor::new().with_worker_pool(Arc::clone(&pool));

        // Simulate execute() having registered a warm dispatch in flight.
        let job_id = JobId::from("job-warm-1");
        executor
            .warm_running
            .lock()
            .insert(job_id.to_string(), "env-w".to_string());

        executor.cancel(&job_id).await.unwrap();

        // The job must be marked cancelled so execute() returns Cancelled
        // instead of re-running the job via cold spawn.
        assert!(
            executor.warm_cancelled.lock().contains(job_id.as_str()),
            "cancel() must mark the warm job as cancelled"
        );

        // The worker (and its process group) must be gone from the pool —
        // behaviourally: a dispatch on the same env key finds no worker.
        let result = pool
            .dispatch(
                "env-w",
                &serde_json::json!({"cmd": "exec"}),
                std::time::Duration::from_millis(200),
            )
            .await;
        assert!(
            matches!(result, Err(crate::worker_pool::WorkerError::PythonError(_))),
            "warm worker must be evicted after cancel, got: {result:?}"
        );
    }

    #[test]
    fn is_conda_file_spec_detects_yaml() {
        assert!(is_conda_file_spec("env.yaml"));
        assert!(is_conda_file_spec("env.yml"));
        assert!(is_conda_file_spec("path/to/environment.yaml"));
        assert!(is_conda_file_spec("path/to/environment.yml"));
        assert!(!is_conda_file_spec("myenv"));
        assert!(!is_conda_file_spec("bioinfo"));
        assert!(!is_conda_file_spec("yaml_env"));
    }

    #[test]
    fn resolve_env_none_passthrough() {
        let (cmd, vars) =
            resolve_environment("echo hi", &None, ox_core::model::DEFAULT_SHELL).unwrap();
        assert_eq!(cmd, "echo hi");
        assert!(vars.is_empty());
    }

    #[test]
    fn resolve_env_system_passthrough() {
        let (cmd, vars) = resolve_environment(
            "echo hi",
            &Some(EnvSpec::System),
            ox_core::model::DEFAULT_SHELL,
        )
        .unwrap();
        assert_eq!(cmd, "echo hi");
        assert!(vars.is_empty());
    }

    #[test]
    fn resolve_env_conda_named() {
        let (cmd, _) = resolve_environment(
            "bwa mem ref.fa",
            &Some(EnvSpec::Conda {
                env: "bioinfo".into(),
            }),
            ox_core::model::DEFAULT_SHELL,
        )
        .unwrap();
        assert!(cmd.contains("conda run"));
        assert!(cmd.contains("-n 'bioinfo'"));
        assert!(!cmd.contains("env create"));
    }

    #[test]
    fn resolve_env_conda_yaml_file() {
        let (cmd, _) = resolve_environment(
            "python train.py",
            &Some(EnvSpec::Conda {
                env: "env.yaml".into(),
            }),
            ox_core::model::DEFAULT_SHELL,
        )
        .unwrap();
        assert!(cmd.contains("conda env create -f 'env.yaml'"));
        assert!(cmd.contains("-n 'env'"));
        assert!(cmd.contains("conda env update -f 'env.yaml'"));
        assert!(cmd.contains("conda run --no-banner -n 'env'"));
    }

    #[test]
    fn resolve_env_conda_yml_file() {
        let (cmd, _) = resolve_environment(
            "python train.py",
            &Some(EnvSpec::Conda {
                env: "environment.yml".into(),
            }),
            ox_core::model::DEFAULT_SHELL,
        )
        .unwrap();
        assert!(cmd.contains("conda env create -f 'environment.yml'"));
        assert!(cmd.contains("-n 'environment'"));
    }

    #[test]
    fn resolve_env_conda_yaml_with_path() {
        let (cmd, _) = resolve_environment(
            "echo hi",
            &Some(EnvSpec::Conda {
                env: "configs/ml-env.yaml".into(),
            }),
            ox_core::model::DEFAULT_SHELL,
        )
        .unwrap();
        assert!(cmd.contains("conda env create -f 'configs/ml-env.yaml'"));
        assert!(cmd.contains("-n 'ml-env'"));
    }

    #[test]
    fn resolve_env_docker() {
        let (cmd, _) = resolve_environment(
            "echo hi",
            &Some(EnvSpec::Docker {
                image: "python:3.12".into(),
            }),
            ox_core::model::DEFAULT_SHELL,
        )
        .unwrap();
        assert!(cmd.contains("docker run --rm"));
        assert!(cmd.contains("'python:3.12'"));
    }

    #[test]
    fn resolve_env_uv_with_requirements() {
        let (cmd, _) = resolve_environment(
            "echo hi",
            &Some(EnvSpec::Uv {
                requirements: Some("req.txt".into()),
            }),
            ox_core::model::DEFAULT_SHELL,
        )
        .unwrap();
        assert!(cmd.contains("uv run"));
        assert!(cmd.contains("-r 'req.txt'"));
    }

    #[test]
    fn resolve_env_uv_without_requirements() {
        let (cmd, _) = resolve_environment(
            "echo hi",
            &Some(EnvSpec::Uv { requirements: None }),
            ox_core::model::DEFAULT_SHELL,
        )
        .unwrap();
        assert!(cmd.contains("uv run"));
        assert!(!cmd.contains("-r"));
    }

    #[test]
    fn resolve_env_nix() {
        let (cmd, _) = resolve_environment(
            "echo hi",
            &Some(EnvSpec::Nix {
                expr: "shell.nix".into(),
            }),
            ox_core::model::DEFAULT_SHELL,
        )
        .unwrap();
        assert!(cmd.contains("nix develop"));
        assert!(cmd.contains("'shell.nix'"));
    }

    #[test]
    fn resolve_env_apptainer() {
        let (cmd, _) = resolve_environment(
            "echo hi",
            &Some(EnvSpec::Apptainer {
                image: "container.sif".into(),
            }),
            ox_core::model::DEFAULT_SHELL,
        )
        .unwrap();
        assert!(cmd.contains("apptainer exec"));
        assert!(cmd.contains("'container.sif'"));
    }

    #[test]
    fn resolve_env_shell_is_escaped() {
        // Regression: shell parameter must be wrapped in shell_escape() to
        // prevent injection (ox-v9jb).
        let evil_shell = "sh; rm -rf /";
        let envs: Vec<Option<EnvSpec>> = vec![
            Some(EnvSpec::Conda {
                env: "bioinfo".into(),
            }),
            Some(EnvSpec::Conda {
                env: "env.yaml".into(),
            }),
            Some(EnvSpec::Docker {
                image: "python:3.12".into(),
            }),
            Some(EnvSpec::Uv { requirements: None }),
            Some(EnvSpec::Nix {
                expr: "shell.nix".into(),
            }),
            Some(EnvSpec::Apptainer {
                image: "container.sif".into(),
            }),
        ];
        for env in &envs {
            let (cmd, _) = resolve_environment("echo hi", env, evil_shell).unwrap();
            assert!(
                cmd.contains(&shell_escape(evil_shell)),
                "shell not escaped in env {:?}: {cmd}",
                env,
            );
            assert!(
                !cmd.contains(&format!(" {} ", evil_shell)),
                "raw shell found unescaped in env {:?}: {cmd}",
                env,
            );
        }
    }

    #[test]
    fn is_python_lang_variants() {
        assert!(is_python_lang("python"));
        assert!(is_python_lang("python3"));
        assert!(is_python_lang("python3.12"));
        assert!(is_python_lang(" python "));
        assert!(!is_python_lang("bash"));
        assert!(!is_python_lang("Rscript"));
    }

    #[test]
    fn py_str_escapes_quotes_and_backslashes() {
        assert_eq!(py_str("a"), "\"a\"");
        assert_eq!(py_str("a\"b"), "\"a\\\"b\"");
        assert_eq!(py_str("a\\b"), "\"a\\\\b\"");
    }

    fn run_job(
        code: &str,
        inputs: Vec<ResolvedInput>,
        outputs: Vec<ResolvedOutput>,
    ) -> ConcreteJob {
        use ox_core::model::*;
        ConcreteJob {
            id: JobId::from("run-test"),
            rule: RuleName::from("analyze"),
            wildcards: std::collections::BTreeMap::from([("sample".into(), "s1".into())]),
            tags: std::collections::BTreeMap::new(),
            inputs,
            outputs,
            execution: ExecutionBlock::Run {
                code: code.into(),
                lang: "python".into(),
            },
            resources: std::collections::BTreeMap::from([("cpu".into(), ResourceValue::Int(4))]),
            environment: None,
            error_strategy: ErrorStrategy::default(),
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: std::collections::BTreeMap::from([("seed".into(), "42".into())]),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        }
    }

    #[test]
    fn python_run_preamble_defines_snakemake_objects() {
        use ox_core::model::*;
        let job = run_job(
            "pass",
            vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("data/s1.csv")),
                name: Some("table".into()),
                format: None,
            }],
            vec![ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("results/s1.json")),
                name: None,
                format: None,
                lifecycle: OutputLifecycle::default(),
                materialize: MaterializePolicy::default(),
            }],
        );
        let preamble = python_run_preamble(&job);
        assert!(
            preamble.contains(r#"input = _OxyIO(["data/s1.csv"]"#),
            "{preamble}"
        );
        assert!(preamble.contains(r#""table": "data/s1.csv""#), "{preamble}");
        assert!(
            preamble.contains(r#"output = _OxyIO(["results/s1.json"]"#),
            "{preamble}"
        );
        assert!(preamble.contains(r#""seed": "42""#), "{preamble}");
        assert!(preamble.contains(r#""sample": "s1""#), "{preamble}");
        assert!(preamble.contains("threads = 4"), "{preamble}");
    }

    /// End-to-end: the preamble + `input[0]` body must actually run under
    /// python3 without the `'builtin_function_or_method'` TypeError.
    #[tokio::test]
    async fn run_block_input_indexing_executes() {
        use ox_core::model::*;
        let dir = tempfile::tempdir().unwrap();
        let in_path = dir.path().join("in.txt");
        let out_path = dir.path().join("out.txt");
        std::fs::write(&in_path, "hello").unwrap();

        let code = "with open(input[0]) as f:\n    data = f.read()\nwith open(output[0], 'w') as f:\n    f.write(data.upper())\n";
        let job = run_job(
            code,
            vec![ResolvedInput {
                reference: OutputRef::File(in_path.clone()),
                name: None,
                format: None,
            }],
            vec![ResolvedOutput {
                reference: OutputRef::File(out_path.clone()),
                name: None,
                format: None,
                lifecycle: OutputLifecycle::default(),
                materialize: MaterializePolicy::default(),
            }],
        );

        let cmd = LocalExecutor::shell_command(&job).unwrap().unwrap();
        // Pin the child's cwd to the tempdir: a concurrent cwd-mutating test
        // (CWD_LOCK group) can delete the process cwd out from under us, which
        // would make the child spawn fail for reasons unrelated to this test.
        let status = tokio::process::Command::new(ox_core::model::DEFAULT_SHELL)
            .arg("-c")
            .arg(&cmd)
            .current_dir(dir.path())
            .status()
            .await
            .unwrap();
        assert!(status.success(), "run block should execute cleanly");
        assert_eq!(std::fs::read_to_string(&out_path).unwrap(), "HELLO");
    }

    #[test]
    fn temp_path_appends_suffix() {
        let p = std::path::Path::new("/tmp/output.csv");
        let t = temp_path(p);
        assert_eq!(t, PathBuf::from("/tmp/output.csv.oxytmp"));
    }

    // -- Atomic write tests --------------------------------------------------

    use ox_core::model::{MaterializePolicy, OutputLifecycle, ResolvedInput, ResolvedOutput};
    use ox_core::traits::executor::{JobResult, Workspace};
    use std::time::Duration;

    fn make_result(exit_code: i32) -> JobResult {
        JobResult {
            job_id: ox_core::model::JobId::from("test-job"),
            exit_code,
            duration: Duration::from_millis(100),
            peak_memory_bytes: None,
            cpu_time: None,
            log_path: None,
            stderr_tail: None,
        }
    }

    fn make_atomic_workspace(work_dir: PathBuf, output_files: Vec<PathBuf>) -> Workspace {
        Workspace::with_state(work_dir, AtomicOutputState { output_files })
    }

    #[tokio::test]
    async fn finalize_success_single_output() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("result.csv");
        std::fs::write(&out, "header\nrow1").unwrap();

        let ws = make_atomic_workspace(dir.path().to_path_buf(), vec![out.clone()]);
        let exec = LocalExecutor::new();
        exec.finalize_workspace(ws, &make_result(0)).await.unwrap();

        // Output should still exist at its final path.
        assert!(out.exists());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "header\nrow1");
        // No .oxytmp left behind.
        assert!(!temp_path(&out).exists());
    }

    #[tokio::test]
    async fn finalize_success_multi_output_atomicity() {
        let dir = tempfile::tempdir().unwrap();
        let out1 = dir.path().join("a.csv");
        let out2 = dir.path().join("b.csv");
        std::fs::write(&out1, "a-data").unwrap();
        std::fs::write(&out2, "b-data").unwrap();

        let ws = make_atomic_workspace(dir.path().to_path_buf(), vec![out1.clone(), out2.clone()]);
        let exec = LocalExecutor::new();
        exec.finalize_workspace(ws, &make_result(0)).await.unwrap();

        assert!(out1.exists());
        assert!(out2.exists());
        assert!(!temp_path(&out1).exists());
        assert!(!temp_path(&out2).exists());
    }

    #[tokio::test]
    async fn finalize_failure_cleans_partial_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("result.csv");
        std::fs::write(&out, "partial-junk").unwrap();

        let ws = make_atomic_workspace(dir.path().to_path_buf(), vec![out.clone()]);
        let exec = LocalExecutor::new();
        exec.finalize_workspace(ws, &make_result(1)).await.unwrap();

        // Output should be cleaned up on failure.
        assert!(!out.exists());
    }

    #[tokio::test]
    async fn finalize_success_missing_output_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("never_created.csv");

        let ws = make_atomic_workspace(dir.path().to_path_buf(), vec![out.clone()]);
        let exec = LocalExecutor::new();
        let err = exec
            .finalize_workspace(ws, &make_result(0))
            .await
            .unwrap_err();

        assert!(
            matches!(err, ExecLocalError::OutputMissing { .. }),
            "expected OutputMissing, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn finalize_no_outputs_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let ws = make_atomic_workspace(dir.path().to_path_buf(), vec![]);
        let exec = LocalExecutor::new();
        exec.finalize_workspace(ws, &make_result(0)).await.unwrap();
    }

    #[tokio::test]
    async fn finalize_no_state_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path().to_path_buf());
        let exec = LocalExecutor::new();
        // Workspace without AtomicOutputState — should be a no-op.
        exec.finalize_workspace(ws, &make_result(0)).await.unwrap();
    }

    #[tokio::test]
    async fn prepare_removes_stale_oxytmp() {
        let dir = tempfile::tempdir().unwrap();
        let out_rel = std::path::Path::new("output.csv");
        let out_abs = dir.path().join(out_rel);
        let tmp = temp_path(&out_abs);

        // Simulate a stale .oxytmp from a previous crash.
        std::fs::write(&tmp, "stale").unwrap();

        let job = ox_core::model::ConcreteJob {
            id: ox_core::model::JobId::from("j1"),
            rule: ox_core::model::RuleName::from("r1"),
            wildcards: std::collections::BTreeMap::new(),
            tags: std::collections::BTreeMap::new(),
            inputs: vec![],
            outputs: vec![ResolvedOutput {
                reference: OutputRef::File(out_rel.to_path_buf()),
                name: None,
                format: None,
                lifecycle: OutputLifecycle::default(),
                materialize: MaterializePolicy::default(),
            }],
            execution: ExecutionBlock::Shell {
                command: "true".into(),
            },
            resources: std::collections::BTreeMap::new(),
            environment: None,
            error_strategy: ox_core::model::ErrorStrategy::default(),
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: std::collections::BTreeMap::new(),
            param_files: Vec::new(),
            log: ox_core::model::LogConfig::default(),
            shell_executable: None,
            reproducibility: ox_core::model::ReproducibilityClass::default(),
        };

        let ctx = ox_core::traits::executor::ExecContext {
            global_job_limit: 1,
            run_id: "test".into(),
            log_dir: dir.path().join("logs"),
            project_dir: dir.path().to_path_buf(),
            trusted_dirs: vec![],
            input_data: std::collections::HashMap::new(),
            memory_map: None,
        };

        // Override cwd to the temp dir so prepare_workspace resolves correctly.
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let exec = LocalExecutor::new();
        let result = exec.prepare_workspace(&job, &ctx).await;
        std::env::set_current_dir(&prev).unwrap();

        let _ws = result.unwrap();
        assert!(!tmp.exists(), "stale .oxytmp should be removed");
        assert!(!out_abs.exists(), "stale output should be removed");
    }

    // -- Path handling tests (ox-58w3) ----------------------------------------

    #[test]
    fn normalize_path_resolves_parent_components() {
        let p = std::path::Path::new("/a/b/../c/./d");
        assert_eq!(normalize_path(p), PathBuf::from("/a/c/d"));
    }

    #[test]
    fn normalize_path_absolute_trailing_slash() {
        let p = std::path::Path::new("/a/b/c/");
        assert_eq!(normalize_path(p), PathBuf::from("/a/b/c"));
    }

    #[test]
    fn normalize_path_relative_dotdot() {
        let p = std::path::Path::new("a/b/../../c");
        assert_eq!(normalize_path(p), PathBuf::from("c"));
    }

    #[test]
    fn normalize_path_preserves_unicode() {
        let p = std::path::Path::new("/données/résultats/../output");
        assert_eq!(normalize_path(p), PathBuf::from("/données/output"));
    }

    #[test]
    fn normalize_path_preserves_spaces() {
        let p = std::path::Path::new("/my project/sub dir/../out put");
        assert_eq!(normalize_path(p), PathBuf::from("/my project/out put"));
    }

    #[test]
    fn shell_escape_handles_spaces() {
        assert_eq!(shell_escape("my file.txt"), "'my file.txt'");
    }

    #[test]
    fn shell_escape_handles_single_quotes() {
        assert_eq!(shell_escape("it's a file"), "'it'\\''s a file'");
    }

    #[test]
    fn shell_escape_handles_unicode() {
        assert_eq!(
            shell_escape("données/résultats.csv"),
            "'données/résultats.csv'"
        );
    }

    #[test]
    fn temp_path_with_spaces() {
        let p = std::path::Path::new("/tmp/my project/output file.csv");
        let t = temp_path(p);
        assert_eq!(t, PathBuf::from("/tmp/my project/output file.csv.oxytmp"));
    }

    #[test]
    fn temp_path_with_unicode() {
        let p = std::path::Path::new("/tmp/données/résultats.csv");
        let t = temp_path(p);
        assert_eq!(t, PathBuf::from("/tmp/données/résultats.csv.oxytmp"));
    }

    #[tokio::test]
    async fn finalize_success_with_spaces_in_path() {
        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("my project");
        std::fs::create_dir(&spaced).unwrap();
        let out = spaced.join("result file.csv");
        std::fs::write(&out, "data").unwrap();

        let ws = make_atomic_workspace(dir.path().to_path_buf(), vec![out.clone()]);
        let exec = LocalExecutor::new();
        exec.finalize_workspace(ws, &make_result(0)).await.unwrap();

        assert!(out.exists());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "data");
    }

    #[tokio::test]
    async fn finalize_success_with_unicode_in_path() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("données");
        std::fs::create_dir(&subdir).unwrap();
        let out = subdir.join("résultats.csv");
        std::fs::write(&out, "données").unwrap();

        let ws = make_atomic_workspace(dir.path().to_path_buf(), vec![out.clone()]);
        let exec = LocalExecutor::new();
        exec.finalize_workspace(ws, &make_result(0)).await.unwrap();

        assert!(out.exists());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "données");
    }

    #[tokio::test]
    async fn finalize_failure_cleans_up_unicode_paths() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("日本語");
        std::fs::create_dir(&subdir).unwrap();
        let out = subdir.join("出力.csv");
        std::fs::write(&out, "data").unwrap();

        let ws = make_atomic_workspace(dir.path().to_path_buf(), vec![out.clone()]);
        let exec = LocalExecutor::new();
        exec.finalize_workspace(ws, &make_result(1)).await.unwrap();

        assert!(!out.exists(), "output should be cleaned up on failure");
    }

    #[tokio::test]
    async fn execute_resolves_in_memory_inputs_to_temp_files() {
        use ox_core::model::*;
        use std::collections::BTreeMap;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        let out_path = dir.path().join("output.txt");

        // Create a shell job that copies input to output.
        let job = ConcreteJob {
            id: JobId::from("mem-test"),
            rule: RuleName::from("copy"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("input.dat")),
                name: None,
                format: None,
            }],
            outputs: vec![ResolvedOutput {
                reference: OutputRef::File(out_path.clone()),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
                format: None,
            }],
            execution: ExecutionBlock::Shell {
                command: format!("cat .oxymake_mem_input_dat.dat > {}", out_path.display()),
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

        // Build context with in-memory data for the input.
        let mut input_data = std::collections::HashMap::new();
        let data: Arc<[u8]> = Arc::from(b"in-memory content" as &[u8]);
        input_data.insert("input.dat".to_string(), data);

        let ctx = ox_core::traits::executor::ExecContext {
            global_job_limit: 1,
            run_id: "test-mem".into(),
            log_dir,
            project_dir: dir.path().to_path_buf(),
            trusted_dirs: vec![dir.path().to_path_buf()],
            input_data,
            memory_map: None,
        };

        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(dir.path());

        let ws = exec.prepare_workspace(&job, &ctx).await.unwrap();
        let result = exec.execute(&job, &ws, &ctx).await.unwrap();

        let _ = std::env::set_current_dir(&prev_dir);

        assert_eq!(result.exit_code, 0, "job should succeed");
        // The temp file should have been written, and the shell command
        // should have read it to produce the output.
        let output_content = std::fs::read_to_string(&out_path).unwrap();
        assert_eq!(output_content, "in-memory content");
    }

    #[tokio::test]
    async fn prepare_workspace_materializes_input_data_to_disk() {
        // Verify that prepare_workspace writes input_data to the expected
        // file path so shell jobs can read it. This is the primary Stage 2
        // optimization: data flows memory → disk (at the expected path)
        // instead of waiting for the async disk writer.
        use ox_core::model::*;
        use std::collections::BTreeMap;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        let input_path = dir.path().join("data").join("input.csv");
        let out_path = dir.path().join("output.txt");

        let job = ConcreteJob {
            id: JobId::from("prep-test"),
            rule: RuleName::from("proc"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![ResolvedInput {
                reference: OutputRef::File(input_path.clone()),
                name: None,
                format: None,
            }],
            outputs: vec![ResolvedOutput {
                reference: OutputRef::File(out_path.clone()),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
                format: None,
            }],
            execution: ExecutionBlock::Shell {
                command: "true".into(),
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

        // Build context with in-memory data.
        let mut input_data = std::collections::HashMap::new();
        let csv_data: Arc<[u8]> = Arc::from(b"col1,col2\na,b\n" as &[u8]);
        input_data.insert(input_path.display().to_string(), csv_data.clone());

        let ctx = ox_core::traits::executor::ExecContext {
            global_job_limit: 1,
            run_id: "test-prep".into(),
            log_dir,
            project_dir: dir.path().to_path_buf(),
            trusted_dirs: vec![dir.path().to_path_buf()],
            input_data,
            memory_map: None,
        };

        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(dir.path());

        let _ws = LocalExecutor::new()
            .prepare_workspace(&job, &ctx)
            .await
            .unwrap();

        let _ = std::env::set_current_dir(&prev_dir);

        // The input file should now exist on disk, written from memory.
        assert!(input_path.exists(), "input should be materialized to disk");
        let content = std::fs::read(&input_path).unwrap();
        assert_eq!(content, b"col1,col2\na,b\n");
    }

    #[tokio::test]
    async fn prepare_workspace_prefers_input_data_over_memory_map() {
        // When both input_data and memory_map have data for the same key,
        // input_data should take priority (it comes from the scheduler's
        // memory_store, which is the authoritative source).
        use ox_core::model::*;
        use std::collections::BTreeMap;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        let input_path = dir.path().join("input.bin");
        let out_path = dir.path().join("output.bin");

        let job = ConcreteJob {
            id: JobId::from("prio-test"),
            rule: RuleName::from("rule"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![ResolvedInput {
                reference: OutputRef::File(input_path.clone()),
                name: None,
                format: None,
            }],
            outputs: vec![ResolvedOutput {
                reference: OutputRef::File(out_path.clone()),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
                format: None,
            }],
            execution: ExecutionBlock::Shell {
                command: "true".into(),
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

        // input_data has "FRESH" data.
        let mut input_data = std::collections::HashMap::new();
        let fresh: Arc<[u8]> = Arc::from(b"FRESH" as &[u8]);
        input_data.insert(input_path.display().to_string(), fresh);

        // memory_map has "STALE" data for the same key.
        let mem_map = ox_core::memory_map::OutputMemoryMap::new();
        mem_map.put(
            input_path.display().to_string(),
            Arc::from(b"STALE".to_vec()),
        );

        let ctx = ox_core::traits::executor::ExecContext {
            global_job_limit: 1,
            run_id: "test-prio".into(),
            log_dir,
            project_dir: dir.path().to_path_buf(),
            trusted_dirs: vec![dir.path().to_path_buf()],
            input_data,
            memory_map: Some(mem_map),
        };

        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(dir.path());

        let _ws = LocalExecutor::new()
            .prepare_workspace(&job, &ctx)
            .await
            .unwrap();

        let _ = std::env::set_current_dir(&prev_dir);

        let content = std::fs::read(&input_path).unwrap();
        assert_eq!(
            content, b"FRESH",
            "input_data should take priority over memory_map"
        );
    }
}
