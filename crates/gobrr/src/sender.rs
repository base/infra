use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use alloy_primitives::B256;
use alloy_provider::Provider;
use alloy_rpc_client::BatchRequest;
use anyhow::Result;
use tokio::{
    sync::{Semaphore, broadcast, mpsc},
    task::JoinSet,
};
use tracing::{debug, error, warn};

use crate::{
    SenderId,
    client::{self, Provider as OpProvider},
    signer::{ResignRequest, SignedTx},
    tracker::TrackerEvent,
};

/// Batch size for transaction batching
const BATCH_SIZE: usize = 5;
/// Timeout to flush partial batches
const BATCH_FLUSH_TIMEOUT: Duration = Duration::from_millis(50);

/// Consecutive error threshold before backoff kicks in
const BACKOFF_THRESHOLD: u32 = 3;
/// Base backoff duration
const BACKOFF_BASE: Duration = Duration::from_millis(100);
/// Maximum backoff duration
const BACKOFF_MAX: Duration = Duration::from_secs(5);

/// Classification of send errors
enum SendErrorKind {
    /// "underpriced", "replacement" — re-sign with bumped fees
    Underpriced,
    /// Network errors, timeouts, rate limits — trigger backoff
    Transient,
    /// "nonce too low" — tx with this nonce already mined
    NonceTooLow,
    /// "already known" — node already has this tx in mempool
    AlreadyKnown,
}

impl SendErrorKind {
    fn classify(error_msg: &str) -> Self {
        let lower = error_msg.to_lowercase();
        if lower.contains("underpriced") || lower.contains("replacement") {
            Self::Underpriced
        } else if lower.contains("nonce too low") {
            Self::NonceTooLow
        } else if lower.contains("already known") {
            Self::AlreadyKnown
        } else {
            Self::Transient
        }
    }

    const fn label(&self) -> &'static str {
        match self {
            Self::Underpriced => "underpriced",
            Self::Transient => "transient",
            Self::NonceTooLow => "nonce_too_low",
            Self::AlreadyKnown => "already_known",
        }
    }
}

/// Sender sends signed transactions in batches
pub(crate) struct Sender {
    sender_id: SenderId,
    provider: OpProvider,
    semaphore: Arc<Semaphore>,
    tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
    confirmer_tx: mpsc::UnboundedSender<B256>,
    flashblock_tx: mpsc::UnboundedSender<B256>,
    resign_tx: mpsc::Sender<ResignRequest>,
    consecutive_errors: Arc<AtomicU32>,
}

