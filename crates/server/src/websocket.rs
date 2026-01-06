//! WebSocket handler for Roxy RPC proxy.
//!
//! This module provides WebSocket support for subscription-based JSON-RPC methods
//! like `eth_subscribe` and `eth_unsubscribe`. It handles:
//!
//! - WebSocket connection upgrades
//! - Subscription lifecycle management
//! - Connection limits and tracking
//! - Message parsing and serialization

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, mpsc, oneshot};

/// Application state shared across WebSocket connections.
///
/// This is a placeholder that should be replaced with the actual AppState
/// when the HTTP handler module provides it.
#[derive(Debug)]
pub struct AppState {
    /// Connection tracker for limiting concurrent WebSocket connections
    pub connection_tracker: ConnectionTracker,
    /// Maximum subscriptions per connection
    pub max_subscriptions_per_connection: usize,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            connection_tracker: ConnectionTracker::new(1000),
            max_subscriptions_per_connection: 100,
        }
    }
}

// ============================================================================
// WebSocket Message Types
// ============================================================================

/// Incoming WebSocket JSON-RPC request.
#[derive(Debug, Clone, Deserialize)]
pub struct WsRequest {
    /// JSON-RPC version (should be "2.0")
    pub jsonrpc: String,
    /// The RPC method being called
    pub method: String,
    /// Optional parameters for the method
    #[serde(default)]
    pub params: Option<serde_json::Value>,
    /// Request ID for correlation
    pub id: serde_json::Value,
}

/// JSON-RPC response for WebSocket requests.
#[derive(Debug, Clone, Serialize)]
pub struct WsResponse {
    /// JSON-RPC version
    pub jsonrpc: String,
    /// Request ID for correlation
    pub id: serde_json::Value,
    /// Result on success
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error on failure
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WsError>,
}

impl WsResponse {
    /// Create a successful response.
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self { jsonrpc: "2.0".to_string(), id, result: Some(result), error: None }
    }

    /// Create an error response.
    pub fn error(id: serde_json::Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(WsError { code, message: message.into(), data: None }),
        }
    }
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize)]
pub struct WsError {
    /// Error code
    pub code: i64,
    /// Error message
    pub message: String,
    /// Optional additional data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Outgoing subscription notification.
#[derive(Debug, Clone, Serialize)]
pub struct WsNotification {
    /// JSON-RPC version
    pub jsonrpc: String,
    /// Method for the notification (e.g., "eth_subscription")
    pub method: String,
    /// Subscription parameters containing the data
    pub params: SubscriptionParams,
}

impl WsNotification {
    /// Create a new subscription notification.
    pub fn new(subscription_id: String, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: "eth_subscription".to_string(),
            params: SubscriptionParams { subscription: subscription_id, result },
        }
    }
}

/// Parameters for subscription notifications.
#[derive(Debug, Clone, Serialize)]
pub struct SubscriptionParams {
    /// The subscription ID
    pub subscription: String,
    /// The subscription result/data
    pub result: serde_json::Value,
}

// ============================================================================
// Connection Tracking
// ============================================================================

/// Tracks active WebSocket connections with configurable limits.
///
/// This tracker uses atomic operations and is designed to be wrapped in `Arc`
/// for shared access across async tasks.
#[derive(Debug)]
pub struct ConnectionTracker {
    /// Current number of active connections
    current: AtomicUsize,
    /// Maximum allowed connections
    max: usize,
}

impl ConnectionTracker {
    /// Create a new connection tracker with the given maximum.
    pub const fn new(max: usize) -> Self {
        Self { current: AtomicUsize::new(0), max }
    }

    /// Try to acquire a connection slot.
    ///
    /// Returns `true` if a slot was acquired, `false` if the limit was reached.
    /// If `true` is returned, the caller is responsible for calling `release()`
    /// when the connection closes.
    pub fn try_acquire(&self) -> bool {
        // Use a CAS loop to safely increment
        loop {
            let current = self.current.load(Ordering::SeqCst);
            if current >= self.max {
                return false;
            }
            if self
                .current
                .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
            // CAS failed, retry
        }
    }

