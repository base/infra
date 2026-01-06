//! Command-line interface definitions for Roxy.
//!
//! This module provides the CLI argument parsing using clap.

use std::path::PathBuf;

use clap::Parser;

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

#[cfg(test)]
mod tests {
    use super::*;

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
