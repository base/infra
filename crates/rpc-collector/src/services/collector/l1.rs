//! L1 collector service — assembles L1-specific metric recorders
//! and wraps the shared [`Collector`](super::Collector) logic.

use std::{sync::Arc, time::Duration};

use alloy_primitives::Address;
use alloy_provider::Provider;
use alloy_sol_types::SolEvent;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::{
    bindings,
    metrics::{self, Metrics},
    recorders::{
        MetricRecorder, batches_info::BatchesInfoRecorder,
        dispute_game_factory::DisputeGameFactoryRecorder, eth_balance::EthBalanceRecorder,
        event_counter::EventCounterRecorder, output_oracle::OutputOracleRecorder,
        reorg_detector::ReorgDetectorRecorder,
    },
    services::{Service, collector::Collector},
    utils::parse_contract_balance_calls,
};

/// Configuration needed to build the L1 collector service.
#[derive(Debug, Clone)]
pub struct L1CollectorConfig {
    pub poll_interval: Duration,
    pub optimism_portal_address: Address,
    pub output_oracle_address: Option<Address>,
    pub dispute_game_factory_address: Option<Address>,
    pub l1_standard_bridge_address: Address,
    pub ethereum_proposer_address: Address,
    pub ethereum_batcher_address: Address,
    pub batch_inbox_address: Address,
    pub balance_tracker_address: Option<Address>,
    pub contract_balance_calls: Vec<String>,
}

// ── L1Collector ─────────────────────────────────────────────

/// The L1 collector service.
///
/// Wraps the shared [`Collector`] polling loop with the L1-specific
/// set of metric recorders.
pub struct L1Collector<P> {
    inner: Collector<Arc<P>>,
}

impl<P> std::fmt::Debug for L1Collector<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("L1Collector").field("inner", &self.inner).finish()
    }
}

impl<P: Provider + Clone + Send + Sync + 'static> L1Collector<P> {
    /// Build a fully-wired L1 collector.
    pub fn new(cfg: L1CollectorConfig, provider: Arc<P>, metrics: Metrics) -> anyhow::Result<Self> {
        let recorders = Self::build_recorders(&cfg, &provider)?;
        Ok(Self {
            inner: Collector::new(
                provider,
                cfg.poll_interval,
                recorders,
                metrics,
                "l1",
                metrics::L1_LATEST_BLOCK,
            ),
        })
    }

    /// Assemble the L1-specific set of metric recorders.
    fn build_recorders(
        cfg: &L1CollectorConfig,
        l1: &Arc<P>,
    ) -> anyhow::Result<Vec<Arc<dyn MetricRecorder>>> {
        let mut rec: Vec<Arc<dyn MetricRecorder>> = vec![Arc::new(ReorgDetectorRecorder::new())];

        // Bridge + portal event counters.
        let bridge = cfg.l1_standard_bridge_address;
        let portal = cfg.optimism_portal_address;
        for (addr, topic, metric) in [
            (
                bridge,
                bindings::L1StandardBridge::ETHBridgeInitiated::SIGNATURE_HASH,
                metrics::DEPOSIT_INITIATED_COUNT,
            ),
            (
                bridge,
                bindings::L1StandardBridge::ERC20BridgeInitiated::SIGNATURE_HASH,
                metrics::DEPOSIT_INITIATED_COUNT,
            ),
            (
                bridge,
                bindings::L1StandardBridge::ETHWithdrawalFinalized::SIGNATURE_HASH,
                metrics::WITHDRAWAL_FINALIZED_COUNT,
            ),
            (
                bridge,
                bindings::L1StandardBridge::ERC20WithdrawalFinalized::SIGNATURE_HASH,
                metrics::WITHDRAWAL_FINALIZED_COUNT,
            ),
            (
                portal,
                bindings::OptimismPortal::TransactionDeposited::SIGNATURE_HASH,
                metrics::DEPOSIT_INITIATED_COUNT,
            ),
            (
                portal,
                bindings::OptimismPortal::WithdrawalFinalized::SIGNATURE_HASH,
                metrics::WITHDRAWAL_FINALIZED_COUNT,
            ),
        ] {
            rec.push(Arc::new(EventCounterRecorder::new(
                Arc::clone(l1),
                addr,
                topic,
                metric.to_string(),
            )));
        }

        // Proposer + batcher balances.
        rec.push(Arc::new(EthBalanceRecorder::eth(
            Arc::clone(l1),
            cfg.ethereum_proposer_address,
            metrics::PROPOSER_BALANCE.to_string(),
        )));
        rec.push(Arc::new(EthBalanceRecorder::eth(
            Arc::clone(l1),
            cfg.ethereum_batcher_address,
            metrics::BATCH_BALANCE.to_string(),
        )));

        // Batches info.
        rec.push(Arc::new(BatchesInfoRecorder::new(
            cfg.batch_inbox_address,
            cfg.ethereum_batcher_address,
        )));

        // Contract balance calls.
        let calls = parse_contract_balance_calls(&cfg.contract_balance_calls)?;
        for c in calls {
            rec.push(Arc::new(EthBalanceRecorder::contract(
                Arc::clone(l1),
                c.address,
                c.metric,
                c.calldata,
            )));
        }

        // Output Oracle or Dispute Game Factory.
        if let Some(dgf) = cfg.dispute_game_factory_address {
            rec.push(Arc::new(DisputeGameFactoryRecorder::new(Arc::clone(l1), dgf)));
        } else if let Some(oo) = cfg.output_oracle_address {
            rec.push(Arc::new(OutputOracleRecorder::new(Arc::clone(l1), oo)));
        } else {
            anyhow::bail!(
                "either --dispute-game-factory-address or --output-oracle-address must be set"
            );
        }

        // Balance tracker (optional).
        if let Some(bt) = cfg.balance_tracker_address {
            info!("monitoring BalanceTracker events");
            rec.push(Arc::new(EventCounterRecorder::new(
                Arc::clone(l1),
                bt,
                bindings::BalanceTracker::ReceivedFunds::SIGNATURE_HASH,
                metrics::RECEIVED_FUNDS_COUNT.to_string(),
            )));
            rec.push(Arc::new(EventCounterRecorder::new(
                Arc::clone(l1),
                bt,
                bindings::BalanceTracker::SentProfit::SIGNATURE_HASH,
                metrics::SENT_PROFIT_COUNT.to_string(),
            )));
        }

        Ok(rec)
    }
}

