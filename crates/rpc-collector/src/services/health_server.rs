//! Health check HTTP server.
//!
//! Exposes a `/_health` endpoint that returns 200 OK.

use std::net::SocketAddr;

use axum::{Router, routing::get};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::services::Service;

async fn health() -> &'static str {
    "OK"
}

/// Bind and serve the health check server.
///
/// The server shuts down gracefully when the `cancel` token is triggered.
async fn serve(addr: SocketAddr, cancel: CancellationToken) -> anyhow::Result<()> {
    let app = Router::new().route("/_health", get(health));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("health server listening on {addr}");

    axum::serve(listener, app).with_graceful_shutdown(cancel.cancelled_owned()).await?;

    Ok(())
}

/// The health HTTP server, implementing [`Service`].
#[derive(Debug)]
pub struct HealthServer {
    /// Address to bind the health check server.
    pub addr: SocketAddr,
}

impl Service for HealthServer {
    fn name(&self) -> &str {
        "health-server"
    }

    fn spawn(self: Box<Self>, set: &mut JoinSet<()>, cancel: CancellationToken) {
        let addr = self.addr;
        set.spawn(async move {
            if let Err(e) = serve(addr, cancel).await {
                error!(error = %e, "health server failed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: start the health server on an OS-assigned port, returning the
    /// bound address and a cancel token.
    async fn start_server() -> (SocketAddr, CancellationToken) {
        let cancel = CancellationToken::new();
        // Bind to port 0 to get an OS-assigned port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            let app = Router::new().route("/_health", get(health));
            axum::serve(listener, app)
                .with_graceful_shutdown(cancel_clone.cancelled_owned())
                .await
                .unwrap();
        });

        // Give the server a moment to start.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        (addr, cancel)
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let (addr, cancel) = start_server().await;

        let url = format!("http://{addr}/_health");
        let resp = reqwest::get(&url).await.unwrap();

        assert_eq!(resp.status(), 200);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "OK");

        cancel.cancel();
    }

    #[tokio::test]
    async fn health_server_shuts_down() {
        let (addr, cancel) = start_server().await;

        // Verify it's running.
        let url = format!("http://{addr}/_health");
        assert!(reqwest::get(&url).await.is_ok());

        // Cancel and give it time to shut down.
        cancel.cancel();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // After shutdown, connections should fail.
        let result = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap()
            .get(&url)
            .send()
            .await;

        assert!(result.is_err());
    }
}
