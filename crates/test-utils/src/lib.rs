#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/refcell/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use async_trait::async_trait;
use roxy_traits::{Backend, HealthStatus};
use roxy_types::RoxyError;

// ============================================================================
// Mock Backend
// ============================================================================

/// Response type for mock backend.
///
/// # Example
///
/// ```
/// use roxy_test_utils::MockResponse;
///
/// let success = MockResponse::Success(r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#.to_string());
/// let error = MockResponse::Error("connection refused".to_string());
/// let timeout = MockResponse::Timeout;
/// ```
#[derive(Debug, Clone)]
pub enum MockResponse {
    /// Successful JSON response.
    Success(String),
    /// Error with message.
    Error(String),
    /// Simulated timeout.
    Timeout,
}

/// A mock backend for testing.
///
/// Provides configurable responses, latency simulation, and call counting.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use roxy_test_utils::{MockBackend, MockResponse};
/// use roxy_traits::HealthStatus;
///
/// let backend = MockBackend::new("test-backend")
///     .with_response(MockResponse::Success(r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#.to_string()))
///     .with_latency(Duration::from_millis(10))
///     .with_health(HealthStatus::Healthy);
///
/// assert_eq!(backend.call_count(), 0);
/// ```
pub struct MockBackend {
    name: String,
    url: String,
    responses: Mutex<Vec<MockResponse>>,
    call_count: AtomicUsize,
    latency: Duration,
    health: HealthStatus,
    latency_ema_value: Duration,
}

impl std::fmt::Debug for MockBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockBackend")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("call_count", &self.call_count.load(Ordering::SeqCst))
            .field("latency", &self.latency)
            .field("health", &self.health)
            .finish()
    }
}

impl MockBackend {
    /// Create a new mock backend with the given name.
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_test_utils::MockBackend;
    ///
    /// let backend = MockBackend::new("my-backend");
    /// assert_eq!(backend.call_count(), 0);
    /// ```
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            url: format!("http://mock-{}.local", name),
            responses: Mutex::new(Vec::new()),
            call_count: AtomicUsize::new(0),
            latency: Duration::ZERO,
            health: HealthStatus::Healthy,
            latency_ema_value: Duration::from_millis(10),
        }
    }

    /// Set the RPC URL for the mock backend.
    #[must_use]
    pub fn with_url(mut self, url: &str) -> Self {
        self.url = url.to_string();
        self
    }

    /// Add a single response to the queue.
    ///
    /// Responses are consumed in FIFO order. If no responses remain,
    /// the backend returns a default success response.
    #[must_use]
    pub fn with_response(self, response: MockResponse) -> Self {
        self.responses.lock().unwrap().push(response);
        self
    }

    /// Set multiple responses at once.
    #[must_use]
    pub fn with_responses(self, responses: Vec<MockResponse>) -> Self {
        *self.responses.lock().unwrap() = responses;
        self
    }

    /// Set simulated latency for requests.
    #[must_use]
    pub const fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// Set the health status.
    #[must_use]
    pub const fn with_health(mut self, health: HealthStatus) -> Self {
        self.health = health;
        self
    }

    /// Set the latency EMA value for load balancing.
    #[must_use]
    pub const fn with_latency_ema(mut self, latency_ema: Duration) -> Self {
        self.latency_ema_value = latency_ema;
        self
    }

    /// Get the number of times this backend has been called.
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Reset the call count to zero.
    pub fn reset_call_count(&self) {
        self.call_count.store(0, Ordering::SeqCst);
    }

    /// Get the next response from the queue.
    fn next_response(&self) -> Option<MockResponse> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() { None } else { Some(responses.remove(0)) }
    }
}

