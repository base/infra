//! Mempool listener — polls `txpool_content` from Geth and Reth nodes,
//! reclassifies underpriced Geth transactions, and compares snapshots.

use std::{collections::HashMap, time::Duration};

use alloy_provider::Provider;
use serde::Deserialize;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::{
    metrics::{self, Metrics},
    services::Service,
};

// ── Types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct MempoolTransaction {
    #[serde(rename = "gasPrice", default)]
    gas_price: String,
    #[serde(rename = "maxFeePerGas", default)]
    max_fee_per_gas: String,
    #[serde(rename = "type", default)]
    tx_type: String,
}

/// `txpool_content` response shape: `{ pending: { addr: { nonce: tx } }, queued: ... }`.
type TxpoolContent = HashMap<String, HashMap<String, HashMap<String, MempoolTransaction>>>;
/// Pool category: `"pending"` or `"queued"`.
type TxPool = HashMap<String, HashMap<String, MempoolTransaction>>;

/// Count total transactions across all addresses in a pool.
fn count_pool_txs(pool: Option<&TxPool>) -> i64 {
    pool.map_or(0, |p| p.values().map(|txs| txs.len() as i64).sum())
}

#[derive(Debug, Default)]
struct MempoolSnapshot {
    pending_count: i64,
    queued_count: i64,
    reclassified_count: i64,
    original_pending_count: i64,
}

// ── Service ──────────────────────────────────────────────────

/// The mempool listener service, implementing [`Service`].
#[derive(Debug)]
pub struct MempoolListenerService<P> {
    /// Geth provider for `txpool_content`.
    pub geth: P,
    /// Reth provider for `txpool_content`.
    pub reth: P,
    /// Interval between mempool polls.
    pub poll_interval: Duration,
    /// Metrics client.
    pub metrics: Metrics,
    /// Human-readable listener name.
    pub listener_name: String,
}

impl<P: Provider + Send + Sync + 'static> Service for MempoolListenerService<P> {
    fn name(&self) -> &str {
        &self.listener_name
    }

    fn spawn(self: Box<Self>, set: &mut JoinSet<()>, cancel: CancellationToken) {
        let name = self.listener_name.clone();
        set.spawn(async move {
            if let Err(e) = run(
                self.geth,
                self.reth,
                self.poll_interval,
                self.metrics,
                self.listener_name,
                cancel,
            )
            .await
            {
                error!(listener = name, error = %e, "mempool listener failed");
            }
        });
    }
}

// ── Core logic ───────────────────────────────────────────────

/// Run the mempool listener loop. Both Geth and Reth providers are required.
async fn run<P: Provider + Send + Sync + 'static>(
    geth: P,
    reth: P,
    poll_interval: Duration,
    metrics: Metrics,
    listener_name: String,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    info!(listener = listener_name, "starting mempool listener");

    let mut interval = tokio::time::interval(poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!(listener = listener_name, "mempool listener shutting down");
                return Ok(());
            }
            _ = interval.tick() => {
                collect_and_compare(&geth, &reth, &metrics, &listener_name).await;
            }
        }
    }
}

async fn collect_and_compare<P: Provider>(
    geth: &P,
    reth: &P,
    metrics: &Metrics,
    listener_name: &str,
) {
    // Fetch base fee from Geth (needed for reclassification).
    let base_fee: Option<u128> = match geth
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
        .await
    {
        Ok(Some(block)) => block.header.base_fee_per_gas.map(|b| b as u128),
        _ => {
            metrics.count_with_tags(
                metrics::MEMPOOL_ERROR,
                1,
                &[("listener", listener_name), ("client", "geth"), ("error", "base_fee_failed")],
            );
            None
        }
    };

    let geth_snap = collect_geth_snapshot(geth, base_fee, metrics, listener_name).await;
    let reth_snap = collect_reth_snapshot(reth, metrics, listener_name).await;

    if let Some(ref snap) = geth_snap {
        report_snapshot(snap, "geth", metrics, listener_name);
    }
    if let Some(ref snap) = reth_snap {
        report_snapshot(snap, "reth", metrics, listener_name);
    }

    if let (Some(g), Some(r)) = (&geth_snap, &reth_snap) {
        compare_snapshots(g, r, metrics, listener_name);
    }
}

