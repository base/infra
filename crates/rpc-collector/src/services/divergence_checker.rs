//! Divergence checker — fetches the same block from Geth and Reth and compares
//! block hash, state root, transaction root and receipt root.

use std::time::Duration;

use alloy_provider::Provider;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    metrics::{self, Metrics},
    services::Service,
};

// ── Service ──────────────────────────────────────────────────

/// The divergence checker service, implementing [`Service`].
#[derive(Debug)]
pub struct DivergenceCheckerService<P> {
    /// Geth provider.
    pub geth: P,
    /// Reth provider.
    pub reth: P,
    /// Interval between divergence check polls.
    pub poll_interval: Duration,
    /// Grace period for Geth node sync.
    pub geth_grace_period: Duration,
    /// Metrics client.
    pub metrics: Metrics,
    /// Human-readable checker name.
    pub checker_name: String,
}

impl<P: Provider + Send + Sync + 'static> Service for DivergenceCheckerService<P> {
    fn name(&self) -> &str {
        &self.checker_name
    }

    fn spawn(self: Box<Self>, set: &mut JoinSet<()>, cancel: CancellationToken) {
        let name = self.checker_name.clone();
        set.spawn(async move {
            if let Err(e) = run(
                self.geth,
                self.reth,
                self.poll_interval,
                self.geth_grace_period,
                self.metrics,
                self.checker_name,
                cancel,
            )
            .await
            {
                error!(checker = name, error = %e, "divergence checker failed");
            }
        });
    }
}

// ── Core logic ───────────────────────────────────────────────

/// Run the divergence checker loop.
async fn run<P: Provider + Send + Sync + 'static>(
    geth: P,
    reth: P,
    poll_interval: Duration,
    geth_grace_period: Duration,
    metrics: Metrics,
    checker_name: String,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let geth_block = geth.get_block_number().await?;
    let reth_block = reth.get_block_number().await?;
    let mut current_block = geth_block.min(reth_block);

    info!(checker = checker_name, start_block = current_block, "starting divergence checker");

    let mut interval = tokio::time::interval(poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let tags = [("checker", checker_name.as_str())];

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = interval.tick() => {
                let geth_latest = match geth.get_block_number().await {
                    Ok(n) => n,
                    Err(e) => {
                        error!(error = %e, "failed to get latest block from Geth");
                        metrics.count_with_tags(
                            metrics::DIVERGENCE_NODE_ERROR,
                            1,
                            &[("checker", checker_name.as_str()), ("node_type", "geth")],
                        );
                        continue;
                    }
                };

                let reth_latest = match reth.get_block_number().await {
                    Ok(n) => n,
                    Err(e) => {
                        error!(error = %e, "failed to get latest block from Reth");
                        metrics.count_with_tags(
                            metrics::DIVERGENCE_NODE_ERROR,
                            1,
                            &[("checker", checker_name.as_str()), ("node_type", "reth")],
                        );
                        continue;
                    }
                };

                // Check geth staleness.
                if let Ok(Some(header)) = geth.get_header_by_number(geth_latest.into()).await {
                    let age = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH + Duration::from_secs(header.timestamp))
                        .unwrap_or_default();
                    if age > geth_grace_period {
                        warn!(checker = checker_name, geth_block = geth_latest, "geth node appears stale");
                        metrics.count_with_tags(metrics::DIVERGENCE_GETH_TIMEOUT, 1, &tags);
                    }
                }

                let next_block = current_block + 1;
                if geth_latest.min(reth_latest) < next_block {
                    continue;
                }

                // Fetch block from both nodes in parallel.
                let (geth_res, reth_res) = tokio::join!(
                    geth.get_block_by_number(next_block.into()).full(),
                    reth.get_block_by_number(next_block.into()).full(),
                );

                let geth_block = match geth_res {
                    Ok(Some(b)) => b,
                    _ => {
                        metrics.count_with_tags(
                            metrics::DIVERGENCE_NODE_ERROR,
                            1,
                            &[("checker", checker_name.as_str()), ("node_type", "geth")],
                        );
                        continue;
                    }
                };

                let reth_block = match reth_res {
                    Ok(Some(b)) => b,
                    _ => {
                        metrics.count_with_tags(
                            metrics::DIVERGENCE_NODE_ERROR,
                            1,
                            &[("checker", checker_name.as_str()), ("node_type", "reth")],
                        );
                        continue;
                    }
                };

                // Compare the four critical roots.
                let matched = geth_block.header.hash == reth_block.header.hash
                    && geth_block.header.state_root == reth_block.header.state_root
                    && geth_block.header.transactions_root == reth_block.header.transactions_root
                    && geth_block.header.receipts_root == reth_block.header.receipts_root;

                if matched {
                    metrics.count_with_tags(metrics::DIVERGENCE_CROSS_GROUP_DETECTED, 0, &tags);
                } else {
                    error!(
                        block = next_block,
                        geth_hash = %geth_block.header.hash,
                        reth_hash = %reth_block.header.hash,
                        "divergence detected between geth and reth"
                    );
                    metrics.count_with_tags(metrics::DIVERGENCE_CROSS_GROUP_DETECTED, 1, &tags);
                }

                metrics.count_with_tags(metrics::DIVERGENCE_BLOCK_PROCESSED, 1, &tags);
                current_block = next_block;
            }
        }
    }
}
