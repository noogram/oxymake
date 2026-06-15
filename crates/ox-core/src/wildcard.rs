//! Wildcard pattern parsing, matching, and interpolation.
//!
//! OxyMake patterns use `{name}` syntax for wildcards, e.g.
//! `"results/{sample}/{method}.bam"`. This module provides:
//!
//! - **Parsing**: Convert a raw pattern string into a [`Pattern`].
//! - **Resolution**: Match a concrete path against a pattern, extracting
//!   wildcard values into a [`Wildcards`] map.
//! - **Interpolation**: Substitute wildcard values into a pattern to produce
//!   a concrete path.
//! - **Constraints**: Optional per-wildcard regex constraints.

use std::collections::BTreeMap;
use std::fmt;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::WildcardError;

/// Resolved wildcard values: a map from wildcard name to its concrete value.
pub type Wildcards = BTreeMap<String, String>;

/// A parsed segment of a wildcard pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Segment {
    /// A literal string (no wildcards).
    Literal(String),
    /// A wildcard placeholder — the name inside `{braces}`.
    Wildcard(String),
}

/// A parsed wildcard pattern like `"results/{sample}/{method}.bam"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    /// The raw pattern string as written by the user.
    raw: String,
    /// Parsed segments (alternating literals and wildcards).
    segments: Vec<Segment>,
}

/// A pattern paired with its pre-compiled regex for fast repeated matching.
///
/// Use this when the same pattern + constraints will be matched against many
/// target strings (e.g., `find_producer` scanning 50K targets). Compiling the
/// regex once and reusing it avoids the dominant cost in large-scale resolution.
#[derive(Debug, Clone)]
pub struct CompiledPattern {
    pattern: Pattern,
    regex: Regex,
}

impl CompiledPattern {
    /// Compile a pattern with the given wildcard constraints.
    ///
    /// The regex is built once and reused for all subsequent `resolve` calls.
    pub fn new(
        pattern: Pattern,
        constraints: &BTreeMap<String, String>,
    ) -> Result<Self, WildcardError> {
        let regex = pattern.compile_regex(constraints)?;
        Ok(Self { pattern, regex })
    }

    /// Match against a concrete path using the pre-compiled regex.
    pub fn resolve(&self, path: &str) -> Option<Wildcards> {
        let caps = self.regex.captures(path)?;
        extract_wildcards(&self.pattern.wildcard_names(), &caps)
    }

    /// The underlying pattern.
    pub fn pattern(&self) -> &Pattern {
        &self.pattern
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw && self.segments == other.segments
    }
}

impl Eq for Pattern {}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl Pattern {
    /// Parse a pattern string into a `Pattern`.
    ///
    /// Returns [`WildcardError::InvalidPattern`] for malformed patterns
    /// (unmatched braces, empty wildcard names, adjacent wildcards).
    pub fn parse(raw: &str) -> Result<Self, WildcardError> {
        let mut segments = Vec::new();
        let mut chars = raw.chars().peekable();
        let mut literal_buf = String::new();

        while let Some(&ch) = chars.peek() {
            match ch {
                '{' => {
                    chars.next();
                    // Check for escaped brace `{{`
                    if chars.peek() == Some(&'{') {
                        chars.next();
                        // Consume until `}}`
                        let mut inner = String::new();
                        loop {
                            match chars.next() {
                                Some('}') if chars.peek() == Some(&'}') => {
                                    chars.next();
                                    break;
                                }
                                Some(c) => inner.push(c),
                                None => {
                                    return Err(WildcardError::InvalidPattern {
                                        pattern: raw.to_owned(),
                                        reason: "unmatched `{{` — expected `}}`".into(),
                                    });
                                }
                            }
                        }
                        // Escaped braces produce a literal
                        literal_buf.push('{');
                        literal_buf.push_str(&inner);
                        literal_buf.push('}');
                    } else {
                        // Regular wildcard `{name}`
                        let mut name = String::new();
                        loop {
                            match chars.next() {
                                Some('}') => break,
                                Some(c) => name.push(c),
                                None => {
                                    return Err(WildcardError::InvalidPattern {
                                        pattern: raw.to_owned(),
                                        reason: "unmatched `{` — expected `}`".into(),
                                    });
                                }
                            }
                        }
                        if name.is_empty() {
                            return Err(WildcardError::InvalidPattern {
                                pattern: raw.to_owned(),
                                reason: "empty wildcard name `{}`".into(),
                            });
                        }
                        // Validate wildcard name: must be alphanumeric/underscore
                        if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                            return Err(WildcardError::InvalidPattern {
                                pattern: raw.to_owned(),
                                reason: format!(
                                    "wildcard name `{name}` contains invalid characters"
                                ),
                            });
                        }
                        // Flush any accumulated literal
                        if !literal_buf.is_empty() {
                            segments.push(Segment::Literal(literal_buf.clone()));
                            literal_buf.clear();
                        }
                        // Check for adjacent wildcards
                        if let Some(Segment::Wildcard(_)) = segments.last() {
                            return Err(WildcardError::InvalidPattern {
                                pattern: raw.to_owned(),
                                reason: "adjacent wildcards are ambiguous — \
                                         add a literal separator between them"
                                    .into(),
                            });
                        }
                        segments.push(Segment::Wildcard(name));
                    }
                }
                '}' => {
                    chars.next();
                    // A lone `}` without a matching `{` is invalid
                    return Err(WildcardError::InvalidPattern {
                        pattern: raw.to_owned(),
                        reason: "unmatched `}` without opening `{`".into(),
                    });
                }
                _ => {
                    chars.next();
                    literal_buf.push(ch);
                }
            }
        }

