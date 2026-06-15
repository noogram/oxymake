//! Implementation of `ox snapshot` — create, list, and diff named snapshots
//! of workflow state.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ox_core::model::ContentHash;

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct SnapshotArgs {
    #[command(subcommand)]
    pub action: SnapshotAction,
}

#[derive(clap::Subcommand)]
pub enum SnapshotAction {
    /// Create a named snapshot of current workflow state
    Create {
        /// Snapshot name
        name: String,

        /// Optional description
        #[arg(short = 'm', long)]
        message: Option<String>,

        /// Oxymakefile path
        #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
        file: String,

        /// Output NDJSON (one JSON object per line)
        #[arg(long)]
        json: bool,
    },

    /// List all snapshots
    List {
        /// Output NDJSON (one JSON object per line)
        #[arg(long)]
        json: bool,
    },

    /// Diff two snapshots, or a snapshot against current state
    Diff {
        /// First snapshot name
        left: String,

        /// Second snapshot name (omit to diff against current state)
        right: Option<String>,

        /// Output NDJSON (one JSON object per line)
        #[arg(long)]
        json: bool,
    },

    /// Delete a snapshot
    Delete {
        /// Snapshot name to delete
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

pub fn cmd_snapshot(args: SnapshotArgs) -> Result<()> {
    match args.action {
        SnapshotAction::Create {
            name,
            message,
            file,
            json,
        } => create_snapshot(&name, message.as_deref(), &file, json),
        SnapshotAction::List { json } => list_snapshots(json),
        SnapshotAction::Diff { left, right, json } => diff_snapshots(&left, right.as_deref(), json),
        SnapshotAction::Delete { name } => delete_snapshot(&name),
    }
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

fn create_snapshot(
    name: &str,
    description: Option<&str>,
    oxymakefile: &str,
    json: bool,
) -> Result<()> {
    let db_path = Path::new(".oxymake/state.db");
    if !db_path.exists() {
        bail!("No OxyMake state found. Run 'ox run' first.");
    }

    let workflow_hash = compute_workflow_hash(oxymakefile);

    let db = ox_state::db::StateDb::open(db_path).context("failed to open state.db")?;
    db.create_snapshot(name, workflow_hash.as_ref(), description)
        .context("failed to create snapshot")?;

    let snap = db.get_snapshot(name)?.expect("just created");

    if json {
        let obj = serde_json::json!({
            "name": snap.name,
            "created_at": format_iso8601(snap.created_at),
            "workflow_hash": snap.workflow_hash,
            "note": snap.description,
            "job_count": snap.job_count,
        });
        println!("{}", serde_json::to_string(&obj)?);
    } else {
        println!(
            "Snapshot '{}' created ({} jobs captured)",
            name, snap.job_count
        );
        if let Some(ref hash) = workflow_hash {
            println!("  Workflow hash: {}", &hash.as_str()[..16]);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

fn list_snapshots(json: bool) -> Result<()> {
    let db_path = Path::new(".oxymake/state.db");
    if !db_path.exists() {
        if json {
            // empty stream
        } else {
            println!("No OxyMake state found. Run 'ox run' first.");
        }
        return Ok(());
    }

    let db = ox_state::db::StateDb::open(db_path).context("failed to open state.db")?;
    let snapshots = db.list_snapshots()?;

    if snapshots.is_empty() {
        if !json {
            println!("No snapshots found.");
        }
        return Ok(());
    }

    if json {
        for snap in &snapshots {
            let obj = serde_json::json!({
                "name": snap.name,
                "created_at": format_iso8601(snap.created_at),
                "workflow_hash": snap.workflow_hash,
                "note": snap.description,
                "job_count": snap.job_count,
            });
            println!("{}", serde_json::to_string(&obj)?);
        }
    } else {
        let header = format!(
            "{:<24} {:>20} {:>6}  DESCRIPTION",
            "NAME", "CREATED", "JOBS"
        );
        println!("{header}");
        println!("{}", "-".repeat(72));
        for snap in &snapshots {
            let created = format_timestamp(snap.created_at);
            let desc = snap.description.as_deref().unwrap_or("");
            println!(
                "{:<24} {:>20} {:>6}  {}",
                snap.name, created, snap.job_count, desc
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

fn diff_snapshots(left_name: &str, right_name: Option<&str>, json: bool) -> Result<()> {
    let db_path = Path::new(".oxymake/state.db");
    if !db_path.exists() {
        bail!("No OxyMake state found. Run 'ox run' first.");
    }

    let db = ox_state::db::StateDb::open(db_path).context("failed to open state.db")?;

    let left_snap = db
        .get_snapshot(left_name)?
        .ok_or_else(|| anyhow::anyhow!("snapshot '{}' not found", left_name))?;

    // If right_name is given, load that snapshot; otherwise use current state.
    let (right_label, right_wf_hash, right_job_count, right_jobs) = if let Some(rn) = right_name {
        let snap = db
            .get_snapshot(rn)?
            .ok_or_else(|| anyhow::anyhow!("snapshot '{}' not found", rn))?;
        let jobs = db.snapshot_jobs(rn)?;
        (rn.to_string(), snap.workflow_hash, snap.job_count, jobs)
    } else {
        let jobs = db.current_jobs().context("failed to read current jobs")?;
        let count = jobs.len();
        let wf_hash = compute_workflow_hash("Oxymakefile.toml");
        ("(current)".to_string(), wf_hash, count, jobs)
    };

    let left_jobs = db.snapshot_jobs(left_name)?;

    // Build lookup maps: job_id -> (status, output_hashes)
    let left_map: BTreeMap<&str, (&str, Option<&str>)> = left_jobs
        .iter()
        .map(|j| {
            (
                j.job_id.as_str(),
                (j.status.as_str(), j.output_hashes.as_deref()),
            )
        })
        .collect();
    let right_map: BTreeMap<&str, (&str, Option<&str>)> = right_jobs
        .iter()
        .map(|j| {
            (
                j.job_id.as_str(),
                (j.status.as_str(), j.output_hashes.as_deref()),
            )
        })
        .collect();

    // Collect all job IDs
    let mut all_jobs: Vec<&str> = left_map.keys().chain(right_map.keys()).copied().collect();
    all_jobs.sort();
    all_jobs.dedup();

    // Compute diffs
    struct DiffEntry<'a> {
        job_id: &'a str,
        kind: &'static str, // "added", "removed", "changed", "unchanged"
        left_status: Option<&'a str>,
        right_status: Option<&'a str>,
        outputs_changed: bool,
    }

    let mut diffs: Vec<DiffEntry> = Vec::new();
    for &job_id in &all_jobs {
        match (left_map.get(job_id), right_map.get(job_id)) {
            (None, Some((rs, _))) => diffs.push(DiffEntry {
                job_id,
                kind: "added",
                left_status: None,
                right_status: Some(rs),
                outputs_changed: false,
            }),
            (Some((ls, _)), None) => diffs.push(DiffEntry {
                job_id,
                kind: "removed",
                left_status: Some(ls),
                right_status: None,
                outputs_changed: false,
            }),
            (Some((ls, lo)), Some((rs, ro))) => {
                let outputs_changed = lo != ro;
                let status_changed = ls != rs;
                if status_changed || outputs_changed {
                    diffs.push(DiffEntry {
                        job_id,
                        kind: "changed",
                        left_status: Some(ls),
                        right_status: Some(rs),
                        outputs_changed,
                    });
                } else {
                    diffs.push(DiffEntry {
                        job_id,
                        kind: "unchanged",
                        left_status: Some(ls),
                        right_status: Some(rs),
                        outputs_changed: false,
                    });
                }
            }
            (None, None) => unreachable!(),
        }
    }

    // Check workflow hash difference
    let wf_changed = left_snap.workflow_hash != right_wf_hash;

    // Compute summary counts
    let added = diffs.iter().filter(|d| d.kind == "added").count();
    let removed = diffs.iter().filter(|d| d.kind == "removed").count();
    let changed = diffs.iter().filter(|d| d.kind == "changed").count();
    let unchanged = diffs.iter().filter(|d| d.kind == "unchanged").count();

    if json {
        if wf_changed {
            let obj = serde_json::json!({
                "type": "workflow_hash",
                "left": left_snap.workflow_hash,
                "right": right_wf_hash,
            });
            println!("{}", serde_json::to_string(&obj)?);
        }
        for d in &diffs {
            if d.kind == "unchanged" {
                continue;
            }
            let obj = serde_json::json!({
                "type": "job",
                "job_id": d.job_id,
                "change": d.kind,
                "left_status": d.left_status,
                "right_status": d.right_status,
                "outputs_changed": d.outputs_changed,
            });
            println!("{}", serde_json::to_string(&obj)?);
        }
        // Always emit summary so consumers can distinguish "identical" from "error"
        let summary = serde_json::json!({
            "type": "summary",
            "unchanged": unchanged,
            "changed": changed,
            "added": added,
            "removed": removed,
        });
        println!("{}", serde_json::to_string(&summary)?);
    } else {
        println!(
            "Diff: {} ({} jobs) vs {} ({} jobs)",
            left_name, left_snap.job_count, right_label, right_job_count
        );
        println!();

        if wf_changed {
            let lh = left_snap
                .workflow_hash
                .as_ref()
                .map(|h| {
                    let s = h.as_str();
                    &s[..16.min(s.len())]
                })
                .unwrap_or("(none)");
            let rh = right_wf_hash
                .as_ref()
                .map(|h| {
                    let s = h.as_str();
                    &s[..16.min(s.len())]
                })
                .unwrap_or("(none)");
            println!("  Workflow hash: {} -> {}", lh, rh);
            println!();
        }

        let has_changes = changed > 0 || added > 0 || removed > 0;
        if !has_changes {
            println!("  No differences. ({} jobs unchanged)", unchanged);
            return Ok(());
        }

        println!(
            "  {} changed, {} added, {} removed, {} unchanged",
            changed, added, removed, unchanged
        );
        println!();

        println!(
            "  {:<4} {:<28} {:<14} {:<14} OUTPUTS",
            "", "JOB", left_name, &right_label
        );
        println!("  {}", "-".repeat(68));

        for d in &diffs {
            if d.kind == "unchanged" {
                continue;
            }
            let sigil = match d.kind {
                "added" => "+",
                "removed" => "-",
                "changed" => "~",
                _ => " ",
            };
            let ls = d.left_status.unwrap_or("-");
            let rs = d.right_status.unwrap_or("-");
            let out = if d.outputs_changed { "changed" } else { "" };
            println!(
                "  {:<4} {:<28} {:<14} {:<14} {}",
                sigil, d.job_id, ls, rs, out
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

fn delete_snapshot(name: &str) -> Result<()> {
    let db_path = Path::new(".oxymake/state.db");
    if !db_path.exists() {
        bail!("No OxyMake state found.");
    }

    let db = ox_state::db::StateDb::open(db_path).context("failed to open state.db")?;
    if db.delete_snapshot(name)? {
        println!("Snapshot '{}' deleted.", name);
    } else {
        bail!("Snapshot '{}' not found.", name);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compute_workflow_hash(path: &str) -> Option<ContentHash> {
    let content = std::fs::read(path).ok()?;
    let hash = blake3::hash(&content);
    Some(ContentHash::from(hash))
}

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
    let days = ts / 86400;
    let rem = ts % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;

    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
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
