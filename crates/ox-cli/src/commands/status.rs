//! Implementation of `ox status` — show current workflow execution state.
//!
//! Automatically detects Ray DAG submissions from state.db and polls the
//! Ray Jobs API for driver status using the specific driver job ID from
//! meta.json (follow mode). For SLURM, syncs results from sacct using
//! the job mapping in meta.json.
//! Use `--ray-address` to override the auto-detected Ray dashboard address.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct StatusArgs {
    /// Show status for a specific run ID (default: latest run)
    #[arg(long)]
    pub run: Option<String>,

    /// Group output by field
    #[arg(long)]
    pub group_by: Option<String>,

    /// Output JSON
    #[arg(long)]
    pub json: bool,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,

    /// Ray dashboard address for remote DAG status (e.g., http://127.0.0.1:8265)
    #[arg(long)]
    pub ray_address: Option<String>,
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// Supported `--group-by` dimensions.  Currently only `rule_name` is stored
/// in the database; `stage` is accepted as an alias (the TUI already treats
/// rule names as pipeline stages).
const VALID_GROUP_BY: &[&str] = &["rule", "stage"];

/// Format a duration in seconds into a human-readable string.
fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m{}s", secs / 3600, (secs % 3600) / 60, secs % 60)
    }
}

/// Format a job's display name from its rule name and wildcard bindings.
fn format_job_name(rule_name: &str, wildcards_json: &str) -> String {
    // Parse wildcards; if non-empty, append them to make the name unique.
    if let Ok(map) =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(wildcards_json)
    {
        if map.is_empty() {
            return rule_name.to_string();
        }
        let vals: Vec<String> = map
            .values()
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        format!("{}[{}]", rule_name, vals.join(","))
    } else {
        rule_name.to_string()
    }
}

