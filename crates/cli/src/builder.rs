//! Application builder for constructing the Roxy server.
//!
//! This module provides a fluent builder interface for configuring and building
//! the HTTP server router from configuration.

use std::sync::Arc;

use eyre::{Context, Result};
use roxy_cache::MemoryCache;
use roxy_config::RoxyConfig;
use roxy_rpc::RpcCodec;
use roxy_server::{ServerBuilder, create_router};
use roxy_traits::DefaultCodecConfig;

use crate::{
    backends::BackendFactory,
    groups::GroupFactory,
    routing::RouterFactory,
    validators::{RateLimiterFactory, ValidatorFactory},
};

/// Builder for constructing the Roxy application.
///
/// Provides a fluent interface for configuring and building the HTTP server
/// router from configuration, following the same pattern as [`ServerBuilder`].
///
/// # Example
///
/// ```ignore
/// use roxyproxy_cli::AppBuilder;
/// use roxy_config::RoxyConfig;
///
/// let config = RoxyConfig::from_file("roxy.toml")?;
/// let app = AppBuilder::new()
///     .with_batch_size(200)
///     .build(&config)
///     .await?;
/// ```
#[derive(Debug)]
pub struct AppBuilder {
    backend_factory: BackendFactory,
    group_factory: GroupFactory,
    router_factory: RouterFactory,
    validator_factory: ValidatorFactory,
    rate_limiter_factory: RateLimiterFactory,
}

impl AppBuilder {
    /// Create a new application builder with default settings.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            backend_factory: BackendFactory::new(),
            group_factory: GroupFactory::new(),
            router_factory: RouterFactory::new(),
            validator_factory: ValidatorFactory::new(),
            rate_limiter_factory: RateLimiterFactory::new(),
        }
    }

    /// Set the backend factory.
    #[must_use]
    pub const fn backend_factory(mut self, factory: BackendFactory) -> Self {
        self.backend_factory = factory;
        self
    }

    /// Set the default batch size for backends.
    #[must_use]
    pub const fn with_batch_size(mut self, size: usize) -> Self {
        self.backend_factory = self.backend_factory.with_batch_size(size);
        self
    }

    /// Build the application router from configuration.
    ///
    /// This method orchestrates the creation of all components:
    /// 1. Creates backends from config
    /// 2. Creates backend groups with load balancers
    /// 3. Creates RPC codec with configured limits
    /// 4. Creates method router
    /// 5. Creates validators
    /// 6. Creates rate limiter if enabled
    /// 7. Builds server state
    /// 8. Creates cache if enabled
    /// 9. Creates and returns the axum Router
    ///
    /// # Errors
    ///
    /// Returns an error if the application cannot be built.
    pub async fn build(self, config: &RoxyConfig) -> Result<roxy_server::Router> {
        // 1. Create backends from config
        let backends = self.backend_factory.create(config)?;
        debug!(count = backends.len(), "Created backends");

        // 2. Create backend groups with load balancers
        let groups = self.group_factory.create(config, &backends)?;
        debug!(count = groups.len(), "Created backend groups");

        // 3. Create RPC codec with configured limits
        let codec =
            RpcCodec::new(DefaultCodecConfig::new().with_max_size(config.server.max_request_size));

        // 4. Create method router
        let router = self.router_factory.create(config);

        // 5. Create validators
        let validators = self.validator_factory.create(config);

        // 6. Create rate limiter if enabled
        let rate_limiter = self.rate_limiter_factory.create(config);

        // 7. Build server state
        let mut builder = ServerBuilder::new().codec(codec).router(router).validators(validators);

        if let Some(rl) = rate_limiter {
            debug!("Adding rate limiter to server");
            builder = builder.rate_limiter(Arc::new(rl));
        }

        for (name, group) in groups {
            debug!(name = %name, "Adding backend group to server");
            builder = builder.add_group(name, Arc::new(group));
        }

        // 8. Create cache if enabled
        if config.cache.enabled {
            let cache = Arc::new(MemoryCache::new(config.cache.memory_size));
            debug!(size = config.cache.memory_size, "Created memory cache");
            let state = builder.cache(cache).build().wrap_err("failed to build server state")?;
            // 9. Create router
            return Ok(create_router(state));
        }

        let state = builder.build().wrap_err("failed to build server state")?;

        // 9. Create router
        Ok(create_router(state))
    }
}

impl Default for AppBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the application router from configuration.
///
/// This is a convenience function that creates an [`AppBuilder`] and builds
/// the application with default settings.
///
/// # Arguments
///
/// * `config` - The Roxy configuration
///
/// # Returns
///
/// Returns the configured axum Router.
///
/// # Errors
///
/// Returns an error if the application cannot be built.
pub async fn build_app(config: &RoxyConfig) -> Result<roxy_server::Router> {
    AppBuilder::new().build(config).await
}
