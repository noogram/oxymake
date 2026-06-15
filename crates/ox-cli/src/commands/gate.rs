//! `ox gate` — Manual approval gates in workflows.
//!
//! Gates allow human-in-the-loop approval points in automated pipelines.
//! When a rule has a gate, the scheduler pauses and waits for explicit
//! approval via `ox gate approve`.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Arguments for `ox gate`.
#[derive(clap::Args)]
pub struct GateArgs {
    /// Subcommand: list (default), approve, reject
    pub action: Option<String>,

    /// Gate ID to approve/reject
    pub gate_id: Option<String>,

    /// Approver identity
    #[arg(long)]
    pub approver: Option<String>,

    /// Approval reason or rejection message
    #[arg(long)]
    pub reason: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Oxymakefile path
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,
}

pub fn cmd_gate(args: GateArgs) -> Result<()> {
    let db_path = PathBuf::from(".oxymake/state.db");
    if !db_path.exists() {
        anyhow::bail!("No OxyMake state found. Run 'ox run' first.");
    }

    let db = ox_state::db::StateDb::open(&db_path).context("Failed to open state database")?;

    let action = args.action.as_deref().unwrap_or("list");

    match action {
        "list" => {
            let gates = db.list_gates()?;
            if gates.is_empty() {
                println!("No pending gates.");
                return Ok(());
            }
            if args.json {
                for g in &gates {
                    println!(
                        "{{\"id\":{},\"rule\":\"{}\",\"job_id\":\"{}\",\"status\":\"{}\",\"created_at\":{}}}",
                        g.id, g.rule_name, g.job_id, g.status, g.created_at
                    );
                }
            } else {
                println!("{:<6} {:<20} {:<30} {:<10}", "ID", "Rule", "Job", "Status");
                println!("{}", "-".repeat(70));
                for g in &gates {
                    println!(
                        "{:<6} {:<20} {:<30} {:<10}",
                        g.id, g.rule_name, g.job_id, g.status
                    );
                }
            }
        }
        "approve" => {
            let gate_id: i64 = args
                .gate_id
                .as_deref()
                .context("Gate ID required for approve")?
                .parse()
                .context("Gate ID must be a number")?;
            let approver = args.approver.as_deref().unwrap_or("unknown");
            let reason = args.reason.as_deref().unwrap_or("");
            db.approve_gate(gate_id, approver, reason)?;
            println!("Gate {} approved by {}", gate_id, approver);
        }
        "reject" => {
            let gate_id: i64 = args
                .gate_id
                .as_deref()
                .context("Gate ID required for reject")?
                .parse()
                .context("Gate ID must be a number")?;
            let approver = args.approver.as_deref().unwrap_or("unknown");
            let reason = args.reason.as_deref().unwrap_or("");
            db.reject_gate(gate_id, approver, reason)?;
            println!("Gate {} rejected by {}", gate_id, approver);
        }
        other => {
            anyhow::bail!(
                "Unknown gate action: '{}'. Use list, approve, or reject.",
                other
            );
        }
    }

    Ok(())
}