        // Flush trailing literal
        if !literal_buf.is_empty() {
            segments.push(Segment::Literal(literal_buf));
        }

        Ok(Pattern {
            raw: raw.to_owned(),
            segments,
        })
    }

    /// Get the wildcard names in this pattern, in order of appearance.
    pub fn wildcard_names(&self) -> Vec<&str> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                Segment::Wildcard(name) => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Check if this pattern contains any wildcards.
    pub fn has_wildcards(&self) -> bool {
        self.segments
            .iter()
            .any(|seg| matches!(seg, Segment::Wildcard(_)))
    }

    /// The raw pattern string.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// The parsed segments.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Match this pattern against a concrete path, extracting wildcard values.
    ///
    /// Returns `None` if the path doesn't match.
    pub fn resolve(&self, path: &str) -> Option<Wildcards> {
        let constraints = BTreeMap::new();
        self.resolve_with_constraints(path, &constraints).ok()?
    }

    /// Match with wildcard constraints (regex per wildcard name).
    ///
    /// Each entry in `constraints` maps a wildcard name to a regex pattern
    /// that the extracted value must fully match.
    pub fn resolve_with_constraints(
        &self,
        path: &str,
        constraints: &BTreeMap<String, String>,
    ) -> Result<Option<Wildcards>, WildcardError> {
        let regex = self.compile_regex(constraints)?;
        let caps = match regex.captures(path) {
            Some(c) => c,
            None => return Ok(None),
        };

        Ok(extract_wildcards(&self.wildcard_names(), &caps))
    }

    /// Interpolate wildcard values into the pattern to produce a concrete path.
    ///
    /// Returns an error if a required wildcard is missing from `wildcards`.
    pub fn interpolate(&self, wildcards: &Wildcards) -> Result<String, WildcardError> {
        let mut result = String::new();
        for seg in &self.segments {
            match seg {
                Segment::Literal(lit) => result.push_str(lit),
                Segment::Wildcard(name) => {
                    let value =
                        wildcards
                            .get(name)
                            .ok_or_else(|| WildcardError::UnresolvableWildcard {
                                name: name.clone(),
                            })?;
                    result.push_str(value);
                }
            }
        }
        Ok(result)
    }

    /// Compile the pattern to a regex for matching.
    ///
    /// Each wildcard occurrence is captured in a *named* group `w0`, `w1`, …
    /// (in order of appearance) so that capturing groups inside a user
    /// constraint cannot shift the extraction of subsequent wildcards.
    /// If a wildcard has a constraint, the group wraps that regex; otherwise
    /// it captures one or more non-`/` characters.
    fn compile_regex(
        &self,
        constraints: &BTreeMap<String, String>,
    ) -> Result<Regex, WildcardError> {
        let mut regex_str = String::from("^");
        let mut occurrence = 0usize;
        for seg in &self.segments {
            match seg {
                Segment::Literal(lit) => {
                    regex_str.push_str(&regex::escape(lit));
                }
                Segment::Wildcard(name) => {
                    if let Some(constraint) = constraints.get(name) {
                        // Validate the constraint is a valid regex
                        Regex::new(constraint).map_err(|_| WildcardError::InvalidPattern {
                            pattern: self.raw.clone(),
                            reason: format!(
                                "invalid regex constraint `{constraint}` for wildcard `{name}`"
                            ),
                        })?;
                        regex_str.push_str(&format!("(?P<w{occurrence}>{constraint})"));
                    } else {
                        // Default: match one or more chars that are not `/`
                        regex_str.push_str(&format!("(?P<w{occurrence}>[^/]+)"));
                    }
                    occurrence += 1;
                }
            }
        }
        regex_str.push('$');

        // Safety: individual constraints are validated above, and literal
        // segments are regex-escaped, so the combined pattern is always valid.
        let compiled = Regex::new(&regex_str)
            .expect("BUG: composed regex must be valid — constraints are pre-validated");

        Ok(compiled)
    }
}

