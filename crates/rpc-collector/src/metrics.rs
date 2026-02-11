//! `StatsD` metrics client wrapper and metric name constants.

use std::{net::UdpSocket, sync::Arc};

use cadence::{
    BufferedUdpMetricSink, Counted, CountedExt, Gauged, Histogrammed, QueuingMetricSink,
    StatsdClient,
};

// ── Metric name constants ────────────────────────────────────

// Block / gas metrics
pub const BLOCK_GAS_LIMIT: &str = "base.gas.limit";
pub const BLOCK_GAS_USED: &str = "base.gas.used";
pub const BLOCK_GAS_TARGET: &str = "base.gas.target";
pub const BLOCK_BASE_GAS_FEE: &str = "base.gas.base.fee";
pub const MIN_BASE_FEE: &str = "base.gas.minbasefee";
pub const DA_FOOTPRINT: &str = "base.gas.dafootprint";
pub const DA_FOOTPRINT_GAS_SCALAR: &str = "base.gas.dafootprintscalar";
pub const ELASTICITY: &str = "base.elasticity";

// Collector progress
pub const L2_LATEST_BLOCK: &str = "base.l2.collector.block";
pub const L1_LATEST_BLOCK: &str = "base.l1.collector.block";

// Transactions
pub const TRANSACTION_COUNT: &str = "base.transactions";
pub const TRANSACTION_TYPE: &str = "base.txn.type";
pub const TRANSACTION_GAS_PRICE: &str = "base.txn.gas.price";
pub const TRANSACTION_GAS_PRICE_MAX: &str = "base.txn.gas.maximum";
pub const TRANSACTION_MAX_PRIORITY_FEE: &str = "base.txn.gas.priorityfee";
pub const TRANSACTION_TOTAL_PRI_FEES: &str = "base.txn.gas.totalpriorityfee";
pub const TRANSACTION_L2_FEES: &str = "base.txn.l2.fees";
pub const TRANSACTIONS_SUCCESS: &str = "base.l2.txn.success";
pub const TRANSACTIONS_FAILED: &str = "base.l2.txn.failed";
pub const EMPTY_BLOCKS: &str = "base.blocks.empty";

// Vault balances
pub const SEQUENCER_BALANCE: &str = "base.sequencer.vault.balance";
pub const BASE_FEE_BALANCE: &str = "base.base.fee.vault.balance";
pub const L1_FEE_BALANCE: &str = "base.l1.fee.vault.balance";

// Bridge events
pub const WITHDRAWAL_INITIATED_COUNT: &str = "base.withdrawal.initiated.count";
pub const WITHDRAWAL_FINALIZED_COUNT: &str = "base.withdrawal.finalized.count";
pub const DEPOSIT_INITIATED_COUNT: &str = "base.deposit.initiated.count";
pub const DEPOSIT_FINALIZED_COUNT: &str = "base.deposit.finalized.count";

// Gas price oracle
pub const L1_BASE_FEE: &str = "base.l1.base.fee";
pub const L1_BLOB_BASE_FEE: &str = "base.l1.blob.base.fee";

// Proposer / batcher
pub const PROPOSER_BALANCE: &str = "base.proposer.balance";
pub const BATCH_BALANCE: &str = "base.batch.balance";

// Output oracle
pub const OUTPUT_ORACLE_LATEST_BLOCK: &str = "base.output.latest.block";

// Batch inbox
pub const BATCH_SENT_BY_NON_BATCH_SENDER: &str = "base.batch.inbox.unknown";
pub const BATCH_SENT_BY_BATCH_SENDER: &str = "base.batch.inbox.batcher";

// Fee disburser / balance tracker
pub const FEES_DISBURSED_COUNT: &str = "base.fees.disbursed.count";
pub const RECEIVED_FUNDS_COUNT: &str = "base.received.funds.count";
pub const SENT_PROFIT_COUNT: &str = "base.sent.profit.count";

// Reorg
pub const REORG_DEPTH: &str = "base.reorg.depth";

// Snapshots
pub const SNAPSHOT_ERROR: &str = "base.snapshot.error";
pub const SNAPSHOT_SUCCESS: &str = "base.snapshot.time";
pub const SNAPSHOT_AGE: &str = "base.snapshot.age";
pub const SNAPSHOT_SIZE: &str = "base.snapshot.size";

// Flashblocks
pub const FLASHBLOCK_HASH_MISMATCH: &str = "base.flashblock.hash.mismatch";
pub const FLASHBLOCK_VALIDATION_SUCCESS: &str = "base.flashblock.validation.success";
pub const FLASHBLOCK_MISSING_INDICES: &str = "base.flashblock.missing.indices";
pub const FLASHBLOCK_TOTAL_RECEIVED: &str = "base.flashblock.total.received";
pub const FLASHBLOCK_NO_DATA_RECEIVED: &str = "base.flashblock.no.data.received";
pub const FLASHBLOCK_SUCCESS: &str = "base.flashblock.success";
pub const FLASHBLOCK_REORG: &str = "base.flashblock.reorg";
pub const FLASHBLOCK_TX_REORGED: &str = "base.flashblock.tx.reorged";
pub const FLASHBLOCK_TX_INCLUDED: &str = "base.flashblock.tx.included";
pub const FLASHBLOCK_TX_TOTAL: &str = "base.flashblock.tx.total";
pub const FLASHBLOCK_TX_ORDER_VIOLATION: &str = "base.flashblock.tx.order.violation";
pub const FLASHBLOCK_TX_ORDER_SUCCESS: &str = "base.flashblock.tx.order.success";
pub const FLASHBLOCK_PRIORITY_FEE_MIN: &str = "base.flashblock.priorityfee.min";
pub const FLASHBLOCK_PRIORITY_FEE_TX_COUNT: &str = "base.flashblock.priorityfee.tx.count";
pub const FLASHBLOCK_PRIORITY_FEE_TIP: &str = "base.flashblock.priorityfee.tip";