pub fn cmd_status(args: StatusArgs, theme: &ox_render::Theme) -> Result<()> {
    let db_path = Path::new(".oxymake/state.db");
    if !db_path.exists() {
        if args.json {
            // Emit a structured object so an agent parsing stdout as JSON does
            // not crash on a human-readable sentence. Absence of state is a
            // valid empty state, not an error — exit 0.
            println!(
                "{}",
                serde_json::json!({
                    "state": "absent",
                    "message": "No OxyMake state found.",
                    "hint": "Run 'ox run' first.",
                })
            );
        } else {
            println!("No OxyMake state found. Run 'ox run' first.");
        }
        return Ok(());
    }

    // Validate --group-by early.
    if let Some(ref g) = args.group_by {
        if !VALID_GROUP_BY.contains(&g.as_str()) {
            anyhow::bail!(
                "unsupported --group-by value '{}'; valid values: {}",
                g,
                VALID_GROUP_BY.join(", ")
            );
        }
    }

    let db = ox_state::db::StateDb::open(db_path)?;

    // Resolve the target run: explicit --run, or latest from the runs table.
    let run_id = match args.run {
        Some(ref id) => Some(id.clone()),
        None => db.latest_run_id()?,
    };

    // Determine executor type and Ray address for the target run.
    // Scope detection to the current run to avoid bleeding metadata from
    // prior runs (e.g., showing Ray info after a local run).
    let (executor_type, ray_addr) = if let Some(ref addr) = args.ray_address {
        ("ray".to_string(), Some(addr.clone()))
    } else if let Some(ref rid) = run_id {
        match db.dag_submission_for_run(rid) {
            Ok(Some((executor, addr))) => {
                let ray = if executor == "ray" { addr } else { None };
                (executor, ray)
            }
            _ => {
                // No dag_submission for this run — check meta.json scoped to run.
                if let Some(meta) = read_run_meta(rid) {
                    let executor = meta
                        .get("executor")
                        .and_then(|v| v.as_str())
                        .unwrap_or("local")
                        .to_string();
                    let ray = if executor == "ray" {
                        meta.get("ray_address")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    } else {
                        None
                    };
                    (executor, ray)
                } else {
                    ("local".to_string(), None)
                }
            }
        }
    } else {
        ("local".to_string(), None)
    };

    // Sync results from the remote executor *before* reading counts so
    // that the displayed state is up-to-date on the very first call.
    if let Some(ref rid) = run_id {
        match executor_type.as_str() {
            "ray" => sync_results_from_driver(&db),
            "slurm" => sync_results_from_sacct(&db, rid),
            _ => {}
        }
    }

    // Scope queries to the target run when available.  Fall back to the
    // unscoped (all-jobs) queries when no run is recorded yet (e.g., old
    // databases that predate the run_id column on jobs).
    let counts = match run_id {
        Some(ref rid) => db.job_counts_for_run(rid)?,
        None => db.job_counts()?,
    };
    let sessions = db.active_sessions()?;

    let total = counts.pending
        + counts.running
        + counts.completed
        + counts.failed
        + counts.skipped
        + counts.cancelled;

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if args.json {
        let running_jobs = match run_id {
            Some(ref rid) => db.running_jobs_detail_for_run(rid)?,
            None => db.running_jobs_detail()?,
        };
        let pending_jobs = match run_id {
            Some(ref rid) => db.pending_jobs_with_blockers_for_run(rid)?,
            None => db.pending_jobs_with_blockers()?,
        };

        let running_json: Vec<serde_json::Value> = running_jobs
            .iter()
            .map(|j| {
                let elapsed = j.started_at.map(|s| now_secs.saturating_sub(s));
                serde_json::json!({
                    "name": format_job_name(&j.rule_name, &j.wildcards),
                    "status": "running",
                    "elapsed_secs": elapsed,
                })
            })
            .collect();

        let pending_json: Vec<serde_json::Value> = pending_jobs
            .iter()
            .map(|j| {
                serde_json::json!({
                    "name": format_job_name(&j.rule_name, &j.wildcards),
                    "status": "pending",
                    "waiting_for": j.waiting_for,
                })
            })
            .collect();

        let mut json = serde_json::json!({
            "run_id": run_id,
            "executor": executor_type,
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
            "running_jobs": running_json,
            "pending_jobs": pending_json,
        });

        if args.group_by.is_some() {
            let stats = match run_id {
                Some(ref rid) => db.pipeline_stats_for_run(rid)?,
                None => db.pipeline_stats()?,
            };
            let groups: Vec<serde_json::Value> = stats
                .iter()
                .map(|s| {
                    let pending = s.total.saturating_sub(s.completed + s.running);
                    let progress_pct = if s.total == 0 {
                        0.0
                    } else {
                        (s.completed as f64 / s.total as f64) * 100.0
                    };
                    serde_json::json!({
                        "name": s.rule_name,
                        "total": s.total,
                        "completed": s.completed,
                        "running": s.running,
                        "pending": pending,
                        "progress_pct": (progress_pct * 100.0).round() / 100.0,
                    })
                })
                .collect();
            json.as_object_mut()
                .unwrap()
                .insert("groups".to_string(), serde_json::Value::Array(groups));
        }

        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        // ------ Run header ------
        if let Some(ref rid) = run_id {
            println!(
                "{} {}  ({})",
                theme.header.apply_to("Run:"),
                theme.highlight.apply_to(rid),
                theme.info.apply_to(&executor_type)
            );
        }
        println!(
            "{} {} active",
            theme.header.apply_to("Sessions:"),
            sessions.len()
        );
        println!(
            "{} {} total ({}, {}, {}, {} pending, {} cached, {} cancelled)",
            theme.header.apply_to("Jobs:"),
            total,
            theme
                .success
                .apply_to(format!("{} completed", counts.completed)),
            theme
                .running
                .apply_to(format!("{} running", counts.running)),
            theme.error.apply_to(format!("{} failed", counts.failed)),
            counts.pending,
            counts.cached,
            counts.cancelled
        );

        // ------ Running jobs with elapsed time ------
        if counts.running > 0 {
            let running_jobs = match run_id {
                Some(ref rid) => db.running_jobs_detail_for_run(rid)?,
                None => db.running_jobs_detail()?,
            };
            println!(
                "{} {} jobs in progress",
                theme.running.apply_to("Running:"),
                running_jobs.len()
            );
            for j in &running_jobs {
                let name = format_job_name(&j.rule_name, &j.wildcards);
                let elapsed = j
                    .started_at
                    .map(|s| format_elapsed(now_secs.saturating_sub(s)))
                    .unwrap_or_else(|| "?".to_string());
                println!(
                    "  {:<30} {}  {}",
                    theme.highlight.apply_to(&name),
                    theme.running.apply_to("running"),
                    theme.muted.apply_to(&elapsed)
                );
            }
            println!();
        }

        // ------ Pending jobs with blockers ------
        if counts.pending > 0 {
            let pending_jobs = match run_id {
                Some(ref rid) => db.pending_jobs_with_blockers_for_run(rid)?,
                None => db.pending_jobs_with_blockers()?,
            };
            println!(
                "{} {} jobs waiting",
                theme.muted.apply_to("Pending:"),
                pending_jobs.len()
            );
            for j in &pending_jobs {
                let name = format_job_name(&j.rule_name, &j.wildcards);
                if j.waiting_for.is_empty() {
                    println!(
                        "  {:<30} {}",
                        theme.highlight.apply_to(&name),
                        theme.success.apply_to("ready")
                    );
                } else {
                    let blockers = j.waiting_for.join(", ");
                    println!(
                        "  {:<30} waiting for: {}",
                        theme.highlight.apply_to(&name),
                        theme.muted.apply_to(&blockers)
                    );
                }
            }
            println!();
        }

        // ------ Group-by breakdown ------
        if args.group_by.is_some() {
            let stats = match run_id {
                Some(ref rid) => db.pipeline_stats_for_run(rid)?,
                None => db.pipeline_stats()?,
            };
            for s in &stats {
                let progress_pct = if s.total == 0 {
                    0.0
                } else {
                    (s.completed as f64 / s.total as f64) * 100.0
                };
                println!(
                    "  {:<20} {}/{} ({:.1}%)  {} running",
                    theme.highlight.apply_to(&s.rule_name),
                    s.completed,
                    s.total,
                    progress_pct,
                    s.running
                );
            }
        }
    }

    if let Some(ref addr) = ray_addr {
        // Print run metadata from meta.json if available.
        if !args.json {
            if let Some(ref rid) = run_id {
                print_run_meta(rid, theme);
            }
        }
        sync_and_print_ray_status(addr, run_id.as_deref(), args.json, theme)?;
    }

    Ok(())
}

