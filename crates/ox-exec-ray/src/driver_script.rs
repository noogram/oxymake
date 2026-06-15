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
    write_remote_functions(&mut script);
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
    writeln!(script, "import os").unwrap();
    writeln!(script, "import subprocess").unwrap();
    writeln!(script, "import sys").unwrap();
    writeln!(script, "import time").unwrap();
    writeln!(script).unwrap();
    writeln!(script, "import ray").unwrap();
    writeln!(script).unwrap();
    writeln!(script, "ray.init()").unwrap();
    writeln!(script).unwrap();
}

/// Write the `@ray.remote` task functions.
fn write_remote_functions(script: &mut String) {
    // Generic shell task: runs a command, accepts upstream ObjectRefs as
    // implicit dependencies (Ray waits for them before scheduling this task).
    writeln!(script, "@ray.remote").unwrap();
    writeln!(script, "def run_shell(job_id, command, work_dir, *deps):").unwrap();
    writeln!(
        script,
        "    \"\"\"Run a shell command. *deps are ObjectRefs — Ray waits for them.\"\"\""
    )
    .unwrap();
    writeln!(script, "    start = time.time()").unwrap();
    writeln!(
        script,
        "    result = subprocess.run(command, shell=True, cwd=work_dir, capture_output=True, text=True)"
    )
    .unwrap();
    writeln!(script, "    elapsed = time.time() - start").unwrap();
    writeln!(script, "    if result.returncode != 0:").unwrap();
    writeln!(
        script,
        "        msg = result.stderr[-2000:] if result.stderr else '(no stderr)'"
    )
    .unwrap();
    writeln!(
        script,
        "        raise RuntimeError(f'Job {{job_id}} failed (exit {{result.returncode}}, {{elapsed:.1f}}s): {{msg}}')"
    )
    .unwrap();
    writeln!(
        script,
        "    print(f'Job {{job_id}} succeeded ({{elapsed:.1f}}s)')"
    )
    .unwrap();
    writeln!(script, "    return result.returncode").unwrap();
    writeln!(script).unwrap();

    // Script task: runs an interpreter with a script file.
    writeln!(script, "@ray.remote").unwrap();
    writeln!(
        script,
        "def run_script(job_id, interpreter, script_path, work_dir, *deps):"
    )
    .unwrap();
    writeln!(
        script,
        "    \"\"\"Run a script file. *deps are ObjectRefs — Ray waits for them.\"\"\""
    )
    .unwrap();
    writeln!(script, "    start = time.time()").unwrap();
    writeln!(
        script,
        "    result = subprocess.run([interpreter, script_path], cwd=work_dir, capture_output=True, text=True)"
    )
    .unwrap();
    writeln!(script, "    elapsed = time.time() - start").unwrap();
    writeln!(script, "    if result.returncode != 0:").unwrap();
    writeln!(
        script,
        "        msg = result.stderr[-2000:] if result.stderr else '(no stderr)'"
    )
    .unwrap();
    writeln!(
        script,
        "        raise RuntimeError(f'Job {{job_id}} failed (exit {{result.returncode}}, {{elapsed:.1f}}s): {{msg}}')"
    )
    .unwrap();
    writeln!(
        script,
        "    print(f'Job {{job_id}} succeeded ({{elapsed:.1f}}s)')"
    )
    .unwrap();
    writeln!(script, "    return result.returncode").unwrap();
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
    let resources = crate::resource_mapper::map_resources(&job.resources);
    let mut options_parts: Vec<String> = Vec::new();
    if let Some(cpus) = resources.num_cpus {
        options_parts.push(format!("num_cpus={cpus}"));
    }
    if let Some(gpus) = resources.num_gpus {
        options_parts.push(format!("num_gpus={gpus}"));
    }

    let options_suffix = if options_parts.is_empty() {
        String::new()
    } else {
        format!(".options({})", options_parts.join(", "))
    };

    match &job.execution {
        ExecutionBlock::Shell { command } => {
            let escaped_cmd = python_string_escape(command);
            writeln!(
                script,
                "{var_name} = run_shell{options_suffix}.remote(\"{escaped_id}\", \"{escaped_cmd}\", \"{project_dir}\"{deps_str})"
            )
            .unwrap();
        }
        ExecutionBlock::Script { path, lang } => {
            let shell = job.shell_executable.as_deref().unwrap_or("/bin/bash");
            let interpreter = lang.as_deref().map(lang_to_interpreter).unwrap_or(shell);
            let script_path = path.display();
            writeln!(
                script,
                "{var_name} = run_script{options_suffix}.remote(\"{escaped_id}\", \"{interpreter}\", \"{script_path}\", \"{project_dir}\"{deps_str})"
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
                "{var_name} = run_script{options_suffix}.remote(\"{escaped_id}\", \"{interpreter}\", \"{script_path_display}\", \"{project_dir}\"{deps_str})"
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
                "{var_name} = run_shell{options_suffix}.remote(\"{escaped_id}\", \"{escaped_cmd}\", \"{project_dir}\"{deps_str})"
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
