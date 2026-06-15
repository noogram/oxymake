//! Implementation of `ox logs` — display captured stdout/stderr for jobs.

use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct LogsArgs {
    /// Job ID to show logs for
    pub job_id: Option<String>,

    /// Filter by rule name (show logs for all matching jobs)
    #[arg(long)]
    pub rule: Option<String>,

    /// Show only failed jobs
    #[arg(long)]
    pub failed: bool,

    /// Follow (tail -f) the log for an active job
    #[arg(short = 'f', long)]
    pub follow: bool,

    /// Output structured JSON
    #[arg(long)]
    pub json: bool,

    /// Path to state.db
    #[arg(long, default_value = ".oxymake/state.db")]
    pub db: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Default log directory used by the local executor.
const DEFAULT_LOG_DIR: &str = ".oxymake/logs";

/// Resolve the log file path for a job.
///
/// Tries (in order):
/// 1. `log_path` stored in the database
/// 2. Convention: `{DEFAULT_LOG_DIR}/{job_id}.log`
fn resolve_log_path(db_log_path: Option<&str>, job_id: &str) -> PathBuf {
    if let Some(p) = db_log_path {
        PathBuf::from(p)
    } else {
        PathBuf::from(DEFAULT_LOG_DIR).join(format!("{job_id}.log"))
    }
}

/// Print a single job's log content.
fn print_log(path: &Path, job_id: &str, json: bool) -> Result<()> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("cannot read log file: {}", path.display()))?;

    if json {
        let json_val = serde_json::json!({
            "job_id": job_id,
            "log_path": path.to_string_lossy(),
            "content": contents,
        });
        println!("{}", serde_json::to_string(&json_val)?);
    } else {
        print!("{contents}");
    }
    Ok(())
}

/// Follow a log file (tail -f style), printing new lines as they appear.
fn follow_log(path: &Path) -> Result<()> {
    let file = fs::File::open(path)
        .with_context(|| format!("cannot open log file: {}", path.display()))?;
    let mut reader = BufReader::new(file);

    // Print existing content first.
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        print!("{line}");
    }

    // Now poll for new content.
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            // Check if file was truncated (e.g., log rotation).
            let metadata = reader.get_ref().metadata()?;
            let pos = reader.stream_position()?;
            if metadata.len() < pos {
                reader.seek(SeekFrom::Start(0))?;
            }
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        print!("{line}");
    }
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

pub fn cmd_logs(args: LogsArgs, theme: &ox_render::Theme) -> Result<()> {
    let db_path = Path::new(&args.db);

    // If a specific job_id is given and no DB exists, try the convention path.
    if let Some(ref job_id) = args.job_id {
        if !db_path.exists() {
            let log_path = resolve_log_path(None, job_id);
            if !log_path.exists() {
                bail!(
                    "No OxyMake state found and no log file at {}. Run 'ox run' first.",
                    log_path.display()
                );
            }
            if args.follow {
                return follow_log(&log_path);
            }
            return print_log(&log_path, job_id, args.json);
        }
    } else if !db_path.exists() {
        bail!("No OxyMake state found. Run 'ox run' first.");
    }

    let db = ox_state::db::StateDb::open(db_path)?;

    if let Some(ref job_id) = args.job_id {
        // Single job mode.
        let info = db
            .job_log_info(job_id)
            .context("failed to query job")?
            .with_context(|| format!("job '{job_id}' not found in state database"))?;

        let (db_log_path, status) = (info.log_path, info.status);
        let log_path = resolve_log_path(db_log_path.as_deref(), job_id);

        if !log_path.exists() {
            if args.json {
                let json_val = serde_json::json!({
                    "job_id": job_id,
                    "status": status,
                    "log_path": log_path.to_string_lossy(),
                    "error": "log file not found",
                });
                println!("{}", serde_json::to_string(&json_val)?);
                return Ok(());
            }
            bail!(
                "Job '{job_id}' (status: {status}) has no log file at {}",
                log_path.display()
            );
        }

        if args.follow {
            if status != "running" {
                eprintln!(
                    "warning: job '{job_id}' is not running (status: {status}), showing full log"
                );
            }
            return follow_log(&log_path);
        }

        return print_log(&log_path, job_id, args.json);
    }

    // List mode: show logs for multiple jobs.
    let jobs = db.jobs_with_logs(args.rule.as_deref(), args.failed)?;

    if jobs.is_empty() {
        if args.json {
            println!("[]");
        } else {
            println!("No matching jobs found.");
        }
        return Ok(());
    }

    if args.json {
        let entries: Vec<serde_json::Value> = jobs
            .iter()
            .map(|j| {
                let log_path = resolve_log_path(j.log_path.as_deref(), &j.id);
                let exists = log_path.exists();
                serde_json::json!({
                    "job_id": j.id,
                    "rule": j.rule_name,
                    "status": j.status,
                    "log_path": log_path.to_string_lossy(),
                    "log_exists": exists,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for j in &jobs {
            let log_path = resolve_log_path(j.log_path.as_deref(), &j.id);
            let marker = if log_path.exists() { " " } else { "!" };
            let status_styled = match j.status.as_str() {
                "completed" => theme.success.apply_to(&j.status),
                "failed" => theme.error.apply_to(&j.status),
                "running" => theme.running.apply_to(&j.status),
                _ => theme.muted.apply_to(&j.status),
            };
            println!(
                "{marker} {}  [{}]  rule={}  log={}",
                theme.highlight.apply_to(&j.id),
                status_styled,
                theme.info.apply_to(&j.rule_name),
                theme.muted.apply_to(log_path.display())
            );
        }
    }

    Ok(())
}
