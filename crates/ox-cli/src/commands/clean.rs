//! Implementation of `ox clean` — remove stale state, orphan cache entries,
//! and temporary files from `.oxymake/`.

use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};

const STALE_THRESHOLD_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct CleanArgs {
    /// Show what would be removed without deleting anything
    #[arg(long)]
    pub dry_run: bool,

    /// Only clean cache entries
    #[arg(long, conflicts_with = "state_only")]
    pub cache_only: bool,

    /// Only clean state database
    #[arg(long, conflicts_with = "cache_only")]
    pub state_only: bool,

    /// Remove retained NDJSON event logs and temporaries older than 300 seconds
    #[arg(long, conflicts_with_all = ["cache_only", "state_only", "state", "all"])]
    pub events: bool,

    /// Delete the state database file entirely (recovery escape hatch
    /// for a corrupt state.db — it is a regenerable cache)
    #[arg(long, conflicts_with_all = ["cache_only", "state_only", "all"])]
    pub state: bool,

    /// Also remove the audit trail (runs and job history)
    #[arg(long)]
    pub all: bool,

    /// Skip confirmation prompt
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Clean execution state or event logs even if live sessions exist
    #[arg(long)]
    pub force: bool,

    /// Output JSON
    #[arg(long)]
    pub json: bool,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

pub fn cmd_clean(args: CleanArgs) -> Result<()> {
    let oxymake_dir = Path::new(".oxymake");
    if !oxymake_dir.exists() {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "actions": [],
                    "dry_run": args.dry_run,
                    "removed": {},
                }))?
            );
        } else {
            println!("Nothing to clean — .oxymake/ does not exist.");
        }
        return Ok(());
    }

    // --state is the corruption escape hatch: delete the state database
    // file without ever opening it (a corrupt DB cannot be opened, so
    // every other clean path would fail before reaching the removal).
    if args.state {
        return clean_state_file(oxymake_dir, &args);
    }

    let clean_cache = !args.state_only && !args.events;
    let clean_state = !args.cache_only && !args.events;
    let clean_logs = !args.cache_only && !args.state_only && !args.events;
    let clean_events = args.events;

    // -----------------------------------------------------------------------
    // Gather what would be cleaned
    // -----------------------------------------------------------------------

    let mut actions: Vec<String> = Vec::new();

    // Cache
    let orphan_count = if clean_cache {
        let cache_dir = oxymake_dir.join("cache");
        if cache_dir.exists() {
            let store =
                ox_cache::CacheStore::open(oxymake_dir).context("failed to open cache store")?;
            let total = store.len();
            let orphans = store.count_orphans();
            if orphans > 0 {
                actions.push(format!(
                    "Remove {orphans} orphan cache entries ({} valid entries kept)",
                    total - orphans,
                ));
            }
            if args.all && total > orphans {
                actions.push(format!(
                    "Remove {} remaining valid cache entries (--all)",
                    total - orphans,
                ));
            }
            if args.all { total } else { orphans }
        } else {
            0
        }
    } else {
        0
    };

    // State
    let state_path = oxymake_dir.join("state.db");
    let (stale_sessions, stale_jobs) = if (clean_state || clean_events) && state_path.exists() {
        let db = ox_state::db::StateDb::open(&state_path).context("failed to open state.db")?;
        let counts = db.job_counts()?;
        let sessions = db.active_sessions()?;
        let stale = db.find_stale_sessions(STALE_THRESHOLD_SECS)?; // 5 min threshold

        let total_jobs =
            counts.pending + counts.running + counts.completed + counts.failed + counts.skipped;

        // H17: a session with a fresh heartbeat is a run in flight.
        // Clearing state or event logs under it would disrupt the run or
        // its subscribers. Refuse unless --force.
        let live = sessions.iter().filter(|s| !stale.contains(&s.id)).count();
        if live > 0 && !args.force && !args.dry_run {
            anyhow::bail!(
                "refusing to clear execution state: {live} live session(s) \
                 still heartbeating (a run appears to be in flight).\n\
                 Wait for it to finish, or re-run with --force."
            );
        }

        if clean_state && !stale.is_empty() {
            actions.push(format!("Reclaim {} stale sessions", stale.len()));
        }
        if clean_state && total_jobs > 0 {
            actions.push(format!(
                "Clear execution state ({total_jobs} jobs, {} sessions)",
                sessions.len(),
            ));
        }
        if clean_state && args.all {
            let runs = db.list_runs()?;
            if !runs.is_empty() {
                actions.push(format!("Remove audit trail ({} runs) (--all)", runs.len(),));
            }
        }
        (stale.len(), total_jobs)
    } else {
        (0, 0)
    };

    // Logs
    let logs_dir = oxymake_dir.join("logs");
    let log_size = if clean_logs && logs_dir.exists() {
        let size = dir_size(&logs_dir);
        if size > 0 {
            actions.push(format!("Remove logs directory ({})", human_size(size)));
        }
        size
    } else {
        0
    };

    // Event streams are retained by default. Their removal is an explicit
    // operator action, intentionally separate from ordinary log cleanup.
    let events_dir = oxymake_dir.join("events");
    let mut event_logs = Vec::new();
    if clean_events && events_dir.exists() {
        for entry in std::fs::read_dir(&events_dir).context("failed to read events directory")? {
            let entry = entry.context("failed to read event log entry")?;
            let path = entry.path();
            if entry.file_type()?.is_file() && is_removable_event_log(&path) {
                if ox_report_json::reporter::is_event_log_temporary(&path) {
                    actions.push(format!(
                        "Remove stale event log temporary {}",
                        path.display()
                    ));
                }
                event_logs.push(path);
            }
        }
        if !event_logs.is_empty() {
            actions.push(format!("Remove {} retained event log(s)", event_logs.len()));
        }
    }

    // -----------------------------------------------------------------------
    // Report
    // -----------------------------------------------------------------------

    if actions.is_empty() {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "actions": [],
                    "dry_run": args.dry_run,
                    "removed": {},
                }))?
            );
        } else {
            println!("Nothing to clean.");
        }
        return Ok(());
    }

    if !args.json {
        println!("Planned actions:");
        for action in &actions {
            println!("  - {action}");
        }
    }

    if args.dry_run {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "actions": actions,
                    "dry_run": true,
                    "removed": {},
                }))?
            );
        } else {
            println!("\n(dry run — no changes made)");
        }
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // Confirm (--json implies --yes)
    // -----------------------------------------------------------------------

    if !args.yes && !args.json {
        print!("\nProceed? [y/N] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // -----------------------------------------------------------------------
    // Execute
    // -----------------------------------------------------------------------

    let mut removed_cache: usize = 0;
    let mut removed_state = false;
    let mut removed_logs = false;
    let mut removed_events = 0usize;

    // Cache
    if clean_cache && orphan_count > 0 {
        let mut store =
            ox_cache::CacheStore::open(oxymake_dir).context("failed to open cache store")?;
        if args.all {
            let n = store.len();
            store.clear();
            store.save().context("failed to save cache manifest")?;
            if !args.json {
                println!("  Removed {n} cache entries");
            }
            removed_cache = n;
        } else {
            let n = store.remove_orphans();
            store.save().context("failed to save cache manifest")?;
            if !args.json {
                println!("  Removed {n} orphan cache entries");
            }
            removed_cache = n;
        }
    }

    // State
    if clean_state && state_path.exists() {
        if args.all {
            std::fs::remove_file(&state_path).context("failed to remove state.db")?;
            if !args.json {
                println!("  Removed state.db (all state and audit trail)");
            }
            removed_state = true;
        } else if stale_sessions > 0 || stale_jobs > 0 {
            let db = ox_state::db::StateDb::open(&state_path).context("failed to open state.db")?;

            // Reclaim stale sessions
            let stale = db.find_stale_sessions(STALE_THRESHOLD_SECS)?;
            let cutoff = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_sub(STALE_THRESHOLD_SECS);
            for session_id in &stale {
                // Re-checked inside the transaction: a session that
                // heartbeated since `find_stale_sessions` keeps its jobs.
                let reclaimed = db.reclaim_stale_jobs_if_stale(session_id, cutoff)?;
                if reclaimed > 0 && !args.json {
                    println!("  Reclaimed {reclaimed} jobs from stale session {session_id}");
                }
            }

            // Clear execution state (jobs + sessions) preserving audit trail
            db.clear_execution_state()
                .context("failed to clear execution state")?;
            if !args.json {
                println!("  Cleared execution state (jobs and sessions)");
            }
            removed_state = true;
        }
    }

    // Logs
    if clean_logs && log_size > 0 {
        std::fs::remove_dir_all(&logs_dir).context("failed to remove logs directory")?;
        if !args.json {
            println!("  Removed logs directory");
        }
        removed_logs = true;
    }

    if clean_events && !event_logs.is_empty() {
        for path in &event_logs {
            // A temporary may have been touched since the inventory.
            if is_removable_event_log(path) {
                std::fs::remove_file(path).context("failed to remove event log")?;
                removed_events += 1;
            }
        }
        if !args.json {
            println!("  Removed {removed_events} event log(s)");
        }
    }

    if args.json {
        let json = serde_json::json!({
            "actions": actions,
            "dry_run": false,
            "removed": {
                "cache_entries": removed_cache,
                "state": removed_state,
                "logs": removed_logs,
                "log_bytes": if removed_logs { log_size } else { 0 },
                "event_logs": removed_events,
            },
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if removed_cache > 0 || removed_state || removed_logs || removed_events > 0 {
        println!("Done.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// --state: delete the state database without opening it
// ---------------------------------------------------------------------------

/// Remove `state.db` and its WAL/SHM sidecars by file deletion only.
///
/// The sidecars must go with the main file: a stale `-wal` applied to a
/// freshly regenerated database would reintroduce the corruption.
fn clean_state_file(oxymake_dir: &Path, args: &CleanArgs) -> Result<()> {
    let targets: Vec<std::path::PathBuf> = ["state.db", "state.db-wal", "state.db-shm"]
        .iter()
        .map(|name| oxymake_dir.join(name))
        .filter(|p| p.exists())
        .collect();

    if targets.is_empty() {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "actions": [],
                    "dry_run": args.dry_run,
                    "removed": {},
                }))?
            );
        } else {
            println!("Nothing to clean — no state database found.");
        }
        return Ok(());
    }

    let actions: Vec<String> = targets
        .iter()
        .map(|p| format!("Remove {}", p.display()))
        .collect();

    if !args.json {
        println!("Planned actions:");
        for action in &actions {
            println!("  - {action}");
        }
        println!("\nstate.db is a regenerable cache — the next `ox run` recreates it.");
    }

    if args.dry_run {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "actions": actions,
                    "dry_run": true,
                    "removed": {},
                }))?
            );
        } else {
            println!("\n(dry run — no changes made)");
        }
        return Ok(());
    }

    if !args.yes && !args.json {
        print!("\nProceed? [y/N] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    for path in &targets {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        if !args.json {
            println!("  Removed {}", path.display());
        }
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "actions": actions,
                "dry_run": false,
                "removed": { "state": true },
            }))?
        );
    } else {
        println!("Done.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Preserve recent temporaries and files whose age cannot be established.
fn is_removable_event_log(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "ndjson")
        || (ox_report_json::reporter::is_event_log_temporary(path)
            && path
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|mtime| mtime.elapsed().ok())
                .is_some_and(|age| age > std::time::Duration::from_secs(STALE_THRESHOLD_SECS)))
}

/// Calculate total size of a directory tree in bytes.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if ft.is_dir() {
                total += dir_size(&entry.path());
            }
        }
    }
    total
}

/// Format a byte count in human-readable form.
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
