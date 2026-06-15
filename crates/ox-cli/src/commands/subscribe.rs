//! Implementation of `ox subscribe` — stream events from an active session.
//!
//! This is the CLI equivalent of the MCP `ox_subscribe` tool. It tails the
//! NDJSON event log written by `ox run` and streams matching events to stdout.

use std::collections::HashSet;
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
pub struct SubscribeArgs {
    /// Session (run) ID to subscribe to. If omitted, subscribes to the most
    /// recent active session.
    #[arg(long)]
    pub session_id: Option<String>,

    /// Filter to specific event types (repeatable). If omitted, all events
    /// are streamed. Valid types: run_started, job_queued, job_started,
    /// job_completed, job_failed, job_skipped, gate_reached, gate_approved,
    /// run_completed, run_failed.
    #[arg(long = "event-type", value_name = "TYPE")]
    pub event_types: Vec<String>,

    /// Filter events by tag values on the originating job (repeatable,
    /// KEY=VALUE format, AND logic).
    #[arg(long = "where", value_name = "KEY=VALUE")]
    pub filters: Vec<String>,

    /// Replay all events from session start before streaming live events.
    /// Without this flag, only new events are shown.
    #[arg(long)]
    pub replay: bool,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const EVENTS_DIR: &str = ".oxymake/events";
const POLL_INTERVAL: Duration = Duration::from_millis(200);

const VALID_EVENT_TYPES: &[&str] = &[
    "run_started",
    "job_queued",
    "job_started",
    "job_completed",
    "job_failed",
    "job_skipped",
    "gate_reached",
    "gate_approved",
    "run_completed",
    "run_failed",
];

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

pub fn cmd_subscribe(args: SubscribeArgs) -> Result<()> {
    // Validate event types early.
    for et in &args.event_types {
        if !VALID_EVENT_TYPES.contains(&et.as_str()) {
            bail!(
                "unknown event type '{}'; valid types: {}",
                et,
                VALID_EVENT_TYPES.join(", ")
            );
        }
    }

    // Parse tag filters.
    let tag_filters: Vec<(String, String)> = args
        .filters
        .iter()
        .map(|f| {
            f.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .ok_or_else(|| {
                    anyhow::anyhow!("invalid --where filter '{}': expected KEY=VALUE", f)
                })
        })
        .collect::<Result<Vec<_>>>()?;

    let event_types: HashSet<&str> = args.event_types.iter().map(|s| s.as_str()).collect();

    let log_path = resolve_event_log(&args.session_id)?;

    let file = fs::File::open(&log_path)
        .with_context(|| format!("cannot open event log: {}", log_path.display()))?;
    let mut reader = BufReader::new(file);

    if !args.replay {
        // Skip to end — only show new events.
        reader.seek(SeekFrom::End(0))?;
    }

    // Tail the event log, printing matching lines.
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            // Check for truncation (log rotation).
            if let Ok(metadata) = reader.get_ref().metadata() {
                if let Ok(pos) = reader.stream_position() {
                    if metadata.len() < pos {
                        reader.seek(SeekFrom::Start(0))?;
                        continue;
                    }
                }
            }
            // Check if the run has ended by looking for a terminal event.
            // We peek at the last event we printed; if it was run_completed
            // or run_failed, exit.
            thread::sleep(POLL_INTERVAL);
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if should_emit(trimmed, &event_types, &tag_filters) {
            print!("{line}");

            // Exit after terminal events.
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(et) = v.get("event").and_then(|e| e.as_str()) {
                    if et == "run_completed" || et == "run_failed" {
                        return Ok(());
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Decide whether a JSON event line should be emitted given the active filters.
fn should_emit(
    json_line: &str,
    event_types: &HashSet<&str>,
    tag_filters: &[(String, String)],
) -> bool {
    let v: serde_json::Value = match serde_json::from_str(json_line) {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Event type filter.
    if !event_types.is_empty() {
        if let Some(et) = v.get("event").and_then(|e| e.as_str()) {
            if !event_types.contains(et) {
                return false;
            }
        } else {
            return false;
        }
    }

    // Tag filter — only applies to events that carry tags (job_queued).
    if !tag_filters.is_empty() {
        if let Some(tags) = v.get("tags") {
            for (key, value) in tag_filters {
                match tags.get(key).and_then(|v| v.as_str()) {
                    Some(tv) if tv == value => {}
                    _ => return false,
                }
            }
        } else {
            // Event has no tags — skip if tag filters are active.
            return false;
        }
    }

    true
}

/// Find the event log file for the given session, or the most recent one.
fn resolve_event_log(session_id: &Option<String>) -> Result<PathBuf> {
    let events_dir = Path::new(EVENTS_DIR);
    if !events_dir.exists() {
        bail!(
            "No event logs found. Run 'ox run' first (events are written to {}).",
            EVENTS_DIR
        );
    }

    if let Some(sid) = session_id {
        let path = events_dir.join(format!("{sid}.ndjson"));
        if path.exists() {
            return Ok(path);
        }
        bail!(
            "No event log for session '{}'. Available logs:\n{}",
            sid,
            list_event_logs(events_dir)?
        );
    }

    // Find the most recent event log by modification time.
    let mut newest: Option<(PathBuf, std::time::SystemTime)> = None;
    for entry in fs::read_dir(events_dir).context("cannot read events directory")? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("ndjson") {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if newest.as_ref().is_none_or(|(_, t)| modified > *t) {
                        newest = Some((path, modified));
                    }
                }
            }
        }
    }

    match newest {
        Some((path, _)) => Ok(path),
        None => bail!("No event logs found in {}. Run 'ox run' first.", EVENTS_DIR),
    }
}

/// List available event log files for error messages.
fn list_event_logs(dir: &Path) -> Result<String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().and_then(|e| e.to_str()) == Some("ndjson") {
            if let Some(stem) = entry.path().file_stem() {
                names.push(format!("  {}", stem.to_string_lossy()));
            }
        }
    }
    names.sort();
    if names.is_empty() {
        Ok("  (none)".to_string())
    } else {
        Ok(names.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_emit_no_filters() {
        let line = r#"{"event":"run_started","total_jobs":10,"to_run":3,"cached":7}"#;
        assert!(should_emit(line, &HashSet::new(), &[]));
    }

    #[test]
    fn should_emit_event_type_match() {
        let line = r#"{"event":"job_started","job_id":"j1","executor":"local"}"#;
        let types: HashSet<&str> = ["job_started"].into_iter().collect();
        assert!(should_emit(line, &types, &[]));
    }

    #[test]
    fn should_emit_event_type_mismatch() {
        let line = r#"{"event":"job_started","job_id":"j1","executor":"local"}"#;
        let types: HashSet<&str> = ["job_completed"].into_iter().collect();
        assert!(!should_emit(line, &types, &[]));
    }

    #[test]
    fn should_emit_tag_filter_match() {
        let line = r#"{"event":"job_queued","job_id":"align_S001","rule":"align","tags":{"sample":"S001"}}"#;
        let filters = vec![("sample".to_string(), "S001".to_string())];
        assert!(should_emit(line, &HashSet::new(), &filters));
    }

    #[test]
    fn should_emit_tag_filter_mismatch() {
        let line = r#"{"event":"job_queued","job_id":"align_S001","rule":"align","tags":{"sample":"S001"}}"#;
        let filters = vec![("sample".to_string(), "S002".to_string())];
        assert!(!should_emit(line, &HashSet::new(), &filters));
    }

    #[test]
    fn should_emit_tag_filter_no_tags() {
        let line = r#"{"event":"run_started","total_jobs":10,"to_run":3,"cached":7}"#;
        let filters = vec![("sample".to_string(), "S001".to_string())];
        assert!(!should_emit(line, &HashSet::new(), &filters));
    }

    #[test]
    fn should_emit_invalid_json() {
        assert!(!should_emit("not json", &HashSet::new(), &[]));
    }

    #[test]
    fn should_emit_combined_filters() {
        let line = r#"{"event":"job_queued","job_id":"align_S001","rule":"align","tags":{"sample":"S001"}}"#;
        let types: HashSet<&str> = ["job_queued"].into_iter().collect();
        let filters = vec![("sample".to_string(), "S001".to_string())];
        assert!(should_emit(line, &types, &filters));
    }

    #[test]
    fn should_emit_combined_type_mismatch() {
        let line = r#"{"event":"job_queued","job_id":"align_S001","rule":"align","tags":{"sample":"S001"}}"#;
        let types: HashSet<&str> = ["job_started"].into_iter().collect();
        let filters = vec![("sample".to_string(), "S001".to_string())];
        assert!(!should_emit(line, &types, &filters));
    }
}