#[async_trait]
impl Backend for MockBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn rpc_url(&self) -> &str {
        &self.url
    }

    async fn forward(&self, request: RequestPacket) -> Result<ResponsePacket, RoxyError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        // Simulate latency
        if !self.latency.is_zero() {
            tokio::time::sleep(self.latency).await;
        }

        // Get response or use default
        let response = self.next_response().unwrap_or_else(|| {
            // Default: return a success response matching the request
            let id = match &request {
                RequestPacket::Single(req) => req.id().clone(),
                RequestPacket::Batch(reqs) => {
                    reqs.first().map(|r| r.id().clone()).unwrap_or(alloy_json_rpc::Id::None)
                }
            };
            MockResponse::Success(format!(
                r#"{{"jsonrpc":"2.0","id":{},"result":"0x1"}}"#,
                serde_json::to_string(&id).unwrap_or_else(|_| "null".to_string())
            ))
        });

        match response {
            MockResponse::Success(json) => {
                let packet: ResponsePacket = serde_json::from_str(&json).map_err(|e| {
                    RoxyError::Internal(format!("Failed to parse mock response: {e}"))
                })?;
                Ok(packet)
            }
            MockResponse::Error(msg) => Err(RoxyError::BackendOffline { backend: msg }),
            MockResponse::Timeout => {
                // Simulate a very long delay that would trigger a timeout
                tokio::time::sleep(Duration::from_secs(60)).await;
                Err(RoxyError::BackendTimeout { backend: self.name.clone() })
            }
        }
    }

    fn health_status(&self) -> HealthStatus {
        self.health
    }

    fn latency_ema(&self) -> Duration {
        self.latency_ema_value
    }
}

// ============================================================================
// Request Builder
// ============================================================================

/// Builder for JSON-RPC test requests.
///
/// # Example
///
/// ```
/// use roxy_test_utils::RequestBuilder;
/// use serde_json::json;
///
/// let request = RequestBuilder::new("eth_blockNumber")
///     .with_id(1)
///     .build();
///
/// assert!(request.contains("eth_blockNumber"));
///
/// let request_with_params = RequestBuilder::new("eth_getBalance")
///     .with_params(json!(["0x742d35Cc6634C0532925a3b844Bc9e7595f", "latest"]))
///     .with_id(2)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct RequestBuilder {
    method: String,
    params: Option<serde_json::Value>,
    id: serde_json::Value,
}

impl RequestBuilder {
    /// Create a new request builder with the given method.
    pub fn new(method: &str) -> Self {
        Self { method: method.to_string(), params: None, id: serde_json::Value::Number(1.into()) }
    }

    /// Set the request parameters.
    #[must_use]
    pub fn with_params(mut self, params: serde_json::Value) -> Self {
        self.params = Some(params);
        self
    }

    /// Set the request ID.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<serde_json::Value>) -> Self {
        self.id = id.into();
        self
    }

    /// Build the request as a JSON string.
    pub fn build(self) -> String {
        let mut obj = serde_json::json!({
            "jsonrpc": "2.0",
            "method": self.method,
            "id": self.id,
        });

        if let Some(params) = self.params {
            obj["params"] = params;
        }

        serde_json::to_string(&obj).expect("Failed to serialize request")
    }

    /// Build the request as bytes.
    pub fn build_bytes(self) -> Vec<u8> {
        self.build().into_bytes()
    }
}

/// Create a batch request from multiple individual request JSON strings.
///
/// # Example
///
/// ```
/// use roxy_test_utils::{batch_request, RequestBuilder};
///
/// let requests = vec![
///     RequestBuilder::new("eth_blockNumber").with_id(1).build(),
///     RequestBuilder::new("eth_chainId").with_id(2).build(),
/// ];
///
/// let batch = batch_request(requests);
/// assert!(batch.starts_with('['));
/// assert!(batch.ends_with(']'));
/// ```
pub fn batch_request(requests: Vec<String>) -> String {
    format!("[{}]", requests.into_iter().collect::<Vec<_>>().join(","))
}

// ============================================================================
// Response Builder
// ============================================================================

/// Error structure for JSON-RPC test responses.
#[derive(Debug, Clone)]
pub struct JsonRpcTestError {
    /// Error code.
    pub code: i64,
    /// Error message.
    pub message: String,
    /// Optional additional data.
    pub data: Option<serde_json::Value>,
}

/// Builder for JSON-RPC test responses.
///
/// # Example
///
/// ```
/// use roxy_test_utils::ResponseBuilder;
/// use serde_json::json;
///
/// // Success response
/// let response = ResponseBuilder::success(1, json!("0x1234"))
///     .build();
/// assert!(response.contains("result"));
///
/// // Error response
/// let error_response = ResponseBuilder::error(1, -32600, "Invalid Request")
///     .build();
/// assert!(error_response.contains("error"));
/// ```
#[derive(Debug, Clone)]
pub struct ResponseBuilder {
    id: serde_json::Value,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcTestError>,
}

