//! TOML parser for Oxymakefile format.
//!
//! Reads an `Oxymakefile.toml` and produces a [`Workflow`] containing
//! parsed rules, gates, configuration, and metadata.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use ox_core::error::ParseError;
use ox_core::model::{
    Backoff, EnvSpec, ErrorStrategy, ExecutionBlock, ExpandMode, GuardExpr, InputPattern,
    LogConfig, MaterializePolicy, OutputLifecycle, OutputPattern, ReproducibilityClass,
    ResourceValue, Rule, RuleMeta, RuleName,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The default `format_version` used when an Oxymakefile omits the field.
///
/// Files written before `format_version` existed are treated as `"1"`.
/// See `STATUS.md` §2 for the migration policy.
pub const DEFAULT_FORMAT_VERSION: &str = "1";

/// A parsed workflow — rules + config + metadata.
#[derive(Debug, Clone)]
pub struct Workflow {
    /// The `ox_version` declared at the top of the file.
    pub ox_version: Option<String>,
    /// The `format_version` declared at the top of the file.
    ///
    /// This versions the **Oxymakefile schema** independently of the
    /// `ox` binary version. Files without `format_version` are read as
    /// [`DEFAULT_FORMAT_VERSION`] (`"1"`). See `STATUS.md` §2 for the
    /// stability contract and migration policy.
    pub format_version: String,
    /// Config section key-value pairs (used for wildcard expansion).
    pub config: BTreeMap<String, ConfigValue>,
    /// Parsed rules.
    pub rules: Vec<Rule>,
    /// Parsed gates (human-in-the-loop checkpoints).
    pub gates: Vec<Gate>,
    /// Include directives (paths to other Oxymakefiles).
    pub includes: Vec<PathBuf>,
    /// Global default environment for all rules.
    pub global_environment: Option<EnvSpec>,
    /// Named profiles — bundles of CLI flag overrides.
    pub profiles: BTreeMap<String, Profile>,
    /// Executor-specific configuration from `[executor.*]` sections.
    pub executor_config: ExecutorConfig,
}

/// Executor-specific configuration sections from the Oxymakefile.
///
/// ```toml
/// [executor.slurm]
/// api_url = "http://localhost:6820"
/// token_cmd = "scontrol token lifespan=3600"
/// partition = "gpu"
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExecutorConfig {
    /// SLURM executor configuration from `[executor.slurm]`.
    pub slurm: Option<SlurmExecutorToml>,
}

/// SLURM executor configuration from `[executor.slurm]`.
///
/// When present, `ox run --executor slurm` reads defaults from here
/// instead of requiring CLI flags or environment variables.
///
/// Mode auto-detection: if `api_url` is set → REST mode, otherwise → CLI mode.
#[derive(Debug, Clone, PartialEq)]
pub struct SlurmExecutorToml {
    /// Explicit mode override: `"rest"` or `"cli"`. Auto-detected if omitted.
    pub mode: Option<String>,
    /// slurmrestd API URL (e.g., `http://localhost:6820`).
    /// Setting this implies REST mode unless `mode = "cli"`.
    pub api_url: Option<String>,
    /// Command to execute to obtain a JWT token before each submission.
    /// The command's stdout (trimmed) is used as the token value.
    /// Example: `"scontrol token lifespan=3600"` or `"gcloud auth print-access-token"`.
    pub token_cmd: Option<String>,
    /// Default SLURM partition.
    pub partition: Option<String>,
    /// Default SLURM account.
    pub account: Option<String>,
    /// Default QoS.
    pub qos: Option<String>,
    /// Base staging directory for job scripts/logs.
    pub staging_dir: Option<String>,
    /// Extra sbatch flags passed through verbatim.
    pub extra_flags: Vec<String>,
}

/// A named profile — a bundle of CLI-equivalent flag overrides.
///
/// Profiles are defined in the Oxymakefile under `[profile.NAME]`:
/// ```toml
/// [profile.ci]
/// cache_validation = "hash"
/// jobs = 4
///
/// [profile.dev]
/// cache_validation = "mtime"
/// jobs = 1
/// verbose = true
/// ```
///
/// Usage: `ox run --profile ci`
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// Maximum concurrent jobs (`-j`).
    pub jobs: Option<usize>,
    /// Cache validation strategy (`--cache-validation`).
    pub cache_validation: Option<String>,
    /// Verbose output level (bool = 1, integer = exact level).
    pub verbose: Option<u8>,
    /// Executor backend (`--executor`).
    pub executor: Option<String>,
    /// Disable the content-addressable cache (`--no-cache`).
    pub no_cache: Option<bool>,
    /// Continue on independent branches after failure (`-k`).
    pub keep_going: Option<bool>,
    /// SLURM partition.
    pub partition: Option<String>,
    /// SLURM account.
    pub account: Option<String>,
    /// SLURM QoS.
    pub qos: Option<String>,
    /// Open dashboard in browser after DAG submission (`--open-dashboard`).
    pub open_dashboard: Option<bool>,
    /// Config overrides (equivalent to `--set KEY=VALUE`).
    pub set: BTreeMap<String, String>,
}

/// A configuration value — either a list of strings, a file source, or a scalar.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    /// A list of string values, e.g. `samples = ["A", "B", "C"]`.
    List(Vec<String>),
    /// A file-based config source, e.g. `{ source = "file.csv", key = "column" }`.
    /// If `columns` is set, multiple columns create multiple config lists.
    FileSource {
        source: PathBuf,
        key: String,
        columns: Vec<String>,
    },
    /// A scalar string value.
    Scalar(String),
}

/// A gate definition — a human-in-the-loop checkpoint in the workflow.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    /// Gate name.
    pub name: String,
    /// Rules that must complete before this gate.
    pub after: Vec<String>,
    /// Rules that are blocked until this gate is approved.
    pub before: Vec<String>,
    /// Human-readable message displayed when the gate is reached.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Serde intermediate types
// ---------------------------------------------------------------------------

/// Top-level TOML structure.
#[derive(Debug, Deserialize)]
struct RawOxymakefile {
    ox_version: Option<String>,
    /// Schema version of the Oxymakefile itself (independent of `ox_version`,
    /// which is the binary version). Optional today; missing values default
    /// to [`DEFAULT_FORMAT_VERSION`].
    format_version: Option<String>,
    #[serde(default)]
    config: BTreeMap<String, toml::Value>,
    #[serde(default)]
    rule: BTreeMap<String, RawRule>,
    #[serde(default)]
    gate: BTreeMap<String, RawGate>,
    #[serde(default)]
    profile: BTreeMap<String, RawProfile>,
    #[serde(default)]
    include: Vec<String>,
    /// Global default environment for all rules.
    #[serde(default)]
    environment: Option<BTreeMap<String, String>>,
    /// Executor-specific configuration sections.
    #[serde(default)]
    executor: Option<RawExecutorTable>,
}

/// Raw `[executor]` table containing per-backend sub-tables.
#[derive(Debug, Deserialize, Default)]
struct RawExecutorTable {
    #[serde(default)]
    slurm: Option<RawSlurmExecutor>,
}

/// Raw `[executor.slurm]` section as it appears in TOML.
#[derive(Debug, Deserialize)]
struct RawSlurmExecutor {
    mode: Option<String>,
    api_url: Option<String>,
    token_cmd: Option<String>,
    partition: Option<String>,
    account: Option<String>,
    qos: Option<String>,
    staging_dir: Option<String>,
    #[serde(default)]
    extra_flags: Vec<String>,
}

/// A raw profile as it appears in TOML.
#[derive(Debug, Deserialize)]
struct RawProfile {
    jobs: Option<usize>,
    cache_validation: Option<String>,
    verbose: Option<toml::Value>,
    executor: Option<String>,
    no_cache: Option<bool>,
    keep_going: Option<bool>,
    partition: Option<String>,
    account: Option<String>,
    qos: Option<String>,
    open_dashboard: Option<bool>,
    #[serde(default)]
    set: BTreeMap<String, String>,
}

/// A raw rule as it appears in TOML, before validation.
#[derive(Debug, Deserialize)]
struct RawRule {
    #[serde(default)]
    input: Option<toml::Value>,
    #[serde(default)]
    output: Option<toml::Value>,

    // Execution modes (at most one should be present)
    shell: Option<String>,
    run: Option<String>,
    script: Option<String>,
    call: Option<String>,
    lang: Option<String>,

    // Optional fields
    #[serde(default)]
    tags: BTreeMap<String, String>,
    #[serde(default)]
    resources: BTreeMap<String, toml::Value>,
    #[serde(default)]
    environment: Option<BTreeMap<String, String>>,
    when: Option<toml::Value>,
    expand: Option<String>,
    error_strategy: Option<toml::Value>,
    timeout: Option<String>,
    executor: Option<String>,
    priority: Option<u32>,
    description: Option<String>,
    benchmark: Option<String>,
    retries: Option<u32>,

    #[serde(default)]
    wildcard_constraints: BTreeMap<String, String>,

    // Params (named parameters)
    #[serde(default)]
    params: BTreeMap<String, toml::Value>,

    // Parameter files tracked as cache inputs
    #[serde(default)]
    param_files: Vec<String>,

    // Log config
    #[serde(default)]
    log: Option<RawLogConfig>,

    // Shell executable override (e.g., "/bin/sh", "/usr/bin/zsh")
    shell_executable: Option<String>,

    // Reproducibility classification for outputs
    reproducibility: Option<String>,