// Mempool
pub const MEMPOOL_PENDING_COUNT: &str = "base.mempool.pending.count";
pub const MEMPOOL_QUEUED_COUNT: &str = "base.mempool.queued.count";
pub const MEMPOOL_ERROR: &str = "base.mempool.error";
pub const MEMPOOL_COLLECTION_SUCCESS: &str = "base.mempool.collection.success";
pub const MEMPOOL_RECLASSIFIED_COUNT: &str = "base.mempool.reclassified.count";
pub const MEMPOOL_ORIGINAL_PENDING_COUNT: &str = "base.mempool.original.pending.count";
pub const MEMPOOL_PENDING_DIFF: &str = "base.mempool.pending.diff";
pub const MEMPOOL_QUEUED_DIFF: &str = "base.mempool.queued.diff";
pub const MEMPOOL_TOTAL_DIFF: &str = "base.mempool.total.diff";

// Node health
pub const NODE_LATEST_BLOCK: &str = "base.node.latest.num";
pub const NODE_LATEST_TIME: &str = "base.node.latest.time";
pub const NODE_HEALTHY: &str = "base.node.healthy";
pub const NODE_ERROR: &str = "base.node.error";
pub const NODE_STALL: &str = "base.node.stall";
pub const NODE_RATE_LIMITED: &str = "base.node.ratelimited";
pub const SEQUENCER_LATEST_BLOCK: &str = "base.sequencer.latest.num";
pub const SEQUENCER_DELTA: &str = "base.node.sequencer.delta";

// Divergence
pub const DIVERGENCE_BLOCK_PROCESSED: &str = "base.divergence.block.processed";
pub const DIVERGENCE_CROSS_GROUP_DETECTED: &str = "base.divergence.cross.group.detected";
pub const DIVERGENCE_GETH_TIMEOUT: &str = "base.divergence.geth.timeout";
pub const DIVERGENCE_NODE_ERROR: &str = "base.divergence.node.error";

// ── StatsD client wrapper ────────────────────────────────────

/// Thin wrapper around [`cadence::StatsdClient`].
///
/// For untagged metrics, use the inherent `gauge`, `count`, and `histogram`
/// methods. For tagged metrics, use the `*_with_tags` convenience methods.
#[derive(Clone, Debug)]
pub struct Metrics {
    inner: Arc<StatsdClient>,
}

impl Metrics {
    /// Create a new [`Metrics`] client connected to the given `StatsD` endpoint.
    pub fn new(host: &str, port: u16, prefix: &str) -> anyhow::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        let addr = format!("{host}:{port}");
        let udp_sink = BufferedUdpMetricSink::from(addr.as_str(), socket)?;
        let queuing_sink = QueuingMetricSink::from(udp_sink);
        let client = StatsdClient::from_sink(prefix, queuing_sink);
        Ok(Self { inner: Arc::new(client) })
    }

    /// Create a no-op metrics client that silently drops all metrics.
    pub fn noop() -> Self {
        Self { inner: Arc::new(StatsdClient::from_sink("", cadence::NopMetricSink)) }
    }

    // ── Untagged metric helpers ────────────────────────────

    pub fn gauge(&self, key: &str, value: f64) -> cadence::MetricResult<cadence::Gauge> {
        self.inner.gauge(key, value)
    }

    pub fn count(&self, key: &str, value: i64) -> cadence::MetricResult<cadence::Counter> {
        self.inner.count(key, value)
    }

    pub fn histogram(&self, key: &str, value: f64) -> cadence::MetricResult<cadence::Histogram> {
        self.inner.histogram(key, value)
    }

    // ── Tagged convenience helpers ───────────────────────────

    pub fn gauge_with_tags(&self, name: &str, value: f64, tags: &[(&str, &str)]) {
        let mut builder = self.inner.gauge_with_tags(name, value);
        for &(k, v) in tags {
            builder = builder.with_tag(k, v);
        }
        builder.send();
    }

    pub fn count_with_tags(&self, name: &str, value: i64, tags: &[(&str, &str)]) {
        let mut builder = self.inner.count_with_tags(name, value);
        for &(k, v) in tags {
            builder = builder.with_tag(k, v);
        }
        builder.send();
    }

    pub fn incr_with_tags(&self, name: &str, tags: &[(&str, &str)]) {
        let mut builder = self.inner.incr_with_tags(name);
        for &(k, v) in tags {
            builder = builder.with_tag(k, v);
        }
        builder.send();
    }

    pub fn histogram_with_tags(&self, name: &str, value: f64, tags: &[(&str, &str)]) {
        let mut builder = self.inner.histogram_with_tags(name, value);
        for &(k, v) in tags {
            builder = builder.with_tag(k, v);
        }
        builder.send();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_does_not_panic() {
        let m = Metrics::noop();

        // Untagged helpers
        let _ = m.gauge("test.gauge", 42.0);
        let _ = m.count("test.count", 1);
        let _ = m.histogram("test.histogram", 2.78);

        // Tagged helpers
        let tags = [("env", "test"), ("layer", "l2")];
        m.gauge_with_tags("test.gauge.tagged", 1.0, &tags);
        m.count_with_tags("test.count.tagged", 1, &tags);
        m.incr_with_tags("test.incr.tagged", &tags);
        m.histogram_with_tags("test.histogram.tagged", 2.0, &tags);
    }
}
