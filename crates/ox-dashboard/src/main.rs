//! OxyMake Dashboard — web server entry point.
//!
//! Starts the axum server on `127.0.0.1:9876` and serves the dashboard
//! UI with live data from `state.db`.
//!
//! # Usage
//!
//! ```text
//! ox-dashboard                         # Default: .oxymake/state.db on port 9876
//! ox-dashboard --db path/to/state.db   # Custom database path
//! ox-dashboard --port 8080             # Custom port
//! ```

use std::sync::Arc;

use ox_dashboard::server::{DashboardState, create_router};

#[tokio::main]
async fn main() {
    // Simple arg parsing — a full clap integration comes later
    // when this is wired into the `ox dashboard` subcommand.
    let mut db_path = String::from(".oxymake/state.db");
    let mut port: u16 = 9876;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => {
                i += 1;
                if i < args.len() {
                    db_path = args[i].clone();
                }
            }
            "--port" => {
                i += 1;
                if i < args.len() {
                    port = args[i].parse().expect("invalid port number");
                }
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let state = Arc::new(DashboardState {
        db_path: db_path.into(),
    });

    let app = create_router(state);
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to bind to {addr}: {e}");
            std::process::exit(1);
        });

    println!("Dashboard at http://{addr}");
    axum::serve(listener, app).await.unwrap();
}