    // 1-based line in the source file (Snakefile, .wdl) when this Oxymakefile
    // was produced by `ox translate`. Surfaced by ox-plan errors so failures
    // can cite the original source location.
    source_line: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RawLogConfig {
    stdout: Option<String>,
    stderr: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawGate {
    #[serde(default)]
    after: Vec<String>,
    #[serde(default)]
    before: Vec<String>,
    #[serde(default)]
    message: String,
}

// ---------------------------------------------------------------------------
// Tilde expansion
// ---------------------------------------------------------------------------

/// Expand a leading `~` or `~/` in a path string to the user's home directory.
///
/// - `~/foo` → `/home/user/foo`
/// - `~` → `/home/user`
/// - Paths not starting with `~` are returned unchanged.
/// - If `HOME` is not set, the path is returned unchanged.
fn expand_tilde(path: &str) -> String {
    if path == "~" {
        std::env::var("HOME").unwrap_or_else(|_| path.to_string())
    } else if let Some(rest) = path.strip_prefix("~/") {
        match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => path.to_string(),
        }
    } else {
        path.to_string()
    }
}

/// Expand tildes in a [`PathBuf`] if the path starts with `~`.
fn expand_tilde_path(p: &PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if s.starts_with('~') {
        PathBuf::from(expand_tilde(&s))
    } else {
        p.clone()
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse an Oxymakefile from a TOML string.
///
/// `include` directives are expanded transitively: rules, config keys, and
/// gates from each included file are merged into the returned [`Workflow`].
/// The including file wins on config-key conflicts; a duplicate rule name
/// is an error, as are missing files and include cycles (H29).
pub fn parse_workflow(content: &str, file_path: &Path) -> Result<Workflow, ParseError> {
    // Seed cycle detection with the root file. Canonicalization may fail
    // for in-memory parses (tests pass paths that don't exist) — fall back
    // to the literal path; includes themselves must exist on disk.
    let root_id = file_path
        .canonicalize()
        .unwrap_or_else(|_| file_path.to_path_buf());
    let mut visited = vec![root_id];
    parse_workflow_inner(content, file_path, &mut visited)
}

fn parse_workflow_inner(
    content: &str,
    file_path: &Path,
    visited: &mut Vec<PathBuf>,
) -> Result<Workflow, ParseError> {
    let raw: RawOxymakefile = toml::from_str(content).map_err(|e| ParseError::Toml {
        file: file_path.to_path_buf(),
        message: e.to_string(),
    })?;

    let mut config = parse_config(&raw.config);

    // Resolve file-based config sources (read_csv/read_tsv at parse time).
    let base_dir = file_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    resolve_file_sources(&mut config, base_dir)?;

    // Parse global environment default.
    let global_environment = parse_environment(&raw.environment);

    let mut rules = Vec::new();
    for (name, raw_rule) in &raw.rule {
        let mut rule = parse_rule(name, raw_rule, file_path)?;
        // Inherit global environment if rule doesn't specify one.
        if rule.environment.is_none() {
            rule.environment = global_environment.clone();
        }
        rules.push(rule);
    }

    let gates = raw
        .gate
        .into_iter()
        .map(|(name, g)| Gate {
            name,
            after: g.after,
            before: g.before,
            message: g.message,
        })
        .collect();

    let profiles = parse_profiles(raw.profile);
    let executor_config = parse_executor_config(raw.executor);

    let includes: Vec<PathBuf> = raw
        .include
        .into_iter()
        .map(|s| PathBuf::from(expand_tilde(&s)))
        .collect();

    // Expand ~ in config values.
    for cv in config.values_mut() {
        match cv {
            ConfigValue::Scalar(s) => *s = expand_tilde(s),
            ConfigValue::List(items) => {
                for item in items.iter_mut() {
                    *item = expand_tilde(item);
                }
            }
            ConfigValue::FileSource { source, .. } => {
                *source = expand_tilde_path(source);
            }
        }
    }

    // Expand ~ in rule paths.
    for rule in &mut rules {
        for inp in &mut rule.inputs {
            inp.pattern = expand_tilde(&inp.pattern).into();
        }
        for out in &mut rule.outputs {
            out.pattern = expand_tilde(&out.pattern).into();
        }
        if let ExecutionBlock::Script { path, .. } = &mut rule.execution {
            *path = expand_tilde_path(path);
        }
        if let Some(ref mut stdout) = rule.log.stdout {
            *stdout = expand_tilde(stdout);
        }
        if let Some(ref mut stderr) = rule.log.stderr {
            *stderr = expand_tilde(stderr);
        }
        if let Some(ref mut bench) = rule.benchmark {
            *bench = expand_tilde(bench);
        }
    }

    let format_version = raw
        .format_version
        .unwrap_or_else(|| DEFAULT_FORMAT_VERSION.to_string());

    let mut workflow = Workflow {
        ox_version: raw.ox_version,
        format_version,
        config,
        rules,
        gates,
        includes,
        global_environment,
        profiles,
        executor_config,
    };

    expand_includes(&mut workflow, base_dir, file_path, visited)?;

    Ok(workflow)
}

/// Merge each `include` directive into `workflow` (H29).
///
/// - Rules and gates are appended; a duplicate rule name is an error.
/// - Config keys from included files are merged; the including file wins.
/// - `[profile.*]` and `[executor.*]` are root-only: an included file that
///   defines them is rejected so nothing is silently dropped.
/// - Cycles and missing files are errors (`CircularInclude`, `IncludeNotFound`).
fn expand_includes(
    workflow: &mut Workflow,
    base_dir: &Path,
    file_path: &Path,
    visited: &mut Vec<PathBuf>,
) -> Result<(), ParseError> {
    if workflow.includes.is_empty() {
        return Ok(());
    }

    for inc in workflow.includes.clone() {
        let inc_path = if inc.is_absolute() {
            inc.clone()
        } else {
            base_dir.join(&inc)
        };
        let canon = inc_path
            .canonicalize()
            .map_err(|_| ParseError::IncludeNotFound {
                path: inc_path.clone(),
            })?;
        if visited.contains(&canon) {
            let mut chain = visited.clone();
            chain.push(canon);
            return Err(ParseError::CircularInclude { chain });
        }
        visited.push(canon);

        let inc_content =
            std::fs::read_to_string(&inc_path).map_err(|_| ParseError::IncludeNotFound {
                path: inc_path.clone(),
            })?;
        let inc_wf = parse_workflow_inner(&inc_content, &inc_path, visited)?;

        // Root-only sections: reject rather than silently drop.
        if !inc_wf.profiles.is_empty() {
            return Err(ParseError::InvalidField {
                field: "include".into(),
                reason: format!(
                    "included file {} defines [profile.*] — profiles are only \
                     allowed in the root Oxymakefile",
                    inc_path.display()
                ),
            });
        }
        if inc_wf.executor_config != ExecutorConfig::default() {
            return Err(ParseError::InvalidField {
                field: "include".into(),
                reason: format!(
                    "included file {} defines [executor.*] — executor config is \
                     only allowed in the root Oxymakefile",
                    inc_path.display()
                ),
            });
        }

        // Rules: append, duplicate names are an error.
        for rule in inc_wf.rules {
            if workflow.rules.iter().any(|r| r.name == rule.name) {
                return Err(ParseError::DuplicateRule {
                    name: rule.name.as_str().to_string(),
                    first: file_path.to_path_buf(),
                    second: inc_path.clone(),
                });
            }
            workflow.rules.push(rule);
        }

        // Config: the including file wins on conflicts.
        for (key, value) in inc_wf.config {
            workflow.config.entry(key).or_insert(value);
        }

        // Gates: append.
        workflow.gates.extend(inc_wf.gates);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Config parsing
// ---------------------------------------------------------------------------

fn parse_executor_config(raw: Option<RawExecutorTable>) -> ExecutorConfig {
    let raw = match raw {
        Some(r) => r,
        None => return ExecutorConfig::default(),
    };
    ExecutorConfig {
        slurm: raw.slurm.map(|rs| SlurmExecutorToml {
            mode: rs.mode,
            api_url: rs.api_url,
            token_cmd: rs.token_cmd,
            partition: rs.partition,
            account: rs.account,
            qos: rs.qos,
            staging_dir: rs.staging_dir,
            extra_flags: rs.extra_flags,
        }),
    }
}

fn parse_profiles(raw: BTreeMap<String, RawProfile>) -> BTreeMap<String, Profile> {
    raw.into_iter()
        .map(|(name, rp)| {
            let verbose = rp.verbose.and_then(|v| match v {
                toml::Value::Boolean(true) => Some(1),
                toml::Value::Boolean(false) => Some(0),
                toml::Value::Integer(n) => Some(n as u8),
                _ => None,
            });
            let profile = Profile {
                jobs: rp.jobs,
                cache_validation: rp.cache_validation,
                verbose,
                executor: rp.executor,
                no_cache: rp.no_cache,
                keep_going: rp.keep_going,
                partition: rp.partition,
                account: rp.account,
                qos: rp.qos,
                open_dashboard: rp.open_dashboard,
                set: rp.set,
            };
            (name, profile)
        })
        .collect()
}

fn parse_config(raw: &BTreeMap<String, toml::Value>) -> BTreeMap<String, ConfigValue> {
    let mut config = BTreeMap::new();
    for (key, value) in raw {
        let cv = match value {
            toml::Value::Array(arr) => {
                let items: Vec<String> = arr
                    .iter()
                    .map(|v| match v {
                        toml::Value::String(s) => s.clone(),
                        toml::Value::Integer(i) => i.to_string(),
                        toml::Value::Float(f) => f.to_string(),
                        other => other.to_string(),
                    })
                    .collect();
                ConfigValue::List(items)
            }
            toml::Value::Table(tbl) => {
                if let (Some(toml::Value::String(source)), Some(toml::Value::String(k))) =
                    (tbl.get("source"), tbl.get("key"))
                {
                    let columns = tbl
                        .get("columns")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    ConfigValue::FileSource {
                        source: PathBuf::from(source),
                        key: k.clone(),
                        columns,
                    }
                } else {
                    // Fallback: serialize as string
                    ConfigValue::Scalar(value.to_string())
                }
            }
            toml::Value::String(s) => ConfigValue::Scalar(s.clone()),
            toml::Value::Integer(i) => ConfigValue::Scalar(i.to_string()),
            toml::Value::Float(f) => ConfigValue::Scalar(f.to_string()),
            toml::Value::Boolean(b) => ConfigValue::Scalar(b.to_string()),
            toml::Value::Datetime(dt) => ConfigValue::Scalar(dt.to_string()),
        };
        config.insert(key.clone(), cv);
    }
    config
}

/// Resolve file-based config sources by reading CSV/TSV files at parse time.
///
/// For each `FileSource` config entry, read the file and extract unique values
/// from the key column, converting to a `List`. If `columns` are specified,
/// create a list entry for each column.
///
/// Returns an error if a source file cannot be read or a key column is missing.
fn resolve_file_sources(
    config: &mut BTreeMap<String, ConfigValue>,
    base_dir: &Path,
) -> Result<(), ParseError> {
    let sources: Vec<(String, PathBuf, PathBuf, String, Vec<String>)> = config
        .iter()
        .filter_map(|(name, cv)| match cv {
            ConfigValue::FileSource {
                source,
                key,
                columns,
            } => Some((
                name.clone(),
                source.clone(),
                base_dir.join(source),
                key.clone(),
                columns.clone(),
            )),
            _ => None,
        })
        .collect();

    for (name, source_path, path, key_col, extra_columns) in sources {
        let content = std::fs::read_to_string(&path).map_err(|e| ParseError::ConfigFileRead {
            key: name.clone(),
            path: source_path.clone(),
            reason: e.to_string(),
        })?;

        let mut lines = content.lines();
        let header_line = match lines.next() {
            Some(h) if !h.trim().is_empty() => h,
            _ => {
                return Err(ParseError::ConfigFileRead {
                    key: name,
                    path: source_path,
                    reason: "file is empty".into(),
                });
            }
        };

        // Detect delimiter (tab or comma).
        let delim = if header_line.contains('\t') {
            '\t'
        } else {
            ','
        };
        let headers: Vec<&str> = header_line.split(delim).map(|s| s.trim()).collect();

        // Find the key column index.
        let key_idx = headers.iter().position(|h| *h == key_col).ok_or_else(|| {
            ParseError::ConfigColumnNotFound {
                key: name.clone(),
                path: source_path.clone(),
                column: key_col.clone(),
                available: headers.join(", "),
            }
        })?;

        // Validate extra columns exist.
        let col_indices: Vec<(String, usize)> = extra_columns
            .iter()
            .map(|c| {
                headers
                    .iter()
                    .position(|h| *h == c.as_str())
                    .map(|idx| (c.clone(), idx))
                    .ok_or_else(|| ParseError::ConfigColumnNotFound {
                        key: name.clone(),
                        path: source_path.clone(),
                        column: c.clone(),
                        available: headers.join(", "),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut key_values: Vec<String> = Vec::new();
        let mut col_values: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for col_name in &extra_columns {
            col_values.insert(col_name.clone(), Vec::new());
        }

        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(delim).map(|s| s.trim()).collect();
            if let Some(val) = fields.get(key_idx) {
                let v = val.to_string();
                if !v.is_empty() && !key_values.contains(&v) {
                    key_values.push(v);
                }
            }
            for (col_name, col_idx) in &col_indices {
                if let Some(val) = fields.get(*col_idx) {
                    let v = val.to_string();
                    if let Some(vals) = col_values.get_mut(col_name) {
                        if !v.is_empty() && !vals.contains(&v) {
                            vals.push(v);
                        }
                    }
                }
            }
        }

        // Replace the FileSource with a List of unique values from the key column.
        config.insert(name, ConfigValue::List(key_values));

        // Add extra column lists to config.
        for (col_name, values) in col_values {
            config.insert(col_name, ConfigValue::List(values));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Rule parsing
// ---------------------------------------------------------------------------

fn parse_rule(name: &str, raw: &RawRule, file_path: &Path) -> Result<Rule, ParseError> {
    let inputs = parse_inputs(&raw.input);
    let outputs = parse_outputs(&raw.output)?;
    let execution = parse_execution(name, raw, file_path)?;
    let resources = parse_resources(&raw.resources);
    let environment = parse_environment(&raw.environment);
    let expand_mode = parse_expand_mode(&raw.expand);
    let error_strategy = if let Some(n) = raw.retries {
        // `retries = N` is shorthand for error_strategy = retry with defaults.
        // Explicit error_strategy takes precedence if both are set.
        if raw.error_strategy.is_some() {
            parse_error_strategy(&raw.error_strategy)?
        } else {
            ErrorStrategy::Retry {
                count: n,
                backoff: Backoff::Exponential,
            }
        }
    } else {
        parse_error_strategy(&raw.error_strategy)?
    };
    let timeout = raw.timeout.as_deref().and_then(parse_duration);
    let when = raw.when.as_ref().map(parse_guard_expr).transpose()?;
    let log = raw
        .log
        .as_ref()
        .map(|l| LogConfig {
            stdout: l.stdout.clone(),
            stderr: l.stderr.clone(),
        })
        .unwrap_or_default();

    Ok(Rule {
        name: RuleName(name.to_string()),
        priority: raw.priority,
        inputs,
        outputs,
        execution,
        resources,
        environment,
        tags: raw.tags.clone(),
        meta: RuleMeta {
            description: raw.description.clone(),
        },
        wildcard_constraints: raw.wildcard_constraints.clone(),
        when,
        expand_mode,
        error_strategy,
        timeout,
        executor: raw.executor.clone(),
        log,
        benchmark: raw.benchmark.clone(),
        retries: raw.retries,
        params: parse_params(&raw.params),
        param_files: raw.param_files.clone(),
        shell_executable: raw.shell_executable.clone(),
        reproducibility: match raw.reproducibility.as_deref() {
            Some("deterministic") => ReproducibilityClass::Deterministic,
            Some("seed_deterministic") => ReproducibilityClass::SeedDeterministic,
            Some("approximate") => ReproducibilityClass::Approximate,
            Some("non_reproducible") => ReproducibilityClass::NonReproducible,
            _ => ReproducibilityClass::default(),
        },
        source_line: raw.source_line,
    })
}

// ---------------------------------------------------------------------------
// Guard expression parsing
// ---------------------------------------------------------------------------

/// Parse a TOML value into a [`GuardExpr`].
///
/// Supported formats:
///
/// ```toml
/// when = { op = "eq", field = "sample", value = "A" }
/// when = { op = "in", field = "sample", values = ["A", "B"] }
/// when = { op = "not_in", field = "sample", values = ["X"] }
/// when = { op = "not_eq", field = "sample", value = "X" }
/// when = { op = "regex", field = "sample", pattern = "^patient_.*" }
/// when = { op = "config_eq", key = "mode", value = "production" }
/// when = { op = "env_set", var = "CI" }
/// when = { op = "env_eq", var = "STAGE", value = "prod" }
/// when = { op = "file_exists", path = "data/override.csv" }
/// when = { op = "and", conditions = [{ op = "env_set", var = "CI" }, ...] }
/// when = { op = "or", conditions = [...] }
/// when = { op = "not", condition = { op = "env_set", var = "CI" } }
/// ```
fn parse_guard_expr(value: &toml::Value) -> Result<GuardExpr, ParseError> {
    let tbl = value.as_table().ok_or_else(|| ParseError::InvalidField {
        field: "when".into(),
        reason: "expected a table with an 'op' key".into(),
    })?;

    let op = tbl
        .get("op")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ParseError::InvalidField {
            field: "when.op".into(),
            reason: "missing or non-string 'op' key".into(),
        })?;

    match op {
        "eq" => {
            let field = require_str(tbl, "field", "when")?;
            let value = require_str(tbl, "value", "when")?;
            Ok(GuardExpr::Eq { field, value })
        }
        "not_eq" => {
            let field = require_str(tbl, "field", "when")?;
            let value = require_str(tbl, "value", "when")?;
            Ok(GuardExpr::NotEq { field, value })
        }
        "in" => {
            let field = require_str(tbl, "field", "when")?;
            let values = require_str_array(tbl, "values", "when")?;
            Ok(GuardExpr::In { field, values })
        }
        "not_in" => {
            let field = require_str(tbl, "field", "when")?;
            let values = require_str_array(tbl, "values", "when")?;
            Ok(GuardExpr::NotIn { field, values })
        }
        "regex" => {
            let field = require_str(tbl, "field", "when")?;
            let pattern = require_str(tbl, "pattern", "when")?;
            Ok(GuardExpr::Regex { field, pattern })
        }
        "config_eq" => {
            let key = require_str(tbl, "key", "when")?;
            let value = require_str(tbl, "value", "when")?;
            Ok(GuardExpr::ConfigEq { key, value })
        }
        "env_set" => {
            let var = require_str(tbl, "var", "when")?;
            Ok(GuardExpr::EnvSet { var })
        }
        "env_eq" => {
            let var = require_str(tbl, "var", "when")?;
            let value = require_str(tbl, "value", "when")?;
            Ok(GuardExpr::EnvEq { var, value })
        }
        "file_exists" => {
            let path = require_str(tbl, "path", "when")?;
            Ok(GuardExpr::FileExists { path })
        }
        "and" => {
            let conditions = require_guard_array(tbl, "conditions", "when")?;
            Ok(GuardExpr::And { conditions })
        }
        "or" => {
            let conditions = require_guard_array(tbl, "conditions", "when")?;
            Ok(GuardExpr::Or { conditions })
        }
        "not" => {
            let inner = tbl
                .get("condition")
                .ok_or_else(|| ParseError::InvalidField {
                    field: "when.condition".into(),
                    reason: "missing 'condition' for 'not' op".into(),
                })?;
            let condition = parse_guard_expr(inner)?;
            Ok(GuardExpr::Not {
                condition: Box::new(condition),
            })
        }
        other => Err(ParseError::InvalidField {
            field: "when.op".into(),
            reason: format!("unknown guard op: {other:?}"),
        }),
    }
}

fn require_str(
    tbl: &toml::map::Map<String, toml::Value>,
    key: &str,
    context: &str,
) -> Result<String, ParseError> {
    tbl.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| ParseError::InvalidField {
            field: format!("{context}.{key}"),
            reason: format!("missing or non-string '{key}'"),
        })
}

fn require_str_array(
    tbl: &toml::map::Map<String, toml::Value>,
    key: &str,
    context: &str,
) -> Result<Vec<String>, ParseError> {
    let arr = tbl
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| ParseError::InvalidField {
            field: format!("{context}.{key}"),
            reason: format!("missing or non-array '{key}'"),
        })?;
    Ok(arr
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect())
}

fn require_guard_array(
    tbl: &toml::map::Map<String, toml::Value>,
    key: &str,
    context: &str,
) -> Result<Vec<GuardExpr>, ParseError> {
    let arr = tbl
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| ParseError::InvalidField {
            field: format!("{context}.{key}"),
            reason: format!("missing or non-array '{key}'"),
        })?;
    arr.iter().map(parse_guard_expr).collect()
}

// ---------------------------------------------------------------------------
// Input parsing
// ---------------------------------------------------------------------------

fn parse_inputs(value: &Option<toml::Value>) -> Vec<InputPattern> {
    let Some(value) = value else {
        return Vec::new();
    };

    match value {
        // Simple array: input = ["path/{wc}.ext", ...]
        toml::Value::Array(arr) => arr
            .iter()
            .filter_map(|item| match item {
                toml::Value::String(s) => Some(InputPattern {
                    pattern: s.clone().into(),
                    name: None,
                    format: None,
                }),
                toml::Value::Table(tbl) => {
                    let path = tbl
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let format = tbl.get("format").and_then(|v| v.as_str()).map(String::from);
                    let name = tbl.get("name").and_then(|v| v.as_str()).map(String::from);
                    Some(InputPattern {
                        pattern: path.into(),
                        name,
                        format,
                    })
                }
                _ => None,
            })
            .collect(),

        // Named map: input = { features = "path/{wc}.ext", ... }
        toml::Value::Table(tbl) => tbl
            .iter()
            .map(|(k, v)| InputPattern {
                pattern: v.as_str().unwrap_or_default().to_string().into(),
                name: Some(k.clone()),
                format: None,
            })
            .collect(),

        // Single string: input = "path/{wc}.ext"
        toml::Value::String(s) => vec![InputPattern {
            pattern: s.clone().into(),
            name: None,
            format: None,
        }],

        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Output parsing
// ---------------------------------------------------------------------------

fn parse_outputs(value: &Option<toml::Value>) -> Result<Vec<OutputPattern>, ParseError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    match value {
        toml::Value::Array(arr) => {
            let mut outputs = Vec::new();
            for item in arr {
                match item {
                    toml::Value::String(s) => {
                        outputs.push(OutputPattern {
                            pattern: s.clone().into(),
                            name: None,
                            format: None,
                            lifecycle: OutputLifecycle::default(),
                            materialize: MaterializePolicy::default(),
                        });
                    }
                    toml::Value::Table(tbl) => {
                        let path = tbl
                            .get("path")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let format = tbl.get("format").and_then(|v| v.as_str()).map(String::from);
                        let name = tbl.get("name").and_then(|v| v.as_str()).map(String::from);
                        let lifecycle = match tbl.get("lifecycle").and_then(|v| v.as_str()) {
                            Some(s) => parse_lifecycle(s)?,
                            None => OutputLifecycle::default(),
                        };
                        let materialize = match tbl.get("materialize").and_then(|v| v.as_str()) {
                            Some(s) => parse_materialize_policy(s)?,
                            None => MaterializePolicy::default(),
                        };
                        outputs.push(OutputPattern {
                            pattern: path.into(),
                            name,
                            format,
                            lifecycle,
                            materialize,
                        });
                    }
                    _ => {}
                }
            }
            Ok(outputs)
        }

        // Named map: output = { counts = "results/{sample}_counts.txt", ... }
        toml::Value::Table(tbl) => Ok(tbl
            .iter()
            .map(|(k, v)| OutputPattern {
                pattern: v.as_str().unwrap_or_default().to_string().into(),
                name: Some(k.clone()),
                format: None,
                lifecycle: OutputLifecycle::default(),
                materialize: MaterializePolicy::default(),
            })
            .collect()),

        toml::Value::String(s) => Ok(vec![OutputPattern {
            pattern: s.clone().into(),
            name: None,
            format: None,
            lifecycle: OutputLifecycle::default(),
            materialize: MaterializePolicy::default(),
        }]),

        _ => Ok(Vec::new()),
    }
}

fn parse_lifecycle(s: &str) -> Result<OutputLifecycle, ParseError> {
    match s {
        "temporary" => Ok(OutputLifecycle::Temporary),
        "protected" => Ok(OutputLifecycle::Protected),
        "permanent" => Ok(OutputLifecycle::Permanent),
        other => Err(ParseError::InvalidField {
            field: "lifecycle".to_string(),
            reason: format!(
                "unknown lifecycle value `{other}`, expected one of: temporary, protected, permanent"
            ),
        }),
    }
}

fn parse_materialize_policy(s: &str) -> Result<MaterializePolicy, ParseError> {
    match s {
        "auto" => Ok(MaterializePolicy::Auto),
        "never" => Ok(MaterializePolicy::Never),
        "final" => Ok(MaterializePolicy::Final),
        "always" => Ok(MaterializePolicy::Always),
        other => Err(ParseError::InvalidField {
            field: "materialize".to_string(),
            reason: format!(
                "unknown materialize value `{other}`, expected one of: auto, never, final, always"
            ),
        }),
    }
}

// ---------------------------------------------------------------------------
// Execution mode parsing
// ---------------------------------------------------------------------------

fn parse_execution(
    rule_name: &str,
    raw: &RawRule,
    _file_path: &Path,
) -> Result<ExecutionBlock, ParseError> {
    let modes: Vec<(&str, bool)> = vec![
        ("shell", raw.shell.is_some()),
        ("run", raw.run.is_some()),
        ("script", raw.script.is_some()),
        ("call", raw.call.is_some()),
    ];

    let present: Vec<&str> = modes.iter().filter(|(_, p)| *p).map(|(n, _)| *n).collect();

    match present.len() {
        0 => {
            // Rules with only inputs (like "all") are valid — they act as
            // aggregation targets. Use a no-op shell command.
            if !raw.inputs_only() {
                return Err(ParseError::MissingExecution {
                    rule: rule_name.to_string(),
                });
            }
            // Default: phony/aggregation rule
            Ok(ExecutionBlock::Shell {
                command: String::new(),
            })
        }
        1 => {
            if let Some(shell) = raw.shell.clone() {
                Ok(ExecutionBlock::Shell { command: shell })
            } else if let Some(run) = raw.run.clone() {
                Ok(ExecutionBlock::Run {
                    code: run,
                    lang: raw.lang.clone().unwrap_or_else(|| "python".to_string()),
                })
            } else if let Some(script) = raw.script.clone() {
                Ok(ExecutionBlock::Script {
                    path: PathBuf::from(script),
                    lang: raw.lang.clone(),
                })
            } else if let Some(call) = raw.call.clone() {
                Ok(ExecutionBlock::Call {
                    function: call,
                    lang: raw.lang.clone().unwrap_or_else(|| "python".to_string()),
                })
            } else {
                unreachable!("present.len() == 1 but no execution field matched")
            }
        }
        _ => {
            let a = present[0].to_string();
            let b = present[1].to_string();
            Err(ParseError::ConflictingExecution {
                rule: rule_name.to_string(),
                a,
                b,
            })
        }
    }
}

impl RawRule {
    /// Returns true if this rule has only inputs and no outputs or execution fields.
    /// Such rules serve as aggregation targets (like "all").
    fn inputs_only(&self) -> bool {
        self.output.is_none()
            && self.shell.is_none()
            && self.run.is_none()
            && self.script.is_none()
            && self.call.is_none()
    }
}

// ---------------------------------------------------------------------------
// Resource parsing
// ---------------------------------------------------------------------------

fn parse_resources(raw: &BTreeMap<String, toml::Value>) -> BTreeMap<String, ResourceValue> {
    let mut out = BTreeMap::new();
    for (k, v) in raw {
        let rv = match v {
            toml::Value::Integer(i) => ResourceValue::Int(*i),
            toml::Value::Float(f) => ResourceValue::Float((*f).into()),
            toml::Value::String(s) => ResourceValue::Str(s.clone()),
            _ => ResourceValue::Str(v.to_string()),
        };
        out.insert(k.clone(), rv);
    }
    out
}

// ---------------------------------------------------------------------------
// Environment parsing
// ---------------------------------------------------------------------------

fn parse_environment(raw: &Option<BTreeMap<String, String>>) -> Option<EnvSpec> {
    let env = raw.as_ref()?;

    if let Some(req) = env.get("uv") {
        // Empty string or pyproject.toml → no -r flag needed.
        // uv auto-discovers pyproject.toml, and -r only accepts
        // requirements.txt-style files.
        let requirements = if req.is_empty() || req == "pyproject.toml" {
            None
        } else {
            Some(req.clone())
        };
        return Some(EnvSpec::Uv { requirements });
    }
    if let Some(e) = env.get("conda") {
        return Some(EnvSpec::Conda { env: e.clone() });
    }
    if let Some(img) = env.get("docker") {
        return Some(EnvSpec::Docker { image: img.clone() });
    }
    if let Some(expr) = env.get("nix") {
        return Some(EnvSpec::Nix { expr: expr.clone() });
    }
    if let Some(img) = env.get("apptainer") {
        return Some(EnvSpec::Apptainer { image: img.clone() });
    }

    None
}

// ---------------------------------------------------------------------------
// Params parsing
// ---------------------------------------------------------------------------

/// Parse params table: `params = { key = "value", key2 = 42 }`.
fn parse_params(raw: &BTreeMap<String, toml::Value>) -> BTreeMap<String, String> {
    raw.iter()
        .map(|(k, v)| {
            let s = match v {
                toml::Value::String(s) => s.clone(),
                toml::Value::Integer(n) => n.to_string(),
                toml::Value::Float(f) => f.to_string(),
                toml::Value::Boolean(b) => b.to_string(),
                _ => v.to_string(),
            };
            (k.clone(), s)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Expand mode parsing
// ---------------------------------------------------------------------------

fn parse_expand_mode(raw: &Option<String>) -> ExpandMode {
    match raw.as_deref() {
        Some("zip") => ExpandMode::Zip,
        _ => ExpandMode::Product,
    }
}

// ---------------------------------------------------------------------------
// Error strategy parsing
// ---------------------------------------------------------------------------

fn parse_error_strategy(raw: &Option<toml::Value>) -> Result<ErrorStrategy, ParseError> {
    let Some(value) = raw else {
        return Ok(ErrorStrategy::default());
    };

    match value {
        toml::Value::String(s) => Ok(match s.as_str() {
            "terminate" => ErrorStrategy::Terminate,
            "ignore" => ErrorStrategy::Ignore,
            "finish" => ErrorStrategy::Finish,
            _ => ErrorStrategy::default(),
        }),
        toml::Value::Table(tbl) => {
            if tbl.contains_key("jitter") {
                return Err(ParseError::InvalidField {
                    field: "error_strategy.jitter".into(),
                    reason: "the `jitter` option is not supported; remove it from your Oxymakefile"
                        .into(),
                });
            }
            let count = tbl.get("retry").and_then(|v| v.as_integer()).unwrap_or(1) as u32;
            let backoff = tbl
                .get("backoff")
                .and_then(|v| v.as_str())
                .map(|s| match s {
                    "constant" => Backoff::Constant,
                    "linear" => Backoff::Linear,
                    _ => Backoff::Exponential,
                })
                .unwrap_or_default();
            Ok(ErrorStrategy::Retry { count, backoff })
        }
        _ => Ok(ErrorStrategy::default()),
    }
}

// ---------------------------------------------------------------------------
// Duration parsing
// ---------------------------------------------------------------------------

/// Parse a human-friendly duration string like "30m", "2h", "90s".
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Split into numeric part and multiplier
    let (num_str, multiplier) = if s.ends_with('s') {
        (&s[..s.len() - 1], 1)
    } else if s.ends_with('m') {
        (&s[..s.len() - 1], 60)
    } else if s.ends_with('h') {
        (&s[..s.len() - 1], 3600)
    } else if s.ends_with('d') {
        (&s[..s.len() - 1], 86400)
    } else {
        // Assume seconds
        (s, 1)
    };

    let num: u64 = num_str.parse().ok()?;
    Some(Duration::from_secs(num * multiplier))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_debug_snapshot;
    use std::path::Path;

    fn fixture_path(name: &str) -> PathBuf {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        PathBuf::from(manifest_dir)
            .join("../../tests/fixtures")
            .join(name)
            .join("Oxymakefile.toml")
    }

    fn parse_fixture(name: &str) -> Workflow {
        let path = fixture_path(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
        parse_workflow(&content, &path).unwrap_or_else(|e| panic!("Parse failed: {e}"))
    }

    #[test]
    fn parse_simple_fixture() {
        let wf = parse_fixture("simple");
        assert_debug_snapshot!(wf);
    }

    #[test]
    fn parse_genomics_fixture() {
        let wf = parse_fixture("genomics");
        assert_debug_snapshot!(wf);
    }

    #[test]
    fn format_version_defaults_to_1_when_absent() {
        let toml = r#"
ox_version = "0.1"

[rule.noop]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp {input} {output}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.format_version, "1");
    }

    #[test]
    fn format_version_is_read_from_toml_when_present() {
        let toml = r#"
ox_version = "0.1"
format_version = "1"

[rule.noop]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp {input} {output}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.format_version, "1");
    }

    #[test]
    fn format_version_accepts_arbitrary_string_until_validation_lands() {
        // The validator may reject unknown versions in the future; today
        // we surface whatever the file declared so STATUS.md / lint can
        // decide.
        let toml = r#"
format_version = "2-rc"

[rule.noop]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp {input} {output}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.format_version, "2-rc");
    }

    #[test]
    fn parse_minimal_rule() {
        let toml = r#"
ox_version = "0.1"

[rule.build]
input = ["src/main.rs"]
output = ["target/main"]
shell = "rustc {input} -o {output}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules.len(), 1);
        assert_eq!(wf.rules[0].name.as_str(), "build");
        assert!(matches!(
            wf.rules[0].execution,
            ExecutionBlock::Shell { .. }
        ));
    }

    #[test]
    fn parse_call_mode() {
        let toml = r#"
[rule.transform]
input = ["data.csv"]
output = ["out.parquet"]
lang = "python"
call = "my_module:transform"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(wf.rules[0].execution, ExecutionBlock::Call { .. }));
    }

    #[test]
    fn parse_run_mode() {
        let toml = r#"
[rule.inline]
input = ["data.csv"]
output = ["out.csv"]
lang = "python"
run = "import pandas; pandas.read_csv('data.csv').to_csv('out.csv')"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(wf.rules[0].execution, ExecutionBlock::Run { .. }));
    }

    #[test]
    fn parse_script_mode() {
        let toml = r#"
[rule.run_script]
input = ["data.csv"]
output = ["out.csv"]
script = "scripts/process.py"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.rules[0].execution,
            ExecutionBlock::Script { .. }
        ));
    }

    #[test]
    fn error_conflicting_execution() {
        let toml = r#"
[rule.bad]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"
call = "mod:func"
lang = "python"
"#;
        let err = parse_workflow(toml, Path::new("test.toml")).unwrap_err();
        assert!(matches!(err, ParseError::ConflictingExecution { .. }));
    }

    #[test]
    fn error_invalid_toml() {
        let toml = "this is not valid toml [[[";
        let err = parse_workflow(toml, Path::new("test.toml")).unwrap_err();
        assert!(matches!(err, ParseError::Toml { .. }));
    }

    #[test]
    fn parse_input_string_array() {
        let toml = r#"
[rule.test]
input = ["a.txt", "b.txt"]
output = ["c.txt"]
shell = "cat {input} > {output}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].inputs.len(), 2);
        assert_eq!(wf.rules[0].inputs[0].pattern, "a.txt");
        assert!(wf.rules[0].inputs[0].name.is_none());
    }

    #[test]
    fn parse_input_object_array() {
        let toml = r#"
[rule.test]
input = [{ path = "data.parquet", format = "parquet", name = "features" }]
output = ["out.csv"]
shell = "process {input}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].inputs.len(), 1);
        assert_eq!(wf.rules[0].inputs[0].pattern, "data.parquet");
        assert_eq!(wf.rules[0].inputs[0].format.as_deref(), Some("parquet"));
        assert_eq!(wf.rules[0].inputs[0].name.as_deref(), Some("features"));
    }

    #[test]
    fn parse_input_named_map() {
        let toml = r#"
[rule.test]
output = ["out.csv"]
lang = "python"
call = "mod:func"

[rule.test.input]
features = "features/{sample}.parquet"
labels = "labels/{sample}.csv"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].inputs.len(), 2);
        let names: Vec<_> = wf.rules[0]
            .inputs
            .iter()
            .filter_map(|i| i.name.as_deref())
            .collect();
        assert!(names.contains(&"features"));
        assert!(names.contains(&"labels"));
    }

    #[test]
    fn parse_error_strategy_string() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp {input} {output}"
error_strategy = "ignore"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].error_strategy, ErrorStrategy::Ignore);
    }

    #[test]
    fn parse_error_strategy_object() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp {input} {output}"

