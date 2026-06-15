//! HTTP server exposing the `/metrics` endpoint for Prometheus scraping.
//!
//! This module provides [`serve_metrics`], which starts a lightweight
//! [axum] HTTP server that responds to `GET /metrics` with the
//! Prometheus text exposition format.
//!
//! # Usage
//!
//! ```no_run
//! use std::sync::Arc;
//! use ox_metrics::metrics::OxMetrics;
//! use ox_metrics::server::serve_metrics;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let metrics = Arc::new(OxMetrics::new());
//! serve_metrics(metrics, 9091).await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use axum::{Router, routing::get};

use crate::metrics::OxMetrics;

/// Default bind host for the metrics endpoint.
///
/// Loopback only: `/metrics` exposes the structure of the pipeline
/// (rule names, session metadata), which on a shared host — an HPC
/// login node is the canonical deployment — must not be scrapeable by
/// arbitrary peers. Exposing on other interfaces is an explicit opt-in
/// via [`serve_metrics_on`].
pub const DEFAULT_METRICS_HOST: &str = "127.0.0.1";

/// Whether a host string names a loopback-only interface.
fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// Build the `host:port` bind address for the metrics listener.
fn bind_address(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

/// Start an HTTP server on `127.0.0.1:{port}` serving Prometheus metrics.
///
/// The server exposes a single route:
///
/// - `GET /metrics` — returns all registered OxyMake metrics in
///   Prometheus text exposition format.
///
/// This function runs until the server is shut down (e.g., via
/// `tokio::select!` or process exit). It is typically spawned as a
/// background task alongside the main `ox run` execution loop.
///
/// Binds loopback only (see [`DEFAULT_METRICS_HOST`]). To expose the
/// endpoint on other interfaces, use [`serve_metrics_on`] — that is an
/// explicit opt-in and logs a warning.
///
/// # Errors
///
/// Returns an error if the TCP listener cannot bind to the requested
/// port or if the server encounters a fatal I/O error.
pub async fn serve_metrics(
    metrics: Arc<OxMetrics>,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_metrics_on(metrics, DEFAULT_METRICS_HOST, port).await
}

/// Start the metrics server on an explicit host interface.
///
/// Binding a non-loopback host (e.g. `0.0.0.0`) makes `/metrics` —
/// rule names, session metadata, pipeline shape — readable by anyone
/// who can reach that interface, with no authentication. A warning is
/// emitted on stderr in that case; prefer the loopback default plus a
/// reverse proxy or SSH tunnel for remote scraping.
pub async fn serve_metrics_on(
    metrics: Arc<OxMetrics>,
    host: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !is_loopback_host(host) {
        eprintln!(
            "warning: metrics endpoint bound to {host}:{port} without authentication — \
             /metrics exposes rule names and session metadata to any peer that can \
             reach this interface; prefer 127.0.0.1 with a reverse proxy or SSH tunnel"
        );
    }

    let app = Router::new().route(
        "/metrics",
        get(move || {
            let metrics = metrics.clone();
            async move { metrics.encode() }
        }),
    );

    let listener = tokio::net::TcpListener::bind(bind_address(host, port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// H38: the default bind must be loopback — on a shared HPC login
    /// node, 0.0.0.0 lets every peer scrape the pipeline's structure.
    #[test]
    fn default_host_is_loopback() {
        assert!(is_loopback_host(DEFAULT_METRICS_HOST));
        assert_eq!(bind_address(DEFAULT_METRICS_HOST, 9091), "127.0.0.1:9091");
    }

    #[test]
    fn non_loopback_hosts_are_flagged() {
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("::"));
        assert!(!is_loopback_host("192.168.1.10"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("::1"));
    }

    /// The default entry point must actually listen on loopback: a
    /// socket bound by serve_metrics is reachable via 127.0.0.1.
    #[tokio::test]
    async fn serve_metrics_listens_on_loopback() {
        // Reserve a free port, release it, then race to rebind it.
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let metrics = Arc::new(OxMetrics::new());
        let server = tokio::spawn(serve_metrics(metrics, port));

        // Wait until the listener accepts.
        let mut connected = false;
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                connected = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(connected, "metrics server must accept on 127.0.0.1");
        server.abort();
    }
}
