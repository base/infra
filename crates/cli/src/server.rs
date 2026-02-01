//! Server execution utilities.
//!
//! This module provides the HTTP server startup and lifecycle management.

use std::sync::Arc;

use axum::{Router, routing::get};
use eyre::{Context, Result};
use roxy_config::RoxyConfig;
use roxy_server::{RoxyMetrics, metrics_handler};
use tokio::sync::broadcast;

/// Wait for a shutdown signal (SIGINT or SIGTERM on Unix, Ctrl+C on Windows).
#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");

    tokio::select! {
        _ = sigterm.recv() => {
            info!("Received SIGTERM, initiating graceful shutdown...");
        }
        _ = sigint.recv() => {
            info!("Received SIGINT, initiating graceful shutdown...");
        }
    }
}

/// Wait for a shutdown signal (Ctrl+C on non-Unix platforms).
#[cfg(not(unix))]
async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.ok();
    info!("Received Ctrl+C, initiating graceful shutdown...");
}

/// Run the HTTP server with optional metrics endpoint.
///
/// This function starts the HTTP server and optionally a separate metrics server
/// if metrics are enabled in the configuration. Both servers will be shut down
/// gracefully when a shutdown signal is received (SIGINT/SIGTERM on Unix, Ctrl+C on Windows).
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
        .wrap_err_with(|| format!("failed to bind to {addr}"))?;

    info!(address = %addr, "Roxy RPC proxy listening");

    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    let metrics_handle = if config.metrics.enabled {
        let metrics_addr = format!("{}:{}", config.metrics.host, config.metrics.port);
        let metrics_listener = tokio::net::TcpListener::bind(&metrics_addr)
            .await
            .wrap_err_with(|| format!("failed to bind metrics server to {metrics_addr}"))?;

        info!(address = %metrics_addr, "Metrics server listening");

        let metrics =
            Arc::new(RoxyMetrics::new().wrap_err("failed to initialize metrics recorder")?);

        let metrics_app = Router::new().route("/metrics", get(metrics_handler)).with_state(metrics);

        let mut shutdown_rx = shutdown_tx.subscribe();
        Some(tokio::spawn(async move {
            let shutdown = async move {
                shutdown_rx.recv().await.ok();
            };
            axum::serve(metrics_listener, metrics_app).with_graceful_shutdown(shutdown).await.ok();
        }))
    } else {
        None
    };

    let shutdown = {
        let shutdown_tx = shutdown_tx.clone();
        async move {
            shutdown_signal().await;
            shutdown_tx.send(()).ok();
        }
    };

    axum::serve(listener, app).with_graceful_shutdown(shutdown).await.wrap_err("server error")?;

    if let Some(handle) = metrics_handle {
        handle.await.ok();
    }

    info!("Server shut down successfully");
    Ok(())
}
