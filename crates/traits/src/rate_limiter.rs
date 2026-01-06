//! Rate limiting trait for request throttling.

use std::time::Duration;

use derive_more::{Debug, Display, Error};

/// Error returned when a rate limit is exceeded.
#[derive(Debug, Display, Error)]
#[display("rate limited: retry after {retry_after:?}")]
pub struct RateLimitError {
    /// Duration to wait before retrying.
    pub retry_after: Duration,
}

/// Result of checking a rate limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitResult {
    /// Request is allowed.
    Allowed,
    /// Request is rate limited with the given retry duration.
    Limited {
        /// Duration to wait before retrying.
        retry_after: Duration,
    },
}

impl RateLimitResult {
    /// Returns true if the request is allowed.
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// Returns true if the request is rate limited.
    pub const fn is_limited(&self) -> bool {
        matches!(self, Self::Limited { .. })
    }

    /// Returns the retry duration if rate limited.
    pub const fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Allowed => None,
            Self::Limited { retry_after } => Some(*retry_after),
        }
    }
}

/// Rate limiter trait for request throttling.
///
/// Implementations should be keyed by some identifier (e.g., IP address, API key)
/// and track request counts over time windows.
pub trait RateLimiter: Send + Sync + 'static {
    /// Check if a request is allowed for the given key.
    ///
    /// Returns `RateLimitResult::Allowed` if the request should proceed,
    /// or `RateLimitResult::Limited` with retry duration if throttled.
    fn check(&self, key: &str) -> RateLimitResult;

    /// Record a request for the given key.
    ///
    /// This should be called after a request is processed to update counters.
    fn record(&self, key: &str);

    /// Check and record a request atomically.
    ///
    /// Returns the result of the check and records the request if allowed.
    fn check_and_record(&self, key: &str) -> RateLimitResult {
        let result = self.check(key);
        if result.is_allowed() {
            self.record(key);
        }
        result
    }

    /// Reset the rate limit for a specific key.
    fn reset(&self, key: &str);

    /// Get the current request count for a key.
    fn count(&self, key: &str) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_result_allowed() {
        let result = RateLimitResult::Allowed;
        assert!(result.is_allowed());
        assert!(!result.is_limited());
        assert_eq!(result.retry_after(), None);
    }

    #[test]
    fn test_rate_limit_result_limited() {
        let result = RateLimitResult::Limited { retry_after: Duration::from_secs(60) };
        assert!(!result.is_allowed());
        assert!(result.is_limited());
        assert_eq!(result.retry_after(), Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_rate_limit_error_display() {
        let error = RateLimitError { retry_after: Duration::from_secs(30) };
        assert!(error.to_string().contains("30"));
    }
}
