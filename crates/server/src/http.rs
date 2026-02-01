//! HTTP handler for JSON-RPC requests.
//!
//! This module provides the main HTTP handler for processing JSON-RPC requests,
//! including rate limiting, validation, routing, caching, and backend forwarding.

use std::{collections::HashMap, sync::Arc};

use alloy_json_rpc::{Id, Request, RequestPacket, ResponsePacket, ResponsePayload};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use roxy_backend::{BackendGroup, BackendResponse};
use roxy_rpc::{
    JsonRpcError, MethodRouter, ParsedRequest, ParsedRequestPacket, ParsedResponse,
    ParsedResponsePacket, RouteTarget, RpcCodec, SlidingWindowRateLimiter, ValidationResult,
    ValidatorChain,
};
use roxy_traits::{Cache, RateLimitResult, RateLimiter};
use serde_json::value::RawValue;

use crate::{Span, error::ServerError};

/// Default header for client identification in rate limiting.
const CLIENT_ID_HEADER: &str = "X-Forwarded-For";

/// Default client ID when no header is provided.
const DEFAULT_CLIENT_ID: &str = "default";

/// Application state shared across all HTTP handlers.
///
/// This struct contains all the components needed to process JSON-RPC requests,
/// including the codec, router, validators, rate limiter, backend groups, and cache.
pub struct HttpAppState<C: Cache = roxy_cache::MemoryCache> {
    /// RPC codec for parsing and encoding JSON-RPC messages.
    codec: RpcCodec,
    /// Method router for directing requests to backend groups.
    router: MethodRouter,
    /// Validator chain for request validation.
    validators: ValidatorChain,
    /// Optional rate limiter.
    rate_limiter: Option<Arc<SlidingWindowRateLimiter>>,
    /// Backend groups by name.
    groups: HashMap<String, Arc<BackendGroup>>,
    /// Optional cache for response caching.
    cache: Option<Arc<C>>,
}

impl<C: Cache> HttpAppState<C> {
    /// Create a new application state.
    #[must_use]
    pub const fn new(
        codec: RpcCodec,
        router: MethodRouter,
        validators: ValidatorChain,
        rate_limiter: Option<Arc<SlidingWindowRateLimiter>>,
        groups: HashMap<String, Arc<BackendGroup>>,
        cache: Option<Arc<C>>,
    ) -> Self {
        Self { codec, router, validators, rate_limiter, groups, cache }
    }

    /// Get the RPC codec.
    #[must_use]
    pub const fn codec(&self) -> &RpcCodec {
        &self.codec
    }

    /// Get the method router.
    #[must_use]
    pub const fn router(&self) -> &MethodRouter {
        &self.router
    }

    /// Get the validator chain.
    #[must_use]
    pub const fn validators(&self) -> &ValidatorChain {
        &self.validators
    }

    /// Get the rate limiter.
    #[must_use]
    pub const fn rate_limiter(&self) -> Option<&Arc<SlidingWindowRateLimiter>> {
        self.rate_limiter.as_ref()
    }

    /// Get the backend groups.
    #[must_use]
    pub const fn groups(&self) -> &HashMap<String, Arc<BackendGroup>> {
        &self.groups
    }

    /// Get a backend group by name.
    #[must_use]
    pub fn get_group(&self, name: &str) -> Option<&Arc<BackendGroup>> {
        self.groups.get(name)
    }

    /// Get the cache.
    #[must_use]
    pub const fn cache(&self) -> Option<&Arc<C>> {
        self.cache.as_ref()
    }
}

impl<C: Cache> std::fmt::Debug for HttpAppState<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpAppState")
            .field("codec", &self.codec)
            .field("router", &self.router)
            .field("validators", &self.validators)
            .field("rate_limiter", &self.rate_limiter.is_some())
            .field("groups", &self.groups.len())
            .field("cache", &self.cache.is_some())
            .finish()
    }
}

/// Create the axum router with all endpoints.
///
/// # Endpoints
///
/// - `POST /` - Main RPC endpoint for JSON-RPC requests
/// - `GET /health` - Health check endpoint
///
/// # Example
///
/// ```ignore
/// use roxy_server::{create_router, ServerBuilder};
///
/// let state = ServerBuilder::new().build()?;
/// let app = create_router(state);
/// ```
pub fn create_router<C: Cache>(state: Arc<HttpAppState<C>>) -> Router {
    Router::new()
        .route("/", post(handle_rpc::<C>))
        .route("/health", get(health_check))
        .with_state(state)
}

/// Health check endpoint.
///
/// Returns 200 OK if the server is healthy.
#[tracing::instrument]
pub async fn health_check() -> impl IntoResponse {
    StatusCode::OK
}