[rule.test.error_strategy]
retry = 3
backoff = "exponential"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].error_strategy,
            ErrorStrategy::Retry {
                count: 3,
                backoff: Backoff::Exponential,
            }
        );
    }

    #[test]
    fn parse_error_strategy_jitter_rejected() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp {input} {output}"

[rule.test.error_strategy]
retry = 3
backoff = "exponential"
jitter = true
"#;
        let err = parse_workflow(toml, Path::new("test.toml")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("jitter"),
            "error should mention jitter, got: {msg}"
        );
    }

    #[test]
    fn parse_aggregation_rule_no_execution() {
        let toml = r#"
[rule.all]
input = ["results/{sample}.txt"]
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules.len(), 1);
        // Aggregation rules get a no-op shell
        assert!(matches!(
            wf.rules[0].execution,
            ExecutionBlock::Shell { ref command } if command.is_empty()
        ));
    }

    #[test]
    fn parse_config_values() {
        let toml = r#"
ox_version = "0.1"

[config]
samples = ["A", "B", "C"]
lookbacks = [5, 10, 20]
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.config.get("samples"),
            Some(ConfigValue::List(v)) if v == &["A", "B", "C"]
        ));
        assert!(matches!(
            wf.config.get("lookbacks"),
            Some(ConfigValue::List(v)) if v == &["5", "10", "20"]
        ));
    }

    #[test]
    fn parse_config_file_source_missing_file_errors() {
        let toml = r#"
[config]
symbols = { source = "symbols.csv", key = "ticker" }
"#;
        let err = parse_workflow(toml, Path::new("test.toml"));
        assert!(err.is_err(), "should error when source file doesn't exist");
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("symbols"), "error should mention config key");
        assert!(
            msg.contains("symbols.csv"),
            "error should mention file path"
        );
    }

    #[test]
    fn parse_config_file_source_tsv() {
        let dir = tempfile::tempdir().unwrap();
        let tsv = dir.path().join("units.tsv");
        std::fs::write(
            &tsv,
            "sample_name\tgroup\nA\tcontrol\nB\ttreatment\nC\tcontrol\n",
        )
        .unwrap();
        let oxf = dir.path().join("Oxymakefile.toml");
        let toml = r#"
[config]
samples = { source = "units.tsv", key = "sample_name" }
"#;
        let wf = parse_workflow(toml, &oxf).unwrap();
        assert_eq!(
            wf.config.get("samples"),
            Some(&ConfigValue::List(vec!["A".into(), "B".into(), "C".into()]))
        );
    }

    #[test]
    fn parse_config_file_source_csv_with_columns() {
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("data.csv");
        std::fs::write(
            &csv,
            "name,color,size\nalpha,red,10\nbeta,blue,20\nalpha,green,30\n",
        )
        .unwrap();
        let oxf = dir.path().join("Oxymakefile.toml");
        let toml = r#"
[config]
items = { source = "data.csv", key = "name", columns = ["color"] }
"#;
        let wf = parse_workflow(toml, &oxf).unwrap();
        assert_eq!(
            wf.config.get("items"),
            Some(&ConfigValue::List(vec!["alpha".into(), "beta".into()]))
        );
        assert_eq!(
            wf.config.get("color"),
            Some(&ConfigValue::List(vec![
                "red".into(),
                "blue".into(),
                "green".into()
            ]))
        );
    }

    #[test]
    fn parse_config_file_source_missing_column_errors() {
        let dir = tempfile::tempdir().unwrap();
        let tsv = dir.path().join("units.tsv");
        std::fs::write(&tsv, "sample_name\tgroup\nA\tcontrol\n").unwrap();
        let oxf = dir.path().join("Oxymakefile.toml");
        let toml = r#"
[config]
samples = { source = "units.tsv", key = "nonexistent" }
"#;
        let err = parse_workflow(toml, &oxf);
        assert!(err.is_err(), "should error when key column not found");
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("nonexistent"),
            "error should mention column name"
        );
        assert!(
            msg.contains("sample_name"),
            "error should list available columns"
        );
    }

    #[test]
    fn parse_config_file_source_empty_rows_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let tsv = dir.path().join("units.tsv");
        std::fs::write(&tsv, "id\n\nA\n\nB\n\n").unwrap();
        let oxf = dir.path().join("Oxymakefile.toml");
        let toml = r#"