impl Sender {
    /// Creates a new Sender
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        http_client: reqwest::Client,
        sender_id: SenderId,
        rpc_url: &str,
        in_flight_limit: u32,
        tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
        confirmer_tx: mpsc::UnboundedSender<B256>,
        flashblock_tx: mpsc::UnboundedSender<B256>,
        resign_tx: mpsc::Sender<ResignRequest>,
    ) -> Result<Self> {
        let provider = client::create_provider(http_client, rpc_url)?;
        let semaphore = Arc::new(Semaphore::new(in_flight_limit as usize));

        Ok(Self {
            sender_id,
            provider,
            semaphore,
            tracker_tx,
            confirmer_tx,
            flashblock_tx,
            resign_tx,
            consecutive_errors: Arc::new(AtomicU32::new(0)),
        })
    }

    /// Spawns a task to send a batch of transactions
    async fn spawn_batch(&self, tasks: &mut JoinSet<()>, batch: Vec<SignedTx>) -> Result<()> {
        let permit = Arc::clone(&self.semaphore).acquire_many_owned(batch.len() as u32).await?;
        let provider = self.provider.clone();
        let tracker = self.tracker_tx.clone();
        let confirmer = self.confirmer_tx.clone();
        let flashblock = self.flashblock_tx.clone();
        let resign = self.resign_tx.clone();
        let errors = Arc::clone(&self.consecutive_errors);
        let idx = self.sender_id;
        tasks.spawn(async move {
            send_batch(idx, batch, provider, tracker, confirmer, flashblock, resign, errors).await;
            drop(permit);
        });
        Ok(())
    }

    /// Adaptive backoff based on consecutive error count
    async fn maybe_backoff(&self) {
        let errors = self.consecutive_errors.load(Ordering::Relaxed);
        if errors >= BACKOFF_THRESHOLD {
            let exp = errors - BACKOFF_THRESHOLD;
            let backoff = BACKOFF_BASE.saturating_mul(1u32.wrapping_shl(exp));
            let backoff = backoff.min(BACKOFF_MAX);
            warn!(
                sender = self.sender_id,
                consecutive_errors = errors,
                backoff_ms = backoff.as_millis() as u64,
                "Backing off due to consecutive errors"
            );
            tokio::time::sleep(backoff).await;
        }
    }

    /// Runs the sender loop, receiving signed transactions and sending them in batches
    pub(crate) async fn run(
        self,
        mut signed_rx: mpsc::Receiver<SignedTx>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<()> {
        let mut tasks: JoinSet<()> = JoinSet::new();
        let mut batch_buffer: Vec<SignedTx> = Vec::with_capacity(BATCH_SIZE);

        debug!(sender = self.sender_id, "Sender started with batching enabled");

        loop {
            tokio::select! {
                biased;
                _ = shutdown.recv() => {
                    debug!(sender = self.sender_id, "Sender shutting down");
                    // Flush any remaining transactions in the buffer
                    if !batch_buffer.is_empty() {
                        let batch = std::mem::take(&mut batch_buffer);
                        self.spawn_batch(&mut tasks, batch).await?;
                    }
                    // Wait for all spawned tasks to complete
                    while tasks.join_next().await.is_some() {}
                    break;
                }
                Some(signed) = signed_rx.recv() => {
                    batch_buffer.push(signed);

                    if batch_buffer.len() >= BATCH_SIZE {
                        let batch = std::mem::take(&mut batch_buffer);
                        self.spawn_batch(&mut tasks, batch).await?;
                        self.maybe_backoff().await;
                    }
                }
                _ = tokio::time::sleep(BATCH_FLUSH_TIMEOUT), if !batch_buffer.is_empty() => {
                    // Flush partial batch on timeout
                    let batch = std::mem::take(&mut batch_buffer);
                    self.spawn_batch(&mut tasks, batch).await?;
                    self.maybe_backoff().await;
                }
            }
        }

        Ok(())
    }
}

