use std::time::Duration;

use alloy_primitives::B256;
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
};
use tracing::{info, warn};

use crate::tracker::{self, Stats, TrackerEvent};

/// Handle to a running load test, returned by `start_load_test`.
#[derive(Debug)]
pub struct LoadTestHandle {
    /// Tracker channel for stats polling
    tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
    /// Shutdown signal for preparers/signers/senders
    shutdown_tx: broadcast::Sender<()>,
    /// Shutdown signal for drain-phase tasks (stats reporter, block watcher)
    drain_shutdown_tx: broadcast::Sender<()>,
    /// Block event channel (dropped to signal consumers to exit)
    block_tx: broadcast::Sender<crate::blocks::OpBlock>,
    /// Pipeline task handles (preparers, signers, senders)
    pipeline_handles: Vec<JoinHandle<()>>,
    /// Stats reporter handle
    stats_handle: JoinHandle<()>,
    /// Block watcher handle
    block_handle: JoinHandle<()>,
    /// Block logger handle
    logger_handle: JoinHandle<()>,
    /// Confirmer handle
    confirmer_handle: JoinHandle<()>,
    /// Confirmer pending tx sender (dropped after shutdown to signal confirmer)
    confirmer_pending_tx: mpsc::UnboundedSender<B256>,
    /// Flashblock watcher handle
    flashblock_handle: JoinHandle<()>,
    /// Flashblock pending tx sender (dropped after shutdown to signal watcher)
    flashblock_pending_tx: mpsc::UnboundedSender<B256>,
    /// Rate limiter replenisher handle
    rate_limiter_handle: Option<JoinHandle<()>>,
    /// Tracker task handle
    tracker_handle: JoinHandle<()>,
    /// Management hosts from config
    txpool_hosts: Vec<String>,
    /// Test duration
    duration: Option<Duration>,
    /// HTTP client
    http_client: reqwest::Client,
    /// Target TPS
    target_tps: Option<u32>,
}

/// Cloneable stats poller that can be used during drain when the handle is consumed.
#[derive(Debug, Clone)]
pub struct StatsPoller {
    tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
}

impl StatsPoller {
    /// Poll current stats from the tracker.
    pub async fn get_stats(&self) -> Option<Stats> {
        tracker::get_stats(&self.tracker_tx).await
    }
}

impl LoadTestHandle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
        shutdown_tx: broadcast::Sender<()>,
        drain_shutdown_tx: broadcast::Sender<()>,
        block_tx: broadcast::Sender<crate::blocks::OpBlock>,
        pipeline_handles: Vec<JoinHandle<()>>,
        stats_handle: JoinHandle<()>,
        block_handle: JoinHandle<()>,
        logger_handle: JoinHandle<()>,
        confirmer_handle: JoinHandle<()>,
        confirmer_pending_tx: mpsc::UnboundedSender<B256>,
        flashblock_handle: JoinHandle<()>,
        flashblock_pending_tx: mpsc::UnboundedSender<B256>,
        rate_limiter_handle: Option<JoinHandle<()>>,
        tracker_handle: JoinHandle<()>,
        txpool_hosts: Vec<String>,
        duration: Option<Duration>,
        http_client: reqwest::Client,
        target_tps: Option<u32>,
    ) -> Self {
        Self {
            tracker_tx,
            shutdown_tx,
            drain_shutdown_tx,
            block_tx,
            pipeline_handles,
            stats_handle,
            block_handle,
            logger_handle,
            confirmer_handle,
            confirmer_pending_tx,
            flashblock_handle,
            flashblock_pending_tx,
            rate_limiter_handle,
            tracker_handle,
            txpool_hosts,
            duration,
            http_client,
            target_tps,
        }
    }

    /// Poll current stats from the tracker.
    pub async fn get_stats(&self) -> Option<Stats> {
        tracker::get_stats(&self.tracker_tx).await
    }

    /// Create a cloneable stats poller (useful during drain when handle is consumed).
    pub fn stats_poller(&self) -> StatsPoller {
        StatsPoller { tracker_tx: self.tracker_tx.clone() }
    }

    /// Signal the pipeline to stop sending new transactions.
    pub fn shutdown(&self) {
        info!("Shutting down load test...");
        if let Err(e) = self.shutdown_tx.send(()) {
            warn!(error = %e, "Failed to send shutdown signal");
        }
    }

    /// Consume the handle, wait for pending transactions to drain, return final stats.
    pub async fn wait_and_drain(self) -> Stats {
        // Wait for pipeline tasks to complete
        for (i, handle) in self.pipeline_handles.into_iter().enumerate() {
            if let Err(e) = handle.await {
                warn!(task = i, error = %e, "Task panicked during shutdown");
            }
        }

        // Stop rate limiter replenisher
        if let Some(handle) = self.rate_limiter_handle {
            handle.abort();
            let _ = handle.await;
        }

        // Close the pending channels now that all senders have finished
        drop(self.confirmer_pending_tx);
        drop(self.flashblock_pending_tx);

        // Wait for pending transactions to confirm
        info!("Waiting for pending transactions to confirm...");
        let mut last_pending = u64::MAX;
        let mut last_change = std::time::Instant::now();
        let stall_timeout = std::time::Duration::from_secs(20);

        loop {
            let stats = tracker::get_stats(&self.tracker_tx).await.unwrap_or_default();
            let pending = stats.pending();

            if pending == 0 {
                info!("All transactions confirmed or timed out");
                break;
            }

            if pending != last_pending {
                last_pending = pending;
                last_change = std::time::Instant::now();
            } else if last_change.elapsed() > stall_timeout {
                warn!(pending, "No confirmation progress for 20s, proceeding with final report");
                break;
            }

            info!(pending, "Waiting for confirmations...");
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }

        // Shut down drain-phase tasks
        if let Err(e) = self.drain_shutdown_tx.send(()) {
            warn!(error = %e, "Failed to send drain shutdown signal");
        }
        let _ = self.stats_handle.await;
        let _ = self.block_handle.await;
        let _ = self.flashblock_handle.await;

        // Close block channel to signal consumers
        drop(self.block_tx);
        let _ = self.logger_handle.await;
        let _ = self.confirmer_handle.await;

        // Get final stats
        let final_stats = tracker::get_stats(&self.tracker_tx).await.unwrap_or_default();
        if let Err(e) = self.tracker_tx.send(TrackerEvent::Shutdown) {
            warn!(error = %e, "Failed to send shutdown to tracker");
        }
        if let Err(e) = self.tracker_handle.await {
            warn!(error = %e, "Tracker task panicked");
        }

        final_stats
    }

    /// Get management hosts from the config.
    pub fn txpool_hosts(&self) -> &[String] {
        &self.txpool_hosts
    }

    /// Get the configured test duration.
    pub const fn duration(&self) -> Option<Duration> {
        self.duration
    }

    /// Get the shared HTTP client.
    pub const fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }

    /// Get the configured target TPS.
    pub const fn target_tps(&self) -> Option<u32> {
        self.target_tps
    }
}