/// Read and parse a specific run's meta.json file.
fn read_run_meta(run_id: &str) -> Option<serde_json::Value> {
    let meta_path = std::path::Path::new(".oxymake/runs")
        .join(run_id)
        .join("meta.json");
    let content = std::fs::read_to_string(&meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Print Ray run metadata from a specific run's meta.json file.
fn print_run_meta(run_id: &str, theme: &ox_render::Theme) {
    let meta_path = std::path::Path::new(".oxymake/runs")
        .join(run_id)
        .join("meta.json");

    let content = match std::fs::read_to_string(&meta_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let meta: serde_json::Value = match serde_json::from_str(&content) {
        Ok(m) => m,
        Err(_) => return,
    };

    if meta.get("executor").and_then(|v| v.as_str()) != Some("ray") {
        return;
    }

    let ray_addr = meta
        .get("ray_address")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let ray_job_id = meta
        .get("ray_job_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let active = meta
        .get("active_jobs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let skipped = meta
        .get("skipped_jobs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total = meta.get("total_jobs").and_then(|v| v.as_u64()).unwrap_or(0);

    println!(
        "\n{} {}",
        theme.header.apply_to("Executor:"),
        theme.info.apply_to("Ray")
    );
    println!("  Dashboard: {}", theme.highlight.apply_to(ray_addr));
    println!("  Driver job: {}", theme.muted.apply_to(ray_job_id));
    println!(
        "  Jobs: {} active, {} cached, {} total",
        theme.running.apply_to(active),
        theme.cached.apply_to(skipped),
        total
    );
}

/// Poll the Ray Jobs API for the specific driver job ID (follow mode).
///
/// Instead of listing all jobs on the cluster and filtering, reads the
/// driver submission ID from meta.json and polls its status directly.
/// Falls back to listing all OxyMake DAG drivers if no run-specific
/// driver ID is available.
fn sync_and_print_ray_status(
    ray_address: &str,
    run_id: Option<&str>,
    json_output: bool,
    theme: &ox_render::Theme,
) -> Result<()> {
    // Use a blocking tokio runtime since cmd_status is sync.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        let http_client = match ox_exec_ray::ray_client_http(std::time::Duration::from_secs(10)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("\nFailed to create HTTP client: {}", e);
                return Ok(());
            }
        };

        let ray_client =
            ox_exec_ray::ray_client::RayClient::new(ray_address.to_string(), http_client);

        // Follow mode: poll the specific driver job ID from meta.json.
        if let Some(rid) = run_id {
            if let Some(driver_job_id) = read_run_meta(rid).and_then(|m| {
                m.get("ray_job_id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            }) {
                match ray_client.get_job_details(&driver_job_id).await {
                    Ok(details) => {
                        let status = format!("{:?}", details.status).to_uppercase();
                        if json_output {
                            println!(
                                "\n{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "ray_driver": {
                                        "run_id": rid,
                                        "driver_job_id": driver_job_id,
                                        "status": status,
                                        "message": details.message,
                                    }
                                }))?
                            );
                        } else {
                            let styled_status = match status.as_str() {
                                "RUNNING" | "PENDING" => theme.running.apply_to(&status),
                                "SUCCEEDED" => theme.success.apply_to(&status),
                                "FAILED" | "STOPPED" => theme.error.apply_to(&status),
                                _ => theme.muted.apply_to(&status),
                            };
                            println!(
                                "\n{} driver {} ({})",
                                theme.header.apply_to("Ray:"),
                                styled_status,
                                theme.muted.apply_to(&driver_job_id),
                            );
                            if let Some(ref msg) = details.message {
                                if !msg.is_empty() {
                                    println!("  {}", theme.muted.apply_to(msg));
                                }
                            }
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!("\nFailed to poll Ray driver job {}: {}", driver_job_id, e);
                        // Fall through to list-all mode.
                    }
                }
            }
        }

        // Fallback: list all OxyMake DAG driver jobs on the cluster.
        let job_list = match ray_client.list_jobs().await {
            Ok(list) => list,
            Err(e) => {
                eprintln!("\nRay cluster at {} unreachable: {}", ray_address, e);
                return Ok(());
            }
        };

        let driver_jobs: Vec<&ox_exec_ray::JobSummary> = job_list
            .data
            .iter()
            .filter(|j| {
                j.metadata
                    .as_ref()
                    .and_then(|m| m.get("oxymake_dag_driver"))
                    .map(|v| v == "true")
                    .unwrap_or(false)
            })
            .collect();

        if driver_jobs.is_empty() {
            if !json_output {
                eprintln!("\nNo OxyMake DAG submissions found on Ray cluster.");
            }
            return Ok(());
        }

        if json_output {
            let ray_status: Vec<serde_json::Value> = driver_jobs
                .iter()
                .map(|j| {
                    let run_id = j
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("oxymake_run_id"))
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    let total: usize = j
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("oxymake_dag_total_jobs"))
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    serde_json::json!({
                        "run_id": run_id,
                        "driver_status": j.status,
                        "total_jobs": total,
                    })
                })
                .collect();
            println!(
                "\n{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "ray_dag_submissions": ray_status
                }))?
            );
        } else {
            println!("\n{}", theme.header.apply_to("--- Ray DAG Submissions ---"));
            for j in &driver_jobs {
                let run_id = j
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("oxymake_run_id"))
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                let total: usize = j
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("oxymake_dag_total_jobs"))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let status_upper = j.status.to_uppercase();
                let styled_status = match status_upper.as_str() {
                    "RUNNING" | "PENDING" => theme.running.apply_to(&status_upper),
                    "SUCCEEDED" => theme.success.apply_to(&status_upper),
                    "FAILED" | "STOPPED" => theme.error.apply_to(&status_upper),
                    _ => theme.muted.apply_to(&status_upper),
                };
                println!(
                    "  Run {}: driver {} ({} tasks)",
                    theme.highlight.apply_to(&run_id),
                    styled_status,
                    total,
                );
            }
        }

        Ok(())
    })
}

