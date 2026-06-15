//! Stub implementations for commands not yet fully implemented.
//!
//! All original stubs (cancel, clean, gate, history, invalidate, logs,
//! snapshot) have been promoted to full commands. Only `top` (TUI monitor)
//! remains here as it delegates to the `ox-monitor-tui` crate.

use anyhow::Result;

// ---------------------------------------------------------------------------
// Top
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct TopArgs {
    /// Path to state.db
    #[arg(long, default_value = ".oxymake/state.db")]
    pub db: String,

    /// Use mock data for development
    #[arg(long)]
    pub mock: bool,

    /// Read NDJSON events from stdin
    #[arg(long)]
    pub stdin: bool,
}

pub fn cmd_top(args: TopArgs) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if args.mock {
            ox_monitor_tui::run_tui_mock().await
        } else if args.stdin {
            ox_monitor_tui::run_tui_stdin().await
        } else {
            let db_path = std::path::PathBuf::from(&args.db);
            ox_monitor_tui::run_tui(&db_path).await
        }
    })
}
