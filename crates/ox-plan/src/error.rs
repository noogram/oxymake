//! Error types for optimization passes and the `ox plan` command.
//!
//! These variants carry a `source_snakefile_line` hint so that failures on
//! Oxymakefiles produced by `ox translate` can cite the line in the original
//! Snakefile (or other source) instead of leaving the operator to guess.
//!
//! ## Translated-workflow detection
//!
//! `ox translate` emits two artifacts side-by-side:
//!
//! - `Oxymakefile.toml`               — the translated workflow
//! - `Oxymakefile.toml.escalations.toml` — structured escalations for
//!   constructs the translator could not mechanise (silently dropped rules,
//!   dynamic params, etc.)
//!
//! The presence of the sidecar is the signal that a workflow was translated.
//! When a `no rule produces output: X` error fires, the producing rule may
//! have been *dropped* by the translator — the operator needs to consult
//! `.escalations.toml` to recover what was lost.  When a rule was preserved,
//! its `source_line` field carries the original Snakefile line through to
//! the error message.

use std::path::{Path, PathBuf};

use ox_core::error::{DagError, ParseError, WildcardError};
use thiserror::Error;

/// Errors surfaced by `ox plan` (validation, RuleGraph build, target
/// resolution, JobGraph build).
///
/// Every variant carries an optional `source_snakefile_line`. When the
/// workflow was produced by `ox translate`, the line is populated from the
/// failing rule's `source_line` (threaded through from
/// `ox-translate::snakemake::parser`) so error messages cite the original
/// source location.
#[derive(Debug, Error)]
pub enum PlanError {
    /// The workflow failed semantic validation (duplicate rules, bad output
    /// wildcards, …).  `messages` are the per-error strings from
    /// `ox-format::validate`.
    #[error("validation errors:\n  {}{}", .messages.join("\n  "), format_translation_hint(*source_snakefile_line, escalations_path.as_deref(), None))]
    Validation {
        messages: Vec<String>,
        source_snakefile_line: Option<usize>,
        escalations_path: Option<PathBuf>,
    },

    /// `RuleGraph::build` failed — usually a static cycle or a conflicting
    /// output between two rules.
    #[error("failed to build RuleGraph: {source}{}", format_translation_hint(*source_snakefile_line, escalations_path.as_deref(), None))]
    RuleGraphBuild {
        #[source]
        source: DagError,
        source_snakefile_line: Option<usize>,
        escalations_path: Option<PathBuf>,
    },

    /// Resolving the requested targets failed.  The most common variant is
    /// `WildcardError::NoProducer { path }`, which now gets an explicit
    /// "may have been dropped by translation — see .escalations.toml" hint
    /// when an escalation sidecar exists.
    #[error("failed to resolve targets: {source}{}", format_translation_hint(*source_snakefile_line, escalations_path.as_deref(), missing_path.as_deref()))]
    ResolveTargets {
        #[source]
        source: DagError,
        /// The output pattern that has no producer, when known.  Used to
        /// shape the hint differently for `NoProducer` vs other DAG errors.
        missing_path: Option<String>,
        source_snakefile_line: Option<usize>,
        escalations_path: Option<PathBuf>,
    },

    /// `JobGraph::build` failed — a dynamic cycle was discovered after
    /// wildcards were expanded.
    #[error("failed to build JobGraph: {source}{}", format_translation_hint(*source_snakefile_line, escalations_path.as_deref(), None))]
    JobGraphBuild {
        #[source]
        source: DagError,
        source_snakefile_line: Option<usize>,
        escalations_path: Option<PathBuf>,
    },
}

impl PlanError {
    /// Wrap a vector of `ParseError`s from `ox-format::validate`.
    ///
    /// `workflow_path` is the path to the Oxymakefile being planned; the
    /// `.escalations.toml` sidecar is looked up next to it.
    pub fn validation(errors: Vec<ParseError>, workflow_path: &Path) -> Self {
        let messages = errors.iter().map(|e| e.to_string()).collect();
        Self::Validation {
            messages,
            source_snakefile_line: None,
            escalations_path: locate_escalations(workflow_path),
        }
    }

