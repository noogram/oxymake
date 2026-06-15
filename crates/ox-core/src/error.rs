//! Error types for OxyMake, organized by domain.
//!
//! Each error variant carries enough context to build causal-chain error
//! messages: *what* went wrong, *where* it happened, and *why* it matters.

use std::path::PathBuf;
use thiserror::Error;

/// Errors during wildcard pattern parsing and resolution.
#[derive(Debug, Error)]
pub enum WildcardError {
    #[error("invalid wildcard pattern `{pattern}`: {reason}")]
    InvalidPattern { pattern: String, reason: String },

    #[error(
        "wildcard `{name}` value `{value}` does not match constraint `{constraint}` in pattern `{pattern}`"
    )]
    ConstraintViolation {
        name: String,
        value: String,
        pattern: String,
        constraint: String,
    },

    #[error("no rule produces output matching `{path}`")]
    NoProducer { path: String },

    #[error(
        "`{path}` is not produced by any rule and does not exist on disk; \
         add a rule whose `output` matches it, or create it as a source file"
    )]
    MissingSource { path: String },

    #[error("multiple rules can produce `{path}`: {rules:?}")]
    AmbiguousProducer { path: String, rules: Vec<String> },

    #[error("wildcard `{name}` could not be resolved from config or filesystem")]
    UnresolvableWildcard { name: String },

    #[error("unknown config key `{key}` in pattern `{pattern}` — not defined in [config]")]
    UnknownConfigKey { key: String, pattern: String },
}

/// Errors during Oxymakefile parsing.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("TOML parse error in {file}: {message}")]
    Toml { file: PathBuf, message: String },

    #[error("unknown field `{field}` in rule `{rule}`")]
    UnknownField { rule: String, field: String },

    #[error("rule `{rule}` must have exactly one of: shell, run, script, call")]
    MissingExecution { rule: String },

    #[error("rule `{rule}` has both `{a}` and `{b}` — only one execution mode allowed")]
    ConflictingExecution { rule: String, a: String, b: String },

    #[error("include file not found: {path}")]
    IncludeNotFound { path: PathBuf },

    #[error("circular include detected: {chain:?}")]
    CircularInclude { chain: Vec<PathBuf> },

    #[error("duplicate rule name `{name}` (first defined in {first}, redefined in {second})")]
    DuplicateRule {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },

    #[error("unsupported ox_version `{version}` (supported: {supported:?})")]
    UnsupportedVersion {
        version: String,
        supported: Vec<String>,
    },

    #[error("invalid field `{field}`: {reason}")]
    InvalidField { field: String, reason: String },

    #[error("config `{key}`: cannot read source file `{path}`: {reason}")]
    ConfigFileRead {
        key: String,
        path: PathBuf,
        reason: String,
    },

    #[error("config `{key}`: column `{column}` not found in `{path}` (available: {available})")]
    ConfigColumnNotFound {
        key: String,
        path: PathBuf,
        column: String,
        available: String,
    },
}

/// An invalid hash literal was rejected at the type boundary.
///
/// Raised by `ContentHash::from_hex` / `ComputationHash::from_hex` (and
/// their deserializers) when a value is not a 64-character lowercase hex
/// string — forged, truncated or uppercase hashes never enter the system.
#[derive(Debug, Error)]
#[error("invalid {kind} `{value}`: expected a 64-character lowercase hex string")]
pub struct InvalidHashError {
    /// Which hash kind rejected the value (`content hash` / `computation hash`).
    pub kind: &'static str,
    /// The rejected value.
    pub value: String,
}

/// Errors during DAG construction and validation.
#[derive(Debug, Error)]
pub enum DagError {
    #[error("cycle detected in DAG: {cycle:?}")]
    CycleDetected { cycle: Vec<String> },

    #[error("missing source file: `{path}` (required by rule `{rule}`)")]
    MissingSource { path: PathBuf, rule: String },

    #[error("rule `{a}` and rule `{b}` both produce `{output}`")]
    ConflictingOutputs {
        a: String,
        b: String,
        output: String,
    },

