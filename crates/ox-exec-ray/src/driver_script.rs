//! DAG driver script generation for Ray.
//!
//! Instead of submitting N individual Ray jobs and polling for upstream
//! completion sequentially, this module generates a single Python driver
//! script that encodes the entire DAG using `@ray.remote` functions and
//! ObjectRef dependency chaining. The driver is submitted as ONE Ray job;
//! Ray schedules all tasks optimally via its internal scheduler.
//!
//! Benefits:
//! - One Ray job submission instead of N
//! - Ray handles all scheduling, retries, and placement
//! - `ray.cancel()` on the driver cascades to downstream tasks
//! - The Ray dashboard shows the full task graph
//! - `submit_dag()` returns immediately (fire-and-forget)

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use ox_core::job_graph::JobGraph;
use ox_core::model::{ConcreteJob, ExecutionBlock, JobId};
use ox_core::traits::executor::ExecContext;

use crate::call_mode;
use crate::error::RayError;

/// Generate a Python driver script that encodes the uncached DAG for Ray.
///
/// Jobs whose IDs appear in `skip_jobs` are omitted from the generated
/// script — they are cached and don't need re-execution.
///
/// The generated script:
/// 1. Connects to the Ray cluster via `ray.init()`
/// 2. Defines a `@ray.remote` function that runs shell commands
/// 3. Submits only uncached jobs as Ray tasks with ObjectRef dependencies
/// 4. Waits for all submitted tasks to complete
/// 5. Reports results
pub fn generate_driver(
    graph: &JobGraph,
    ctx: &ExecContext,
    staging_dir: &std::path::Path,
    skip_jobs: &HashSet<JobId>,
) -> Result<String, RayError> {
    let topo_order = graph
        .topological_order()
        .map_err(|e| RayError::CallModeError(format!("topological sort failed: {e}")))?;

    // Filter out cached jobs.
    let active_order: Vec<&JobId> = topo_order
        .into_iter()
        .filter(|id| !skip_jobs.contains(id))
        .collect();

    if active_order.is_empty() {
        return Ok(empty_driver());
    }

    let mut script = String::with_capacity(8192);

    write_header(&mut script);
    write_remote_functions(&mut script, staging_dir);
    write_dag_tasks(&mut script, graph, &active_order, ctx, staging_dir)?;
    write_wait_and_report(&mut script, graph, &active_order, staging_dir);

    Ok(script)
}

/// Generate an empty driver that immediately exits (for empty DAGs).
fn empty_driver() -> String {
    "#!/usr/bin/env python3\nimport sys\nprint('OxyMake: empty DAG, nothing to do')\nsys.exit(0)\n"
        .to_string()
}

/// Write the script header with imports.
fn write_header(script: &mut String) {
    writeln!(script, "#!/usr/bin/env python3").unwrap();
    writeln!(
        script,
        "\"\"\"Auto-generated OxyMake DAG driver for Ray.\n\
         \n\
         This script encodes the full job DAG using @ray.remote tasks with\n\
         ObjectRef dependency chaining. Ray handles scheduling, parallelism,\n\
         and failure propagation natively.\"\"\""
    )
    .unwrap();
    writeln!(script, "import json").unwrap();
    writeln!(script, "import os").unwrap();
    writeln!(script, "import socket").unwrap();
    writeln!(script, "import subprocess").unwrap();
    writeln!(script, "import sys").unwrap();
    writeln!(script, "import tempfile").unwrap();
    writeln!(script, "import time").unwrap();
    writeln!(script).unwrap();
    writeln!(script, "import ray").unwrap();
    writeln!(script).unwrap();
    writeln!(script, "ray.init()").unwrap();
    writeln!(script).unwrap();
}

/// Python helpers shared by the task functions: run the task's single child,
/// read its `os.wait4` usage, and publish the per-task record (see
/// [`crate::task_record`]) by writing a temporary file and renaming it.
/// Expects `TASKS_DIR` to be defined before it.
const MEASURED_TASK_HELPERS: &str = r#"
def _record_path(job_id):
    name = job_id.replace('%', '%25').replace('/', '%2F') + '.json'
    return os.path.join(TASKS_DIR, name)


def _write_record(record):
    """Publish a task record atomically. A record never fails its task."""
    try:
        os.makedirs(TASKS_DIR, exist_ok=True)
        path = _record_path(record['job_id'])
        temporary = f'{path}.tmp-{os.getpid()}'
        with open(temporary, 'w') as f:
            json.dump(record, f)
        os.replace(temporary, path)
    except Exception as e:
        print(f"OxyMake: could not write the record of {record['job_id']}: {e}", file=sys.stderr)


def _node_id():
    try:
        return ray.get_runtime_context().get_node_id()
    except Exception:
        return None


def _remove_output(path):
    try:
        os.remove(path)
    except OSError:
        pass  # Match local best-effort remove_file cleanup.


def _cleanup_outputs(outputs, clean_outputs):
    for path, temporary in outputs:
        if clean_outputs != 'never':
            _remove_output(path)
        _remove_output(temporary)


def _finalize_outputs(outputs, clean_outputs):
    for path, _ in outputs:
        if not os.path.exists(path):
            _cleanup_outputs(outputs, clean_outputs)
            raise RuntimeError(f'OutputMissing: {path}')
    if clean_outputs == 'never':
        for _, temporary in outputs:
            _remove_output(temporary)
        return
    staged = []
    for path, temporary in outputs:
        try:
            os.replace(path, temporary)
        except OSError as e:
            for original, scratch in staged:
                try:
                    os.replace(scratch, original)
                except OSError:
                    pass
            raise RuntimeError(f'AtomicWriteFailed: {path}: stage rename failed: {e}') from e
        staged.append((path, temporary))
    for path, temporary in outputs:
        try:
            os.replace(temporary, path)
        except OSError as e:
            _cleanup_outputs(outputs, clean_outputs)
            raise RuntimeError(f'AtomicWriteFailed: {path}: commit rename failed: {e}') from e


