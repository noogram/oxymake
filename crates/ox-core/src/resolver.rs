//! Backward-chaining wildcard resolver for OxyMake.
//!
//! # Overview
//!
//! The resolver is the bridge between the *declarative* world of rules and the
//! *concrete* world of jobs.  It takes a set of [`Rule`]s (with wildcard
//! patterns in their inputs and outputs), a list of target paths to produce,
//! and configuration values, then backward-chains through the rules to produce
//! an ordered list of [`ConcreteJob`]s with every wildcard bound to a concrete
//! value.
//!
//! # The backward-chaining algorithm
//!
//! The algorithm works like Prolog-style goal resolution, starting from the
//! desired outputs and working backwards toward source files:
//!
//! 1. **Goal selection** -- The user requests one or more targets (concrete
//!    file paths or pattern strings like `"results/sample_A.bam"`).
//!
//! 2. **Producer search** -- For each target, the resolver scans all rules to
//!    find one whose output pattern matches the target path.  Matching uses
//!    [`Pattern::resolve`] to extract wildcard values.  If zero rules match,
//!    the target is either a source file (exists on disk) or an error.  If
//!    multiple rules match, the one with the highest `priority` wins, or an
//!    `AmbiguousProducer` error is raised.
//!
//! 3. **Guard evaluation** -- If the matched rule has a `when` guard, the
//!    resolver evaluates it against the extracted wildcard values and the
//!    configuration.  If the guard is false, the rule is skipped and the
//!    search continues with remaining candidates.
//!
//! 4. **Instantiation** -- The rule's input patterns are interpolated with the
//!    extracted wildcard values, producing concrete input paths.
//!
//! 5. **Recursive descent** -- Each concrete input path becomes a new goal.
//!    If it exists on disk, it is recorded as a source file and the recursion
//!    stops.  Otherwise, steps 2-5 repeat for that input.
//!
//! 6. **Cycle detection** -- The resolver tracks the set of targets currently
//!    being resolved.  If a target appears in its own dependency chain, a
//!    `CycleDetected` error is raised.
//!
//! 7. **Assembly** -- Once all goals are resolved, the collected jobs are
//!    returned in dependency order (topological sort).
//!
//! # How wildcards flow from targets to concrete values
//!
//! When the resolver matches `"results/patient_42.bam"` against the pattern
//! `"results/{sample}.bam"`, it extracts `{sample} = "patient_42"`.  These
//! values are then substituted into the rule's input patterns.  If the rule
//! has `inputs = ["data/{sample}.fastq"]`, the concrete input becomes
//! `"data/patient_42.fastq"`.  This is how wildcard values propagate through
//! the entire chain without the user specifying them explicitly.
//!
//! # How guards filter job creation
//!
//! Guards are boolean conditions on wildcard values.  For example:
//!
//! ```text
//! when = { op = "in", field = "sample", values = ["A", "B", "C"] }
//! ```
//!
//! This means the rule only applies when `{sample}` is one of `"A"`, `"B"`,
//! or `"C"`.  The resolver evaluates guards after extracting wildcard values
//! from pattern matching.  If the guard is false, the rule is not used for
//! that particular target, and the resolver tries other candidate rules.
//!
//! # How aggregation rules expand from config
//!
//! An aggregation rule (one with no outputs, like a `phony` target in Make)
//! lists wildcard patterns in its inputs but has no output pattern to match
//! against.  The resolver cannot extract wildcard values from pattern matching
//! in this case, so it expands them from configuration lists.
//!
//! For example, if the `all` rule has inputs `["results/{sample}.bam"]` and
//! the config defines `lists.sample = ["A", "B", "C"]`, the aggregation
//! expands to three concrete inputs: `results/A.bam`, `results/B.bam`,
//! `results/C.bam`.  Each of these then becomes a goal for backward chaining.
//!
//! # Example
//!
//! Consider these rules:
//!
//! ```text
//! [align]
//! inputs = ["data/{sample}.fastq"]
//! outputs = ["results/{sample}.bam"]
//! shell = "bwa mem ref.fa data/{sample}.fastq > results/{sample}.bam"
//!
//! [sort]
//! inputs = ["results/{sample}.bam"]
//! outputs = ["results/{sample}.sorted.bam"]
//! shell = "samtools sort results/{sample}.bam > results/{sample}.sorted.bam"
//! ```
//!
//! Requesting target `"results/patient_42.sorted.bam"`:
//!
//! 1. Match `sort` output pattern: `{sample} = "patient_42"`
//! 2. Instantiate `sort` input: `"results/patient_42.bam"`
//! 3. Recurse -- match `align` output pattern: `{sample} = "patient_42"`
//! 4. Instantiate `align` input: `"data/patient_42.fastq"`
//! 5. `"data/patient_42.fastq"` exists on disk -- source file, stop
//! 6. Return: `[align(sample=patient_42), sort(sample=patient_42)]`

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

use regex::Regex;

use crate::error::{DagError, WildcardError};
use crate::model::*;
use crate::wildcard::{CompiledPattern, Pattern, Wildcards};

/// Maximum depth of the backward-chaining recursion in [`resolve`].
///
/// The resolver recurses once per link of a dependency chain; without a
/// bound, a sufficiently deep chain overflows the thread stack (SIGSEGV).
/// 1 000 is far beyond any realistic pipeline depth while staying well
/// inside the default stack budget.
const MAX_RESOLVE_DEPTH: usize = 1_000;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration values from the `[config]` section of the Oxymakefile.
///
/// The config provides named lists of values that are used for:
/// - Aggregation rule expansion (when wildcards cannot be inferred from outputs)
/// - Guard evaluation (checking wildcard membership in named lists)
///
/// ```
/// use ox_core::resolver::Config;
///
/// let mut config = Config::default();
/// config.lists.insert("sample".into(), vec!["A".into(), "B".into()]);
/// assert_eq!(config.lists["sample"], vec!["A", "B"]);
/// ```
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Named lists of values for wildcard expansion.
    pub lists: BTreeMap<String, Vec<String>>,
    /// Named scalar values for guard expression evaluation.
    pub scalars: BTreeMap<String, String>,
}

/// A request to resolve specific targets.
///
/// ```
/// use ox_core::resolver::{Config, ResolveRequest};
///
/// let request = ResolveRequest {
///     targets: vec!["results/A.bam".into()],
///     config: Config::default(),
///     existing_files: vec!["data/A.fastq".into()],
/// };
/// assert_eq!(request.targets.len(), 1);
/// ```
#[derive(Debug, Clone)]
pub struct ResolveRequest {
    /// Target file paths or patterns to resolve.
    pub targets: Vec<String>,
    /// Config values for wildcard expansion.
    pub config: Config,
    /// Files that already exist on disk (source files).
    pub existing_files: Vec<PathBuf>,
}

/// Result of resolution: concrete jobs needed to produce targets.
///
/// Jobs are ordered so that dependencies come before dependents (topological
/// order).  Source files are inputs that exist on disk and have no producing
/// rule.
#[derive(Debug, Clone)]
pub struct ResolveResult {
    /// All concrete jobs needed, in dependency order.
    pub jobs: Vec<ConcreteJob>,
    /// Source files (inputs that exist and no rule produces).
    pub sources: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// A pre-compiled entry for a single rule output pattern.
struct ProducerEntry {
    /// Index into the rules slice.
    rule_idx: usize,
    /// Pre-compiled pattern with regex ready for matching.
    compiled: CompiledPattern,
}

/// Index of all rule output patterns, pre-compiled for fast repeated lookup.
///
/// Built once at the start of resolution. Eliminates per-target regex
/// compilation, which is the dominant cost at scale (50K+ targets).
struct ProducerIndex {
    entries: Vec<ProducerEntry>,
}

impl ProducerIndex {
    /// Build the index by parsing and compiling all output patterns upfront.
    ///
    /// `{config.X}` references in output patterns are substituted with scalar
    /// values before parsing, so they become literal path fragments.
    fn build(
        rules: &[Rule],
        config_scalars: &BTreeMap<String, String>,
    ) -> Result<Self, WildcardError> {
        let mut entries = Vec::new();
        for (rule_idx, rule) in rules.iter().enumerate() {
            for output_pat in &rule.outputs {
                let expanded = substitute_config_scalars(&output_pat.pattern, config_scalars)?;
                let pattern = Pattern::parse(&expanded)?;
                let compiled = CompiledPattern::new(pattern, &rule.wildcard_constraints)?;
                entries.push(ProducerEntry { rule_idx, compiled });
            }
        }
        Ok(ProducerIndex { entries })
    }

    /// Find the rule whose output pattern matches a given target path.
    ///
    /// Uses pre-compiled regexes so each call is a simple regex match
    /// (no parsing or compilation). Returns `(rule, wildcards, rule_index)`.
    fn find_producer<'a>(
        &self,
        rules: &'a [Rule],
        target: &str,
    ) -> Result<(&'a Rule, Wildcards, usize), WildcardError> {
        let mut candidates: Vec<(usize, &Rule, Wildcards)> = Vec::new();
        let mut last_rule_idx = usize::MAX;

        for entry in &self.entries {
            // Skip if we already matched this rule (one match per rule is enough).
            if entry.rule_idx == last_rule_idx {
                continue;
            }
            if let Some(wc) = entry.compiled.resolve(target) {
                candidates.push((entry.rule_idx, &rules[entry.rule_idx], wc));
                last_rule_idx = entry.rule_idx;
            }
        }

        match candidates.len() {
            0 => Err(WildcardError::NoProducer {
                path: target.to_owned(),
            }),
            1 => {
                let (idx, rule, wc) = candidates.remove(0);
                Ok((rule, wc, idx))
            }
            _ => {
                // Try to resolve by priority.
                candidates.sort_by(|a, b| {
                    let pa = a.1.priority.unwrap_or(0);
                    let pb = b.1.priority.unwrap_or(0);
                    pb.cmp(&pa)
                });
                let best_priority = candidates[0].1.priority.unwrap_or(0);
                let second_priority = candidates[1].1.priority.unwrap_or(0);
                if best_priority > second_priority {
                    let (idx, rule, wc) = candidates.remove(0);
                    Ok((rule, wc, idx))
                } else {
                    Err(WildcardError::AmbiguousProducer {
                        path: target.to_owned(),
                        rules: candidates
                            .iter()
                            .map(|(_, r, _)| r.name.0.clone())
                            .collect(),
                    })
                }
            }
        }
    }
}

/// Pre-compiled constraint regex, reused across all `validate_constraints` calls.
struct CompiledConstraint {
    name: String,
    regex: Regex,
}

/// Tracks the state of the backward-chaining resolution process.
struct ResolveState<'a> {
    rules: &'a [Rule],
    config: &'a Config,
    existing: HashSet<PathBuf>,
    /// Jobs accumulated in dependency order.
    jobs: Vec<ConcreteJob>,
    /// Set of output paths already produced by accumulated jobs (dedup).
    produced: HashSet<String>,
    /// Source files discovered during resolution.
    sources: BTreeSet<PathBuf>,
    /// Set of targets currently being resolved (cycle detection).
    resolving: HashSet<String>,
    /// Pre-compiled producer index for fast target-to-rule lookup.
    producer_index: ProducerIndex,
    /// Pre-compiled constraint regexes per rule (keyed by rule index).
    compiled_constraints: Vec<Vec<CompiledConstraint>>,
    /// Cached parsed input patterns per rule (keyed by rule index, then input index).
    parsed_input_patterns: Vec<Vec<Pattern>>,
    /// Cached parsed output patterns per rule (keyed by rule index, then output index).
    parsed_output_patterns: Vec<Vec<Pattern>>,
}

impl<'a> ResolveState<'a> {
    fn new(
        rules: &'a [Rule],
        config: &'a Config,
        existing_files: &[PathBuf],
    ) -> Result<Self, DagError> {
        let producer_index = ProducerIndex::build(rules, &config.scalars)?;

        // Pre-compile constraint regexes for each rule.
        let mut compiled_constraints = Vec::with_capacity(rules.len());
        for rule in rules {
            let mut rule_constraints = Vec::new();
            for (name, constraint) in &rule.wildcard_constraints {
                let regex = Regex::new(&format!("^(?:{constraint})$")).map_err(|_| {
                    WildcardError::InvalidPattern {
                        pattern: rule.name.0.clone(),
                        reason: format!(
                            "invalid regex constraint `{constraint}` for wildcard `{name}`"
                        ),
                    }
                })?;
                rule_constraints.push(CompiledConstraint {
                    name: name.clone(),
                    regex,
                });
            }
            compiled_constraints.push(rule_constraints);
        }

        // Pre-parse all input and output patterns.
        // Substitute `{config.X}` references before parsing so that config-derived
        // path fragments become literal text.
        let mut parsed_input_patterns = Vec::with_capacity(rules.len());
        let mut parsed_output_patterns = Vec::with_capacity(rules.len());
        for rule in rules {
            let inputs: Vec<Pattern> = rule
                .inputs
                .iter()
                .map(|ip| {
                    let expanded = substitute_config_scalars(&ip.pattern, &config.scalars)?;
                    Pattern::parse(&expanded)
                })
                .collect::<Result<_, _>>()?;
            parsed_input_patterns.push(inputs);

            let outputs: Vec<Pattern> = rule
                .outputs
                .iter()
                .map(|op| {
                    let expanded = substitute_config_scalars(&op.pattern, &config.scalars)?;
                    Pattern::parse(&expanded)
                })
                .collect::<Result<_, _>>()?;
            parsed_output_patterns.push(outputs);
        }

        Ok(Self {
            rules,
            config,
            existing: existing_files.iter().cloned().collect(),
            jobs: Vec::new(),
            produced: HashSet::new(),
            sources: BTreeSet::new(),
            resolving: HashSet::new(),
            producer_index,
            compiled_constraints,
            parsed_input_patterns,
            parsed_output_patterns,
        })
    }