/// Extract wildcard values from regex captures.
///
/// `names` lists the wildcard occurrences in order of appearance; occurrence
/// `i` was captured by the named group `w{i}` (see [`Pattern::compile_regex`]).
/// Extraction by group *name* is immune to capturing groups inside user
/// constraints (which would shift positional indices).
///
/// A wildcard repeated in the pattern (`{x}/{x}.txt`) must capture the same
/// value at every occurrence (Snakemake semantics); on disagreement the
/// whole match is rejected with `None`.
fn extract_wildcards(names: &[&str], caps: &regex::Captures<'_>) -> Option<Wildcards> {
    let mut wildcards = Wildcards::new();
    for (i, name) in names.iter().enumerate() {
        let value = caps
            .name(&format!("w{i}"))
            .map(|m| m.as_str().to_owned())
            .unwrap_or_default();
        if let Some(previous) = wildcards.get(*name) {
            if *previous != value {
                return None;
            }
        }
        wildcards.insert((*name).to_owned(), value);
    }
    Some(wildcards)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Parsing ──────────────────────────────────────────────────────────

    #[test]
    fn parse_basic_pattern() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        assert_eq!(
            pat.segments(),
            &[
                Segment::Literal("results/".into()),
                Segment::Wildcard("sample".into()),
                Segment::Literal(".bam".into()),
            ]
        );
    }

    #[test]
    fn parse_multiple_wildcards() {
        let pat = Pattern::parse("results/{sample}/{method}.txt").unwrap();
        assert_eq!(
            pat.segments(),
            &[
                Segment::Literal("results/".into()),
                Segment::Wildcard("sample".into()),
                Segment::Literal("/".into()),
                Segment::Wildcard("method".into()),
                Segment::Literal(".txt".into()),
            ]
        );
    }

    #[test]
    fn parse_no_wildcards() {
        let pat = Pattern::parse("data/genome.fa").unwrap();
        assert_eq!(pat.segments(), &[Segment::Literal("data/genome.fa".into())]);
        assert!(!pat.has_wildcards());
    }

    #[test]
    fn parse_adjacent_wildcards_errors() {
        let err = Pattern::parse("{a}{b}").unwrap_err();
        assert!(matches!(&err, WildcardError::InvalidPattern { .. }));
        assert!(err.to_string().contains("adjacent"), "got: {err}");
    }

    #[test]
    fn parse_escaped_braces() {
        let pat = Pattern::parse("results/{{literal}}.txt").unwrap();
        // `{{literal}}` should become a literal segment containing `{literal}`
        assert_eq!(
            pat.segments(),
            &[Segment::Literal("results/{literal}.txt".into())]
        );
        assert!(!pat.has_wildcards());
    }

    #[test]
    fn parse_empty_wildcard_errors() {
        let err = Pattern::parse("results/{}.bam").unwrap_err();
        assert!(matches!(&err, WildcardError::InvalidPattern { .. }));
        assert!(err.to_string().contains("empty"), "got: {err}");
    }

    #[test]
    fn parse_unmatched_open_brace() {
        let err = Pattern::parse("results/{sample.bam").unwrap_err();
        assert!(matches!(&err, WildcardError::InvalidPattern { .. }));
        assert!(err.to_string().contains("unmatched"), "got: {err}");
    }

    #[test]
    fn parse_unmatched_close_brace() {
        let err = Pattern::parse("results/sample}.bam").unwrap_err();
        assert!(matches!(&err, WildcardError::InvalidPattern { .. }));
        assert!(err.to_string().contains("unmatched"), "got: {err}");
    }

    // ── wildcard_names / has_wildcards ───────────────────────────────────

    #[test]
    fn wildcard_names_returns_correct_order() {
        let pat = Pattern::parse("results/{sample}/{method}.txt").unwrap();
        assert_eq!(pat.wildcard_names(), vec!["sample", "method"]);
    }

    #[test]
    fn wildcard_names_empty_for_literal() {
        let pat = Pattern::parse("data/genome.fa").unwrap();
        assert!(pat.wildcard_names().is_empty());
    }

    #[test]
    fn has_wildcards_true() {
        let pat = Pattern::parse("{x}.txt").unwrap();
        assert!(pat.has_wildcards());
    }

    #[test]
    fn has_wildcards_false() {
        let pat = Pattern::parse("plain.txt").unwrap();
        assert!(!pat.has_wildcards());
    }

    // ── Resolution ──────────────────────────────────────────────────────

    #[test]
    fn resolve_basic() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        let wc = pat.resolve("results/patient_42.bam").unwrap();
        assert_eq!(wc.get("sample").unwrap(), "patient_42");
    }

    #[test]
    fn resolve_multiple_wildcards() {
        let pat = Pattern::parse("results/{sample}/{method}.txt").unwrap();
        let wc = pat.resolve("results/patient_42/bwa_mem.txt").unwrap();
        assert_eq!(wc.get("sample").unwrap(), "patient_42");
        assert_eq!(wc.get("method").unwrap(), "bwa_mem");
    }

    #[test]
    fn resolve_no_match_returns_none() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        assert!(pat.resolve("data/patient_42.bam").is_none());
    }

    #[test]
    fn resolve_literal_pattern() {
        let pat = Pattern::parse("data/genome.fa").unwrap();
        let wc = pat.resolve("data/genome.fa").unwrap();
        assert!(wc.is_empty());
    }

    #[test]
    fn resolve_literal_pattern_no_match() {
        let pat = Pattern::parse("data/genome.fa").unwrap();
        assert!(pat.resolve("data/other.fa").is_none());
    }

    #[test]
    fn resolve_wildcard_does_not_cross_slash() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        // The wildcard should NOT match across `/`
        assert!(pat.resolve("results/sub/dir.bam").is_none());
    }

    // ── Interpolation ───────────────────────────────────────────────────

    #[test]
    fn interpolate_basic() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "patient_42".into());
        assert_eq!(pat.interpolate(&wc).unwrap(), "results/patient_42.bam");
    }

    #[test]
    fn interpolate_multiple_wildcards() {
        let pat = Pattern::parse("results/{sample}/{method}.txt").unwrap();
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "patient_42".into());
        wc.insert("method".into(), "bwa_mem".into());
        assert_eq!(
            pat.interpolate(&wc).unwrap(),
            "results/patient_42/bwa_mem.txt"
        );
    }

    #[test]
    fn interpolate_missing_wildcard_errors() {
        let pat = Pattern::parse("results/{sample}/{method}.txt").unwrap();
        let mut wc = Wildcards::new();
        wc.insert("sample".into(), "patient_42".into());
        // "method" is missing
        let err = pat.interpolate(&wc).unwrap_err();
        assert!(matches!(&err, WildcardError::UnresolvableWildcard { .. }));
        assert!(err.to_string().contains("method"), "got: {err}");
    }

    #[test]
    fn interpolate_literal_pattern() {
        let pat = Pattern::parse("data/genome.fa").unwrap();
        let wc = Wildcards::new();
        assert_eq!(pat.interpolate(&wc).unwrap(), "data/genome.fa");
    }

    // ── Constraints ─────────────────────────────────────────────────────

    #[test]
    fn resolve_with_matching_constraint() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        let mut constraints = BTreeMap::new();
        constraints.insert("sample".into(), "[a-z_0-9]+".into());
        let wc = pat
            .resolve_with_constraints("results/patient_42.bam", &constraints)
            .unwrap()
            .unwrap();
        assert_eq!(wc.get("sample").unwrap(), "patient_42");
    }

    #[test]
    fn resolve_with_non_matching_constraint() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        let mut constraints = BTreeMap::new();
        constraints.insert("sample".into(), "[A-Z]+".into());
        // "patient_42" does not match `[A-Z]+`
        let result = pat
            .resolve_with_constraints("results/patient_42.bam", &constraints)
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn constraint_with_capturing_group_does_not_shift_following_wildcards() {
        // H1: a user constraint containing a capturing group like `(foo|bar)`
        // must not shift the positional indexing of subsequent wildcards.
        let pat = Pattern::parse("results/{a}/{b}.txt").unwrap();
        let mut constraints = BTreeMap::new();
        constraints.insert("a".into(), "(foo|bar)".into());
        let wc = pat
            .resolve_with_constraints("results/foo/x.txt", &constraints)
            .unwrap()
            .unwrap();
        assert_eq!(wc.get("a").unwrap(), "foo");
        assert_eq!(wc.get("b").unwrap(), "x");
    }

    #[test]
    fn compiled_pattern_constraint_with_capturing_group() {
        // H1: same guarantee on the pre-compiled fast path.
        let pat = Pattern::parse("results/{a}/{b}.txt").unwrap();
        let mut constraints = BTreeMap::new();
        constraints.insert("a".into(), "(foo|bar)".into());
        let compiled = CompiledPattern::new(pat, &constraints).unwrap();
        let wc = compiled.resolve("results/bar/value.txt").unwrap();
        assert_eq!(wc.get("a").unwrap(), "bar");
        assert_eq!(wc.get("b").unwrap(), "value");
    }

    // ── Repeated wildcards (H2) ──────────────────────────────────────────

    #[test]
    fn repeated_wildcard_requires_equal_values() {
        // H2: `{x}/{x}.txt` must only match when both occurrences agree
        // (Snakemake semantics). Previously the last occurrence silently won.
        let pat = Pattern::parse("{x}/{x}.txt").unwrap();
        assert!(pat.resolve("a/b.txt").is_none());
        let wc = pat.resolve("a/a.txt").unwrap();
        assert_eq!(wc.get("x").unwrap(), "a");
    }

    #[test]
    fn repeated_wildcard_compiled_requires_equal_values() {
        let pat = Pattern::parse("{x}/{x}.txt").unwrap();
        let compiled = CompiledPattern::new(pat, &BTreeMap::new()).unwrap();
        assert!(compiled.resolve("a/b.txt").is_none());
        let wc = compiled.resolve("s1/s1.txt").unwrap();
        assert_eq!(wc.get("x").unwrap(), "s1");
    }

    #[test]
    fn repeated_wildcard_mixed_with_other_wildcards() {
        let pat = Pattern::parse("{s}/{m}/{s}.bam").unwrap();
        assert!(pat.resolve("A/bwa/B.bam").is_none());
        let wc = pat.resolve("A/bwa/A.bam").unwrap();
        assert_eq!(wc.get("s").unwrap(), "A");
        assert_eq!(wc.get("m").unwrap(), "bwa");
    }

    #[test]
    fn resolve_with_invalid_constraint_regex() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        let mut constraints = BTreeMap::new();
        constraints.insert("sample".into(), "[invalid".into());
        let err = pat
            .resolve_with_constraints("results/x.bam", &constraints)
            .unwrap_err();
        assert!(matches!(&err, WildcardError::InvalidPattern { .. }));
        assert!(err.to_string().contains("invalid regex"), "got: {err}");
    }

    // ── Display ─────────────────────────────────────────────────────────

    #[test]
    fn display_shows_raw_pattern() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        assert_eq!(format!("{pat}"), "results/{sample}.bam");
    }

    // ── Round-trip: resolve then interpolate ────────────────────────────

    #[test]
    fn roundtrip_resolve_interpolate() {
        let pat = Pattern::parse("results/{sample}/{method}.bam").unwrap();
        let path = "results/patient_42/bwa_mem.bam";
        let wc = pat.resolve(path).unwrap();
        let reconstructed = pat.interpolate(&wc).unwrap();
        assert_eq!(reconstructed, path);
    }

    // ── Edge cases ──────────────────────────────────────────────────────

    #[test]
    fn wildcard_at_start() {
        let pat = Pattern::parse("{sample}.bam").unwrap();
        let wc = pat.resolve("patient_42.bam").unwrap();
        assert_eq!(wc.get("sample").unwrap(), "patient_42");
    }

    #[test]
    fn wildcard_at_end() {
        let pat = Pattern::parse("results/{sample}").unwrap();
        let wc = pat.resolve("results/patient_42").unwrap();
        assert_eq!(wc.get("sample").unwrap(), "patient_42");
    }

    #[test]
    fn pattern_with_dots_in_literal() {
        let pat = Pattern::parse("data/{sample}.sorted.bam").unwrap();
        let wc = pat.resolve("data/s1.sorted.bam").unwrap();
        assert_eq!(wc.get("sample").unwrap(), "s1");
    }

    #[test]
    fn empty_pattern_is_valid() {
        let pat = Pattern::parse("").unwrap();
        assert!(!pat.has_wildcards());
        assert!(pat.wildcard_names().is_empty());
        let wc = pat.resolve("").unwrap();
        assert!(wc.is_empty());
        assert!(pat.resolve("notempty").is_none());
    }

    // ── PartialEq and Eq ────────────────────────────────────────────────

    #[test]
    fn pattern_equality() {
        let a = Pattern::parse("results/{sample}.bam").unwrap();
        let b = Pattern::parse("results/{sample}.bam").unwrap();
        let c = Pattern::parse("results/{method}.bam").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    // ── raw() method ────────────────────────────────────────────────────

    #[test]
    fn raw_returns_original_pattern() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        assert_eq!(pat.raw(), "results/{sample}.bam");
    }

    // ── Invalid wildcard name characters ────────────────────────────────

    #[test]
    fn invalid_wildcard_name_characters() {
        let err = Pattern::parse("results/{sample-name}.bam").unwrap_err();
        assert!(matches!(&err, WildcardError::InvalidPattern { .. }));
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn wildcard_name_with_spaces_errors() {
        let err = Pattern::parse("results/{sample name}.bam").unwrap_err();
        assert!(matches!(&err, WildcardError::InvalidPattern { .. }));
        assert!(err.to_string().contains("invalid characters"));
    }

    // ── Unmatched escaped braces ─────────────────────────────────────────

    #[test]
    fn unmatched_escaped_braces() {
        let err = Pattern::parse("results/{{unclosed").unwrap_err();
        assert!(matches!(&err, WildcardError::InvalidPattern { .. }));
        assert!(err.to_string().contains("unmatched `{{`"), "got: {err}");
    }

    // ── Path handling edge cases (ox-58w3) ────────────────────────────

    #[test]
    fn pattern_with_spaces_in_literal() {
        let pat = Pattern::parse("my results/{sample}.csv").unwrap();
        let wc = pat.resolve("my results/A.csv").unwrap();
        assert_eq!(wc.get("sample").unwrap(), "A");
        assert!(pat.resolve("myresults/A.csv").is_none());
    }

    #[test]
    fn pattern_with_unicode_in_literal() {
        let pat = Pattern::parse("données/{sample}/résultats.csv").unwrap();
        let wc = pat.resolve("données/exp1/résultats.csv").unwrap();
        assert_eq!(wc.get("sample").unwrap(), "exp1");
    }

    #[test]
    fn pattern_resolves_unicode_wildcard_value() {
        let pat = Pattern::parse("output/{name}.csv").unwrap();
        let wc = pat.resolve("output/résultats.csv").unwrap();
        assert_eq!(wc.get("name").unwrap(), "résultats");
    }

    #[test]
    fn pattern_interpolate_with_unicode_values() {
        let pat = Pattern::parse("data/{sample}/output.csv").unwrap();
        let mut wc = BTreeMap::new();
        wc.insert("sample".to_string(), "échantillon_α".to_string());
        let result = pat.interpolate(&wc).unwrap();
        assert_eq!(result, "data/échantillon_α/output.csv");
    }

    #[test]
    fn pattern_with_spaces_in_wildcard_value() {
        // Wildcard values cannot contain `/` but spaces are fine.
        let pat = Pattern::parse("results/{sample}.csv").unwrap();
        let wc = pat.resolve("results/sample A.csv").unwrap();
        assert_eq!(wc.get("sample").unwrap(), "sample A");
    }

    #[test]
    fn compiled_pattern_with_unicode() {
        let pat = Pattern::parse("données/{sample}/résultats.csv").unwrap();
        let compiled = CompiledPattern::new(pat, &BTreeMap::new()).unwrap();
        let wc = compiled.resolve("données/exp1/résultats.csv").unwrap();
        assert_eq!(wc.get("sample").unwrap(), "exp1");
    }

    #[test]
    fn compiled_pattern_with_spaces() {
        let pat = Pattern::parse("my project/{sample}/result.csv").unwrap();
        let compiled = CompiledPattern::new(pat, &BTreeMap::new()).unwrap();
        let wc = compiled.resolve("my project/A/result.csv").unwrap();
        assert_eq!(wc.get("sample").unwrap(), "A");
    }

    // ── Cached regex reuse ──────────────────────────────────────────────

    #[test]
    fn resolve_twice_uses_consistent_results() {
        let pat = Pattern::parse("results/{sample}.bam").unwrap();
        // First resolve
        let wc1 = pat.resolve("results/A.bam").unwrap();
        assert_eq!(wc1.get("sample").unwrap(), "A");
        // Second resolve (exercises same code path)
        let wc2 = pat.resolve("results/B.bam").unwrap();
        assert_eq!(wc2.get("sample").unwrap(), "B");
    }

    // ── Clone and Debug ─────────────────────────────────────────────────

    #[test]
    fn pattern_clone() {
        let pat = Pattern::parse("data/{x}.txt").unwrap();
        let cloned = pat.clone();
        assert_eq!(pat.raw(), cloned.raw());
        assert_eq!(pat.segments(), cloned.segments());
    }

    #[test]
    fn segment_clone_and_debug() {
        let seg = Segment::Wildcard("sample".into());
        let cloned = seg.clone();
        assert_eq!(seg, cloned);
        let debug = format!("{:?}", cloned);
        assert!(debug.contains("Wildcard"));
    }

    // ── Property tests ─────────────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for valid wildcard names (alphanumeric + underscore, non-empty).
        fn wildcard_name() -> impl Strategy<Value = String> {
            "[a-zA-Z_][a-zA-Z0-9_]{0,9}"
        }

        /// Strategy for wildcard values (non-empty, no slashes — wildcards don't cross `/`).
        fn wildcard_value() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9_.]{1,20}"
        }

        /// Strategy for literal segments (no braces, no slashes for simplicity).
        fn literal_segment() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9_.]{1,10}"
        }

        /// Build a pattern string from alternating literals and wildcard names.
        /// Ensures no adjacent wildcards (separated by literals).
        fn pattern_and_values() -> impl Strategy<Value = (String, BTreeMap<String, String>)> {
            // Generate 1-4 wildcard names and literal separators
            (1..=4usize)
                .prop_flat_map(|n_wildcards| {
                    let names = proptest::collection::vec(wildcard_name(), n_wildcards);
                    let values = proptest::collection::vec(wildcard_value(), n_wildcards);
                    // n_wildcards + 1 literals to surround all wildcards
                    let literals = proptest::collection::vec(literal_segment(), n_wildcards + 1);
                    (names, values, literals)
                })
                .prop_filter_map("unique wildcard names", |(names, values, literals)| {
                    // Wildcard names must be unique within a pattern
                    let mut seen = std::collections::HashSet::new();
                    if !names.iter().all(|n| seen.insert(n.clone())) {
                        return None;
                    }
                    // Build pattern: lit0 {name0} lit1 {name1} lit2 ...
                    let mut pattern = String::new();
                    let mut wc_map = BTreeMap::new();
                    for (i, name) in names.iter().enumerate() {
                        pattern.push_str(&literals[i]);
                        pattern.push('{');
                        pattern.push_str(name);
                        pattern.push('}');
                        wc_map.insert(name.clone(), values[i].clone());
                    }
                    pattern.push_str(&literals[names.len()]);
                    Some((pattern, wc_map))
                })
        }

        proptest! {
            /// Parse is deterministic: parsing the same string twice yields equal patterns.
            #[test]
            fn parse_deterministic(
                (pat_str, _) in pattern_and_values()
            ) {
                let p1 = Pattern::parse(&pat_str).unwrap();
                let p2 = Pattern::parse(&pat_str).unwrap();
                prop_assert_eq!(p1, p2);
            }

            /// Round-trip: interpolate → resolve → interpolate reproduces the path.
            #[test]
            fn roundtrip_interpolate_resolve(
                (pat_str, wc_map) in pattern_and_values()
            ) {
                let pat = Pattern::parse(&pat_str).unwrap();

                // Interpolate wildcards into the pattern to get a concrete path
                let path = pat.interpolate(&wc_map).unwrap();

                // Resolve the concrete path back against the pattern
                let resolved = pat.resolve(&path);
                prop_assert!(
                    resolved.is_some(),
                    "resolve failed for path={:?} pattern={:?}",
                    path, pat_str
                );
                let resolved = resolved.unwrap();

                // Re-interpolate should produce the same path
                let reconstructed = pat.interpolate(&resolved).unwrap();
                prop_assert_eq!(
                    &reconstructed, &path,
                    "round-trip failed: pattern={:?} wc={:?}",
                    pat_str, wc_map
                );
            }

            /// Parse → raw() round-trip: the raw string is preserved through parsing.
            #[test]
            fn raw_preserved(
                (pat_str, _) in pattern_and_values()
            ) {
                let pat = Pattern::parse(&pat_str).unwrap();
                prop_assert_eq!(pat.raw(), &pat_str);
            }

            /// Wildcard names extracted from a pattern match the names we put in.
            #[test]
            fn wildcard_names_match_input(
                (pat_str, wc_map) in pattern_and_values()
            ) {
                let pat = Pattern::parse(&pat_str).unwrap();
                let names = pat.wildcard_names();
                // Every name in the pattern should be a key in our map
                for name in &names {
                    prop_assert!(
                        wc_map.contains_key(*name),
                        "unexpected wildcard name: {}", name
                    );
                }
                // And the count should match
                prop_assert_eq!(names.len(), wc_map.len());
            }

            /// A literal-only pattern never has wildcards.
            #[test]
            fn literal_has_no_wildcards(s in "[a-zA-Z0-9_./]{0,30}") {
                if let Ok(pat) = Pattern::parse(&s) {
                    prop_assert!(!pat.has_wildcards());
                }
            }

            /// Adjacent wildcards always produce a parse error.
            #[test]
            fn adjacent_wildcards_rejected(
                a in wildcard_name(),
                b in wildcard_name()
            ) {
                let input = format!("{{{a}}}{{{b}}}");
                prop_assert!(Pattern::parse(&input).is_err());
            }

            /// Wildcards don't cross `/` boundaries: a value with `/` won't match.
            #[test]
            fn wildcard_no_slash_crossing(
                prefix in literal_segment(),
                name in wildcard_name(),
                suffix in literal_segment(),
            ) {
                let pat_str = format!("{prefix}{{{name}}}{suffix}");
                let pat = Pattern::parse(&pat_str).unwrap();
                let path_with_slash = format!("{prefix}a/b{suffix}");
                prop_assert!(
                    pat.resolve(&path_with_slash).is_none(),
                    "wildcard crossed `/` boundary"
                );
            }

            /// CompiledPattern resolves identically to Pattern::resolve.
            #[test]
            fn compiled_matches_uncompiled(
                (pat_str, wc_map) in pattern_and_values()
            ) {
                let pat = Pattern::parse(&pat_str).unwrap();
                let path = pat.interpolate(&wc_map).unwrap();

                let compiled = CompiledPattern::new(pat.clone(), &BTreeMap::new()).unwrap();
                let from_compiled = compiled.resolve(&path);
                let from_pat = pat.resolve(&path);
                prop_assert_eq!(from_compiled, from_pat);
            }
        }
    }
}
