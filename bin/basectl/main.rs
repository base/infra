use std::{fs::File, path::Path};

use anyhow::bail;
use basectl_cli::{
    app::{ViewId, run_app_with_view, run_loadtest_logs, run_loadtest_tui},
    config::ChainConfig,
};
use chrono::Local;
use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, fmt::layer, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, Parser)]
#[command(name = "basectl")]
#[command(about = "Base infrastructure control CLI")]
struct Cli {
    /// Chain configuration (mainnet, sepolia, devnet, or path to config file)
    #[arg(short = 'c', long = "config", default_value = "mainnet", global = true)]
    config: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Chain configuration operations
    #[command(visible_alias = "c")]
    Config,
    /// Flashblocks operations
    #[command(visible_alias = "f")]
    Flashblocks,
    /// DA (Data Availability) backlog monitor
    #[command(visible_alias = "d")]
    Da,
    /// Command center (combined view)
    #[command(visible_alias = "cc")]
    CommandCenter,
    /// Run a load test with real-time TUI dashboard
    #[command(visible_alias = "lt")]
    Loadtest {
        /// Path to gobrr YAML config file
        #[arg(long = "file", short = 'f')]
        file: String,
        /// Output text summary every 2s instead of TUI (for headless/CI)
        #[arg(long = "logs")]
        logs: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let chain_config = ChainConfig::load(&cli.config).await?;

    match cli.command {
        Some(Commands::Loadtest { file, logs }) => {
            let path = Path::new(&file);
            if !path.is_file() {
                bail!("Load test config not found: {}", path.display());
            }

            // Create log file with timestamp
            let timestamp = Local::now().format("%Y%m%d-%H%M%S");
            let log_filename = format!("load-test-{timestamp}.log");
            let log_file = File::create(&log_filename)?;

            let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("warn,gobrr=debug,basectl=debug,basectl_cli=debug")
            });

            if logs {
                // Headless mode: write to both file and stdout
                let file_layer = layer().with_writer(log_file).with_ansi(false);
                let stdout_layer = layer().with_writer(std::io::stdout);

                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(file_layer)
                    .with(stdout_layer)
                    .init();

                eprintln!("Logging to: {log_filename}");
                run_loadtest_logs(chain_config, file).await
            } else {
                // TUI mode: write only to file (TUI controls terminal)
                let file_layer = layer().with_writer(log_file).with_ansi(false);

                tracing_subscriber::registry().with(env_filter).with(file_layer).init();

                eprintln!("Logging to: {log_filename}");
                run_loadtest_tui(chain_config, file).await
            }
        }
        Some(Commands::Config) => run_app_with_view(chain_config, ViewId::Config).await,
        Some(Commands::Flashblocks) => run_app_with_view(chain_config, ViewId::Flashblocks).await,
        Some(Commands::Da) => run_app_with_view(chain_config, ViewId::DaMonitor).await,
        Some(Commands::CommandCenter) => {
            run_app_with_view(chain_config, ViewId::CommandCenter).await
        }
        None => run_app_with_view(chain_config, ViewId::Home).await,
    }
}