/// Read results.json files from .oxymake/runs/{run_id}/ and sync job
/// completion status back to state.db.
fn sync_results_from_driver(db: &ox_state::db::StateDb) {
    let runs_dir = std::path::Path::new(".oxymake/runs");
    let entries = match std::fs::read_dir(runs_dir) {
        Ok(e) => e,
        Err(_) => return, // No runs directory yet.
    };

    // Create a sync session so claim_job has a valid session_id reference.
    let sync_sid = match db.create_session(std::process::id(), "sync", None) {
        Ok(sid) => sid,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let results_path = entry.path().join("results.json");
        if !results_path.exists() {
            continue;
        }

        let content = match std::fs::read_to_string(&results_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let results: std::collections::HashMap<String, serde_json::Value> =
            match serde_json::from_str(&content) {
                Ok(r) => r,
                Err(_) => continue,
            };

        for (job_id, info) in &results {
            let status = info
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let exit_code = info.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(1) as i32;

            match status {
                "completed" => {
                    // Claim first so the job transitions pending→running,
                    // then running→completed. The executor's results.json
                    // is the authority on terminal state here, so the
                    // session-agnostic reconcile variant is correct even
                    // when the job was claimed by the original run session.
                    let _ = db.claim_job(job_id, &sync_sid);
                    let _ = db.reconcile_complete_job(job_id, exit_code, "");
                }
                "failed" => {
                    let _ = db.claim_job(job_id, &sync_sid);
                    let _ = db.reconcile_fail_job(job_id, exit_code);
                }
                _ => {}
            }
        }
    }
}

/// Sync SLURM job results from sacct back to state.db.
///
/// Reads the job_mapping from the run's meta.json (OxyMake job ID →
/// SLURM job ID), queries sacct for each job's status, and writes
/// terminal results back to state.db.
fn sync_results_from_sacct(db: &ox_state::db::StateDb, run_id: &str) {
    let meta = match read_run_meta(run_id) {
        Some(m) => m,
        None => return,
    };

    if meta.get("executor").and_then(|v| v.as_str()) != Some("slurm") {
        return;
    }

    let job_mapping = match meta.get("job_mapping").and_then(|v| v.as_object()) {
        Some(m) => m,
        None => return,
    };

    // Collect SLURM job IDs to query in a single sacct call.
    let mut ox_to_slurm: Vec<(String, u32)> = Vec::new();
    for (ox_id, slurm_val) in job_mapping {
        if let Some(slurm_id) = slurm_val
            .as_str()
            .and_then(|s| s.parse::<u32>().ok())
            .or_else(|| slurm_val.as_u64().map(|n| n as u32))
        {
            ox_to_slurm.push((ox_id.clone(), slurm_id));
        }
    }

    if ox_to_slurm.is_empty() {
        return;
    }

    let slurm_ids: Vec<u32> = ox_to_slurm.iter().map(|(_, sid)| *sid).collect();

    // Build a reverse map: SLURM ID → OxyMake job ID.
    let slurm_to_ox: std::collections::HashMap<u32, &str> = ox_to_slurm
        .iter()
        .map(|(ox_id, sid)| (*sid, ox_id.as_str()))
        .collect();

    // Query sacct (requires a tokio runtime since slurm_cli is async).
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return,
    };

    let records = match rt.block_on(ox_exec_slurm::slurm_cli::sacct(&slurm_ids)) {
        Ok(r) => r,
        Err(_) => return, // sacct not available — skip silently.
    };

    for record in &records {
        if !ox_exec_slurm::status_parser::is_terminal(&record.state) {
            continue;
        }

        let ox_id = match slurm_to_ox.get(&record.job_id) {
            Some(id) => *id,
            None => continue,
        };

        let job_status = ox_exec_slurm::status_parser::slurm_state_to_job_status(&record.state);

        match job_status {
            // sacct is the authority on terminal state for SLURM jobs;
            // the syncing process is not the session that claimed them,
            // so the session-agnostic reconcile variants are correct.
            ox_core::traits::executor::JobStatus::Completed => {
                let _ = db.reconcile_complete_job(ox_id, record.exit_code, "");
            }
            ox_core::traits::executor::JobStatus::Failed(_) => {
                let _ = db.reconcile_fail_job(ox_id, record.exit_code);
            }
            ox_core::traits::executor::JobStatus::Cancelled => {
                let _ = db.reconcile_fail_job(ox_id, -1);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_run_meta_returns_none_for_missing_dir() {
        assert!(read_run_meta("nonexistent-run-id-xyz").is_none());
    }

    #[test]
    fn ray_meta_json_round_trip() {
        let meta = serde_json::json!({
            "executor": "ray",
            "ray_address": "http://127.0.0.1:8265",
            "ray_job_id": "raysubmit_abc123",
            "run_id": "run-001",
            "total_jobs": 10,
            "active_jobs": 8,
            "skipped_jobs": 2,
        });

        assert_eq!(meta.get("executor").and_then(|v| v.as_str()), Some("ray"));
        assert_eq!(
            meta.get("ray_job_id").and_then(|v| v.as_str()),
            Some("raysubmit_abc123")
        );
        assert_eq!(
            meta.get("ray_address").and_then(|v| v.as_str()),
            Some("http://127.0.0.1:8265")
        );
    }

    #[test]
    fn slurm_meta_json_round_trip() {
        let meta = serde_json::json!({
            "executor": "slurm",
            "version": 1,
            "run_id": "run-002",
            "total_jobs": 5,
            "active_jobs": 3,
            "skipped_jobs": 2,
            "job_mapping": {
                "job-a": "12345",
                "job-b": "12346",
                "job-c": "12347",
            },
        });

        assert_eq!(meta.get("executor").and_then(|v| v.as_str()), Some("slurm"));
        let mapping = meta.get("job_mapping").and_then(|v| v.as_object()).unwrap();
        assert_eq!(mapping.len(), 3);
        assert_eq!(mapping.get("job-a").and_then(|v| v.as_str()), Some("12345"));
    }

    #[test]
    fn results_json_parsing() {
        let results_str = r#"{"job-a":{"status":"completed","exit_code":0},"job-b":{"status":"failed","exit_code":1},"job-c":{"status":"running"}}"#;
        let parsed: std::collections::HashMap<String, serde_json::Value> =
            serde_json::from_str(results_str).unwrap();

        assert_eq!(parsed.len(), 3);
        assert_eq!(
            parsed["job-a"].get("status").and_then(|v| v.as_str()),
            Some("completed")
        );
        assert_eq!(
            parsed["job-b"].get("exit_code").and_then(|v| v.as_i64()),
            Some(1)
        );
    }

    #[test]
    fn slurm_job_mapping_parses_string_and_numeric_ids() {
        let meta = serde_json::json!({
            "executor": "slurm",
            "job_mapping": {
                "job-a": "12345",
                "job-b": 12346,
            },
        });

        let mapping = meta.get("job_mapping").and_then(|v| v.as_object()).unwrap();
        let mut parsed: Vec<(String, u32)> = Vec::new();
        for (ox_id, slurm_val) in mapping {
            if let Some(slurm_id) = slurm_val
                .as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .or_else(|| slurm_val.as_u64().map(|n| n as u32))
            {
                parsed.push((ox_id.clone(), slurm_id));
            }
        }

        assert_eq!(parsed.len(), 2);
        parsed.sort_by_key(|(_, id)| *id);
        assert_eq!(parsed[0], ("job-a".to_string(), 12345));
        assert_eq!(parsed[1], ("job-b".to_string(), 12346));
    }

    #[test]
    fn format_elapsed_formatting() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(59), "59s");
        assert_eq!(format_elapsed(60), "1m0s");
        assert_eq!(format_elapsed(3661), "1h1m1s");
    }

    #[test]
    fn format_job_name_no_wildcards() {
        assert_eq!(format_job_name("build", "{}"), "build");
    }

    #[test]
    fn format_job_name_with_wildcards() {
        assert_eq!(format_job_name("build", r#"{"arch":"x86"}"#), "build[x86]");
    }
}
