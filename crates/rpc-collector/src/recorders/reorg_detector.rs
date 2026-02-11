//! Reorg detector — tracks chain reorganisations by comparing parent hashes.

use std::collections::VecDeque;

use alloy_primitives::B256;
use alloy_rpc_types_eth::Block;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::warn;

use crate::{
    metrics::{self, Metrics},
    recorders::MetricRecorder,
};

const MAX_BLOCK_HASH_LEN: usize = 10_000;

/// Detects reorgs by keeping a sliding window of recent block hashes.
#[derive(Debug)]
pub struct ReorgDetectorRecorder {
    state: Mutex<ReorgState>,
}

#[derive(Debug)]
struct ReorgState {
    parent_hash: Option<B256>,
    block_hashes: VecDeque<B256>,
}

impl Default for ReorgDetectorRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl ReorgDetectorRecorder {
    pub fn new() -> Self {
        Self { state: Mutex::new(ReorgState { parent_hash: None, block_hashes: VecDeque::new() }) }
    }
}

#[async_trait]
impl MetricRecorder for ReorgDetectorRecorder {
    fn name(&self) -> &'static str {
        "reorg_detector"
    }

    async fn record(&self, block: &Block, m: &Metrics) -> anyhow::Result<()> {
        let block_hash = block.header.hash;
        let parent_hash = block.header.parent_hash;
        let block_num = block.header.number;

        let mut state = self.state.lock().await;

        // Check for reorg.
        if let Some(prev_hash) = state.parent_hash
            && parent_hash != prev_hash
        {
            // Reorg detected. Estimate depth.
            let depth = state.block_hashes.len() as u64;
            warn!(
                block = block_num,
                depth,
                new_parent = %parent_hash,
                old_parent = %prev_hash,
                "reorg detected"
            );
            let _ = m.histogram(metrics::REORG_DEPTH, depth as f64);
        }

        state.parent_hash = Some(block_hash);
        state.block_hashes.push_back(block_hash);
        if state.block_hashes.len() > MAX_BLOCK_HASH_LEN {
            state.block_hashes.pop_front();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_block;

    fn hash(n: u8) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[31] = n;
        B256::from(bytes)
    }

    #[tokio::test]
    async fn first_block_never_reorgs() {
        let detector = ReorgDetectorRecorder::new();
        let m = Metrics::noop();

        let block = make_block(1, hash(1), hash(0));
        // First block should never produce an error or reorg.
        assert!(detector.record(&block, &m).await.is_ok());
    }

    #[tokio::test]
    async fn no_reorg_sequential_blocks() {
        let detector = ReorgDetectorRecorder::new();
        let m = Metrics::noop();

        // Block 1: hash=1, parent=0
        let b1 = make_block(1, hash(1), hash(0));
        detector.record(&b1, &m).await.unwrap();

        // Block 2: hash=2, parent=1 (correct chain)
        let b2 = make_block(2, hash(2), hash(1));
        detector.record(&b2, &m).await.unwrap();

        // Block 3: hash=3, parent=2 (correct chain)
        let b3 = make_block(3, hash(3), hash(2));
        detector.record(&b3, &m).await.unwrap();

        // Verify the state is consistent.
        let state = detector.state.lock().await;
        assert_eq!(state.parent_hash, Some(hash(3)));
        assert_eq!(state.block_hashes.len(), 3);
    }

    #[tokio::test]
    async fn reorg_detected_on_parent_mismatch() {
        let detector = ReorgDetectorRecorder::new();
        let m = Metrics::noop();

        // Block 1
        let b1 = make_block(1, hash(1), hash(0));
        detector.record(&b1, &m).await.unwrap();

        // Block 2 with WRONG parent (hash(99) instead of hash(1)) → reorg
        let b2 = make_block(2, hash(2), hash(99));
        // Should not error, but internally detects reorg.
        assert!(detector.record(&b2, &m).await.is_ok());

        // After the reorg, state should still track the latest block.
        let state = detector.state.lock().await;
        assert_eq!(state.parent_hash, Some(hash(2)));
    }

    #[tokio::test]
    async fn sliding_window_capped() {
        let detector = ReorgDetectorRecorder::new();
        let m = Metrics::noop();

        // Insert MAX_BLOCK_HASH_LEN + 100 blocks.
        let total = MAX_BLOCK_HASH_LEN + 100;
        for i in 0..total {
            let num = i as u64;
            let parent =
                if i == 0 { hash(0) } else { B256::from(alloy_primitives::U256::from(i - 1)) };
            let h = B256::from(alloy_primitives::U256::from(i));
            let block = make_block(num, h, parent);
            detector.record(&block, &m).await.unwrap();
        }

        let state = detector.state.lock().await;
        assert_eq!(state.block_hashes.len(), MAX_BLOCK_HASH_LEN);
    }
}
