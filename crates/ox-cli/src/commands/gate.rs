//! `ox gate` — Manual approval gates in workflows.
//!
//! A `[gate.<name>]` table in the Oxymakefile blocks every rule listed in
//! its `before` until the gate is approved. When `ox run` reaches such a
//! rule it registers the gate as pending in `.oxymake/state.db`, prints the
//! gate message and waits; `ox gate approve <name>` lets the run continue,
//! `ox gate reject <name>` cancels the guarded jobs. Gates are addressed by
//! their name (the `[gate.<name>]` key); the numeric id shown by `ox gate
//! list` is accepted too and disambiguates when several runs wait on the
//! same gate.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Arguments for `ox gate`.
#[derive(clap::Args)]
#[command(
    after_help = "EXAMPLES:\n    ox gate list\n    ox gate approve qc_check --approver alice --reason \"metrics look good\"\n    ox gate reject qc_check --reason \"coverage too low\"\n    ox gate approve 3                      # by id, when two runs wait on the same gate"
)]
pub struct GateArgs {
    /// Subcommand: list (default), approve, reject
    pub action: Option<String>,

    /// Gate to approve/reject: its name (the `[gate.<name>]` key) or the
    /// numeric id shown by `ox gate list`
    pub gate: Option<String>,

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
            if args.json {
                let rows: Vec<serde_json::Value> = gates
                    .iter()
                    .map(|g| {
                        serde_json::json!({
                            "id": g.id,
                            "name": g.name,
                            "run_id": g.run_id,
                            "status": g.status,
                            "created_at": g.created_at,
                            "decided_at": g.decided_at,
                            "decided_by": g.decided_by,
                            "reason": g.reason,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string(&rows)?);
                return Ok(());
            }
            if gates.is_empty() {
                println!("No gates.");
                return Ok(());
            }
            println!(
                "{:<6} {:<24} {:<10} {:<20} Decided by",
                "ID", "Gate", "Status", "Run"
            );
            println!("{}", "-".repeat(76));
            for g in &gates {
                println!(
                    "{:<6} {:<24} {:<10} {:<20} {}",
                    g.id,
                    g.name,
                    g.status,
                    g.run_id.as_deref().unwrap_or("-"),
                    g.decided_by.as_deref().unwrap_or("-"),
                );
            }
            if gates.iter().any(|g| g.status == "pending") {
                println!();
                println!(
                    "Approve with `ox gate approve <name>`; reject with `ox gate reject <name>`."
                );
            }
        }
        "approve" | "reject" => {
            let gate = args
                .gate
                .as_deref()
                .with_context(|| format!("Gate name (or id) required for {action}"))?;
            let gate_id = db.resolve_pending_gate(gate)?;
            let approver = args.approver.as_deref().unwrap_or("unknown");
            let reason = args.reason.as_deref().unwrap_or("");
            let verb = if action == "approve" {
                db.approve_gate(gate_id, approver, reason)?;
                "approved"
            } else {
                db.reject_gate(gate_id, approver, reason)?;
                "rejected"
            };
            let record = db.list_gates()?.into_iter().find(|g| g.id == gate_id);
            let name = record.map(|g| g.name).unwrap_or_else(|| gate.to_string());
            if args.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "id": gate_id,
                        "name": name,
                        "status": verb,
                        "decided_by": approver,
                        "reason": reason,
                    })
                );
            } else {
                println!("Gate '{name}' (id {gate_id}) {verb} by {approver}");
            }
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
