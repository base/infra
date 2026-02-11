//! L2 collector service — assembles L2-specific metric recorders
//! and wraps the shared [`Collector`](super::Collector) logic.

use std::{sync::Arc, time::Duration};

use alloy_primitives::Address;
use alloy_provider::Provider;
use alloy_sol_types::SolEvent;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::{
    bindings::{
        self, BASE_FEE_VAULT, L1_FEE_VAULT, L2_STANDARD_BRIDGE, L2_TO_L1_MSG_PASSER,
        SEQUENCER_FEE_VAULT,
    },
    metrics::{self, Metrics},
    recorders::{
        MetricRecorder, eth_balance::EthBalanceRecorder, event_counter::EventCounterRecorder,
        gas_price_oracle::GasPriceOracleRecorder, l2_block_info::L2BlockInfoRecorder,
        reorg_detector::ReorgDetectorRecorder,
    },
    services::{Service, collector::Collector},
    utils::parse_contract_balance_calls,
};

/// Configuration needed to build the L2 collector service.
#[derive(Debug, Clone)]
pub struct L2CollectorConfig {
    pub poll_interval: Duration,
    pub fee_disburser_address: Option<Address>,
    pub l1_system_config_address: Address,
    pub contract_balance_calls: Vec<String>,
}

// ── L2Collector ─────────────────────────────────────────────

/// The L2 collector service.
///
/// Wraps the shared [`Collector`] polling loop with the L2-specific
/// set of metric recorders.
pub struct L2Collector<P> {
    inner: Collector<Arc<P>>,
}

impl<P> std::fmt::Debug for L2Collector<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("L2Collector").field("inner", &self.inner).finish()
    }
}

impl<P: Provider + Clone + Send + Sync + 'static> L2Collector<P> {
    /// Build a fully-wired L2 collector.
    pub fn new(cfg: L2CollectorConfig, provider: Arc<P>, metrics: Metrics) -> anyhow::Result<Self> {
        let recorders = Self::build_recorders(&cfg, &provider)?;
        Ok(Self {
            inner: Collector::new(
                provider,
                cfg.poll_interval,
                recorders,
                metrics,
                "l2",
                metrics::L2_LATEST_BLOCK,
            ),
        })
    }

    /// Assemble the L2-specific set of metric recorders.
    fn build_recorders(
        cfg: &L2CollectorConfig,
        l2: &Arc<P>,
    ) -> anyhow::Result<Vec<Arc<dyn MetricRecorder>>> {
        let mut rec: Vec<Arc<dyn MetricRecorder>> = Vec::new();

        // Fee Disburser (optional).
        if let Some(addr) = cfg.fee_disburser_address {
            info!("monitoring FeeDisburser events");
            rec.push(Arc::new(EventCounterRecorder::new(
                Arc::clone(l2),
                addr,
                bindings::FeeDisburser::FeesDisbursed::SIGNATURE_HASH,
                metrics::FEES_DISBURSED_COUNT.to_string(),
            )));
        }

        rec.push(Arc::new(L2BlockInfoRecorder::new(Arc::clone(l2), cfg.l1_system_config_address)));
        rec.push(Arc::new(ReorgDetectorRecorder::new()));

        // Predeploy balances.
        for (addr, metric) in [
            (SEQUENCER_FEE_VAULT, metrics::SEQUENCER_BALANCE),
            (BASE_FEE_VAULT, metrics::BASE_FEE_BALANCE),
            (L1_FEE_VAULT, metrics::L1_FEE_BALANCE),
        ] {
            rec.push(Arc::new(EthBalanceRecorder::eth(Arc::clone(l2), addr, metric.to_string())));
        }

        // L2 event counters.
        for (addr, topic, metric) in [
            (
                L2_TO_L1_MSG_PASSER,
                bindings::L2ToL1MessagePasser::MessagePassed::SIGNATURE_HASH,
                metrics::WITHDRAWAL_INITIATED_COUNT,
            ),
            (
                L2_STANDARD_BRIDGE,
                bindings::L2StandardBridge::WithdrawalInitiated::SIGNATURE_HASH,
                metrics::WITHDRAWAL_INITIATED_COUNT,
            ),
            (
                L2_STANDARD_BRIDGE,
                bindings::L2StandardBridge::DepositFinalized::SIGNATURE_HASH,
                metrics::DEPOSIT_FINALIZED_COUNT,
            ),
        ] {
            rec.push(Arc::new(EventCounterRecorder::new(
                Arc::clone(l2),
                addr,
                topic,
                metric.to_string(),
            )));
        }

        // Gas Price Oracle.
        rec.push(Arc::new(GasPriceOracleRecorder::new(Arc::clone(l2))));

        // Contract balance calls.
        let calls = parse_contract_balance_calls(&cfg.contract_balance_calls)?;
        for c in calls {
            rec.push(Arc::new(EthBalanceRecorder::contract(
                Arc::clone(l2),
                c.address,
                c.metric,
                c.calldata,
            )));
        }

        Ok(rec)
    }
}

impl<P: Provider + Clone + Send + Sync + 'static> Service for L2Collector<P> {
    fn name(&self) -> &str {
        self.inner.label()
    }

    fn spawn(self: Box<Self>, set: &mut JoinSet<()>, cancel: CancellationToken) {
        self.inner.spawn(set, cancel);
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::Address;
    use alloy_provider::ProviderBuilder;

    use super::*;
    use crate::services::Service;

    /// Dummy provider pointing at a non-existent endpoint (never contacted).
    fn dummy_provider() -> Arc<alloy_provider::RootProvider> {
        Arc::new(
            ProviderBuilder::new()
                .disable_recommended_fillers()
                .connect_http("http://127.0.0.1:1".parse().unwrap()),
        )
    }

    fn minimal_cfg() -> L2CollectorConfig {
        L2CollectorConfig {
            poll_interval: Duration::from_millis(50),
            fee_disburser_address: None,
            l1_system_config_address: Address::ZERO,
            contract_balance_calls: vec![],
        }
    }

    #[test]
    fn l2_collector_new_minimal() {
        let collector = L2Collector::new(minimal_cfg(), dummy_provider(), Metrics::noop());
        assert!(collector.is_ok(), "expected Ok with minimal L2 config");
    }

    #[test]
    fn l2_collector_name_returns_l2() {
        let collector = L2Collector::new(minimal_cfg(), dummy_provider(), Metrics::noop()).unwrap();
        assert_eq!(collector.name(), "l2");
    }
}
