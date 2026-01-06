//! Roxy - High-performance Ethereum JSON-RPC proxy
//!
//! This binary provides the main entry point for running the Roxy RPC proxy server.

use clap::Parser;
use eyre::{Context, Result};
use roxy_cli::{Cli, build_app, init_tracing, log_config_summary};
use roxy_config::RoxyConfig;

/// Main entry point for the Roxy RPC proxy.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    init_tracing(&cli.log_level)?;

    tracing::info!(config_path = %cli.config.display(), "Loading configuration");
    let config = RoxyConfig::from_file(&cli.config)
        .wrap_err_with(|| format!("failed to load config from {}", cli.config.display()))?;

    if cli.check {
        println!("Configuration is valid");
        return Ok(());
    }

    log_config_summary(&config);

    let app = build_app(&config).await?;
    roxy_cli::run_server(app, &config).await
}
