//! Server execution utilities.
//!
//! This module provides the HTTP server startup and lifecycle management.

use eyre::{Context, Result};
use roxy_config::RoxyConfig;

/// Run the HTTP server.
///
/// This function starts the HTTP server and blocks until shutdown is requested
/// (via Ctrl+C signal).
///
/// # Arguments
///
/// * `app` - The axum Router
/// * `config` - The Roxy configuration
///
/// # Errors
///
/// Returns an error if the server fails to start or encounters an error while running.
pub async fn run_server(app: roxy_server::Router, config: &RoxyConfig) -> Result<()> {
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .wrap_err_with(|| format!("failed to bind to {}", addr))?;

    info!(address = %addr, "Roxy RPC proxy listening");

    // Graceful shutdown on Ctrl+C
    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        info!("Received shutdown signal, shutting down gracefully...");
    };

    axum::serve(listener, app).with_graceful_shutdown(shutdown).await.wrap_err("server error")?;

    info!("Server shut down successfully");
    Ok(())
}
