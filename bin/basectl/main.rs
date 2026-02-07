use basectl_cli::{
    app::{ViewId, run_app_with_view, run_loadtest_tui},
    config::ChainConfig,
};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "basectl")]
#[command(about = "Base infrastructure control CLI")]
struct Cli {
    /// Chain configuration (mainnet, sepolia, or path to config file)
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
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let chain_config = ChainConfig::load(&cli.config)?;

    match cli.command {
        Some(Commands::Loadtest { file }) => run_loadtest_tui(chain_config, file).await,
        Some(Commands::Config) => run_app_with_view(chain_config, ViewId::Config).await,
        Some(Commands::Flashblocks) => run_app_with_view(chain_config, ViewId::Flashblocks).await,
        Some(Commands::Da) => run_app_with_view(chain_config, ViewId::DaMonitor).await,
        Some(Commands::CommandCenter) => {
            run_app_with_view(chain_config, ViewId::CommandCenter).await
        }
        None => run_app_with_view(chain_config, ViewId::Home).await,
    }
}
