use std::collections::HashSet;

use alloy_primitives::{B256, keccak256};
use base_flashtypes::Flashblock;
use futures_util::StreamExt;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::connect_async;
use tracing::{debug, info, warn};

use crate::tracker::TrackerEvent;

/// Watches for flashblocks via WebSocket and reports transactions seen.
pub(crate) async fn run_flashblock_watcher(
    ws_url: String,
    mut pending_rx: mpsc::UnboundedReceiver<B256>,
    tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
    mut shutdown: broadcast::Receiver<()>,
) {
    let mut pending: HashSet<B256> = HashSet::new();

    // Connect to WebSocket
    let ws_stream = match connect_async(&ws_url).await {
        Ok((stream, _)) => {
            info!(url = %ws_url, "Connected to flashblocks WebSocket");
            stream
        }
        Err(e) => {
            warn!(url = %ws_url, error = %e, "Failed to connect to flashblocks WebSocket");
            return;
        }
    };

    let (_, mut read) = ws_stream.split();

    loop {
        tokio::select! {
            biased;
            // Handle new pending tx hashes to track
            Some(tx_hash) = pending_rx.recv() => {
                pending.insert(tx_hash);
            }
            // Handle incoming flashblock messages
            msg_result = read.next() => {
                match msg_result {
                    Some(Ok(msg)) => {
                        if !msg.is_binary() && !msg.is_text() {
                            continue;
                        }
                        match Flashblock::try_decode_message(msg.into_data()) {
                            Ok(fb) => {
                                let mut our_count = 0u64;
                                // Extract tx hashes from flashblock transactions
                                for tx_bytes in &fb.diff.transactions {
                                    // Compute tx hash from raw transaction bytes
                                    let tx_hash = keccak256(tx_bytes);
                                    if pending.remove(&tx_hash) {
                                        our_count += 1;
                                        if let Err(e) = tracker_tx.send(TrackerEvent::FlashblockReceived {
                                            tx_hash,
                                        }) {
                                            warn!(tx_hash = %tx_hash, error = %e, "Failed to send FB inclusion to tracker");
                                        }
                                    }
                                }
                                if our_count > 0 {
                                    debug!(
                                        block = fb.metadata.block_number,
                                        index = fb.index,
                                        our_txs = our_count,
                                        pending = pending.len(),
                                        "Flashblock inclusion"
                                    );
                                }
                            }
                            Err(e) => {
                                debug!(error = %e, "Failed to decode flashblock message");
                            }
                        }
                    }
                    Some(Err(e)) => {
                        warn!(error = %e, "WebSocket error");
                        break;
                    }
                    None => {
                        debug!("WebSocket stream closed");
                        break;
                    }
                }
            }
            // Handle shutdown
            _ = shutdown.recv() => {
                debug!("Flashblock watcher shutting down");
                break;
            }
        }
    }
}
