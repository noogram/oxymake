//! MCP tool definitions and handlers.
//!
//! Each tool maps to an existing ox command, translating between MCP JSON
//! parameters and the Rust API.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::json;

use crate::protocol::{ToolCallResult, ToolDefinition};

/// Build the full tool catalog with JSON Schema input definitions.
pub fn tool_catalog() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "ox_status".into(),
            description: "Get the current execution status. Shows job counts by state and optional grouping by rule/stage.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "group_by": {
                        "type": "string",
                        "enum": ["rule", "stage"],
                        "description": "Group results by dimension."
                    }
                },
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "ox_plan".into(),
            description: "Show the execution plan without running. Resolves the DAG and reports what would execute, what is cached, and the dependency structure.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "targets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Specific targets to plan for. If empty, plans for default targets."
                    },
                    "rule": {
                        "type": "string",
                        "description": "Filter by rule name (exact or /regex/)."
                    }
                },
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "ox_dag".into(),
            description: "Get the DAG structure as nodes and edges for visualization.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "format": {
                        "type": "string",
                        "enum": ["json", "dot"],
                        "default": "json",
                        "description": "Output format. 'json' returns nodes/edges, 'dot' returns Graphviz DOT."
                    }
                },
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "ox_logs".into(),
            description: "Retrieve job log content. Returns stdout/stderr for a specific job or the most recent failed jobs.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "job_id": {
                        "type": "string",
                        "description": "Specific job ID to get logs for."
                    },
                    "failed": {
                        "type": "boolean",
                        "default": false,
                        "description": "If true, return logs for all failed jobs."
                    },
                    "tail": {
                        "type": "integer",
                        "default": 50,
                        "description": "Number of lines from the end to return."
                    }
                },
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "ox_history".into(),
            description: "List past runs with timestamps, job counts, and notes.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "default": 20,
                        "description": "Maximum number of runs to return."
                    }
                },
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "ox_lint".into(),
            description: "Validate the Oxymakefile. Returns errors, warnings, and info diagnostics.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Path to Oxymakefile. Defaults to 'Oxymakefile.toml'."
                    }
                },
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "ox_explain".into(),
            description: "Show the full dependency chain for a target — why it will be built, what triggers it.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "The output file path to explain."
                    }
                },
                "required": ["target"],
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "ox_clean".into(),
            description: "Remove job logs and orphan cache entries. Returns counts of removed items.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "what": {
                        "type": "string",
                        "enum": ["cache", "logs", "all"],
                        "default": "all",
                        "description": "What to clean: orphan cache entries, job logs, or both."
                    }
                },
                "additionalProperties": false
            }),
        },
    ]
}

