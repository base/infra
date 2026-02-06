use std::collections::HashSet;

use alloy_primitives::B256;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use crate::{blocks::BlockEvent, tracker::TrackerEvent};

/// Runs the confirmer task that matches pending transactions against block contents.
///
/// The confirmer exits when the block channel closes, which happens after the drain period.
/// This ensures it keeps processing confirmations until shutdown is complete.
pub(crate) async fn run_confirmer(
    mut pending_rx: mpsc::UnboundedReceiver<B256>,
    mut block_rx: broadcast::Receiver<BlockEvent>,
    tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
) {
    let mut pending: HashSet<B256> = HashSet::new();

    loop {
        tokio::select! {
            biased;
            Some(tx_hash) = pending_rx.recv() => {
                pending.insert(tx_hash);
            }
            result = block_rx.recv() => {
                match result {
                    Ok(block) => {
                        let total_txs = block.tx_hashes.len();
                        let mut our_count: u64 = 0;

                        for tx_hash in &block.tx_hashes {
                            if pending.remove(tx_hash) {
                                our_count += 1;
                                if let Err(e) = tracker_tx.send(TrackerEvent::ReceiptReceived {
                                    tx_hash: *tx_hash,
                                }) {
                                    warn!(tx_hash = %tx_hash, error = %e, "Failed to send receipt confirmation to tracker");
                                }
                            }
                        }

                        let gas_used_pct = if block.gas_limit > 0 {
                            (block.gas_used as f64 / block.gas_limit as f64) * 100.0
                        } else {
                            0.0
                        };

                        if our_count > 0 || total_txs > 0 {
                            info!(
                                block = block.block_num,
                                our_txs = our_count,
                                total_txs,
                                gas_used_pct = format!("{gas_used_pct:.1}%"),
                                pending = pending.len(),
                                "Block inclusion"
                            );
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!(missed = n, "Confirmer lagged behind block events");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("Block broadcast channel closed, confirmer shutting down");
                        break;
                    }
                }
            }
        }
    }
}
