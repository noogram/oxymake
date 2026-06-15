//! `ox dashboard` — Launch the web dashboard for monitoring and DAG visualization.

use std::sync::Arc;

use anyhow::Result;

use ox_dashboard::server::{DashboardState, create_router};

#[derive(clap::Args)]
pub struct DashboardArgs {
    /// Path to state.db
    #[arg(long, default_value = ".oxymake/state.db")]
    pub db: String,

    /// Port to listen on
    #[arg(long, default_value = "9876")]
    pub port: u16,

    /// Bind address
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: String,
}

pub fn cmd_dashboard(args: DashboardArgs) -> Result<()> {
    let addr = format!("{}:{}", args.bind, args.port);

    let state = Arc::new(DashboardState {
        db_path: args.db.into(),
    });
    let app = create_router(state);

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let url = format!("http://{addr}");
        eprintln!(
            "OxyMake dashboard: \x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
            url, url
        );
        ox_dashboard::serve(&addr, app)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    })
}