[config]
ids = { source = "units.tsv", key = "id" }
"#;
        let wf = parse_workflow(toml, &oxf).unwrap();
        assert_eq!(
            wf.config.get("ids"),
            Some(&ConfigValue::List(vec!["A".into(), "B".into()]))
        );
    }

    #[test]
    fn parse_config_file_source_rejects_blank_header_and_skips_empty_values() {
        let dir = tempfile::tempdir().unwrap();
        let blank_header = dir.path().join("blank.csv");
        std::fs::write(&blank_header, "   \nA\n").unwrap();
        let oxf = dir.path().join("Oxymakefile.toml");
        let err = parse_workflow(
            "[config]\nitems = { source = \"blank.csv\", key = \"id\" }\n",
            &oxf,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::ConfigFileRead { .. }));
        assert!(err.to_string().contains("file is empty"));

        let csv = dir.path().join("items.csv");
        std::fs::write(&csv, "id,color\nA,red\n,\nA,red\nB,blue\n").unwrap();
        let wf = parse_workflow(
            "[config]\nitems = { source = \"items.csv\", key = \"id\", columns = [\"color\"] }\n",
            &oxf,
        )
        .unwrap();
        assert_eq!(
            wf.config.get("items"),
            Some(&ConfigValue::List(vec!["A".into(), "B".into()]))
        );
        assert_eq!(
            wf.config.get("color"),
            Some(&ConfigValue::List(vec!["red".into(), "blue".into()]))
        );
    }

    #[test]
    fn parse_executor_slurm_config_preserves_all_fields() {
        let toml = r#"
[executor.slurm]
mode = "rest"
api_url = "http://slurm.example:6820"
token_cmd = "scontrol token lifespan=3600"
partition = "gpu"
account = "research"
qos = "high"
staging_dir = "/scratch/ox"
extra_flags = ["--exclusive", "--gres=gpu:1"]
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.executor_config.slurm,
            Some(SlurmExecutorToml {
                mode: Some("rest".into()),
                api_url: Some("http://slurm.example:6820".into()),
                token_cmd: Some("scontrol token lifespan=3600".into()),
                partition: Some("gpu".into()),
                account: Some("research".into()),
                qos: Some("high".into()),
                staging_dir: Some("/scratch/ox".into()),
                extra_flags: vec!["--exclusive".into(), "--gres=gpu:1".into()],
            })
        );
    }

    #[test]
    fn parse_gate() {
        let toml = r#"
[gate.review]
after = ["build"]
before = ["deploy"]
message = "Review the build before deploying"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.gates.len(), 1);
        assert_eq!(wf.gates[0].name, "review");
        assert_eq!(wf.gates[0].after, vec!["build"]);
        assert_eq!(wf.gates[0].before, vec!["deploy"]);
    }

    #[test]
    fn parse_resources() {
        let toml = r#"
[rule.heavy]
input = ["a.txt"]
output = ["b.txt"]
shell = "process {input} > {output}"

[rule.heavy.resources]
cpu = 4
mem = "8G"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].resources.get("cpu"),
            Some(&ResourceValue::Int(4))
        );
        assert_eq!(
            wf.rules[0].resources.get("mem"),
            Some(&ResourceValue::Str("8G".into()))
        );
    }

    #[test]
    fn parse_environment_uv() {
        let toml = r#"
[rule.py]
input = ["a.csv"]
output = ["b.csv"]
lang = "python"
call = "mod:func"

[rule.py.environment]
uv = "requirements.txt"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.rules[0].environment,
            Some(EnvSpec::Uv { ref requirements })
            if requirements.as_deref() == Some("requirements.txt")
        ));
    }

    #[test]
    fn parse_environment_uv_empty_string() {
        let toml = r#"
[rule.py]
input = ["a.csv"]
output = ["b.csv"]
lang = "python"
call = "mod:func"

[rule.py.environment]
uv = ""
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.rules[0].environment,
            Some(EnvSpec::Uv { ref requirements })
            if requirements.is_none()
        ));
    }

    #[test]
    fn parse_environment_uv_pyproject_toml() {
        let toml = r#"
[rule.py]
input = ["a.csv"]
output = ["b.csv"]
lang = "python"
call = "mod:func"

[rule.py.environment]
uv = "pyproject.toml"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.rules[0].environment,
            Some(EnvSpec::Uv { ref requirements })
            if requirements.is_none()
        ));
    }

    #[test]
    fn parse_timeout() {
        let toml = r#"
[rule.slow]
input = ["a.txt"]
output = ["b.txt"]
shell = "sleep 100 && cp {input} {output}"
timeout = "30m"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].timeout, Some(Duration::from_secs(1800)));
    }

    #[test]
    fn parse_wildcard_constraints() {
        let toml = r#"
[rule.test]
input = ["data/{sample}.csv"]
output = ["out/{sample}.txt"]
shell = "process {input} > {output}"

[rule.test.wildcard_constraints]
sample = "[A-Z]+"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].wildcard_constraints.get("sample"),
            Some(&"[A-Z]+".to_string())
        );
    }

    #[test]
    fn parse_duration_variants() {
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("1d"), Some(Duration::from_secs(86400)));
        assert_eq!(parse_duration(""), None);
    }

    // -----------------------------------------------------------------------
    // Additional tests for 100% line coverage
    // -----------------------------------------------------------------------

    #[test]
    fn parse_duration_bare_number() {
        // No suffix => assumes seconds
        assert_eq!(parse_duration("42"), Some(Duration::from_secs(42)));
    }

    #[test]
    fn parse_duration_invalid_number() {
        assert_eq!(parse_duration("notanumber_s"), None);
    }

    #[test]
    fn parse_config_scalar_string() {
        let toml = r#"
[config]
name = "hello"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.config.get("name"),
            Some(&ConfigValue::Scalar("hello".to_string()))
        );
    }

    #[test]
    fn parse_config_scalar_integer() {
        let toml = r#"
[config]
count = 42
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.config.get("count"),
            Some(&ConfigValue::Scalar("42".to_string()))
        );
    }

    #[test]
    fn parse_config_scalar_float() {
        let toml = r#"
[config]
ratio = 3.14
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.config.get("ratio"),
            Some(&ConfigValue::Scalar("3.14".to_string()))
        );
    }

    #[test]
    fn parse_config_scalar_boolean() {
        let toml = r#"
[config]
flag = true
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.config.get("flag"),
            Some(&ConfigValue::Scalar("true".to_string()))
        );
    }

    #[test]
    fn parse_config_scalar_datetime() {
        let toml = r#"
[config]
date = 2024-01-15
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.config.get("date"),
            Some(&ConfigValue::Scalar("2024-01-15".to_string()))
        );
    }

    #[test]
    fn parse_config_table_fallback() {
        // Table without source/key fields falls back to Scalar serialization
        let toml = r#"
[config]
[config.nested]
x = "y"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.config.get("nested"),
            Some(ConfigValue::Scalar(_))
        ));
    }

    #[test]
    fn parse_config_array_with_floats() {
        let toml = r#"
[config]
thresholds = [1.5, 2.5]
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.config.get("thresholds"),
            Some(ConfigValue::List(v)) if v == &["1.5", "2.5"]
        ));
    }

    #[test]
    fn parse_config_array_with_booleans() {
        // Boolean in array hits the `other => other.to_string()` branch
        let toml = r#"
[config]
flags = [true, false]
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.config.get("flags"),
            Some(ConfigValue::List(v)) if v == &["true", "false"]
        ));
    }

    #[test]
    fn parse_log_config() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cat a.txt > b.txt"