    /// Release a connection slot.
    ///
    /// This should be called when a connection closes after a successful `try_acquire()`.
    pub fn release(&self) {
        self.current.fetch_sub(1, Ordering::SeqCst);
    }

    /// Get the current number of active connections.
    pub fn current(&self) -> usize {
        self.current.load(Ordering::SeqCst)
    }

    /// Get the maximum number of allowed connections.
    pub const fn max(&self) -> usize {
        self.max
    }
}

/// RAII guard for connection tracking.
///
/// When dropped, automatically decrements the connection count.
/// This guard owns an Arc to the state, allowing it to be moved across async boundaries.
#[derive(Debug)]
pub struct ConnectionGuard {
    state: Arc<AppState>,
}

impl ConnectionGuard {
    /// Create a new connection guard that will release the connection slot when dropped.
    pub const fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.state.connection_tracker.release();
    }
}

// ============================================================================
// Subscription Management
// ============================================================================

/// Handle for an active subscription.
#[derive(Debug)]
pub struct SubscriptionHandle {
    /// Unique subscription ID
    pub id: String,
    /// The subscription method (e.g., "newHeads", "logs")
    pub method: String,
    /// Channel to signal cancellation
    cancel: oneshot::Sender<()>,
}

/// Manages active subscriptions for a WebSocket connection.
#[derive(Debug)]
pub struct SubscriptionManager {
    /// Map of subscription ID to handle
    subscriptions: HashMap<String, SubscriptionHandle>,
    /// Sender for outgoing messages
    sender: mpsc::Sender<String>,
    /// Maximum number of subscriptions allowed
    max_subscriptions: usize,
    /// Counter for generating subscription IDs
    next_id: u64,
}

impl SubscriptionManager {
    /// Create a new subscription manager.
    pub fn new(sender: mpsc::Sender<String>, max_subscriptions: usize) -> Self {
        Self { subscriptions: HashMap::new(), sender, max_subscriptions, next_id: 1 }
    }

    /// Generate a new subscription ID.
    fn generate_id(&mut self) -> String {
        let id = format!("0x{:x}", self.next_id);
        self.next_id += 1;
        id
    }

    /// Add a new subscription.
    ///
    /// # Arguments
    /// * `method` - The subscription method (e.g., "newHeads", "logs", "newPendingTransactions")
    ///
    /// # Returns
    /// The subscription ID on success, or an error if limits are exceeded.
    pub async fn subscribe(&mut self, method: String) -> eyre::Result<String> {
        if self.subscriptions.len() >= self.max_subscriptions {
            return Err(eyre::eyre!("Maximum subscriptions ({}) exceeded", self.max_subscriptions));
        }

        let id = self.generate_id();
        let (cancel_tx, cancel_rx) = oneshot::channel();

        // Store the handle
        let handle =
            SubscriptionHandle { id: id.clone(), method: method.clone(), cancel: cancel_tx };
        self.subscriptions.insert(id.clone(), handle);

        // Spawn the subscription task based on method
        let sender = self.sender.clone();
        let sub_id = id.clone();

        tokio::spawn(async move {
            Self::run_subscription(sub_id, method, sender, cancel_rx).await;
        });

        Ok(id)
    }

    /// Run a subscription task.
    async fn run_subscription(
        id: String,
        method: String,
        sender: mpsc::Sender<String>,
        mut cancel: oneshot::Receiver<()>,
    ) {
        // For now, this is a placeholder that simulates subscription events
        // In a real implementation, this would connect to the backend and stream events
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(12));

