//! Schema dispatch and include-graph preflight. No config sources are read here.
use ox_core::error::ParseError;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use toml::Value;

pub(crate) fn error(path: &Path, message: impl Into<String>) -> ParseError {
    ParseError::Toml {
        file: path.to_path_buf(),
        message: message.into(),
    }
}

pub(crate) fn document(content: &str, path: &Path) -> Result<Value, ParseError> {
    toml::from_str(content).map_err(|e| error(path, e.to_string()))
}

pub(crate) fn check(value: &Value, path: &Path) -> Result<(), ParseError> {
    match value.get("format_version").and_then(Value::as_str) {
        None if value.get("format_version").is_none() => Ok(()),
        Some("1") => Ok(()),
        Some("2") => {
            let declared = value
                .get("ox_version")
                .map(Value::to_string)
                .unwrap_or_else(|| "<missing>".into());
            let running = env!("CARGO_PKG_VERSION");
            let fail = || {
                error(
                    path,
                    format!(
                        "ox_version {declared}: invalid or unsatisfied minimum capable binary requirement; expected >=MAJOR.MINOR.PATCH; running ox {running}; this is not a range language or a reproducibility guarantee"
                    ),
                )
            };
            let text = value
                .get("ox_version")
                .and_then(Value::as_str)
                .and_then(|s| s.strip_prefix(">="))
                .ok_or_else(fail)?;
            // Only three canonical decimal components. No ranges, whitespace,
            // prerelease labels, build metadata, or shorthand are accepted.
            let minimum = semver::Version::parse(text).map_err(|_| fail())?;
            if !minimum.pre.is_empty()
                || !minimum.build.is_empty()
                || minimum.to_string() != text
                || semver::Version::parse(running).map_err(|_| fail())? < minimum
            {
                return Err(fail());
            }
            structural(value, path, "", "root")
        }
        other => Err(error(
            path,
            format!(
                "unsupported format_version {} (supported: 1, 2)",
                other
                    .map(str::to_owned)
                    .unwrap_or_else(|| value["format_version"].to_string())
            ),
        )),
    }
}

pub(crate) fn collect_sources(
    content: &str,
    path: &Path,
) -> Result<BTreeMap<PathBuf, String>, ParseError> {
    fn visit(
        content: &str,
        path: &Path,
        active: &mut Vec<PathBuf>,
        sources: &mut BTreeMap<PathBuf, String>,
    ) -> Result<(), ParseError> {
        let id = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if active.contains(&id) {
            let mut chain = active.clone();
            chain.push(id);
            return Err(ParseError::CircularInclude { chain });
        }
        // Traverse each inclusion context: a symlink can give the same physical
        // source a different parent for its relative includes. Reuse retained
        // bytes, but never skip validation of that context's descendants.
        let content = sources
            .get(&id)
            .cloned()
            .unwrap_or_else(|| content.to_owned());
        let value = document(&content, path)?;
        check(&value, path)?;
        active.push(id.clone());
        if let Some(includes) = value.get("include") {
            let includes = includes
                .as_array()
                .ok_or_else(|| error(path, "include must be an array of paths"))?;
            for include in includes {
                let include = include
                    .as_str()
                    .ok_or_else(|| error(path, "include must contain string paths"))?;
                let include = crate::parse::expand_tilde(include);
                let child = path.parent().unwrap_or(Path::new(".")).join(include);
                let text =
                    std::fs::read_to_string(&child).map_err(|_| ParseError::IncludeNotFound {
                        path: child.clone(),
                    })?;
                visit(&text, &child, active, sources)?;
            }
        }
        active.pop();
        sources.insert(id, content.to_owned());
        Ok(())
    }
    let mut sources = BTreeMap::new();
    visit(content, path, &mut Vec::new(), &mut sources)?;
    Ok(sources)
}

