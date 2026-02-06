use std::collections::HashSet;

use alloy_primitives::B256;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, warn};

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
                        for tx_hash in block.tx_hashes {
                            if pending.remove(&tx_hash) {
                                if let Err(e) = tracker_tx.send(TrackerEvent::ReceiptReceived {
                                    tx_hash,
                                }) {
                                    warn!(tx_hash = %tx_hash, error = %e, "Failed to send receipt confirmation to tracker");
                                }
                            }
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