def _run_measured(job_id, args, shell, work_dir, env_vars, outputs, clean_outputs):
    """Run one command and record its usage, wherever and however it ends.

    The child is reaped with os.wait4 rather than read from RUSAGE_CHILDREN:
    Ray reuses worker processes, and that counter accumulates across tasks.
    """
    outputs = [(os.path.join(work_dir, path), os.path.join(work_dir, temporary))
               for path, temporary in outputs]
    start = time.time()
    record = {'job_id': job_id, 'started_at': start, 'node_id': _node_id(),
              'hostname': socket.gethostname()}
    try:
        with tempfile.TemporaryFile() as err:
            child = subprocess.Popen(args, shell=shell, cwd=work_dir, env={**os.environ, **env_vars},
                                     stdout=subprocess.DEVNULL, stderr=err)
            try:
                if hasattr(os, 'wait4'):
                    _, status, usage = os.wait4(child.pid, 0)
                    child.returncode = os.WEXITSTATUS(status) if os.WIFEXITED(status) else -os.WTERMSIG(status)
                else:  # No per-child usage on this platform (Windows).
                    child.wait()
                    usage = None
            except BaseException:
                child.kill()
                child.wait()
                raise
            err.seek(0, os.SEEK_END)
            err.seek(max(0, err.tell() - 8192))
            stderr = err.read().decode(errors='replace')
    except Exception as e:
        _cleanup_outputs(outputs, clean_outputs)
        end = time.time()
        record.update(status='failed', exit_code=None, ended_at=end,
                      wall_time_ms=int((end - start) * 1000), cpu_time_ms=None,
                      peak_rss_bytes=None, error=str(e)[-500:])
        _write_record(record)
        raise
    end = time.time()
    elapsed = end - start
    record.update(
        status='completed' if child.returncode == 0 else 'failed',
        exit_code=child.returncode,
        ended_at=end,
        wall_time_ms=int(elapsed * 1000),
        cpu_time_ms=None if usage is None else int(round((usage.ru_utime + usage.ru_stime) * 1000)),
        # ru_maxrss is in bytes on macOS and in KiB on Linux.
        peak_rss_bytes=None if usage is None else usage.ru_maxrss * (1 if sys.platform == 'darwin' else 1024),
    )
    if child.returncode != 0:
        _cleanup_outputs(outputs, clean_outputs)
        msg = stderr[-2000:] if stderr else '(no stderr)'
        record['error'] = msg[-500:]
        _write_record(record)
        raise RuntimeError(f'Job {job_id} failed (exit {child.returncode}, {elapsed:.1f}s): {msg}')
    try:
        _finalize_outputs(outputs, clean_outputs)
    except Exception as e:
        record.update(status='failed', error=str(e)[-500:])
        _write_record(record)
        raise
    _write_record(record)
    print(f'Job {job_id} succeeded ({elapsed:.1f}s)')
    return child.returncode

"#;

/// Write the `@ray.remote` task functions.
fn write_remote_functions(script: &mut String, staging_dir: &std::path::Path) {
    let tasks_dir = staging_dir.join(crate::task_record::TASKS_DIR);
    writeln!(
        script,
        "TASKS_DIR = \"{}\"",
        python_string_escape(&tasks_dir.to_string_lossy())
    )
    .unwrap();
    script.push_str(MEASURED_TASK_HELPERS);
    writeln!(script).unwrap();

    // Generic shell task: runs a command, accepts upstream ObjectRefs as
    // implicit dependencies (Ray waits for them before scheduling this task).
    writeln!(script, "@ray.remote").unwrap();
    writeln!(
        script,
        "def run_shell(job_id, command, work_dir, env_vars, outputs, clean_outputs, *deps):"
    )
    .unwrap();
    writeln!(
        script,
        "    \"\"\"Run a shell command. *deps are ObjectRefs — Ray waits for them.\"\"\""
    )
    .unwrap();
    writeln!(
        script,
        "    return _run_measured(job_id, command, True, work_dir, env_vars, outputs, clean_outputs)"
    )
    .unwrap();
    writeln!(script).unwrap();

    // Script task: runs an interpreter with a script file.
    writeln!(script, "@ray.remote").unwrap();
    writeln!(
        script,
        "def run_script(job_id, interpreter, script_path, work_dir, env_vars, outputs, clean_outputs, *deps):"
    )
    .unwrap();
    writeln!(
        script,
        "    \"\"\"Run a script file. *deps are ObjectRefs — Ray waits for them.\"\"\""
    )
    .unwrap();
    writeln!(
        script,
        "    return _run_measured(job_id, [interpreter, script_path], False, work_dir, env_vars, outputs, clean_outputs)"
    )
    .unwrap();
    writeln!(script).unwrap();
}

/// Write the DAG task submissions with ObjectRef dependency chaining.
fn write_dag_tasks(
    script: &mut String,
    graph: &JobGraph,
    topo_order: &[&JobId],
    ctx: &ExecContext,
    staging_dir: &std::path::Path,
) -> Result<(), RayError> {
    writeln!(script, "# --- DAG task submissions ---").unwrap();
    writeln!(
        script,
        "print(f'OxyMake: submitting DAG with {} tasks to Ray')",
        topo_order.len()
    )
    .unwrap();
    writeln!(script).unwrap();

    // Map job IDs to Python variable names for ObjectRef references.
    let var_names: HashMap<&str, String> = topo_order
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), format!("ref_{i}")))
        .collect();

    for job_id in topo_order {
        let job = match graph.get_job(job_id) {
            Some(j) => j,
            None => continue,
        };

        let var_name = &var_names[job_id.as_str()];

        // Build the dependency argument list (upstream ObjectRefs).
        let upstream_ids = graph.upstream(job_id);
        let deps_args: Vec<String> = upstream_ids
            .iter()
            .filter_map(|uid| var_names.get(uid.as_str()).cloned())
            .collect();

        let deps_str = if deps_args.is_empty() {
            String::new()
        } else {
            format!(", {}", deps_args.join(", "))
        };

        // Emit the task submission based on execution block type.
        write_task_submission(script, job_id, job, var_name, &deps_str, ctx, staging_dir)?;
    }

    Ok(())
}