        loop {
            tokio::select! {
                _ = &mut cancel => {
                    debug!(subscription_id = %id, "Subscription cancelled");
                    break;
                }
                _ = interval.tick() => {
                    // Create a notification based on the subscription type
                    let result = match method.as_str() {
                        "newHeads" => {
                            // Placeholder for block header
                            serde_json::json!({
                                "number": "0x0",
                                "hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                                "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                                "timestamp": "0x0"
                            })
                        }
                        "logs" => {
                            // Placeholder for log entry
                            serde_json::json!({
                                "address": "0x0000000000000000000000000000000000000000",
                                "topics": [],
                                "data": "0x"
                            })
                        }
                        "newPendingTransactions" => {
                            // Placeholder for pending tx hash
                            serde_json::json!("0x0000000000000000000000000000000000000000000000000000000000000000")
                        }
                        _ => {
                            serde_json::json!(null)
                        }
                    };

                    let notification = WsNotification::new(id.clone(), result);
                    if let Ok(msg) = serde_json::to_string(&notification)
                        && sender.send(msg).await.is_err() {
                            // Channel closed, stop the subscription
                            break;
                        }
                }
            }
        }
    }

    /// Remove a subscription by ID.
    ///
    /// # Returns
    /// `true` if the subscription was found and removed, `false` otherwise.
    pub fn unsubscribe(&mut self, id: &str) -> bool {
        if let Some(handle) = self.subscriptions.remove(id) {
            // Send cancellation signal (ignore if receiver is dropped)
            let _ = handle.cancel.send(());
            true
        } else {
            false
        }
    }

    /// Get the number of active subscriptions.
    pub fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    /// Check if a subscription exists.
    pub fn has_subscription(&self, id: &str) -> bool {
        self.subscriptions.contains_key(id)
    }

    /// Clean up all subscriptions.
    pub fn close_all(&mut self) {
        for (_, handle) in self.subscriptions.drain() {
            let _ = handle.cancel.send(());
        }
    }
}

impl Drop for SubscriptionManager {
    fn drop(&mut self) {
        self.close_all();
    }
}

// ============================================================================
// WebSocket Handler
// ============================================================================

/// WebSocket connection handler.
///
/// This is the main entry point for WebSocket connections. It upgrades the
/// HTTP connection to a WebSocket connection and spawns a task to handle
/// the connection.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Try to acquire a connection slot
    if state.connection_tracker.try_acquire() {
        // Create a guard that will release the slot when dropped
        let guard = ConnectionGuard::new(state.clone());
        ws.on_upgrade(move |socket| handle_socket(socket, state, guard))
    } else {
        // Connection limit reached - still need to upgrade but close immediately
        ws.on_upgrade(|socket: WebSocket| async {
            let (mut sender, _) = socket.split();
            let error_msg =
                WsResponse::error(serde_json::Value::Null, -32000, "Connection limit reached");
            if let Ok(msg) = serde_json::to_string(&error_msg) {
                let _ = sender.send(Message::Text(msg)).await;
            }
            let _ = sender.close().await;
        })
    }
}

/// Handle an individual WebSocket connection.
///
/// The `_guard` is kept alive for the duration of the connection and will
/// automatically release the connection slot when dropped.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>, _guard: ConnectionGuard) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Create a channel for outgoing messages
    let (msg_tx, mut msg_rx) = mpsc::channel::<String>(100);

    // Create subscription manager wrapped in Arc<RwLock> for shared access
    let sub_manager = Arc::new(RwLock::new(SubscriptionManager::new(
        msg_tx.clone(),
        state.max_subscriptions_per_connection,
    )));

    // Spawn task to forward messages from the channel to the WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = msg_rx.recv().await {
            if ws_sender.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
        // Try to close the WebSocket gracefully
        let _ = ws_sender.close().await;
    });

    // Handle incoming messages
    let sub_manager_clone = sub_manager.clone();
    let msg_tx_clone = msg_tx.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let response = handle_message(&text, &sub_manager_clone, &msg_tx_clone).await;
                    if let Some(resp) = response
                        && msg_tx_clone.send(resp).await.is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Binary(data)) => {
                    if let Ok(text) = String::from_utf8(data.to_vec()) {
                        let response =
                            handle_message(&text, &sub_manager_clone, &msg_tx_clone).await;
                        if let Some(resp) = response
                            && msg_tx_clone.send(resp).await.is_err()
                        {
                            break;
                        }
                    }
                }
                Ok(Message::Ping(_)) => {
                    // Pong is handled automatically by axum
                }
                Ok(Message::Pong(_)) => {
                    // Ignore pong messages
                }
                Ok(Message::Close(_)) => {
                    debug!("WebSocket close received");
                    break;
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = send_task => {
            debug!("Send task completed");
        }
        _ = recv_task => {
            debug!("Receive task completed");
        }
    }

    // Clean up subscriptions
    sub_manager.write().await.close_all();
    debug!("WebSocket connection closed");
}

