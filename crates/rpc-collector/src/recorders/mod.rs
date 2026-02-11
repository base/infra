//! Metric recorder trait and implementations.
//!
//! Each recorder receives a block and emits `StatsD` metrics for a specific
//! aspect of the chain (balances, events, gas prices, etc.).

use alloy_rpc_types_eth::Block;
use async_trait::async_trait;

use crate::metrics::Metrics;

/// A metric recorder receives a block and emits `StatsD` metrics.
///
/// Implementations should be cheap to clone (wrap heavy state in [`Arc`]).
#[async_trait]
pub trait MetricRecorder: Send + Sync + 'static {
    /// Human-readable name, used in error logs.
    fn name(&self) -> &'static str;

    /// Record metrics for the given block.
    async fn record(&self, block: &Block, metrics: &Metrics) -> anyhow::Result<()>;
}

pub mod batches_info;
pub mod dispute_game_factory;
pub mod eth_balance;
pub mod event_counter;
pub mod gas_price_oracle;
pub mod l2_block_info;
pub mod output_oracle;
pub mod reorg_detector;
