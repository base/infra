//! Batch inbox recorder — counts data posted to the batch inbox address.

use alloy_consensus::Transaction as _;
use alloy_eips::Typed2718;
use alloy_network::TransactionResponse as _;
use alloy_primitives::Address;
use alloy_rpc_types_eth::Block;
use async_trait::async_trait;

use crate::{
    metrics::{self, Metrics},
    recorders::MetricRecorder,
};

/// Size of a single blob: 4096 field elements × 32 bytes.
const BLOB_SIZE: usize = 4096 * 32;

/// Reports the volume of data sent to the batch inbox, split by whether the
/// sender is the canonical batcher or not.
#[derive(Debug)]
pub struct BatchesInfoRecorder {
    batch_inbox: Address,
    batch_sender: Address,
}

impl BatchesInfoRecorder {
    pub const fn new(batch_inbox: Address, batch_sender: Address) -> Self {
        Self { batch_inbox, batch_sender }
    }
}

#[async_trait]
impl MetricRecorder for BatchesInfoRecorder {
    fn name(&self) -> &'static str {
        "batches_info"
    }

    async fn record(&self, block: &Block, m: &Metrics) -> anyhow::Result<()> {
        for tx in block.transactions.txns() {
            let to = match tx.to() {
                Some(addr) => addr,
                None => continue,
            };

            if to != self.batch_inbox {
                continue;
            }

            let data_len = if tx.ty() == alloy_consensus::TxType::Eip4844 as u8 {
                tx.blob_versioned_hashes()
                    .map_or(0, |h: &[alloy_primitives::B256]| h.len() * BLOB_SIZE)
            } else {
                tx.input().len()
            };

            let from = tx.from();
            if from == self.batch_sender {
                let _ = m.count(metrics::BATCH_SENT_BY_BATCH_SENDER, data_len as i64);
            } else {
                let _ = m.count(metrics::BATCH_SENT_BY_NON_BATCH_SENDER, data_len as i64);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::Bytes;

    use super::*;
    use crate::test_helpers::{make_block_with_txs, make_legacy_tx};

    const BATCH_INBOX: Address = Address::new([0xBB; 20]);
    const BATCH_SENDER: Address = Address::new([0xCC; 20]);
    const RANDOM_SENDER: Address = Address::new([0xDD; 20]);
    const RANDOM_ADDR: Address = Address::new([0xEE; 20]);

    fn recorder() -> BatchesInfoRecorder {
        BatchesInfoRecorder::new(BATCH_INBOX, BATCH_SENDER)
    }

    #[tokio::test]
    async fn counts_batcher_calldata() {
        let r = recorder();
        let m = Metrics::noop();
        let input = Bytes::from(vec![0u8; 100]);
        let tx = make_legacy_tx(BATCH_SENDER, BATCH_INBOX, input);
        let block = make_block_with_txs(1, vec![tx]);
        // Should not error; metric is emitted via BATCH_SENT_BY_BATCH_SENDER.
        assert!(r.record(&block, &m).await.is_ok());
    }

    #[tokio::test]
    async fn counts_non_batcher_calldata() {
        let r = recorder();
        let m = Metrics::noop();
        let input = Bytes::from(vec![0u8; 50]);
        let tx = make_legacy_tx(RANDOM_SENDER, BATCH_INBOX, input);
        let block = make_block_with_txs(1, vec![tx]);
        assert!(r.record(&block, &m).await.is_ok());
    }

    #[tokio::test]
    async fn ignores_tx_to_other_addresses() {
        let r = recorder();
        let m = Metrics::noop();
        let tx = make_legacy_tx(BATCH_SENDER, RANDOM_ADDR, Bytes::from(vec![0u8; 10]));
        let block = make_block_with_txs(1, vec![tx]);
        // Should succeed but emit no batch metrics.
        assert!(r.record(&block, &m).await.is_ok());
    }

    #[tokio::test]
    async fn empty_block_no_metrics() {
        let r = recorder();
        let m = Metrics::noop();
        let block = make_block_with_txs(1, vec![]);
        assert!(r.record(&block, &m).await.is_ok());
    }
}