    #[error("corrupted graph state: {detail}")]
    CorruptedGraph { detail: String },

    #[error(
        "dependency chain exceeds the maximum resolution depth of {limit} \
         (while resolving `{target}`) — the workflow has an unusually deep \
         input chain; restructure it or split the pipeline"
    )]
    DependencyChainTooDeep { target: String, limit: usize },

    #[error(
        "duplicate job id `{id}` minted for rules `{a}` and `{b}` — \
         one of the jobs would silently shadow the other"
    )]
    DuplicateJobId { id: String, a: String, b: String },

    #[error(transparent)]
    Wildcard(#[from] WildcardError),
}

/// Errors during job execution.
#[derive(Debug, Error)]
pub enum ExecError {
    #[error("job `{job_id}` (rule `{rule}`) failed with exit code {exit_code}")]
    JobFailed {
        job_id: String,
        rule: String,
        exit_code: i32,
        stderr_tail: String,
    },

    #[error("job `{job_id}` (rule `{rule}`) timed out after {timeout_secs}s")]
    Timeout {
        job_id: String,
        rule: String,
        timeout_secs: u64,
    },

    #[error("job `{job_id}` (rule `{rule}`) killed by signal {signal}")]
    Killed {
        job_id: String,
        rule: String,
        signal: i32,
    },

    #[error("job `{job_id}` did not create expected output: {missing_outputs:?}")]
    MissingOutputs {
        job_id: String,
        missing_outputs: Vec<PathBuf>,
    },

    #[error("executor error: {message}")]
    Executor { message: String },
}

