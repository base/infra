//! Flashblock validator — WebSocket listener, in-memory cache,
//! and priority-fee ordering validation.
//!
//! Uses the canonical [`base_primitives::flashtypes::Flashblock`] type for
//! message decoding (brotli + JSON) rather than hand-rolled structs.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use alloy_consensus::Transaction as _;
use alloy_primitives::{B256, U256, utils::Unit};
use alloy_provider::Provider;
use base_primitives::flashblocks::Flashblock;
use futures_util::StreamExt;
use tokio::{sync::RwLock, task::JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    metrics::{self, Metrics},
    services::Service,
    utils::{self, wei_to_unit},
};

// ── Types ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct FlashblockTransaction {
    effective_priority_fee: u128,
    is_user: bool,
}

#[derive(Debug, Clone)]
struct FlashblockData {
    flashblock_indices: HashSet<u64>,
    last_updated: Instant,
    transaction_hashes: HashMap<B256, u64>,
    base_fee: Option<u128>,
}

impl Default for FlashblockData {
    fn default() -> Self {
        Self {
            flashblock_indices: HashSet::new(),
            last_updated: Instant::now(),
            transaction_hashes: HashMap::new(),
            base_fee: None,
        }
    }
}

// ── Validator ────────────────────────────────────────────────

/// The main flashblock validator, owning the cache and config.
#[derive(Debug)]
pub struct FlashblockValidator<P> {
    name: String,
    ws_url: String,
    provider: Arc<P>,
    cache: Arc<RwLock<HashMap<u64, FlashblockData>>>,
    current_block: Arc<AtomicU64>,
    poll_interval: Duration,
    metrics: Metrics,
}

impl<P: Provider + Clone + Send + Sync + 'static> FlashblockValidator<P> {
    pub fn new(
        name: String,
        ws_url: String,
        provider: Arc<P>,
        poll_interval: Duration,
        metrics: Metrics,
    ) -> Self {
        Self {
            name,
            ws_url,
            provider,
            cache: Arc::new(RwLock::new(HashMap::new())),
            current_block: Arc::new(AtomicU64::new(0)),
            poll_interval,
            metrics,
        }
    }
}

// ── Service impl ─────────────────────────────────────────────

impl<P: Provider + Clone + Send + Sync + 'static> Service for FlashblockValidator<P> {
    fn name(&self) -> &str {
        &self.name
    }

    fn spawn(self: Box<Self>, set: &mut JoinSet<()>, cancel: CancellationToken) {
        let name = self.name.clone();
        set.spawn(async move {
            if let Err(e) = self.run(cancel).await {
                error!(name, error = %e, "flashblock validator failed");
            }
        });
    }
}

// ── Core logic ───────────────────────────────────────────────

impl<P: Provider + Clone + Send + Sync + 'static> FlashblockValidator<P> {
    /// Start the flashblock validator: spawns WS listener, L2 poller, and
    /// cache cleanup tasks, then awaits cancellation.
    async fn run(self, cancel: CancellationToken) -> anyhow::Result<()> {
        let current = self.provider.get_block_number().await?;
        self.current_block.store(current, Ordering::Relaxed);

        tokio::spawn(listen_stream(
            self.ws_url.clone(),
            Arc::clone(&self.cache),
            self.metrics.clone(),
            self.name.clone(),
            cancel.clone(),
        ));

        tokio::spawn(poll_l2_blocks(
            Arc::clone(&self.provider),
            Arc::clone(&self.cache),
            Arc::clone(&self.current_block),
            self.poll_interval,
            self.metrics.clone(),
            self.name.clone(),
            cancel.clone(),
        ));

        tokio::spawn(cleanup_loop(Arc::clone(&self.cache), cancel.clone()));

        cancel.cancelled().await;
        Ok(())
    }
}

// ── WebSocket listener ───────────────────────────────────────

