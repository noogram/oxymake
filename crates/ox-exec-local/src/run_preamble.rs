//! Snakemake-compatible preamble for translated `run:` blocks.
//!
//! Snakemake executes `run:` bodies with `input`, `output`, `params`,
//! `wildcards`, `threads`, `resources` and `log` injected as objects into
//! the namespace.  Translated Oxymakefiles carry the body verbatim, so the
//! executor must provide an equivalent namespace — otherwise `input[0]`
//! resolves to the Python builtin `input` and the job crashes with
//! `TypeError: 'builtin_function_or_method' object is not subscriptable`.
//!
//! [`python_preamble`] renders the job's resolved values into a small block
//! of Python that is prepended to the user code before it is handed to the
//! interpreter.  Only Python `run:` blocks receive a preamble; other
//! languages run verbatim.

use ox_core::model::{ConcreteJob, OutputRef, ResourceValue};
use std::fmt::Write;

/// Python helper classes mimicking Snakemake's `Namedlist`:
/// positional indexing (`input[0]`), attribute access (`input.ref`),
/// iteration, and space-joined `str()` for lists.
const PREAMBLE_HELPERS: &str = r#"class _OxNamedList(list):
    def __init__(self, items, names):
        super().__init__(items)
        self._names = names
    def __getattr__(self, name):
        try:
            return object.__getattribute__(self, '_names')[name]
        except KeyError:
            raise AttributeError(name) from None
    def __str__(self):
        return ' '.join(str(x) for x in self)
class _OxNamedMap(dict):
    def __getattr__(self, name):
        try:
            return self[name]
        except KeyError:
            raise AttributeError(name) from None
"#;

/// Returns `true` if the run-block language is Python (the only language
/// Snakemake `run:` blocks exist in, and the only one that gets a preamble).
pub fn is_python(lang: &str) -> bool {
    matches!(lang, "python" | "python3")
}

/// Normalize the interpreter command for a run-block language.
///
/// Translated Oxymakefiles emit `lang = "python"`, but many systems
/// (notably macOS) ship only a `python3` binary.
pub fn interpreter(lang: &str) -> &str {
    if lang == "python" { "python3" } else { lang }
}

/// Escape a string as a Python single-quoted literal.
fn py_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Render `(name, value)` pairs as an `_OxNamedList(...)` constructor call.
fn py_named_list(items: &[(Option<&str>, String)]) -> String {
    let positional: Vec<String> = items.iter().map(|(_, v)| py_str(v)).collect();
    let named: Vec<String> = items
        .iter()
        .filter_map(|(n, v)| n.map(|n| format!("{}: {}", py_str(n), py_str(v))))
        .collect();
    format!(
        "_OxNamedList([{}], {{{}}})",
        positional.join(", "),
        named.join(", ")
    )
}

/// Render a Python literal for a resource value.
fn py_resource(value: &ResourceValue) -> String {
    match value {
        ResourceValue::Int(n) => n.to_string(),
        ResourceValue::Float(f) => f.to_string(),
        ResourceValue::Str(s) => py_str(s),
    }
}