/// Dispatch a tool call to the appropriate handler.
pub fn handle_tool_call(name: &str, args: &serde_json::Value, workdir: &Path) -> ToolCallResult {
    let result = match name {
        "ox_status" => handle_status(args, workdir),
        "ox_plan" => handle_plan(args, workdir),
        "ox_dag" => handle_dag(args, workdir),
        "ox_logs" => handle_logs(args, workdir),
        "ox_history" => handle_history(args, workdir),
        "ox_lint" => handle_lint(args, workdir),
        "ox_explain" => handle_explain(args, workdir),
        "ox_clean" => handle_clean(args, workdir),
        _ => return ToolCallResult::error(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(r) => r,
        Err(e) => ToolCallResult::error(format!("{e:#}")),
    }
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

fn handle_status(args: &serde_json::Value, workdir: &Path) -> Result<ToolCallResult> {
    let db_path = workdir.join(".oxymake/state.db");
    if !db_path.exists() {
        return Ok(ToolCallResult::json(&json!({
            "error": {
                "code": "NO_STATE",
                "message": "No OxyMake state found. Run 'ox run' first.",
                "hint": "Execute a workflow with ox_run to create state."
            }
        })));
    }

    let db = ox_state::db::StateDb::open(&db_path).context("failed to open state database")?;
    let counts = db.job_counts()?;
    let sessions = db.active_sessions()?;

    let total = counts.pending
        + counts.running
        + counts.completed
        + counts.failed
        + counts.skipped
        + counts.cancelled;

    let group_by = args.get("group_by").and_then(|v| v.as_str());

    if group_by.is_some() {
        let stats = db.pipeline_stats()?;
        let groups: Vec<serde_json::Value> = stats
            .iter()
            .map(|s| {
                let pending = s.total.saturating_sub(s.completed + s.running);
                let progress_pct = if s.total == 0 {
                    0.0
                } else {
                    (s.completed as f64 / s.total as f64) * 100.0
                };
                json!({
                    "name": s.rule_name,
                    "total": s.total,
                    "completed": s.completed,
                    "running": s.running,
                    "pending": pending,
                    "progress_pct": (progress_pct * 100.0).round() / 100.0,
                })
            })
            .collect();

        Ok(ToolCallResult::json(&json!({
            "sessions": sessions.len(),
            "jobs": {
                "total": total,
                "completed": counts.completed,
                "running": counts.running,
                "failed": counts.failed,
                "pending": counts.pending,
                "skipped": counts.skipped,
                "cached": counts.cached,
                "cancelled": counts.cancelled,
            },
            "groups": groups,
        })))
    } else {
        Ok(ToolCallResult::json(&json!({
            "sessions": sessions.len(),
            "jobs": {
                "total": total,
                "completed": counts.completed,
                "running": counts.running,
                "failed": counts.failed,
                "pending": counts.pending,
                "skipped": counts.skipped,
                "cached": counts.cached,
                "cancelled": counts.cancelled,
            }
        })))
    }
}

fn handle_plan(args: &serde_json::Value, workdir: &Path) -> Result<ToolCallResult> {
    let file = workdir.join("Oxymakefile.toml");
    if !file.exists() {
        return Ok(ToolCallResult::json(&json!({
            "error": {
                "code": "OXYMAKEFILE_NOT_FOUND",
                "message": format!("No Oxymakefile.toml found in {}", workdir.display()),
                "hint": "Run 'ox init' to create a starter workflow, or specify --workdir"
            }
        })));
    }

    let content = std::fs::read_to_string(&file).context("cannot read Oxymakefile.toml")?;
    let workflow = ox_format::parse::parse_workflow(&content, &file)
        .context("parse error in Oxymakefile.toml")?;

    // Validate
    if let Err(errs) = ox_format::validate::validate(&workflow) {
        let messages: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        return Ok(ToolCallResult::json(&json!({
            "error": {
                "code": "CONFIG_ERROR",
                "message": format!("Validation errors: {}", messages.join("; ")),
            }
        })));
    }

    // Resolve targets
    let user_targets: Vec<String> = args
        .get("targets")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let config = build_config(&workflow);
    let targets = resolve_targets_from_workflow(&workflow, &user_targets);

    if targets.is_empty() {
        return Ok(ToolCallResult::json(&json!({
            "level": "jobs",
            "total_nodes": 0,
            "to_run": 0,
            "cached": 0,
            "nodes": [],
        })));
    }

    let existing_files = discover_files(&file);
    let request = ox_core::resolver::ResolveRequest {
        targets: targets.clone(),
        config,
        existing_files,
    };

    let resolve_result = ox_core::resolver::resolve(&workflow.rules, &request)
        .context("failed to resolve targets")?;

    let job_graph = ox_core::job_graph::JobGraph::build(resolve_result.jobs)
        .context("failed to build JobGraph")?;

    let topo = job_graph.topological_order().unwrap_or_default();
    let nodes: Vec<serde_json::Value> = topo
        .iter()
        .filter_map(|job_id| {
            let job = job_graph.get_job(job_id)?;
            let outputs: Vec<String> = job.outputs.iter().map(format_output_ref).collect();
            let inputs: Vec<String> = job
                .inputs
                .iter()
                .map(|i| format_output_ref_inner(&i.reference))
                .collect();
            Some(json!({
                "id": job_id.as_str(),
                "rule": job.rule.as_str(),
                "status": "to_run",
                "inputs": inputs,
                "outputs": outputs,
                "tags": job.wildcards,
            }))
        })
        .collect();

    Ok(ToolCallResult::json(&json!({
        "level": "jobs",
        "total_nodes": nodes.len(),
        "to_run": nodes.len(),
        "cached": 0,
        "nodes": nodes,
    })))
}

fn handle_dag(args: &serde_json::Value, workdir: &Path) -> Result<ToolCallResult> {
    let db_path = workdir.join(".oxymake/state.db");
    let format = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("json");

    if !db_path.exists() {
        return Ok(ToolCallResult::json(&json!({
            "error": {
                "code": "NO_STATE",
                "message": "No OxyMake state found. Run a workflow first.",
            }
        })));
    }

    let db = ox_state::db::StateDb::open(&db_path)?;
    let edges = db.job_edges()?;
    // Get all jobs with their info via jobs_with_logs (rule, status included)
    let all_jobs = db.jobs_with_logs(None, false)?;

    if format == "dot" {
        let mut dot = String::from("digraph oxymake {\n  rankdir=LR;\n");
        for (from, to) in &edges {
            dot.push_str(&format!("  \"{from}\" -> \"{to}\";\n"));
        }
        dot.push('}');
        Ok(ToolCallResult::text(dot))
    } else {
        let nodes: Vec<serde_json::Value> = all_jobs
            .iter()
            .map(|j| json!({ "id": j.id, "rule": j.rule_name, "status": j.status }))
            .collect();
        let edge_values: Vec<serde_json::Value> = edges
            .iter()
            .map(|(from, to)| json!({ "from": from, "to": to }))
            .collect();
        Ok(ToolCallResult::json(&json!({
            "nodes": nodes,
            "edges": edge_values,
        })))
    }
}

fn handle_logs(args: &serde_json::Value, workdir: &Path) -> Result<ToolCallResult> {
    let log_dir = workdir.join(".oxymake/logs");
    let tail = args.get("tail").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    let job_id = args.get("job_id").and_then(|v| v.as_str());
    let failed_only = args
        .get("failed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if let Some(jid) = job_id {
        let stdout_path = log_dir.join(format!("{jid}.stdout"));
        let stderr_path = log_dir.join(format!("{jid}.stderr"));
        let stdout = read_tail(&stdout_path, tail);
        let stderr = read_tail(&stderr_path, tail);
        return Ok(ToolCallResult::json(&json!({
            "job_id": jid,
            "stdout": stdout,
            "stderr": stderr,
        })));
    }

    if failed_only {
        let db_path = workdir.join(".oxymake/state.db");
        if db_path.exists() {
            let db = ox_state::db::StateDb::open(&db_path)?;
            let failed_jobs = db.jobs_by_status("failed")?;
            let logs: Vec<serde_json::Value> = failed_jobs
                .iter()
                .map(|jid| {
                    let stdout_path = log_dir.join(format!("{jid}.stdout"));
                    let stderr_path = log_dir.join(format!("{jid}.stderr"));
                    json!({
                        "job_id": jid,
                        "stdout": read_tail(&stdout_path, tail),
                        "stderr": read_tail(&stderr_path, tail),
                    })
                })
                .collect();
            return Ok(ToolCallResult::json(&json!({ "failed_jobs": logs })));
        }
    }

    Ok(ToolCallResult::json(&json!({
        "error": {
            "code": "MISSING_PARAMS",
            "message": "Specify job_id or set failed=true.",
        }
    })))
}

fn handle_history(args: &serde_json::Value, workdir: &Path) -> Result<ToolCallResult> {
    let db_path = workdir.join(".oxymake/state.db");
    if !db_path.exists() {
        return Ok(ToolCallResult::json(&json!({
            "error": {
                "code": "NO_STATE",
                "message": "No OxyMake state found.",
            }
        })));
    }

    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let db = ox_state::db::StateDb::open(&db_path)?;
    let mut runs = db.list_runs()?;
    runs.truncate(limit);

    let entries: Vec<serde_json::Value> = runs
        .iter()
        .map(|r| {
            json!({
                "run_id": r.id,
                "started_at": format_iso8601(r.started_at),
                "completed_at": r.completed_at.map(format_iso8601),
                "total_jobs": r.job_count,
                "succeeded": r.succeeded,
                "failed": r.failed,
                "skipped": r.skipped,
                "note": r.note,
            })
        })
        .collect();

    Ok(ToolCallResult::json(&json!({ "runs": entries })))
}

fn handle_lint(args: &serde_json::Value, workdir: &Path) -> Result<ToolCallResult> {
    let file_name = args
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or("Oxymakefile.toml");
    let file = workdir.join(file_name);

    if !file.exists() {
        return Ok(ToolCallResult::json(&json!({
            "error": {
                "code": "OXYMAKEFILE_NOT_FOUND",
                "message": format!("No {} found in {}", file_name, workdir.display()),
                "hint": "Run 'ox init' to create a starter workflow, or specify --workdir"
            }
        })));
    }

    let content = std::fs::read_to_string(&file)?;
    let workflow = match ox_format::parse::parse_workflow(&content, &file) {
        Ok(wf) => wf,
        Err(e) => {
            return Ok(ToolCallResult::json(&json!({
                "valid": false,
                "diagnostics": [{
                    "level": "error",
                    "message": format!("{e}"),
                }],
            })));
        }
    };

    match ox_format::validate::validate(&workflow) {
        Ok(()) => Ok(ToolCallResult::json(&json!({
            "valid": true,
            "diagnostics": [],
        }))),
        Err(errs) => {
            let diags: Vec<serde_json::Value> = errs
                .iter()
                .map(|e| json!({ "level": "error", "message": e.to_string() }))
                .collect();
            Ok(ToolCallResult::json(&json!({
                "valid": false,
                "diagnostics": diags,
            })))
        }
    }
}

fn handle_explain(args: &serde_json::Value, workdir: &Path) -> Result<ToolCallResult> {
    let target = args
        .get("target")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("'target' parameter is required"))?;

    let file = workdir.join("Oxymakefile.toml");
    if !file.exists() {
        return Ok(ToolCallResult::json(&json!({
            "error": {
                "code": "OXYMAKEFILE_NOT_FOUND",
                "message": format!("No Oxymakefile.toml found in {}", workdir.display()),
            }
        })));
    }

    let content = std::fs::read_to_string(&file)?;
    let workflow = ox_format::parse::parse_workflow(&content, &file)?;

    let config = build_config(&workflow);
    let existing_files = discover_files(&file);
    let request = ox_core::resolver::ResolveRequest {
        targets: vec![target.to_string()],
        config,
        existing_files,
    };

    let resolve_result = ox_core::resolver::resolve(&workflow.rules, &request)?;
    let job_graph = ox_core::job_graph::JobGraph::build(resolve_result.jobs)?;
    let topo = job_graph.topological_order().unwrap_or_default();

    let chain: Vec<serde_json::Value> = topo
        .iter()
        .filter_map(|job_id| {
            let job = job_graph.get_job(job_id)?;
            let outputs: Vec<String> = job.outputs.iter().map(format_output_ref).collect();
            let inputs: Vec<String> = job
                .inputs
                .iter()
                .map(|i| format_output_ref_inner(&i.reference))
                .collect();
            Some(json!({
                "id": job_id.as_str(),
                "rule": job.rule.as_str(),
                "inputs": inputs,
                "outputs": outputs,
                "wildcards": job.wildcards,
            }))
        })
        .collect();

    Ok(ToolCallResult::json(&json!({
        "target": target,
        "dependency_chain": chain,
        "total_steps": chain.len(),
    })))
}

fn handle_clean(args: &serde_json::Value, workdir: &Path) -> Result<ToolCallResult> {
    let what = args.get("what").and_then(|v| v.as_str()).unwrap_or("all");
    if !matches!(what, "cache" | "logs" | "all") {
        anyhow::bail!("invalid 'what': {what} (expected \"cache\", \"logs\", or \"all\")");
    }

    let oxymake_dir = workdir.join(".oxymake");
    let mut removed = Vec::new();
    let mut cache_entries_removed = 0usize;

    // Clean logs
    if matches!(what, "logs" | "all") {
        let log_dir = oxymake_dir.join("logs");
        if log_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&log_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && std::fs::remove_file(&path).is_ok() {
                        removed.push(path.display().to_string());
                    }
                }
            }
        }
    }

    // Clean cache: delegate to the real cache store (.oxymake/cache),
    // removing orphan entries like `ox clean` does (H35).
    if matches!(what, "cache" | "all") && oxymake_dir.join("cache").exists() {
        let mut store = ox_cache::CacheStore::open(&oxymake_dir)
            .map_err(|e| anyhow::anyhow!("failed to open cache store: {e}"))?;
        cache_entries_removed = store.remove_orphans();
        if cache_entries_removed > 0 {
            store
                .save()
                .map_err(|e| anyhow::anyhow!("failed to save cache manifest: {e}"))?;
        }
    }

    Ok(ToolCallResult::json(&json!({
        "what": what,
        "removed": removed.len() + cache_entries_removed,
        "cache_entries_removed": cache_entries_removed,
        "files": removed,
    })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn format_iso8601(ts: u64) -> String {
    let days = ts / 86400;
    let rem = ts % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let days = days + 719_468;
    let era = days / 146_097;
    let doe = days % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn format_output_ref(output: &ox_core::model::ResolvedOutput) -> String {
    format_output_ref_inner(&output.reference)
}

fn format_output_ref_inner(r: &ox_core::model::OutputRef) -> String {
    match r {
        ox_core::model::OutputRef::File(p) => p.display().to_string(),
        ox_core::model::OutputRef::Virtual { id, .. } => id.clone(),
        ox_core::model::OutputRef::InMemory { type_hint } => {
            type_hint.clone().unwrap_or_else(|| "<memory>".into())
        }
    }
}

fn read_tail(path: &Path, max_lines: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            if lines.len() > max_lines {
                lines[lines.len() - max_lines..].join("\n")
            } else {
                content
            }
        }
        Err(_) => String::new(),
    }
}

// Target resolution and config conversion are the shared implementation
// from ox-format::targets — the same one the CLI and the public API use,
// including {config.X} substitution and wildcard expansion (H34).

fn build_config(workflow: &ox_format::parse::Workflow) -> ox_core::resolver::Config {
    ox_format::targets::workflow_config(workflow)
}

fn resolve_targets_from_workflow(
    workflow: &ox_format::parse::Workflow,
    user_targets: &[String],
) -> Vec<String> {
    ox_format::targets::resolve_targets(workflow, user_targets)
}

fn discover_files(oxymakefile_path: &Path) -> Vec<PathBuf> {
    let base_dir = oxymakefile_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut files = Vec::new();
    walk_dir(base_dir, base_dir, &mut files, 5);
    files
}

fn walk_dir(dir: &Path, base: &Path, files: &mut Vec<PathBuf>, depth: usize) {
    if depth == 0 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" {
                continue;
            }
            walk_dir(&path, base, files, depth - 1);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(base) {
                let rel_str = rel.to_string_lossy();
                let clean = rel_str.strip_prefix("./").unwrap_or(&rel_str);
                files.push(PathBuf::from(clean));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // -- tool_catalog tests --

    #[test]
    fn tool_catalog_has_eight_tools() {
        let catalog = tool_catalog();
        assert_eq!(catalog.len(), 8);
    }

    #[test]
    fn tool_catalog_names_are_unique() {
        let catalog = tool_catalog();
        let mut names: Vec<&str> = catalog.iter().map(|t| t.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 8);
    }

    #[test]
    fn tool_catalog_expected_names() {
        let catalog = tool_catalog();
        let names: Vec<&str> = catalog.iter().map(|t| t.name.as_str()).collect();
        for expected in &[
            "ox_status",
            "ox_plan",
            "ox_dag",
            "ox_logs",
            "ox_history",
            "ox_lint",
            "ox_explain",
            "ox_clean",
        ] {
            assert!(names.contains(expected), "missing tool: {expected}");
        }
    }

    #[test]
    fn tool_catalog_schemas_are_objects() {
        let catalog = tool_catalog();
        for tool in &catalog {
            assert_eq!(
                tool.input_schema["type"], "object",
                "tool {} schema type must be 'object'",
                tool.name
            );
        }
    }

    #[test]
    fn tool_catalog_descriptions_non_empty() {
        let catalog = tool_catalog();
        for tool in &catalog {
            assert!(
                !tool.description.is_empty(),
                "tool {} has empty description",
                tool.name
            );
        }
    }

    // -- handle_tool_call dispatch tests --

    #[test]
    fn unknown_tool_returns_error() {
        let result = handle_tool_call("nonexistent", &serde_json::json!({}), Path::new("/tmp"));
        assert_eq!(result.is_error, Some(true));
        assert!(result.content[0].text.contains("Unknown tool"));
    }

    // -- format_iso8601 tests --

    #[test]
    fn format_iso8601_unix_epoch() {
        assert_eq!(format_iso8601(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_iso8601_known_date() {
        // 2024-01-01T00:00:00Z = 1704067200
        assert_eq!(format_iso8601(1704067200), "2024-01-01T00:00:00Z");
    }

    #[test]
    fn format_iso8601_with_time() {
        // Verify the output matches our own implementation
        let ts = 1718457045_u64;
        let result = format_iso8601(ts);
        // Verify format: YYYY-MM-DDTHH:MM:SSZ
        assert!(result.starts_with("2024-06-15T"));
        assert!(result.ends_with('Z'));
        assert_eq!(result.len(), 20);
    }

    // -- days_to_ymd tests --

    #[test]
    fn days_to_ymd_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_leap_year() {
        // 2024-02-29 is day 19782 from epoch
        let (y, m, d) = days_to_ymd(19782);
        assert_eq!((y, m, d), (2024, 2, 29));
    }

    #[test]
    fn days_to_ymd_end_of_year() {
        // 2023-12-31 is day 19722 from epoch
        let (y, m, d) = days_to_ymd(19722);
        assert_eq!((y, m, d), (2023, 12, 31));
    }

    // -- read_tail tests --

    #[test]
    fn read_tail_missing_file_returns_empty() {
        assert_eq!(read_tail(Path::new("/nonexistent/path"), 10), "");
    }

    #[test]
    fn read_tail_small_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.txt");
        std::fs::write(&path, "line1\nline2\nline3\n").unwrap();
        let result = read_tail(&path, 10);
        assert!(result.contains("line1"));
        assert!(result.contains("line3"));
    }

    #[test]
    fn read_tail_truncates_long_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.txt");
        let mut f = std::fs::File::create(&path).unwrap();
        for i in 0..100 {
            writeln!(f, "line {i}").unwrap();
        }
        drop(f);
        let result = read_tail(&path, 5);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains("95"));
        assert!(lines[4].contains("99"));
    }

    // -- format_output_ref_inner tests --

    #[test]
    fn format_output_ref_file() {
        let r = ox_core::model::OutputRef::File(PathBuf::from("results/out.bam"));
        assert_eq!(format_output_ref_inner(&r), "results/out.bam");
    }

    #[test]
    fn format_output_ref_virtual() {
        let r = ox_core::model::OutputRef::Virtual {
            id: "v1".into(),
            check: "SELECT 1".into(),
        };
        assert_eq!(format_output_ref_inner(&r), "v1");
    }

    #[test]
    fn format_output_ref_in_memory_with_hint() {
        let r = ox_core::model::OutputRef::InMemory {
            type_hint: Some("dataframe".into()),
        };
        assert_eq!(format_output_ref_inner(&r), "dataframe");
    }

    #[test]
    fn format_output_ref_in_memory_without_hint() {
        let r = ox_core::model::OutputRef::InMemory { type_hint: None };
        assert_eq!(format_output_ref_inner(&r), "<memory>");
    }

    // -- resolve_targets_from_workflow tests --

    fn empty_workflow() -> ox_format::parse::Workflow {
        ox_format::parse::Workflow {
            ox_version: None,
            format_version: ox_format::parse::DEFAULT_FORMAT_VERSION.to_string(),
            config: Default::default(),
            rules: vec![],
            gates: vec![],
            includes: vec![],
            global_environment: None,
            profiles: Default::default(),
            executor_config: Default::default(),
        }
    }

    #[test]
    fn resolve_targets_returns_user_targets_when_provided() {
        let workflow = empty_workflow();
        let targets = resolve_targets_from_workflow(&workflow, &["a.txt".into(), "b.txt".into()]);
        assert_eq!(targets, vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn resolve_targets_empty_workflow_returns_empty() {
        let workflow = empty_workflow();
        let targets = resolve_targets_from_workflow(&workflow, &[]);
        assert!(targets.is_empty());
    }

    #[test]
    fn resolve_targets_expands_wildcards_and_config_refs() {
        // The MCP surface must resolve targets exactly like the CLI:
        // {config.X} substitution + wildcard expansion (H34 — the MCP
        // copy used to return raw patterns, so the same Oxymakefile
        // worked in the CLI and failed through the MCP demo).
        let toml = r#"
[config]
results_dir = "out"
samples = ["A", "B"]

[rule.all]
input = ["{config.results_dir}/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["{config.results_dir}/{sample}.txt"]
shell = "echo"
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();
        let targets = resolve_targets_from_workflow(&wf, &[]);
        assert_eq!(targets, vec!["out/A.txt", "out/B.txt"]);
    }

    #[test]
    fn resolve_targets_substitutes_config_in_user_targets() {
        let toml = r#"
[config]
results_dir = "out"

[rule.process]
input = ["data/raw.csv"]
output = ["out/summary.json"]
shell = "echo"
"#;
        let wf = ox_format::parse::parse_workflow(toml, Path::new("test.toml")).unwrap();
        let targets =
            resolve_targets_from_workflow(&wf, &["{config.results_dir}/summary.json".into()]);
        assert_eq!(targets, vec!["out/summary.json"]);
    }

    // -- handle_clean tests --

    #[test]
    fn handle_clean_nonexistent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_clean(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["removed"], 0);
    }

    #[test]
    fn handle_clean_removes_log_files() {
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join(".oxymake/logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("j1.stdout"), "output").unwrap();
        std::fs::write(log_dir.join("j1.stderr"), "error").unwrap();

        let result = handle_clean(&serde_json::json!({"what": "all"}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["removed"], 2);
    }

    #[test]
    fn handle_clean_what_logs_leaves_cache_alone() {
        // `what: "logs"` must remove only log files (H35 — the parameter
        // used to be ignored entirely).
        let dir = tempfile::tempdir().unwrap();
        let oxymake_dir = dir.path().join(".oxymake");
        let log_dir = oxymake_dir.join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("j1.stdout"), "output").unwrap();

        // A real cache store with one orphan entry.
        let out = dir.path().join("gone.txt");
        std::fs::write(&out, "data").unwrap();
        let mut store = ox_cache::CacheStore::open(&oxymake_dir).unwrap();
        store
            .record(
                ox_core::model::ContentHash::from_hex("a".repeat(64)).unwrap(),
                &[out.as_path()],
                None,
            )
            .unwrap();
        store.save().unwrap();
        std::fs::remove_file(&out).unwrap(); // entry is now an orphan

        let result = handle_clean(&serde_json::json!({"what": "logs"}), dir.path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(v["removed"], 1, "one log file removed");

        let store = ox_cache::CacheStore::open(&oxymake_dir).unwrap();
        assert_eq!(store.len(), 1, "cache must be untouched by what=logs");
    }

    #[test]
    fn handle_clean_what_cache_delegates_to_cache_store() {
        // `what: "cache"` must clean the REAL cache store (.oxymake/cache),
        // not the legacy cache.json path which is created nowhere (H35).
        let dir = tempfile::tempdir().unwrap();
        let oxymake_dir = dir.path().join(".oxymake");
        let log_dir = oxymake_dir.join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("j1.stdout"), "output").unwrap();

        let out = dir.path().join("gone.txt");
        std::fs::write(&out, "data").unwrap();
        let mut store = ox_cache::CacheStore::open(&oxymake_dir).unwrap();
        store
            .record(
                ox_core::model::ContentHash::from_hex("a".repeat(64)).unwrap(),
                &[out.as_path()],
                None,
            )
            .unwrap();
        store.save().unwrap();
        std::fs::remove_file(&out).unwrap(); // entry is now an orphan

        let result = handle_clean(&serde_json::json!({"what": "cache"}), dir.path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(v["cache_entries_removed"], 1, "orphan entry removed");

        let store = ox_cache::CacheStore::open(&oxymake_dir).unwrap();
        assert_eq!(store.len(), 0, "orphan entry gone from store");
        assert!(
            log_dir.join("j1.stdout").exists(),
            "logs must be untouched by what=cache"
        );
    }

    #[test]
    fn handle_clean_rejects_unknown_what() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_clean(&serde_json::json!({"what": "outputs"}), dir.path());
        assert!(
            result.is_err(),
            "unimplemented 'what' values must be rejected, not silently accepted"
        );
    }

    // -- handle_status with missing db --

    #[test]
    fn handle_status_no_state_db() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_status(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["error"]["code"], "NO_STATE");
    }

    // -- handle_logs tests --

    #[test]
    fn handle_logs_specific_job() {
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join(".oxymake/logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("j42.stdout"), "hello stdout").unwrap();
        std::fs::write(log_dir.join("j42.stderr"), "hello stderr").unwrap();

        let result = handle_logs(&serde_json::json!({"job_id": "j42"}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["job_id"], "j42");
        assert_eq!(v["stdout"], "hello stdout");
        assert_eq!(v["stderr"], "hello stderr");
    }

    #[test]
    fn handle_logs_missing_params() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_logs(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["error"]["code"], "MISSING_PARAMS");
    }

    // -- handle_lint tests --

    #[test]
    fn handle_lint_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_lint(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["error"]["code"], "OXYMAKEFILE_NOT_FOUND");
    }

    // -- handle_plan tests --

    #[test]
    fn handle_plan_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_plan(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["error"]["code"], "OXYMAKEFILE_NOT_FOUND");
    }

    // -- handle_explain tests --

    #[test]
    fn handle_explain_missing_target_param() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_explain(&serde_json::json!({}), dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn handle_explain_missing_oxymakefile() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_explain(&serde_json::json!({"target": "out.txt"}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["error"]["code"], "OXYMAKEFILE_NOT_FOUND");
    }

    // -- handle_dag tests --

    #[test]
    fn handle_dag_no_state() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_dag(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["error"]["code"], "NO_STATE");
    }

    // -- handle_history tests --

    #[test]
    fn handle_history_no_state() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_history(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["error"]["code"], "NO_STATE");
    }

    // -- Integration tests with seeded data --

    /// Create a temp dir with a seeded state.db and return the dir handle.
    fn seeded_workdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let db_dir = dir.path().join(".oxymake");
        std::fs::create_dir_all(&db_dir).unwrap();
        let db_path = db_dir.join("state.db");
        let db = ox_state::db::StateDb::open(&db_path).unwrap();

        let sid = db.create_session(1, "test-host", None).unwrap();
        let jobs = vec![
            ox_state::db::JobRecord {
                id: "build-A".into(),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            ox_state::db::JobRecord {
                id: "test-A".into(),
                rule_name: "test".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();
        db.register_edges(&[("build-A".into(), "test-A".into())])
            .unwrap();
        db.claim_job("build-A", &sid).unwrap();
        db.complete_job("build-A", &sid, 0, "{}").unwrap();
        db.claim_job("test-A", &sid).unwrap();
        db.fail_job("test-A", &sid, 1).unwrap();

        db.begin_run("run-1", None, 2, Some("test run")).unwrap();
        db.end_run("run-1", 1, 1, 0).unwrap();
        db.begin_run("run-2", None, 1, None).unwrap();
        db.end_run("run-2", 1, 0, 0).unwrap();

        drop(db);
        dir
    }

    #[test]
    fn handle_status_with_seeded_db() {
        let dir = seeded_workdir();
        let result = handle_status(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["jobs"]["completed"], 1);
        assert_eq!(v["jobs"]["failed"], 1);
        assert!(v["jobs"]["total"].as_u64().unwrap() >= 2);
    }

    #[test]
    fn handle_status_with_group_by() {
        let dir = seeded_workdir();
        let result = handle_status(&serde_json::json!({"group_by": "rule"}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        let groups = v["groups"].as_array().unwrap();
        assert!(groups.len() >= 2);
    }

    #[test]
    fn handle_dag_json_with_seeded_db() {
        let dir = seeded_workdir();
        let result = handle_dag(&serde_json::json!({"format": "json"}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        let nodes = v["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        let edges = v["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["from"], "build-A");
        assert_eq!(edges[0]["to"], "test-A");
    }

    #[test]
    fn handle_dag_dot_format() {
        let dir = seeded_workdir();
        let result = handle_dag(&serde_json::json!({"format": "dot"}), dir.path()).unwrap();
        let text = &result.content[0].text;
        assert!(text.contains("digraph oxymake"));
        assert!(text.contains("\"build-A\" -> \"test-A\""));
    }

    #[test]
    fn handle_history_with_seeded_db() {
        let dir = seeded_workdir();
        let result = handle_history(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        let runs = v["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 2);
    }

    #[test]
    fn handle_history_with_limit() {
        let dir = seeded_workdir();
        let result = handle_history(&serde_json::json!({"limit": 1}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        let runs = v["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
    }

    #[test]
    fn handle_logs_failed_jobs() {
        let dir = seeded_workdir();
        // Create log files for the failed job
        let log_dir = dir.path().join(".oxymake/logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("test-A.stdout"), "test output").unwrap();
        std::fs::write(log_dir.join("test-A.stderr"), "assertion failed").unwrap();

        let result = handle_logs(&serde_json::json!({"failed": true}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        let failed = v["failed_jobs"].as_array().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["stderr"], "assertion failed");
    }

    // -- Oxymakefile-based tool tests --

    fn workdir_with_oxymakefile() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"
ox_version = "0.1"

[rule.build]
input = ["src/main.rs"]
output = ["target/main"]
shell = "rustc {input} -o {output}"
"#;
        std::fs::write(dir.path().join("Oxymakefile.toml"), content).unwrap();
        dir
    }

    #[test]
    fn handle_lint_valid_oxymakefile() {
        let dir = workdir_with_oxymakefile();
        let result = handle_lint(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["valid"], true);
        assert_eq!(v["diagnostics"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn handle_lint_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Oxymakefile.toml"),
            "this is not valid toml [[[",
        )
        .unwrap();
        let result = handle_lint(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["valid"], false);
        assert!(!v["diagnostics"].as_array().unwrap().is_empty());
    }

    #[test]
    fn handle_lint_custom_filename() {
        let dir = tempfile::tempdir().unwrap();
        let result = handle_lint(&serde_json::json!({"file": "Custom.toml"}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["error"]["code"], "OXYMAKEFILE_NOT_FOUND");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Custom.toml")
        );
    }

    #[test]
    fn handle_plan_valid_workflow() {
        let dir = workdir_with_oxymakefile();
        // Create the input file so the resolver can discover it
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let result = handle_plan(&serde_json::json!({}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(v.get("level").is_some());
        assert!(v["total_nodes"].as_u64().is_some());
    }

    #[test]
    fn handle_plan_with_user_targets() {
        let dir = workdir_with_oxymakefile();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let result =
            handle_plan(&serde_json::json!({"targets": ["target/main"]}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(v.get("level").is_some());
    }

    #[test]
    fn handle_explain_valid_target() {
        let dir = workdir_with_oxymakefile();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let result =
            handle_explain(&serde_json::json!({"target": "target/main"}), dir.path()).unwrap();
        let text = &result.content[0].text;
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["target"], "target/main");
        assert!(v.get("dependency_chain").is_some());
        assert!(v.get("total_steps").is_some());
    }

    // -- build_config tests --

    #[test]
    fn build_config_from_workflow_with_lists_and_scalars() {
        let mut wf = empty_workflow();
        wf.config.insert(
            "samples".into(),
            ox_format::parse::ConfigValue::List(vec!["S001".into(), "S002".into()]),
        );
        wf.config.insert(
            "genome".into(),
            ox_format::parse::ConfigValue::Scalar("hg38".into()),
        );
        let config = build_config(&wf);
        assert_eq!(config.lists["samples"], vec!["S001", "S002"]);
        assert_eq!(config.scalars["genome"], "hg38");
    }

    #[test]
    fn build_config_empty_workflow() {
        let wf = empty_workflow();
        let config = build_config(&wf);
        assert!(config.lists.is_empty());
        assert!(config.scalars.is_empty());
    }

    // -- resolve_targets_from_workflow with rules --

    #[test]
    fn resolve_targets_uses_first_rule_outputs() {
        let toml = r#"
ox_version = "0.1"

[rule.build]
input = ["src/main.rs"]
output = ["out.txt"]
shell = "echo hello > out.txt"
"#;
        let wf = ox_format::parse::parse_workflow(toml, std::path::Path::new("test.toml")).unwrap();
        let targets = resolve_targets_from_workflow(&wf, &[]);
        assert_eq!(targets, vec!["out.txt"]);
    }

    #[test]
    fn resolve_targets_prefers_all_rule() {
        let toml = r#"
ox_version = "0.1"

[rule.all]
input = ["final.txt"]

[rule.build]
input = ["src/main.rs"]
output = ["final.txt"]
shell = "echo done > final.txt"
"#;
        let wf = ox_format::parse::parse_workflow(toml, std::path::Path::new("test.toml")).unwrap();
        let targets = resolve_targets_from_workflow(&wf, &[]);
        // "all" rule has no outputs, so uses its inputs as targets
        assert_eq!(targets, vec!["final.txt"]);
    }

    // -- discover_files tests --

    #[test]
    fn discover_files_finds_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Oxymakefile.toml"), "").unwrap();
        std::fs::write(dir.path().join("data.csv"), "a,b").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();

        let files = discover_files(&dir.path().join("Oxymakefile.toml"));
        let file_strs: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        assert!(file_strs.iter().any(|f| f.contains("data.csv")));
        assert!(file_strs.iter().any(|f| f.contains("main.rs")));
    }

    #[test]
    fn discover_files_skips_hidden_and_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Oxymakefile.toml"), "").unwrap();
        std::fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        std::fs::write(dir.path().join(".hidden/secret"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/binary"), "").unwrap();

        let files = discover_files(&dir.path().join("Oxymakefile.toml"));
        let file_strs: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        assert!(!file_strs.iter().any(|f| f.contains("secret")));
        assert!(!file_strs.iter().any(|f| f.contains("binary")));
    }
}