/// Errors during cache operations.
#[derive(Debug, Error)]
pub enum CacheError {
    #[error("cache corruption: {path} has expected hash {expected} but actual hash {actual}")]
    Corruption {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("I/O error accessing cache: {0}")]
    Io(#[from] std::io::Error),
}

/// Top-level error type for OxyMake operations.
#[derive(Debug, Error)]
pub enum OxError {
    #[error(transparent)]
    Parse(#[from] ParseError),

    #[error(transparent)]
    Wildcard(#[from] WildcardError),

    #[error(transparent)]
    Dag(#[from] DagError),

    #[error(transparent)]
    Exec(#[from] ExecError),

    #[error(transparent)]
    Cache(#[from] CacheError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Display tests ────────────────────────────────────────────────

    #[test]
    fn wildcard_invalid_pattern_display() {
        let err = WildcardError::InvalidPattern {
            pattern: "{{bad".into(),
            reason: "unmatched brace".into(),
        };
        assert_eq!(
            err.to_string(),
            "invalid wildcard pattern `{{bad`: unmatched brace"
        );
    }

    #[test]
    fn wildcard_constraint_violation_display() {
        let err = WildcardError::ConstraintViolation {
            name: "country".into(),
            value: "ZZZ".into(),
            pattern: "data/{country}/pop.csv".into(),
            constraint: "iso3166".into(),
        };
        assert!(err.to_string().contains("country"));
        assert!(err.to_string().contains("ZZZ"));
        assert!(err.to_string().contains("iso3166"));
    }

    #[test]
    fn wildcard_no_producer_display() {
        let err = WildcardError::NoProducer {
            path: "out/missing.txt".into(),
        };
        assert_eq!(
            err.to_string(),
            "no rule produces output matching `out/missing.txt`"
        );
    }

    #[test]
    fn wildcard_ambiguous_producer_display() {
        let err = WildcardError::AmbiguousProducer {
            path: "data/x.csv".into(),
            rules: vec!["rule_a".into(), "rule_b".into()],
        };
        let msg = err.to_string();
        assert!(msg.contains("data/x.csv"));
        assert!(msg.contains("rule_a"));
        assert!(msg.contains("rule_b"));
    }

    #[test]
    fn wildcard_unresolvable_display() {
        let err = WildcardError::UnresolvableWildcard {
            name: "year".into(),
        };
        assert!(err.to_string().contains("year"));
    }

    #[test]
    fn wildcard_unknown_config_key_display() {
        let err = WildcardError::UnknownConfigKey {
            key: "results_dir".into(),
            pattern: "{config.results_dir}/data.csv".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("results_dir"));
        assert!(msg.contains("[config]"));
    }

    #[test]
    fn parse_toml_display() {
        let err = ParseError::Toml {
            file: PathBuf::from("Oxymakefile.toml"),
            message: "expected `=`".into(),
        };
        assert!(err.to_string().contains("Oxymakefile.toml"));
        assert!(err.to_string().contains("expected `=`"));
    }

    #[test]
    fn parse_unknown_field_display() {
        let err = ParseError::UnknownField {
            rule: "build".into(),
            field: "shel".into(),
        };
        assert_eq!(err.to_string(), "unknown field `shel` in rule `build`");
    }

    #[test]
    fn parse_missing_execution_display() {
        let err = ParseError::MissingExecution {
            rule: "compile".into(),
        };
        assert!(err.to_string().contains("compile"));
        assert!(err.to_string().contains("shell, run, script, call"));
    }

    #[test]
    fn parse_conflicting_execution_display() {
        let err = ParseError::ConflictingExecution {
            rule: "build".into(),
            a: "shell".into(),
            b: "run".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("shell"));
        assert!(msg.contains("run"));
        assert!(msg.contains("build"));
    }

    #[test]
    fn parse_include_not_found_display() {
        let err = ParseError::IncludeNotFound {
            path: PathBuf::from("missing.toml"),
        };
        assert!(err.to_string().contains("missing.toml"));
    }

    #[test]
    fn parse_circular_include_display() {
        let err = ParseError::CircularInclude {
            chain: vec![PathBuf::from("a.toml"), PathBuf::from("b.toml")],
        };
        assert!(err.to_string().contains("a.toml"));
    }

    #[test]
    fn parse_duplicate_rule_display() {
        let err = ParseError::DuplicateRule {
            name: "build".into(),
            first: PathBuf::from("a.toml"),
            second: PathBuf::from("b.toml"),
        };
        let msg = err.to_string();
        assert!(msg.contains("build"));
        assert!(msg.contains("a.toml"));
        assert!(msg.contains("b.toml"));
    }

    #[test]
    fn parse_unsupported_version_display() {
        let err = ParseError::UnsupportedVersion {
            version: "99.0".into(),
            supported: vec!["1.0".into()],
        };
        assert!(err.to_string().contains("99.0"));
    }

    #[test]
    fn dag_cycle_detected_display() {
        let err = DagError::CycleDetected {
            cycle: vec!["a".into(), "b".into(), "a".into()],
        };
        assert!(err.to_string().contains("cycle detected"));
    }

    #[test]
    fn dag_missing_source_display() {
        let err = DagError::MissingSource {
            path: PathBuf::from("input.csv"),
            rule: "transform".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("input.csv"));
        assert!(msg.contains("transform"));
    }

    #[test]
    fn dag_conflicting_outputs_display() {
        let err = DagError::ConflictingOutputs {
            a: "rule1".into(),
            b: "rule2".into(),
            output: "out.csv".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("rule1"));
        assert!(msg.contains("rule2"));
        assert!(msg.contains("out.csv"));
    }

    #[test]
    fn exec_job_failed_display() {
        let err = ExecError::JobFailed {
            job_id: "j-001".into(),
            rule: "compile".into(),
            exit_code: 1,
            stderr_tail: "error: something broke".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("j-001"));
        assert!(msg.contains("compile"));
        assert!(msg.contains("exit code 1"));
    }

    #[test]
    fn exec_timeout_display() {
        let err = ExecError::Timeout {
            job_id: "j-002".into(),
            rule: "slow_task".into(),
            timeout_secs: 300,
        };
        assert!(err.to_string().contains("300s"));
    }

    #[test]
    fn exec_killed_display() {
        let err = ExecError::Killed {
            job_id: "j-003".into(),
            rule: "build".into(),
            signal: 9,
        };
        assert!(err.to_string().contains("signal 9"));
    }

    #[test]
    fn exec_missing_outputs_display() {
        let err = ExecError::MissingOutputs {
            job_id: "j-004".into(),
            missing_outputs: vec![PathBuf::from("out/a.txt")],
        };
        assert!(err.to_string().contains("out/a.txt"));
    }

    #[test]
    fn exec_executor_display() {
        let err = ExecError::Executor {
            message: "docker daemon unavailable".into(),
        };
        assert!(err.to_string().contains("docker daemon unavailable"));
    }

    #[test]
    fn cache_corruption_display() {
        let err = CacheError::Corruption {
            path: PathBuf::from("cache/abc"),
            expected: "aaa".into(),
            actual: "bbb".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("aaa"));
        assert!(msg.contains("bbb"));
    }

    #[test]
    fn cache_io_display() {
        let err = CacheError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert!(err.to_string().contains("gone"));
    }

    // ── From conversion tests ────────────────────────────────────────

    #[test]
    fn ox_error_from_parse() {
        let inner = ParseError::MissingExecution { rule: "r".into() };
        let outer: OxError = inner.into();
        assert!(matches!(outer, OxError::Parse(_)));
        assert!(outer.to_string().contains("r"));
    }

    #[test]
    fn ox_error_from_wildcard() {
        let inner = WildcardError::NoProducer { path: "x".into() };
        let outer: OxError = inner.into();
        assert!(matches!(outer, OxError::Wildcard(_)));
    }

    #[test]
    fn ox_error_from_dag() {
        let inner = DagError::CycleDetected {
            cycle: vec!["a".into()],
        };
        let outer: OxError = inner.into();
        assert!(matches!(outer, OxError::Dag(_)));
    }

    #[test]
    fn ox_error_from_exec() {
        let inner = ExecError::Executor {
            message: "boom".into(),
        };
        let outer: OxError = inner.into();
        assert!(matches!(outer, OxError::Exec(_)));
    }

    #[test]
    fn ox_error_from_cache() {
        let inner = CacheError::Io(std::io::Error::new(std::io::ErrorKind::Other, "disk full"));
        let outer: OxError = inner.into();
        assert!(matches!(outer, OxError::Cache(_)));
    }

    #[test]
    fn ox_error_from_io() {
        let inner = std::io::Error::new(std::io::ErrorKind::Other, "oops");
        let outer: OxError = inner.into();
        assert!(matches!(outer, OxError::Io(_)));
    }

    // ── DagError from WildcardError ─────────────────────────────────

    #[test]
    fn dag_error_from_wildcard() {
        let inner = WildcardError::NoProducer {
            path: "missing.txt".into(),
        };
        let outer: DagError = inner.into();
        assert!(matches!(outer, DagError::Wildcard(_)));
        assert!(outer.to_string().contains("missing.txt"));
    }

    // ── OxError display for transparent variants ────────────────────

    #[test]
    fn ox_error_display_transparent_parse() {
        let inner = ParseError::MissingExecution {
            rule: "build".into(),
        };
        let outer: OxError = inner.into();
        let msg = outer.to_string();
        assert!(msg.contains("build"));
    }

    #[test]
    fn ox_error_display_transparent_dag() {
        let inner = DagError::CycleDetected {
            cycle: vec!["a".into(), "b".into()],
        };
        let outer: OxError = inner.into();
        assert!(outer.to_string().contains("cycle"));
    }

    #[test]
    fn ox_error_display_transparent_exec() {
        let inner = ExecError::Executor {
            message: "connection lost".into(),
        };
        let outer: OxError = inner.into();
        assert!(outer.to_string().contains("connection lost"));
    }

    #[test]
    fn ox_error_display_io() {
        let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "not here");
        let outer: OxError = inner.into();
        assert!(outer.to_string().contains("not here"));
    }
}
