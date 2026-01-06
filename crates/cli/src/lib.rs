#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

// Re-export for convenience
pub use clap;
use clap::Parser;
pub use eyre;
use eyre::{Context, Result};

/// Macro to check CLI configuration and exit early if the check flag is set.
///
/// This macro handles the common pattern of validating configuration and exiting
/// early when the `--check` flag is passed to the CLI. It prints a success message
/// and returns `Ok(())` if the check flag is set.
///
/// # Example
///
/// ```ignore
/// use roxy_cli::{Cli, check_config};
/// use clap::Parser;
///
/// #[tokio::main]
/// async fn main() -> eyre::Result<()> {
///     let cli = Cli::parse();
///     // ... load config ...
///     check_config!(cli);
///     // ... continue with server startup ...
///     Ok(())
/// }
/// ```
#[macro_export]
macro_rules! check_config {
    ($cli:expr) => {
        if $cli.check {
            println!("Configuration is valid");
            return Ok(());
        }
    };
}
use roxy_backend::{
    BackendConfig as HttpBackendConfig, BackendGroup, EmaLoadBalancer, HttpBackend,
    RoundRobinBalancer,
};
use roxy_cache::MemoryCache;
use roxy_config::{LoadBalancerType, RoxyConfig};
use roxy_rpc::{
    MethodBlocklist, MethodRouter, RateLimiterConfig, RouteTarget, RpcCodec,
    SlidingWindowRateLimiter, ValidatorChain,
};
use roxy_server::{ServerBuilder, create_router};
use roxy_traits::{Backend, DefaultCodecConfig, LoadBalancer};

/// Command-line interface for Roxy RPC proxy.
#[derive(Parser, Debug, Clone)]
#[command(name = "roxy")]
#[command(about = "High-performance Ethereum JSON-RPC proxy")]
#[command(version)]
pub struct Cli {
    /// Path to the configuration file
    #[arg(short, long, default_value = "roxy.toml")]
    pub config: PathBuf,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    pub log_level: String,

    /// Validate config and exit
    #[arg(long)]
    pub check: bool,
}

/// Initialize the tracing subscriber for logging.
///
/// # Arguments
///
/// * `level` - The log level string (trace, debug, info, warn, error)
///
/// # Errors
///
/// Returns an error if the tracing subscriber cannot be initialized.
pub fn init_tracing(level: &str) -> Result<()> {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    let filter = EnvFilter::try_new(level)
        .or_else(|_| EnvFilter::try_new("info"))
        .wrap_err("failed to create log filter")?;

    tracing_subscriber::registry().with(fmt::layer()).with(filter).init();

    Ok(())
}

/// A logger for Roxy configuration.
///
/// Provides methods for logging configuration summaries at startup.
#[derive(Debug, Default, Clone, Copy)]
pub struct Logger;

impl Logger {
    /// Create a new Logger instance.
    pub const fn new() -> Self {
        Self
    }

    /// Log a summary of the configuration at startup.
    pub fn log(&self, config: &RoxyConfig) {
        tracing::info!(
            host = %config.server.host,
            port = config.server.port,
            max_connections = config.server.max_connections,
            "Server configuration"
        );

        tracing::info!(count = config.backends.len(), "Backends configured");

        for backend in &config.backends {
            tracing::debug!(
                name = %backend.name,
                url = %backend.url,
                weight = backend.weight,
                "Backend"
            );
        }

        tracing::info!(count = config.groups.len(), "Backend groups configured");

        for group in &config.groups {
            tracing::debug!(
                name = %group.name,
                backends = ?group.backends,
                load_balancer = %group.load_balancer,
                "Group"
            );
        }

        if config.cache.enabled {
            tracing::info!(
                memory_size = config.cache.memory_size,
                default_ttl_ms = config.cache.default_ttl_ms,
                "Cache enabled"
            );
        }

        if config.rate_limit.enabled {
            tracing::info!(
                requests_per_second = config.rate_limit.requests_per_second,
                burst_size = config.rate_limit.burst_size,
                "Rate limiting enabled"
            );
        }

        if !config.routing.blocked_methods.is_empty() {
            tracing::info!(
                methods = ?config.routing.blocked_methods,
                "Blocked methods"
            );
        }

        if config.metrics.enabled {
            tracing::info!(
                host = %config.metrics.host,
                port = config.metrics.port,
                "Metrics enabled"
            );
        }
    }
}

/// Log a summary of the configuration at startup.
///
/// This is a convenience function that creates a [`Logger`] and calls [`Logger::log`].
#[deprecated(since = "0.1.0", note = "Use Logger::new().log(&config) instead")]
pub fn log_config_summary(config: &RoxyConfig) {
    Logger.log(config);
}

