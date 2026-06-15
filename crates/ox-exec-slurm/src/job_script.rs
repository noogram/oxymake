//! Generates sbatch job scripts from OxyMake job definitions.

use std::path::Path;

use ox_core::model::{ConcreteJob, EnvSpec, ExecutionBlock};

use crate::error::SlurmError;
use crate::executor::SlurmConfig;
use crate::job_array::{ArrayTaskParams, JobArraySpec};
use crate::resource_mapper;

/// Generate a complete sbatch job script for the given job.
///
/// The script includes `#SBATCH` directives derived from the job's resources,
/// environment setup based on the `EnvSpec`, and the execution command.
///
/// `project_dir` is the directory where the Oxymakefile lives. The generated
/// script `cd`s to this directory so that relative output paths resolve to the
/// same locations as the local executor, which is required for cache correctness.
pub fn generate(
    job: &ConcreteJob,
    config: &SlurmConfig,
    staging_dir: &Path,
    excluded_nodes: &[String],
    project_dir: &Path,
) -> Result<String, SlurmError> {
    let mut script = String::with_capacity(2048);

    // Shebang
    script.push_str("#!/bin/bash\n");

    // --- SBATCH directives ---
    let job_name = format!("ox_{}_{}", job.rule, job.id);
    // Truncate to 255 bytes (SLURM limit)
    let job_name = truncate_to_char_boundary(&job_name, 255);
    write_directive(&mut script, "--job-name", job_name);

    let out_path = staging_dir.join(format!("{}/slurm-%j.out", job.id));
    let err_path = staging_dir.join(format!("{}/slurm-%j.err", job.id));
    write_directive(&mut script, "--output", &out_path.display().to_string());
    write_directive(&mut script, "--error", &err_path.display().to_string());

    // Partition from config (overridable per-job via resources)
    if let Some(ref partition) = config.partition {
        write_directive(&mut script, "--partition", partition);
    }

    // Account
    if let Some(ref account) = config.account {
        write_directive(&mut script, "--account", account);
    }

    // QOS
    if let Some(ref qos) = config.qos {
        write_directive(&mut script, "--qos", qos);
    }

    // Resource directives from job resources
    let directives = resource_mapper::map_resources(&job.resources, job.timeout)?;
    for d in &directives {
        script.push_str(&format!("{d}\n"));
    }

    // Excluded nodes (failed in this run)
    if !excluded_nodes.is_empty() {
        write_directive(&mut script, "--exclude", &excluded_nodes.join(","));
    }

    // Extra flags from config
    for flag in &config.extra_flags {
        script.push_str(&format!("#SBATCH {flag}\n"));
    }

    script.push('\n');

    // --- Environment setup ---
    script.push_str("# --- Environment setup ---\n");
    script.push_str("set -euo pipefail\n");
    generate_env_setup(&mut script, &job.environment);
    script.push('\n');

    // --- Working directory ---
    // Use the project directory as working dir so that relative output paths
    // resolve to the same locations as the local executor. This is essential
    // for the content-addressable cache: outputs must exist at the paths
    // recorded in the cache manifest for cache hits to work on re-runs.
    script.push_str("# --- Working directory ---\n");
    script.push_str(&format!("cd \"{}\"\n\n", project_dir.display()));

    // --- Create output directories ---
    // The local executor creates parent directories for outputs automatically
    // (see ox-exec-local create_dir_all). SLURM jobs run on compute nodes where
    // these directories may not exist yet, so we create them in the script.
    let output_dirs = collect_output_dirs(job);
    if !output_dirs.is_empty() {
        script.push_str("# --- Create output directories ---\n");
        for dir in &output_dirs {
            script.push_str(&format!("mkdir -p \"{dir}\"\n"));
        }
        script.push('\n');
    }

    // --- Command ---
    script.push_str("# --- Execute command ---\n");
    let command = resolve_command(job)?;
    script.push_str(&command);
    script.push('\n');

    Ok(script)
}