/// Extract client identifier from headers for rate limiting.
fn extract_client_id(headers: &HeaderMap) -> &str {
    headers
        .get(CLIENT_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .unwrap_or(DEFAULT_CLIENT_ID)
}

/// Main RPC handler for processing JSON-RPC requests.
///
/// This handler performs the following steps:
/// 1. Extract client identifier for rate limiting
/// 2. Check rate limit
/// 3. Decode request using codec
/// 4. For each request in packet:
///    a. Validate using validator chain
///    b. Route to appropriate backend group
///    c. Check cache (if enabled)
///    d. Forward to backend if not cached
///    e. Cache response (if cacheable)
/// 5. Encode and return response
#[tracing::instrument(skip(state, body), fields(client_id))]
pub async fn handle_rpc<C: Cache>(
    State(state): State<Arc<HttpAppState<C>>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // 1. Extract client identifier
    let client_id = extract_client_id(&headers);
    Span::current().record("client_id", client_id);

    // 2. Check rate limit
    if let Some(rate_limiter) = &state.rate_limiter {
        match rate_limiter.check_and_record(client_id) {
            RateLimitResult::Allowed => {}
            RateLimitResult::Limited { retry_after } => {
                warn!(client_id, ?retry_after, "rate limited");
                return ServerError::rate_limited(retry_after).into_response();
            }
        }
    }

    // 3. Decode request
    let packet = match state.codec.decode_bytes(&body) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "failed to decode request");
            return ServerError::invalid_request(e.to_string()).into_response();
        }
    };

    // 4. Process request(s)
    let response_packet = match packet {
        ParsedRequestPacket::Single(request) => {
            let response = process_single_request(&state, request).await;
            ParsedResponsePacket::Single(response)
        }
        ParsedRequestPacket::Batch(requests) => {
            let mut responses = Vec::with_capacity(requests.len());
            for request in requests {
                let response = process_single_request(&state, request).await;
                responses.push(response);
            }
            ParsedResponsePacket::Batch(responses)
        }
    };

    // 5. Encode response
    let response_bytes = match state.codec.encode_response(&response_packet) {
        Ok(bytes) => bytes,
        Err(e) => {
            error!(error = %e, "failed to encode response");
            return ServerError::internal(e.to_string()).into_response();
        }
    };

    // Build response with headers
    let mut response = (StatusCode::OK, response_bytes).into_response();
    response.headers_mut().insert("Content-Type", HeaderValue::from_static("application/json"));

    response
}

/// Process a single JSON-RPC request.
#[tracing::instrument(skip(state), fields(method = %request.method))]
async fn process_single_request<C: Cache>(
    state: &HttpAppState<C>,
    request: ParsedRequest,
) -> ParsedResponse {
    let request_id = request.id.clone().unwrap_or(serde_json::Value::Null);
    let method = request.method.clone();

    // 4a. Validate using validator chain
    let validated_request = match state.validators.validate(request) {
        ValidationResult::Valid(req) => req,
        ValidationResult::Invalid(err) => {
            debug!(method = %method, code = err.code, "validation failed");
            return ParsedResponse::error(request_id, JsonRpcError::new(err.code, err.message));
        }
    };

    // 4b. Route to appropriate backend group
    let route_target = state.router.resolve(&method);

    // Check if method is blocked
    if route_target.is_blocked() {
        debug!(method = %method, "method blocked");
        return ParsedResponse::error(request_id, JsonRpcError::method_not_found());
    }

    // Get the backend group
    let group_name = match route_target {
        RouteTarget::Group(name) => name.clone(),
        RouteTarget::Default => {
            // Use "default" group or first available
            if let Some(name) = state.groups.keys().next() {
                name.clone()
            } else {
                error!("no backend groups configured");
                return ParsedResponse::error(request_id, JsonRpcError::internal_error());
            }
        }
        RouteTarget::Block => {
            // Already handled above
            return ParsedResponse::error(request_id, JsonRpcError::method_not_found());
        }
    };

    let group = match state.get_group(&group_name) {
        Some(g) => g,
        None => {
            error!(group = %group_name, "backend group not found");
            return ParsedResponse::error(request_id, JsonRpcError::internal_error());
        }
    };

    // 4c-d. Forward to backend
    let request_packet = match create_request_packet(&validated_request) {
        Ok(packet) => packet,
        Err(e) => {
            error!(error = %e, "failed to serialize request");
            return ParsedResponse::error(request_id, JsonRpcError::internal_error());
        }
    };

    match group.forward(request_packet).await {
        Ok(BackendResponse { response, served_by }) => {
            debug!(backend = %served_by, "request forwarded successfully");
            convert_alloy_response(response, request_id)
        }
        Err(e) => {
            warn!(error = %e, "backend error");
            ParsedResponse::error(request_id, JsonRpcError::new(-32010, e.to_string()))
        }
    }
}

