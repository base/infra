//! Gas price oracle recorder — fetches L1 base fee and blob base fee from the
//! `GasPriceOracle` L2 predeploy.

use std::sync::Arc;

use alloy_provider::Provider;
use alloy_rpc_types_eth::Block;
use async_trait::async_trait;

use crate::{
    bindings::{self, GasPriceOracle},
    metrics::{self, Metrics},
    recorders::MetricRecorder,
};

/// Reports L1 base fee and blob base fee from the `GasPriceOracle` contract.
#[derive(Debug)]
pub struct GasPriceOracleRecorder<P> {
    provider: Arc<P>,
}

impl<P> GasPriceOracleRecorder<P> {
    pub const fn new(provider: Arc<P>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl<P: Provider + Send + Sync + 'static> MetricRecorder for GasPriceOracleRecorder<P> {
    fn name(&self) -> &'static str {
        "gas_price_oracle"
    }

    async fn record(&self, block: &Block, m: &Metrics) -> anyhow::Result<()> {
        let block_id = alloy_eips::BlockId::Number(block.header.number.into());
        let oracle = GasPriceOracle::new(bindings::GAS_PRICE_ORACLE, &*self.provider);

        // L1 base fee
        match oracle.l1BaseFee().block(block_id).call().await {
            Ok(ret) => {
                let fee: u128 = ret.try_into().unwrap_or(u128::MAX);
                let _ = m.gauge(metrics::L1_BASE_FEE, fee as f64);
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to call GasPriceOracle.l1BaseFee");
            }
        }

        // Blob base fee
        match oracle.blobBaseFee().block(block_id).call().await {
            Ok(ret) => {
                let fee: u128 = ret.try_into().unwrap_or(u128::MAX);
                let _ = m.gauge(metrics::L1_BLOB_BASE_FEE, fee as f64);
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to call GasPriceOracle.blobBaseFee");
            }
        }

        Ok(())
    }
}