async fn collect_geth_snapshot<P: Provider>(
    provider: &P,
    base_fee: Option<u128>,
    metrics: &Metrics,
    listener: &str,
) -> Option<MempoolSnapshot> {
    let content: TxpoolContent = match provider.client().request_noparams("txpool_content").await {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "failed to get Geth txpool_content");
            metrics.count_with_tags(
                metrics::MEMPOOL_ERROR,
                1,
                &[("listener", listener), ("client", "geth"), ("error", "snapshot_failed")],
            );
            return None;
        }
    };

    let mut snap = MempoolSnapshot::default();

    if let Some(pending) = content.get("pending") {
        for txs_by_nonce in pending.values() {
            let addr_count = txs_by_nonce.len() as i64;
            snap.original_pending_count += addr_count;

            if let Some(bf) = base_fee {
                let result = reclassify_for_address(txs_by_nonce, bf);
                snap.pending_count += result.0;
                snap.reclassified_count += result.1;
            } else {
                snap.pending_count += addr_count;
            }
        }
    }

    snap.queued_count = count_pool_txs(content.get("queued")) + snap.reclassified_count;
    Some(snap)
}

async fn collect_reth_snapshot<P: Provider>(
    provider: &P,
    metrics: &Metrics,
    listener: &str,
) -> Option<MempoolSnapshot> {
    let content: TxpoolContent = match provider.client().request_noparams("txpool_content").await {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "failed to get Reth txpool_content");
            metrics.count_with_tags(
                metrics::MEMPOOL_ERROR,
                1,
                &[("listener", listener), ("client", "reth"), ("error", "snapshot_failed")],
            );
            return None;
        }
    };

    Some(MempoolSnapshot {
        pending_count: count_pool_txs(content.get("pending")),
        queued_count: count_pool_txs(content.get("queued")),
        ..Default::default()
    })
}

/// Geth reclassification: returns `(remaining_pending, reclassified)`.
fn reclassify_for_address(
    txs_by_nonce: &HashMap<String, MempoolTransaction>,
    base_fee: u128,
) -> (i64, i64) {
    let total = txs_by_nonce.len() as i64;
    if total == 0 {
        return (0, 0);
    }

    // Sort by nonce.
    let mut nonces: Vec<(u64, &MempoolTransaction)> = txs_by_nonce
        .iter()
        .filter_map(|(nonce_str, tx)| {
            let nonce_str = nonce_str.strip_prefix("0x").unwrap_or(nonce_str);
            u64::from_str_radix(nonce_str, 16).ok().map(|n| (n, tx))
        })
        .collect();
    nonces.sort_by_key(|(n, _)| *n);

    if nonces.is_empty() {
        return (total, 0);
    }

    // Find first underpriced tx — everything from there on is reclassified.
    nonces.iter().position(|(_, tx)| is_underpriced(tx, base_fee)).map_or((total, 0), |idx| {
        let pending = idx as i64;
        (pending, nonces.len() as i64 - pending)
    })
}

fn is_underpriced(tx: &MempoolTransaction, base_fee: u128) -> bool {
    let gas_price_str = if tx.tx_type == "0x2" && !tx.max_fee_per_gas.is_empty() {
        &tx.max_fee_per_gas
    } else {
        &tx.gas_price
    };

    if gas_price_str.is_empty() {
        return false;
    }

    let hex = gas_price_str.strip_prefix("0x").unwrap_or(gas_price_str);
    u128::from_str_radix(hex, 16).is_ok_and(|price| price < base_fee)
}

fn report_snapshot(snap: &MempoolSnapshot, client: &str, metrics: &Metrics, listener: &str) {
    let tags = [("listener", listener), ("client", client)];
    metrics.gauge_with_tags(metrics::MEMPOOL_PENDING_COUNT, snap.pending_count as f64, &tags);
    metrics.gauge_with_tags(metrics::MEMPOOL_QUEUED_COUNT, snap.queued_count as f64, &tags);

    if client == "geth" {
        metrics.gauge_with_tags(
            metrics::MEMPOOL_RECLASSIFIED_COUNT,
            snap.reclassified_count as f64,
            &tags,
        );
        metrics.gauge_with_tags(
            metrics::MEMPOOL_ORIGINAL_PENDING_COUNT,
            snap.original_pending_count as f64,
            &tags,
        );
    }

    metrics.count_with_tags(metrics::MEMPOOL_COLLECTION_SUCCESS, 1, &tags);
}