[rule.test.log]
stdout = "logs/out.log"
stderr = "logs/err.log"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].log.stdout.as_deref(), Some("logs/out.log"));
        assert_eq!(wf.rules[0].log.stderr.as_deref(), Some("logs/err.log"));
    }

    #[test]
    fn parse_input_single_string() {
        let toml = r#"
[rule.test]
input = "single.txt"
output = ["b.txt"]
shell = "cat single.txt > b.txt"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].inputs.len(), 1);
        assert_eq!(wf.rules[0].inputs[0].pattern, "single.txt");
        assert!(wf.rules[0].inputs[0].name.is_none());
        assert!(wf.rules[0].inputs[0].format.is_none());
    }

    #[test]
    fn parse_input_array_with_non_string_item() {
        // An integer in the input array hits the `_ => None` branch
        // We test via the internal function directly for the edge case
        let val = toml::Value::Array(vec![
            toml::Value::String("a.txt".into()),
            toml::Value::Integer(42),
        ]);
        let inputs = parse_inputs(&Some(val));
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].pattern, "a.txt");
    }

    #[test]
    fn parse_input_unexpected_type() {
        // Integer value for input hits the `_ => Vec::new()` fallback
        let val = toml::Value::Integer(99);
        let inputs = parse_inputs(&Some(val));
        assert!(inputs.is_empty());
    }

    #[test]
    fn parse_output_single_string() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = "result.txt"