/// Generate the Snakemake-compatible namespace preamble for a Python
/// `run:` block, ending with a newline so user code can be appended.
pub fn python_preamble(job: &ConcreteJob) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str(PREAMBLE_HELPERS);

    let file_items = |refs: Vec<(Option<&str>, &OutputRef)>| -> Vec<(Option<&str>, String)> {
        refs.into_iter()
            .filter_map(|(name, r)| match r {
                OutputRef::File(p) => Some((name, p.display().to_string())),
                _ => None,
            })
            .collect()
    };

    let inputs = file_items(
        job.inputs
            .iter()
            .map(|i| (i.name.as_deref(), &i.reference))
            .collect(),
    );
    let outputs = file_items(
        job.outputs
            .iter()
            .map(|o| (o.name.as_deref(), &o.reference))
            .collect(),
    );
    writeln!(out, "input = {}", py_named_list(&inputs)).unwrap();
    writeln!(out, "output = {}", py_named_list(&outputs)).unwrap();

    let logs: Vec<(Option<&str>, String)> = [&job.log.stdout, &job.log.stderr]
        .into_iter()
        .flatten()
        .map(|p| (None, p.clone()))
        .collect();
    writeln!(out, "log = {}", py_named_list(&logs)).unwrap();

    let map_entries = |pairs: Vec<(&str, String)>| -> String {
        let entries: Vec<String> = pairs
            .into_iter()
            .map(|(k, v)| format!("{}: {}", py_str(k), v))
            .collect();
        format!("_OxNamedMap({{{}}})", entries.join(", "))
    };

    let params: Vec<(&str, String)> = job
        .params
        .iter()
        .map(|(k, v)| (k.as_str(), py_str(v)))
        .collect();
    writeln!(out, "params = {}", map_entries(params)).unwrap();

    let wildcards: Vec<(&str, String)> = job
        .wildcards
        .iter()
        .map(|(k, v)| (k.as_str(), py_str(v)))
        .collect();
    writeln!(out, "wildcards = {}", map_entries(wildcards)).unwrap();

    let resources: Vec<(&str, String)> = job
        .resources
        .iter()
        .map(|(k, v)| (k.as_str(), py_resource(v)))
        .collect();
    writeln!(out, "resources = {}", map_entries(resources)).unwrap();

    // Snakemake's `threads:` directive translates to the `cpu` resource.
    let threads = job
        .resources
        .get("cpu")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);
    writeln!(out, "threads = {threads}").unwrap();

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::model::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn make_job() -> ConcreteJob {
        ConcreteJob {
            id: JobId::from("analyze-sample1"),
            rule: RuleName::from("analyze"),
            wildcards: BTreeMap::from([("sample".to_string(), "sample1".to_string())]),
            tags: BTreeMap::new(),
            inputs: vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("data/sample1.csv")),
                name: None,
                format: None,
            }],
            outputs: vec![ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("results/sample1.json")),
                name: None,
                format: None,
                lifecycle: OutputLifecycle::default(),
                materialize: MaterializePolicy::default(),
            }],
            execution: ExecutionBlock::Run {
                code: "pass".into(),
                lang: "python".into(),
            },
            resources: BTreeMap::from([("cpu".to_string(), ResourceValue::Int(4))]),
            environment: None,
            error_strategy: ErrorStrategy::default(),
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::from([("alpha".to_string(), "0.5".to_string())]),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        }
    }

    #[test]
    fn is_python_matches_aliases() {
        assert!(is_python("python"));
        assert!(is_python("python3"));
        assert!(!is_python("Rscript"));
        assert!(!is_python("bash"));
    }

    #[test]
    fn interpreter_normalizes_python() {
        assert_eq!(interpreter("python"), "python3");
        assert_eq!(interpreter("python3"), "python3");
        assert_eq!(interpreter("Rscript"), "Rscript");
    }

    #[test]
    fn py_str_escapes_quotes_and_backslashes() {
        assert_eq!(py_str("plain"), "'plain'");
        assert_eq!(py_str("it's"), r"'it\'s'");
        assert_eq!(py_str(r"a\b"), r"'a\\b'");
        assert_eq!(py_str("a\nb"), r"'a\nb'");
    }

    #[test]
    fn preamble_defines_snakemake_namespace() {
        let preamble = python_preamble(&make_job());
        assert!(preamble.contains("input = _OxNamedList(['data/sample1.csv'], {})"));
        assert!(preamble.contains("output = _OxNamedList(['results/sample1.json'], {})"));
        assert!(preamble.contains("params = _OxNamedMap({'alpha': '0.5'})"));
        assert!(preamble.contains("wildcards = _OxNamedMap({'sample': 'sample1'})"));
        assert!(preamble.contains("resources = _OxNamedMap({'cpu': 4})"));
        assert!(preamble.contains("threads = 4"));
        assert!(preamble.contains("log = _OxNamedList([], {})"));
    }

    #[test]
    fn preamble_includes_named_inputs() {
        let mut job = make_job();
        job.inputs.push(ResolvedInput {
            reference: OutputRef::File(PathBuf::from("ref/genome.fa")),
            name: Some("genome".into()),
            format: None,
        });
        let preamble = python_preamble(&job);
        assert!(
            preamble.contains(
                "input = _OxNamedList(['data/sample1.csv', 'ref/genome.fa'], \
                 {'genome': 'ref/genome.fa'})"
            ),
            "preamble: {preamble}"
        );
    }

    #[test]
    fn preamble_defaults_threads_to_one() {
        let mut job = make_job();
        job.resources.clear();
        let preamble = python_preamble(&job);
        assert!(preamble.contains("threads = 1"));
    }

    /// The generated preamble must be valid Python that gives Snakemake
    /// semantics: indexing, attribute access, and space-joined str().
    #[test]
    fn preamble_executes_with_snakemake_semantics() {
        let mut job = make_job();
        job.inputs.push(ResolvedInput {
            reference: OutputRef::File(PathBuf::from("ref/genome.fa")),
            name: Some("genome".into()),
            format: None,
        });
        let