/// Truncate `s` to at most `max` bytes without splitting a UTF-8 character.
/// Rule names are user input and may be non-ASCII; a raw byte slice panics
/// when `max` falls inside a multi-byte character.
fn truncate_to_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Collect unique parent directories of a job's file outputs, excluding the
/// current directory (`.` or empty). Returns sorted, deduplicated paths.
fn collect_output_dirs(job: &ConcreteJob) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut dirs = BTreeSet::new();
    for output in &job.outputs {
        if let ox_core::model::OutputRef::File(path) = &output.reference {
            if let Some(parent) = path.parent() {
                let s = parent.to_string_lossy();
                if !s.is_empty() && s != "." {
                    dirs.insert(s.into_owned());
                }
            }
        }
    }
    dirs.into_iter().collect()
}

/// Write a single `#SBATCH` directive.
fn write_directive(script: &mut String, flag: &str, value: &str) {
    script.push_str(&format!("#SBATCH {flag}={value}\n"));
}

/// Resolve the job's `ExecutionBlock` into a shell command string.
fn resolve_command(job: &ConcreteJob) -> Result<String, SlurmError> {
    match &job.execution {
        ExecutionBlock::Shell { command } => Ok(command.clone()),
        ExecutionBlock::Run { code, lang } => {
            Ok(format!("{lang} -c '{}'", code.replace('\'', "'\\''")))
        }
        ExecutionBlock::Script { path, lang } => {
            let interpreter = lang.as_deref().unwrap_or("sh");
            Ok(format!("{interpreter} {}", path.display()))
        }
        ExecutionBlock::Call { function, lang, .. } => {
            // Call mode is not directly supported on SLURM — it requires
            // same-process execution. Generate a Python wrapper instead.
            Ok(format!(
                "{lang} -c 'from {mod_path} import {func_name}; {func_name}()'",
                mod_path = function
                    .rsplit_once('.')
                    .map(|(m, _)| m)
                    .unwrap_or("__main__"),
                func_name = function
                    .rsplit_once('.')
                    .map(|(_, f)| f)
                    .unwrap_or(function),
            ))
        }
    }
}