impl ResponseBuilder {
    /// Create a success response builder.
    pub fn success(id: impl Into<serde_json::Value>, result: serde_json::Value) -> Self {
        Self { id: id.into(), result: Some(result), error: None }
    }

    /// Create an error response builder.
    pub fn error(id: impl Into<serde_json::Value>, code: i64, message: &str) -> Self {
        Self {
            id: id.into(),
            result: None,
            error: Some(JsonRpcTestError { code, message: message.to_string(), data: None }),
        }
    }

    /// Add error data.
    #[must_use]
    pub fn with_error_data(mut self, data: serde_json::Value) -> Self {
        if let Some(ref mut err) = self.error {
            err.data = Some(data);
        }
        self
    }

    /// Build the response as a JSON string.
    pub fn build(self) -> String {
        let mut obj = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.id,
        });

        if let Some(result) = self.result {
            obj["result"] = result;
        }

        if let Some(error) = self.error {
            let mut err_obj = serde_json::json!({
                "code": error.code,
                "message": error.message,
            });
            if let Some(data) = error.data {
                err_obj["data"] = data;
            }
            obj["error"] = err_obj;
        }

        serde_json::to_string(&obj).expect("Failed to serialize response")
    }

    /// Build the response as bytes.
    pub fn build_bytes(self) -> Vec<u8> {
        self.build().into_bytes()
    }
}

// ============================================================================
// Fixtures
// ============================================================================

/// Common test fixtures for JSON-RPC requests and responses.
pub mod fixtures {
    use serde_json::json;

    /// Sample `eth_blockNumber` request.
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_test_utils::fixtures;
    ///
    /// let request = fixtures::block_number_request();
    /// assert!(request.contains("eth_blockNumber"));
    /// ```
    pub fn block_number_request() -> String {
        json!({
            "jsonrpc": "2.0",
            "method": "eth_blockNumber",
            "params": [],
            "id": 1
        })
        .to_string()
    }

    /// Sample `eth_getBalance` request.
    ///
    /// # Arguments
    ///
    /// * `address` - The Ethereum address (with or without 0x prefix)
    /// * `block` - Block parameter (e.g., "latest", "pending", or block number)
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_test_utils::fixtures;
    ///
    /// let request = fixtures::get_balance_request(
    ///     "0x742d35Cc6634C0532925a3b844Bc9e7595f",
    ///     "latest"
    /// );
    /// assert!(request.contains("eth_getBalance"));
    /// ```
    pub fn get_balance_request(address: &str, block: &str) -> String {
        json!({
            "jsonrpc": "2.0",
            "method": "eth_getBalance",
            "params": [address, block],
            "id": 1
        })
        .to_string()
    }

    /// Sample `eth_call` request.
    ///
    /// # Arguments
    ///
    /// * `to` - Contract address
    /// * `data` - Call data (hex encoded)
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_test_utils::fixtures;
    ///
    /// let request = fixtures::eth_call_request(
    ///     "0xContractAddress",
    ///     "0x70a08231"  // balanceOf selector
    /// );
    /// assert!(request.contains("eth_call"));
    /// ```
    pub fn eth_call_request(to: &str, data: &str) -> String {
        json!({
            "jsonrpc": "2.0",
            "method": "eth_call",
            "params": [{"to": to, "data": data}, "latest"],
            "id": 1
        })
        .to_string()
    }

    /// Sample `eth_sendRawTransaction` request.
    ///
    /// # Arguments
    ///
    /// * `raw_tx` - The signed transaction data (hex encoded)
    pub fn send_raw_transaction_request(raw_tx: &str) -> String {
        json!({
            "jsonrpc": "2.0",
            "method": "eth_sendRawTransaction",
            "params": [raw_tx],
            "id": 1
        })
        .to_string()
    }

    /// Sample `eth_chainId` request.
    pub fn chain_id_request() -> String {
        json!({
            "jsonrpc": "2.0",
            "method": "eth_chainId",
            "params": [],
            "id": 1
        })
        .to_string()
    }

