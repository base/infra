//! Error types for the Roxy proxy.

use std::time::Duration;

use alloy_json_rpc::ErrorPayload;
use derive_more::{Debug, Display, Error};

/// Custom error codes for Roxy proxy.
pub mod error_codes {
    /// Rate limited error code.
    pub const RATE_LIMITED: i64 = -32016;
    /// Backend offline error code.
    pub const BACKEND_OFFLINE: i64 = -32010;
}

/// Error type for the Roxy proxy.
#[derive(Debug, Display, Error)]
#[error(ignore)]
pub enum RoxyError {
    /// Rate limited error.
    #[display("rate limited, retry after {retry_after:?}")]
    RateLimited {
        /// Duration to wait before retrying.
        retry_after: Duration,
    },

    /// Backend offline error.
    #[display("backend offline: {backend}")]
    BackendOffline {
        /// Name of the offline backend.
        backend: String,
    },

    /// No healthy backends available.
    #[display("no healthy backends")]
    NoHealthyBackends,

    /// Backend timeout error.
    #[display("backend timeout: {backend}")]
    BackendTimeout {
        /// Name of the timed out backend.
        backend: String,
    },

    /// Cache error.
    #[display("cache error: {_0}")]
    CacheError(String),

    /// Internal error.
    #[display("internal error: {_0}")]
    Internal(String),
}

impl RoxyError {
    /// Convert to an alloy ErrorPayload for JSON-RPC responses.
    #[must_use]
    pub fn to_error_payload(&self) -> ErrorPayload {
        match self {
            Self::RateLimited { retry_after } => ErrorPayload {
                code: error_codes::RATE_LIMITED,
                message: format!("rate limited, retry after {:?}", retry_after).into(),
                data: None,
            },
            Self::BackendOffline { backend } => ErrorPayload {
                code: error_codes::BACKEND_OFFLINE,
                message: format!("backend {} is offline", backend).into(),
                data: None,
            },
            Self::NoHealthyBackends => ErrorPayload {
                code: error_codes::BACKEND_OFFLINE,
                message: "no healthy backends".into(),
                data: None,
            },
            _ => ErrorPayload::internal_error(),
        }
    }

    /// Whether this error should trigger failover to next backend.
    #[must_use]
    pub const fn should_failover(&self) -> bool {
        matches!(self, Self::BackendOffline { .. } | Self::BackendTimeout { .. })
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Test that should_failover returns the expected value for each error type.
    #[rstest]
    #[case::rate_limited(RoxyError::RateLimited { retry_after: Duration::from_secs(5) }, false)]
    #[case::backend_offline(RoxyError::BackendOffline { backend: "primary".to_string() }, true)]
    #[case::backend_timeout(RoxyError::BackendTimeout { backend: "secondary".to_string() }, true)]
    #[case::no_healthy_backends(RoxyError::NoHealthyBackends, false)]
    #[case::cache_error(RoxyError::CacheError("connection failed".to_string()), false)]
    #[case::internal_error(RoxyError::Internal("unexpected state".to_string()), false)]
    fn test_should_failover(#[case] error: RoxyError, #[case] expected: bool) {
        assert_eq!(error.should_failover(), expected);
    }

    /// Test that to_error_payload returns the expected error code for each error type.
    #[rstest]
    #[case::rate_limited(RoxyError::RateLimited { retry_after: Duration::from_secs(5) }, error_codes::RATE_LIMITED)]
    #[case::backend_offline(RoxyError::BackendOffline { backend: "primary".to_string() }, error_codes::BACKEND_OFFLINE)]
    #[case::no_healthy_backends(RoxyError::NoHealthyBackends, error_codes::BACKEND_OFFLINE)]
    #[case::cache_error(RoxyError::CacheError("connection failed".to_string()), -32603)]
    #[case::internal_error(RoxyError::Internal("unexpected state".to_string()), -32603)]
    fn test_error_payload_code(#[case] error: RoxyError, #[case] expected_code: i64) {
        let payload = error.to_error_payload();
        assert_eq!(payload.code, expected_code);
    }

    /// Test that error Display messages contain expected substrings.
    #[rstest]
    #[case::rate_limited(RoxyError::RateLimited { retry_after: Duration::from_secs(5) }, "rate limited")]
    #[case::backend_offline(RoxyError::BackendOffline { backend: "primary".to_string() }, "primary")]
    #[case::backend_timeout(RoxyError::BackendTimeout { backend: "secondary".to_string() }, "secondary")]
    #[case::no_healthy_backends(RoxyError::NoHealthyBackends, "no healthy backends")]
    #[case::cache_error(RoxyError::CacheError("connection failed".to_string()), "connection failed")]
    #[case::internal_error(RoxyError::Internal("unexpected state".to_string()), "unexpected state")]
    fn test_error_display(#[case] error: RoxyError, #[case] expected_substring: &str) {
        assert!(
            error.to_string().contains(expected_substring),
            "Expected '{}' to contain '{}'",
            error,
            expected_substring
        );
    }

    /// Test that backend offline error payload message contains the backend name.
    #[rstest]
    #[case::primary("primary")]
    #[case::secondary("secondary")]
    #[case::node_1("node-1")]
    fn test_backend_offline_payload_contains_name(#[case] backend_name: &str) {
        let err = RoxyError::BackendOffline { backend: backend_name.to_string() };
        let payload = err.to_error_payload();
        assert!(
            payload.message.contains(backend_name),
            "Expected payload message '{}' to contain '{}'",
            payload.message,
            backend_name
        );
    }
}
