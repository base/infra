//! L2 block info recorder — the richest per-block metric recorder.
//!
//! Emits gas usage/limits/target, base fee, transaction type breakdown,
//! priority fees, DA footprint, empty block counts and transaction statuses.

use std::sync::Arc;

use alloy_consensus::Transaction;
use alloy_eips::Typed2718;
use alloy_network::TransactionResponse;
use alloy_primitives::{Address, U256, utils::Unit};
use alloy_provider::Provider;
use alloy_rpc_types_eth::Block;
use async_trait::async_trait;
use tokio::task::JoinSet;
use tracing::{error, warn};

use crate::{
    bindings,
    metrics::{self, Metrics},
    recorders::MetricRecorder,
    utils::{self, wei_to_unit},
};

/// Reports L2 block-level metrics: gas, fees, transactions, DA footprint.

#[derive(Debug)]
pub struct L2BlockInfoRecorder<P> {
    provider: Arc<P>,
    system_config_address: Address,
}

impl<P> L2BlockInfoRecorder<P> {
    pub const fn new(provider: Arc<P>, system_config_address: Address) -> Self {
        Self { provider, system_config_address }
    }
}

#[async_trait]
impl<P: Provider + Send + Sync + 'static> MetricRecorder for L2BlockInfoRecorder<P> {
    fn name(&self) -> &'static str {
        "l2_block_info"
    }

    async fn record(&self, block: &Block, m: &Metrics) -> anyhow::Result<()> {
        let block_num = block.header.number;
        let gas_limit = block.header.gas_limit;
        let gas_used = block.header.gas_used;
        let base_fee = block.header.base_fee_per_gas.unwrap_or(0);

        let _ = m.gauge(metrics::BLOCK_GAS_LIMIT, gas_limit as f64);
        let _ = m.histogram(metrics::BLOCK_GAS_USED, gas_used as f64);
        let _ = m.histogram(metrics::BLOCK_BASE_GAS_FEE, base_fee as f64);
        let tx_count = block.transactions.len();
        let _ = m.count(metrics::TRANSACTION_COUNT, tx_count as i64);

        // Extract minBaseFee from ExtraData (Jovian format: bytes 9..17).
        let extra = &block.header.extra_data;
        if extra.len() >= 17 {
            let min_base_fee = u64::from_be_bytes(extra[9..17].try_into().unwrap_or_default());
            let _ = m.gauge(metrics::MIN_BASE_FEE, min_base_fee as f64);
        }

        // DA footprint from blob_gas_used field (Jovian format).
        if let Some(blob_gas_used) = block.header.blob_gas_used {
            let _ = m.gauge(metrics::DA_FOOTPRINT, blob_gas_used as f64);
        }

        // Fetch DA footprint gas scalar from L1Block predeploy.
        let l1_block = bindings::L1Block::new(bindings::L1_BLOCK, &*self.provider);
        match l1_block.daFootprintGasScalar().call().await {
            Ok(ret) => {
                let _ = m.gauge(metrics::DA_FOOTPRINT_GAS_SCALAR, ret as f64);
            }
            Err(e) => {
                warn!(error = %e, "failed to get DA footprint gas scalar");
            }
        }

        // Empty block detection.
        if tx_count <= 1 {
            warn!(block = block_num, tx_count, "empty block detected");
            let _ = m.count(metrics::EMPTY_BLOCKS, 1);
        }

        // EIP-1559 elasticity + gas target (from L1 SystemConfig).
        let sys_config = bindings::SystemConfig::new(self.system_config_address, &*self.provider);
        match sys_config.eip1559Elasticity().call().await {
            Ok(elasticity) => {
                if elasticity > 0 {
                    let gas_target = gas_limit as f64 / f64::from(elasticity);
                    let _ = m.gauge(metrics::BLOCK_GAS_TARGET, gas_target);
                    let _ = m.gauge(metrics::ELASTICITY, f64::from(elasticity));
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to get EIP-1559 elasticity");
            }
        }

        // Per-transaction metrics.
        for tx in block.transactions.txns() {
            let tx_type = tx.ty();
            let type_str = tx_type.to_string();
            let tags: [(&str, &str); 1] = [("txnType", type_str.as_str())];

            m.count_with_tags(metrics::TRANSACTION_TYPE, i64::from(tx_type), &tags);

            let gas_price = Transaction::gas_price(tx).unwrap_or(0);
            let tip_cap = tx.max_priority_fee_per_gas().unwrap_or(0);
            let effective_tip =
                tip_cap.min(Transaction::max_fee_per_gas(tx).saturating_sub(base_fee as u128));
            let effective_price = base_fee as u128 + effective_tip;

            m.histogram_with_tags(
                metrics::TRANSACTION_GAS_PRICE_MAX,
                wei_to_unit(U256::from(gas_price), Unit::GWEI),
                &tags,
            );
            m.histogram_with_tags(
                metrics::TRANSACTION_MAX_PRIORITY_FEE,
                wei_to_unit(U256::from(tip_cap), Unit::GWEI),
                &tags,
            );
            m.histogram_with_tags(
                metrics::TRANSACTION_GAS_PRICE,
                wei_to_unit(U256::from(effective_price), Unit::GWEI),
                &tags,
            );
        }

        // Receipt-based metrics (tx success/fail, L1 DA fees, priority fees).
        self.record_transaction_status(block, m).await;

        Ok(())
    }
}

impl<P: Provider + Send + Sync + 'static> L2BlockInfoRecorder<P> {
    async fn record_transaction_status(&self, block: &Block, m: &Metrics) {
        let base_fee = block.header.base_fee_per_gas.unwrap_or(0);
        let mut set = JoinSet::new();

        for tx in block.transactions.txns() {
            let tx_hash = tx.tx_hash();
            let provider = Arc::clone(&self.provider);
            let metrics = m.clone();
            let tip_cap = tx.max_priority_fee_per_gas().unwrap_or(0);
            let max_fee = Transaction::max_fee_per_gas(tx);
            let tx_type = tx.ty();

            set.spawn(async move {
                let receipt = match provider.get_transaction_receipt(tx_hash).await {
                    Ok(Some(r)) => r,
                    Ok(None) => return,
                    Err(e) => {
                        error!(error = %e, "error fetching receipt");
                        return;
                    }
                };

                // Skip system transactions (type 126).
                if tx_type != utils::DEPOSIT_TX_TYPE {
                    // Compute L2 fee.
                    let effective_gas_price = receipt.effective_gas_price;
                    let gas_used = receipt.gas_used as u128;
                    let l2_fee = effective_gas_price * gas_used;
                    let _ = metrics.histogram(
                        metrics::TRANSACTION_L2_FEES,
                        wei_to_unit(U256::from(l2_fee), Unit::GWEI),
                    );

                    // Priority fees.
                    let effective_tip = tip_cap.min(max_fee.saturating_sub(base_fee as u128));
                    let tx_priority_fees = effective_tip * gas_used;
                    let _ = metrics.gauge(
                        metrics::TRANSACTION_TOTAL_PRI_FEES,
                        wei_to_unit(U256::from(tx_priority_fees), Unit::GWEI),
                    );
                }

                if receipt.status() {
                    let _ = metrics.count(metrics::TRANSACTIONS_SUCCESS, 1);
                } else {
                    let _ = metrics.count(metrics::TRANSACTIONS_FAILED, 1);
                }
            });
        }

        while set.join_next().await.is_some() {}
    }
}