shell = "cat a.txt > result.txt"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].outputs.len(), 1);
        assert_eq!(wf.rules[0].outputs[0].pattern, "result.txt");
        assert!(wf.rules[0].outputs[0].name.is_none());
    }

    #[test]
    fn parse_output_unexpected_type() {
        // Integer value for output hits the `_ => Ok(Vec::new())` fallback
        let val = toml::Value::Integer(99);
        let outputs = parse_outputs(&Some(val)).unwrap();
        assert!(outputs.is_empty());
    }

    #[test]
    fn parse_output_array_with_non_string_item() {
        // Non-string/non-table item in output array
        let val = toml::Value::Array(vec![
            toml::Value::String("a.txt".into()),
            toml::Value::Integer(42),
        ]);
        let outputs = parse_outputs(&Some(val)).unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].pattern, "a.txt");
    }

    #[test]
    fn parse_output_table_item_with_all_fields() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
shell = "process a.txt"
output = [
    { path = "out.parquet", format = "parquet", name = "features", lifecycle = "temporary", materialize = "never" },
    { path = "report.csv", lifecycle = "protected", materialize = "final" },
    { path = "data.json", materialize = "auto" },
]
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].outputs.len(), 3);

        let o0 = &wf.rules[0].outputs[0];
        assert_eq!(o0.pattern, "out.parquet");
        assert_eq!(o0.format.as_deref(), Some("parquet"));
        assert_eq!(o0.name.as_deref(), Some("features"));
        assert_eq!(o0.lifecycle, OutputLifecycle::Temporary);
        assert_eq!(o0.materialize, MaterializePolicy::Never);

        let o1 = &wf.rules[0].outputs[1];
        assert_eq!(o1.pattern, "report.csv");
        assert_eq!(o1.lifecycle, OutputLifecycle::Protected);
        assert_eq!(o1.materialize, MaterializePolicy::Final);

        let o2 = &wf.rules[0].outputs[2];
        assert_eq!(o2.materialize, MaterializePolicy::Auto);
    }

    #[test]
    fn parse_output_named_table_preserves_names_and_patterns() {
        let toml = r#"
[rule.test]
shell = "process"

[rule.test.output]
counts = "results/{sample}.counts"
report = "results/{sample}.html"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].outputs.len(), 2);
        assert!(wf.rules[0].outputs.iter().any(|output| {
            output.name.as_deref() == Some("counts") && output.pattern == "results/{sample}.counts"
        }));
        assert!(wf.rules[0].outputs.iter().any(|output| {
            output.name.as_deref() == Some("report") && output.pattern == "results/{sample}.html"
        }));
    }

    #[test]
    fn parse_lifecycle_variants() {
        assert_eq!(
            parse_lifecycle("temporary").unwrap(),
            OutputLifecycle::Temporary
        );
        assert_eq!(
            parse_lifecycle("protected").unwrap(),
            OutputLifecycle::Protected
        );
        assert_eq!(
            parse_lifecycle("permanent").unwrap(),
            OutputLifecycle::Permanent
        );
    }

    #[test]
    fn parse_lifecycle_rejects_unknown() {
        let err = parse_lifecycle("temporarry").unwrap_err();
        assert!(matches!(err, ParseError::InvalidField { .. }));
        let msg = err.to_string();
        assert!(
            msg.contains("temporarry"),
            "error should contain the invalid value"
        );
        assert!(
            msg.contains("lifecycle"),
            "error should mention the field name"
        );
    }

    #[test]
    fn parse_materialize_policy_variants() {
        assert_eq!(
            parse_materialize_policy("auto").unwrap(),
            MaterializePolicy::Auto
        );
        assert_eq!(
            parse_materialize_policy("never").unwrap(),
            MaterializePolicy::Never
        );
        assert_eq!(
            parse_materialize_policy("final").unwrap(),
            MaterializePolicy::Final
        );
        assert_eq!(
            parse_materialize_policy("always").unwrap(),
            MaterializePolicy::Always
        );
    }

    #[test]
    fn parse_materialize_policy_rejects_unknown() {
        let err = parse_materialize_policy("alwayz").unwrap_err();
        assert!(matches!(err, ParseError::InvalidField { .. }));
        let msg = err.to_string();
        assert!(
            msg.contains("alwayz"),
            "error should contain the invalid value"
        );
        assert!(
            msg.contains("materialize"),
            "error should mention the field name"
        );
    }

    #[test]
    fn error_invalid_lifecycle_in_workflow() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
shell = "cat a.txt"
output = [
    { path = "out.txt", lifecycle = "temporarry" },
]
"#;
        let err = parse_workflow(toml, Path::new("test.toml")).unwrap_err();
        assert!(matches!(err, ParseError::InvalidField { .. }));
        assert!(err.to_string().contains("temporarry"));
    }

    #[test]
    fn error_invalid_materialize_in_workflow() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
shell = "cat a.txt"
output = [
    { path = "out.txt", materialize = "alwayz" },
]
"#;
        let err = parse_workflow(toml, Path::new("test.toml")).unwrap_err();
        assert!(matches!(err, ParseError::InvalidField { .. }));
        assert!(err.to_string().contains("alwayz"));
    }

    #[test]
    fn error_missing_execution() {
        // Rule with output but no execution mode
        let toml = r#"
[rule.bad]
output = ["b.txt"]
"#;
        let err = parse_workflow(toml, Path::new("test.toml")).unwrap_err();
        assert!(matches!(err, ParseError::MissingExecution { .. }));
    }

    #[test]
    fn parse_resource_float() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.resources]
ratio = 1.5
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].resources.get("ratio"),
            Some(&ResourceValue::Float(1.5.into()))
        );
    }

    #[test]
    fn parse_resource_fallback_boolean() {
        // Boolean resource hits the fallback `_ => ResourceValue::Str(v.to_string())`
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.resources]
flag = true
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].resources.get("flag"),
            Some(&ResourceValue::Str("true".to_string()))
        );
    }

    #[test]
    fn parse_environment_conda() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.environment]
conda = "myenv"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.rules[0].environment,
            Some(EnvSpec::Conda { ref env }) if env == "myenv"
        ));
    }

    #[test]
    fn parse_environment_conda_yaml_file() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.environment]
conda = "env.yaml"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.rules[0].environment,
            Some(EnvSpec::Conda { ref env }) if env == "env.yaml"
        ));
    }

    #[test]
    fn parse_environment_conda_yml_file() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.environment]
conda = "path/to/environment.yml"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.rules[0].environment,
            Some(EnvSpec::Conda { ref env }) if env == "path/to/environment.yml"
        ));
    }

    #[test]
    fn parse_environment_docker() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.environment]
docker = "python:3.11"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.rules[0].environment,
            Some(EnvSpec::Docker { ref image }) if image == "python:3.11"
        ));
    }

    #[test]
    fn parse_environment_nix() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.environment]
nix = "pkgs.python3"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.rules[0].environment,
            Some(EnvSpec::Nix { ref expr }) if expr == "pkgs.python3"
        ));
    }

    #[test]
    fn parse_environment_apptainer() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.environment]
apptainer = "image.sif"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            wf.rules[0].environment,
            Some(EnvSpec::Apptainer { ref image }) if image == "image.sif"
        ));
    }

    #[test]
    fn parse_environment_unknown_returns_none() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.environment]
unknown_env = "something"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(wf.rules[0].environment.is_none());
    }

    #[test]
    fn parse_expand_mode_zip() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"
expand = "zip"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].expand_mode, ExpandMode::Zip);
    }

    #[test]
    fn parse_error_strategy_terminate() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"
error_strategy = "terminate"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].error_strategy, ErrorStrategy::Terminate);
    }

    #[test]
    fn parse_error_strategy_finish() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"
error_strategy = "finish"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].error_strategy, ErrorStrategy::Finish);
    }

    #[test]
    fn parse_error_strategy_unknown_string() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"
error_strategy = "bogus"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].error_strategy, ErrorStrategy::default());
    }

    #[test]
    fn parse_error_strategy_retry_with_constant_backoff() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.error_strategy]
retry = 2
backoff = "constant"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].error_strategy,
            ErrorStrategy::Retry {
                count: 2,
                backoff: Backoff::Constant,
            }
        );
    }

    #[test]
    fn parse_error_strategy_retry_with_linear_backoff() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"

[rule.test.error_strategy]
retry = 5
backoff = "linear"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].error_strategy,
            ErrorStrategy::Retry {
                count: 5,
                backoff: Backoff::Linear,
            }
        );
    }

    #[test]
    fn parse_error_strategy_unexpected_type() {
        // Integer value for error_strategy hits `_ => ErrorStrategy::default()`
        let val = toml::Value::Integer(42);
        let strategy = parse_error_strategy(&Some(val)).unwrap();
        assert_eq!(strategy, ErrorStrategy::default());
    }

    // -- include expansion (H29) --------------------------------------------

    #[test]
    fn include_expands_rules_from_included_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lib.toml"),
            r#"
[config]
extra_key = "from-include"
shared_key = "include-value"

[rule.extra]
input = ["x.txt"]
output = ["y.txt"]
shell = "echo extra"
"#,
        )
        .unwrap();
        let root = dir.path().join("Oxymakefile.toml");
        std::fs::write(
            &root,
            r#"
include = ["lib.toml"]

[config]
shared_key = "root-value"

[rule.build]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo build"
"#,
        )
        .unwrap();

        let content = std::fs::read_to_string(&root).unwrap();
        let wf = parse_workflow(&content, &root).unwrap();

        // Both the root rule and the included rule must be present.
        let names: Vec<&str> = wf.rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"build"), "root rule present: {names:?}");
        assert!(names.contains(&"extra"), "included rule present: {names:?}");

        // Included config keys are visible; the including file wins on conflict.
        assert_eq!(
            wf.config.get("extra_key"),
            Some(&ConfigValue::Scalar("from-include".into()))
        );
        assert_eq!(
            wf.config.get("shared_key"),
            Some(&ConfigValue::Scalar("root-value".into()))
        );

        // The includes list is preserved as a record of what was expanded.
        assert_eq!(wf.includes.len(), 1);
    }

    #[test]
    fn include_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Oxymakefile.toml");
        std::fs::write(&root, "include = [\"nope.toml\"]\n").unwrap();

        let content = std::fs::read_to_string(&root).unwrap();
        let err = parse_workflow(&content, &root).unwrap_err();
        assert!(
            matches!(err, ParseError::IncludeNotFound { .. }),
            "expected IncludeNotFound, got: {err}"
        );
    }

    #[test]
    fn include_circular_errors() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.toml");
        let b = dir.path().join("b.toml");
        std::fs::write(&a, "include = [\"b.toml\"]\n").unwrap();
        std::fs::write(&b, "include = [\"a.toml\"]\n").unwrap();

        let content = std::fs::read_to_string(&a).unwrap();
        let err = parse_workflow(&content, &a).unwrap_err();
        assert!(
            matches!(err, ParseError::CircularInclude { .. }),
            "expected CircularInclude, got: {err}"
        );
    }

    #[test]
    fn include_duplicate_rule_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lib.toml"),
            r#"