fn compare_snapshots(
    geth: &MempoolSnapshot,
    reth: &MempoolSnapshot,
    metrics: &Metrics,
    listener: &str,
) {
    let pending_diff = geth.pending_count - reth.pending_count;
    let queued_diff = geth.queued_count - reth.queued_count;
    let total_geth = geth.pending_count + geth.queued_count;
    let total_reth = reth.pending_count + reth.queued_count;
    let total_diff = total_geth - total_reth;

    let tags = [("listener", listener)];
    metrics.gauge_with_tags(metrics::MEMPOOL_PENDING_DIFF, pending_diff as f64, &tags);
    metrics.gauge_with_tags(metrics::MEMPOOL_QUEUED_DIFF, queued_diff as f64, &tags);
    metrics.gauge_with_tags(metrics::MEMPOOL_TOTAL_DIFF, total_diff as f64, &tags);

    info!(
        listener,
        geth_pending = geth.pending_count,
        reth_pending = reth.pending_count,
        pending_diff,
        geth_queued = geth.queued_count,
        reth_queued = reth.queued_count,
        queued_diff,
        total_diff,
        "mempool comparison"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── count_pool_txs ───────────────────────────────────────

    #[test]
    fn count_pool_txs_none() {
        assert_eq!(count_pool_txs(None), 0);
    }

    #[test]
    fn count_pool_txs_empty_map() {
        let pool: TxPool = HashMap::new();
        assert_eq!(count_pool_txs(Some(&pool)), 0);
    }

    #[test]
    fn count_pool_txs_counts_all() {
        let mut pool: TxPool = HashMap::new();
        // Address "0xAA" with 2 txs
        let mut aa_txs = HashMap::new();
        aa_txs.insert(
            "0x0".to_string(),
            MempoolTransaction {
                gas_price: "0x3b9aca00".to_string(),
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );
        aa_txs.insert(
            "0x1".to_string(),
            MempoolTransaction {
                gas_price: "0x3b9aca00".to_string(),
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );
        pool.insert("0xAA".to_string(), aa_txs);
        // Address "0xBB" with 1 tx
        let mut bb_txs = HashMap::new();
        bb_txs.insert(
            "0x0".to_string(),
            MempoolTransaction {
                gas_price: "0x3b9aca00".to_string(),
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );
        pool.insert("0xBB".to_string(), bb_txs);

        assert_eq!(count_pool_txs(Some(&pool)), 3);
    }

    // ── is_underpriced ───────────────────────────────────────

    #[test]
    fn is_underpriced_type2_below_base() {
        // EIP-1559 tx with max_fee < base_fee → underpriced
        let tx = MempoolTransaction {
            gas_price: String::new(),
            max_fee_per_gas: "0x5".to_string(), // 5 wei
            tx_type: "0x2".to_string(),
        };
        assert!(is_underpriced(&tx, 10)); // base_fee = 10
    }

    #[test]
    fn is_underpriced_type2_above_base() {
        let tx = MempoolTransaction {
            gas_price: String::new(),
            max_fee_per_gas: "0x14".to_string(), // 20 wei
            tx_type: "0x2".to_string(),
        };
        assert!(!is_underpriced(&tx, 10));
    }

    #[test]
    fn is_underpriced_legacy_above_base() {
        let tx = MempoolTransaction {
            gas_price: "0x3b9aca00".to_string(), // 1 Gwei
            max_fee_per_gas: String::new(),
            tx_type: "0x0".to_string(),
        };
        assert!(!is_underpriced(&tx, 1_000)); // base_fee = 1000 wei
    }

    #[test]
    fn is_underpriced_legacy_below_base() {
        let tx = MempoolTransaction {
            gas_price: "0x1".to_string(), // 1 wei
            max_fee_per_gas: String::new(),
            tx_type: "0x0".to_string(),
        };
        assert!(is_underpriced(&tx, 1_000)); // base_fee = 1000 wei
    }

    #[test]
    fn is_underpriced_empty_gas_price() {
        let tx = MempoolTransaction {
            gas_price: String::new(),
            max_fee_per_gas: String::new(),
            tx_type: "0x0".to_string(),
        };
        // Empty gas price → not underpriced (can't parse)
        assert!(!is_underpriced(&tx, 10));
    }

    // ── reclassify_for_address ───────────────────────────────

    #[test]
    fn reclassify_all_priced_ok() {
        let mut txs: HashMap<String, MempoolTransaction> = HashMap::new();
        // Two txs both above base_fee
        txs.insert(
            "0x0".to_string(),
            MempoolTransaction {
                gas_price: "0x64".to_string(), // 100 wei
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );
        txs.insert(
            "0x1".to_string(),
            MempoolTransaction {
                gas_price: "0xc8".to_string(), // 200 wei
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );

        let (pending, reclassified) = reclassify_for_address(&txs, 50); // base_fee = 50
        assert_eq!(pending, 2);
        assert_eq!(reclassified, 0);
    }

    #[test]
    fn reclassify_first_underpriced_cascades() {
        let mut txs: HashMap<String, MempoolTransaction> = HashMap::new();
        // Nonce 0: underpriced (1 wei < 50 base fee)
        txs.insert(
            "0x0".to_string(),
            MempoolTransaction {
                gas_price: "0x1".to_string(),
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );
        // Nonce 1: properly priced but after an underpriced one → still reclassified
        txs.insert(
            "0x1".to_string(),
            MempoolTransaction {
                gas_price: "0x64".to_string(),
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );
        // Nonce 2: also properly priced
        txs.insert(
            "0x2".to_string(),
            MempoolTransaction {
                gas_price: "0xc8".to_string(),
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );

        let (pending, reclassified) = reclassify_for_address(&txs, 50);
        // First tx (nonce 0) is underpriced → all 3 reclassified, 0 pending
        assert_eq!(pending, 0);
        assert_eq!(reclassified, 3);
    }

    #[test]
    fn reclassify_middle_underpriced_cascades() {
        let mut txs: HashMap<String, MempoolTransaction> = HashMap::new();
        // Nonce 0: properly priced
        txs.insert(
            "0x0".to_string(),
            MempoolTransaction {
                gas_price: "0x64".to_string(),
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );
        // Nonce 1: underpriced
        txs.insert(
            "0x1".to_string(),
            MempoolTransaction {
                gas_price: "0x1".to_string(),
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );
        // Nonce 2: properly priced but after an underpriced one
        txs.insert(
            "0x2".to_string(),
            MempoolTransaction {
                gas_price: "0x64".to_string(),
                max_fee_per_gas: String::new(),
                tx_type: "0x0".to_string(),
            },
        );

        let (pending, reclassified) = reclassify_for_address(&txs, 50);
        assert_eq!(pending, 1); // Only nonce 0 is pending
        assert_eq!(reclassified, 2); // Nonce 1 and 2 reclassified
    }

    #[test]
    fn reclassify_empty_map() {
        let txs: HashMap<String, MempoolTransaction> = HashMap::new();
        let (pending, reclassified) = reclassify_for_address(&txs, 50);
        assert_eq!(pending, 0);
        assert_eq!(reclassified, 0);
    }

    // ── compare_snapshots ────────────────────────────────────

    #[test]
    fn compare_snapshots_diffs() {
        let m = Metrics::noop();
        let geth = MempoolSnapshot {
            pending_count: 100,
            queued_count: 50,
            reclassified_count: 10,
            original_pending_count: 110,
        };
        let reth = MempoolSnapshot { pending_count: 80, queued_count: 30, ..Default::default() };

        // This should not panic. The diffs are:
        // pending_diff = 100 - 80 = 20
        // queued_diff = 50 - 30 = 20
        // total_diff = (100+50) - (80+30) = 40
        compare_snapshots(&geth, &reth, &m, "test_listener");
    }

    #[test]
    fn compare_snapshots_negative_diff() {
        let m = Metrics::noop();
        let geth = MempoolSnapshot { pending_count: 10, queued_count: 5, ..Default::default() };
        let reth = MempoolSnapshot { pending_count: 50, queued_count: 30, ..Default::default() };

        // Negative diffs should not panic.
        compare_snapshots(&geth, &reth, &m, "test_listener");
    }

    // ── report_snapshot ──────────────────────────────────────

    #[test]
    fn report_snapshot_does_not_panic() {
        let m = Metrics::noop();
        let snap = MempoolSnapshot {
            pending_count: 42,
            queued_count: 13,
            reclassified_count: 5,
            original_pending_count: 47,
        };
        report_snapshot(&snap, "geth", &m, "test_listener");
        report_snapshot(&snap, "reth", &m, "test_listener");
    }
}