/// Write a single task submission line.
fn write_task_submission(
    script: &mut String,
    job_id: &JobId,
    job: &ConcreteJob,
    var_name: &str,
    deps_str: &str,
    ctx: &ExecContext,
    staging_dir: &std::path::Path,
) -> Result<(), RayError> {
    let project_dir = ctx.project_dir.display();
    let job_id_str = job_id.as_str();
    let escaped_id = python_string_escape(job_id_str);

    // Build resource options string for .options() if resources are specified.
    let resources = crate::resource_mapper::map_resources(&job.resources)?;
    let mut options_parts: Vec<String> = Vec::new();
    if let Some(cpus) = resources.num_cpus {
        options_parts.push(format!("num_cpus={cpus}"));
    }
    if let Some(gpus) = resources.num_gpus {
        options_parts.push(format!("num_gpus={gpus}"));
    }
    if let Some(memory_bytes) = resources.memory_bytes {
        options_parts.push(format!("memory={memory_bytes}"));
    }

    if !resources.custom.is_empty() {
        let data = serde_json::to_string(&resources.custom).expect("validated finite resources");
        options_parts.push(format!(
            "resources=json.loads(\"{}\")",
            python_string_escape(&data)
        ));
    }

    let options_suffix = if options_parts.is_empty() {
        String::new()
    } else {
        format!(".options({})", options_parts.join(", "))
    };
    let env_json = serde_json::to_string(&job.env).expect("environment variables serialize");
    let env_arg = format!("json.loads(\"{}\")", python_string_escape(&env_json));

    // Resolve staging pairs with the same path function as local. Call-mode
    // files participate only when its wrapper actually materializes them.
    let outputs: Vec<_> = job
        .outputs
        .iter()
        .filter_map(|output| {
            if matches!(job.execution, ExecutionBlock::Call { .. })
                && !crate::object_store::strategy_for_policy(output.materialize, false)
                    .write_to_shared_fs
            {
                return None;
            }
            match &output.reference {
                ox_core::model::OutputRef::File(path) => {
                    Some((path.clone(), ox_core::atomic_output::temp_path(path)))
                }
                _ => None,
            }
        })
        .collect();
    let output_json = serde_json::to_string(&outputs).expect("output paths serialize");
    let output_args = format!(
        ", json.loads(\"{}\"), \"{}\"",
        python_string_escape(&output_json),
        job.clean_outputs
    );

    match &job.execution {
        ExecutionBlock::Shell { command } => {
            let escaped_cmd = python_string_escape(command);
            writeln!(
                script,
                "{var_name} = run_shell{options_suffix}.remote(\"{escaped_id}\", \"{escaped_cmd}\", \"{project_dir}\", {env_arg}{output_args}{deps_str})"
            )
            .unwrap();
        }
        ExecutionBlock::Script { path, lang } => {
            let shell = job.shell_executable.as_deref().unwrap_or("/bin/bash");
            let interpreter = lang.as_deref().map(lang_to_interpreter).unwrap_or(shell);
            let script_path = path.display();
            writeln!(
                script,
                "{var_name} = run_script{options_suffix}.remote(\"{escaped_id}\", \"{interpreter}\", \"{script_path}\", \"{project_dir}\", {env_arg}{output_args}{deps_str})"
            )
            .unwrap();
        }
        ExecutionBlock::Run { code, lang } => {
            // Write inline code to a staging file and run it.
            let interpreter = lang_to_interpreter(lang.as_str());
            let script_file = staging_dir.join(format!("{job_id_str}.script"));
            let script_path_display = script_file.display();

            // We'll write the inline code to disk at generation time (Rust side).
            // The driver just references the file.
            writeln!(
                script,
                "{var_name} = run_script{options_suffix}.remote(\"{escaped_id}\", \"{interpreter}\", \"{script_path_display}\", \"{project_dir}\", {env_arg}{output_args}{deps_str})"
            )
            .unwrap();

            // Store the inline code so the caller can write it to disk.
            // We write it directly here since we have the staging_dir.
            std::fs::create_dir_all(staging_dir).map_err(RayError::Io)?;
            let script_content = format!("#!/usr/bin/env {interpreter}\n{code}\n");
            std::fs::write(&script_file, script_content).map_err(RayError::Io)?;
        }
        ExecutionBlock::Call { .. } => {
            // For call-mode, generate the wrapper script and run it.
            let wrapper = call_mode::generate_wrapper(job)?;
            let wrapper_file = staging_dir.join(format!("{job_id_str}.call_wrapper.py"));
            std::fs::create_dir_all(staging_dir).map_err(RayError::Io)?;
            std::fs::write(&wrapper_file, wrapper).map_err(RayError::Io)?;

            let wrapper_cmd = call_mode::wrapper_command(&wrapper_file);
            let escaped_cmd = python_string_escape(&wrapper_cmd);
            writeln!(
                script,
                "{var_name} = run_shell{options_suffix}.remote(\"{escaped_id}\", \"{escaped_cmd}\", \"{project_dir}\", {env_arg}{output_args}{deps_str})"
            )
            .unwrap();
        }
    }

    Ok(())
}