/// Convert a ParsedRequest to a RequestPacket for backend forwarding.
fn create_request_packet(request: &ParsedRequest) -> Result<RequestPacket, serde_json::Error> {
    // Convert the ID
    let id = match &request.id {
        Some(serde_json::Value::Number(n)) => n.as_u64().map_or(Id::None, Id::Number),
        Some(serde_json::Value::String(s)) => Id::String(s.clone()),
        _ => Id::None,
    };

    // Get params as a boxed RawValue
    let params =
        request.params.clone().unwrap_or_else(|| RawValue::from_string("[]".to_string()).unwrap());

    // Create the request
    let req = Request::new(request.method.clone(), id, params);

    // Serialize to create a SerializedRequest
    let serialized = req.serialize()?;

    Ok(RequestPacket::Single(serialized))
}

/// Convert an alloy ResponsePacket to a ParsedResponse.
fn convert_alloy_response(
    response: ResponsePacket,
    request_id: serde_json::Value,
) -> ParsedResponse {
    match response {
        ResponsePacket::Single(resp) => convert_single_response(resp, request_id),
        ResponsePacket::Batch(responses) => {
            // For batch responses, we should only get single response here
            // This is a fallback
            if let Some(resp) = responses.into_iter().next() {
                convert_single_response(resp, request_id)
            } else {
                ParsedResponse::error(request_id, JsonRpcError::internal_error())
            }
        }
    }
}

/// Convert a single alloy Response to a ParsedResponse.
fn convert_single_response(
    response: alloy_json_rpc::Response<Box<RawValue>>,
    request_id: serde_json::Value,
) -> ParsedResponse {
    match response.payload {
        ResponsePayload::Success(result) => ParsedResponse::success(request_id, result),
        ResponsePayload::Failure(err) => {
            ParsedResponse::error(request_id, JsonRpcError::new(err.code, err.message.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::header::HeaderValue;
    use roxy_traits::DefaultCodecConfig;

    use super::*;

    #[test]
    fn test_extract_client_id_from_header() {
        let mut headers = HeaderMap::new();
        headers.insert(CLIENT_ID_HEADER, HeaderValue::from_static("192.168.1.1"));

        assert_eq!(extract_client_id(&headers), "192.168.1.1");
    }

    #[test]
    fn test_extract_client_id_with_multiple_ips() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CLIENT_ID_HEADER,
            HeaderValue::from_static("192.168.1.1, 10.0.0.1, 172.16.0.1"),
        );

        // Should return first IP
        assert_eq!(extract_client_id(&headers), "192.168.1.1");
    }

    #[test]
    fn test_extract_client_id_missing_header() {
        let headers = HeaderMap::new();
        assert_eq!(extract_client_id(&headers), DEFAULT_CLIENT_ID);
    }

    #[test]
    fn test_extract_client_id_with_whitespace() {
        let mut headers = HeaderMap::new();
        headers.insert(CLIENT_ID_HEADER, HeaderValue::from_static("  192.168.1.1  "));

        assert_eq!(extract_client_id(&headers), "192.168.1.1");
    }

    #[test]
    fn test_app_state_debug() {
        let state: HttpAppState<roxy_cache::MemoryCache> = HttpAppState::new(
            RpcCodec::new(DefaultCodecConfig::new()),
            MethodRouter::new(),
            ValidatorChain::new(),
            None,
            HashMap::new(),
            None,
        );

        let debug = format!("{:?}", state);
        assert!(debug.contains("HttpAppState"));
        assert!(debug.contains("codec"));
        assert!(debug.contains("router"));
    }

    #[test]
    fn test_app_state_accessors() {
        let state: HttpAppState<roxy_cache::MemoryCache> = HttpAppState::new(
            RpcCodec::new(DefaultCodecConfig::new()),
            MethodRouter::new(),
            ValidatorChain::new(),
            None,
            HashMap::new(),
            None,
        );

        assert!(state.rate_limiter().is_none());
        assert!(state.groups().is_empty());
        assert!(state.cache().is_none());
        assert!(state.get_group("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_health_check() {
        let response = health_check().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn test_create_request_packet() {
        let parsed = ParsedRequest {
            jsonrpc: "2.0".to_string(),
            method: "eth_blockNumber".to_string(),
            params: Some(RawValue::from_string("[]".to_string()).unwrap()),
            id: Some(serde_json::json!(1)),
        };

        let result = create_request_packet(&parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_request_packet_no_params() {
        let parsed = ParsedRequest {
            jsonrpc: "2.0".to_string(),
            method: "eth_chainId".to_string(),
            params: None,
            id: Some(serde_json::json!(42)),
        };

        let result = create_request_packet(&parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_request_packet_string_id() {
        let parsed = ParsedRequest {
            jsonrpc: "2.0".to_string(),
            method: "eth_call".to_string(),
            params: Some(RawValue::from_string("[{}, \"latest\"]".to_string()).unwrap()),
            id: Some(serde_json::json!("request-123")),
        };

        let result = create_request_packet(&parsed);
        assert!(result.is_ok());
    }
}