[rule.build]
input = ["x.txt"]
output = ["y.txt"]
shell = "echo dup"
"#,
        )
        .unwrap();
        let root = dir.path().join("Oxymakefile.toml");
        std::fs::write(
            &root,
            r#"
include = ["lib.toml"]

[rule.build]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo build"
"#,
        )
        .unwrap();

        let content = std::fs::read_to_string(&root).unwrap();
        let err = parse_workflow(&content, &root).unwrap_err();
        assert!(
            matches!(err, ParseError::DuplicateRule { .. }),
            "expected DuplicateRule, got: {err}"
        );
    }

    #[test]
    fn include_nested_includes_expand_transitively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("leaf.toml"),
            r#"
[rule.leaf]
input = ["l.txt"]
output = ["m.txt"]
shell = "echo leaf"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("mid.toml"),
            r#"
include = ["leaf.toml"]

[rule.mid]
input = ["p.txt"]
output = ["q.txt"]
shell = "echo mid"
"#,
        )
        .unwrap();
        let root = dir.path().join("Oxymakefile.toml");
        std::fs::write(&root, "include = [\"mid.toml\"]\n").unwrap();

        let content = std::fs::read_to_string(&root).unwrap();
        let wf = parse_workflow(&content, &root).unwrap();
        let names: Vec<&str> = wf.rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"mid"), "{names:?}");
        assert!(names.contains(&"leaf"), "{names:?}");
    }

    #[test]
    fn include_profiles_in_included_file_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lib.toml"),
            r#"
[profile.ci]
jobs = 4
"#,
        )
        .unwrap();
        let root = dir.path().join("Oxymakefile.toml");
        std::fs::write(&root, "include = [\"lib.toml\"]\n").unwrap();

        let content = std::fs::read_to_string(&root).unwrap();
        let err = parse_workflow(&content, &root).unwrap_err();
        assert!(
            err.to_string().contains("profile"),
            "profiles in included files must be rejected, got: {err}"
        );
    }

    #[test]
    fn parse_no_ox_version() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(wf.ox_version.is_none());
    }

    #[test]
    fn parse_script_with_lang() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
script = "scripts/process.R"
lang = "R"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            &wf.rules[0].execution,
            ExecutionBlock::Script { path, lang }
            if path == &PathBuf::from("scripts/process.R") && lang.as_deref() == Some("R")
        ));
    }

    #[test]
    fn parse_call_default_lang() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
call = "mod:func"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            &wf.rules[0].execution,
            ExecutionBlock::Call { function, lang }
            if function == "mod:func" && lang == "python"
        ));
    }

    #[test]
    fn parse_run_default_lang() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
run = "print('hello')"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(matches!(
            &wf.rules[0].execution,
            ExecutionBlock::Run { code, lang }
            if code == "print('hello')" && lang == "python"
        ));
    }

    #[test]
    fn parse_rule_with_priority_and_description() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"
priority = 10
description = "A test rule"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].priority, Some(10));
        assert_eq!(wf.rules[0].meta.description.as_deref(), Some("A test rule"));
    }

    #[test]
    fn parse_rule_with_executor() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hi"
executor = "slurm"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].executor.as_deref(), Some("slurm"));
    }

    #[test]
    fn parse_input_named_map_with_non_string_value() {
        // Named map where value is not a string, hitting as_str().unwrap_or_default()
        let mut tbl = toml::map::Map::new();
        tbl.insert("count".to_string(), toml::Value::Integer(42));
        let val = toml::Value::Table(tbl);
        let inputs = parse_inputs(&Some(val));
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].pattern, "");
        assert_eq!(inputs[0].name.as_deref(), Some("count"));
    }

    // -- When clause parsing ------------------------------------------------

    #[test]
    fn parse_when_eq() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"

[rule.test.when]
op = "eq"
field = "sample"
value = "A"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].when,
            Some(GuardExpr::Eq {
                field: "sample".into(),
                value: "A".into(),
            })
        );
    }

    #[test]
    fn parse_when_in() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
when = { op = "in", field = "sample", values = ["A", "B", "C"] }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].when,
            Some(GuardExpr::In {
                field: "sample".into(),
                values: vec!["A".into(), "B".into(), "C".into()],
            })
        );
    }

    #[test]
    fn parse_when_not_in() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
when = { op = "not_in", field = "sample", values = ["X"] }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].when,
            Some(GuardExpr::NotIn {
                field: "sample".into(),
                values: vec!["X".into()],
            })
        );
    }

    #[test]
    fn parse_when_regex() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
when = { op = "regex", field = "sample", pattern = "^patient_.*" }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].when,
            Some(GuardExpr::Regex {
                field: "sample".into(),
                pattern: "^patient_.*".into(),
            })
        );
    }

    #[test]
    fn parse_when_config_eq() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
when = { op = "config_eq", key = "mode", value = "production" }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].when,
            Some(GuardExpr::ConfigEq {
                key: "mode".into(),
                value: "production".into(),
            })
        );
    }

    #[test]
    fn parse_when_env_set() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
when = { op = "env_set", var = "CI" }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].when,
            Some(GuardExpr::EnvSet { var: "CI".into() })
        );
    }

    #[test]
    fn parse_when_env_eq() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
when = { op = "env_eq", var = "STAGE", value = "prod" }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].when,
            Some(GuardExpr::EnvEq {
                var: "STAGE".into(),
                value: "prod".into(),
            })
        );
    }

    #[test]
    fn parse_when_file_exists() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
when = { op = "file_exists", path = "data/override.csv" }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].when,
            Some(GuardExpr::FileExists {
                path: "data/override.csv".into(),
            })
        );
    }

    #[test]
    fn parse_when_not() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"

[rule.test.when]
op = "not"

[rule.test.when.condition]
op = "env_set"
var = "SKIP"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].when,
            Some(GuardExpr::Not {
                condition: Box::new(GuardExpr::EnvSet { var: "SKIP".into() }),
            })
        );
    }

    #[test]
    fn parse_when_and() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"

[rule.test.when]
op = "and"
conditions = [
    { op = "env_set", var = "CI" },
    { op = "eq", field = "sample", value = "A" },
]
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(
            matches!(wf.rules[0].when, Some(GuardExpr::And { ref conditions }) if conditions.len() == 2)
        );
    }

    #[test]
    fn parse_when_or() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"

[rule.test.when]
op = "or"
conditions = [
    { op = "env_set", var = "CI" },
    { op = "env_set", var = "LOCAL" },
]
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(
            matches!(wf.rules[0].when, Some(GuardExpr::Or { ref conditions }) if conditions.len() == 2)
        );
    }

    #[test]
    fn parse_when_none() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(wf.rules[0].when.is_none());
    }

    #[test]
    fn parse_when_unknown_op() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
when = { op = "bogus" }
"#;
        let err = parse_workflow(toml, Path::new("test.toml"));
        assert!(err.is_err());
    }

    #[test]
    fn parse_when_not_eq() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
