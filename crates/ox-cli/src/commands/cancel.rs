//! Implementation of `ox cancel` — cancel running or pending jobs.
//!
//! Supports three modes:
//! - `ox cancel <job-name>...` — cancel specific jobs and their downstream dependents
//! - `ox cancel --all` — cancel every running/pending job
//! - `ox cancel --rule <name>` — cancel all jobs for a rule (legacy)
//!
//! When job names are given, the Oxymakefile is loaded to build the dependency
//! graph so that downstream cascade can be computed.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ox_core::job_graph::JobGraph;
use ox_core::model::{JobId, OutputRef};
use ox_core::resolver::{self, ResolveRequest};

use super::common;

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct CancelArgs {
    /// Job names to cancel (cancels each job and its downstream dependents).
    ///
    /// Accepts job IDs (e.g. `call-sample01`) or output file paths.
    /// When omitted, use --all or --rule to select jobs.
    pub names: Vec<String>,

    /// Cancel ALL running and pending jobs.
    #[arg(long)]
    pub all: bool,

    /// Filter by tag (KEY=VALUE)
    #[arg(long = "where", value_name = "KEY=VALUE")]
    pub filters: Vec<String>,

    /// Cancel only jobs for this rule
    #[arg(long)]
    pub rule: Option<String>,

    /// Cancel only jobs in this session
    #[arg(long)]
    pub session: Option<u64>,

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

pub fn cmd_cancel(args: CancelArgs) -> Result<()> {
    let db_path = Path::new(".oxymake/state.db");
    if !db_path.exists() {
        anyhow::bail!("No OxyMake state found. Run 'ox run' first.");
    }

    let db = ox_state::db::StateDb::open(db_path).context("failed to open state database")?;

    // Collect PIDs of running jobs before cancelling so we can send signals.
    let running_before = db.running_jobs_detail()?;

    // Determine which job IDs to cancel.
    let cancelled_ids = if !args.names.is_empty() {
        // Granular cancel: resolve names → job IDs, compute downstream cascade,
        // then cancel exactly those IDs in the database.
        cancel_by_names(&db, &args)?
    } else if args.all || (args.rule.is_none() && args.session.is_none()) {
        // --all, or no filters at all → cancel everything.
        let session_id = resolve_session_id(&db, args.session)?;
        db.cancel_jobs(args.rule.as_deref(), session_id.as_deref())?
    } else {
        // Legacy: --rule / --session filters.
        let session_id = resolve_session_id(&db, args.session)?;
        db.cancel_jobs(args.rule.as_deref(), session_id.as_deref())?
    };

    // Send SIGTERM to processes that were running (not just pending).
    let mut signalled = 0usize;
    for detail in &running_before {
        if cancelled_ids.contains(&detail.id) {
            if let Some(pid) = find_job_pid(&db, &detail.id)? {
                send_sigterm(pid);
                signalled += 1;
            }
        }
    }

    if args.json {
        let json = serde_json::json!({
            "cancelled": cancelled_ids.len(),
            "signalled": signalled,
            "job_ids": cancelled_ids,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if cancelled_ids.is_empty() {
        println!("No running or pending jobs to cancel.");
    } else {
        println!(
            "Cancelled {} job(s) ({} running process(es) signalled).",
            cancelled_ids.len(),
            signalled,
        );
        for id in &cancelled_ids {
            println!("  {id}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Granular cancel with downstream cascade
// ---------------------------------------------------------------------------

/// Cancel specific jobs by name and cascade to their downstream dependents.
///
/// Loads the Oxymakefile to build the job graph, resolves each name to a
/// `JobId`, computes the transitive downstream closure, then cancels every
/// matching ID in the state database.
fn cancel_by_names(db: &ox_state::db::StateDb, args: &CancelArgs) -> Result<Vec<String>> {
    let file_path = PathBuf::from(&args.file);
    let workflow = common::load_workflow(&file_path)?;
    let config = common::workflow_config(&workflow);
    let targets = common::resolve_targets(&workflow, &[]);

    // Build full graph so we can traverse dependencies.
    let existing_files = common::discover_existing_files(&file_path);
    let request = ResolveRequest {
        targets,
        config,
        existing_files,
    };
    let resolve_result =
        resolver::resolve(&workflow.rules, &request).context("failed to resolve targets")?;
    let job_graph = JobGraph::build(resolve_result.jobs).context("failed to build JobGraph")?;

    // Resolve each name → JobId, collect IDs + downstream closure.
    let mut to_cancel: HashSet<JobId> = HashSet::new();
    for name in &args.names {
        let job_id = find_target_job(&job_graph, name)?;
        let closure = downstream_closure(&job_graph, &job_id);
        to_cancel.extend(closure);
    }

    // Cancel matching IDs in the database.
    let id_strings: Vec<String> = to_cancel.iter().map(|id| id.to_string()).collect();
    Ok(db.cancel_job_ids(&id_strings)?)
}

/// Find the job that produces the given target (job ID or output file path).
fn find_target_job(job_graph: &JobGraph, target: &str) -> Result<JobId> {
    let target_id = JobId::from(target);
    if job_graph.get_job(&target_id).is_some() {
        return Ok(target_id);
    }

    // Otherwise, find the job that produces an output matching the target path.
    for job_id in job_graph.job_ids() {
        let job = job_graph
            .get_job(job_id)
            .expect("BUG: job_ids() returned an ID not present in the graph");
        for output in &job.outputs {
            let key = match &output.reference {
                OutputRef::File(p) => p.to_string_lossy().to_string(),
                OutputRef::Virtual { id, .. } => id.clone(),
                OutputRef::InMemory { type_hint } => {
                    type_hint.clone().unwrap_or_else(|| "<memory>".into())
                }
            };
            if key == target {
                return Ok(job_id.clone());
            }
        }
    }

    anyhow::bail!(
        "no job found matching '{}'. Use `ox plan` to see available targets.",
        target
    )
}

/// Collect a job and all its transitive downstream dependents.
fn downstream_closure(job_graph: &JobGraph, root: &JobId) -> HashSet<JobId> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(root.clone());
    queue.push_back(root.clone());
    while let Some(current) = queue.pop_front() {
        for downstream in job_graph.downstream(&current) {
            if visited.insert(downstream.clone()) {
                queue.push_back(downstream.clone());
            }
        }
    }
    visited
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve `--session N` (a numeric shorthand) to the actual session ID string.
///
/// If `--session` is not provided, returns `None` (cancel across all sessions).
/// The numeric value is matched against the session's PID.
fn resolve_session_id(
    db: &ox_state::db::StateDb,
    session_num: Option<u64>,
) -> Result<Option<String>> {
    let pid = match session_num {
        Some(p) => p,
        None => return Ok(None),
    };

    let sessions = db.active_sessions()?;
    for s in &sessions {
        if u64::from(s.pid) == pid {
            return Ok(Some(s.id.clone()));
        }
    }

    // Also try matching against session index (1-based order by start time).
    let mut sorted = sessions;
    sorted.sort_by_key(|s| s.started_at);
    let idx = pid as usize;
    if idx >= 1 && idx <= sorted.len() {
        return Ok(Some(sorted[idx - 1].id.clone()));
    }

    anyhow::bail!("No active session matching --session {pid}");
}

/// Find the PID of the `ox run` session that OWNS a running job.
///
/// Jobs track their session_id; sessions track their PID. The session PID
/// is the `ox run` process, not the individual job subprocess: `ox cancel`
/// runs out-of-band, so it sends SIGTERM to the owning `ox run`, which
/// handles it on the same graceful path as Ctrl+C and cancels its
/// in-flight jobs via the executor (B8).
///
/// Returns `None` when the job has no *active* owning session — never the
/// first active session in the table, which could be an unrelated run
/// (or, with a recycled PID, an unrelated process).
fn find_job_pid(db: &ox_state::db::StateDb, job_id: &str) -> Result<Option<u32>> {
    Ok(db.job_session_pid(job_id)?)
}

/// Send SIGTERM to a process via the `kill` command.
fn send_sigterm(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
}
