//! Error types for the HTTP server.
//!
//! This module provides error types for the Roxy HTTP server that can be
//! converted into proper JSON-RPC error responses.

use std::time::Duration;

use axum::{
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use roxy_rpc::{JsonRpcError, ParsedResponse, ParsedResponsePacket, RpcCodec};
use roxy_traits::DefaultCodecConfig;

/// Server errors that can occur during request processing.
#[derive(Debug)]
pub enum ServerError {
    /// Request was rate limited.
    RateLimited {
        /// Duration to wait before retrying.
        retry_after: Duration,
    },
    /// Request was invalid (parse error, etc.).
    InvalidRequest(String),
    /// Backend returned an error.
    BackendError(String),
    /// Method is blocked or not allowed.
    MethodBlocked(String),
    /// Validation failed.
    ValidationFailed {
        /// JSON-RPC error code.
        code: i64,
        /// Error message.
        message: String,
    },
    /// No healthy backends available.
    NoHealthyBackends,
    /// Internal server error.
    InternalError(String),
}

impl ServerError {
    /// Convert the error to a JSON-RPC error.
    pub fn to_json_rpc_error(&self) -> JsonRpcError {
        match self {
            Self::RateLimited { retry_after } => {
                JsonRpcError::new(-32016, format!("rate limited, retry after {:?}", retry_after))
            }
            Self::InvalidRequest(msg) => JsonRpcError::new(-32600, msg.clone()),
            Self::BackendError(msg) => JsonRpcError::new(-32010, msg.clone()),
            Self::MethodBlocked(method) => {
                JsonRpcError::new(-32601, format!("method '{}' is not allowed", method))
            }
            Self::ValidationFailed { code, message } => JsonRpcError::new(*code, message.clone()),
            Self::NoHealthyBackends => JsonRpcError::new(-32010, "no healthy backends"),
            Self::InternalError(msg) => JsonRpcError::new(-32603, msg.clone()),
        }
    }

    /// Create a rate limited error response with retry-after header.
    #[must_use]
    pub const fn rate_limited(retry_after: Duration) -> Self {
        Self::RateLimited { retry_after }
    }

    /// Create an invalid request error.
    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::InvalidRequest(message.into())
    }

    /// Create a backend error.
    #[must_use]
    pub fn backend_error(message: impl Into<String>) -> Self {
        Self::BackendError(message.into())
    }

    /// Create a method blocked error.
    #[must_use]
    pub fn method_blocked(method: impl Into<String>) -> Self {
        Self::MethodBlocked(method.into())
    }

    /// Create a validation failed error.
    #[must_use]
    pub fn validation_failed(code: i64, message: impl Into<String>) -> Self {
        Self::ValidationFailed { code, message: message.into() }
    }

    /// Create an internal error.
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::InternalError(message.into())
    }
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited { retry_after } => {
                write!(f, "rate limited, retry after {:?}", retry_after)
            }
            Self::InvalidRequest(msg) => write!(f, "invalid request: {}", msg),
            Self::BackendError(msg) => write!(f, "backend error: {}", msg),
            Self::MethodBlocked(method) => write!(f, "method blocked: {}", method),
            Self::ValidationFailed { code, message } => {
                write!(f, "validation failed ({}): {}", code, message)
            }
            Self::NoHealthyBackends => write!(f, "no healthy backends"),
            Self::InternalError(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

impl std::error::Error for ServerError {}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        let json_rpc_error = self.to_json_rpc_error();

        // Create a JSON-RPC error response with null ID (since we don't have the request ID)
        let response = ParsedResponse::error(serde_json::Value::Null, json_rpc_error);
        let packet = ParsedResponsePacket::Single(response);

        let codec = RpcCodec::new(DefaultCodecConfig::new());
        let body = codec.encode_response(&packet).unwrap_or_else(|_| {
            Bytes::from_static(
                br#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal error"},"id":null}"#,
            )
        });

        let status = match &self {
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            Self::MethodBlocked(_) | Self::ValidationFailed { .. } => StatusCode::FORBIDDEN,
            Self::NoHealthyBackends | Self::BackendError(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let mut response = (status, body).into_response();

        // Add retry-after header for rate limited responses
        if let Self::RateLimited { retry_after } = &self
            && let Ok(value) = HeaderValue::from_str(&retry_after.as_secs().to_string())
        {
            response.headers_mut().insert("Retry-After", value);
        }

        // Always set content-type to JSON
        response.headers_mut().insert("Content-Type", HeaderValue::from_static("application/json"));

        response
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn test_rate_limited_error() {
        let err = ServerError::rate_limited(Duration::from_secs(30));
        let json_err = err.to_json_rpc_error();
        assert_eq!(json_err.code, -32016);
        assert!(json_err.message.contains("30"));
    }

    #[test]
    fn test_invalid_request_error() {
        let err = ServerError::invalid_request("malformed JSON");
        let json_err = err.to_json_rpc_error();
        assert_eq!(json_err.code, -32600);
        assert!(json_err.message.contains("malformed JSON"));
    }

    #[test]
    fn test_backend_error() {
        let err = ServerError::backend_error("connection refused");
        let json_err = err.to_json_rpc_error();
        assert_eq!(json_err.code, -32010);
        assert!(json_err.message.contains("connection refused"));
    }

    #[test]
    fn test_method_blocked_error() {
        let err = ServerError::method_blocked("admin_addPeer");
        let json_err = err.to_json_rpc_error();
        assert_eq!(json_err.code, -32601);
        assert!(json_err.message.contains("admin_addPeer"));
    }

    #[test]
    fn test_validation_failed_error() {
        let err = ServerError::validation_failed(-32602, "invalid params");
        let json_err = err.to_json_rpc_error();
        assert_eq!(json_err.code, -32602);
        assert!(json_err.message.contains("invalid params"));
    }

    #[test]
    fn test_no_healthy_backends_error() {
        let err = ServerError::NoHealthyBackends;
        let json_err = err.to_json_rpc_error();
        assert_eq!(json_err.code, -32010);
        assert!(json_err.message.contains("no healthy backends"));
    }

    #[test]
    fn test_internal_error() {
        let err = ServerError::internal("unexpected state");
        let json_err = err.to_json_rpc_error();
        assert_eq!(json_err.code, -32603);
        assert!(json_err.message.contains("unexpected state"));
    }

    #[rstest]
    #[case::rate_limited(ServerError::rate_limited(Duration::from_secs(5)), "rate limited")]
    #[case::invalid_request(ServerError::invalid_request("bad"), "invalid request")]
    #[case::backend_error(ServerError::backend_error("offline"), "backend error")]
    #[case::method_blocked(ServerError::method_blocked("admin"), "method blocked")]
    #[case::validation(ServerError::validation_failed(-32602, "error"), "validation failed")]
    #[case::no_backends(ServerError::NoHealthyBackends, "no healthy backends")]
    #[case::internal(ServerError::internal("panic"), "internal error")]
    fn test_error_display(#[case] err: ServerError, #[case] expected: &str) {
        let display = err.to_string();
        assert!(display.contains(expected), "Expected '{}' to contain '{}'", display, expected);
    }

    #[test]
    fn test_error_debug() {
        let err = ServerError::internal("test");
        let debug = format!("{:?}", err);
        assert!(debug.contains("InternalError"));
    }
}