/// Handle a single incoming message.
async fn handle_message(
    text: &str,
    sub_manager: &Arc<RwLock<SubscriptionManager>>,
    _msg_tx: &mpsc::Sender<String>,
) -> Option<String> {
    // Parse the request
    let request: WsRequest = match serde_json::from_str(text) {
        Ok(req) => req,
        Err(e) => {
            let response =
                WsResponse::error(serde_json::Value::Null, -32700, format!("Parse error: {}", e));
            return serde_json::to_string(&response).ok();
        }
    };

    // Handle the request based on method
    let response = match request.method.as_str() {
        "eth_subscribe" => handle_subscribe(&request, sub_manager).await,
        "eth_unsubscribe" => handle_unsubscribe(&request, sub_manager).await,
        _ => {
            // For non-subscription methods, we could forward to the HTTP handler
            // For now, return an error
            WsResponse::error(
                request.id,
                -32601,
                format!("Method '{}' not supported over WebSocket", request.method),
            )
        }
    };

    serde_json::to_string(&response).ok()
}

/// Handle eth_subscribe requests.
async fn handle_subscribe(
    request: &WsRequest,
    sub_manager: &Arc<RwLock<SubscriptionManager>>,
) -> WsResponse {
    // Extract subscription type from params
    let subscription_type = match &request.params {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => match arr[0].as_str() {
            Some(s) => s.to_string(),
            None => {
                return WsResponse::error(request.id.clone(), -32602, "Invalid subscription type");
            }
        },
        _ => {
            return WsResponse::error(
                request.id.clone(),
                -32602,
                "Missing subscription type parameter",
            );
        }
    };

    // Validate subscription type
    match subscription_type.as_str() {
        "newHeads" | "logs" | "newPendingTransactions" | "syncing" => {}
        _ => {
            return WsResponse::error(
                request.id.clone(),
                -32602,
                format!("Unknown subscription type: {}", subscription_type),
            );
        }
    }

    // Create the subscription
    let mut manager = sub_manager.write().await;
    match manager.subscribe(subscription_type).await {
        Ok(subscription_id) => {
            WsResponse::success(request.id.clone(), serde_json::Value::String(subscription_id))
        }
        Err(e) => WsResponse::error(request.id.clone(), -32000, e.to_string()),
    }
}

