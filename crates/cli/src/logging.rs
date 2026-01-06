//! Logging and tracing utilities for Roxy.
//!
//! This module provides tracing initialization and configuration logging.

use eyre::{Context, Result};
use roxy_config::RoxyConfig;

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
        info!(
            host = %config.server.host,
            port = config.server.port,
            max_connections = config.server.max_connections,
            "Server configuration"
        );

        info!(count = config.backends.len(), "Backends configured");

        for backend in &config.backends {
            debug!(
                name = %backend.name,
                url = %backend.url,
                weight = backend.weight,
                "Backend"
            );
        }

        info!(count = config.groups.len(), "Backend groups configured");

        for group in &config.groups {
            debug!(
                name = %group.name,
                backends = ?group.backends,
                load_balancer = %group.load_balancer,
                "Group"
            );
        }

        if config.cache.enabled {
            info!(
                memory_size = config.cache.memory_size,
                default_ttl_ms = config.cache.default_ttl_ms,
                "Cache enabled"
            );
        }

        if config.rate_limit.enabled {
            info!(
                requests_per_second = config.rate_limit.requests_per_second,
                burst_size = config.rate_limit.burst_size,
                "Rate limiting enabled"
            );
        }

        if !config.routing.blocked_methods.is_empty() {
            info!(
                methods = ?config.routing.blocked_methods,
                "Blocked methods"
            );
        }

        if config.metrics.enabled {
            info!(
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
