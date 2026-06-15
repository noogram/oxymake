//! `ox-top` binary entry point.
//!
//! Usage:
//!   ox-top [--db <path>] [--mock] [--stdin]
//!
//! Without arguments, looks for `.oxymake/state.db` in the current directory.
//! With `--mock`, displays realistic mock data for development.
//! With `--stdin`, reads NDJSON events from stdin (piped from `ox run --json`).

use std::path::PathBuf;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--mock") {
        return ox_monitor_tui::run_tui_mock().await;
    }

    if args.iter().any(|a| a == "--stdin") {
        return ox_monitor_tui::run_tui_stdin().await;
    }

    let db_path = args
        .windows(2)
        .find(|w| w[0] == "--db")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| PathBuf::from(".oxymake/state.db"));

    ox_monitor_tui::run_tui(&db_path).await
}