/// Generate a job array sbatch script and the associated params file.
///
/// The params file is a JSON-lines file (one JSON object per line) mapping
/// `SLURM_ARRAY_TASK_ID` to the wildcard values and command for each task.
/// The script reads the appropriate line at runtime.
///
/// Returns `(script_content, params_content)`.
pub fn generate_array(
    spec: &JobArraySpec,
    config: &SlurmConfig,
    staging_dir: &Path,
    excluded_nodes: &[String],
    project_dir: &Path,
    max_concurrent: Option<usize>,
) -> Result<(String, String), SlurmError> {
    if spec.is_empty() {
        return Err(SlurmError::SubmitFailed("empty job array".into()));
    }

    // Use the first job as a template for shared properties (resources, env, etc.).
    let template = &spec.jobs[0];

    let mut script = String::with_capacity(4096);

    // Shebang
    script.push_str("#!/bin/bash\n");

    // --- SBATCH directives ---
    let job_name = format!("ox_array_{}", spec.rule);
    let job_name = truncate_to_char_boundary(&job_name, 255);
    write_directive(&mut script, "--job-name", job_name);

    // Array directive
    let array_val = spec.array_flag(max_concurrent);
    write_directive(&mut script, "--array", &array_val);

    // Output/error use %A (array parent ID) and %a (task index)
    let out_path = staging_dir.join("slurm-%A_%a.out");
    let err_path = staging_dir.join("slurm-%A_%a.err");
    write_directive(&mut script, "--output", &out_path.display().to_string());
    write_directive(&mut script, "--error", &err_path.display().to_string());

    // Partition, account, QOS from config
    if let Some(ref partition) = config.partition {
        write_directive(&mut script, "--partition", partition);
    }
    if let Some(ref account) = config.account {
        write_directive(&mut script, "--account", account);
    }
    if let Some(ref qos) = config.qos {
        write_directive(&mut script, "--qos", qos);
    }

    // Resource directives from template job
    let directives = resource_mapper::map_resources(&template.resources, template.timeout)?;
    for d in &directives {
        script.push_str(&format!("{d}\n"));
    }

    // Excluded nodes
    if !excluded_nodes.is_empty() {
        write_directive(&mut script, "--exclude", &excluded_nodes.join(","));
    }

    // Extra flags from config
    for flag in &config.extra_flags {
        script.push_str(&format!("#SBATCH {flag}\n"));
    }

    script.push('\n');

    // --- Environment setup ---
    script.push_str("# --- Environment setup ---\n");
    script.push_str("set -euo pipefail\n");
    generate_env_setup(&mut script, &template.environment);
    script.push('\n');

    // --- Working directory ---
    script.push_str("# --- Working directory ---\n");
    script.push_str(&format!("cd \"{}\"\n\n", project_dir.display()));

    // --- Array dispatch ---
    // The params file lives next to the script. Each line is a JSON object
    // keyed by `index`. We use `sed` to extract the line matching our task ID.
    script.push_str("# --- Array task dispatch ---\n");
    script.push_str("PARAMS_FILE=\"$(dirname \"$0\")/array_params.jsonl\"\n");
    script.push_str("TASK_LINE=$(sed -n \"$((SLURM_ARRAY_TASK_ID + 1))p\" \"$PARAMS_FILE\")\n");
    script.push_str("if [ -z \"$TASK_LINE\" ]; then\n");
    script
        .push_str("  echo \"ERROR: no params for SLURM_ARRAY_TASK_ID=$SLURM_ARRAY_TASK_ID\" >&2\n");
    script.push_str("  exit 1\n");
    script.push_str("fi\n\n");

    // Export wildcards as environment variables (OX_WC_<name>=<value>)
    script.push_str("# Export wildcard values as environment variables\n");
    script.push_str("export OX_JOB_ID=$(echo \"$TASK_LINE\" | ");
    // Use python for reliable JSON parsing (available on virtually all HPC systems)
    script.push_str("python3 -c 'import sys,json; print(json.load(sys.stdin)[\"job_id\"])')\n");

    // Extract and export each wildcard
    if let Some(first_job) = spec.jobs.first() {
        for wc_name in first_job.wildcards.keys() {
            script.push_str(&format!(
                "export OX_WC_{name}=$(echo \"$TASK_LINE\" | python3 -c 'import sys,json; print(json.load(sys.stdin)[\"wildcards\"][\"{name}\"])')\n",
                name = wc_name,
            ));
        }
    }
    script.push('\n');

    // --- Create output directories ---
    // Collect all unique output dirs across all tasks in the array.
    {
        use std::collections::BTreeSet;
        let mut all_dirs = BTreeSet::new();
        for job in &spec.jobs {
            for dir in collect_output_dirs(job) {
                all_dirs.insert(dir);
            }
        }
        if !all_dirs.is_empty() {
            script.push_str("# --- Create output directories ---\n");
            for dir in &all_dirs {
                script.push_str(&format!("mkdir -p \"{dir}\"\n"));
            }
            script.push('\n');
        }
    }

    // Extract and execute the command
    script.push_str("# --- Execute command ---\n");
    script.push_str("TASK_CMD=$(echo \"$TASK_LINE\" | python3 -c 'import sys,json; print(json.load(sys.stdin)[\"command\"])')\n");
    script.push_str("eval \"$TASK_CMD\"\n");

    // --- Generate params file (JSON lines) ---
    let mut params = String::with_capacity(1024);
    for (index, job) in spec.jobs.iter().enumerate() {
        let command = resolve_command(job)?;
        let entry = ArrayTaskParams {
            index,
            job_id: job.id.as_str().to_string(),
            wildcards: job.wildcards.clone(),
            command,
        };
        let line = serde_json::to_string(&entry)
            .map_err(|e| SlurmError::ParseError(format!("failed to serialize params: {e}")))?;
        params.push_str(&line);
        params.push('\n');
    }

    Ok((script, params))
}