/// Send a batch of transactions using JSON-RPC batching
#[allow(clippy::too_many_arguments)]
async fn send_batch(
    sender_id: SenderId,
    batch: Vec<SignedTx>,
    provider: OpProvider,
    tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
    confirmer_tx: mpsc::UnboundedSender<B256>,
    flashblock_tx: mpsc::UnboundedSender<B256>,
    resign_tx: mpsc::Sender<ResignRequest>,
    consecutive_errors: Arc<AtomicU32>,
) {
    let original_size = batch.len();
    debug!(sender = sender_id, batch_size = original_size, "Sending transaction batch");

    // Build batch request using BatchRequest directly
    let mut batch_req = BatchRequest::new(provider.client());
    let mut queued = Vec::with_capacity(original_size);

    for tx in batch {
        match batch_req.add_call::<_, B256>("eth_sendRawTransaction", &(tx.raw_bytes.clone(),)) {
            Ok(fut) => queued.push((tx, fut)),
            Err(e) => {
                consecutive_errors.fetch_add(1, Ordering::Relaxed);
                error!(
                    sender = sender_id,
                    tx_hash = %tx.tx_hash,
                    error = ?e,
                    "Failed to add tx to batch"
                );
                let _ = tracker_tx.send(TrackerEvent::TxFailed {
                    tx_hash: tx.tx_hash,
                    tx_type: tx.tx_type,
                    reason: "batch_add_error".to_owned(),
                    retried: tx.is_retry,
                });
            }
        }
    }

    if queued.is_empty() {
        return;
    }

    // Send the batch request (single HTTP request for all transactions)
    if let Err(e) = batch_req.send().await {
        error!(
            sender = sender_id,
            batch_size = queued.len(),
            error = ?e,
            "Failed to send batch request"
        );
        // Whole-batch HTTP failure — count all txs as transient failures
        consecutive_errors.fetch_add(queued.len() as u32, Ordering::Relaxed);
        for (tx, _) in queued {
            let _ = tracker_tx.send(TrackerEvent::TxFailed {
                tx_hash: tx.tx_hash,
                tx_type: tx.tx_type,
                reason: "batch_http_error".to_owned(),
                retried: tx.is_retry,
            });
        }
        return;
    }

    // Collect results
    for (tx, fut) in queued {
        match fut.await {
            Ok(_) => {
                // Success — decay consecutive error counter
                let _ =
                    consecutive_errors.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                        Some(v.saturating_sub(1))
                    });

                // Notify tracker that tx was sent
                if let Err(e) = tracker_tx
                    .send(TrackerEvent::TxSent { tx_hash: tx.tx_hash, tx_type: tx.tx_type })
                {
                    warn!(tx_hash = %tx.tx_hash, error = %e, "Failed to send TxSent to tracker");
                }

                // Send tx hash to confirmer after successful send
                if let Err(e) = confirmer_tx.send(tx.tx_hash) {
                    warn!(tx_hash = %tx.tx_hash, error = %e, "Failed to send tx hash to confirmer");
                }

                // Send tx hash to flashblock watcher for FB inclusion tracking
                if let Err(e) = flashblock_tx.send(tx.tx_hash) {
                    warn!(tx_hash = %tx.tx_hash, error = %e, "Failed to send tx hash to flashblock watcher");
                }

                debug!(
                    sender = sender_id,
                    tx_hash = %tx.tx_hash,
                    nonce = tx.nonce,
                    "Transaction sent via batch"
                );
            }
            Err(e) => {
                let error_msg = format!("{e:?}");
                let kind = SendErrorKind::classify(&error_msg);

                match kind {
                    SendErrorKind::Underpriced if !tx.is_retry => {
                        // First underpriced — send to signer for re-sign
                        debug!(
                            sender = sender_id,
                            nonce = tx.nonce,
                            tx_hash = %tx.tx_hash,
                            "Underpriced tx, requesting re-sign"
                        );
                        let _ = resign_tx
                            .send(ResignRequest { nonce: tx.nonce, unsigned: tx.unsigned })
                            .await;
                    }
                    SendErrorKind::Underpriced => {
                        // Already retried — give up
                        warn!(
                            sender = sender_id,
                            nonce = tx.nonce,
                            tx_hash = %tx.tx_hash,
                            "Underpriced tx after retry, giving up"
                        );
                        let _ = tracker_tx.send(TrackerEvent::TxFailed {
                            tx_hash: tx.tx_hash,
                            tx_type: tx.tx_type,
                            reason: kind.label().to_owned(),
                            retried: true,
                        });
                    }
                    SendErrorKind::Transient => {
                        consecutive_errors.fetch_add(1, Ordering::Relaxed);
                        warn!(
                            sender = sender_id,
                            nonce = tx.nonce,
                            tx_hash = %tx.tx_hash,
                            error = %error_msg,
                            "Transient send error"
                        );
                        let _ = tracker_tx.send(TrackerEvent::TxFailed {
                            tx_hash: tx.tx_hash,
                            tx_type: tx.tx_type,
                            reason: kind.label().to_owned(),
                            retried: tx.is_retry,
                        });
                    }
                    SendErrorKind::NonceTooLow | SendErrorKind::AlreadyKnown => {
                        debug!(
                            sender = sender_id,
                            nonce = tx.nonce,
                            tx_hash = %tx.tx_hash,
                            kind = kind.label(),
                            "Stale tx"
                        );
                        let _ = tracker_tx.send(TrackerEvent::TxFailed {
                            tx_hash: tx.tx_hash,
                            tx_type: tx.tx_type,
                            reason: kind.label().to_owned(),
                            retried: tx.is_retry,
                        });
                    }
                }
            }
        }
    }
}
