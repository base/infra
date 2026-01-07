//! Demo client for making requests through roxy.

use std::time::{Duration, Instant};

use eyre::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// JSON-RPC request structure.
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    method: String,
    params: Value,
    id: u64,
}

/// JSON-RPC response structure.
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Value,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

/// JSON-RPC error structure.
#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

/// Demo client for making JSON-RPC requests through roxy.
pub(crate) struct DemoClient {
    client: reqwest::Client,
    roxy_url: String,
    next_id: std::sync::atomic::AtomicU64,
}

/// Result of a timed request.
pub(crate) struct TimedResult {
    /// The result value.
    pub value: Value,
    /// Time taken for the request.
    pub duration: Duration,
}

impl DemoClient {
    /// Create a new demo client.
    pub(crate) fn new(roxy_port: u16) -> Self {
        Self {
            client: reqwest::Client::new(),
            roxy_url: format!("http://127.0.0.1:{}", roxy_port),
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Get the next request ID.
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Send a single JSON-RPC request and return the result.
    pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
            id: self.next_id(),
        };

        let response = self
            .client
            .post(&self.roxy_url)
            .json(&request)
            .send()
            .await
            .wrap_err("failed to send request")?;

        let json_response: JsonRpcResponse =
            response.json().await.wrap_err("failed to parse response")?;

        if let Some(error) = json_response.error {
            bail!("RPC error {}: {}", error.code, error.message);
        }

        json_response.result.ok_or_else(|| eyre::eyre!("no result in response"))
    }

    /// Send a single JSON-RPC request and return the result with timing info.
    pub(crate) async fn request_timed(&self, method: &str, params: Value) -> Result<TimedResult> {
        let start = Instant::now();
        let value = self.request(method, params).await?;
        let duration = start.elapsed();
        Ok(TimedResult { value, duration })
    }

    /// Send a batch of JSON-RPC requests.
    pub(crate) async fn batch(&self, requests: Vec<(&str, Value)>) -> Result<Vec<Value>> {
        let batch: Vec<JsonRpcRequest> = requests
            .into_iter()
            .map(|(method, params)| JsonRpcRequest {
                jsonrpc: "2.0",
                method: method.to_string(),
                params,
                id: self.next_id(),
            })
            .collect();

        let response = self
            .client
            .post(&self.roxy_url)
            .json(&batch)
            .send()
            .await
            .wrap_err("failed to send batch request")?;

        let json_responses: Vec<JsonRpcResponse> =
            response.json().await.wrap_err("failed to parse batch response")?;

        json_responses
            .into_iter()
            .map(|r| {
                if let Some(error) = r.error {
                    bail!("RPC error {}: {}", error.code, error.message);
                }
                r.result.ok_or_else(|| eyre::eyre!("no result in response"))
            })
            .collect()
    }

    /// Get the client version (useful for identifying which backend served the request).
    pub(crate) async fn get_client_version(&self) -> Result<String> {
        let result = self.request("web3_clientVersion", json!([])).await?;
        result.as_str().map(String::from).ok_or_else(|| eyre::eyre!("invalid client version"))
    }

    /// Get the current block number.
    pub(crate) async fn get_block_number(&self) -> Result<String> {
        let result = self.request("eth_blockNumber", json!([])).await?;
        result.as_str().map(String::from).ok_or_else(|| eyre::eyre!("invalid block number"))
    }

    /// Get the chain ID.
    #[allow(dead_code)]
    pub(crate) async fn get_chain_id(&self) -> Result<String> {
        let result = self.request("eth_chainId", json!([])).await?;
        result.as_str().map(String::from).ok_or_else(|| eyre::eyre!("invalid chain id"))
    }

    /// Get network version (cacheable).
    #[allow(dead_code)]
    pub(crate) async fn get_net_version(&self) -> Result<String> {
        let result = self.request("net_version", json!([])).await?;
        result.as_str().map(String::from).ok_or_else(|| eyre::eyre!("invalid net version"))
    }
}

/// Parse node name from client version string.
///
/// Client version format: "MockNode/{name}/v1.0.0"
pub(crate) fn parse_node_name(client_version: &str) -> Option<String> {
    let parts: Vec<&str> = client_version.split('/').collect();
    if parts.len() >= 2 && parts[0] == "MockNode" { Some(parts[1].to_string()) } else { None }
}