    /// Sample batch request containing multiple RPC calls.
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_test_utils::fixtures;
    ///
    /// let batch = fixtures::sample_batch_request();
    /// assert!(batch.starts_with('['));
    /// ```
    pub fn sample_batch_request() -> String {
        json!([
            {
                "jsonrpc": "2.0",
                "method": "eth_blockNumber",
                "params": [],
                "id": 1
            },
            {
                "jsonrpc": "2.0",
                "method": "eth_chainId",
                "params": [],
                "id": 2
            },
            {
                "jsonrpc": "2.0",
                "method": "net_version",
                "params": [],
                "id": 3
            }
        ])
        .to_string()
    }

    /// Sample successful response.
    ///
    /// # Arguments
    ///
    /// * `id` - Request ID
    /// * `result` - Result value (hex string typically)
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_test_utils::fixtures;
    ///
    /// let response = fixtures::success_response(1, "0x1234");
    /// assert!(response.contains("result"));
    /// ```
    pub fn success_response(id: u64, result: &str) -> String {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        })
        .to_string()
    }

    /// Sample error response.
    ///
    /// # Arguments
    ///
    /// * `id` - Request ID
    /// * `code` - Error code
    /// * `message` - Error message
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_test_utils::fixtures;
    ///
    /// let response = fixtures::error_response(1, -32600, "Invalid Request");
    /// assert!(response.contains("error"));
    /// ```
    pub fn error_response(id: u64, code: i64, message: &str) -> String {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message
            }
        })
        .to_string()
    }

    /// Sample block number result (hex encoded).
    pub fn block_number_result(block: u64) -> String {
        format!("0x{block:x}")
    }

    /// Sample balance result (hex encoded wei).
    pub fn balance_result(wei: u64) -> String {
        format!("0x{wei:x}")
    }
}

// ============================================================================
// Async Test Helpers
// ============================================================================

/// Run a future with a timeout.
///
/// # Arguments
///
/// * `future` - The future to execute
/// * `timeout` - Maximum time to wait
///
/// # Returns
///
/// `Ok(T)` if the future completes in time, `Err("timeout")` otherwise.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use roxy_test_utils::with_timeout;
///
/// #[tokio::main]
/// async fn main() {
///     let result = with_timeout(
///         async { 42 },
///         Duration::from_secs(1)
///     ).await;
///
///     assert_eq!(result, Ok(42));
/// }
/// ```
pub async fn with_timeout<F, T>(future: F, timeout: Duration) -> Result<T, &'static str>
where
    F: std::future::Future<Output = T>,
{
    tokio::time::timeout(timeout, future).await.map_err(|_| "timeout")
}

/// Assert that a future completes within a timeout.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use roxy_test_utils::assert_completes;
///
/// #[tokio::main]
/// async fn main() {
///     let result = assert_completes!(async { 42 }, Duration::from_secs(1));
///     assert_eq!(result, 42);
/// }
/// ```
#[macro_export]
macro_rules! assert_completes {
    ($future:expr, $timeout:expr) => {
        $crate::with_timeout($future, $timeout).await.expect("future did not complete in time")
    };
}

/// Assert that a future times out.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use roxy_test_utils::assert_times_out;
///
/// #[tokio::main]
/// async fn main() {
///     assert_times_out!(
///         async {
///             tokio::time::sleep(Duration::from_secs(10)).await;
///         },
///         Duration::from_millis(10)
///     );
/// }
/// ```
#[macro_export]
macro_rules! assert_times_out {
    ($future:expr, $timeout:expr) => {
        match $crate::with_timeout($future, $timeout).await {
            Err(_) => {}
            Ok(_) => panic!("expected future to time out, but it completed"),
        }
    };
}

// ============================================================================
// Test Configuration Builder
// ============================================================================

/// Backend configuration for test config builder.
#[derive(Debug, Clone)]
pub struct TestBackendConfig {
    /// Backend name.
    pub name: String,
    /// Backend URL.
    pub url: String,
    /// Weight for load balancing.
    pub weight: Option<u32>,
}