/// Write the final wait-and-report block.
///
/// The driver waits for each task individually and writes a JSON results
/// file so that `ox status` can sync results back to state.db without
/// requiring the Ray Jobs API to expose per-task status.
fn write_wait_and_report(
    script: &mut String,
    _graph: &JobGraph,
    topo_order: &[&JobId],
    staging_dir: &std::path::Path,
) {
    writeln!(script).unwrap();
    writeln!(script, "import json").unwrap();
    writeln!(script).unwrap();
    writeln!(script, "# --- Collect results per task ---").unwrap();

    let total = topo_order.len();

    // Build a list of (var_name, job_id) for all tasks.
    writeln!(script, "task_refs = [").unwrap();
    for (i, job_id) in topo_order.iter().enumerate() {
        let escaped_id = python_string_escape(job_id.as_str());
        writeln!(script, "    (ref_{i}, \"{escaped_id}\"),").unwrap();
    }
    writeln!(script, "]").unwrap();
    writeln!(script).unwrap();

    // Wait for each task and record results.
    writeln!(script, "results = {{}}").unwrap();
    writeln!(script, "failed_count = 0").unwrap();
    writeln!(script, "for ref_obj, job_id in task_refs:").unwrap();
    writeln!(script, "    try:").unwrap();
    writeln!(script, "        ray.get(ref_obj)").unwrap();
    writeln!(
        script,
        "        results[job_id] = {{\"status\": \"completed\", \"exit_code\": 0}}"
    )
    .unwrap();
    writeln!(script, "    except Exception as e:").unwrap();
    writeln!(
        script,
        "        print(f'Job {{job_id}} failed: {{e}}', file=sys.stderr)"
    )
    .unwrap();
    writeln!(
        script,
        "        results[job_id] = {{\"status\": \"failed\", \"exit_code\": 1, \"error\": str(e)[:500]}}"
    )
    .unwrap();
    writeln!(script, "        failed_count += 1").unwrap();
    writeln!(script).unwrap();

    // Write results file for ox status to read.
    let results_path = staging_dir.join("results.json");
    let results_path_display = results_path.display();
    writeln!(script, "with open(\"{results_path_display}\", \"w\") as f:").unwrap();
    writeln!(script, "    json.dump(results, f)").unwrap();
    writeln!(script).unwrap();

    writeln!(script, "succeeded = {total} - failed_count").unwrap();
    writeln!(
        script,
        "print(f'OxyMake: {{succeeded}}/{total} tasks succeeded, {{failed_count}} failed')"
    )
    .unwrap();
    writeln!(script, "if failed_count > 0:").unwrap();
    writeln!(script, "    sys.exit(1)").unwrap();
}

/// Convert a language name to an interpreter command.
fn lang_to_interpreter(lang: &str) -> &str {
    match lang {
        "python" | "py" => "python3",
        "r" | "R" => "Rscript",
        "julia" | "jl" => "julia",
        _ => "/bin/bash",
    }
}