/// Build the application router from configuration.
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
    // 1. Create backends from config
    let backends = create_backends(config)?;
    tracing::debug!(count = backends.len(), "Created backends");

    // 2. Create backend groups with load balancers
    let groups = create_groups(config, &backends)?;
    tracing::debug!(count = groups.len(), "Created backend groups");

    // 3. Create RPC codec with configured limits
    let codec =
        RpcCodec::new(DefaultCodecConfig::new().with_max_size(config.server.max_request_size));

    // 4. Create method router
    let router = create_method_router(config);

    // 5. Create validators
    let validators = create_validators(config);

    // 6. Create rate limiter if enabled
    let rate_limiter = create_rate_limiter(config);

    // 7. Build server state
    let mut builder = ServerBuilder::new().codec(codec).router(router).validators(validators);

    if let Some(rl) = rate_limiter {
        tracing::debug!("Adding rate limiter to server");
        builder = builder.rate_limiter(Arc::new(rl));
    }

    for (name, group) in groups {
        tracing::debug!(name = %name, "Adding backend group to server");
        builder = builder.add_group(name, Arc::new(group));
    }

    // 8. Create cache if enabled
    if config.cache.enabled {
        let cache = Arc::new(MemoryCache::new(config.cache.memory_size));
        tracing::debug!(size = config.cache.memory_size, "Created memory cache");
        builder = builder.cache(cache);
    }

    let state = builder.build().wrap_err("failed to build server state")?;

    // 9. Create router
    Ok(create_router(state))
}

/// Run the HTTP server.
///
/// # Arguments
///
/// * `app` - The axum Router
/// * `config` - The Roxy configuration
///
/// # Errors
///
/// Returns an error if the server fails to start or encounters an error while running.
pub async fn run_server(app: roxy_server::Router, config: &RoxyConfig) -> Result<()> {
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .wrap_err_with(|| format!("failed to bind to {}", addr))?;

    tracing::info!(address = %addr, "Roxy RPC proxy listening");

    // Graceful shutdown on Ctrl+C
    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Received shutdown signal, shutting down gracefully...");
    };

    axum::serve(listener, app).with_graceful_shutdown(shutdown).await.wrap_err("server error")?;

    tracing::info!("Server shut down successfully");
    Ok(())
}

/// Create HTTP backends from configuration.
///
/// # Arguments
///
/// * `config` - The Roxy configuration
///
/// # Returns
///
/// A map of backend names to Backend trait objects.
///
/// # Errors
///
/// Returns an error if backend creation fails.
pub fn create_backends(config: &RoxyConfig) -> Result<HashMap<String, Arc<dyn Backend>>> {
    let mut backends = HashMap::new();

    for backend_config in &config.backends {
        let http_config = HttpBackendConfig {
            timeout: Duration::from_millis(backend_config.timeout_ms),
            max_retries: backend_config.max_retries,
            max_batch_size: 100, // Default batch size
        };

        let backend =
            HttpBackend::new(backend_config.name.clone(), backend_config.url.clone(), http_config);

        backends.insert(backend_config.name.clone(), Arc::new(backend) as Arc<dyn Backend>);
        tracing::trace!(name = %backend_config.name, url = %backend_config.url, "Created backend");
    }

    Ok(backends)
}

/// Create backend groups with load balancers from configuration.
///
/// # Arguments
///
/// * `config` - The Roxy configuration
/// * `backends` - Map of backend names to Backend trait objects
///
/// # Returns
///
/// A map of group names to BackendGroup instances.
///
/// # Errors
///
/// Returns an error if a referenced backend doesn't exist.
pub fn create_groups(
    config: &RoxyConfig,
    backends: &HashMap<String, Arc<dyn Backend>>,
) -> Result<HashMap<String, BackendGroup>> {
    let mut groups = HashMap::new();

    for group_config in &config.groups {
        // Collect backends for this group
        let mut group_backends = Vec::new();
        for backend_name in &group_config.backends {
            let backend = backends
                .get(backend_name)
                .ok_or_else(|| {
                    eyre::eyre!(
                        "backend '{}' not found for group '{}'",
                        backend_name,
                        group_config.name
                    )
                })?
                .clone();
            group_backends.push(backend);
        }

        // Create the appropriate load balancer
        let load_balancer: Arc<dyn LoadBalancer> = match group_config.load_balancer {
            LoadBalancerType::Ema => Arc::new(EmaLoadBalancer),
            LoadBalancerType::RoundRobin => Arc::new(RoundRobinBalancer::new()),
            LoadBalancerType::Random => {
                // Fall back to EMA for now as Random is not implemented
                tracing::warn!(
                    group = %group_config.name,
                    "Random load balancer not implemented, using EMA"
                );
                Arc::new(EmaLoadBalancer)
            }
            LoadBalancerType::LeastConnections => {
                // Fall back to EMA for now as LeastConnections is not implemented
                tracing::warn!(
                    group = %group_config.name,
                    "LeastConnections load balancer not implemented, using EMA"
                );
                Arc::new(EmaLoadBalancer)
            }
        };

        let group = BackendGroup::new(group_config.name.clone(), group_backends, load_balancer);

        groups.insert(group_config.name.clone(), group);
        tracing::trace!(
            name = %group_config.name,
            backends = ?group_config.backends,
            load_balancer = %group_config.load_balancer,
            "Created backend group"
        );
    }

    Ok(groups)
}