/// Builder for test configuration files.
///
/// Generates TOML configuration strings suitable for testing.
///
/// # Example
///
/// ```
/// use roxy_test_utils::TestConfigBuilder;
///
/// let config = TestConfigBuilder::new()
///     .with_backend("primary", "http://localhost:8545")
///     .with_backend("fallback", "http://localhost:8546")
///     .with_cache(true)
///     .with_rate_limit(100)
///     .build_toml();
///
/// assert!(config.contains("primary"));
/// assert!(config.contains("cache"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct TestConfigBuilder {
    backends: Vec<TestBackendConfig>,
    cache_enabled: bool,
    cache_ttl_seconds: Option<u64>,
    rate_limit_rps: Option<u64>,
    bind_address: Option<String>,
    bind_port: Option<u16>,
}

impl TestConfigBuilder {
    /// Create a new test configuration builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a backend to the configuration.
    #[must_use]
    pub fn with_backend(mut self, name: &str, url: &str) -> Self {
        self.backends.push(TestBackendConfig {
            name: name.to_string(),
            url: url.to_string(),
            weight: None,
        });
        self
    }

    /// Add a backend with a specific weight.
    #[must_use]
    pub fn with_weighted_backend(mut self, name: &str, url: &str, weight: u32) -> Self {
        self.backends.push(TestBackendConfig {
            name: name.to_string(),
            url: url.to_string(),
            weight: Some(weight),
        });
        self
    }

    /// Enable or disable caching.
    #[must_use]
    pub const fn with_cache(mut self, enabled: bool) -> Self {
        self.cache_enabled = enabled;
        self
    }

    /// Set cache TTL in seconds.
    #[must_use]
    pub const fn with_cache_ttl(mut self, ttl_seconds: u64) -> Self {
        self.cache_ttl_seconds = Some(ttl_seconds);
        self
    }

    /// Set rate limit in requests per second.
    #[must_use]
    pub const fn with_rate_limit(mut self, rps: u64) -> Self {
        self.rate_limit_rps = Some(rps);
        self
    }

    /// Set bind address.
    #[must_use]
    pub fn with_bind_address(mut self, address: &str) -> Self {
        self.bind_address = Some(address.to_string());
        self
    }

    /// Set bind port.
    #[must_use]
    pub const fn with_bind_port(mut self, port: u16) -> Self {
        self.bind_port = Some(port);
        self
    }