impl<P: Provider + Clone + Send + Sync + 'static> Service for L1Collector<P> {
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

    /// Minimal L1 config with both oracle addresses set to `None`.
    fn base_cfg() -> L1CollectorConfig {
        L1CollectorConfig {
            poll_interval: Duration::from_millis(50),
            optimism_portal_address: Address::ZERO,
            output_oracle_address: None,
            dispute_game_factory_address: None,
            l1_standard_bridge_address: Address::ZERO,
            ethereum_proposer_address: Address::ZERO,
            ethereum_batcher_address: Address::ZERO,
            batch_inbox_address: Address::ZERO,
            balance_tracker_address: None,
            contract_balance_calls: vec![],
        }
    }

    #[test]
    fn l1_collector_new_fails_without_oracle_or_dgf() {
        let cfg = base_cfg(); // both addresses are None
        let result = L1Collector::new(cfg, dummy_provider(), Metrics::noop());
        assert!(result.is_err(), "expected error when both oracle addresses are None");
    }

    #[test]
    fn l1_collector_new_with_dgf() {
        let mut cfg = base_cfg();
        cfg.dispute_game_factory_address = Some(Address::ZERO);

        let collector = L1Collector::new(cfg, dummy_provider(), Metrics::noop());
        assert!(collector.is_ok(), "expected Ok when dispute_game_factory_address is set");
    }

    #[test]
    fn l1_collector_new_with_output_oracle() {
        let mut cfg = base_cfg();
        cfg.output_oracle_address = Some(Address::ZERO);

        let collector = L1Collector::new(cfg, dummy_provider(), Metrics::noop());
        assert!(collector.is_ok(), "expected Ok when output_oracle_address is set");
    }

    #[test]
    fn l1_collector_name_returns_l1() {
        let mut cfg = base_cfg();
        cfg.dispute_game_factory_address = Some(Address::ZERO);

        let collector = L1Collector::new(cfg, dummy_provider(), Metrics::noop()).unwrap();
        assert_eq!(collector.name(), "l1");
    }
}
