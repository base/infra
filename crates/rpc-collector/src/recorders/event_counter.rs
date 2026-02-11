//! Generic event counter recorder — counts contract events in a block range.

use std::{fmt, sync::Arc};

use alloy_primitives::{Address, FixedBytes};
use alloy_provider::Provider;
use alloy_rpc_types_eth::{Block, Filter};
use async_trait::async_trait;

use crate::{metrics::Metrics, recorders::MetricRecorder};

/// Reports the count of a specific event emitted by a contract.
pub struct EventCounterRecorder<P> {
    provider: Arc<P>,
    address: Address,
    event_topic: FixedBytes<32>,
    metric_name: String,
}

impl<P> fmt::Debug for EventCounterRecorder<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventCounterRecorder")
            .field("address", &self.address)
            .field("event_topic", &self.event_topic)
            .field("metric_name", &self.metric_name)
            .finish()
    }
}

impl<P> EventCounterRecorder<P> {
    pub const fn new(
        provider: Arc<P>,
        address: Address,
        event_topic: FixedBytes<32>,
        metric_name: String,
    ) -> Self {
        Self { provider, address, event_topic, metric_name }
    }
}

#[async_trait]
impl<P: Provider + Send + Sync + 'static> MetricRecorder for EventCounterRecorder<P> {
    fn name(&self) -> &'static str {
        "event_counter"
    }

    async fn record(&self, block: &Block, m: &Metrics) -> anyhow::Result<()> {
        let block_num = block.header.number;

        let filter = Filter::new()
            .address(self.address)
            .event_signature(self.event_topic)
            .from_block(block_num)
            .to_block(block_num);

        let count = self.provider.get_logs(&filter).await?.len();
        if count > 0 {
            let _ = m.count(&self.metric_name, count as i64);
        }

        Ok(())
    }
}