    /// Wrap a `RuleGraph::build` failure.
    pub fn rule_graph_build(source: DagError, workflow_path: &Path) -> Self {
        Self::RuleGraphBuild {
            source,
            source_snakefile_line: None,
            escalations_path: locate_escalations(workflow_path),
        }
    }

    /// Wrap a `resolver::resolve` failure.  If the underlying error is
    /// `WildcardError::NoProducer`, the missing path is extracted so that
    /// the message can carry an explicit translation hint.
    pub fn resolve_targets(source: DagError, workflow_path: &Path) -> Self {
        let missing_path = match &source {
            DagError::Wildcard(
                WildcardError::NoProducer { path } | WildcardError::MissingSource { path },
            ) => Some(path.clone()),
            _ => None,
        };
        Self::ResolveTargets {
            source,
            missing_path,
            source_snakefile_line: None,
            escalations_path: locate_escalations(workflow_path),
        }
    }

    /// Wrap a `JobGraph::build` failure.
    pub fn job_graph_build(source: DagError, workflow_path: &Path) -> Self {
        Self::JobGraphBuild {
            source,
            source_snakefile_line: None,
            escalations_path: locate_escalations(workflow_path),
        }
    }
}

/// Locate the `.escalations.toml` sidecar next to a workflow path.
///
/// Returns the path if and only if the file exists on disk.  The presence
/// of this file is what tells `ox plan` that the Oxymakefile came from
/// `ox translate` and is therefore eligible for the "see escalations"
/// hint.
fn locate_escalations(workflow_path: &Path) -> Option<PathBuf> {
    let mut candidate = workflow_path.as_os_str().to_owned();
    candidate.push(".escalations.toml");
    let candidate = PathBuf::from(candidate);
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// Compose the trailing hint appended to every error message.
///
/// Three cases:
///
/// 1. A specific source line is known → "(originally produced by Snakefile:N)".
/// 2. A `NoProducer` error fired and escalations exist → "(producing rule
///    may have been dropped by translation — see <path>)".
/// 3. Escalations exist but the failure is not `NoProducer` →
///    "(workflow was translated by `ox translate` — see <path> for
///    structured escalations)".
///
/// Returns the empty string when there is nothing useful to add.
fn format_translation_hint(
    source_line: Option<usize>,
    escalations_path: Option<&Path>,
    missing_path: Option<&str>,
) -> String {
    if let Some(line) = source_line {
        return format!(" (originally produced by Snakefile:{line})");
    }
    let Some(esc) = escalations_path else {
        return String::new();
    };
    let esc_disp = esc.display();
    if let Some(missing) = missing_path {
        // Attempt to match the missing path against an escalation's
        // original_code / data_file to recover a specific source line.
        if let Some(line) = lookup_dropped_rule_line(esc, missing) {
            return format!(
                " (producing rule for `{missing}` may have been dropped by translation; \
                 see {esc_disp} — closest escalation at Snakefile:{line})"
            );
        }
        return format!(" (producing rule may have been dropped by translation — see {esc_disp})");
    }
    format!(
        " (workflow was translated by `ox translate` — see {esc_disp} for structured escalations)"
    )
}

/// Parse the `.escalations.toml` sidecar and try to find an escalation that
/// mentions `missing_path`.  Returns the escalation's `source_line` when a
/// match is found.
///
/// The matching is intentionally permissive: we look for substring matches
/// against `original_code`, `construct`, `directive_value`, and `data_file`.
/// A miss is silent — the surrounding code falls back to the generic hint.
fn lookup_dropped_rule_line(escalations_path: &Path, missing_path: &str) -> Option<usize> {
    let content = std::fs::read_to_string(escalations_path).ok()?;
    let parsed: EscalationsFile = toml::from_str(&content).ok()?;
    let needle = missing_path;
    parsed
        .escalation
        .into_iter()
        .find(|e| {
            let in_original = e.original_code.contains(needle);
            let in_construct = e.construct.contains(needle);
            let in_ctx = e.context.as_ref().is_some_and(|c| {
                c.directive_value
                    .as_deref()
                    .is_some_and(|v| v.contains(needle))
                    || c.data_file.as_deref().is_some_and(|d| d.contains(needle))
            });
            in_original || in_construct || in_ctx
        })
        .and_then(|e| e.source_line)
}

#[derive(serde::Deserialize)]
struct EscalationsFile {
    #[serde(default)]
    escalation: Vec<EscalationEntry>,
}

#[derive(serde::Deserialize)]
struct EscalationEntry {
    #[serde(default)]
    source_line: Option<usize>,
    #[serde(default)]
    original_code: String,
    #[serde(default)]
    construct: String,
    #[serde(default)]
    context: Option<EscalationCtx>,
}

#[derive(serde::Deserialize)]
struct EscalationCtx {
    #[serde(default)]
    directive_value: Option<String>,
    #[serde(default)]
    data_file: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn validation_error_without_translation_has_no_hint() {
        let err = PlanError::Validation {
            messages: vec!["duplicate rule `foo`".into()],
            source_snakefile_line: None,
            escalations_path: None,
        };
        let s = err.to_string();
        assert!(s.contains("duplicate rule `foo`"), "{s}");
        assert!(!s.contains("translated"), "{s}");
        assert!(!s.contains("Snakefile"), "{s}");
    }

    #[test]
    fn validation_error_with_source_line_cites_snakefile() {
        let err = PlanError::Validation {
            messages: vec!["duplicate rule `foo`".into()],
            source_snakefile_line: Some(42),
            escalations_path: None,
        };
        assert!(err.to_string().contains("Snakefile:42"));
    }

    #[test]
    fn no_producer_with_escalations_mentions_dropped_translation() {
        let err = PlanError::ResolveTargets {
            source: DagError::Wildcard(WildcardError::NoProducer {
                path: "results/out.txt".into(),
            }),
            missing_path: Some("results/out.txt".into()),
            source_snakefile_line: None,
            escalations_path: Some(PathBuf::from("Oxymakefile.toml.escalations.toml")),
        };
        let s = err.to_string();
        assert!(s.contains("results/out.txt"), "{s}");
        assert!(s.contains("dropped by translation"), "{s}");
        assert!(s.contains("Oxymakefile.toml.escalations.toml"), "{s}");
    }

    #[test]
    fn no_producer_finds_specific_dropped_rule_line() {
        let tmp = std::env::temp_dir().join("ox-plan-error-test-no-producer");
        let _ = std::fs::remove_dir_all(&tmp);
        let oxy = tmp.join("Oxymakefile.toml");
        let esc = tmp.join("Oxymakefile.toml.escalations.toml");
        write_temp(&oxy, "ox_version = \"0.1\"\n");
        write_temp(
            &esc,
            r#"
[meta]
total_escalations = 1
tier_counts = { mechanical_deferred = 0, assisted = 0, human = 1 }

[[escalation]]
id = "esc-0001"
tier = "Human"
category = "SilentDrop"
severity = "Correctness"
rule_name = "produce_results"
construct = "rule"
source_line = 17
original_code = """
rule produce_results:
    output: "results/out.txt"
    shell: "touch {output}"
"""
"#,
        );

        let err = PlanError::resolve_targets(
            DagError::Wildcard(WildcardError::NoProducer {
                path: "results/out.txt".into(),
            }),
            &oxy,
        );
        let s = err.to_string();
        assert!(
            s.contains("Snakefile:17"),
            "expected a specific Snakefile:17 reference, got: {s}"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn locate_escalations_returns_none_when_file_absent() {
        let tmp = std::env::temp_dir().join("ox-plan-error-test-absent");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let oxy = tmp.join("Oxymakefile.toml");
        std::fs::File::create(&oxy).unwrap();
        assert!(locate_escalations(&oxy).is_none());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn rule_graph_build_with_escalations_mentions_translation() {
        let err = PlanError::RuleGraphBuild {
            source: DagError::CycleDetected {
                cycle: vec!["a".into(), "b".into(), "a".into()],
            },
            source_snakefile_line: None,
            escalations_path: Some(PathBuf::from("Oxymakefile.toml.escalations.toml")),
        };
        let s = err.to_string();
        assert!(s.contains("cycle"), "{s}");
        assert!(s.contains("translated by `ox translate`"), "{s}");
    }
}