when = { op = "not_eq", field = "sample", value = "X" }
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].when,
            Some(GuardExpr::NotEq {
                field: "sample".into(),
                value: "X".into(),
            })
        );
    }

    // -----------------------------------------------------------------------
    // Tilde expansion tests
    // -----------------------------------------------------------------------

    #[test]
    fn expand_tilde_in_config_scalar() {
        let toml = r#"
ox_version = "0.1"

[config]
data_dir = "~/data/lab"

[rule.build]
input = ["src/main.rs"]
output = ["target/main"]
shell = "rustc {input} -o {output}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let home = std::env::var("HOME").unwrap();
        match wf.config.get("data_dir").unwrap() {
            ConfigValue::Scalar(s) => assert_eq!(s, &format!("{home}/data/lab")),
            other => panic!("expected Scalar, got {other:?}"),
        }
    }

    #[test]
    fn expand_tilde_in_input_output_paths() {
        let toml = r#"
[rule.test]
input = ["~/inputs/data.csv"]
output = ["~/outputs/result.csv"]
shell = "process {input} {output}"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            wf.rules[0].inputs[0].pattern,
            format!("{home}/inputs/data.csv")
        );
        assert_eq!(
            wf.rules[0].outputs[0].pattern,
            format!("{home}/outputs/result.csv")
        );
    }

    #[test]
    fn expand_tilde_in_config_list() {
        let toml = r#"
[config]
paths = ["~/a", "~/b", "relative/c"]

[rule.noop]
input = ["x"]
shell = "true"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let home = std::env::var("HOME").unwrap();
        match wf.config.get("paths").unwrap() {
            ConfigValue::List(items) => {
                assert_eq!(items[0], format!("{home}/a"));
                assert_eq!(items[1], format!("{home}/b"));
                assert_eq!(items[2], "relative/c");
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn expand_tilde_in_script_path() {
        let toml = r#"
[rule.run_script]
input = ["data.csv"]
output = ["out.csv"]
script = "~/scripts/process.py"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let home = std::env::var("HOME").unwrap();
        if let ExecutionBlock::Script { path, .. } = &wf.rules[0].execution {
            assert_eq!(path, &PathBuf::from(format!("{home}/scripts/process.py")));
        } else {
            panic!("expected Script execution block");
        }
    }

    #[test]
    fn expand_tilde_in_include_paths() {
        // The include path is tilde-expanded before resolution: the
        // IncludeNotFound error must carry the expanded path, not `~/…`.
        let toml = r#"
ox_version = "0.1"
include = ["~/.oxymake-test-nonexistent-9f3k/common.toml"]

[rule.noop]
input = ["x"]
shell = "true"
"#;
        let err = parse_workflow(toml, Path::new("test.toml")).unwrap_err();
        let home = std::env::var("HOME").unwrap();
        match err {
            ParseError::IncludeNotFound { path } => {
                assert_eq!(
                    path,
                    PathBuf::from(format!("{home}/.oxymake-test-nonexistent-9f3k/common.toml"))
                );
            }
            other => panic!("expected IncludeNotFound, got: {other}"),
        }
    }

    #[test]
    fn expand_tilde_bare() {
        // Just "~" with no trailing path should expand to $HOME
        assert_eq!(super::expand_tilde("~"), std::env::var("HOME").unwrap());
    }

    #[test]
    fn no_expand_tilde_in_middle() {
        // Tilde in the middle of a path should NOT be expanded
        assert_eq!(super::expand_tilde("foo/~/bar"), "foo/~/bar");
    }

    #[test]
    fn expand_tilde_in_log_paths() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"

[rule.test.log]
stdout = "~/logs/out.log"
stderr = "~/logs/err.log"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            wf.rules[0].log.stdout.as_deref(),
            Some(format!("{home}/logs/out.log").as_str())
        );
        assert_eq!(
            wf.rules[0].log.stderr.as_deref(),
            Some(format!("{home}/logs/err.log").as_str())
        );
    }

    #[test]
    fn expand_tilde_in_benchmark_path() {
        let toml = r#"
[rule.test]
input = ["a.txt"]
output = ["b.txt"]
shell = "cp a b"
benchmark = "~/benchmarks/test.tsv"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            wf.rules[0].benchmark.as_deref(),
            Some(format!("{home}/benchmarks/test.tsv").as_str())
        );
    }

    // -- Path handling edge cases (ox-58w3) ------------------------------------

    #[test]
    fn expand_tilde_with_spaces_in_path() {
        let result = expand_tilde("~/my project/data file.csv");
        let home = std::env::var("HOME").unwrap();
        assert_eq!(result, format!("{home}/my project/data file.csv"));
    }

    #[test]
    fn expand_tilde_with_unicode_path() {
        let result = expand_tilde("~/données/résultats.csv");
        let home = std::env::var("HOME").unwrap();
        assert_eq!(result, format!("{home}/données/résultats.csv"));
    }

    #[test]
    fn expand_tilde_path_with_spaces() {
        let p = PathBuf::from("~/my project/output");
        let result = expand_tilde_path(&p);
        let home = std::env::var("HOME").unwrap();
        assert_eq!(result, PathBuf::from(format!("{home}/my project/output")));
    }

    #[test]
    fn expand_tilde_path_with_unicode() {
        let p = PathBuf::from("~/données/日本語");
        let result = expand_tilde_path(&p);
        let home = std::env::var("HOME").unwrap();
        assert_eq!(result, PathBuf::from(format!("{home}/données/日本語")));
    }

    #[test]
    fn no_tilde_expansion_for_non_tilde_paths() {
        assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
        assert_eq!(expand_tilde("relative/path"), "relative/path");
        assert_eq!(expand_tilde("~other/path"), "~other/path");
    }

    #[test]
    fn parse_workflow_with_spaces_in_paths() {
        let toml = r#"
[rule.process]
input = ["data dir/input file.csv"]
output = ["out put/result file.csv"]
shell = "cp 'data dir/input file.csv' 'out put/result file.csv'"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].inputs[0].pattern, "data dir/input file.csv");
        assert_eq!(wf.rules[0].outputs[0].pattern, "out put/result file.csv");
    }

    #[test]
    fn parse_workflow_with_unicode_in_paths() {
        let toml = r#"
[rule.process]
input = ["données/entrée.csv"]
output = ["résultats/sortie.csv"]
shell = "process données/entrée.csv résultats/sortie.csv"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.rules[0].inputs[0].pattern, "données/entrée.csv");
        assert_eq!(wf.rules[0].outputs[0].pattern, "résultats/sortie.csv");
    }

    #[test]
    fn param_files_parsed_from_toml() {
        let toml = r#"
[rule.optimizer]
output = ["model.bin"]
shell = "train --config config/optimizer.yaml"
param_files = ["config/optimizer.yaml", "config/hyperparams.json"]
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].param_files,
            vec!["config/optimizer.yaml", "config/hyperparams.json"]
        );
    }

    #[test]
    fn param_files_defaults_to_empty() {
        let toml = r#"
[rule.simple]
output = ["out.txt"]
shell = "echo hello > out.txt"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(wf.rules[0].param_files.is_empty());
    }

    #[test]
    fn parse_params_converts_scalar_types_to_strings() {
        let toml = r#"
[rule.optimizer]
output = ["model.bin"]
shell = "train"

[rule.optimizer.params]
name = "baseline"
epochs = 42
learning_rate = 0.125
enabled = true
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(
            wf.rules[0].params,
            BTreeMap::from([
                ("enabled".into(), "true".into()),
                ("epochs".into(), "42".into()),
                ("learning_rate".into(), "0.125".into()),
                ("name".into(), "baseline".into()),
            ])
        );
    }

    #[test]
    fn parse_rule_reproducibility_classes() {
        for (value, expected) in [
            ("deterministic", ReproducibilityClass::Deterministic),
            (
                "seed_deterministic",
                ReproducibilityClass::SeedDeterministic,
            ),
            ("approximate", ReproducibilityClass::Approximate),
            ("non_reproducible", ReproducibilityClass::NonReproducible),
        ] {
            let toml = format!(
                "[rule.test]\noutput = [\"out.txt\"]\nshell = \"echo hi\"\nreproducibility = \"{value}\"\n"
            );
            let wf = parse_workflow(&toml, Path::new("test.toml")).unwrap();
            assert_eq!(wf.rules[0].reproducibility, expected, "{value}");
        }
    }

    // -- Profile parsing tests -------------------------------------------------

    #[test]
    fn parse_profile_basic() {
        let toml = r#"
[profile.ci]
cache_validation = "hash"
jobs = 4

[profile.dev]
cache_validation = "mtime"
jobs = 1
verbose = true

[rule.build]
input = ["src/main.rs"]
output = ["target/main"]
shell = "echo build"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.profiles.len(), 2);

        let ci = &wf.profiles["ci"];
        assert_eq!(ci.jobs, Some(4));
        assert_eq!(ci.cache_validation.as_deref(), Some("hash"));
        assert_eq!(ci.verbose, None);

        let dev = &wf.profiles["dev"];
        assert_eq!(dev.jobs, Some(1));
        assert_eq!(dev.cache_validation.as_deref(), Some("mtime"));
        assert_eq!(dev.verbose, Some(1));
    }

    #[test]
    fn parse_profile_all_fields() {
        let toml = r#"
[profile.full]
jobs = 8
cache_validation = "hash"
verbose = 2
executor = "slurm"
no_cache = true
keep_going = true
partition = "gpu"
account = "research"
qos = "high"

[profile.full.set]
genome = "hg38"
samples = "A,B,C"

[rule.build]
input = ["src/main.rs"]
output = ["target/main"]
shell = "echo build"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        let p = &wf.profiles["full"];
        assert_eq!(p.jobs, Some(8));
        assert_eq!(p.cache_validation.as_deref(), Some("hash"));
        assert_eq!(p.verbose, Some(2));
        assert_eq!(p.executor.as_deref(), Some("slurm"));
        assert_eq!(p.no_cache, Some(true));
        assert_eq!(p.keep_going, Some(true));
        assert_eq!(p.partition.as_deref(), Some("gpu"));
        assert_eq!(p.account.as_deref(), Some("research"));
        assert_eq!(p.qos.as_deref(), Some("high"));
        assert_eq!(p.set.get("genome").unwrap(), "hg38");
        assert_eq!(p.set.get("samples").unwrap(), "A,B,C");
    }

    #[test]
    fn parse_no_profiles() {
        let toml = r#"
[rule.build]
input = ["src/main.rs"]
output = ["target/main"]
shell = "echo build"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert!(wf.profiles.is_empty());
    }

    #[test]
    fn parse_profile_verbose_integer() {
        let toml = r#"
[profile.debug]
verbose = 3

[rule.build]
input = ["a"]
output = ["b"]
shell = "echo"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.profiles["debug"].verbose, Some(3));
    }

    #[test]
    fn parse_profile_verbose_false() {
        let toml = r#"
[profile.quiet]
verbose = false

[rule.build]
input = ["a"]
output = ["b"]
shell = "echo"
"#;
        let wf = parse_workflow(toml, Path::new("test.toml")).unwrap();
        assert_eq!(wf.profiles["quiet"].verbose, Some(0));
    }

    // ── Property-based tests ─────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for a valid TOML identifier (rule/config name).
        fn ident() -> impl Strategy<Value = String> {
            "[a-z][a-z0-9_]{0,7}"
        }

        /// Strategy for a file-like pattern string.
        fn file_pattern() -> impl Strategy<Value = String> {
            "[a-z][a-z0-9_/.-]{0,20}"
        }

        /// Strategy for a shell command string.
        fn shell_cmd() -> impl Strategy<Value = String> {
            "[a-z][a-z0-9 _./|-]{0,30}"
        }

        /// Strategy for a single well-formed rule in TOML syntax.
        fn arb_rule_toml(name: String) -> impl Strategy<Value = String> {
            let inputs = proptest::collection::vec(file_pattern(), 0..=3);
            let outputs = proptest::collection::vec(file_pattern(), 1..=3);
            let cmd = shell_cmd();
            (inputs, outputs, cmd).prop_map(move |(inp, out, cmd)| {
                let inp_str: Vec<String> = inp.iter().map(|s| format!("\"{}\"", s)).collect();
                let out_str: Vec<String> = out.iter().map(|s| format!("\"{}\"", s)).collect();
                format!(
                    "[rule.{}]\ninput = [{}]\noutput = [{}]\nshell = \"{}\"\n",
                    name,
                    inp_str.join(", "),
                    out_str.join(", "),
                    cmd,
                )
            })
        }

        /// Strategy for a valid Oxymakefile with 1-4 rules.
        fn arb_oxymakefile() -> impl Strategy<Value = String> {
            proptest::collection::vec(ident(), 1..=4)
                .prop_filter("rule names must be unique", |names| {
                    let set: std::collections::HashSet<_> = names.iter().collect();
                    set.len() == names.len()
                })
                .prop_flat_map(|names| {
                    let rule_strats: Vec<_> = names.into_iter().map(|n| arb_rule_toml(n)).collect();
                    rule_strats
                })
                .prop_map(|rules| rules.join("\n"))
        }

        proptest! {
            /// parse_workflow must never panic on arbitrary string input.
            ///
            /// It may return Ok or Err, but must not abort.
            #[test]
            fn parse_never_panics_on_arbitrary_input(input in "\\PC{0,500}") {
                let _ = parse_workflow(&input, Path::new("fuzz.toml"));
            }

            /// parse_workflow must never panic on arbitrary valid TOML that
            /// doesn't match the Oxymakefile schema.
            #[test]
            fn parse_never_panics_on_random_toml(
                key in ident(),
                value in "[a-zA-Z0-9_ ]{0,20}",
            ) {
                let toml = format!("{key} = \"{value}\"");
                let _ = parse_workflow(&toml, Path::new("fuzz.toml"));
            }

            /// A well-formed Oxymakefile with random rules parses successfully,
            /// and the number of parsed rules matches what was generated.
            #[test]
            fn valid_oxymakefile_parses(toml in arb_oxymakefile()) {
                let result = parse_workflow(&toml, Path::new("gen.toml"));
                prop_assert!(
                    result.is_ok(),
                    "valid TOML should parse: {:?}\nInput:\n{}",
                    result.err(),
                    toml,
                );
                let wf = result.unwrap();
                prop_assert!(!wf.rules.is_empty(), "should have at least one rule");
            }

            /// Every parsed rule has at least one output pattern.
            #[test]
            fn parsed_rules_have_outputs(toml in arb_oxymakefile()) {
                if let Ok(wf) = parse_workflow(&toml, Path::new("gen.toml")) {
                    for rule in &wf.rules {
                        prop_assert!(
                            !rule.outputs.is_empty(),
                            "rule '{}' should have at least one output",
                            rule.name.as_str(),
                        );
                    }
                }
            }

            /// Parsing is deterministic: the same input always yields the same
            /// rules (same names in the same order).
            #[test]
            fn parse_is_deterministic(toml in arb_oxymakefile()) {
                let r1 = parse_workflow(&toml, Path::new("a.toml"));
                let r2 = parse_workflow(&toml, Path::new("a.toml"));
                match (r1, r2) {
                    (Ok(w1), Ok(w2)) => {
                        let names1: Vec<&str> =
                            w1.rules.iter().map(|r| r.name.as_str()).collect();
                        let names2: Vec<&str> =
                            w2.rules.iter().map(|r| r.name.as_str()).collect();
                        prop_assert_eq!(names1, names2, "parse must be deterministic");
                    }
                    (Err(_), Err(_)) => {} // both fail is fine
                    _ => prop_assert!(false, "determinism violation: one Ok, one Err"),
                }
            }
        }
    }
}
