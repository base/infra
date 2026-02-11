//! Dispute game factory recorder — reports the latest L2 block number from the
//! most recent dispute game.

use std::sync::Arc;

use alloy_primitives::Address;
use alloy_provider::Provider;
use alloy_rpc_types_eth::Block;
use async_trait::async_trait;

use crate::{
    bindings::{DisputeGameFactory, FaultDisputeGame},
    metrics::{self, Metrics},
    recorders::MetricRecorder,
};

/// Reports the latest L2 block from the newest dispute game.
#[derive(Debug)]
pub struct DisputeGameFactoryRecorder<P> {
    provider: Arc<P>,
    address: Address,
}

impl<P> DisputeGameFactoryRecorder<P> {
    pub const fn new(provider: Arc<P>, address: Address) -> Self {
        Self { provider, address }
    }
}

#[async_trait]
impl<P: Provider + Send + Sync + 'static> MetricRecorder for DisputeGameFactoryRecorder<P> {
    fn name(&self) -> &'static str {
        "dispute_game_factory"
    }

    async fn record(&self, block: &Block, m: &Metrics) -> anyhow::Result<()> {
        let block_id = alloy_eips::BlockId::Number(block.header.number.into());
        let factory = DisputeGameFactory::new(self.address, &*self.provider);

        // 1. Get total game count.
        let count_ret = factory.gameCount().block(block_id).call().await?;
        let game_count: u64 = count_ret.try_into().unwrap_or(0);
        if game_count == 0 {
            return Ok(());
        }

        // 2. Get the latest game.
        let game_ret = factory
            .gameAtIndex(alloy_primitives::U256::from(game_count - 1))
            .block(block_id)
            .call()
            .await?;

        // 3. Query the fault dispute game proxy for l2BlockNumber.
        let fault_game = FaultDisputeGame::new(game_ret.proxy, &*self.provider);
        let bn_ret = fault_game.l2BlockNumber().block(block_id).call().await?;
        let l2_block: u128 = bn_ret.try_into().unwrap_or(u128::MAX);

        let _ = m.gauge(metrics::OUTPUT_ORACLE_LATEST_BLOCK, l2_block as f64);

        Ok(())
    }
}
