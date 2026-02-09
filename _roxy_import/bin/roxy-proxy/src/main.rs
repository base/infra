//! Roxy - High-performance Ethereum JSON-RPC proxy
//!
//! This binary provides the main entry point for running the Roxy RPC proxy server.

#[macro_use]
extern crate tracing;

use clap::Parser;
use eyre::{Context, Result};
use roxy_config::RoxyConfig;
use roxyproxy_cli::{Cli, Logger, build_app, check_config, init_tracing};

/// Main entry point for the Roxy RPC proxy.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    init_tracing(&cli.log_level)?;

    info!(config_path = %cli.config.display(), "Loading configuration");
    let config = RoxyConfig::from_file(&cli.config)
        .wrap_err_with(|| format!("failed to load config from {}", cli.config.display()))?;

    check_config!(cli);

    Logger::new().log(&config);

    let app = build_app(&config).await?;
    roxyproxy_cli::run_server(app, &config).await
}
