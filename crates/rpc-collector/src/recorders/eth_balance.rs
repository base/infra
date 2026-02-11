//! ETH and contract balance recorder.

use std::sync::Arc;

use alloy_primitives::{Address, Bytes, U256, utils::Unit};
use alloy_provider::Provider;
use alloy_rpc_types_eth::{Block, TransactionRequest};
use async_trait::async_trait;

use crate::{metrics::Metrics, recorders::MetricRecorder, utils::wei_to_unit};

/// Reports the ETH or contract balance of an address as a gauge.
#[derive(Debug)]
pub struct EthBalanceRecorder<P> {
    provider: Arc<P>,
    address: Address,
    metric_name: String,
    calldata: Option<Vec<u8>>,
}

impl<P> EthBalanceRecorder<P> {
    /// Create a recorder for a plain ETH balance.
    pub const fn eth(provider: Arc<P>, address: Address, metric_name: String) -> Self {
        Self { provider, address, metric_name, calldata: None }
    }

    /// Create a recorder for a contract balance (calls the contract with `calldata`).
    pub const fn contract(
        provider: Arc<P>,
        address: Address,
        metric_name: String,
        calldata: Vec<u8>,
    ) -> Self {
        Self { provider, address, metric_name, calldata: Some(calldata) }
    }
}

#[async_trait]
impl<P: Provider + Send + Sync + 'static> MetricRecorder for EthBalanceRecorder<P> {
    fn name(&self) -> &'static str {
        "eth_balance"
    }

    async fn record(&self, block: &Block, m: &Metrics) -> anyhow::Result<()> {
        let block_id = alloy_eips::BlockId::Number(block.header.number.into());

        let balance = match &self.calldata {
            None => self.provider.get_balance(self.address).block_id(block_id).await?,
            Some(cd) => {
                let tx = TransactionRequest::default()
                    .to(self.address)
                    .input(Bytes::copy_from_slice(cd).into());
                let result = self.provider.call(tx).block(block_id).await?;
                U256::from_be_slice(&result)
            }
        };

        let _ = m.gauge(&self.metric_name, wei_to_unit(balance, Unit::ETHER));
        Ok(())
    }
}