    /// Build the configuration as a TOML string.
    pub fn build_toml(self) -> String {
        let mut toml = String::new();

        // Server section
        toml.push_str("[server]\n");
        toml.push_str(&format!(
            "bind = \"{}\"\n",
            self.bind_address.as_deref().unwrap_or("127.0.0.1")
        ));
        toml.push_str(&format!("port = {}\n", self.bind_port.unwrap_or(8080)));
        toml.push('\n');

        // Backends section
        for backend in &self.backends {
            toml.push_str("[[backends]]\n");
            toml.push_str(&format!("name = \"{}\"\n", backend.name));
            toml.push_str(&format!("url = \"{}\"\n", backend.url));
            if let Some(weight) = backend.weight {
                toml.push_str(&format!("weight = {weight}\n"));
            }
            toml.push('\n');
        }

        // Cache section
        if self.cache_enabled {
            toml.push_str("[cache]\n");
            toml.push_str("enabled = true\n");
            if let Some(ttl) = self.cache_ttl_seconds {
                toml.push_str(&format!("ttl_seconds = {ttl}\n"));
            }
            toml.push('\n');
        }

        // Rate limit section
        if let Some(rps) = self.rate_limit_rps {
            toml.push_str("[rate_limit]\n");
            toml.push_str("enabled = true\n");
            toml.push_str(&format!("requests_per_second = {rps}\n"));
            toml.push('\n');
        }

        toml
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    mod mock_backend {
        use super::*;

        #[test]
        fn test_new_backend() {
            let backend = MockBackend::new("test");
            assert_eq!(backend.name(), "test");
            assert_eq!(backend.call_count(), 0);
            assert!(matches!(backend.health_status(), HealthStatus::Healthy));
        }

        #[test]
        fn test_with_health() {
            let backend =
                MockBackend::new("test").with_health(HealthStatus::Unhealthy { error_rate: 0.5 });
            assert!(matches!(
                backend.health_status(),
                HealthStatus::Unhealthy { error_rate } if (error_rate - 0.5).abs() < f64::EPSILON
            ));
        }

        #[test]
        fn test_with_latency() {
            let backend = MockBackend::new("test").with_latency(Duration::from_millis(100));
            assert_eq!(backend.latency, Duration::from_millis(100));
        }

        #[tokio::test]
        async fn test_forward_counts_calls() {
            let response = fixtures::success_response(1, "0x1");
            let backend =
                MockBackend::new("test").with_response(MockResponse::Success(response.clone()));

            let request = create_test_request_packet("eth_blockNumber");
            let _ = backend.forward(request).await;

            assert_eq!(backend.call_count(), 1);
        }

        #[tokio::test]
        async fn test_forward_error_response() {
            let backend =
                MockBackend::new("test").with_response(MockResponse::Error("test error".into()));

            let request = create_test_request_packet("eth_blockNumber");
            let result = backend.forward(request).await;

            assert!(result.is_err());
        }

        /// Helper to create a test request packet
        fn create_test_request_packet(method: &'static str) -> RequestPacket {
            use alloy_json_rpc::{Id, Request};
            let req: Request<()> = Request::new(method, Id::Number(1), ());
            RequestPacket::Single(req.serialize().unwrap())
        }
    }

    mod request_builder {
        use super::*;

        #[test]
        fn test_basic_request() {
            let request = RequestBuilder::new("eth_blockNumber").with_id(1).build();

            let parsed: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(parsed["jsonrpc"], "2.0");
            assert_eq!(parsed["method"], "eth_blockNumber");
            assert_eq!(parsed["id"], 1);
        }

        #[test]
        fn test_request_with_params() {
            let request = RequestBuilder::new("eth_getBalance")
                .with_params(serde_json::json!(["0x1234", "latest"]))
                .with_id(2)
                .build();

            let parsed: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(parsed["params"][0], "0x1234");
            assert_eq!(parsed["params"][1], "latest");
        }

        #[test]
        fn test_build_bytes() {
            let request = RequestBuilder::new("eth_blockNumber").build_bytes();
            assert!(!request.is_empty());
        }
    }

    mod response_builder {
        use super::*;

        #[test]
        fn test_success_response() {
            let response = ResponseBuilder::success(1, serde_json::json!("0x1234")).build();

            let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(parsed["jsonrpc"], "2.0");
            assert_eq!(parsed["id"], 1);
            assert_eq!(parsed["result"], "0x1234");
            assert!(parsed.get("error").is_none());
        }

        #[test]
        fn test_error_response() {
            let response = ResponseBuilder::error(1, -32600, "Invalid Request").build();

            let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(parsed["jsonrpc"], "2.0");
            assert_eq!(parsed["id"], 1);
            assert_eq!(parsed["error"]["code"], -32600);
            assert_eq!(parsed["error"]["message"], "Invalid Request");
        }

        #[test]
        fn test_error_with_data() {
            let response = ResponseBuilder::error(1, -32600, "Invalid Request")
                .with_error_data(serde_json::json!({"details": "test"}))
                .build();

            let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(parsed["error"]["data"]["details"], "test");
        }
    }

    mod batch_request_tests {
        use super::*;

        #[test]
        fn test_batch_request() {
            let requests = vec![
                RequestBuilder::new("eth_blockNumber").with_id(1).build(),
                RequestBuilder::new("eth_chainId").with_id(2).build(),
            ];

            let batch = batch_request(requests);
            let parsed: Vec<serde_json::Value> = serde_json::from_str(&batch).unwrap();

            assert_eq!(parsed.len(), 2);
            assert_eq!(parsed[0]["method"], "eth_blockNumber");
            assert_eq!(parsed[1]["method"], "eth_chainId");
        }
    }

    mod fixtures_tests {
        use super::*;

        #[test]
        fn test_block_number_request() {
            let request = fixtures::block_number_request();
            let parsed: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(parsed["method"], "eth_blockNumber");
        }

        #[test]
        fn test_get_balance_request() {
            let request = fixtures::get_balance_request("0x1234", "latest");
            let parsed: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(parsed["method"], "eth_getBalance");
            assert_eq!(parsed["params"][0], "0x1234");
            assert_eq!(parsed["params"][1], "latest");
        }

        #[test]
        fn test_eth_call_request() {
            let request = fixtures::eth_call_request("0xContract", "0x70a08231");
            let parsed: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(parsed["method"], "eth_call");
            assert_eq!(parsed["params"][0]["to"], "0xContract");
            assert_eq!(parsed["params"][0]["data"], "0x70a08231");
        }

        #[test]
        fn test_sample_batch_request() {
            let batch = fixtures::sample_batch_request();
            let parsed: Vec<serde_json::Value> = serde_json::from_str(&batch).unwrap();
            assert_eq!(parsed.len(), 3);
        }

        #[rstest]
        #[case(1, "0x1234")]
        #[case(42, "0xabcd")]
        fn test_success_response(#[case] id: u64, #[case] result: &str) {
            let response = fixtures::success_response(id, result);
            let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(parsed["id"], id);
            assert_eq!(parsed["result"], result);
        }

        #[rstest]
        #[case(1, -32600, "Invalid Request")]
        #[case(2, -32601, "Method not found")]
        fn test_error_response(#[case] id: u64, #[case] code: i64, #[case] message: &str) {
            let response = fixtures::error_response(id, code, message);
            let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(parsed["id"], id);
            assert_eq!(parsed["error"]["code"], code);
            assert_eq!(parsed["error"]["message"], message);
        }

        #[test]
        fn test_block_number_result() {
            assert_eq!(fixtures::block_number_result(255), "0xff");
            assert_eq!(fixtures::block_number_result(4096), "0x1000");
        }
    }

    mod async_helpers {
        use super::*;

        #[tokio::test]
        async fn test_with_timeout_success() {
            let result = with_timeout(async { 42 }, Duration::from_secs(1)).await;
            assert_eq!(result, Ok(42));
        }

        #[tokio::test]
        async fn test_with_timeout_timeout() {
            let result = with_timeout(
                async {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    42
                },
                Duration::from_millis(10),
            )
            .await;
            assert_eq!(result, Err("timeout"));
        }

        #[tokio::test]
        async fn test_assert_completes_macro() {
            let result = assert_completes!(async { 42 }, Duration::from_secs(1));
            assert_eq!(result, 42);
        }

        #[tokio::test]
        async fn test_assert_times_out_macro() {
            assert_times_out!(
                async {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                },
                Duration::from_millis(10)
            );
        }
    }

    mod test_config_builder {
        use super::*;

        #[test]
        fn test_empty_config() {
            let config = TestConfigBuilder::new().build_toml();
            assert!(config.contains("[server]"));
            assert!(config.contains("bind = \"127.0.0.1\""));
            assert!(config.contains("port = 8080"));
        }

        #[test]
        fn test_with_backend() {
            let config = TestConfigBuilder::new()
                .with_backend("primary", "http://localhost:8545")
                .build_toml();

            assert!(config.contains("[[backends]]"));
            assert!(config.contains("name = \"primary\""));
            assert!(config.contains("url = \"http://localhost:8545\""));
        }

        #[test]
        fn test_with_weighted_backend() {
            let config = TestConfigBuilder::new()
                .with_weighted_backend("primary", "http://localhost:8545", 10)
                .build_toml();

            assert!(config.contains("weight = 10"));
        }

        #[test]
        fn test_with_cache() {
            let config = TestConfigBuilder::new().with_cache(true).with_cache_ttl(300).build_toml();

            assert!(config.contains("[cache]"));
            assert!(config.contains("enabled = true"));
            assert!(config.contains("ttl_seconds = 300"));
        }

        #[test]
        fn test_with_rate_limit() {
            let config = TestConfigBuilder::new().with_rate_limit(100).build_toml();

            assert!(config.contains("[rate_limit]"));
            assert!(config.contains("enabled = true"));
            assert!(config.contains("requests_per_second = 100"));
        }

        #[test]
        fn test_full_config() {
            let config = TestConfigBuilder::new()
                .with_bind_address("0.0.0.0")
                .with_bind_port(3000)
                .with_backend("primary", "http://localhost:8545")
                .with_backend("fallback", "http://localhost:8546")
                .with_cache(true)
                .with_rate_limit(50)
                .build_toml();

            assert!(config.contains("bind = \"0.0.0.0\""));
            assert!(config.contains("port = 3000"));
            assert!(config.contains("primary"));
            assert!(config.contains("fallback"));
            assert!(config.contains("[cache]"));
            assert!(config.contains("[rate_limit]"));
        }
    }
}
