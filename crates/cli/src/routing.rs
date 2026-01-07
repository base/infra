//! Method routing configuration utilities.
//!
//! This module provides factory types for creating RPC method routers from configuration.

use roxy_config::RoxyConfig;
use roxy_rpc::{MethodRouter, RouteTarget};

/// Factory for creating method routers from configuration.
///
/// # Example
///
/// ```ignore
/// use roxyproxy_cli::RouterFactory;
///
/// let router = RouterFactory::new().create(&config);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct RouterFactory;

impl RouterFactory {
    /// Create a new router factory.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Create a method router from configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - The Roxy configuration
    ///
    /// # Returns
    ///
    /// A configured MethodRouter.
    pub fn create(&self, config: &RoxyConfig) -> MethodRouter {
        let mut router = MethodRouter::new();

        // Add specific routes from config
        for route in &config.routing.routes {
            let target = if route.target == "block" {
                RouteTarget::Block
            } else {
                RouteTarget::group(&route.target)
            };

            // Check if this is a prefix route (ends with _)
            if route.method.ends_with('_') {
                router = router.route_prefix(&route.method, target);
                trace!(prefix = %route.method, target = %route.target, "Added prefix route");
            } else {
                router = router.route(&route.method, target);
                trace!(method = %route.method, target = %route.target, "Added exact route");
            }
        }

        // Add blocked methods as routes
        for method in &config.routing.blocked_methods {
            router = router.route(method, RouteTarget::Block);
            trace!(method = %method, "Added blocked method route");
        }

        // Set the default group
        if !config.routing.default_group.is_empty() {
            router = router.fallback(RouteTarget::group(&config.routing.default_group));
            trace!(default_group = %config.routing.default_group, "Set default route");
        }

        router
    }
}

/// Create the method router from configuration.
///
/// This is a convenience function that creates a [`RouterFactory`] and calls
/// [`RouterFactory::create`].
///
/// # Arguments
///
/// * `config` - The Roxy configuration
///
/// # Returns
///
/// A configured MethodRouter.
pub fn create_method_router(config: &RoxyConfig) -> MethodRouter {
    RouterFactory::new().create(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::minimal_config;

    #[test]
    fn test_create_method_router() {
        let mut config = minimal_config();
        config.routing.blocked_methods = vec!["debug_traceTransaction".to_string()];
        config.routing.default_group = "main".to_string();

        let router = create_method_router(&config);
        assert!(router.is_blocked("debug_traceTransaction"));
        assert!(!router.is_blocked("eth_call"));
    }

    #[test]
    fn test_router_factory_default() {
        let factory = RouterFactory::default();
        let _ = format!("{:?}", factory);
    }
}