/// Create the method router from configuration.
///
/// # Arguments
///
/// * `config` - The Roxy configuration
///
/// # Returns
///
/// A configured MethodRouter.
pub fn create_method_router(config: &RoxyConfig) -> MethodRouter {
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
            tracing::trace!(prefix = %route.method, target = %route.target, "Added prefix route");
        } else {
            router = router.route(&route.method, target);
            tracing::trace!(method = %route.method, target = %route.target, "Added exact route");
        }
    }

    // Add blocked methods as routes
    for method in &config.routing.blocked_methods {
        router = router.route(method, RouteTarget::Block);
        tracing::trace!(method = %method, "Added blocked method route");
    }

    // Set the default group
    if !config.routing.default_group.is_empty() {
        router = router.fallback(RouteTarget::group(&config.routing.default_group));
        tracing::trace!(default_group = %config.routing.default_group, "Set default route");
    }

    router
}

/// Create the validator chain from configuration.
///
/// # Arguments
///
/// * `config` - The Roxy configuration
///
/// # Returns
///
/// A configured ValidatorChain.
pub fn create_validators(config: &RoxyConfig) -> ValidatorChain {
    let mut chain = ValidatorChain::new();

    // Add method blocklist if there are blocked methods
    if !config.routing.blocked_methods.is_empty() {
        let blocklist = MethodBlocklist::new(config.routing.blocked_methods.clone());
        chain = chain.add(blocklist);
        tracing::trace!(
            count = config.routing.blocked_methods.len(),
            "Added method blocklist validator"
        );
    }

    chain
}

/// Create the rate limiter from configuration if enabled.
///
/// # Arguments
///
/// * `config` - The Roxy configuration
///
/// # Returns
///
/// An optional SlidingWindowRateLimiter.
pub fn create_rate_limiter(config: &RoxyConfig) -> Option<SlidingWindowRateLimiter> {
    if !config.rate_limit.enabled {
        return None;
    }

    // Convert requests per second to a window-based configuration
    // Use a 1-second window with the configured requests per second as the limit
    let limiter_config =
        RateLimiterConfig::new(config.rate_limit.requests_per_second, Duration::from_secs(1));

    tracing::trace!(
        requests_per_second = config.rate_limit.requests_per_second,
        burst_size = config.rate_limit.burst_size,
        "Created rate limiter"
    );

    Some(SlidingWindowRateLimiter::new(limiter_config))
}

#[cfg(test)]
mod tests {
    use roxy_config::{BackendConfig, BackendGroupConfig, RoutingConfig, ServerConfig};

    use super::*;

    fn minimal_config() -> RoxyConfig {
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

    #[test]
    fn test_create_backends() {
        let config = minimal_config();
        let backends = create_backends(&config).unwrap();
        assert_eq!(backends.len(), 1);
        assert!(backends.contains_key("primary"));
    }

    #[test]
    fn test_create_groups() {
        let config = minimal_config();
        let backends = create_backends(&config).unwrap();
        let groups = create_groups(&config, &backends).unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups.contains_key("main"));
    }

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
    fn test_cli_parse() {
        let cli = Cli::parse_from(["roxy", "--config", "test.toml", "--log-level", "debug"]);
        assert_eq!(cli.config, PathBuf::from("test.toml"));
        assert_eq!(cli.log_level, "debug");
        assert!(!cli.check);
    }

    #[test]
    fn test_cli_parse_check_flag() {
        let cli = Cli::parse_from(["roxy", "--check"]);
        assert!(cli.check);
    }

    #[test]
    fn test_cli_defaults() {
        let cli = Cli::parse_from(["roxy"]);
        assert_eq!(cli.config, PathBuf::from("roxy.toml"));
        assert_eq!(cli.log_level, "info");
        assert!(!cli.check);
    }
}
