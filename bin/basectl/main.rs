use clap::{Parser, Subcommand};

use basectl_cli::commands::flashblocks::{FlashblocksCommand, run_flashblocks, default_subscribe};
use basectl_cli::config::ChainConfig;
use basectl_cli::tui::{run_homescreen, HomeSelection};

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
    /// Flashblocks operations
    #[command(visible_alias = "f")]
    Flashblocks {
        #[command(subcommand)]
        command: FlashblocksCommand,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let chain_config = ChainConfig::load(&cli.config)?;

    match cli.command {
        Some(Commands::Flashblocks { command }) => run_flashblocks(command, &chain_config).await,
        None => {
            // Show homescreen when no command provided
            match run_homescreen()? {
                HomeSelection::Flashblocks => default_subscribe(&chain_config).await,
                HomeSelection::Quit => Ok(()),
            }
        }
    }
}