/// Escape a string for embedding in a Python double-quoted string.
fn python_string_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::job_graph::JobGraph;
    use ox_core::model::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn shell_job(
        id: &str,
        command: &str,
        inputs: Vec<ResolvedInput>,
        outputs: Vec<ResolvedOutput>,
    ) -> ConcreteJob {
        ConcreteJob {
            id: JobId::from(id),
            rule: RuleName::from("test-rule"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs,
            outputs,
            execution: ExecutionBlock::Shell {
                command: command.to_string(),
            },
            resources: BTreeMap::new(),
            env: Default::default(),
            environment: None,
            error_strategy: ErrorStrategy::default(),
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            clean_outputs: Default::default(),
            platform_scope: Default::default(),
            reproducibility: ReproducibilityClass::default(),
        }
    }

    fn ctx() -> ExecContext {
        ExecContext {
            global_job_limit: 8,
            run_id: "test-run".into(),
            log_dir: PathBuf::from("/tmp/logs"),
            project_dir: PathBuf::from("/tmp/project"),
            trusted_dirs: vec![],
            input_data: std::collections::HashMap::new(),
            memory_map: None,
        }
    }

    fn no_skip() -> HashSet<JobId> {
        HashSet::new()
    }

    #[test]
    fn dag_tasks_receive_declared_environment() {
        let mut job = shell_job("a", "true", vec![], vec![]);
        job.env
            .insert("DECLARED_VALUE".into(), "quotes \" and $()".into());
        let graph = JobGraph::build(vec![job]).unwrap();
        let staging = tempfile::tempdir().unwrap();
        let script = generate_driver(&graph, &ctx(), staging.path(), &no_skip()).unwrap();

        assert!(script.contains("env={**os.environ, **env_vars}"));
        assert!(script.contains("DECLARED_VALUE"));
        assert!(script.contains("quotes"));
        assert!(script.contains("$()"));
        let parsed = std::process::Command::new("python3")
            .args(["-c", "import ast,sys; ast.parse(sys.stdin.read())"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.as_mut().unwrap().write_all(script.as_bytes())?;
                child.wait()
            })
            .unwrap();
        assert!(parsed.success());
    }

    #[test]
    fn task_resources_round_trip_every_task_kind_and_memory_alias() {
        let kinds = [
            ExecutionBlock::Shell {
                command: "true".into(),
            },
            ExecutionBlock::Script {
                path: "task.py".into(),
                lang: Some("python".into()),
            },
            ExecutionBlock::Run {
                code: "pass".into(),
                lang: "python".into(),
            },
            ExecutionBlock::Call {
                function: "tasks:work".into(),
                lang: "python".into(),
            },
        ];
        let memory_aliases = [
            ("mem", ResourceValue::Int(17), 17_u64),
            ("memory", ResourceValue::Str("2KiB".into()), 2_048),
            ("mem_mb", ResourceValue::Int(3), 3 * 1_048_576),
            (
                "mem_gb",
                ResourceValue::Float(ox_core::OrderedFloat(0.5)),
                536_870_912,
            ),
        ];
        let name = "metal'\"\\é";
        for execution in kinds {
            for (memory_key, memory_value, expected_bytes) in &memory_aliases {
                let mut job = shell_job("a", "true", vec![], vec![]);
                job.execution = execution.clone();
                job.resources.insert("cpu".into(), ResourceValue::Int(2));
                job.resources.insert(name.into(), ResourceValue::Int(1));
                job.resources
                    .insert((*memory_key).into(), memory_value.clone());
                let graph = JobGraph::build(vec![job]).unwrap();
                let staging = tempfile::tempdir().unwrap();
                let script = generate_driver(&graph, &ctx(), staging.path(), &no_skip()).unwrap();
                let source = staging.path().join("driver.py");
                std::fs::write(&source, script).unwrap();
                // Decode actual Python syntax, without importing Ray or executing recipes.
                let output = std::process::Command::new("python3")
                    .args(["-c", r#"import ast, json, sys
root = ast.parse(open(sys.argv[1]).read())
options = [n for n in ast.walk(root) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == 'options']
assert len(options) == 1
values = {k.arg: eval(compile(ast.Expression(k.value), '<options>', 'eval'), {'json': json}) for k in options[0].keywords}
print(json.dumps(values))
"#]).arg(source).output().unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(
                    actual,
                    serde_json::json!({
                        "memory": expected_bytes,
                        "num_cpus": 2,
                        "resources": {name: 1.0}
                    }),
                    "memory alias {memory_key}"
                );
            }
        }
    }

    #[test]
    fn explicit_zero_memory_is_emitted_but_absent_memory_is_not() {
        for (memory, expected) in [(Some(0), true), (None, false)] {
            let mut job = shell_job("a", "true", vec![], vec![]);
            if let Some(memory) = memory {
                job.resources
                    .insert("memory".into(), ResourceValue::Int(memory));
            }
            let graph = JobGraph::build(vec![job]).unwrap();
            let staging = tempfile::tempdir().unwrap();
            let script = generate_driver(&graph, &ctx(), staging.path(), &no_skip()).unwrap();
            assert_eq!(script.contains(".options(memory=0)"), expected);
        }
    }

    #[test]
    fn custom_resource_emission_is_byte_deterministic() {
        let mut job = shell_job("a", "true", vec![], vec![]);
        job.resources = BTreeMap::from([
            ("cpu".into(), ResourceValue::Int(2)),
            ("memory".into(), ResourceValue::Int(4096)),
            ("zeta".into(), ResourceValue::Int(3)),
            ("alpha".into(), ResourceValue::Int(1)),
            ("metal".into(), ResourceValue::Float(0.5.into())),
        ]);
        let graph = JobGraph::build(vec![job]).unwrap();
        let staging = tempfile::tempdir().unwrap();
        let expected = "ref_0 = run_shell.options(num_cpus=2, memory=4096, resources=json.loads(\"{\\\"alpha\\\":1.0,\\\"metal\\\":0.5,\\\"zeta\\\":3.0}\")).remote(\"a\", \"true\", \"/tmp/project\", json.loads(\"{}\"), json.loads(\"[]\"), \"always\")";

        for _ in 0..32 {
            let script = generate_driver(&graph, &ctx(), staging.path(), &no_skip()).unwrap();
            let emitted = script
                .lines()
                .find(|line| line.starts_with("ref_0 ="))
                .unwrap();
            assert_eq!(emitted, expected);
        }
    }

    #[test]
    fn task_failure_reaches_driver_logs_and_results() {
        let graph = JobGraph::build(vec![shell_job("a", "must not run", vec![], vec![])]).unwrap();
        let staging = tempfile::tempdir().unwrap();
        let source = generate_driver(&graph, &ctx(), staging.path(), &no_skip()).unwrap();
        let path = staging.path().join("driver.py");
        std::fs::write(&path, source).unwrap();
        // Exercise failure reporting only; this stub does not model scheduling.
        std::fs::write(
            staging.path().join("ray.py"),
            r#"
def init(): pass
class Task:
    def remote(self, *args): return object()
def remote(function): return Task()
def get(ref): raise RuntimeError('synthetic task failure')
"#,
        )
        .unwrap();
        let output = std::process::Command::new("python3")
            .arg(path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("synthetic task failure"));
        let results: serde_json::Value =
            serde_json::from_slice(&std::fs::read(staging.path().join("results.json")).unwrap())
                .unwrap();
        assert_eq!(results["a"]["status"], "failed");
        assert_eq!(results["a"]["error"], "synthetic task failure");
    }

    /// Synchronous stand-in for Ray: each `.remote()` runs its task at once,
    /// so recipes really execute. `OX_STUB_KILL_AFTER=n` SIGKILLs the driver
    /// before the (n+1)-th task, as `ray job stop` or an OOM kill would.
    const SYNC_RAY_STUB: &str = r#"
import os, signal
_runs = 0
class _Context:
    def get_node_id(self): return 'stub-node'
def get_runtime_context(): return _Context()
def init(): pass
class _Stored:
    def hex(self): return '00'
def put(value): return _Stored()
class _Ref:
    def __init__(self, value=None, error=None): self.value, self.error = value, error
class _Task:
    def __init__(self, fn): self.fn = fn
    def options(self, **_): return self
    def remote(self, *args):
        global _runs
        limit = os.environ.get('OX_STUB_KILL_AFTER')
        if limit is not None and _runs >= int(limit):
            os.kill(os.getpid(), signal.SIGKILL)
        _runs += 1
        try:
            return _Ref(value=self.fn(*args))
        except Exception as e:
            return _Ref(error=e)
def remote(fn): return _Task(fn)
def get(ref):
    if ref.error is not None:
        raise ref.error
    return ref.value
"#;

    /// A recipe that holds `mib` MiB resident and burns `cpu_s` seconds of CPU.
    fn allocate_and_burn(mib: u64, cpu_s: f64) -> String {
        format!(
            "python3 -c \"import time; b = b'x' * ({mib} << 20); t = time.process_time(); \
             [0 for _ in iter(lambda: time.process_time() - t < {cpu_s}, False)]\""
        )
    }

    fn run_with_stub(staging: &std::path::Path, script: String, kill_after: Option<u32>) -> i32 {
        let path = staging.join("driver.py");
        std::fs::write(&path, script).unwrap();
        std::fs::write(staging.join("ray.py"), SYNC_RAY_STUB).unwrap();
        let mut command = std::process::Command::new("python3");
        command.arg(path);
        if let Some(n) = kill_after {
            command.env("OX_STUB_KILL_AFTER", n.to_string());
        }
        let output = command.output().unwrap();
        output.status.code().unwrap_or(-1)
    }

    #[test]
    fn ray_call_finalizes_only_materialized_files() {
        for materialize in [
            MaterializePolicy::Always,
            MaterializePolicy::Auto,
            MaterializePolicy::Never,
            MaterializePolicy::Final,
        ] {
            for fail in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let body = if fail {
                    "def produce():\n    open('out.json', 'w').write('partial')\n    open('out.json.oxytmp', 'w').write('scratch')\n    raise RuntimeError('failed call')\n"
                } else {
                    "def produce():\n    return {'answer': 42}\n"
                };
                std::fs::write(dir.path().join("recipe.py"), body).unwrap();
                let output = ResolvedOutput {
                    reference: OutputRef::File("out.json".into()),
                    name: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize,
                    format: None,
                };
                let mut job = shell_job("produce", "", vec![], vec![output]);
                job.execution = ExecutionBlock::Call {
                    function: "recipe:produce".into(),
                    lang: "python".into(),
                };
                let graph = JobGraph::build(vec![job]).unwrap();
                let ctx = ExecContext {
                    project_dir: dir.path().into(),
                    ..ctx()
                };
                let source = generate_driver(&graph, &ctx, dir.path(), &no_skip()).unwrap();
                assert_eq!(
                    run_with_stub(dir.path(), source, None),
                    if fail { 1 } else { 0 }
                );
                let writes_file = materialize == MaterializePolicy::Always;
                // Files outside the declared materialization contract remain
                // owned by the user function; no object-store behavior changes.
                assert_eq!(
                    dir.path().join("out.json").exists(),
                    if fail { !writes_file } else { writes_file }
                );
                assert_eq!(
                    dir.path().join("out.json.oxytmp").exists(),
                    fail && !writes_file
                );
                if !fail && writes_file {
                    let value: serde_json::Value = serde_json::from_str(
                        &std::fs::read_to_string(dir.path().join("out.json")).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(value["answer"], 42);
                }
            }
        }
    }

    #[test]
    fn ray_output_finalization_rename_failures() {
        for phase in ["stage", "commit"] {
            let dir = tempfile::tempdir().unwrap();
            let outputs = ["a", "b"]
                .map(|name| ResolvedOutput {
                    reference: OutputRef::File(name.into()),
                    name: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                    format: None,
                })
                .to_vec();
            let graph = JobGraph::build(vec![shell_job(
                "produce",
                "echo a > a; echo b > b",
                vec![],
                outputs,
            )])
            .unwrap();
            let ctx = ExecContext {
                project_dir: dir.path().into(),
                ..ctx()
            };
            let source = generate_driver(&graph, &ctx, dir.path(), &no_skip()).unwrap();
            let hook = format!(
                r#"
_real_replace = os.replace
def _fail_replace(source, target):
    if (source.endswith('/b') and target.endswith('/b.oxytmp') and '{phase}' == 'stage') or (source.endswith('/b.oxytmp') and target.endswith('/b') and '{phase}' == 'commit'):
        raise OSError('injected rename failure')
    return _real_replace(source, target)
os.replace = _fail_replace
"#
            );
            let source = source.replace(
                "# --- DAG task submissions ---",
                &format!("{hook}\n# --- DAG task submissions ---"),
            );
            assert_eq!(run_with_stub(dir.path(), source, None), 1);
            let record = crate::task_record::read(
                &dir.path().join(crate::task_record::TASKS_DIR),
                "produce",
            )
            .unwrap();
            assert_eq!(record.status, crate::task_record::TaskStatus::Failed);
            assert!(
                record
                    .error
                    .unwrap()
                    .contains(&format!("{phase} rename failed"))
            );
            for name in ["a", "b"] {
                // A stage failure rolls back previous renames like local;
                // a commit failure removes all outputs and scratch.
                assert_eq!(dir.path().join(name).exists(), phase == "stage");
                assert!(!dir.path().join(format!("{name}.oxytmp")).exists());
            }
        }
    }

    #[test]
    fn ray_output_finalization_contract() {
        for execution_mode in ["shell", "script", "run"] {
            for policy in [
                CleanOutputs::Always,
                CleanOutputs::OnFailure,
                CleanOutputs::Never,
            ] {
                for case in ["failure", "missing", "success"] {
                    let dir = tempfile::tempdir().unwrap();
                    let output = PathBuf::from("résultat 'quoted'.txt");
                    let absolute = dir.path().join("absolute.txt");
                    let outputs = [output.clone(), absolute.clone()]
                        .map(|path| ResolvedOutput {
                            reference: OutputRef::File(path),
                            name: None,
                            lifecycle: OutputLifecycle::Permanent,
                            materialize: MaterializePolicy::Always,
                            format: None,
                        })
                        .to_vec();
                    let command = format!(
                        "echo attempt >> attempts; echo partial > \"{}\"; echo scratch > \"{}.oxytmp\"; {}",
                        output.display(),
                        output.display(),
                        match case {
                            "failure" => "exit 7".to_string(),
                            "missing" => "true".to_string(),
                            _ => format!("echo complete > '{}'", absolute.display()),
                        }
                    );
                    let mut job = shell_job("produce", &command, vec![], outputs);
                    job.clean_outputs = policy;
                    if execution_mode == "script" {
                        let path = dir.path().join("recipe.sh");
                        std::fs::write(&path, &command).unwrap();
                        job.execution = ExecutionBlock::Script {
                            path,
                            lang: Some("bash".into()),
                        };
                    } else if execution_mode == "run" {
                        job.execution = ExecutionBlock::Run {
                            code: command,
                            lang: "bash".into(),
                        };
                    }
                    let graph = JobGraph::build(vec![job]).unwrap();
                    let ctx = ExecContext {
                        project_dir: dir.path().into(),
                        ..ctx()
                    };
                    let source = generate_driver(&graph, &ctx, dir.path(), &no_skip()).unwrap();
                    let hook = r#"
_real_replace = os.replace
def _trace_replace(source, target):
    if source.endswith('.oxytmp') or target.endswith('.oxytmp'):
        with open(os.path.join(os.path.dirname(TASKS_DIR), 'renames'), 'a') as f:
            f.write(json.dumps([source, target]) + '\n')
    return _real_replace(source, target)
os.replace = _trace_replace
"#;
                    let source = source.replace(
                        "# --- DAG task submissions ---",
                        &format!("{hook}\n# --- DAG task submissions ---"),
                    );
                    for attempt in 1..=2 {
                        let code = run_with_stub(dir.path(), source.clone(), None);
                        assert_eq!(
                            code,
                            if case == "success" { 0 } else { 1 },
                            "{case}, {policy}, execution={execution_mode}"
                        );
                        let record = crate::task_record::read(
                            &dir.path().join(crate::task_record::TASKS_DIR),
                            "produce",
                        )
                        .unwrap();
                        assert_eq!(
                            record.status,
                            if case == "success" {
                                crate::task_record::TaskStatus::Completed
                            } else {
                                crate::task_record::TaskStatus::Failed
                            }
                        );
                        if case == "missing" {
                            assert!(record.error.unwrap().contains("OutputMissing"));
                        }
                        assert_eq!(
                            dir.path().join(&output).exists(),
                            case == "success" || policy == CleanOutputs::Never
                        );
                        assert_eq!(absolute.exists(), case == "success");
                        if case == "success" && policy != CleanOutputs::Never {
                            let lines =
                                std::fs::read_to_string(dir.path().join("renames")).unwrap();
                            let pairs: Vec<(PathBuf, PathBuf)> = lines
                                .lines()
                                .map(|line| serde_json::from_str(line).unwrap())
                                .collect();
                            let first = dir.path().join(&output);
                            let paths = [&first, &absolute];
                            let expected: Vec<_> =
                                paths
                                    .iter()
                                    .map(|p| ((*p).clone(), ox_core::atomic_output::temp_path(p)))
                                    .chain(paths.iter().map(|p| {
                                        (ox_core::atomic_output::temp_path(p), (*p).clone())
                                    }))
                                    .collect();
                            assert_eq!(&pairs[(attempt - 1) * 4..attempt * 4], expected.as_slice());
                        }
                        // Scratch belongs to the engine even for script-owned files.
                        assert!(
                            !dir.path()
                                .join(format!("{}.oxytmp", output.display()))
                                .exists()
                        );
                        assert!(!dir.path().join("absolute.txt.oxytmp").exists());
                        assert_eq!(
                            std::fs::read_to_string(dir.path().join("attempts"))
                                .unwrap()
                                .lines()
                                .count(),
                            attempt
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_task_records_its_usage_and_node_when_it_ends() {
        // Job IDs with `/` and `%` exercise the shared file-name rule.
        let jobs = vec![
            shell_job("small", &allocate_and_burn(20, 0.05), vec![], vec![]),
            shell_job("big/x%y", &allocate_and_burn(200, 0.5), vec![], vec![]),
            shell_job("bad", "echo boom >&2; exit 3", vec![], vec![]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let staging = tempfile::tempdir().unwrap();
        let ctx = ExecContext {
            project_dir: staging.path().into(),
            ..ctx()
        };
        let script = generate_driver(&graph, &ctx, staging.path(), &no_skip()).unwrap();
        assert_eq!(run_with_stub(staging.path(), script, None), 1);

        let tasks = staging.path().join(crate::task_record::TASKS_DIR);
        let small = crate::task_record::read(&tasks, "small").expect("small record");
        let big = crate::task_record::read(&tasks, "big/x%y").expect("big record");
        let bad = crate::task_record::read(&tasks, "bad").expect("failed task record");
        const MIB: u64 = 1 << 20;

        for record in [&small, &big] {
            assert_eq!(record.status, crate::task_record::TaskStatus::Completed);
            assert_eq!(record.exit_code, Some(0));
            assert_eq!(record.node_id.as_deref(), Some("stub-node"));
            assert!(!record.hostname.as_deref().unwrap_or("").is_empty());
            assert!(record.started_at <= record.ended_at);
        }
        let (small_rss, big_rss) = (small.peak_rss_bytes.unwrap(), big.peak_rss_bytes.unwrap());
        assert!((20 * MIB..100 * MIB).contains(&small_rss), "{small:?}");
        assert!(big_rss >= 200 * MIB, "{big:?}");
        let (small_cpu, big_cpu) = (small.cpu_time_ms.unwrap(), big.cpu_time_ms.unwrap());
        assert!(
            small_cpu >= 50 && big_cpu >= 500 && big_cpu > small_cpu,
            "{small:?} {big:?}"
        );
        assert!(big.wall_time_ms >= 500, "{big:?}");

        assert_eq!(bad.status, crate::task_record::TaskStatus::Failed);
        assert_eq!(bad.exit_code, Some(3));
        assert!(bad.error.as_deref().unwrap().contains("boom"), "{bad:?}");

        // Records are renamed into place: no temporary survives.
        let names: Vec<String> = std::fs::read_dir(&tasks)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");
        // The end-of-run summary is still written for compatibility.
        assert!(staging.path().join("results.json").is_file());
    }

    #[test]
    fn a_killed_driver_leaves_the_records_of_finished_tasks() {
        let jobs = vec![
            shell_job("first", "true", vec![], vec![]),
            shell_job("second", "true", vec![], vec![]),
        ];
        let graph = JobGraph::build(jobs).unwrap();
        let staging = tempfile::tempdir().unwrap();
        let ctx = ExecContext {
            project_dir: staging.path().into(),
            ..ctx()
        };
        let script = generate_driver(&graph, &ctx, staging.path(), &no_skip()).unwrap();
        assert_eq!(run_with_stub(staging.path(), script, Some(1)), -1);

        let records =
            crate::task_record::read_all(&staging.path().join(crate::task_record::TASKS_DIR));
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].status, crate::task_record::TaskStatus::Completed);
        assert!(!staging.path().join("results.json").exists());
    }

    #[test]
    fn test_empty_dag() {
        let graph = JobGraph::build(vec![]).unwrap();
        let script =
            generate_driver(&graph, &ctx(), &PathBuf::from("/tmp/staging"), &no_skip()).unwrap();
        assert!(script.contains("empty DAG"));
    }

    #[test]
    fn test_single_job() {
        let jobs = vec![shell_job(
            "job-a",
            "echo hello",
            vec![],
            vec![ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("out.txt")),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
                format: None,
            }],
        )];
        let graph = JobGraph::build(jobs).unwrap();
        let staging = tempfile::tempdir().unwrap();
        let script = generate_driver(&graph, &ctx(), staging.path(), &no_skip()).unwrap();

        assert!(script.contains("ray.init()"));
        assert!(script.contains("@ray.remote"));
        assert!(script.contains("run_shell"));
        assert!(script.contains("echo hello"));
        assert!(script.contains("ray.get("));
    }

    #[test]
    fn test_diamond_dag() {
        // A -> B, A -> C, B -> D, C -> D
        let out_a = PathBuf::from("a.txt");
        let out_b = PathBuf::from("b.txt");
        let out_c = PathBuf::from("c.txt");
        let out_d = PathBuf::from("d.txt");

        let jobs = vec![
            shell_job(
                "A",
                "echo A",
                vec![],
                vec![ResolvedOutput {
                    reference: OutputRef::File(out_a.clone()),
                    name: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                    format: None,
                }],
            ),
            shell_job(
                "B",
                "echo B",
                vec![ResolvedInput {
                    reference: OutputRef::File(out_a.clone()),
                    name: None,
                    format: None,
                }],
                vec![ResolvedOutput {
                    reference: OutputRef::File(out_b.clone()),
                    name: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                    format: None,
                }],
            ),
            shell_job(
                "C",
                "echo C",
                vec![ResolvedInput {
                    reference: OutputRef::File(out_a.clone()),
                    name: None,
                    format: None,
                }],
                vec![ResolvedOutput {
                    reference: OutputRef::File(out_c.clone()),
                    name: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                    format: None,
                }],
            ),
            shell_job(
                "D",
                "echo D",
                vec![
                    ResolvedInput {
                        reference: OutputRef::File(out_b),
                        name: None,
                        format: None,
                    },
                    ResolvedInput {
                        reference: OutputRef::File(out_c),
                        name: None,
                        format: None,
                    },
                ],
                vec![ResolvedOutput {
                    reference: OutputRef::File(out_d),
                    name: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                    format: None,
                }],
            ),
        ];

        let graph = JobGraph::build(jobs).unwrap();
        let staging = tempfile::tempdir().unwrap();
        let script = generate_driver(&graph, &ctx(), staging.path(), &no_skip()).unwrap();

        // Should have 4 task submissions.
        let ref_count = script.matches("ref_").count();
        assert!(ref_count >= 4, "expected at least 4 refs, got {ref_count}");

        // D should depend on B and C (passed as ObjectRef args).
        // The leaf task (D) should be in the final ray.get() call.
        assert!(script.contains("ray.get("));
    }

    #[test]
    fn test_skip_cached_jobs() {
        // A -> B -> C: if A and B are cached, only C should appear in the driver.
        let out_a = PathBuf::from("a.txt");
        let out_b = PathBuf::from("b.txt");
        let out_c = PathBuf::from("c.txt");

        let jobs = vec![
            shell_job(
                "A",
                "echo A",
                vec![],
                vec![ResolvedOutput {
                    reference: OutputRef::File(out_a.clone()),
                    name: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                    format: None,
                }],
            ),
            shell_job(
                "B",
                "echo B",
                vec![ResolvedInput {
                    reference: OutputRef::File(out_a),
                    name: None,
                    format: None,
                }],
                vec![ResolvedOutput {
                    reference: OutputRef::File(out_b.clone()),
                    name: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                    format: None,
                }],
            ),
            shell_job(
                "C",
                "echo C",
                vec![ResolvedInput {
                    reference: OutputRef::File(out_b),
                    name: None,
                    format: None,
                }],
                vec![ResolvedOutput {
                    reference: OutputRef::File(out_c),
                    name: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                    format: None,
                }],
            ),
        ];

        let graph = JobGraph::build(jobs).unwrap();
        let staging = tempfile::tempdir().unwrap();

        let mut skip = HashSet::new();
        skip.insert(JobId::from("A"));
        skip.insert(JobId::from("B"));

        let script = generate_driver(&graph, &ctx(), staging.path(), &skip).unwrap();

        // Only C should be submitted — A and B are cached.
        assert!(script.contains("echo C"), "C should be in the driver");
        assert!(!script.contains("echo A"), "A should be skipped (cached)");
        assert!(!script.contains("echo B"), "B should be skipped (cached)");
        // Should report 1 task, not 3.
        assert!(script.contains("submitting DAG with 1 tasks"));
    }

    #[test]
    fn test_all_cached_returns_empty_driver() {
        let jobs = vec![shell_job(
            "only-job",
            "echo hello",
            vec![],
            vec![ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("out.txt")),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
                format: None,
            }],
        )];
        let graph = JobGraph::build(jobs).unwrap();
        let staging = tempfile::tempdir().unwrap();

        let mut skip = HashSet::new();
        skip.insert(JobId::from("only-job"));

        let script = generate_driver(&graph, &ctx(), staging.path(), &skip).unwrap();
        assert!(script.contains("empty DAG"), "all cached = empty driver");
    }

    #[test]
    fn test_python_string_escape() {
        assert_eq!(python_string_escape("hello"), "hello");
        assert_eq!(python_string_escape("it's \"fine\""), "it's \\\"fine\\\"");
        assert_eq!(python_string_escape("a\nb"), "a\\nb");
    }
}