/// Generate environment setup lines for the sbatch script.
fn generate_env_setup(script: &mut String, env: &Option<EnvSpec>) {
    match env {
        None | Some(EnvSpec::System) => {
            script.push_str("# No special environment\n");
        }
        Some(EnvSpec::Conda { env: env_val }) => {
            script.push_str("# Conda environment\n");
            script.push_str("module load conda 2>/dev/null || true\n");
            script.push_str("eval \"$(conda shell.bash hook)\"\n");
            script.push_str(&format!("conda activate {env_val}\n"));
        }
        Some(EnvSpec::Docker { image }) => {
            // Docker is typically not available on HPC — warn and use Apptainer.
            script.push_str("# WARNING: Docker not supported on most HPC clusters.\n");
            script.push_str(
                "# Consider using Apptainer (environment = { type = \"apptainer\", ... }).\n",
            );
            script.push_str(&format!(
                "echo 'WARNING: Docker executor on SLURM — using apptainer exec {image} instead' >&2\n"
            ));
            script.push_str(&format!(
                "OXYMAKE_CONTAINER_CMD=\"apptainer exec {image}\"\n"
            ));
        }
        Some(EnvSpec::Apptainer { image }) => {
            script.push_str("# Apptainer container\n");
            script.push_str(&format!(
                "OXYMAKE_CONTAINER_CMD=\"apptainer exec {image}\"\n"
            ));
        }
        Some(EnvSpec::Uv { requirements }) => {
            script.push_str("# uv Python environment\n");
            script.push_str("module load uv 2>/dev/null || true\n");
            if let Some(req) = requirements {
                script.push_str(&format!("uv sync -r {req}\n"));
            }
        }
        Some(EnvSpec::Nix { expr }) => {
            script.push_str("# Nix environment\n");
            script.push_str("# Note: nix develop must be available on compute nodes\n");
            let _ = expr; // Nix wrapping happens at command level
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::model::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::Duration;

    fn test_config() -> SlurmConfig {
        SlurmConfig {
            partition: Some("gpu".into()),
            account: Some("lab-01".into()),
            max_submit: Some(100),
            staging_dir: PathBuf::from("/scratch/staging"),
            poll_interval_min: Duration::from_secs(5),
            poll_interval_max: Duration::from_secs(60),
            extra_flags: vec![],
            qos: None,
            job_array: crate::job_array::JobArrayConfig::default(),
            api_url: None,
            token_cmd: None,
        }
    }

    fn test_job(id: &str, command: &str) -> ConcreteJob {
        ConcreteJob {
            id: JobId::from(id),
            rule: RuleName::from("test_rule"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![],
            outputs: vec![],
            execution: ExecutionBlock::Shell {
                command: command.into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::Terminate,
            timeout: None,
            executor: Some("slurm".into()),
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        }
    }

    fn test_project_dir() -> PathBuf {
        PathBuf::from("/home/user/my-project")
    }

    #[test]
    fn generates_basic_script() {
        let job = test_job("j-001", "echo hello");
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");

        let script = generate(&job, &config, &staging, &[], &test_project_dir()).unwrap();

        assert!(script.starts_with("#!/bin/bash\n"));
        assert!(script.contains("#SBATCH --job-name=ox_test_rule_j-001"));
        assert!(script.contains("#SBATCH --partition=gpu"));
        assert!(script.contains("#SBATCH --account=lab-01"));
        assert!(script.contains("echo hello"));
    }

    #[test]
    fn script_cds_to_project_dir_not_staging() {
        let job = test_job("j-010", "echo hello");
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");
        let project = PathBuf::from("/data/projects/my-pipeline");

        let script = generate(&job, &config, &staging, &[], &project).unwrap();

        // Must cd to project dir for cache correctness.
        assert!(
            script.contains("cd \"/data/projects/my-pipeline\""),
            "script should cd to project dir, got:\n{script}"
        );
        // Must NOT cd to staging dir (the old broken behavior).
        assert!(
            !script.contains("cd \"/scratch/staging/run-001/j-010\""),
            "script should not cd to staging dir"
        );
    }

    #[test]
    fn excluded_nodes_in_script() {
        let job = test_job("j-002", "hostname");
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");

        let script = generate(
            &job,
            &config,
            &staging,
            &["c3".into(), "c7".into()],
            &test_project_dir(),
        )
        .unwrap();
        assert!(script.contains("#SBATCH --exclude=c3,c7"));
    }

    #[test]
    fn script_creates_output_dirs() {
        let mut job = test_job("j-005", "echo hello > data/alpha.txt");
        job.outputs = vec![
            ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("data/alpha.txt")),
                name: None,
                format: None,
                lifecycle: OutputLifecycle::default(),
                materialize: MaterializePolicy::default(),
            },
            ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("results/sub/out.csv")),
                name: None,
                format: None,
                lifecycle: OutputLifecycle::default(),
                materialize: MaterializePolicy::default(),
            },
        ];
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");

        let script = generate(&job, &config, &staging, &[], &test_project_dir()).unwrap();

        assert!(
            script.contains("mkdir -p \"data\""),
            "script should create data dir, got:\n{script}"
        );
        assert!(
            script.contains("mkdir -p \"results/sub\""),
            "script should create results/sub dir, got:\n{script}"
        );
    }

    #[test]
    fn script_no_mkdir_for_root_outputs() {
        let mut job = test_job("j-006", "echo hello > out.txt");
        job.outputs = vec![ResolvedOutput {
            reference: OutputRef::File(PathBuf::from("out.txt")),
            name: None,
            format: None,
            lifecycle: OutputLifecycle::default(),
            materialize: MaterializePolicy::default(),
        }];
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");

        let script = generate(&job, &config, &staging, &[], &test_project_dir()).unwrap();

        assert!(
            !script.contains("mkdir -p"),
            "script should not mkdir for root-level outputs, got:\n{script}"
        );
    }

    #[test]
    fn conda_env_setup() {
        let mut job = test_job("j-003", "python train.py");
        job.environment = Some(EnvSpec::Conda {
            env: "ml-env".into(),
        });
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");

        let script = generate(&job, &config, &staging, &[], &test_project_dir()).unwrap();
        assert!(script.contains("module load conda"));
        assert!(script.contains("conda activate ml-env"));
    }

    #[test]
    fn docker_warns_about_hpc() {
        let mut job = test_job("j-004", "python train.py");
        job.environment = Some(EnvSpec::Docker {
            image: "python:3.12".into(),
        });
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");

        let script = generate(&job, &config, &staging, &[], &test_project_dir()).unwrap();
        assert!(script.contains("WARNING"));
        assert!(script.contains("apptainer"));
    }

    fn test_wildcard_job(id: &str, sample: &str) -> ConcreteJob {
        let mut job = test_job(id, &format!("process --sample={sample}"));
        job.rule = RuleName::from("align");
        job.wildcards.insert("sample".into(), sample.into());
        job
    }

    /// H25: job names longer than 255 bytes must be truncated on a UTF-8
    /// character boundary — a raw byte slice panics when byte 255 falls
    /// inside a multi-byte character (rule names are user input).
    #[test]
    fn long_non_ascii_job_name_does_not_panic() {
        // "a" + 90 × "日" (3 bytes each) → byte 255 of the final job name
        // falls inside a character.
        let mut job = test_job("j-utf8", "echo hello");
        job.rule = RuleName::from(format!("a{}", "日".repeat(90)).as_str());
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");

        let script = generate(&job, &config, &staging, &[], &test_project_dir()).unwrap();

        let name_line = script
            .lines()
            .find(|l| l.starts_with("#SBATCH --job-name="))
            .expect("job-name directive present");
        let name = name_line.trim_start_matches("#SBATCH --job-name=");
        assert!(
            name.len() <= 255,
            "job name must fit SLURM's 255-byte limit"
        );
        assert!(name.starts_with("ox_a日"));
    }

    /// H25 (array variant): same guarantee for `generate_array`.
    #[test]
    fn long_non_ascii_array_job_name_does_not_panic() {
        let mut j1 = test_wildcard_job("j-1", "A");
        let rule = RuleName::from(format!("a{}", "日".repeat(90)).as_str());
        j1.rule = rule.clone();
        let spec = JobArraySpec {
            rule,
            jobs: vec![j1],
        };
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");

        let (script, _) =
            generate_array(&spec, &config, &staging, &[], &test_project_dir(), None).unwrap();
        let name_line = script
            .lines()
            .find(|l| l.starts_with("#SBATCH --job-name="))
            .expect("job-name directive present");
        assert!(name_line.trim_start_matches("#SBATCH --job-name=").len() <= 255);
    }

    #[test]
    fn generates_array_script() {
        let spec = JobArraySpec {
            rule: RuleName::from("align"),
            jobs: vec![
                test_wildcard_job("j-1", "A"),
                test_wildcard_job("j-2", "B"),
                test_wildcard_job("j-3", "C"),
            ],
        };
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001/array_align");

        let (script, params) =
            generate_array(&spec, &config, &staging, &[], &test_project_dir(), None).unwrap();

        // Script basics
        assert!(script.starts_with("#!/bin/bash\n"));
        assert!(script.contains("#SBATCH --job-name=ox_array_align"));
        assert!(script.contains("#SBATCH --array=0-2"));
        assert!(script.contains("#SBATCH --partition=gpu"));
        assert!(script.contains("#SBATCH --account=lab-01"));

        // Array dispatch mechanism
        assert!(script.contains("PARAMS_FILE="));
        assert!(script.contains("SLURM_ARRAY_TASK_ID"));
        assert!(script.contains("array_params.jsonl"));
        assert!(script.contains("eval \"$TASK_CMD\""));

        // Wildcard exports
        assert!(script.contains("OX_WC_sample"));

        // Params file has 3 lines (one per task)
        let lines: Vec<&str> = params.lines().collect();
        assert_eq!(lines.len(), 3);

        // Each line is valid JSON with expected fields
        let entry0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(entry0["index"], 0);
        assert_eq!(entry0["job_id"], "j-1");
        assert_eq!(entry0["wildcards"]["sample"], "A");
        assert!(entry0["command"].as_str().unwrap().contains("--sample=A"));

        let entry2: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(entry2["index"], 2);
        assert_eq!(entry2["job_id"], "j-3");
        assert_eq!(entry2["wildcards"]["sample"], "C");
    }

    #[test]
    fn array_script_with_throttle() {
        let spec = JobArraySpec {
            rule: RuleName::from("count"),
            jobs: vec![
                test_wildcard_job("j-1", "X"),
                test_wildcard_job("j-2", "Y"),
                test_wildcard_job("j-3", "Z"),
            ],
        };
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001/array_count");

        let (script, _) =
            generate_array(&spec, &config, &staging, &[], &test_project_dir(), Some(2)).unwrap();

        assert!(
            script.contains("#SBATCH --array=0-2%2"),
            "expected throttled array flag, got:\n{script}"
        );
    }

    #[test]
    fn array_script_uses_percent_a_for_logs() {
        let spec = JobArraySpec {
            rule: RuleName::from("align"),
            jobs: vec![test_wildcard_job("j-1", "A"), test_wildcard_job("j-2", "B")],
        };
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");

        let (script, _) =
            generate_array(&spec, &config, &staging, &[], &test_project_dir(), None).unwrap();

        // Array jobs should use %A (parent ID) and %a (task index) in log paths
        assert!(
            script.contains("%A_%a"),
            "expected array log pattern %%A_%%a, got:\n{script}"
        );
    }

    #[test]
    fn array_script_cds_to_project_dir() {
        let spec = JobArraySpec {
            rule: RuleName::from("align"),
            jobs: vec![test_wildcard_job("j-1", "A")],
        };
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging/run-001");
        let project = PathBuf::from("/data/projects/pipeline");

        let (script, _) = generate_array(&spec, &config, &staging, &[], &project, None).unwrap();

        assert!(script.contains("cd \"/data/projects/pipeline\""));
    }

    #[test]
    fn array_script_empty_spec_fails() {
        let spec = JobArraySpec {
            rule: RuleName::from("empty"),
            jobs: vec![],
        };
        let config = test_config();
        let staging = PathBuf::from("/scratch/staging");

        let result = generate_array(&spec, &config, &staging, &[], &test_project_dir(), None);
        assert!(result.is_err());
    }
}