    /// Resolve a single target path, recursing into its dependencies.
    fn resolve_target(&mut self, target: &str) -> Result<(), DagError> {
        // Already produced by a job we've scheduled -- nothing to do.
        if self.produced.contains(target) {
            return Ok(());
        }

        // Bound the recursion depth: `resolving` holds exactly the targets
        // on the current resolution stack, so its size is the depth. Without
        // this guard a deep dependency chain overflows the thread stack
        // (SIGSEGV) instead of reporting a usable error.
        if self.resolving.len() >= MAX_RESOLVE_DEPTH {
            return Err(DagError::DependencyChainTooDeep {
                target: target.to_owned(),
                limit: MAX_RESOLVE_DEPTH,
            });
        }

        // Is this a source file that exists on disk?
        let target_path = PathBuf::from(target);
        if self.existing.contains(&target_path) {
            self.sources.insert(target_path);
            return Ok(());
        }

        // Cycle detection.
        if self.resolving.contains(target) {
            let cycle: Vec<String> = self.resolving.iter().cloned().collect();
            return Err(DagError::CycleDetected { cycle });
        }
        self.resolving.insert(target.to_owned());

        // Find a producing rule using the pre-compiled index.
        let (rule, wildcards, rule_idx) =
            match self.producer_index.find_producer(self.rules, target) {
                Ok(found) => found,
                Err(WildcardError::NoProducer { .. }) => {
                    // No rule produces this target.  Check if it exists on disk
                    // as a source file (like Make and Snakemake do).
                    let path = std::path::Path::new(target);
                    if path.exists() {
                        self.sources.insert(PathBuf::from(target));
                        self.resolving.remove(target);
                        return Ok(());
                    }
                    return Err(DagError::Wildcard(WildcardError::MissingSource {
                        path: target.to_owned(),
                    }));
                }
                Err(e) => return Err(DagError::Wildcard(e)),
            };

        // Evaluate guard (if any).  If the guard fails, this producer is
        // rejected.  In the current implementation we treat this as a
        // NoProducer error because we've already filtered to the best
        // candidate.  A more sophisticated version would try the next
        // candidate.
        if let Some(ref guard) = rule.when {
            if !evaluate_guard(guard, &wildcards, self.config) {
                self.resolving.remove(target);
                return Err(DagError::Wildcard(WildcardError::NoProducer {
                    path: target.to_owned(),
                }));
            }
        }

        // Resolve input paths by interpolating wildcards.
        // If an input has wildcards not bound by the output match, expand from config.
        let mut concrete_inputs = Vec::new();
        for (input_idx, input_pat) in rule.inputs.iter().enumerate() {
            let pat = &self.parsed_input_patterns[rule_idx][input_idx];
            match pat.interpolate(&wildcards) {
                Ok(concrete_path) => {
                    concrete_inputs.push((concrete_path, input_pat));
                }
                Err(WildcardError::UnresolvableWildcard { .. }) => {
                    // Input has wildcards not resolved by the output match.
                    // Expand from config lists (Snakemake expand() semantics).
                    let unresolved_names: Vec<String> = pat
                        .wildcard_names()
                        .into_iter()
                        .filter(|n| !wildcards.contains_key(*n))
                        .map(|n| n.to_string())
                        .collect();
                    let mut lists: Vec<(&String, &Vec<String>)> = Vec::new();
                    for name in &unresolved_names {
                        let values = self
                            .config
                            .lists
                            .get(name)
                            .or_else(|| self.config.lists.get(&format!("{}s", name)))
                            .ok_or_else(|| WildcardError::UnresolvableWildcard {
                                name: name.clone(),
                            })?;
                        lists.push((name, values));
                    }
                    let combos = match rule.expand_mode {
                        ExpandMode::Product => cartesian_product(&lists),
                        ExpandMode::Zip => zip_lists(&lists)?,
                    };
                    let compiled_constraints = &self.compiled_constraints[rule_idx];
                    for combo in combos {
                        let mut merged = wildcards.clone();
                        for (k, v) in &combo {
                            merged.insert(k.clone(), v.clone());
                        }
                        // Validate expanded wildcards against pre-compiled constraints.
                        if !compiled_constraints.is_empty() {
                            validate_constraints_compiled(&merged, compiled_constraints)?;
                        }
                        let concrete_path = pat.interpolate(&merged)?;
                        concrete_inputs.push((concrete_path, input_pat));
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }

        // Recurse into each input.
        for (input_path, _) in &concrete_inputs {
            self.resolve_target(input_path)?;
        }

        // Build resolved inputs.
        let resolved_inputs: Vec<ResolvedInput> = concrete_inputs
            .iter()
            .map(|(path, ip)| ResolvedInput {
                reference: OutputRef::File(PathBuf::from(path)),
                name: ip.name.clone(),
                format: ip.format.clone(),
            })
            .collect();

        // Build resolved outputs using cached patterns.
        let mut resolved_outputs = Vec::new();
        for (output_idx, output_pat) in rule.outputs.iter().enumerate() {
            let pat = &self.parsed_output_patterns[rule_idx][output_idx];
            let concrete_path = pat.interpolate(&wildcards)?;
            self.produced.insert(concrete_path.clone());
            resolved_outputs.push(ResolvedOutput {
                reference: OutputRef::File(PathBuf::from(concrete_path)),
                name: output_pat.name.clone(),
                format: output_pat.format.clone(),
                lifecycle: output_pat.lifecycle,
                materialize: output_pat.materialize,
            });
        }

        // Collect concrete input/output paths (with names) for interpolation.
        let named_inputs: Vec<NamedPath> = concrete_inputs
            .iter()
            .map(|(path, ip)| (path.clone(), ip.name.clone()))
            .collect();
        let named_outputs: Vec<NamedPath> = resolved_outputs
            .iter()
            .map(|o| {
                let path = match &o.reference {
                    OutputRef::File(p) => p.to_string_lossy().to_string(),
                    OutputRef::Virtual { id, .. } => id.clone(),
                    OutputRef::InMemory { .. } => String::new(),
                };
                (path, o.name.clone())
            })
            .collect();

        // Interpolate log paths with wildcards.
        let log = LogConfig {
            stdout: rule
                .log
                .stdout
                .as_ref()
                .map(|p| interpolate_simple(p, &wildcards)),
            stderr: rule
                .log
                .stderr
                .as_ref()
                .map(|p| interpolate_simple(p, &wildcards)),
        };

        // Interpolate benchmark path with wildcards.
        let benchmark = rule
            .benchmark
            .as_ref()
            .map(|p| interpolate_simple(p, &wildcards));

        // Interpolate param values with wildcards.
        let params: BTreeMap<String, String> = rule
            .params
            .iter()
            .map(|(k, v)| (k.clone(), interpolate_simple(v, &wildcards)))
            .collect();

        // Interpolate parameter file paths with wildcards.
        let param_files: Vec<PathBuf> = rule
            .param_files
            .iter()
            .map(|p| PathBuf::from(interpolate_simple(p, &wildcards)))
            .collect();

        // Interpolate the execution block (wildcards + {input}/{output}/{params}
        // + {config}/{log}/{threads}).
        let execution = interpolate_execution(
            &rule.execution,
            &wildcards,
            &named_inputs,
            &named_outputs,
            &params,
            &log,
            &rule.resources,
            &self.config.scalars,
        )?;

        // Build tags: explicit rule tags + wildcard values as implicit tags.
        let mut tags = rule.tags.clone();
        for (k, v) in &wildcards {
            tags.entry(k.clone()).or_insert_with(|| v.clone());
        }

        // Build the job ID from rule name + wildcard values.
        let job_id = build_job_id(&rule.name, &wildcards);

        let job = ConcreteJob {
            id: job_id,
            rule: rule.name.clone(),
            wildcards: wildcards.clone(),
            tags,
            inputs: resolved_inputs,
            outputs: resolved_outputs,
            execution,
            resources: rule.resources.clone(),
            environment: rule.environment.clone(),
            error_strategy: rule.error_strategy.clone(),
            timeout: rule.timeout,
            executor: rule.executor.clone(),
            priority: rule.priority,
            benchmark,
            params,
            param_files,
            log,
            shell_executable: rule.shell_executable.clone(),
            reproducibility: rule.reproducibility,
        };

        self.jobs.push(job);
        self.resolving.remove(target);

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve targets by backward-chaining through rules.
///
/// Starting from the requested targets, find which rules produce them,
/// extract wildcard values, recurse into their inputs, and build a
/// complete set of [`ConcreteJob`]s.
///
/// # Algorithm
///
/// 1. For each target, find the rule whose output pattern matches
/// 2. Extract wildcard values from the match
/// 3. Evaluate `when` guard (if present) -- skip if false
/// 4. Instantiate the rule into a ConcreteJob
/// 5. For each input of the job:
///    - If it exists on disk, it is a source file
///    - If not, recurse (find a rule that produces it)
/// 6. Return all jobs in dependency order
///
/// # Examples
///
/// ```
/// use ox_core::resolver::*;
/// use ox_core::model::*;
/// use std::collections::BTreeMap;
///
/// // A single rule: align reads to produce a BAM file.
/// let rule = Rule {
///     name: RuleName::from("align"),
///     priority: None,
///     inputs: vec![InputPattern {
///         pattern: "data/{sample}.fastq".into(),
///         name: None,
///         format: None,
///     }],
///     outputs: vec![OutputPattern {
///         pattern: "results/{sample}.bam".into(),
///         name: None,
///         format: None,
///         lifecycle: OutputLifecycle::Permanent,
///         materialize: MaterializePolicy::Always,
///     }],
///     execution: ExecutionBlock::Shell {
///         command: "bwa mem ref.fa data/{sample}.fastq > results/{sample}.bam".into(),
///     },
///     resources: BTreeMap::new(),
///     environment: None,
///     tags: BTreeMap::new(),
///     meta: RuleMeta { description: None },
///     wildcard_constraints: BTreeMap::new(),
///     when: None,
///     expand_mode: ExpandMode::Product,
///     error_strategy: ErrorStrategy::Terminate,
///     timeout: None,
///     executor: None,
///     log: LogConfig::default(),
///     benchmark: None,
///     retries: None,
///     params: BTreeMap::new(),
///     param_files: Vec::new(),
///     shell_executable: None,
///     reproducibility: ReproducibilityClass::default(),
///     source_line: None,
/// };
///
/// let request = ResolveRequest {
///     targets: vec!["results/patient_42.bam".into()],
///     config: Config::default(),
///     existing_files: vec!["data/patient_42.fastq".into()],
/// };
///
/// let result = resolve(&[rule], &request).unwrap();
/// assert_eq!(result.jobs.len(), 1);
/// assert_eq!(result.jobs[0].rule, RuleName::from("align"));
/// assert_eq!(result.jobs[0].wildcards["sample"], "patient_42");
/// assert_eq!(result.sources.len(), 1);
/// ```
pub fn resolve(rules: &[Rule], request: &ResolveRequest) -> Result<ResolveResult, DagError> {
    let mut state = ResolveState::new(rules, &request.config, &request.existing_files)?;

    for target in &request.targets {
        state.resolve_target(target)?;
    }

    Ok(ResolveResult {
        jobs: state.jobs,
        sources: state.sources.into_iter().collect(),
    })
}

/// Expand an aggregation target's wildcards from config.
///
/// When a rule has wildcards in its inputs but no outputs (an aggregation or
/// phony target), the wildcards cannot be extracted from pattern matching.
/// Instead, they are expanded from config lists.  This function computes the
/// Cartesian product (or zip, depending on `expand_mode`) of all wildcard
/// lists and returns the fully interpolated input paths.
///
/// # Examples
///
/// ```
/// use ox_core::resolver::*;
/// use ox_core::model::*;
/// use std::collections::BTreeMap;
///
/// let rule = Rule {
///     name: RuleName::from("all"),
///     priority: None,
///     inputs: vec![InputPattern {
///         pattern: "results/{sample}.bam".into(),
///         name: None,
///         format: None,
///     }],
///     outputs: vec![],
///     execution: ExecutionBlock::Shell { command: "echo done".into() },
///     resources: BTreeMap::new(),
///     environment: None,
///     tags: BTreeMap::new(),
///     meta: RuleMeta { description: None },
///     wildcard_constraints: BTreeMap::new(),
///     when: None,
///     expand_mode: ExpandMode::Product,
///     error_strategy: ErrorStrategy::Terminate,
///     timeout: None,
///     executor: None,
///     log: LogConfig::default(),
///     benchmark: None,
///     retries: None,
///     params: BTreeMap::new(),
///     param_files: Vec::new(),
///     shell_executable: None,
///     reproducibility: ReproducibilityClass::default(),
///     source_line: None,
/// };
///
/// let mut config = Config::default();
/// config.lists.insert("sample".into(), vec!["A".into(), "B".into(), "C".into()]);
///
/// let expanded = expand_aggregation(&rule, &config).unwrap();
/// assert_eq!(expanded, vec!["results/A.bam", "results/B.bam", "results/C.bam"]);
/// ```
pub fn expand_aggregation(rule: &Rule, config: &Config) -> Result<Vec<String>, WildcardError> {
    // Collect all wildcard names from all input patterns.
    // Substitute `{config.X}` references first.
    let mut all_names: Vec<String> = Vec::new();
    for input in &rule.inputs {
        let expanded = substitute_config_scalars(&input.pattern, &config.scalars)?;
        let pat = Pattern::parse(&expanded)?;
        for name in pat.wildcard_names() {
            if !all_names.contains(&name.to_owned()) {
                all_names.push(name.to_owned());
            }
        }
    }

    if all_names.is_empty() {
        // No wildcards -- just return the config-substituted literal input paths.
        return rule
            .inputs
            .iter()
            .map(|i| substitute_config_scalars(&i.pattern, &config.scalars))
            .collect::<Result<_, _>>();
    }

    // Look up each wildcard in config.lists.
    let mut lists: Vec<(&String, &Vec<String>)> = Vec::new();
    for name in &all_names {
        let values = config
            .lists
            .get(name)
            .ok_or_else(|| WildcardError::UnresolvableWildcard { name: name.clone() })?;
        lists.push((name, values));
    }

    // Generate combinations based on expand_mode.
    let combos = match rule.expand_mode {
        ExpandMode::Product => cartesian_product(&lists),
        ExpandMode::Zip => zip_lists(&lists)?,
    };

    // Validate each combination against wildcard constraints.
    if !rule.wildcard_constraints.is_empty() {
        for combo in &combos {
            validate_constraints(combo, &rule.wildcard_constraints, &rule.name.0)?;
        }
    }

    // Interpolate each input pattern for each combination.
    let mut result = Vec::new();
    for combo in &combos {
        for input in &rule.inputs {
            let expanded = substitute_config_scalars(&input.pattern, &config.scalars)?;
            let pat = Pattern::parse(&expanded)?;
            let path = pat.interpolate(combo)?;
            result.push(path);
        }
    }

    Ok(result)
}

/// Evaluate a guard expression against concrete wildcard values.
///
/// Returns `true` if the guard condition is satisfied, `false` otherwise.
///
/// # Examples
///
/// ```
/// use ox_core::resolver::*;
/// use ox_core::model::GuardExpr;
/// use ox_core::wildcard::Wildcards;
///
/// let guard = GuardExpr::Eq {
///     field: "sample".into(),
///     value: "A".into(),
/// };
/// let mut wc = Wildcards::new();
/// wc.insert("sample".into(), "A".into());
///
/// assert!(evaluate_guard(&guard, &wc, &Config::default()));
///
/// wc.insert("sample".into(), "B".into());
/// assert!(!evaluate_guard(&guard, &wc, &Config::default()));
/// ```
pub fn evaluate_guard(guard: &GuardExpr, wildcards: &Wildcards, config: &Config) -> bool {
    match guard {
        GuardExpr::In { field, values } => {
            let wc_value = match wildcards.get(field) {
                Some(v) => v,
                None => return false,
            };
            // If a value starts with '@', look it up in config.lists.
            // Otherwise, use the literal values list.
            let effective_values: Vec<&String> = values
                .iter()
                .flat_map(|v| {
                    if let Some(list_name) = v.strip_prefix('@') {
                        config
                            .lists
                            .get(list_name)
                            .map(|l| l.iter().collect::<Vec<_>>())
                            .unwrap_or_default()
                    } else {
                        vec![v]
                    }
                })
                .collect();
            effective_values.contains(&wc_value)
        }
        GuardExpr::NotIn { field, values } => {
            let wc_value = match wildcards.get(field) {
                Some(v) => v,
                None => return true,
            };
            let effective_values: Vec<&String> = values
                .iter()
                .flat_map(|v| {
                    if let Some(list_name) = v.strip_prefix('@') {
                        config
                            .lists
                            .get(list_name)
                            .map(|l| l.iter().collect::<Vec<_>>())
                            .unwrap_or_default()
                    } else {
                        vec![v]
                    }
                })
                .collect();
            !effective_values.contains(&wc_value)
        }
        GuardExpr::Eq { field, value } => wildcards.get(field).map(|v| v == value).unwrap_or(false),
        GuardExpr::NotEq { field, value } => {
            wildcards.get(field).map(|v| v != value).unwrap_or(true)
        }
        GuardExpr::Regex { field, pattern } => {
            let wc_value = match wildcards.get(field) {
                Some(v) => v,
                None => return false,
            };
            Regex::new(pattern)
                .map(|re| re.is_match(wc_value))
                .unwrap_or(false)
        }
        GuardExpr::ConfigEq { key, value } => {
            config.scalars.get(key).map(|v| v == value).unwrap_or(false)
        }
        GuardExpr::EnvSet { var } => std::env::var(var).map(|v| !v.is_empty()).unwrap_or(false),
        GuardExpr::EnvEq { var, value } => std::env::var(var).map(|v| v == *value).unwrap_or(false),
        GuardExpr::FileExists { path } => std::path::Path::new(path).exists(),
        GuardExpr::And { conditions } => conditions
            .iter()
            .all(|c| evaluate_guard(c, wildcards, config)),
        GuardExpr::Or { conditions } => conditions
            .iter()
            .any(|c| evaluate_guard(c, wildcards, config)),
        GuardExpr::Not { condition } => !evaluate_guard(condition, wildcards, config),
    }
}

/// Find the rule whose output pattern matches a given target path.
///
/// When multiple rules match, the one with the highest `priority` wins.
/// If priorities are equal or both `None`, an `AmbiguousProducer` error
/// is raised.
///
/// # Examples
///
/// ```
/// use ox_core::resolver::find_producer;
/// use ox_core::model::*;
/// use std::collections::BTreeMap;
///
/// let rule = Rule {
///     name: RuleName::from("align"),
///     priority: None,
///     inputs: vec![],
///     outputs: vec![OutputPattern {
///         pattern: "results/{sample}.bam".into(),
///         name: None,
///         format: None,
///         lifecycle: OutputLifecycle::Permanent,
///         materialize: MaterializePolicy::Always,
///     }],
///     execution: ExecutionBlock::Shell { command: "echo".into() },
///     resources: BTreeMap::new(),
///     environment: None,
///     tags: BTreeMap::new(),
///     meta: RuleMeta { description: None },
///     wildcard_constraints: BTreeMap::new(),
///     when: None,
///     expand_mode: ExpandMode::Product,
///     error_strategy: ErrorStrategy::Terminate,
///     timeout: None,
///     executor: None,
///     log: LogConfig::default(),
///     benchmark: None,
///     retries: None,
///     params: BTreeMap::new(),
///     param_files: Vec::new(),
///     shell_executable: None,
///     reproducibility: ReproducibilityClass::default(),
///     source_line: None,
/// };
///
/// let rules = [rule];
/// let (found, wc) = find_producer(&rules, "results/patient_42.bam").unwrap();
/// assert_eq!(found.name, RuleName::from("align"));
/// assert_eq!(wc["sample"], "patient_42");
/// ```
pub fn find_producer<'a>(
    rules: &'a [Rule],
    target: &str,
) -> Result<(&'a Rule, Wildcards), WildcardError> {
    let mut candidates: Vec<(&Rule, Wildcards)> = Vec::new();

    for rule in rules {
        for output_pat in &rule.outputs {
            let pat = Pattern::parse(&output_pat.pattern)?;
            // Use wildcard constraints to narrow pattern matching.
            if let Some(wc) = pat.resolve_with_constraints(target, &rule.wildcard_constraints)? {
                candidates.push((rule, wc));
                break; // one match per rule is enough
            }
        }
    }

    match candidates.len() {
        0 => Err(WildcardError::NoProducer {
            path: target.to_owned(),
        }),
        1 => Ok(candidates.remove(0)),
        _ => {
            // Try to resolve by priority.
            candidates.sort_by(|a, b| {
                let pa = a.0.priority.unwrap_or(0);
                let pb = b.0.priority.unwrap_or(0);
                pb.cmp(&pa) // highest first
            });
            let best_priority = candidates[0].0.priority.unwrap_or(0);
            let second_priority = candidates[1].0.priority.unwrap_or(0);
            if best_priority > second_priority {
                Ok(candidates.remove(0))
            } else {
                Err(WildcardError::AmbiguousProducer {
                    path: target.to_owned(),
                    rules: candidates.iter().map(|(r, _)| r.name.0.clone()).collect(),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// A named path entry for interpolation: (path, optional_name).
type NamedPath = (String, Option<String>);

/// Interpolate wildcards AND {input}/{output}/{params}/{config} into an execution block.
#[allow(clippy::too_many_arguments)]
fn interpolate_execution(
    exec: &ExecutionBlock,
    wildcards: &Wildcards,
    named_inputs: &[NamedPath],
    named_outputs: &[NamedPath],
    params: &BTreeMap<String, String>,
    log: &LogConfig,
    resources: &BTreeMap<String, ResourceValue>,
    config_scalars: &BTreeMap<String, String>,
) -> Result<ExecutionBlock, WildcardError> {
    match exec {
        ExecutionBlock::Shell { command } => {
            let interpolated = interpolate_full(
                command,
                wildcards,
                named_inputs,
                named_outputs,
                params,
                log,
                resources,
                config_scalars,
            );
            Ok(ExecutionBlock::Shell {
                command: interpolated,
            })
        }
        ExecutionBlock::Run { code, lang } => {
            let interpolated = interpolate_full(
                code,
                wildcards,
                named_inputs,
                named_outputs,
                params,
                log,
                resources,
                config_scalars,
            );
            Ok(ExecutionBlock::Run {
                code: interpolated,
                lang: lang.clone(),
            })
        }
        ExecutionBlock::Script { path, lang } => {
            let interpolated = interpolate_full(
                &path.to_string_lossy(),
                wildcards,
                named_inputs,
                named_outputs,
                params,
                log,
                resources,
                config_scalars,
            );
            Ok(ExecutionBlock::Script {
                path: PathBuf::from(interpolated),
                lang: lang.clone(),
            })
        }
        ExecutionBlock::Call { function, lang } => {
            let interpolated = interpolate_full(
                function,
                wildcards,
                named_inputs,
                named_outputs,
                params,
                log,
                resources,
                config_scalars,
            );
            Ok(ExecutionBlock::Call {
                function: interpolated,
                lang: lang.clone(),
            })
        }
    }
}

/// Substitute `{config.key}` references in a pattern string with config scalar values.
///
/// This runs **before** wildcard pattern parsing so that config-derived path
/// fragments become literal text before the pattern parser sees them.
///
/// Returns the substituted string, or an error if a referenced config key
/// is not defined in `[config]`.
fn substitute_config_scalars(
    pattern: &str,
    scalars: &BTreeMap<String, String>,
) -> Result<String, WildcardError> {
    // Fast path: no config references at all.
    if !pattern.contains("{config.") {
        return Ok(pattern.to_owned());
    }

    let mut result = String::with_capacity(pattern.len());
    let mut rest = pattern;

    while let Some(start) = rest.find("{config.") {
        result.push_str(&rest[..start]);

        // Find the closing brace.
        let after_open = &rest[start + 1..]; // skip the `{`
        if let Some(close) = after_open.find('}') {
            let placeholder = &after_open[..close]; // e.g. "config.results_dir"
            let key = &placeholder["config.".len()..]; // e.g. "results_dir"
            let value = scalars
                .get(key)
                .ok_or_else(|| WildcardError::UnknownConfigKey {
                    key: key.to_owned(),
                    pattern: pattern.to_owned(),
                })?;
            result.push_str(value);
            rest = &rest[start + 1 + close + 1..]; // skip past `}`
        } else {
            // No closing brace — let the wildcard parser report the error.
            result.push_str(&rest[start..]);
            break;
        }
    }
    result.push_str(rest);

    Ok(result)
}

/// Simple wildcard interpolation for paths (log, benchmark, param values).
/// Replaces `{name}` placeholders with wildcard values.
fn interpolate_simple(template: &str, wildcards: &Wildcards) -> String {
    let mut result = template.to_owned();
    for (name, value) in wildcards {
        result = result.replace(&format!("{{{name}}}"), value);
    }
    result
}

/// Full string interpolation: replace wildcards, {input}, {output},
/// {input[N]}, {output[N]}, {input.name}, {output.name}, {wildcards.X},
/// {params.X}, {config.X}, {log}, {threads}, and {resources.X}.
#[allow(clippy::too_many_arguments)]
fn interpolate_full(
    template: &str,
    wildcards: &Wildcards,
    named_inputs: &[NamedPath],
    named_outputs: &[NamedPath],
    params: &BTreeMap<String, String>,
    log: &LogConfig,
    resources: &BTreeMap<String, ResourceValue>,
    config_scalars: &BTreeMap<String, String>,
) -> String {
    let mut result = template.to_owned();

    let input_paths: Vec<&str> = named_inputs.iter().map(|(p, _)| p.as_str()).collect();
    let output_paths: Vec<&str> = named_outputs.iter().map(|(p, _)| p.as_str()).collect();

    // Replace {input.name} and {output.name} (named access) first — most specific.
    // Group by name so that expand-generated inputs with the same name are
    // concatenated with spaces (e.g. {input.csvs} → "a.csv b.csv c.csv").
    {
        let mut by_name: std::collections::BTreeMap<&str, Vec<&str>> =
            std::collections::BTreeMap::new();
        for (path, name) in named_inputs {
            if let Some(name) = name {
                by_name
                    .entry(name.as_str())
                    .or_default()
                    .push(path.as_str());
            }
        }
        for (name, paths) in &by_name {
            result = result.replace(&format!("{{input.{name}}}"), &paths.join(" "));
        }
    }
    {
        let mut by_name: std::collections::BTreeMap<&str, Vec<&str>> =
            std::collections::BTreeMap::new();
        for (path, name) in named_outputs {
            if let Some(name) = name {
                by_name
                    .entry(name.as_str())
                    .or_default()
                    .push(path.as_str());
            }
        }
        for (name, paths) in &by_name {
            result = result.replace(&format!("{{output.{name}}}"), &paths.join(" "));
        }
    }

    // Replace {input[N]} and {output[N]} (indexed access).
    for (i, path) in input_paths.iter().enumerate() {
        result = result.replace(&format!("{{input[{i}]}}"), path);
    }
    for (i, path) in output_paths.iter().enumerate() {
        result = result.replace(&format!("{{output[{i}]}}"), path);
    }

    // Replace {wildcards.X} (explicit wildcard namespace — Snakemake convention).
    for (name, value) in wildcards {
        result = result.replace(&format!("{{wildcards.{name}}}"), value);
    }

    // Replace {params.X} (named parameters).
    for (name, value) in params {
        result = result.replace(&format!("{{params.{name}}}"), value);
    }

    // Replace {config.X} (config scalar values).
    for (name, value) in config_scalars {
        result = result.replace(&format!("{{config.{name}}}"), value);
    }

    // Replace {log} with the log stdout path (Snakemake convention).
    if let Some(ref log_path) = log.stdout {
        result = result.replace("{log}", log_path);
    }

    // Replace {threads} with the cpu resource value (Snakemake convention).
    if let Some(cpu) = resources.get("cpu") {
        result = result.replace("{threads}", &cpu.to_string());
    }

    // Replace {resources.X} (named resource access).
    for (name, value) in resources {
        result = result.replace(&format!("{{resources.{name}}}"), &value.to_string());
    }

    // Replace {input} (all inputs space-separated) and {output} (all outputs).
    result = result.replace("{input}", &input_paths.join(" "));
    result = result.replace("{output}", &output_paths.join(" "));

    // Replace wildcard placeholders: {name} -> value.
    for (name, value) in wildcards {
        let placeholder = format!("{{{name}}}");
        result = result.replace(&placeholder, value);
    }

    result
}

/// Build a job ID from a rule name and wildcard values.
///
/// The id must be *injective* in the wildcard values for a given rule:
/// a naive `-`-join made `{a:"x-y", b:"z"}` and `{a:"x", b:"y-z"}` collide
/// (both `r-x-y-z`), silently shadowing one job. Values are therefore
/// percent-escaped (`%` → `%25`, `-` → `%2D`) before joining, so the `-`
/// separator can never occur inside an encoded value. Values without
/// dashes — the common case — keep the historical readable format.
///
/// Cross-rule collisions (a rule name that ends like another rule's id
/// prefix) are caught by [`JobGraph::build`]'s `DuplicateJobId` check.
///
/// [`JobGraph::build`]: crate::job_graph::JobGraph::build
fn build_job_id(rule_name: &RuleName, wildcards: &Wildcards) -> JobId {
    if wildcards.is_empty() {
        return JobId::from(rule_name.0.as_str());
    }
    let wc_part: Vec<String> = wildcards
        .values()
        .map(|v| v.replace('%', "%25").replace('-', "%2D"))
        .collect();
    JobId::from(format!("{}-{}", rule_name.0, wc_part.join("-")))
}

/// Compute the Cartesian product of named value lists.
fn cartesian_product(lists: &[(&String, &Vec<String>)]) -> Vec<Wildcards> {
    if lists.is_empty() {
        return vec![Wildcards::new()];
    }

    let mut result = vec![Wildcards::new()];
    for (name, values) in lists {
        let mut new_result = Vec::new();
        for combo in &result {
            for value in *values {
                let mut new_combo = combo.clone();
                new_combo.insert((*name).clone(), value.clone());
                new_result.push(new_combo);
            }
        }
        result = new_result;
    }
    result
}

/// Zip named value lists (all must have equal length).
fn zip_lists(lists: &[(&String, &Vec<String>)]) -> Result<Vec<Wildcards>, WildcardError> {
    if lists.is_empty() {
        return Ok(vec![Wildcards::new()]);
    }

    let len = lists[0].1.len();
    for (name, values) in lists.iter().skip(1) {
        if values.len() != len {
            return Err(WildcardError::InvalidPattern {
                pattern: format!("zip({}, {})", lists[0].0, name),
                reason: format!(
                    "zip requires equal-length lists, but `{}` has {} values and `{}` has {}",
                    lists[0].0,
                    len,
                    name,
                    values.len()
                ),
            });
        }
    }

    let mut result = Vec::new();
    for i in 0..len {
        let mut combo = Wildcards::new();
        for (name, values) in lists {
            combo.insert((*name).clone(), values[i].clone());
        }
        result.push(combo);
    }
    Ok(result)
}

/// Validate wildcards against pre-compiled constraint regexes.
///
/// This is the fast path used during resolution — regexes are compiled once
/// at `ResolveState` construction and reused for every wildcard expansion.
fn validate_constraints_compiled(
    wildcards: &Wildcards,
    compiled: &[CompiledConstraint],
) -> Result<(), WildcardError> {
    for cc in compiled {
        if let Some(value) = wildcards.get(&cc.name) {
            if !cc.regex.is_match(value) {
                return Err(WildcardError::ConstraintViolation {
                    name: cc.name.clone(),
                    value: value.clone(),
                    pattern: String::new(),
                    constraint: cc.regex.as_str().to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// Validate that all wildcard values satisfy their constraints.
///
/// Each entry in `constraints` maps a wildcard name to a regex pattern.
/// If a wildcard value does not fully match its constraint, a
/// `ConstraintViolation` error is returned.
fn validate_constraints(
    wildcards: &Wildcards,
    constraints: &BTreeMap<String, String>,
    context_pattern: &str,
) -> Result<(), WildcardError> {
    for (name, constraint) in constraints {
        if let Some(value) = wildcards.get(name) {
            let re = Regex::new(&format!("^(?:{constraint})$")).map_err(|_| {
                WildcardError::InvalidPattern {
                    pattern: context_pattern.to_owned(),
                    reason: format!(
                        "invalid regex constraint `{constraint}` for wildcard `{name}`"
                    ),
                }
            })?;
            if !re.is_match(value) {
                return Err(WildcardError::ConstraintViolation {
                    name: name.clone(),
                    value: value.clone(),
                    pattern: context_pattern.to_owned(),
                    constraint: constraint.clone(),
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // -- Test helpers --------------------------------------------------------

    /// Create a minimal rule with the given name, input patterns, and output
    /// patterns.  Uses shell execution with a placeholder command.
    fn make_rule(name: &str, inputs: &[&str], outputs: &[&str]) -> Rule {
        Rule {
            name: RuleName::from(name),
            priority: None,
            inputs: inputs
                .iter()
                .map(|p| InputPattern {
                    pattern: (*p).to_string().into(),
                    name: None,
                    format: None,
                })
                .collect(),
            outputs: outputs
                .iter()
                .map(|p| OutputPattern {
                    pattern: (*p).to_string().into(),
                    name: None,
                    format: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                })
                .collect(),
            execution: ExecutionBlock::Shell {
                command: format!("echo {name}"),
            },
            resources: BTreeMap::new(),
            environment: None,
            tags: BTreeMap::new(),
            meta: RuleMeta { description: None },
            wildcard_constraints: BTreeMap::new(),
            when: None,
            expand_mode: ExpandMode::Product,
            error_strategy: ErrorStrategy::Terminate,
            timeout: None,
            executor: None,
            log: LogConfig::default(),
            benchmark: None,
            retries: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
            source_line: None,
        }
    }

    fn make_request(targets: &[&str], existing: &[&str]) -> ResolveRequest {
        ResolveRequest {
            targets: targets.iter().map(|s| s.to_string()).collect(),
            config: Config::default(),
            existing_files: existing.iter().map(PathBuf::from).collect(),
        }
    }

    // -- Depth limit (H3) ----------------------------------------------------

    #[test]
    fn deep_dependency_chain_yields_clean_error() {
        // H3: an unbounded recursion in resolve_target turned a deep chain
        // into a stack overflow (SIGSEGV). Chains beyond MAX_RESOLVE_DEPTH
        // must instead fail with a clean, typed error.
        //
        // Run on a thread with a production-sized stack (16 MiB): debug
        // frames are fat, and the point here is the depth *guard*, not the
        // test harness's 2 MiB thread stack.
        let handle = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let n = MAX_RESOLVE_DEPTH + 100;
                let mut rules = Vec::with_capacity(n);
                for i in 1..=n {
                    let input = format!("f{}.txt", i - 1);
                    let output = format!("f{i}.txt");
                    rules.push(make_rule(
                        &format!("r{i}"),
                        &[input.as_str()],
                        &[output.as_str()],
                    ));
                }
                let request = make_request(&[&format!("f{n}.txt")], &["f0.txt"]);
                resolve(&rules, &request).unwrap_err()
            })
            .unwrap();
        let err = handle.join().unwrap();
        assert!(
            matches!(err, DagError::DependencyChainTooDeep { .. }),
            "expected DependencyChainTooDeep, got: {err:?}"
        );
        assert!(err.to_string().contains("depth"), "got: {err}");
    }

    // -- Simple linear chain ------------------------------------------------

    #[test]
    fn simple_linear_chain() {
        let align = make_rule("align", &["data/{sample}.fastq"], &["aligned/{sample}.bam"]);
        let sort = make_rule("sort", &["aligned/{sample}.bam"], &["sorted/{sample}.bam"]);

        let request = make_request(&["sorted/A.bam"], &["data/A.fastq"]);

        let result = resolve(&[align, sort], &request).unwrap();
        assert_eq!(result.jobs.len(), 2);
        assert_eq!(result.jobs[0].rule, RuleName::from("align"));
        assert_eq!(result.jobs[0].wildcards["sample"], "A");
        assert_eq!(result.jobs[1].rule, RuleName::from("sort"));
        assert_eq!(result.jobs[1].wildcards["sample"], "A");
        assert_eq!(result.sources, vec![PathBuf::from("data/A.fastq")]);
    }

    // -- Multi-wildcard resolution -----------------------------------------

    #[test]
    fn multi_wildcard_resolution() {
        let rule = make_rule(
            "process",
            &["data/{sample}/{method}.input"],
            &["results/{sample}/{method}.output"],
        );

        let request = make_request(
            &["results/patient_1/bwa.output"],
            &["data/patient_1/bwa.input"],
        );

        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].wildcards["sample"], "patient_1");
        assert_eq!(result.jobs[0].wildcards["method"], "bwa");
    }

    // -- Aggregation target expansion (product) ----------------------------

    #[test]
    fn aggregation_expansion_product() {
        let rule = make_rule("all", &["results/{sample}.bam"], &[]);
        let mut config = Config::default();
        config
            .lists
            .insert("sample".into(), vec!["A".into(), "B".into(), "C".into()]);

        let expanded = expand_aggregation(&rule, &config).unwrap();
        assert_eq!(
            expanded,
            vec!["results/A.bam", "results/B.bam", "results/C.bam",]
        );
    }

    #[test]
    fn aggregation_expansion_multi_wildcard_product() {
        let rule = make_rule("all", &["results/{sample}/{method}.bam"], &[]);
        let mut config = Config::default();
        config
            .lists
            .insert("sample".into(), vec!["A".into(), "B".into()]);
        config
            .lists
            .insert("method".into(), vec!["bwa".into(), "star".into()]);

        let expanded = expand_aggregation(&rule, &config).unwrap();
        assert_eq!(expanded.len(), 4);
        assert!(expanded.contains(&"results/A/bwa.bam".to_string()));
        assert!(expanded.contains(&"results/A/star.bam".to_string()));
        assert!(expanded.contains(&"results/B/bwa.bam".to_string()));
        assert!(expanded.contains(&"results/B/star.bam".to_string()));
    }

    // -- Guard evaluation: In -----------------------------------------------

    #[test]
    fn guard_in_true() {
        let guard = GuardExpr::In {
            field: "sample".into(),
            values: vec!["A".into(), "B".into()],
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        assert!(evaluate_guard(&guard, &wc, &Config::default()));
    }

    #[test]
    fn guard_in_false() {
        let guard = GuardExpr::In {
            field: "sample".into(),
            values: vec!["A".into(), "B".into()],
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "C".into());
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    #[test]
    fn guard_in_with_config_list_ref() {
        let guard = GuardExpr::In {
            field: "sample".into(),
            values: vec!["@samples".into()],
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "B".into());
        let mut config = Config::default();
        config
            .lists
            .insert("samples".into(), vec!["A".into(), "B".into()]);
        assert!(evaluate_guard(&guard, &wc, &config));
    }

    // -- Guard evaluation: Eq -----------------------------------------------

    #[test]
    fn guard_eq_true() {
        let guard = GuardExpr::Eq {
            field: "method".into(),
            value: "bwa".into(),
        };
        let mut wc = Wildcards::new();
        wc.insert("method".into(), "bwa".into());
        assert!(evaluate_guard(&guard, &wc, &Config::default()));
    }

    #[test]
    fn guard_eq_false() {
        let guard = GuardExpr::Eq {
            field: "method".into(),
            value: "bwa".into(),
        };
        let mut wc = Wildcards::new();
        wc.insert("method".into(), "star".into());
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- Guard evaluation: Regex --------------------------------------------

    #[test]
    fn guard_regex_true() {
        let guard = GuardExpr::Regex {
            field: "sample".into(),
            pattern: r"^patient_\d+$".into(),
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "patient_42".into());
        assert!(evaluate_guard(&guard, &wc, &Config::default()));
    }

    #[test]
    fn guard_regex_false() {
        let guard = GuardExpr::Regex {
            field: "sample".into(),
            pattern: r"^patient_\d+$".into(),
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "control_group".into());
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- Guard evaluation: NotIn --------------------------------------------

    #[test]
    fn guard_notin_true() {
        let guard = GuardExpr::NotIn {
            field: "sample".into(),
            values: vec!["X".into(), "Y".into()],
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        assert!(evaluate_guard(&guard, &wc, &Config::default()));
    }

    #[test]
    fn guard_notin_false() {
        let guard = GuardExpr::NotIn {
            field: "sample".into(),
            values: vec!["A".into(), "B".into()],
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- Guard evaluation: NotEq --------------------------------------------

    #[test]
    fn guard_noteq_true() {
        let guard = GuardExpr::NotEq {
            field: "method".into(),
            value: "bwa".into(),
        };
        let mut wc = Wildcards::new();
        wc.insert("method".into(), "star".into());
        assert!(evaluate_guard(&guard, &wc, &Config::default()));
    }

    #[test]
    fn guard_noteq_false() {
        let guard = GuardExpr::NotEq {
            field: "method".into(),
            value: "bwa".into(),
        };
        let mut wc = Wildcards::new();
        wc.insert("method".into(), "bwa".into());
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- Missing producer ---------------------------------------------------

    #[test]
    fn missing_producer_error() {
        let rule = make_rule("align", &[], &["results/{sample}.bam"]);
        let request = make_request(&["nonexistent/file.txt"], &[]);
        let err = resolve(&[rule], &request).unwrap_err();
        assert!(matches!(
            err,
            DagError::Wildcard(WildcardError::MissingSource { .. })
        ));
    }

    // -- Ambiguous producer -------------------------------------------------

    #[test]
    fn ambiguous_producer_error() {
        let rule_a = make_rule("align_a", &[], &["results/{sample}.bam"]);
        let rule_b = make_rule("align_b", &[], &["results/{sample}.bam"]);
        let request = make_request(&["results/X.bam"], &[]);
        let err = resolve(&[rule_a, rule_b], &request).unwrap_err();
        assert!(matches!(
            err,
            DagError::Wildcard(WildcardError::AmbiguousProducer { .. })
        ));
    }

    // -- Ambiguous producer resolved by priority ----------------------------

    #[test]
    fn ambiguous_producer_resolved_by_priority() {
        let mut rule_a = make_rule(
            "align_a",
            &["data/{sample}.fastq"],
            &["results/{sample}.bam"],
        );
        rule_a.priority = Some(10);
        let mut rule_b = make_rule(
            "align_b",
            &["data/{sample}.fastq"],
            &["results/{sample}.bam"],
        );
        rule_b.priority = Some(5);

        let request = make_request(&["results/X.bam"], &["data/X.fastq"]);
        let result = resolve(&[rule_a, rule_b], &request).unwrap();
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].rule, RuleName::from("align_a"));
    }

    // -- Source file detection ----------------------------------------------

    #[test]
    fn source_file_detection() {
        let rule = make_rule("compile", &["src/{name}.c"], &["build/{name}.o"]);
        let request = make_request(&["build/main.o"], &["src/main.c"]);

        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.sources, vec![PathBuf::from("src/main.c")]);
    }

    #[test]
    fn target_is_existing_file() {
        // If the target itself exists, no job is needed.
        let request = make_request(&["data/existing.txt"], &["data/existing.txt"]);
        let result = resolve(&[], &request).unwrap();
        assert_eq!(result.jobs.len(), 0);
        assert_eq!(result.sources, vec![PathBuf::from("data/existing.txt")]);
    }

    #[test]
    fn source_file_auto_detected_from_disk() {
        // If a rule input exists on disk but is NOT in `existing_files`,
        // the resolver should auto-detect it as a source file instead of
        // failing with NoProducer.
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("data.db");
        std::fs::write(&source_path, b"fake db").unwrap();

        let source_str = source_path.to_str().unwrap();
        let output_path = dir.path().join("result.csv");
        let output_str = output_path.to_str().unwrap();

        let rule = make_rule("process", &[source_str], &[output_str]);
        // Note: existing_files is EMPTY — the resolver must discover the
        // source file via filesystem check.
        let request = make_request(&[output_str], &[]);
        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.sources, vec![source_path]);
    }

    #[test]
    fn missing_source_file_still_errors() {
        // If a rule input neither has a producing rule NOR exists on disk,
        // the resolver should still return an error.
        let rule = make_rule("process", &["/nonexistent/path/data.db"], &["result.csv"]);
        let request = make_request(&["result.csv"], &[]);
        let err = resolve(&[rule], &request).unwrap_err();
        assert!(
            matches!(err, DagError::Wildcard(WildcardError::MissingSource { .. })),
            "expected MissingSource error, got: {err:?}"
        );
        // The message must carry the remedy: this is the first error a new
        // user hits after `ox init` + `ox run` (template inputs don't exist).
        let msg = err.to_string();
        assert!(
            msg.contains("create it as a source file"),
            "no remedy: {msg}"
        );
    }

    // -- Circular dependency detection --------------------------------------

    #[test]
    fn circular_dependency_detected() {
        let rule_a = make_rule("a", &["b.txt"], &["a.txt"]);
        let rule_b = make_rule("b", &["a.txt"], &["b.txt"]);
        let request = make_request(&["a.txt"], &[]);
        let err = resolve(&[rule_a, rule_b], &request).unwrap_err();
        assert!(matches!(err, DagError::CycleDetected { .. }));
    }

    // -- Multiple targets ---------------------------------------------------

    #[test]
    fn multiple_targets() {
        let rule = make_rule("compile", &["src/{name}.c"], &["build/{name}.o"]);

        let request = make_request(
            &["build/main.o", "build/util.o"],
            &["src/main.c", "src/util.c"],
        );

        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs.len(), 2);
    }

    // -- Deduplication: same target requested twice -------------------------

    #[test]
    fn dedup_same_target_twice() {
        let rule = make_rule("compile", &["src/{name}.c"], &["build/{name}.o"]);
        let request = make_request(&["build/main.o", "build/main.o"], &["src/main.c"]);

        let result = resolve(&[rule], &request).unwrap();
        // Should produce only one job, not two.
        assert_eq!(result.jobs.len(), 1);
    }

    // -- Diamond dependency (shared intermediate) ---------------------------

    #[test]
    fn diamond_dependency_dedup() {
        let generate = make_rule("generate", &["src/{name}.raw"], &["tmp/{name}.processed"]);
        let combine_a = make_rule("combine_a", &["tmp/{name}.processed"], &["out/{name}.a"]);
        let combine_b = make_rule("combine_b", &["tmp/{name}.processed"], &["out/{name}.b"]);

        let request = make_request(&["out/x.a", "out/x.b"], &["src/x.raw"]);

        let result = resolve(&[generate, combine_a, combine_b], &request).unwrap();
        // generate should appear only once, even though both combine_a and
        // combine_b depend on it.
        let gen_jobs: Vec<_> = result
            .jobs
            .iter()
            .filter(|j| j.rule == RuleName::from("generate"))
            .collect();
        assert_eq!(gen_jobs.len(), 1);
        assert_eq!(result.jobs.len(), 3);
    }

    // -- Execution interpolation -------------------------------------------

    #[test]
    fn execution_command_interpolated() {
        let mut rule = make_rule("align", &["data/{sample}.fastq"], &["results/{sample}.bam"]);
        rule.execution = ExecutionBlock::Shell {
            command: "bwa mem data/{sample}.fastq > results/{sample}.bam".into(),
        };

        let request = make_request(&["results/A.bam"], &["data/A.fastq"]);

        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(
            result.jobs[0].execution,
            ExecutionBlock::Shell {
                command: "bwa mem data/A.fastq > results/A.bam".into()
            }
        );
    }

    // -- Job ID construction ------------------------------------------------

    #[test]
    fn job_id_includes_wildcards() {
        let rule = make_rule("align", &["data/{sample}.fastq"], &["results/{sample}.bam"]);
        let request = make_request(&["results/patient_42.bam"], &["data/patient_42.fastq"]);
        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs[0].id.as_str(), "align-patient_42");
    }

    #[test]
    fn job_id_no_wildcards() {
        let rule = make_rule("clean", &[], &["build/clean.done"]);
        let request = make_request(&["build/clean.done"], &[]);
        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs[0].id.as_str(), "clean");
    }

    // -- Tags include wildcards as implicit tags ----------------------------

    #[test]
    fn wildcard_values_appear_as_tags() {
        let mut rule = make_rule("align", &["data/{sample}.fastq"], &["results/{sample}.bam"]);
        rule.tags.insert("stage".into(), "alignment".into());

        let request = make_request(&["results/A.bam"], &["data/A.fastq"]);
        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs[0].tags["stage"], "alignment");
        assert_eq!(result.jobs[0].tags["sample"], "A");
    }

    // -- Guard filters out a producer, making target unresolvable ----------

    #[test]
    fn guard_filters_producer() {
        let mut rule = make_rule(
            "special",
            &["data/{sample}.fastq"],
            &["results/{sample}.bam"],
        );
        rule.when = Some(GuardExpr::Eq {
            field: "sample".into(),
            value: "only_this_one".into(),
        });

        let request = make_request(&["results/nope.bam"], &["data/nope.fastq"]);
        let err = resolve(&[rule], &request).unwrap_err();
        assert!(matches!(
            err,
            DagError::Wildcard(WildcardError::NoProducer { .. })
        ));
    }

    // -- Aggregation with zip mode -----------------------------------------

    #[test]
    fn aggregation_zip_mode() {
        let mut rule = make_rule("all", &["results/{sample}/{method}.bam"], &[]);
        rule.expand_mode = ExpandMode::Zip;

        let mut config = Config::default();
        config
            .lists
            .insert("sample".into(), vec!["A".into(), "B".into()]);
        config
            .lists
            .insert("method".into(), vec!["bwa".into(), "star".into()]);

        let expanded = expand_aggregation(&rule, &config).unwrap();
        assert_eq!(expanded.len(), 2);
        assert_eq!(expanded[0], "results/A/bwa.bam");
        assert_eq!(expanded[1], "results/B/star.bam");
    }

    #[test]
    fn aggregation_zip_unequal_length_error() {
        let mut rule = make_rule("all", &["results/{sample}/{method}.bam"], &[]);
        rule.expand_mode = ExpandMode::Zip;

        let mut config = Config::default();
        config
            .lists
            .insert("sample".into(), vec!["A".into(), "B".into()]);
        config.lists.insert("method".into(), vec!["bwa".into()]);

        let err = expand_aggregation(&rule, &config).unwrap_err();
        assert!(matches!(err, WildcardError::InvalidPattern { .. }));
    }

    // -- Aggregation with missing config list ------------------------------

    #[test]
    fn aggregation_missing_config_list() {
        let rule = make_rule("all", &["results/{sample}.bam"], &[]);
        let config = Config::default();

        let err = expand_aggregation(&rule, &config).unwrap_err();
        assert!(matches!(err, WildcardError::UnresolvableWildcard { .. }));
    }

    // -- Helper tests -------------------------------------------------------

    #[test]
    fn build_job_id_empty_wildcards() {
        let id = build_job_id(&RuleName::from("clean"), &Wildcards::new());
        assert_eq!(id.as_str(), "clean");
    }

    #[test]
    fn build_job_id_with_wildcards() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        wc.insert("method".into(), "bwa".into());
        let id = build_job_id(&RuleName::from("process"), &wc);
        // BTreeMap is sorted, so "method" comes before "sample".
        assert_eq!(id.as_str(), "process-bwa-A");
    }

    #[test]
    fn build_job_id_injective_for_dashed_values() {
        // B5: joining wildcard values with `-` made the minting non-injective:
        // {a:"x-y", b:"z"} and {a:"x", b:"y-z"} both produced `r-x-y-z`,
        // so one of the two jobs silently vanished from the job index.
        let rule = RuleName::from("r");
        let wc1 = Wildcards::from([
            ("a".to_string(), "x-y".to_string()),
            ("b".to_string(), "z".to_string()),
        ]);
        let wc2 = Wildcards::from([
            ("a".to_string(), "x".to_string()),
            ("b".to_string(), "y-z".to_string()),
        ]);
        assert_ne!(build_job_id(&rule, &wc1), build_job_id(&rule, &wc2));
    }

    #[test]
    fn build_job_id_plain_values_keep_readable_format() {
        // Values without `-` keep the historical readable format.
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        let id = build_job_id(&RuleName::from("align"), &wc);
        assert_eq!(id.as_str(), "align-A");
    }

    #[test]
    fn interpolate_full_basic() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "patient_42".into());
        let result = interpolate_full(
            "data/{sample}.fastq",
            &wc,
            &[],
            &[],
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(result, "data/patient_42.fastq");
    }

    #[test]
    fn cartesian_product_two_lists() {
        let s = "sample".to_string();
        let m = "method".to_string();
        let sv = vec!["A".into(), "B".into()];
        let mv = vec!["bwa".into(), "star".into()];
        let lists = vec![(&s, &sv), (&m, &mv)];
        let combos = cartesian_product(&lists);
        assert_eq!(combos.len(), 4);
    }

    #[test]
    fn cartesian_product_empty() {
        let combos = cartesian_product(&[]);
        assert_eq!(combos.len(), 1);
        assert!(combos[0].is_empty());
    }

    // -- Guard: In with missing wildcard field returns false ---------------

    #[test]
    fn guard_in_missing_field() {
        let guard = GuardExpr::In {
            field: "missing".into(),
            values: vec!["A".into()],
        };
        let wc = Wildcards::new();
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- Guard: NotIn with missing wildcard field returns true -------------

    #[test]
    fn guard_notin_missing_field() {
        let guard = GuardExpr::NotIn {
            field: "missing".into(),
            values: vec!["A".into()],
        };
        let wc = Wildcards::new();
        assert!(evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- Guard: NotIn with config list ref ---------------------------------

    #[test]
    fn guard_notin_with_config_list_ref() {
        let guard = GuardExpr::NotIn {
            field: "sample".into(),
            values: vec!["@samples".into()],
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "C".into());
        let mut config = Config::default();
        config
            .lists
            .insert("samples".into(), vec!["A".into(), "B".into()]);
        assert!(evaluate_guard(&guard, &wc, &config));

        // Now test when value IS in the config list
        wc.insert("sample".into(), "A".into());
        assert!(!evaluate_guard(&guard, &wc, &config));
    }

    // -- Guard: Regex with missing wildcard field returns false ------------

    #[test]
    fn guard_regex_missing_field() {
        let guard = GuardExpr::Regex {
            field: "missing".into(),
            pattern: ".*".into(),
        };
        let wc = Wildcards::new();
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- Interpolate execution: Run mode ----------------------------------

    #[test]
    fn interpolate_execution_run_mode() {
        let exec = ExecutionBlock::Run {
            code: "print('{sample}')".into(),
            lang: "python".into(),
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        let result = interpolate_execution(
            &exec,
            &wc,
            &[],
            &[],
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            result,
            ExecutionBlock::Run {
                code: "print('A')".into(),
                lang: "python".into()
            }
        );
    }

    // -- Interpolate execution: Script mode -------------------------------

    #[test]
    fn interpolate_execution_script_mode() {
        let exec = ExecutionBlock::Script {
            path: PathBuf::from("scripts/{sample}.py"),
            lang: Some("python".into()),
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        let result = interpolate_execution(
            &exec,
            &wc,
            &[],
            &[],
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            result,
            ExecutionBlock::Script {
                path: PathBuf::from("scripts/A.py"),
                lang: Some("python".into())
            }
        );
    }

    // -- Interpolate execution: Call mode ----------------------------------

    #[test]
    fn interpolate_execution_call_mode() {
        let exec = ExecutionBlock::Call {
            function: "my_func".into(),
            lang: "python".into(),
        };
        let wc = Wildcards::new();
        let result = interpolate_execution(
            &exec,
            &wc,
            &[],
            &[],
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            result,
            ExecutionBlock::Call {
                function: "my_func".into(),
                lang: "python".into()
            }
        );
    }

    #[test]
    fn interpolate_execution_call_mode_with_wildcards() {
        let exec = ExecutionBlock::Call {
            function: "pipeline.{cohort}:compute".into(),
            lang: "python".into(),
        };
        let mut wc = Wildcards::new();
        wc.insert("cohort".into(), "human".into());
        let result = interpolate_execution(
            &exec,
            &wc,
            &[],
            &[],
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            result,
            ExecutionBlock::Call {
                function: "pipeline.human:compute".into(),
                lang: "python".into()
            }
        );
    }

    // -- Interpolate execution: config scalars in shell commands ------------

    #[test]
    fn interpolate_execution_config_scalars() {
        let exec = ExecutionBlock::Shell {
            command: "tool --flag {config.is_dev_end} --name {config.project}".into(),
        };
        let wc = Wildcards::new();
        let mut config = BTreeMap::new();
        config.insert("is_dev_end".into(), "true".into());
        config.insert("project".into(), "myproj".into());
        let result = interpolate_execution(
            &exec,
            &wc,
            &[],
            &[],
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &config,
        )
        .unwrap();
        assert_eq!(
            result,
            ExecutionBlock::Shell {
                command: "tool --flag true --name myproj".into()
            }
        );
    }

    // -- Aggregation with no wildcards returns literal paths ---------------

    #[test]
    fn aggregation_no_wildcards() {
        let rule = make_rule("all", &["data/fixed.csv"], &[]);
        let config = Config::default();
        let expanded = expand_aggregation(&rule, &config).unwrap();
        assert_eq!(expanded, vec!["data/fixed.csv"]);
    }

    // -- Guard rejects producer and clears resolving state ----------------

    #[test]
    fn guard_rejects_producer_clears_state() {
        let mut rule = make_rule(
            "guarded",
            &["data/{sample}.fastq"],
            &["results/{sample}.bam"],
        );
        rule.when = Some(GuardExpr::In {
            field: "sample".into(),
            values: vec!["allowed".into()],
        });

        let request = make_request(&["results/blocked.bam"], &["data/blocked.fastq"]);
        let err = resolve(&[rule], &request).unwrap_err();
        assert!(matches!(
            err,
            DagError::Wildcard(WildcardError::NoProducer { .. })
        ));
    }

    // -- Guard passes and resolution continues ----------------------------

    #[test]
    fn guard_passes_resolution_continues() {
        let mut rule = make_rule(
            "guarded",
            &["data/{sample}.fastq"],
            &["results/{sample}.bam"],
        );
        rule.when = Some(GuardExpr::In {
            field: "sample".into(),
            values: vec!["allowed".into()],
        });

        let request = make_request(&["results/allowed.bam"], &["data/allowed.fastq"]);
        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].rule, RuleName::from("guarded"));
    }

    // -- NotIn with config list ref that doesn't exist --------------------

    #[test]
    fn guard_notin_with_missing_config_list_ref() {
        let guard = GuardExpr::NotIn {
            field: "sample".into(),
            values: vec!["@nonexistent".into()],
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        // When the config list doesn't exist, flat_map returns empty,
        // so the value is not in the (empty) list -> NotIn returns true.
        assert!(evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- In with config list ref that doesn't exist -----------------------

    #[test]
    fn guard_in_with_missing_config_list_ref() {
        let guard = GuardExpr::In {
            field: "sample".into(),
            values: vec!["@nonexistent".into()],
        };
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- zip_lists empty --------------------------------------------------

    #[test]
    fn zip_lists_empty() {
        let result = zip_lists(&[]).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].is_empty());
    }

    #[test]
    fn interpolate_full_with_input_output() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        let inputs = vec![("data/A.csv".to_string(), None)];
        let outputs = vec![("results/A.txt".to_string(), None)];
        let result = interpolate_full(
            "cat {input} > {output}",
            &wc,
            &inputs,
            &outputs,
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(result, "cat data/A.csv > results/A.txt");
    }

    #[test]
    fn interpolate_full_with_indexed_access() {
        let wc = Wildcards::new();
        let inputs = vec![("a.txt".to_string(), None), ("b.txt".to_string(), None)];
        let outputs = vec![("out.txt".to_string(), None)];
        let result = interpolate_full(
            "cat {input[0]} {input[1]} > {output[0]}",
            &wc,
            &inputs,
            &outputs,
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(result, "cat a.txt b.txt > out.txt");
    }

    #[test]
    fn interpolate_full_mixed_wildcards_and_io() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "X".into());
        let inputs = vec![("data/X.csv".to_string(), None)];
        let outputs = vec![("results/X.txt".to_string(), None)];
        let result = interpolate_full(
            "process --sample={sample} {input} > {output}",
            &wc,
            &inputs,
            &outputs,
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(result, "process --sample=X data/X.csv > results/X.txt");
    }

    #[test]
    fn interpolate_full_named_inputs() {
        let wc = Wildcards::new();
        let inputs = vec![
            ("data/genome.fa".to_string(), Some("genome".to_string())),
            ("data/reads.fq".to_string(), Some("reads".to_string())),
        ];
        let outputs = vec![("results/aligned.bam".to_string(), None)];
        let result = interpolate_full(
            "bwa mem {input.genome} {input.reads} > {output}",
            &wc,
            &inputs,
            &outputs,
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            result,
            "bwa mem data/genome.fa data/reads.fq > results/aligned.bam"
        );
    }

    #[test]
    fn interpolate_full_named_inputs_expand() {
        // When expand produces multiple inputs sharing the same name,
        // {input.name} should substitute ALL paths space-separated.
        let wc = Wildcards::new();
        let inputs = vec![
            ("cleaned/sales.csv".to_string(), Some("csvs".to_string())),
            (
                "cleaned/inventory.csv".to_string(),
                Some("csvs".to_string()),
            ),
            ("cleaned/returns.csv".to_string(), Some("csvs".to_string())),
        ];
        let outputs = vec![(
            "output/combined.csv".to_string(),
            Some("combined".to_string()),
        )];
        let result = interpolate_full(
            "for f in {input.csvs}; do cat $f; done > {output.combined}",
            &wc,
            &inputs,
            &outputs,
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            result,
            "for f in cleaned/sales.csv cleaned/inventory.csv cleaned/returns.csv; do cat $f; done > output/combined.csv"
        );
    }

    #[test]
    fn interpolate_full_named_outputs() {
        let wc = Wildcards::new();
        let inputs = vec![("data/input.csv".to_string(), None)];
        let outputs = vec![
            ("results/main.txt".to_string(), Some("main".to_string())),
            ("results/log.txt".to_string(), Some("log".to_string())),
        ];
        let result = interpolate_full(
            "process {input} --out {output.main} --log {output.log}",
            &wc,
            &inputs,
            &outputs,
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            result,
            "process data/input.csv --out results/main.txt --log results/log.txt"
        );
    }

    #[test]
    fn interpolate_full_wildcards_dot_syntax() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "patient_42".into());
        wc.insert("method".into(), "bwa".into());
        let inputs = vec![("data/patient_42.fq".to_string(), None)];
        let outputs = vec![("results/patient_42.bam".to_string(), None)];
        let result = interpolate_full(
            "echo {wildcards.sample} {wildcards.method} && process {input} > {output}",
            &wc,
            &inputs,
            &outputs,
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            result,
            "echo patient_42 bwa && process data/patient_42.fq > results/patient_42.bam"
        );
    }

    #[test]
    fn interpolate_full_all_placeholder_types() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        let inputs = vec![
            ("data/genome.fa".to_string(), Some("genome".to_string())),
            ("data/A.fq".to_string(), Some("reads".to_string())),
        ];
        let outputs = vec![("results/A.bam".to_string(), Some("bam".to_string()))];
        let result = interpolate_full(
            "align --ref={input.genome} --reads={input.reads} --idx={input[0]} \
             --sample={wildcards.sample} --bare={sample} \
             --all-in='{input}' --out={output.bam} --out0={output[0]} --all-out='{output}'",
            &wc,
            &inputs,
            &outputs,
            &BTreeMap::new(),
            &LogConfig::default(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            result,
            "align --ref=data/genome.fa --reads=data/A.fq --idx=data/genome.fa \
             --sample=A --bare=A \
             --all-in='data/genome.fa data/A.fq' --out=results/A.bam --out0=results/A.bam --all-out='results/A.bam'"
        );
    }

    #[test]
    fn interpolate_full_log_placeholder() {
        let wc = Wildcards::new();
        let log = LogConfig {
            stdout: Some("logs/run.log".into()),
            stderr: Some("logs/run.err".into()),
        };
        let result = interpolate_full(
            "command > {log} 2>&1",
            &wc,
            &[],
            &[],
            &BTreeMap::new(),
            &log,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(result, "command > logs/run.log 2>&1");
    }

    #[test]
    fn interpolate_full_threads_placeholder() {
        let wc = Wildcards::new();
        let mut resources = BTreeMap::new();
        resources.insert("cpu".into(), ResourceValue::Int(8));
        let result = interpolate_full(
            "bwa mem -t {threads} ref.fa reads.fq",
            &wc,
            &[],
            &[],
            &BTreeMap::new(),
            &LogConfig::default(),
            &resources,
            &BTreeMap::new(),
        );
        assert_eq!(result, "bwa mem -t 8 ref.fa reads.fq");
    }

    #[test]
    fn interpolate_full_resources_placeholder() {
        let wc = Wildcards::new();
        let mut resources = BTreeMap::new();
        resources.insert("cpu".into(), ResourceValue::Int(4));
        resources.insert("mem".into(), ResourceValue::Str("8G".into()));
        let result = interpolate_full(
            "run --cpus {resources.cpu} --mem {resources.mem}",
            &wc,
            &[],
            &[],
            &BTreeMap::new(),
            &LogConfig::default(),
            &resources,
            &BTreeMap::new(),
        );
        assert_eq!(result, "run --cpus 4 --mem 8G");
    }

    #[test]
    fn interpolate_simple_wildcards() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "patient_42".into());
        assert_eq!(
            interpolate_simple("logs/{sample}.log", &wc),
            "logs/patient_42.log"
        );
    }

    #[test]
    fn interpolate_full_combined_snakemake_style() {
        // Simulate a typical translated Snakemake command with log, threads, params.
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "A".into());
        let inputs = vec![("data/A.csv".to_string(), Some("csv".to_string()))];
        let outputs = vec![("results/A.txt".to_string(), None)];
        let mut params = BTreeMap::new();
        params.insert("n_rows".into(), "100".into());
        let log = LogConfig {
            stdout: Some("logs/A.log".into()),
            stderr: None,
        };
        let mut resources = BTreeMap::new();
        resources.insert("cpu".into(), ResourceValue::Int(4));
        let result = interpolate_full(
            "process -t {threads} -n {params.n_rows} {input.csv} > {output} 2> {log}",
            &wc,
            &inputs,
            &outputs,
            &params,
            &log,
            &resources,
            &BTreeMap::new(),
        );
        assert_eq!(
            result,
            "process -t 4 -n 100 data/A.csv > results/A.txt 2> logs/A.log"
        );
    }

    // -- Guard: ConfigEq ---------------------------------------------------

    #[test]
    fn guard_config_eq_matches() {
        let guard = GuardExpr::ConfigEq {
            key: "mode".into(),
            value: "production".into(),
        };
        let wc = Wildcards::new();
        let mut config = Config::default();
        config.scalars.insert("mode".into(), "production".into());
        assert!(evaluate_guard(&guard, &wc, &config));
    }

    #[test]
    fn guard_config_eq_mismatch() {
        let guard = GuardExpr::ConfigEq {
            key: "mode".into(),
            value: "production".into(),
        };
        let wc = Wildcards::new();
        let mut config = Config::default();
        config.scalars.insert("mode".into(), "dev".into());
        assert!(!evaluate_guard(&guard, &wc, &config));
    }

    #[test]
    fn guard_config_eq_missing_key() {
        let guard = GuardExpr::ConfigEq {
            key: "missing".into(),
            value: "x".into(),
        };
        assert!(!evaluate_guard(
            &guard,
            &Wildcards::new(),
            &Config::default()
        ));
    }

    // -- Guard: EnvSet -----------------------------------------------------

    #[test]
    #[serial(env)]
    fn guard_env_set_present() {
        let guard = GuardExpr::EnvSet {
            var: "OX_TEST_GUARD_VAR".into(),
        };
        unsafe { std::env::set_var("OX_TEST_GUARD_VAR", "1") };
        assert!(evaluate_guard(
            &guard,
            &Wildcards::new(),
            &Config::default()
        ));
        unsafe { std::env::remove_var("OX_TEST_GUARD_VAR") };
    }

    #[test]
    #[serial(env)]
    fn guard_env_set_missing() {
        let guard = GuardExpr::EnvSet {
            var: "OX_TEST_GUARD_MISSING_VAR".into(),
        };
        unsafe { std::env::remove_var("OX_TEST_GUARD_MISSING_VAR") };
        assert!(!evaluate_guard(
            &guard,
            &Wildcards::new(),
            &Config::default()
        ));
    }

    #[test]
    #[serial(env)]
    fn guard_env_set_empty() {
        let guard = GuardExpr::EnvSet {
            var: "OX_TEST_GUARD_EMPTY_VAR".into(),
        };
        unsafe { std::env::set_var("OX_TEST_GUARD_EMPTY_VAR", "") };
        assert!(!evaluate_guard(
            &guard,
            &Wildcards::new(),
            &Config::default()
        ));
        unsafe { std::env::remove_var("OX_TEST_GUARD_EMPTY_VAR") };
    }

    // -- Guard: EnvEq ------------------------------------------------------

    #[test]
    #[serial(env)]
    fn guard_env_eq_matches() {
        let guard = GuardExpr::EnvEq {
            var: "OX_TEST_GUARD_EQ".into(),
            value: "prod".into(),
        };
        unsafe { std::env::set_var("OX_TEST_GUARD_EQ", "prod") };
        assert!(evaluate_guard(
            &guard,
            &Wildcards::new(),
            &Config::default()
        ));
        unsafe { std::env::remove_var("OX_TEST_GUARD_EQ") };
    }

    #[test]
    #[serial(env)]
    fn guard_env_eq_mismatch() {
        let guard = GuardExpr::EnvEq {
            var: "OX_TEST_GUARD_EQ2".into(),
            value: "prod".into(),
        };
        unsafe { std::env::set_var("OX_TEST_GUARD_EQ2", "dev") };
        assert!(!evaluate_guard(
            &guard,
            &Wildcards::new(),
            &Config::default()
        ));
        unsafe { std::env::remove_var("OX_TEST_GUARD_EQ2") };
    }

    // -- Guard: FileExists -------------------------------------------------

    #[test]
    fn guard_file_exists_present() {
        let guard = GuardExpr::FileExists {
            path: "Cargo.toml".into(),
        };
        assert!(evaluate_guard(
            &guard,
            &Wildcards::new(),
            &Config::default()
        ));
    }

    #[test]
    fn guard_file_exists_missing() {
        let guard = GuardExpr::FileExists {
            path: "nonexistent_file_12345.txt".into(),
        };
        assert!(!evaluate_guard(
            &guard,
            &Wildcards::new(),
            &Config::default()
        ));
    }

    // -- Guard: And --------------------------------------------------------

    #[test]
    fn guard_and_all_true() {
        let guard = GuardExpr::And {
            conditions: vec![
                GuardExpr::Eq {
                    field: "s".into(),
                    value: "A".into(),
                },
                GuardExpr::Eq {
                    field: "s".into(),
                    value: "A".into(),
                },
            ],
        };
        let mut wc = Wildcards::new();
        wc.insert("s".into(), "A".into());
        assert!(evaluate_guard(&guard, &wc, &Config::default()));
    }

    #[test]
    fn guard_and_one_false() {
        let guard = GuardExpr::And {
            conditions: vec![
                GuardExpr::Eq {
                    field: "s".into(),
                    value: "A".into(),
                },
                GuardExpr::Eq {
                    field: "s".into(),
                    value: "B".into(),
                },
            ],
        };
        let mut wc = Wildcards::new();
        wc.insert("s".into(), "A".into());
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- Guard: Or ---------------------------------------------------------

    #[test]
    fn guard_or_one_true() {
        let guard = GuardExpr::Or {
            conditions: vec![
                GuardExpr::Eq {
                    field: "s".into(),
                    value: "A".into(),
                },
                GuardExpr::Eq {
                    field: "s".into(),
                    value: "B".into(),
                },
            ],
        };
        let mut wc = Wildcards::new();
        wc.insert("s".into(), "A".into());
        assert!(evaluate_guard(&guard, &wc, &Config::default()));
    }

    #[test]
    fn guard_or_none_true() {
        let guard = GuardExpr::Or {
            conditions: vec![
                GuardExpr::Eq {
                    field: "s".into(),
                    value: "X".into(),
                },
                GuardExpr::Eq {
                    field: "s".into(),
                    value: "Y".into(),
                },
            ],
        };
        let mut wc = Wildcards::new();
        wc.insert("s".into(), "A".into());
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- Guard: Not --------------------------------------------------------

    #[test]
    fn guard_not_true_becomes_false() {
        let guard = GuardExpr::Not {
            condition: Box::new(GuardExpr::Eq {
                field: "s".into(),
                value: "A".into(),
            }),
        };
        let mut wc = Wildcards::new();
        wc.insert("s".into(), "A".into());
        assert!(!evaluate_guard(&guard, &wc, &Config::default()));
    }

    #[test]
    fn guard_not_false_becomes_true() {
        let guard = GuardExpr::Not {
            condition: Box::new(GuardExpr::Eq {
                field: "s".into(),
                value: "B".into(),
            }),
        };
        let mut wc = Wildcards::new();
        wc.insert("s".into(), "A".into());
        assert!(evaluate_guard(&guard, &wc, &Config::default()));
    }

    // -- Wildcard constraint validation ------------------------------------

    #[test]
    fn validate_constraints_passes_when_matching() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "alpha".into());
        let mut constraints = BTreeMap::new();
        constraints.insert("sample".into(), "alpha|beta|gamma".into());
        assert!(validate_constraints(&wc, &constraints, "test_rule").is_ok());
    }

    #[test]
    fn validate_constraints_rejects_non_matching() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "delta".into());
        let mut constraints = BTreeMap::new();
        constraints.insert("sample".into(), "alpha|beta|gamma".into());
        let err = validate_constraints(&wc, &constraints, "test_rule").unwrap_err();
        assert!(matches!(err, WildcardError::ConstraintViolation { .. }));
        assert!(err.to_string().contains("delta"));
        assert!(err.to_string().contains("alpha|beta|gamma"));
    }

    #[test]
    fn validate_constraints_ignores_unconstrained_wildcards() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "anything".into());
        wc.insert("method".into(), "whatever".into());
        let mut constraints = BTreeMap::new();
        constraints.insert("sample".into(), "[a-z]+".into());
        // "method" has no constraint — should pass
        assert!(validate_constraints(&wc, &constraints, "test_rule").is_ok());
    }

    #[test]
    fn validate_constraints_anchored_full_match() {
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "alpha_extra".into());
        let mut constraints = BTreeMap::new();
        constraints.insert("sample".into(), "alpha".into());
        // "alpha_extra" should NOT match the constraint "alpha" (full match required)
        let err = validate_constraints(&wc, &constraints, "test_rule").unwrap_err();
        assert!(matches!(err, WildcardError::ConstraintViolation { .. }));
    }

    #[test]
    fn aggregation_with_constraints_rejects_bad_values() {
        let mut rule = make_rule("all", &["results/{sample}.bam"], &[]);
        rule.wildcard_constraints
            .insert("sample".into(), "alpha|beta|gamma".into());
        let mut config = Config::default();
        config
            .lists
            .insert("sample".into(), vec!["alpha".into(), "delta".into()]);
        let err = expand_aggregation(&rule, &config).unwrap_err();
        assert!(err.to_string().contains("delta"));
    }

    #[test]
    fn aggregation_with_constraints_passes_good_values() {
        let mut rule = make_rule("all", &["results/{sample}.bam"], &[]);
        rule.wildcard_constraints
            .insert("sample".into(), "alpha|beta|gamma".into());
        let mut config = Config::default();
        config
            .lists
            .insert("sample".into(), vec!["alpha".into(), "beta".into()]);
        let expanded = expand_aggregation(&rule, &config).unwrap();
        assert_eq!(expanded, vec!["results/alpha.bam", "results/beta.bam"]);
    }

    #[test]
    fn resolve_constraint_violation_in_config_expansion() {
        // Rule: process takes {method} from config, outputs {sample}/{method}.out
        // Constraint: method must be "bwa" or "star"
        let mut rule = make_rule(
            "process",
            &["data/{sample}/{method}.input"],
            &["results/{sample}.output"],
        );
        rule.wildcard_constraints
            .insert("method".into(), "bwa|star".into());

        let mut config = Config::default();
        config
            .lists
            .insert("method".into(), vec!["bwa".into(), "invalid_method".into()]);

        let request = ResolveRequest {
            targets: vec!["results/A.output".into()],
            config,
            existing_files: vec![
                PathBuf::from("data/A/bwa.input"),
                PathBuf::from("data/A/invalid_method.input"),
            ],
        };

        let err = resolve(&[rule], &request).unwrap_err();
        assert!(err.to_string().contains("invalid_method"));
    }

    // ── {config.X} substitution in paths ─────────────────────────────

    #[test]
    fn substitute_config_scalars_no_reference() {
        let scalars = BTreeMap::new();
        let result = substitute_config_scalars("results/{sample}.bam", &scalars).unwrap();
        assert_eq!(result, "results/{sample}.bam");
    }

    #[test]
    fn substitute_config_scalars_single_key() {
        let mut scalars = BTreeMap::new();
        scalars.insert("results_dir".into(), "/data/results".into());
        let result =
            substitute_config_scalars("{config.results_dir}/{sample}.bam", &scalars).unwrap();
        assert_eq!(result, "/data/results/{sample}.bam");
    }

    #[test]
    fn substitute_config_scalars_multiple_keys() {
        let mut scalars = BTreeMap::new();
        scalars.insert("base".into(), "/mnt/data".into());
        scalars.insert("version".into(), "v2".into());
        let result =
            substitute_config_scalars("{config.base}/{config.version}/{sample}.csv", &scalars)
                .unwrap();
        assert_eq!(result, "/mnt/data/v2/{sample}.csv");
    }

    #[test]
    fn substitute_config_scalars_unknown_key_errors() {
        let scalars = BTreeMap::new();
        let err = substitute_config_scalars("{config.missing}/data.csv", &scalars).unwrap_err();
        assert!(matches!(err, WildcardError::UnknownConfigKey { .. }));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn config_substitution_in_output_pattern() {
        let mut config = Config::default();
        config
            .scalars
            .insert("results_dir".into(), "/shared/results".into());

        let rule = make_rule(
            "process",
            &["data/{sample}.csv"],
            &["{config.results_dir}/{sample}.out"],
        );

        let request = ResolveRequest {
            targets: vec!["/shared/results/A.out".into()],
            config,
            existing_files: vec![PathBuf::from("data/A.csv")],
        };

        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].wildcards["sample"], "A");
    }

    #[test]
    fn config_substitution_in_input_pattern() {
        let mut config = Config::default();
        config
            .scalars
            .insert("data_dir".into(), "/shared/data".into());

        let rule = make_rule(
            "process",
            &["{config.data_dir}/{sample}.csv"],
            &["results/{sample}.out"],
        );

        let request = ResolveRequest {
            targets: vec!["results/A.out".into()],
            config,
            existing_files: vec![PathBuf::from("/shared/data/A.csv")],
        };

        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].wildcards["sample"], "A");
        assert_eq!(result.sources, vec![PathBuf::from("/shared/data/A.csv")]);
    }

    #[test]
    fn config_substitution_in_both_input_and_output() {
        let mut config = Config::default();
        config
            .scalars
            .insert("data_dir".into(), "/shared/data".into());
        config
            .scalars
            .insert("results_dir".into(), "/shared/results".into());

        let rule = make_rule(
            "process",
            &["{config.data_dir}/{sample}.csv"],
            &["{config.results_dir}/{sample}.out"],
        );

        let request = ResolveRequest {
            targets: vec!["/shared/results/B.out".into()],
            config,
            existing_files: vec![PathBuf::from("/shared/data/B.csv")],
        };

        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].wildcards["sample"], "B");
    }

    #[test]
    fn config_substitution_unknown_key_in_output_errors() {
        let rule = make_rule(
            "process",
            &["data/{sample}.csv"],
            &["{config.nonexistent}/{sample}.out"],
        );

        let request = make_request(&["whatever"], &["data/A.csv"]);
        let err = resolve(&[rule], &request).unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn config_substitution_in_aggregation() {
        let mut config = Config::default();
        config.scalars.insert("out_dir".into(), "/results".into());
        config
            .lists
            .insert("sample".into(), vec!["A".into(), "B".into()]);

        let rule = make_rule(
            "all",
            &["{config.out_dir}/{sample}.bam"],
            &[], // aggregation — no outputs
        );

        let expanded = expand_aggregation(&rule, &config).unwrap();
        assert_eq!(expanded, vec!["/results/A.bam", "/results/B.bam"]);
    }

    // ── Property tests ─────────────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        // ── Cycle-detection termination property tests ─────────────────

        /// Strategy that generates a random rule graph.
        ///
        /// Produces `num_files` file names (`f0.txt` .. `f{N-1}.txt`) and
        /// `num_rules` rules.  Rule *i* produces `f{i}.txt` and may consume
        /// any subset of the other files as inputs.  Some non-produced files
        /// are marked as existing sources; at least one produced file is
        /// selected as a target.
        ///
        /// The resulting graph may contain cycles, missing sources, or be
        /// perfectly resolvable — exactly the diversity we need to assert
        /// the resolver always terminates.
        fn arb_rule_graph() -> impl Strategy<Value = (Vec<Rule>, ResolveRequest)> {
            (2..=8usize, 1..=6usize)
                .prop_flat_map(|(nf, nr)| {
                    let nr = nr.min(nf); // can't have more rules than files
                    let input_flags = proptest::collection::vec(
                        proptest::collection::vec(proptest::bool::ANY, nf),
                        nr,
                    );
                    let target_flags = proptest::collection::vec(proptest::bool::ANY, nr);
                    let source_count = nf.saturating_sub(nr);
                    let existing_flags =
                        proptest::collection::vec(proptest::bool::ANY, source_count);
                    (
                        Just(nf),
                        Just(nr),
                        input_flags,
                        target_flags,
                        existing_flags,
                    )
                })
                .prop_map(|(nf, nr, inp, tgt, exist)| {
                    let files: Vec<String> = (0..nf).map(|i| format!("f{i}.txt")).collect();

                    let rules: Vec<Rule> = (0..nr)
                        .map(|i| {
                            let inputs: Vec<&str> = inp[i]
                                .iter()
                                .enumerate()
                                .filter(|e| *e.1 && e.0 != i)
                                .map(|(j, _)| files[j].as_str())
                                .collect();
                            make_rule(&format!("r{i}"), &inputs, &[files[i].as_str()])
                        })
                        .collect();

                    // At least one target.
                    let mut targets: Vec<String> = tgt
                        .iter()
                        .enumerate()
                        .filter(|e| *e.1)
                        .map(|(i, _)| files[i].clone())
                        .collect();
                    if targets.is_empty() {
                        targets.push(files[0].clone());
                    }

                    let existing: Vec<PathBuf> = exist
                        .iter()
                        .enumerate()
                        .filter(|e| *e.1)
                        .map(|(i, _)| PathBuf::from(&files[nr + i]))
                        .collect();

                    let request = ResolveRequest {
                        targets,
                        config: Config::default(),
                        existing_files: existing,
                    };
                    (rules, request)
                })
        }

        proptest! {
            /// The resolver must terminate on every random rule graph.
            ///
            /// The graph may be acyclic (→ Ok), cyclic (→ CycleDetected),
            /// or have dangling inputs (→ MissingSource).  All outcomes are
            /// acceptable; the property under test is *termination* — the
            /// resolver never panics or loops infinitely.
            #[test]
            fn resolver_terminates_on_random_graphs(
                (rules, request) in arb_rule_graph()
            ) {
                let _result = resolve(&rules, &request);
                // Reaching this line proves termination.
            }

            /// When a random graph resolves successfully, the result
            /// contains at least one job per target that needed production,
            /// and every job's output was requested (directly or
            /// transitively).
            #[test]
            fn successful_resolve_covers_targets(
                (rules, request) in arb_rule_graph()
            ) {
                if let Ok(result) = resolve(&rules, &request) {
                    let produced: HashSet<String> = result
                        .jobs
                        .iter()
                        .flat_map(|j| j.outputs.iter().map(|o| match &o.reference {
                            OutputRef::File(p) => p.display().to_string(),
                            _ => String::new(),
                        }))
                        .collect();
                    let source_set: HashSet<String> = result
                        .sources
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect();

                    // Every requested target is either produced by a job
                    // or is an existing source file.
                    for t in &request.targets {
                        prop_assert!(
                            produced.contains(t) || source_set.contains(t),
                            "target `{t}` not covered by jobs or sources"
                        );
                    }
                }
            }
        }

        // ── Guard property tests ───────────────────────────────────────

        /// Strategy for a wildcard field name.
        fn field_name() -> impl Strategy<Value = String> {
            "[a-z_]{1,8}"
        }

        /// Strategy for a simple string value.
        fn value_str() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9_]{1,10}"
        }

        proptest! {
            /// In and NotIn are complementary: for any given wildcard value,
            /// exactly one of In/NotIn returns true (when field exists).
            #[test]
            fn in_notin_complementary(
                field in field_name(),
                wc_value in value_str(),
                values in proptest::collection::vec(value_str(), 1..6),
            ) {
                let mut wc = Wildcards::new();
                wc.insert(field.clone(), wc_value);
                let config = Config::default();

                let guard_in = GuardExpr::In {
                    field: field.clone(),
                    values: values.clone(),
                };
                let guard_notin = GuardExpr::NotIn {
                    field,
                    values,
                };

                let result_in = evaluate_guard(&guard_in, &wc, &config);
                let result_notin = evaluate_guard(&guard_notin, &wc, &config);
                prop_assert_ne!(
                    result_in, result_notin,
                    "In and NotIn must be complementary"
                );
            }

            /// Eq and NotEq are complementary: for any given wildcard value,
            /// exactly one returns true (when field exists).
            #[test]
            fn eq_noteq_complementary(
                field in field_name(),
                wc_value in value_str(),
                test_value in value_str(),
            ) {
                let mut wc = Wildcards::new();
                wc.insert(field.clone(), wc_value);
                let config = Config::default();

                let guard_eq = GuardExpr::Eq {
                    field: field.clone(),
                    value: test_value.clone(),
                };
                let guard_neq = GuardExpr::NotEq {
                    field,
                    value: test_value,
                };

                let result_eq = evaluate_guard(&guard_eq, &wc, &config);
                let result_neq = evaluate_guard(&guard_neq, &wc, &config);
                prop_assert_ne!(
                    result_eq, result_neq,
                    "Eq and NotEq must be complementary"
                );
            }

            /// Not(guard) always negates the result of guard for pure guards.
            #[test]
            fn not_negates(
                field in field_name(),
                wc_value in value_str(),
                values in proptest::collection::vec(value_str(), 1..4),
            ) {
                let mut wc = Wildcards::new();
                wc.insert(field.clone(), wc_value);
                let config = Config::default();

                let inner = GuardExpr::In {
                    field,
                    values,
                };
                let negated = GuardExpr::Not {
                    condition: Box::new(inner.clone()),
                };

                let result = evaluate_guard(&inner, &wc, &config);
                let negated_result = evaluate_guard(&negated, &wc, &config);
                prop_assert_ne!(result, negated_result);
            }

            /// And of a single condition equals the condition itself.
            #[test]
            fn and_single_identity(
                field in field_name(),
                wc_value in value_str(),
                test_value in value_str(),
            ) {
                let mut wc = Wildcards::new();
                wc.insert(field.clone(), wc_value);
                let config = Config::default();

                let inner = GuardExpr::Eq {
                    field,
                    value: test_value,
                };
                let and_single = GuardExpr::And {
                    conditions: vec![inner.clone()],
                };

                prop_assert_eq!(
                    evaluate_guard(&inner, &wc, &config),
                    evaluate_guard(&and_single, &wc, &config),
                );
            }

            /// Or of a single condition equals the condition itself.
            #[test]
            fn or_single_identity(
                field in field_name(),
                wc_value in value_str(),
                test_value in value_str(),
            ) {
                let mut wc = Wildcards::new();
                wc.insert(field.clone(), wc_value);
                let config = Config::default();

                let inner = GuardExpr::Eq {
                    field,
                    value: test_value,
                };
                let or_single = GuardExpr::Or {
                    conditions: vec![inner.clone()],
                };

                prop_assert_eq!(
                    evaluate_guard(&inner, &wc, &config),
                    evaluate_guard(&or_single, &wc, &config),
                );
            }

            /// Double negation is identity: Not(Not(x)) == x.
            #[test]
            fn double_negation_identity(
                field in field_name(),
                wc_value in value_str(),
                test_value in value_str(),
            ) {
                let mut wc = Wildcards::new();
                wc.insert(field.clone(), wc_value);
                let config = Config::default();

                let inner = GuardExpr::Eq {
                    field,
                    value: test_value,
                };
                let double_neg = GuardExpr::Not {
                    condition: Box::new(GuardExpr::Not {
                        condition: Box::new(inner.clone()),
                    }),
                };

                prop_assert_eq!(
                    evaluate_guard(&inner, &wc, &config),
                    evaluate_guard(&double_neg, &wc, &config),
                );
            }

            /// ConfigEq returns true iff the config scalar matches.
            #[test]
            fn config_eq_matches(
                key in field_name(),
                config_value in value_str(),
                test_value in value_str(),
            ) {
                let wc = Wildcards::new();
                let mut config = Config::default();
                config.scalars.insert(key.clone(), config_value.clone());

                let guard = GuardExpr::ConfigEq {
                    key,
                    value: test_value.clone(),
                };

                let expected = config_value == test_value;
                prop_assert_eq!(
                    evaluate_guard(&guard, &wc, &config),
                    expected
                );
            }
        }
    }

    #[test]
    #[serial]
    fn param_files_interpolated_with_wildcards() {
        let mut rule = make_rule("train", &["input.txt"], &["model_{ds}.bin"]);
        rule.param_files = vec!["config/{ds}.yaml".to_string()];

        let request = ResolveRequest {
            targets: vec!["model_v1.bin".into()],
            config: Config::default(),
            existing_files: vec![PathBuf::from("input.txt")],
        };

        let result = resolve(&[rule], &request).unwrap();
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(
            result.jobs[0].param_files,
            vec![PathBuf::from("config/v1.yaml")]
        );
    }
}
