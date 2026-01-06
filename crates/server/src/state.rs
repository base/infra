//! Server state builder for constructing application state.
//!
//! This module provides a builder pattern for constructing the [`HttpAppState`]
//! used by the HTTP server handlers.

use std::{collections::HashMap, sync::Arc};

use roxy_backend::BackendGroup;
use roxy_rpc::{MethodRouter, RpcCodec, SlidingWindowRateLimiter, Validator, ValidatorChain};
use roxy_traits::{Cache, DefaultCodecConfig};

use crate::http::HttpAppState;

/// Builder for constructing [`HttpAppState`].
///
/// This builder provides a fluent interface for configuring all the components
/// needed by the HTTP server.
///
/// # Example
///
/// ```ignore
/// use roxy_server::ServerBuilder;
/// use roxy_rpc::{RpcCodec, MethodRouter, RouteTarget};
///
/// let state = ServerBuilder::new()
///     .codec(RpcCodec::default())
///     .router(MethodRouter::new().fallback(RouteTarget::group("default")))
///     .add_group("default".to_string(), backend_group)
///     .build()?;
/// ```
#[derive(Default)]
pub struct ServerBuilder<C: Cache = roxy_cache::MemoryCache> {
    codec: Option<RpcCodec>,
    router: Option<MethodRouter>,
    validators: ValidatorChain,
    rate_limiter: Option<Arc<SlidingWindowRateLimiter>>,
    groups: HashMap<String, Arc<BackendGroup>>,
    cache: Option<Arc<C>>,
}

impl ServerBuilder<roxy_cache::MemoryCache> {
    /// Create a new server builder with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            codec: None,
            router: None,
            validators: ValidatorChain::new(),
            rate_limiter: None,
            groups: HashMap::new(),
            cache: None,
        }
    }
}

impl<C: Cache> ServerBuilder<C> {
    /// Set the RPC codec for parsing and encoding JSON-RPC messages.
    ///
    /// If not set, a default codec with standard limits will be used.
    #[must_use]
    pub const fn codec(mut self, codec: RpcCodec) -> Self {
        self.codec = Some(codec);
        self
    }

    /// Set the method router for routing requests to backend groups.
    ///
    /// If not set, all requests will use the default route target.
    #[must_use]
    pub fn router(mut self, router: MethodRouter) -> Self {
        self.router = Some(router);
        self
    }

    /// Set the validator chain for request validation.
    #[must_use]
    pub fn validators(mut self, validators: ValidatorChain) -> Self {
        self.validators = validators;
        self
    }

    /// Add a validator to the validation chain.
    #[must_use]
    pub fn add_validator<V: Validator>(mut self, validator: V) -> Self {
        self.validators = self.validators.add(validator);
        self
    }

    /// Add a pre-built Arc validator to the validation chain.
    #[must_use]
    pub fn add_validator_arc(mut self, validator: Arc<dyn Validator>) -> Self {
        self.validators = self.validators.add_arc(validator);
        self
    }

    /// Set the rate limiter for request throttling.
    #[must_use]
    pub fn rate_limiter(mut self, rate_limiter: Arc<SlidingWindowRateLimiter>) -> Self {
        self.rate_limiter = Some(rate_limiter);
        self
    }

    /// Add a backend group.
    #[must_use]
    pub fn add_group(mut self, name: String, group: Arc<BackendGroup>) -> Self {
        self.groups.insert(name, group);
        self
    }

    /// Set the cache for response caching.
    #[must_use]
    pub fn cache<C2: Cache>(self, cache: Arc<C2>) -> ServerBuilder<C2> {
        ServerBuilder {
            codec: self.codec,
            router: self.router,
            validators: self.validators,
            rate_limiter: self.rate_limiter,
            groups: self.groups,
            cache: Some(cache),
        }
    }

    /// Build the application state.
    ///
    /// # Errors
    ///
    /// Returns an error if required components are missing or misconfigured.
    pub fn build(self) -> eyre::Result<Arc<HttpAppState<C>>> {
        let codec = self.codec.unwrap_or_else(|| RpcCodec::new(DefaultCodecConfig::new()));
        let router = self.router.unwrap_or_default();

        Ok(Arc::new(HttpAppState::new(
            codec,
            router,
            self.validators,
            self.rate_limiter,
            self.groups,
            self.cache,
        )))
    }
}

impl<C: Cache> std::fmt::Debug for ServerBuilder<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerBuilder")
            .field("codec", &self.codec.is_some())
            .field("router", &self.router.is_some())
            .field("validators", &self.validators)
            .field("rate_limiter", &self.rate_limiter.is_some())
            .field("groups", &self.groups.len())
            .field("cache", &self.cache.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use roxy_rpc::{NoopValidator, RateLimiterConfig, RouteTarget};

    use super::*;

    #[test]
    fn test_builder_new() {
        let builder = ServerBuilder::new();
        assert!(builder.codec.is_none());
        assert!(builder.router.is_none());
        assert!(builder.rate_limiter.is_none());
        assert!(builder.groups.is_empty());
        assert!(builder.cache.is_none());
    }

    #[test]
    fn test_builder_with_codec() {
        let builder = ServerBuilder::new().codec(RpcCodec::default());
        assert!(builder.codec.is_some());
    }

    #[test]
    fn test_builder_with_router() {
        let router = MethodRouter::new().fallback(RouteTarget::group("default"));
        let builder = ServerBuilder::new().router(router);
        assert!(builder.router.is_some());
    }

    #[test]
    fn test_builder_with_validators() {
        let chain = ValidatorChain::new().add(NoopValidator::new());
        let builder = ServerBuilder::new().validators(chain);
        assert_eq!(builder.validators.len(), 1);
    }

    #[test]
    fn test_builder_add_validator() {
        let builder = ServerBuilder::new()
            .add_validator(NoopValidator::new())
            .add_validator(NoopValidator::new());
        assert_eq!(builder.validators.len(), 2);
    }

    #[test]
    fn test_builder_with_rate_limiter() {
        let config = RateLimiterConfig::new(100, Duration::from_secs(60));
        let limiter = Arc::new(SlidingWindowRateLimiter::new(config));
        let builder = ServerBuilder::new().rate_limiter(limiter);
        assert!(builder.rate_limiter.is_some());
    }

    #[test]
    fn test_builder_build_minimal() {
        let result = ServerBuilder::new().build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_builder_debug() {
        let builder = ServerBuilder::new().codec(RpcCodec::default());
        let debug = format!("{:?}", builder);
        assert!(debug.contains("ServerBuilder"));
        assert!(debug.contains("codec"));
    }
}
