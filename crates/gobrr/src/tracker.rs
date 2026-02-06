use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use alloy_primitives::B256;
use rand::{Rng, SeedableRng, rngs::StdRng};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use crate::config::TxType;

/// Events sent to the tracker
#[derive(Debug)]
pub(crate) enum TrackerEvent {
    TxSent {
        tx_hash: B256,
        tx_type: TxType,
    },
    TxFailed {
        #[allow(dead_code)]
        tx_hash: B256,
        tx_type: TxType,
        reason: String,
        retried: bool,
    },
    ReceiptReceived {
        tx_hash: B256,
    },
    GetStats(oneshot::Sender<Stats>),
    Shutdown,
}

/// Pending transaction info
struct PendingTx {
    sent_at: Instant,
}

/// Timeout for pending transactions (evicted after this)
const TX_TIMEOUT: Duration = Duration::from_secs(600);
/// Interval for sweeping timed-out transactions
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);
/// Maximum number of inclusion times to keep (reservoir sample)
const MAX_INCLUSION_SAMPLES: usize = 10_000;

/// Statistics collected by the tracker
#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub sent: u64,
    pub confirmed: u64,
    pub failed: u64,
    pub failed_after_retry: u64,
    pub timed_out: u64,
    pub type_counts: HashMap<TxType, u64>,
    pub failure_reasons: HashMap<String, u64>,
    pub inclusion_count: u64,
    pub inclusion_sum_ms: u64,
    pub inclusion_min_ms: u64,
    pub inclusion_max_ms: u64,
    /// Bounded reservoir sample of inclusion times for percentile computation
    pub inclusion_times: Vec<u64>,
    pub elapsed_secs: f64,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            sent: 0,
            confirmed: 0,
            failed: 0,
            failed_after_retry: 0,
            timed_out: 0,
            type_counts: HashMap::new(),
            failure_reasons: HashMap::new(),
            inclusion_count: 0,
            inclusion_sum_ms: 0,
            inclusion_min_ms: u64::MAX,
            inclusion_max_ms: 0,
            inclusion_times: Vec::with_capacity(MAX_INCLUSION_SAMPLES),
            elapsed_secs: 0.0,
        }
    }
}

impl Stats {
    pub const fn pending(&self) -> u64 {
        self.sent.saturating_sub(self.confirmed + self.timed_out)
    }

    pub fn avg_inclusion_ms(&self) -> f64 {
        if self.inclusion_count == 0 {
            0.0
        } else {
            self.inclusion_sum_ms as f64 / self.inclusion_count as f64
        }
    }

    pub const fn min_inclusion_ms(&self) -> u64 {
        if self.inclusion_count == 0 { 0 } else { self.inclusion_min_ms }
    }

    pub const fn max_inclusion_ms(&self) -> u64 {
        self.inclusion_max_ms
    }

    pub fn tps(&self) -> f64 {
        if self.elapsed_secs <= 0.0 { 0.0 } else { self.sent as f64 / self.elapsed_secs }
    }

    pub fn confirmed_tps(&self) -> f64 {
        if self.elapsed_secs <= 0.0 { 0.0 } else { self.confirmed as f64 / self.elapsed_secs }
    }

    /// Compute the value at the given percentile from the inclusion times sample
    pub fn percentile(&self, p: f64) -> u64 {
        if self.inclusion_times.is_empty() {
            return 0;
        }
        let mut sorted = self.inclusion_times.clone();
        sorted.sort_unstable();
        let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }
}

/// Helper to request stats from the tracker via a oneshot channel
pub(crate) async fn get_stats(tracker_tx: &mpsc::UnboundedSender<TrackerEvent>) -> Option<Stats> {
    let (tx, rx) = oneshot::channel();
    tracker_tx.send(TrackerEvent::GetStats(tx)).ok()?;
    rx.await.ok()
}

/// Runs the tracker task
pub(crate) async fn run_tracker(mut rx: mpsc::UnboundedReceiver<TrackerEvent>) {
    let mut pending: HashMap<B256, PendingTx> = HashMap::new();
    let mut stats = Stats::default();
    let test_start = Instant::now();
    let mut sweep_interval = tokio::time::interval(SWEEP_INTERVAL);
    let mut rng = StdRng::from_entropy();

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    TrackerEvent::TxSent { tx_hash, tx_type } => {
                        stats.sent += 1;
                        *stats.type_counts.entry(tx_type).or_insert(0) += 1;
                        pending.insert(tx_hash, PendingTx {
                            sent_at: Instant::now(),
                        });
                    }
                    TrackerEvent::TxFailed { tx_type, reason, retried, .. } => {
                        stats.failed += 1;
                        if retried {
                            stats.failed_after_retry += 1;
                        }
                        *stats.type_counts.entry(tx_type).or_insert(0) += 1;
                        *stats.failure_reasons.entry(reason).or_insert(0) += 1;
                    }
                    TrackerEvent::ReceiptReceived { tx_hash } => {
                        if let Some(tx) = pending.remove(&tx_hash) {
                            let inclusion_time = tx.sent_at.elapsed().as_millis() as u64;
                            stats.inclusion_count += 1;
                            stats.inclusion_sum_ms += inclusion_time;
                            if inclusion_time < stats.inclusion_min_ms {
                                stats.inclusion_min_ms = inclusion_time;
                            }
                            if inclusion_time > stats.inclusion_max_ms {
                                stats.inclusion_max_ms = inclusion_time;
                            }
                            // Reservoir sampling: keep a bounded sample
                            let n = stats.inclusion_count as usize;
                            if n <= MAX_INCLUSION_SAMPLES {
                                stats.inclusion_times.push(inclusion_time);
                            } else {
                                let j = rng.gen_range(0..n);
                                if j < MAX_INCLUSION_SAMPLES {
                                    stats.inclusion_times[j] = inclusion_time;
                                }
                            }
                            stats.confirmed += 1;
                        }
                    }
                    TrackerEvent::GetStats(reply) => {
                        stats.elapsed_secs = test_start.elapsed().as_secs_f64();
                        if reply.send(stats.clone()).is_err() {
                            warn!("Failed to send stats response - receiver dropped");
                        }
                    }
                    TrackerEvent::Shutdown => {
                        break;
                    }
                }
            }
            _ = sweep_interval.tick() => {
                // Evict timed-out pending transactions
                let before = pending.len();
                pending.retain(|_hash, tx| {
                    if tx.sent_at.elapsed() > TX_TIMEOUT {
                        stats.timed_out += 1;
                        false
                    } else {
                        true
                    }
                });
                let evicted = before - pending.len();
                if evicted > 0 {
                    debug!(evicted, pending = pending.len(), "Swept timed-out transactions");
                }
            }
        }
    }
}
