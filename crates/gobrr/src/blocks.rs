use std::time::Duration;

use alloy_provider::Provider;
use alloy_rpc_types_eth::BlockNumberOrTag;
use anyhow::{Context, Result};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::client::{self, Provider as OpProvider};

/// An OP-stack block fetched from the RPC.
pub(crate) type OpBlock = <op_alloy_network::Optimism as alloy_network::Network>::BlockResponse;

/// How often the watcher tries to fetch the next block.
const BLOCK_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Watches for new blocks and broadcasts events to consumers
pub(crate) struct BlockWatcher {
    provider: OpProvider,
}

impl BlockWatcher {
    /// Creates a new `BlockWatcher`
    pub(crate) fn new(http_client: reqwest::Client, rpc_url: &str) -> Result<Self> {
        let provider = client::create_provider(http_client, rpc_url)?;
        Ok(Self { provider })
    }

    /// Runs the block watcher loop, broadcasting block events
    pub(crate) async fn run(
        self,
        block_tx: broadcast::Sender<OpBlock>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<()> {
        // Get the starting block number
        let mut last_block =
            self.provider.get_block_number().await.context("Failed to get initial block number")?;

        debug!(block = last_block, "Block watcher started");

        loop {
            tokio::select! {
                biased;
                _ = shutdown.recv() => {
                    debug!("Block watcher shutting down");
                    break;
                }
                _ = tokio::time::sleep(BLOCK_POLL_INTERVAL) => {
                    match self.provider.get_block_by_number(BlockNumberOrTag::Number(last_block + 1)).full().await {
                        Ok(Some(block)) => {
                            last_block += 1;
                            if let Err(e) = block_tx.send(block) {
                                warn!(block = last_block, error = %e, "Failed to broadcast block event");
                            }
                        }
                        Ok(None) => {} // not yet available
                        Err(e) => {
                            warn!(block = last_block + 1, error = %e, "Failed to fetch block");
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// Runs a block logger that subscribes to block events and logs them.
/// Exits when the block channel closes, which happens after the drain period.
pub(crate) async fn run_block_logger(mut block_rx: broadcast::Receiver<OpBlock>) {
    loop {
        match block_rx.recv().await {
            Ok(block) => {
                info!(
                    blockNum = block.header.number,
                    gasUsed = block.header.gas_used,
                    gasLimit = block.header.gas_limit,
                    txnCount = block.transactions.hashes().count(),
                    "New block"
                );
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                debug!(missed = n, "Block logger lagged behind");
            }
            Err(broadcast::error::RecvError::Closed) => {
                debug!("Block broadcast channel closed, logger shutting down");
                break;
            }
        }
    }
}
