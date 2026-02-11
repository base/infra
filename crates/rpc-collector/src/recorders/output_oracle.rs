//! L2 Output Oracle recorder — reports the latest L2 block number posted on L1.

use std::sync::Arc;

use alloy_primitives::Address;
use alloy_provider::Provider;
use alloy_rpc_types_eth::Block;
use async_trait::async_trait;

use crate::{
    bindings::L2OutputOracle,
    metrics::{self, Metrics},
    recorders::MetricRecorder,
};

/// Reports the latest block number from the `L2OutputOracle` contract.
#[derive(Debug)]
pub struct OutputOracleRecorder<P> {
    provider: Arc<P>,
    address: Address,
}

impl<P> OutputOracleRecorder<P> {
    pub const fn new(provider: Arc<P>, address: Address) -> Self {
        Self { provider, address }
    }
}

#[async_trait]
impl<P: Provider + Send + Sync + 'static> MetricRecorder for OutputOracleRecorder<P> {
    fn name(&self) -> &'static str {
        "output_oracle"
    }

    async fn record(&self, block: &Block, m: &Metrics) -> anyhow::Result<()> {
        let block_id = alloy_eips::BlockId::Number(block.header.number.into());
        let oracle = L2OutputOracle::new(self.address, &*self.provider);

        let ret = oracle.latestBlockNumber().block(block_id).call().await?;
        let block_number: u128 = ret.try_into().unwrap_or(u128::MAX);
        let _ = m.gauge(metrics::OUTPUT_ORACLE_LATEST_BLOCK, block_number as f64);

        Ok(())
    }
}
