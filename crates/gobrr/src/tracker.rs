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
    /// Transaction seen in a flashblock (via WebSocket)
    FlashblockReceived {
        tx_hash: B256,
    },
    /// Transaction confirmed in a regular block (via RPC)
    BlockReceived {
        tx_hash: B256,
    },
    GetStats(oneshot::Sender<Stats>),
    Shutdown,
}

/// Pending transaction info
struct PendingTx {
    sent_at: Instant,
    /// When first seen in a flashblock (if any)
    fb_included_at: Option<Instant>,
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
    // Flashblock inclusion metrics (time from tx sent -> seen in flashblock via WS)
    pub fb_inclusion_count: u64,
    pub fb_inclusion_sum_ms: u64,
    pub fb_inclusion_min_ms: u64,
    pub fb_inclusion_max_ms: u64,
    pub fb_inclusion_times: Vec<u64>,
    // Block inclusion metrics (time from tx sent -> seen in block via RPC)
    pub block_inclusion_count: u64,
    pub block_inclusion_sum_ms: u64,
    pub block_inclusion_min_ms: u64,
    pub block_inclusion_max_ms: u64,
    pub block_inclusion_times: Vec<u64>,
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
            fb_inclusion_count: 0,
            fb_inclusion_sum_ms: 0,
            fb_inclusion_min_ms: u64::MAX,
            fb_inclusion_max_ms: 0,
            fb_inclusion_times: Vec::with_capacity(MAX_INCLUSION_SAMPLES),
            block_inclusion_count: 0,
            block_inclusion_sum_ms: 0,
            block_inclusion_min_ms: u64::MAX,
            block_inclusion_max_ms: 0,
            block_inclusion_times: Vec::with_capacity(MAX_INCLUSION_SAMPLES),
            elapsed_secs: 0.0,
        }
    }
}

impl Stats {
    pub const fn pending(&self) -> u64 {
        self.sent.saturating_sub(self.confirmed + self.timed_out)
    }

    // Flashblock inclusion metrics
    pub fn fb_avg_inclusion_ms(&self) -> f64 {
        if self.fb_inclusion_count == 0 {
            0.0
        } else {
            self.fb_inclusion_sum_ms as f64 / self.fb_inclusion_count as f64
        }
    }

    pub const fn fb_min_inclusion_ms(&self) -> u64 {
        if self.fb_inclusion_count == 0 { 0 } else { self.fb_inclusion_min_ms }
    }

    pub const fn fb_max_inclusion_ms(&self) -> u64 {
        self.fb_inclusion_max_ms
    }

    pub fn fb_percentile(&self, p: f64) -> u64 {
        Self::compute_percentile(&self.fb_inclusion_times, p)
    }

    // Block inclusion metrics
    pub fn block_avg_inclusion_ms(&self) -> f64 {
        if self.block_inclusion_count == 0 {
            0.0
        } else {
            self.block_inclusion_sum_ms as f64 / self.block_inclusion_count as f64
        }
    }

    pub const fn block_min_inclusion_ms(&self) -> u64 {
        if self.block_inclusion_count == 0 { 0 } else { self.block_inclusion_min_ms }
    }

    pub const fn block_max_inclusion_ms(&self) -> u64 {
        self.block_inclusion_max_ms
    }

    pub fn block_percentile(&self, p: f64) -> u64 {
        Self::compute_percentile(&self.block_inclusion_times, p)
    }

    fn compute_percentile(times: &[u64], p: f64) -> u64 {
        if times.is_empty() {
            return 0;
        }
        let mut sorted = times.to_vec();
        sorted.sort_unstable();
        let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    pub fn tps(&self) -> f64 {
        if self.elapsed_secs <= 0.0 { 0.0 } else { self.sent as f64 / self.elapsed_secs }
    }

    pub fn confirmed_tps(&self) -> f64 {
        if self.elapsed_secs <= 0.0 { 0.0 } else { self.confirmed as f64 / self.elapsed_secs }
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
    let mut fb_rng = StdRng::from_entropy();
    let mut block_rng = StdRng::from_entropy();

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    TrackerEvent::TxSent { tx_hash, tx_type } => {
                        stats.sent += 1;
                        *stats.type_counts.entry(tx_type).or_insert(0) += 1;
                        pending.insert(tx_hash, PendingTx {
                            sent_at: Instant::now(),
                            fb_included_at: None,
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
                    TrackerEvent::FlashblockReceived { tx_hash } => {
                        // Record FB inclusion time, but don't remove from pending
                        // (we still want to track block inclusion)
                        if let Some(tx) = pending.get_mut(&tx_hash) {
                            // Only record if not already seen in a flashblock
                            if tx.fb_included_at.is_none() {
                                let now = Instant::now();
                                tx.fb_included_at = Some(now);
                                let fb_time = (now - tx.sent_at).as_millis() as u64;

                                stats.fb_inclusion_count += 1;
                                stats.fb_inclusion_sum_ms += fb_time;
                                if fb_time < stats.fb_inclusion_min_ms {
                                    stats.fb_inclusion_min_ms = fb_time;
                                }
                                if fb_time > stats.fb_inclusion_max_ms {
                                    stats.fb_inclusion_max_ms = fb_time;
                                }

                                // Reservoir sampling for FB times
                                let n = stats.fb_inclusion_count as usize;
                                if n <= MAX_INCLUSION_SAMPLES {
                                    stats.fb_inclusion_times.push(fb_time);
                                } else {
                                    let j = fb_rng.gen_range(0..n);
                                    if j < MAX_INCLUSION_SAMPLES {
                                        stats.fb_inclusion_times[j] = fb_time;
                                    }
                                }
                            }
                        }
                    }
                    TrackerEvent::BlockReceived { tx_hash } => {
                        if let Some(tx) = pending.remove(&tx_hash) {
                            let block_time = tx.sent_at.elapsed().as_millis() as u64;

                            stats.block_inclusion_count += 1;
                            stats.block_inclusion_sum_ms += block_time;
                            if block_time < stats.block_inclusion_min_ms {
                                stats.block_inclusion_min_ms = block_time;
                            }
                            if block_time > stats.block_inclusion_max_ms {
                                stats.block_inclusion_max_ms = block_time;
                            }

                            // Reservoir sampling for block times
                            let n = stats.block_inclusion_count as usize;
                            if n <= MAX_INCLUSION_SAMPLES {
                                stats.block_inclusion_times.push(block_time);
                            } else {
                                let j = block_rng.gen_range(0..n);
                                if j < MAX_INCLUSION_SAMPLES {
                                    stats.block_inclusion_times[j] = block_time;
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