async fn listen_stream(
    ws_url: String,
    cache: Arc<RwLock<HashMap<u64, FlashblockData>>>,
    metrics: Metrics,
    name: String,
    cancel: CancellationToken,
) {
    loop {
        if cancel.is_cancelled() {
            return;
        }

        match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((mut ws, _)) => {
                info!(stream = name, "connected to flashblock WebSocket");

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        msg = ws.next() => {
                            match msg {
                                Some(Ok(msg)) if msg.is_binary() || msg.is_text() => {
                                    process_flashblock_message(msg.into_data(), &cache, &metrics, &name).await;
                                }
                                Some(Err(e)) => {
                                    error!(stream = name, error = %e, "WS read error");
                                    break;
                                }
                                None => break,
                                _ => {}
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!(stream = name, error = %e, "failed to connect to flashblock WS");
            }
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn process_flashblock_message(
    message: impl Into<Vec<u8>>,
    cache: &Arc<RwLock<HashMap<u64, FlashblockData>>>,
    metrics: &Metrics,
    validator_name: &str,
) {
    // Decode (brotli + JSON) via the canonical type.
    let fb_entry = match Flashblock::try_decode_message(message.into()) {
        Ok(fb) => fb,
        Err(e) => {
            error!(error = %e, "failed to decode flashblock message");
            return;
        }
    };

    let block_number = fb_entry.metadata.block_number;
    let index = fb_entry.index;

    let mut cache = cache.write().await;
    let fb = cache.entry(block_number).or_default();

    // Store base fee from index 0.
    if index == 0
        && let Some(base) = &fb_entry.base
    {
        fb.base_fee = base.base_fee_per_gas.try_into().ok();
    }

    let base_fee = fb.base_fee;

    fb.flashblock_indices.insert(index);

    // Decode transactions from diff.
    let mut flashblock_txs = Vec::new();
    for tx_raw in &fb_entry.diff.transactions {
        use alloy_consensus::TxEnvelope;
        use alloy_rlp::Decodable;

        let tx = match TxEnvelope::decode(&mut tx_raw.as_ref()) {
            Ok(t) => t,
            Err(_) => continue,
        };

        fb.transaction_hashes.insert(*tx.tx_hash(), index);

        // Priority fee.
        let tip = tx.max_priority_fee_per_gas().unwrap_or(0);
        let effective_tip = base_fee.map_or(tip, |bf| {
            let max_fee = tx.max_fee_per_gas();
            tip.min(max_fee.saturating_sub(bf))
        });

        let is_user = tx.tx_type() as u8 != utils::DEPOSIT_TX_TYPE;
        flashblock_txs
            .push(FlashblockTransaction { effective_priority_fee: effective_tip, is_user });
    }

    fb.last_updated = Instant::now();

    // Priority fee metrics.
    emit_priority_fee_metrics(index, &flashblock_txs, metrics, validator_name);
    validate_priority_fee_ordering(block_number, index, &flashblock_txs, metrics, validator_name);
}

fn emit_priority_fee_metrics(
    index: u64,
    txs: &[FlashblockTransaction],
    metrics: &Metrics,
    name: &str,
) {
    if index > 10 {
        return;
    }

    let idx_str = index.to_string();
    let tags = [("validator", name), ("flashblock_index", idx_str.as_str())];

    let user_txs: Vec<_> = txs.iter().filter(|tx| tx.is_user).collect();

    for tx in &user_txs {
        metrics.histogram_with_tags(
            metrics::FLASHBLOCK_PRIORITY_FEE_TIP,
            wei_to_unit(U256::from(tx.effective_priority_fee), Unit::GWEI),
            &tags,
        );
    }

    metrics.histogram_with_tags(
        metrics::FLASHBLOCK_PRIORITY_FEE_TX_COUNT,
        user_txs.len() as f64,
        &tags,
    );

    if let Some(min) = user_txs.iter().map(|tx| tx.effective_priority_fee).min() {
        metrics.histogram_with_tags(
            metrics::FLASHBLOCK_PRIORITY_FEE_MIN,
            wei_to_unit(U256::from(min), Unit::GWEI),
            &tags,
        );
    }
}

fn validate_priority_fee_ordering(
    block_number: u64,
    flashblock_index: u64,
    txs: &[FlashblockTransaction],
    metrics: &Metrics,
    name: &str,
) {
    let tags = [("validator", name)];

    let violations = txs
        .windows(2)
        .filter(|pair| pair[1].effective_priority_fee > pair[0].effective_priority_fee)
        .count();

    if violations > 0 {
        warn!(
            block = block_number,
            flashblock_index, violations, "priority fee ordering violation"
        );
        metrics.count_with_tags(metrics::FLASHBLOCK_TX_ORDER_VIOLATION, 1, &tags);
    } else {
        metrics.count_with_tags(metrics::FLASHBLOCK_TX_ORDER_SUCCESS, 1, &tags);
    }
}

// ── L2 block poller ──────────────────────────────────────────

async fn poll_l2_blocks<P: Provider + Send + Sync + 'static>(
    provider: Arc<P>,
    cache: Arc<RwLock<HashMap<u64, FlashblockData>>>,
    current_block: Arc<AtomicU64>,
    poll_interval: Duration,
    metrics: Metrics,
    name: String,
    cancel: CancellationToken,
) {
    let mut interval = tokio::time::interval(poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = interval.tick() => {
                let latest = match provider.get_block_number().await {
                    Ok(n) => n,
                    Err(e) => {
                        error!(error = %e, "failed to get L2 block number");
                        continue;
                    }
                };

                let cur = current_block.load(Ordering::Relaxed);
                if latest > cur {
                    for block_num in (cur + 1)..=latest {
                        let cache_guard = cache.read().await;
                        if let Some(fb_data) = cache_guard.get(&block_num) {
                            let fb_data = fb_data.clone();
                            drop(cache_guard);
                            validate_block(&provider, block_num, &fb_data, &metrics, &name).await;
                        } else {
                            drop(cache_guard);
                            warn!(block = block_num, "no flashblocks received for block");
                            metrics.count_with_tags(
                                metrics::FLASHBLOCK_NO_DATA_RECEIVED,
                                1,
                                &[("validator", name.as_str())],
                            );
                        }
                    }
                    current_block.store(latest, Ordering::Relaxed);
                }
            }
        }
    }
}

async fn validate_block<P: Provider>(
    provider: &P,
    block_number: u64,
    cache: &FlashblockData,
    metrics: &Metrics,
    name: &str,
) {
    let block = match provider.get_block_by_number(block_number.into()).full().await {
        Ok(Some(b)) => b,
        Ok(None) | Err(_) => return,
    };

    let l2_tx_hashes: HashSet<B256> = {
        use alloy_network::TransactionResponse as _;
        block.transactions.txns().map(|tx| tx.tx_hash()).collect()
    };

    let total_flashblocks = cache.flashblock_indices.len();
    let tags = [("validator", name)];

    metrics.gauge_with_tags(metrics::FLASHBLOCK_TOTAL_RECEIVED, total_flashblocks as f64, &tags);

    // Find missing indices (0..=10).
    let missing: Vec<u64> = (0..=10).filter(|i| !cache.flashblock_indices.contains(i)).collect();
    if !missing.is_empty() {
        metrics.count_with_tags(metrics::FLASHBLOCK_MISSING_INDICES, missing.len() as i64, &tags);
    }

    // Compare tx sets.
    let mut reorged_indices = HashSet::new();
    let mut reorged_txs = 0usize;

    for (tx_hash, &fb_index) in &cache.transaction_hashes {
        if !l2_tx_hashes.contains(tx_hash) {
            reorged_txs += 1;
            reorged_indices.insert(fb_index);
        }
    }

    let total_txs = cache.transaction_hashes.len();
    let included_txs = total_txs.saturating_sub(reorged_txs);

    metrics.count_with_tags(metrics::FLASHBLOCK_TX_TOTAL, total_txs as i64, &tags);
    metrics.count_with_tags(metrics::FLASHBLOCK_TX_REORGED, reorged_txs as i64, &tags);
    metrics.count_with_tags(metrics::FLASHBLOCK_TX_INCLUDED, included_txs as i64, &tags);

    let successful_flashblocks = total_flashblocks.saturating_sub(reorged_indices.len());
    metrics.count_with_tags(metrics::FLASHBLOCK_SUCCESS, successful_flashblocks as i64, &tags);
    metrics.count_with_tags(metrics::FLASHBLOCK_REORG, reorged_indices.len() as i64, &tags);

    if reorged_txs > 0 {
        metrics.count_with_tags(metrics::FLASHBLOCK_HASH_MISMATCH, 1, &tags);
    } else {
        metrics.count_with_tags(metrics::FLASHBLOCK_VALIDATION_SUCCESS, 1, &tags);
    }
}

// ── Cache cleanup ────────────────────────────────────────────

async fn cleanup_loop(cache: Arc<RwLock<HashMap<u64, FlashblockData>>>, cancel: CancellationToken) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = interval.tick() => {
                // Clean cache entries older than 20 s.
                let mut c = cache.write().await;
                let now = Instant::now();
                c.retain(|_, fb| now.duration_since(fb.last_updated) < Duration::from_secs(20));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_tx(fee: u128) -> FlashblockTransaction {
        FlashblockTransaction { effective_priority_fee: fee, is_user: true }
    }

    fn system_tx(fee: u128) -> FlashblockTransaction {
        FlashblockTransaction { effective_priority_fee: fee, is_user: false }
    }

    // ── validate_priority_fee_ordering ───────────────────────

    #[test]
    fn ordering_valid_no_violations() {
        let m = Metrics::noop();
        // Descending priority fees → no violations
        let txs = vec![user_tx(100), user_tx(80), user_tx(50), user_tx(10)];
        // Should not panic; will emit FLASHBLOCK_TX_ORDER_SUCCESS
        validate_priority_fee_ordering(1, 0, &txs, &m, "test");
    }

    #[test]
    fn ordering_violation_detected() {
        let m = Metrics::noop();
        // A tx with higher fee after a lower-fee tx → violation
        let txs = vec![user_tx(100), user_tx(50), user_tx(80)]; // 80 > 50 → violation
        validate_priority_fee_ordering(1, 0, &txs, &m, "test");
    }

    #[test]
    fn ordering_empty_txs() {
        let m = Metrics::noop();
        validate_priority_fee_ordering(1, 0, &[], &m, "test");
    }

    #[test]
    fn ordering_single_tx() {
        let m = Metrics::noop();
        validate_priority_fee_ordering(1, 0, &[user_tx(42)], &m, "test");
    }

    // ── emit_priority_fee_metrics ────────────────────────────

    #[test]
    fn emit_priority_fee_skips_high_index() {
        let m = Metrics::noop();
        let txs = vec![user_tx(100)];
        // Index > 10 → should return early and emit nothing (no panic)
        emit_priority_fee_metrics(11, &txs, &m, "test");
        emit_priority_fee_metrics(100, &txs, &m, "test");
    }

    #[test]
    fn emit_priority_fee_filters_system_txs() {
        let m = Metrics::noop();
        // Mix of user and system txs: only user txs should contribute to fee metrics
        let txs = vec![
            user_tx(100),
            system_tx(0), // Deposit tx — should be filtered
            user_tx(50),
        ];
        // Index within range → should emit metrics for user txs only (no panic)
        emit_priority_fee_metrics(0, &txs, &m, "test");
    }

    #[test]
    fn emit_priority_fee_boundary_index() {
        let m = Metrics::noop();
        let txs = vec![user_tx(42)];
        // Index exactly 10 → should emit
        emit_priority_fee_metrics(10, &txs, &m, "test");
    }

    #[test]
    fn emit_priority_fee_no_user_txs() {
        let m = Metrics::noop();
        let txs = vec![system_tx(0), system_tx(0)];
        // All system txs → should not panic, just emit 0 count
        emit_priority_fee_metrics(0, &txs, &m, "test");
    }
}
