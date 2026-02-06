use std::{collections::HashMap, time::Instant};

use alloy_primitives::B256;
use tokio::sync::mpsc;
use tracing::warn;

use crate::cli::RpcMethod;

/// Events sent to the tracker
#[derive(Debug)]
pub(crate) enum TrackerEvent {
    /// A transaction was sent
    TxSent {
        tx_hash: B256,
        has_large_calldata: bool,
        endpoint: Option<String>,
    },
    /// An RPC read call was made
    RpcCallSent {
        method: RpcMethod,
        endpoint: Option<String>,
    },
    /// A receipt was received for a transaction
    ReceiptReceived {
        tx_hash: B256,
    },
    /// A replay request was sent
    #[allow(dead_code)]
    ReplaySent {
        is_tx: bool,
        endpoint: Option<String>,
    },
    /// Request current stats
    GetStats(tokio::sync::oneshot::Sender<Stats>),
    /// Shutdown the tracker
    Shutdown,
}

/// Pending transaction info
struct PendingTx {
    sent_at: Instant,
}

/// Per-endpoint statistics
#[derive(Debug, Clone, Default)]
pub(crate) struct EndpointStats {
    pub(crate) requests: u64,
    pub(crate) transactions: u64,
    pub(crate) read_calls: u64,
    pub(crate) replays: u64,
}

/// Per-method statistics
#[derive(Debug, Clone, Default)]
pub(crate) struct MethodStats {
    pub(crate) calls: u64,
    #[allow(dead_code)]
    pub(crate) avg_latency_ms: f64,
}

/// Statistics collected by the tracker
#[derive(Debug, Clone, Default)]
pub(crate) struct Stats {
    // Transaction stats
    pub(crate) sent: u64,
    pub(crate) confirmed: u64,
    pub(crate) timed_out: u64,
    pub(crate) small_calldata_count: u64,
    pub(crate) large_calldata_count: u64,
    pub(crate) inclusion_times_ms: Vec<u64>,

    // RPC method stats
    pub(crate) read_calls: u64,
    pub(crate) replays: u64,

    // Per-endpoint stats
    pub(crate) endpoint_stats: HashMap<String, EndpointStats>,

    // Per-method stats
    pub(crate) method_stats: HashMap<RpcMethod, MethodStats>,
}

impl Stats {
    pub(crate) const fn pending(&self) -> u64 {
        self.sent.saturating_sub(self.confirmed + self.timed_out)
    }

    pub(crate) fn total_requests(&self) -> u64 {
        self.sent + self.read_calls
    }

    pub(crate) fn avg_inclusion_ms(&self) -> f64 {
        if self.inclusion_times_ms.is_empty() {
            0.0
        } else {
            let sum: u64 = self.inclusion_times_ms.iter().sum();
            sum as f64 / self.inclusion_times_ms.len() as f64
        }
    }

    pub(crate) fn min_inclusion_ms(&self) -> u64 {
        self.inclusion_times_ms.iter().copied().min().unwrap_or(0)
    }

    pub(crate) fn max_inclusion_ms(&self) -> u64 {
        self.inclusion_times_ms.iter().copied().max().unwrap_or(0)
    }

    pub(crate) fn p50_inclusion_ms(&self) -> u64 {
        percentile(&self.inclusion_times_ms, 50)
    }

    pub(crate) fn p95_inclusion_ms(&self) -> u64 {
        percentile(&self.inclusion_times_ms, 95)
    }

    pub(crate) fn p99_inclusion_ms(&self) -> u64 {
        percentile(&self.inclusion_times_ms, 99)
    }
}

fn percentile(values: &[u64], p: u8) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let idx = (sorted.len() as f64 * (p as f64 / 100.0)) as usize;
    sorted.get(idx.min(sorted.len() - 1)).copied().unwrap_or(0)
}

/// Runs the tracker task
pub(crate) async fn run_tracker(mut rx: mpsc::UnboundedReceiver<TrackerEvent>) {
    let mut pending: HashMap<B256, PendingTx> = HashMap::new();
    let mut stats = Stats::default();

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    TrackerEvent::TxSent { tx_hash, has_large_calldata, endpoint } => {
                        stats.sent += 1;
                        if has_large_calldata {
                            stats.large_calldata_count += 1;
                        } else {
                            stats.small_calldata_count += 1;
                        }
                        pending.insert(tx_hash, PendingTx {
                            sent_at: Instant::now(),
                        });

                        // Update endpoint stats
                        if let Some(ep) = endpoint {
                            let ep_stats = stats.endpoint_stats.entry(ep).or_default();
                            ep_stats.requests += 1;
                            ep_stats.transactions += 1;
                        }

                        // Update method stats
                        let method_stats = stats.method_stats.entry(RpcMethod::SendRawTransaction).or_default();
                        method_stats.calls += 1;
                    }
                    TrackerEvent::RpcCallSent { method, endpoint } => {
                        stats.read_calls += 1;

                        // Update endpoint stats
                        if let Some(ep) = endpoint {
                            let ep_stats = stats.endpoint_stats.entry(ep).or_default();
                            ep_stats.requests += 1;
                            ep_stats.read_calls += 1;
                        }

                        // Update method stats
                        let method_stats = stats.method_stats.entry(method).or_default();
                        method_stats.calls += 1;
                    }
                    TrackerEvent::ReplaySent { is_tx, endpoint } => {
                        stats.replays += 1;

                        if let Some(ep) = endpoint {
                            let ep_stats = stats.endpoint_stats.entry(ep).or_default();
                            ep_stats.replays += 1;
                            if is_tx {
                                ep_stats.transactions += 1;
                            } else {
                                ep_stats.read_calls += 1;
                            }
                        }
                    }
                    TrackerEvent::ReceiptReceived { tx_hash } => {
                        if let Some(tx) = pending.remove(&tx_hash) {
                            let inclusion_time = tx.sent_at.elapsed().as_millis() as u64;
                            stats.inclusion_times_ms.push(inclusion_time);
                            stats.confirmed += 1;
                        }
                    }
                    TrackerEvent::GetStats(reply) => {
                        if reply.send(stats.clone()).is_err() {
                            warn!("Failed to send stats response - receiver dropped");
                        }
                    }
                    TrackerEvent::Shutdown => {
                        break;
                    }
                }
            }
        }
    }
}

/// Creates a tracker channel pair (unbounded for high throughput)
pub(crate) fn create_tracker_channel()
-> (mpsc::UnboundedSender<TrackerEvent>, mpsc::UnboundedReceiver<TrackerEvent>) {
    mpsc::unbounded_channel()
}
