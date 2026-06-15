//! Implementation of `ox history` — display past runs and job execution details.

use std::path::Path;

use anyhow::Result;

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct HistoryArgs {
    /// Show details for a specific run ID (e.g., "run-1234")
    #[arg(long)]
    pub run_id: Option<String>,

    /// Output NDJSON (one JSON object per line)
    #[arg(long)]
    pub json: bool,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

pub fn cmd_history(args: HistoryArgs, theme: &ox_render::Theme) -> Result<()> {
    let db_path = Path::new(".oxymake/state.db");
    if !db_path.exists() {
        println!("No OxyMake state found. Run 'ox run' first.");
        return Ok(());
    }

    let db = ox_state::db::StateDb::open(db_path)?;

    match args.run_id {
        Some(ref run_id) => show_run_detail(&db, run_id, args.json, theme),
        None => list_runs(&db, args.json, theme),
    }
}

// ---------------------------------------------------------------------------
// List runs
// ---------------------------------------------------------------------------

fn list_runs(db: &ox_state::db::StateDb, json: bool, theme: &ox_render::Theme) -> Result<()> {
    let runs = db.list_runs()?;
    if runs.is_empty() {
        if json {
            // Emit nothing (empty NDJSON stream)
        } else {
            println!("No runs found.");
        }
        return Ok(());
    }

    if json {
        for run in &runs {
            let slowest = db.slowest_rules_for_run(&run.id, 3).unwrap_or_default();
            let slowest_json: Vec<_> = slowest
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "rule": s.rule_name,
                        "total_ms": s.total_ms,
                        "max_ms": s.max_ms,
                        "job_count": s.job_count,
                    })
                })
                .collect();
            let obj = serde_json::json!({
                "run_id": run.id,
                "started_at": format_iso8601(run.started_at),
                "completed_at": run.completed_at.map(format_iso8601),
                "note": run.note,
                "workflow_hash": run.workflow_hash,
                "total_jobs": run.job_count,
                "succeeded": run.succeeded,
                "failed": run.failed,
                "skipped": run.skipped,
                "slowest_rules": slowest_json,
            });
            println!("{}", serde_json::to_string(&obj)?);
        }
    } else {
        let header = format!(
            "{:<16} {:>20} {:>10} {:>6} {:>6} {:>6}  NOTE",
            "RUN", "STARTED", "DURATION", "OK", "FAIL", "SKIP"
        );
        println!("{}", theme.header.apply_to(&header));
        println!("{}", theme.muted.apply_to("-".repeat(80)));
        for run in &runs {
            let started = format_timestamp(run.started_at);
            let duration = match run.completed_at {
                Some(end) => format_duration_secs(end.saturating_sub(run.started_at)),
                None => "running".to_string(),
            };
            let ok = run.succeeded.map_or("-".to_string(), |v| v.to_string());
            let fail = run.failed.map_or("-".to_string(), |v| v.to_string());
            let skip = run.skipped.map_or("-".to_string(), |v| v.to_string());
            let note = run.note.as_deref().unwrap_or("");

            let has_failures = run.failed.unwrap_or(0) > 0;
            println!(
                "{:<16} {:>20} {:>10} {:>6} {:>6} {:>6}  {}",
                theme.highlight.apply_to(&run.id),
                theme.muted.apply_to(&started),
                theme.muted.apply_to(&duration),
                theme.success.apply_to(&ok),
                if has_failures {
                    theme.error.apply_to(&fail)
                } else {
                    theme.muted.apply_to(&fail)
                },
                theme.muted.apply_to(&skip),
                note
            );

            // Show slowest rules for completed runs (ox-mnbb).
            let slowest = db.slowest_rules_for_run(&run.id, 3).unwrap_or_default();
            if !slowest.is_empty() {
                let parts: Vec<String> = slowest
                    .iter()
                    .map(|s| {
                        format!(
                            "{} ({})",
                            theme.highlight.apply_to(&s.rule_name),
                            theme.muted.apply_to(format_duration_ms(s.total_ms))
                        )
                    })
                    .collect();
                println!("  slowest: {}", parts.join(", "));
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Show run detail
// ---------------------------------------------------------------------------

fn show_run_detail(
    db: &ox_state::db::StateDb,
    run_id: &str,
    json: bool,
    theme: &ox_render::Theme,
) -> Result<()> {
    let jobs = db.job_history_for_run(run_id)?;
    if jobs.is_empty() {
        if json {
            // Emit nothing
        } else {
            println!("No job history found for run '{run_id}'.");
        }
        return Ok(());
    }

    if json {
        for job in &jobs {
            let obj = serde_json::json!({
                "run_id": job.run_id,
                "job_id": job.job_id,
                "rule_name": job.rule_name,
                "wildcards": job.wildcards,
                "executor": job.executor,
                "hostname": job.hostname,
                "started_at": job.started_at.map(format_iso8601),
                "completed_at": job.completed_at.map(format_iso8601),
                "wall_time_ms": job.wall_time_ms,
                "peak_mem_mb": job.peak_mem_mb,
                "exit_code": job.exit_code,
            });
            println!("{}", serde_json::to_string(&obj)?);
        }
    } else {
        println!(
            "{} {}",
            theme.header.apply_to("Run:"),
            theme.highlight.apply_to(run_id)
        );
        println!();
        let header = format!(
            "{:<24} {:<20} {:>10} {:>8} {:>6}",
            "JOB", "RULE", "WALL TIME", "MEM MB", "EXIT"
        );
        println!("{}", theme.header.apply_to(&header));
        println!("{}", theme.muted.apply_to("-".repeat(72)));
        for job in &jobs {
            let wall = job.wall_time_ms.map_or("-".to_string(), format_duration_ms);
            let mem = job.peak_mem_mb.map_or("-".to_string(), |v| v.to_string());
            let exit_code = job.exit_code;
            let exit = exit_code.map_or("-".to_string(), |v| v.to_string());
            let exit_styled = match exit_code {
                Some(0) => theme.success.apply_to(&exit),
                Some(_) => theme.error.apply_to(&exit),
                None => theme.muted.apply_to(&exit),
            };
            println!(
                "{:<24} {:<20} {:>10} {:>8} {:>6}",
                theme.highlight.apply_to(&job.job_id),
                theme.info.apply_to(&job.rule_name),
                theme.muted.apply_to(&wall),
                theme.muted.apply_to(&mem),
                exit_styled
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Formatting helpers
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

fn format_timestamp(ts: u64) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let dt = UNIX_EPOCH + Duration::from_secs(ts);
    // Format as ISO-ish local time using libc/chrono-free approach:
    // Since we don't have chrono, use a compact UTC representation.
    let secs = ts;
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;

    // Days since 1970-01-01
    let (y, mo, d) = days_to_ymd(days);
    let _ = dt; // suppress unused warning
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Civil days → (year, month, day). Adapted from Howard Hinnant's algorithm.
    days += 719_468;
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

fn format_duration_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format_duration_secs(ms / 1000)
    }
}
