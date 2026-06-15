//! Session builder — fluent API for configuring an OxyMake build session.
//!
//! # Example
//!
//! ```no_run
//! use ox_api::SessionBuilder;
//!
//! let session = SessionBuilder::new("Oxymakefile.toml")
//!     .targets(["results/A.bam", "results/B.bam"])
//!     .config_override("genome", "hg38")
//!     .build()
//!     .unwrap();
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ox_core::job_graph::JobGraph;
use ox_core::model::ResourceValue;
use ox_core::resolver::{Config, ResolveRequest};
use ox_format::parse::{ConfigValue, Workflow};

use crate::error::ApiError;

/// A fully-configured, ready-to-inspect build session.
///
/// Holds the parsed workflow, resolved job graph, and configuration.
/// Use [`SessionBuilder`] to construct one.
pub struct Session {
    /// The original parsed workflow.
    pub workflow: Workflow,
    /// The resolved and dependency-ordered job graph.
    pub job_graph: JobGraph,
    /// Resolver config used to expand wildcards.
    pub config: Config,
    /// The path to the Oxymakefile that was loaded.
    pub file_path: PathBuf,
}

/// Fluent builder for configuring a build session.
pub struct SessionBuilder {
    file_path: PathBuf,
    targets: Vec<String>,
    overrides: BTreeMap<String, String>,
    list_overrides: BTreeMap<String, Vec<String>>,
}

impl SessionBuilder {
    /// Create a builder that will load the given Oxymakefile.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            file_path: path.into(),
            targets: Vec::new(),
            overrides: BTreeMap::new(),
            list_overrides: BTreeMap::new(),
        }
    }

    /// Set explicit build targets (output file paths).
    ///
    /// If not called, targets are inferred from the workflow (the `all` rule
    /// or the first rule's outputs).
    pub fn targets(mut self, targets: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.targets = targets.into_iter().map(Into::into).collect();
        self
    }

    /// Override a scalar config value (e.g. `genome=hg38`).
    pub fn config_override(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.overrides.insert(key.into(), value.into());
        self
    }

    /// Override a list config value (e.g. `samples=["A","B","C"]`).
    pub fn config_list_override(
        mut self,
        key: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.list_overrides
            .insert(key.into(), values.into_iter().map(Into::into).collect());
        self
    }

    /// Parse, validate, resolve, and build the job graph.
    ///
    /// Returns a [`Session`] ready for inspection or execution.
    pub fn build(self) -> Result<Session, ApiError> {
        // 1. Load & parse.
        let workflow = load_workflow(&self.file_path)?;

        // 2. Validate.
        ox_format::validate::validate(&workflow).map_err(ApiError::Validation)?;

        // 3. Build resolver config from the workflow's [config] section.
        let mut config = ox_format::targets::workflow_config(&workflow);

        // 4. Apply user overrides.
        for (key, value) in &self.overrides {
            config.scalars.insert(key.clone(), value.clone());
            if config.lists.contains_key(key) {
                config.lists.insert(key.clone(), vec![value.clone()]);
            }
        }
        for (key, values) in &self.list_overrides {
            config.lists.insert(key.clone(), values.clone());
        }

        // 5. Determine targets — shared implementation with the CLI and
        // MCP surfaces, including {config.X} substitution (H34). Overrides
        // applied above must be visible to expansion, so substitute against
        // the final `config`, not the workflow's raw [config] section.
        let targets = if self.targets.is_empty() {
            resolve_default_targets(&workflow, &config)
        } else {
            self.targets
                .iter()
                .map(|t| ox_format::targets::substitute_config_refs(t, &config.scalars))
                .collect()
        };

        // 6. Discover existing source files (cached).
        let existing_files = crate::discover::discover_existing_files(&self.file_path);

        // 7. Resolve to concrete jobs.
        let request = ResolveRequest {
            targets,
            config: config.clone(),
            existing_files,
        };
        let result = ox_core::resolver::resolve(&workflow.rules, &request)?;

        // 8. Build job graph.
        let job_graph = JobGraph::build(result.jobs)?;

        Ok(Session {
            workflow,
            job_graph,
            config,
            file_path: self.file_path,
        })
    }
}

// ---------------------------------------------------------------------------
// Internal helpers (ported from ox-cli common.rs to avoid a dependency on
// ox-cli, which carries clap and other CLI-only deps).
// ---------------------------------------------------------------------------

/// Read and parse an Oxymakefile, resolving config interpolation in resources.
fn load_workflow(path: &Path) -> Result<Workflow, ApiError> {
    let content = std::fs::read_to_string(path).map_err(|e| ApiError::ReadFile {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut workflow = ox_format::parse::parse_workflow(&content, path)?;

    // Resolve {config.X} references in resource values.
    let scalars: BTreeMap<String, String> = workflow
        .config
        .iter()
        .filter_map(|(k, v)| match v {
            ConfigValue::Scalar(s) => Some((k.clone(), s.clone())),
            ConfigValue::List(l) if l.len() == 1 => Some((k.clone(), l[0].clone())),
            _ => None,
        })
        .collect();

    for rule in &mut workflow.rules {
        for rv in rule.resources.values_mut() {
            if let ResourceValue::Str(s) = rv {
                let mut resolved = s.clone();
                for (ck, cv) in &scalars {
                    resolved = resolved.replace(&format!("{{config.{}}}", ck), cv);
                }
                if resolved != *s {
                    if let Ok(n) = resolved.parse::<i64>() {
                        *rv = ResourceValue::Int(n);
                    } else if let Ok(f) = resolved.parse::<f64>() {
                        *rv = ResourceValue::Float(f.into());
                    } else {
                        *s = resolved;
                    }
                }
            }
        }
    }

    Ok(workflow)
}

/// Infer default targets from the workflow when none are specified.
///
/// Takes the override-adjusted `config` (unlike the parameterless shared
/// `resolve_targets`) so that `config_override`/`config_list_override`
/// affect expansion. Pattern expansion itself is the shared implementation
/// from `ox_format::targets` (H34).
fn resolve_default_targets(workflow: &Workflow, config: &Config) -> Vec<String> {
    let default_rule = workflow
        .rules
        .iter()
        .find(|r| r.name.as_str() == "all")
        .or_else(|| workflow.rules.first());

    let Some(rule) = default_rule else {
        return Vec::new();
    };

    let mut targets = Vec::new();
    if rule.outputs.is_empty() {
        // Aggregation rule — expand inputs as targets.
        for input in &rule.inputs {
            ox_format::targets::expand_pattern(&input.pattern, config, &mut targets);
        }
    } else {
        for output in &rule.outputs {
            ox_format::targets::expand_pattern(&output.pattern, config, &mut targets);
        }
    }
    targets
}
