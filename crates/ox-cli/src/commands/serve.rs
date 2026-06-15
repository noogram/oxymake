//! `ox serve` — Start server modes (MCP, HTTP, etc.)

use std::path::PathBuf;

use anyhow::{Result, bail};

use ox_mcp::{LogLevel, ServerConfig};

#[derive(clap::Args)]
pub struct ServeArgs {
    /// Start the MCP (Model Context Protocol) server over stdio
    #[arg(long)]
    pub mcp: bool,

    /// Working directory for Oxymakefile resolution
    #[arg(long)]
    pub workdir: Option<String>,

    /// Log level for stderr diagnostics
    #[arg(long, default_value = "info")]
    pub log_level: String,
}

pub fn cmd_serve(args: ServeArgs) -> Result<()> {
    if !args.mcp {
        bail!("ox serve requires --mcp flag. Usage: ox serve --mcp");
    }

    let workdir = match args.workdir {
        Some(ref p) => PathBuf::from(p),
        None => std::env::current_dir()?,
    };

    let log_level = match args.log_level.as_str() {
        "quiet" | "none" => LogLevel::Quiet,
        "debug" | "trace" => LogLevel::Debug,
        _ => LogLevel::Info,
    };

    let config = ServerConfig { workdir, log_level };

    ox_mcp::run_stdio(config)
}
