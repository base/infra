//! Conductor RPC client for sequencer health checks.
//!
//! Ports the JSON-RPC calls from `base/agent/service/service/sequencer_checker.go`.
//! The Conductor is the OP Stack component that manages sequencer leadership.

use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Information about the current leader from `conductor_leaderWithID`.
#[derive(Debug, Clone, Deserialize)]
pub struct LeaderInfo {
    pub id: String,
    pub addr: String,
    pub suffrage: i32,
}

/// JSON-RPC request structure.
#[derive(Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    params: Vec<()>,
    id: u64,
}

/// JSON-RPC response structure.
#[derive(Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcError {
    code: i64,
    message: String,
}

/// Client for the Conductor JSON-RPC API.
///
/// Used by sequencer nodes to check leadership status and health.
#[derive(Clone)]
pub struct ConductorClient {
    client: Client,
    url: String,
}

impl ConductorClient {
    /// Create a new Conductor client.
    pub fn new(url: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("failed to build HTTP client");

        Self { client, url: url.into() }
    }

    /// Make a JSON-RPC call and return the result.
    async fn call<T: for<'de> Deserialize<'de>>(&self, method: &str) -> Result<T, ConductorError> {
        let request = JsonRpcRequest { jsonrpc: "2.0", method, params: vec![], id: 1 };

        let response = self
            .client
            .post(&self.url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ConductorError::Request(e.to_string()))?;

        if !response.status().is_success() {
            return Err(ConductorError::Http(response.status().as_u16()));
        }

        let rpc_response: JsonRpcResponse<T> =
            response.json().await.map_err(|e| ConductorError::Parse(e.to_string()))?;

        if let Some(error) = rpc_response.error {
            return Err(ConductorError::Rpc { code: error.code, message: error.message });
        }

        rpc_response.result.ok_or_else(|| ConductorError::Parse("missing result field".to_string()))
    }

    /// Check if this node is the current leader.
    /// Maps to `conductor_leader` RPC method.
    pub async fn is_leader(&self) -> Result<bool, ConductorError> {
        self.call("conductor_leader").await
    }

    /// Get leader info including ID.
    /// Maps to `conductor_leaderWithID` RPC method.
    pub async fn leader_with_id(&self) -> Result<LeaderInfo, ConductorError> {
        self.call("conductor_leaderWithID").await
    }

    /// Check if the conductor is active.
    /// Maps to `conductor_active` RPC method.
    pub async fn is_active(&self) -> Result<bool, ConductorError> {
        self.call("conductor_active").await
    }

    /// Check if the sequencer is healthy (from conductor's perspective).
    /// Maps to `conductor_sequencerHealthy` RPC method.
    pub async fn is_sequencer_healthy(&self) -> Result<bool, ConductorError> {
        self.call("conductor_sequencerHealthy").await
    }

    /// Check if the conductor is paused.
    /// Maps to `conductor_paused` RPC method.
    pub async fn is_paused(&self) -> Result<bool, ConductorError> {
        self.call("conductor_paused").await
    }

    /// Check if the conductor is stopped.
    /// Maps to `conductor_stopped` RPC method.
    pub async fn is_stopped(&self) -> Result<bool, ConductorError> {
        self.call("conductor_stopped").await
    }
}

/// Errors that can occur when communicating with the Conductor.
#[derive(Debug)]
pub enum ConductorError {
    /// HTTP request failed
    Request(String),
    /// Non-2xx HTTP status
    Http(u16),
    /// Failed to parse response
    Parse(String),
    /// JSON-RPC error returned
    Rpc { code: i64, message: String },
}

impl std::fmt::Display for ConductorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(e) => write!(f, "request failed: {}", e),
            Self::Http(status) => write!(f, "HTTP error: {}", status),
            Self::Parse(e) => write!(f, "parse error: {}", e),
            Self::Rpc { code, message } => write!(f, "RPC error {}: {}", code, message),
        }
    }
}

impl std::error::Error for ConductorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conductor_error_display() {
        let err = ConductorError::Request("timeout".to_string());
        assert_eq!(err.to_string(), "request failed: timeout");

        let err = ConductorError::Http(500);
        assert_eq!(err.to_string(), "HTTP error: 500");

        let err = ConductorError::Rpc { code: -32600, message: "Invalid Request".to_string() };
        assert_eq!(err.to_string(), "RPC error -32600: Invalid Request");
    }
}
