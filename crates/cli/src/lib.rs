#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

#[macro_use]
extern crate tracing;

// Silence unused dependency warnings for macro-only or async runtime deps
use axum as _;
use tokio as _;

mod backends;
pub use backends::{BackendFactory, create_backends};

mod builder;
pub use builder::{AppBuilder, build_app};

mod cli;
pub use cli::Cli;
// Note: check_config! macro is automatically exported via #[macro_export]

mod groups;
pub use groups::{GroupFactory, create_groups};

mod logging;
#[allow(deprecated)]
pub use logging::{Logger, init_tracing, log_config_summary};

mod routing;
pub use routing::{RouterFactory, create_method_router};

mod server;
pub use server::run_server;

mod validators;
pub use validators::{
    RateLimiterFactory, ValidatorFactory, create_rate_limiter, create_validators,
};

// Re-export for convenience
pub use clap;
pub use eyre;

/// Test utilities for creating test configurations.
#[cfg(test)]
pub(crate) mod testutils {
    use roxy_config::{
        BackendConfig, BackendGroupConfig, LoadBalancerType, RoutingConfig, RoxyConfig,
        ServerConfig,
    };

    /// Create a minimal configuration for testing.
    pub(crate) fn minimal_config() -> RoxyConfig {
        RoxyConfig {
            server: ServerConfig::default(),
            backends: vec![BackendConfig {
                name: "primary".to_string(),
                url: "https://eth.example.com".to_string(),
                weight: 1,
                max_retries: 3,
                timeout_ms: 10000,
            }],
            groups: vec![BackendGroupConfig {
                name: "main".to_string(),
                backends: vec!["primary".to_string()],
                load_balancer: LoadBalancerType::Ema,
            }],
            routing: RoutingConfig { default_group: "main".to_string(), ..Default::default() },
            ..Default::default()
        }
    }
}