// Close structural tables only. Named maps (config, tags, params, resources,
// profile.set, wildcard_constraints and named inputs/outputs) remain open.
pub(crate) fn structural(
    value: &Value,
    path: &Path,
    location: &str,
    kind: &str,
) -> Result<(), ParseError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    let allowed: &[&str] = match kind {
        "root" => &[
            "ox_version",
            "format_version",
            "config",
            "rule",
            "gate",
            "profile",
            "include",
            "environment",
            "env",
            "executor",
            "resource_classes",
        ],
        "rule" => &[
            "input",
            "output",
            "shell",
            "run",
            "script",
            "call",
            "lang",
            "tags",
            "resources",
            "resource_class",
            "environment",
            "env",
            "when",
            "expand",
            "error_strategy",
            "timeout",
            "executor",
            "priority",
            "description",
            "benchmark",
            "retries",
            "wildcard_constraints",
            "params",
            "param_files",
            "log",
            "shell_executable",
            "reproducibility",
            "clean_outputs",
            "cache_platform",
            "source_line",
        ],
        "profile" => &[
            "jobs",
            "cache_validation",
            "verbose",
            "executor",
            "no_cache",
            "keep_going",
            "partition",
            "account",
            "qos",
            "open_dashboard",
            "ray_allow_pending",
            "set",
        ],
        "gate" => &["after", "before", "message"],
        "executor" => &["slurm"],
        "slurm" => &[
            "mode",
            "api_url",
            "token_cmd",
            "partition",
            "account",
            "qos",
            "staging_dir",
            "extra_flags",
        ],
        "log" => &["stdout", "stderr"],
        "environment" => &["uv", "conda", "docker", "nix", "apptainer"],
        "input" => &["path", "name", "format"],
        "output" => &["path", "name", "format", "lifecycle", "materialize"],
        "error_strategy" => &["retry", "backoff"],
        "when" => match table.get("op").and_then(Value::as_str) {
            Some("eq" | "not_eq") => &["op", "field", "value"],
            Some("in" | "not_in") => &["op", "field", "values"],
            Some("regex") => &["op", "field", "pattern"],
            Some("config_eq") => &["op", "key", "value"],
            Some("env_set") => &["op", "var"],
            Some("env_eq") => &["op", "var", "value"],
            Some("file_exists") => &["op", "path"],
            Some("and" | "or") => &["op", "conditions"],
            Some("not") => &["op", "condition"],
            _ => &["op"],
        },
        "resource_class" => &[],
        _ => unreachable!("structural kind {kind}"),
    };
    if kind == "resource_class" {
        const NON_RESOURCE_KEYS: &[&str] = &[
            "parent",
            "extends",
            "resource_class",
            "variables",
            "environment",
            "executor",
            "when",
            "shell",
            "run",
            "script",
            "call",
            "input",
            "output",
            "resources",
        ];
        for (key, child) in table {
            if NON_RESOURCE_KEYS.contains(&key.as_str()) {
                return Err(error(
                    path,
                    format!(
                        "non-resource key `{location}.{key}` in resource class; classes contain resource values only"
                    ),
                ));
            }
            resource_value(child, path, &format!("{location}.{key}"))?;
        }
        return Ok(());
    }
    for (key, child) in table {
        let at = if location.is_empty() {
            key.clone()
        } else {
            format!("{location}.{key}")
        };
        if !allowed.contains(&key.as_str()) {
            return Err(error(
                path,
                format!("unknown structural key `{at}` under schema 2"),
            ));
        }
        match (kind, key.as_str()) {
            ("rule", "resources") => {
                if let Some(resources) = child.as_table() {
                    for (name, value) in resources {
                        resource_value(value, path, &format!("{at}.{name}"))?;
                    }
                }
            }
            ("root", "rule" | "profile" | "gate") => {
                if let Some(named) = child.as_table() {
                    for (name, entry) in named {
                        structural(entry, path, &format!("{at}.{name}"), key)?;
                    }
                }
            }
            ("root", "resource_classes") => {
                if let Some(named) = child.as_table() {
                    for (name, entry) in named {
                        structural(entry, path, &format!("{at}.{name}"), "resource_class")?;
                    }
                }
            }
            ("root" | "rule", "environment")
            | ("root", "executor")
            | ("executor", "slurm")
            | ("rule", "log" | "error_strategy" | "when") => structural(child, path, &at, key)?,
            ("rule", "input" | "output") => {
                if let Some(items) = child.as_array() {
                    for item in items {
                        structural(item, path, &at, key)?;
                    }
                }
            }
            ("when", "condition") => structural(child, path, &at, "when")?,
            ("when", "conditions") => {
                if let Some(items) = child.as_array() {
                    for item in items {
                        structural(item, path, &at, "when")?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn resource_value(value: &Value, path: &Path, location: &str) -> Result<(), ParseError> {
    if !matches!(
        value,
        Value::Integer(_) | Value::Float(_) | Value::String(_)
    ) {
        return Err(error(
            path,
            format!(
                "invalid resource value at `{location}`: {value}; expected an integer, float, or string"
            ),
        ));
    }
    Ok(())
}
