use alloy_primitives::B256;
use alloy_provider::Provider;
use alloy_rpc_types_eth::BlockNumberOrTag;
use anyhow::{Context, Result};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::client::{self, Provider as OpProvider};

/// Block event broadcast to consumers
#[derive(Debug, Clone)]
pub(crate) struct BlockEvent {
    pub(crate) block_num: u64,
    pub(crate) tx_hashes: Vec<B256>,
    pub(crate) gas_used: u64,
    pub(crate) gas_limit: u64,
}

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
        block_tx: broadcast::Sender<BlockEvent>,
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
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    // Poll for new blocks
                    match self.provider.get_block_number().await {
                        Ok(current_block) => {
                            // Process any new blocks we haven't seen
                            while last_block < current_block {
                                last_block += 1;
                                if let Err(e) = self.broadcast_block(last_block, &block_tx).await {
                                    warn!(block = last_block, error = %e, "Failed to fetch block");
                                }
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to get block number");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Fetches a block and broadcasts it to consumers
    async fn broadcast_block(
        &self,
        block_num: u64,
        block_tx: &broadcast::Sender<BlockEvent>,
    ) -> Result<()> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(block_num))
            .await
            .context("Failed to get block")?
            .context("Block not found")?;

        let tx_hashes: Vec<B256> = block.transactions.hashes().collect();

        let event = BlockEvent {
            block_num,
            tx_hashes,
            gas_used: block.header.gas_used,
            gas_limit: block.header.gas_limit,
        };

        // Broadcast to all subscribers
        if let Err(e) = block_tx.send(event) {
            warn!(block = block_num, error = %e, "Failed to broadcast block event");
        }

        Ok(())
    }
}

/// Runs a block logger that subscribes to block events and logs them.
/// Exits when the block channel closes, which happens after the drain period.
pub(crate) async fn run_block_logger(mut block_rx: broadcast::Receiver<BlockEvent>) {
    loop {
        match block_rx.recv().await {
            Ok(block) => {
                info!(
                    blockNum = block.block_num,
                    gasUsed = block.gas_used,
                    gasLimit = block.gas_limit,
                    txnCount = block.tx_hashes.len(),
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
