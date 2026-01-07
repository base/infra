//! Request validation and rate limiting utilities.
//!
//! This module provides factory types for creating validator chains and rate limiters
//! from configuration.

use std::time::Duration;

use roxy_config::RoxyConfig;
use roxy_rpc::{MethodBlocklist, RateLimiterConfig, SlidingWindowRateLimiter, ValidatorChain};

/// Factory for creating validator chains from configuration.
///
/// # Example
///
/// ```ignore
/// use roxyproxy_cli::ValidatorFactory;
///
/// let validators = ValidatorFactory::new().create(&config);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidatorFactory;

impl ValidatorFactory {
    /// Create a new validator factory.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Create a validator chain from configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - The Roxy configuration
    ///
    /// # Returns
    ///
    /// A configured ValidatorChain.
    pub fn create(&self, config: &RoxyConfig) -> ValidatorChain {
        let mut chain = ValidatorChain::new();

        // Add method blocklist if there are blocked methods
        if !config.routing.blocked_methods.is_empty() {
            let blocklist = MethodBlocklist::new(config.routing.blocked_methods.clone());
            chain = chain.add(blocklist);
            trace!(
                count = config.routing.blocked_methods.len(),
                "Added method blocklist validator"
            );
        }

        chain
    }
}

/// Factory for creating rate limiters from configuration.
///
/// # Example
///
/// ```ignore
/// use roxyproxy_cli::RateLimiterFactory;
///
/// if let Some(limiter) = RateLimiterFactory::new().create(&config) {
///     // Use the rate limiter
/// }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct RateLimiterFactory;

impl RateLimiterFactory {
    /// Create a new rate limiter factory.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Create a rate limiter from configuration if enabled.
    ///
    /// # Arguments
    ///
    /// * `config` - The Roxy configuration
    ///
    /// # Returns
    ///
    /// An optional SlidingWindowRateLimiter.
    pub fn create(&self, config: &RoxyConfig) -> Option<SlidingWindowRateLimiter> {
        if !config.rate_limit.enabled {
            return None;
        }

        // Convert requests per second to a window-based configuration
        // Use a 1-second window with the configured requests per second as the limit
        let limiter_config =
            RateLimiterConfig::new(config.rate_limit.requests_per_second, Duration::from_secs(1));

        trace!(
            requests_per_second = config.rate_limit.requests_per_second,
            burst_size = config.rate_limit.burst_size,
            "Created rate limiter"
        );

        Some(SlidingWindowRateLimiter::new(limiter_config))
    }
}

/// Create the validator chain from configuration.
///
/// This is a convenience function that creates a [`ValidatorFactory`] and calls
/// [`ValidatorFactory::create`].
///
/// # Arguments
///
/// * `config` - The Roxy configuration
///
/// # Returns
///
/// A configured ValidatorChain.
pub fn create_validators(config: &RoxyConfig) -> ValidatorChain {
    ValidatorFactory::new().create(config)
}

/// Create the rate limiter from configuration if enabled.
///
/// This is a convenience function that creates a [`RateLimiterFactory`] and calls
/// [`RateLimiterFactory::create`].
///
/// # Arguments
///
/// * `config` - The Roxy configuration
///
/// # Returns
///
/// An optional SlidingWindowRateLimiter.
pub fn create_rate_limiter(config: &RoxyConfig) -> Option<SlidingWindowRateLimiter> {
    RateLimiterFactory::new().create(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::minimal_config;

    #[test]
    fn test_create_validators_with_blocklist() {
        let mut config = minimal_config();
        config.routing.blocked_methods = vec!["admin_addPeer".to_string()];

        let validators = create_validators(&config);
        assert_eq!(validators.len(), 1);
    }

    #[test]
    fn test_create_validators_empty() {
        let config = minimal_config();
        let validators = create_validators(&config);
        assert!(validators.is_empty());
    }

    #[test]
    fn test_create_rate_limiter_disabled() {
        let config = minimal_config();
        let limiter = create_rate_limiter(&config);
        assert!(limiter.is_none());
    }

    #[test]
    fn test_create_rate_limiter_enabled() {
        let mut config = minimal_config();
        config.rate_limit.enabled = true;
        config.rate_limit.requests_per_second = 100;

        let limiter = create_rate_limiter(&config);
        assert!(limiter.is_some());
    }

    #[test]
    fn test_validator_factory_default() {
        let factory = ValidatorFactory::default();
        let _ = format!("{:?}", factory);
    }

    #[test]
    fn test_rate_limiter_factory_default() {
        let factory = RateLimiterFactory::default();
        let _ = format!("{:?}", factory);
    }
}
