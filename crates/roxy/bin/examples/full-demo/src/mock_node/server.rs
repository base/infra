//! Mock Ethereum node HTTP server.

use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    routing::{get, post},
};
use eyre::{Context, Result};
use tokio::{sync::oneshot, task::JoinHandle};

use super::{handlers, state::NodeState};

/// A mock Ethereum node server.
///
/// Provides a simple JSON-RPC server that responds to common Ethereum RPC
/// methods with simulated data.
pub(crate) struct MockNode {
    /// Shared node state.
    pub state: Arc<NodeState>,
    /// Port the server is listening on.
    pub port: u16,
    /// Shutdown signal sender.
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Block progression task handle.
    block_task: Option<JoinHandle<()>>,
    /// Server task handle.
    server_task: Option<JoinHandle<()>>,
}

impl MockNode {
    /// Create and start a mock node server.
    ///
    /// # Arguments
    ///
    /// * `name` - Unique identifier for this node
    /// * `port` - Port to listen on
    /// * `chain_id` - Chain ID for responses
    /// * `initial_block` - Starting block number
    /// * `latency` - Simulated response latency
    pub(crate) async fn start(
        name: &str,
        port: u16,
        chain_id: u64,
        initial_block: u64,
        latency: Duration,
    ) -> Result<Self> {
        let state = Arc::new(NodeState::new(name, chain_id, initial_block, latency));

        // Create router
        let app = Router::new()
            .route("/", post(handlers::handle_rpc))
            .route("/health", get(handlers::health_check))
            .with_state(state.clone());

        // Bind to port
        let addr = format!("127.0.0.1:{}", port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .wrap_err_with(|| format!("failed to bind mock node to {}", addr))?;

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // Spawn server task
        let server_task = tokio::spawn(async move {
            let shutdown = async {
                shutdown_rx.await.ok();
            };
            axum::serve(listener, app).with_graceful_shutdown(shutdown).await.ok();
        });

        Ok(Self {
            state,
            port,
            shutdown_tx: Some(shutdown_tx),
            block_task: None,
            server_task: Some(server_task),
        })
    }

    /// Get the URL for this node.
    pub(crate) fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Get the node name.
    pub(crate) fn name(&self) -> &str {
        &self.state.name
    }

    /// Start block progression task.
    ///
    /// Increments the block number at the specified interval.
    pub(crate) fn start_block_progression(&mut self, interval: Duration) {
        let state = self.state.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                state.advance_block();
            }
        });
        self.block_task = Some(task);
    }

    /// Set node health status.
    pub(crate) fn set_healthy(&self, healthy: bool) {
        self.state.set_healthy(healthy);
    }

    /// Get request count for this node.
    pub(crate) fn request_count(&self) -> u64 {
        self.state.request_count()
    }

    /// Gracefully shutdown the node.
    pub(crate) async fn shutdown(mut self) {
        // Stop block progression
        if let Some(task) = self.block_task.take() {
            task.abort();
        }

        // Send shutdown signal
        if let Some(tx) = self.shutdown_tx.take() {
            tx.send(()).ok();
        }

        // Wait for server to stop
        if let Some(task) = self.server_task.take() {
            task.await.ok();
        }
    }
}

/// Configuration for a mock node.
#[derive(Debug, Clone)]
pub(crate) struct MockNodeConfig {
    /// Node name.
    pub name: String,
    /// Port to listen on.
    pub port: u16,
    /// Simulated latency in milliseconds.
    pub latency_ms: u64,
}

impl MockNodeConfig {
    /// Create a new mock node configuration.
    pub(crate) fn new(name: &str, port: u16, latency_ms: u64) -> Self {
        Self { name: name.to_string(), port, latency_ms }
    }
}
