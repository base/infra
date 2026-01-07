//! JSON-RPC method handlers for the mock Ethereum node.

use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::state::NodeState;

/// JSON-RPC request structure.
#[derive(Debug, Deserialize)]
pub(super) struct JsonRpcRequest {
    /// JSON-RPC version (should be "2.0").
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// Method name.
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: Value,
    /// Request ID.
    pub id: Value,
}

/// JSON-RPC response structure.
#[derive(Debug, Serialize)]
pub(super) struct JsonRpcResponse {
    /// JSON-RPC version.
    pub jsonrpc: &'static str,
    /// Request ID.
    pub id: Value,
    /// Success result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error object.
#[derive(Debug, Serialize)]
pub(super) struct JsonRpcError {
    /// Error code.
    pub code: i64,
    /// Error message.
    pub message: String,
}

impl JsonRpcResponse {
    /// Create a success response.
    #[allow(clippy::missing_const_for_fn)] // Value is not const
    pub(super) fn success(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    /// Create an error response.
    pub(super) fn error(id: Value, code: i64, message: &str) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message: message.to_string() }),
        }
    }
}

/// Handle incoming JSON-RPC requests.
///
/// Supports both single requests and batch requests.
pub(super) async fn handle_rpc(
    State(state): State<Arc<NodeState>>,
    body: String,
) -> impl IntoResponse {
    // Simulate latency
    if state.latency > std::time::Duration::ZERO {
        tokio::time::sleep(state.latency).await;
    }

    // Check health
    if !state.is_healthy() {
        return (StatusCode::SERVICE_UNAVAILABLE, "Node unhealthy").into_response();
    }

    // Record request
    state.record_request();

    // Parse request - could be single or batch
    let parsed: Result<Value, _> = serde_json::from_str(&body);
    match parsed {
        Ok(Value::Array(requests)) => {
            // Batch request
            let responses: Vec<JsonRpcResponse> = requests
                .into_iter()
                .filter_map(|req| {
                    serde_json::from_value::<JsonRpcRequest>(req)
                        .ok()
                        .map(|r| handle_single_request(&state, r))
                })
                .collect();
            Json(responses).into_response()
        }
        Ok(value) => {
            // Single request
            serde_json::from_value::<JsonRpcRequest>(value).map_or_else(
                |_| {
                    let error = JsonRpcResponse::error(Value::Null, -32600, "Invalid Request");
                    Json(error).into_response()
                },
                |request| {
                    let response = handle_single_request(&state, request);
                    Json(response).into_response()
                },
            )
        }
        Err(_) => {
            let error = JsonRpcResponse::error(Value::Null, -32700, "Parse error");
            Json(error).into_response()
        }
    }
}

/// Handle a single JSON-RPC request.
fn handle_single_request(state: &NodeState, request: JsonRpcRequest) -> JsonRpcResponse {
    match request.method.as_str() {
        "eth_blockNumber" => JsonRpcResponse::success(request.id, json!(state.block_number())),
        "eth_chainId" => JsonRpcResponse::success(request.id, json!(state.chain_id_hex())),
        "net_version" => JsonRpcResponse::success(request.id, json!(state.net_version())),
        "web3_clientVersion" => JsonRpcResponse::success(request.id, json!(state.client_version())),
        "eth_gasPrice" => JsonRpcResponse::success(request.id, json!(state.gas_price())),
        "eth_getBalance" => {
            // Params: [address, block]
            if let Some(address) = request.params.get(0).and_then(|v| v.as_str()) {
                let balance = state.get_balance(address);
                JsonRpcResponse::success(request.id, json!(balance))
            } else {
                JsonRpcResponse::error(request.id, -32602, "Invalid params: missing address")
            }
        }
        "eth_getTransactionCount" => {
            // Return 0 for simplicity - all accounts have no transactions
            JsonRpcResponse::success(request.id, json!("0x0"))
        }
        "eth_call" => {
            // Return empty data for any call
            JsonRpcResponse::success(request.id, json!("0x"))
        }
        "eth_estimateGas" => {
            // Return standard gas estimate
            JsonRpcResponse::success(request.id, json!("0x5208")) // 21000
        }
        "eth_sendRawTransaction" => {
            // Return a fake transaction hash
            JsonRpcResponse::success(
                request.id,
                json!("0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"),
            )
        }
        "rpc_nodeInfo" => {
            // Custom method for debugging - returns full node info
            JsonRpcResponse::success(
                request.id,
                json!({
                    "name": state.name,
                    "clientVersion": state.client_version(),
                    "chainId": state.chain_id,
                    "blockNumber": state.block_number_u64(),
                    "healthy": state.is_healthy(),
                    "requestCount": state.request_count(),
                    "latencyMs": state.latency.as_millis(),
                }),
            )
        }
        _ => JsonRpcResponse::error(
            request.id,
            -32601,
            &format!("Method not found: {}", request.method),
        ),
    }
}

/// Health check handler.
pub(super) async fn health_check(State(state): State<Arc<NodeState>>) -> impl IntoResponse {
    if state.is_healthy() {
        (StatusCode::OK, "OK")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "Unhealthy")
    }
}