/// Handle eth_unsubscribe requests.
async fn handle_unsubscribe(
    request: &WsRequest,
    sub_manager: &Arc<RwLock<SubscriptionManager>>,
) -> WsResponse {
    // Extract subscription ID from params
    let subscription_id = match &request.params {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => match arr[0].as_str() {
            Some(s) => s.to_string(),
            None => {
                return WsResponse::error(request.id.clone(), -32602, "Invalid subscription ID");
            }
        },
        _ => {
            return WsResponse::error(
                request.id.clone(),
                -32602,
                "Missing subscription ID parameter",
            );
        }
    };

    // Remove the subscription
    let mut manager = sub_manager.write().await;
    let success = manager.unsubscribe(&subscription_id);
    WsResponse::success(request.id.clone(), serde_json::Value::Bool(success))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_tracker_basic() {
        let tracker = ConnectionTracker::new(2);

        assert_eq!(tracker.current(), 0);
        assert_eq!(tracker.max(), 2);

        // Acquire first connection
        assert!(tracker.try_acquire());
        assert_eq!(tracker.current(), 1);

        // Acquire second connection
        assert!(tracker.try_acquire());
        assert_eq!(tracker.current(), 2);

        // Third should fail
        assert!(!tracker.try_acquire());
        assert_eq!(tracker.current(), 2);

        // Release one slot
        tracker.release();
        assert_eq!(tracker.current(), 1);

        // Now we can acquire again
        assert!(tracker.try_acquire());
        assert_eq!(tracker.current(), 2);
    }

    #[test]
    fn test_connection_tracker_zero_max() {
        let tracker = ConnectionTracker::new(0);
        assert!(!tracker.try_acquire());
    }

    #[test]
    fn test_connection_guard_releases_on_drop() {
        let state = Arc::new(AppState::default());

        // Manually acquire a slot
        assert!(state.connection_tracker.try_acquire());
        assert_eq!(state.connection_tracker.current(), 1);

        // Create a guard
        {
            let _guard = ConnectionGuard::new(state.clone());
            // Guard is alive, count should still be 1
            assert_eq!(state.connection_tracker.current(), 1);
        }

        // Guard dropped, count should be 0
        assert_eq!(state.connection_tracker.current(), 0);
    }

    #[tokio::test]
    async fn test_subscription_manager_subscribe() {
        let (tx, _rx) = mpsc::channel(10);
        let mut manager = SubscriptionManager::new(tx, 10);

        // Subscribe to newHeads
        let result = manager.subscribe("newHeads".to_string()).await;
        assert!(result.is_ok());
        let id = result.unwrap();
        assert!(id.starts_with("0x"));
        assert_eq!(manager.subscription_count(), 1);
        assert!(manager.has_subscription(&id));
    }

    #[tokio::test]
    async fn test_subscription_manager_unsubscribe() {
        let (tx, _rx) = mpsc::channel(10);
        let mut manager = SubscriptionManager::new(tx, 10);

        // Subscribe
        let id = manager.subscribe("newHeads".to_string()).await.unwrap();
        assert_eq!(manager.subscription_count(), 1);

        // Unsubscribe
        let result = manager.unsubscribe(&id);
        assert!(result);
        assert_eq!(manager.subscription_count(), 0);
        assert!(!manager.has_subscription(&id));

        // Unsubscribe again should return false
        let result = manager.unsubscribe(&id);
        assert!(!result);
    }

    #[tokio::test]
    async fn test_subscription_manager_limit() {
        let (tx, _rx) = mpsc::channel(10);
        let mut manager = SubscriptionManager::new(tx, 2);

        // Subscribe twice
        let _ = manager.subscribe("newHeads".to_string()).await.unwrap();
        let _ = manager.subscribe("logs".to_string()).await.unwrap();

        // Third should fail
        let result = manager.subscribe("newPendingTransactions".to_string()).await;
        assert!(result.is_err());
        assert_eq!(manager.subscription_count(), 2);
    }

    #[tokio::test]
    async fn test_subscription_manager_close_all() {
        let (tx, _rx) = mpsc::channel(10);
        let mut manager = SubscriptionManager::new(tx, 10);

        // Subscribe multiple times
        let _ = manager.subscribe("newHeads".to_string()).await.unwrap();
        let _ = manager.subscribe("logs".to_string()).await.unwrap();
        assert_eq!(manager.subscription_count(), 2);

        // Close all
        manager.close_all();
        assert_eq!(manager.subscription_count(), 0);
    }

    #[test]
    fn test_ws_request_parsing() {
        let json = r#"{"jsonrpc":"2.0","method":"eth_subscribe","params":["newHeads"],"id":1}"#;
        let request: WsRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.method, "eth_subscribe");
        assert!(request.params.is_some());
        assert_eq!(request.id, serde_json::json!(1));
    }

    #[test]
    fn test_ws_request_without_params() {
        let json = r#"{"jsonrpc":"2.0","method":"eth_blockNumber","id":"abc"}"#;
        let request: WsRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.method, "eth_blockNumber");
        assert!(request.params.is_none());
        assert_eq!(request.id, serde_json::json!("abc"));
    }

    #[test]
    fn test_ws_response_success() {
        let response = WsResponse::success(serde_json::json!(1), serde_json::json!("0x1234"));

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"result\":\"0x1234\""));
        assert!(!json.contains("error"));
    }

    #[test]
    fn test_ws_response_error() {
        let response = WsResponse::error(serde_json::json!(1), -32600, "Invalid Request");

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"code\":-32600"));
        assert!(json.contains("\"message\":\"Invalid Request\""));
        assert!(!json.contains("result"));
    }

    #[test]
    fn test_ws_notification_serialization() {
        let notification =
            WsNotification::new("0x1".to_string(), serde_json::json!({"number": "0x10"}));

        let json = serde_json::to_string(&notification).unwrap();
        assert!(json.contains("\"method\":\"eth_subscription\""));
        assert!(json.contains("\"subscription\":\"0x1\""));
        assert!(json.contains("\"number\":\"0x10\""));
    }
}
