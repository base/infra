use std::time::Duration;

use alloy_provider::ext::TxPoolApi;
use tokio::sync::{mpsc, oneshot};

use crate::{LoadTestHandle, Stats, StatsPoller};

#[derive(Debug, Clone)]
pub struct TxpoolHostStatus {
    pub host: String,
    pub pending: u64,
    pub queued: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadTestPhase {
    Starting,
    Running,
    Draining,
    Complete,
}

impl std::fmt::Display for LoadTestPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => write!(f, "Starting"),
            Self::Running => write!(f, "Running"),
            Self::Draining => write!(f, "Draining"),
            Self::Complete => write!(f, "Complete"),
        }
    }
}

/// Channels for receiving loadtest updates from background tasks.
#[derive(Debug)]
pub struct LoadTestChannels {
    pub stats_rx: mpsc::Receiver<Stats>,
    pub txpool_rx: mpsc::Receiver<Vec<TxpoolHostStatus>>,
    pub phase_rx: mpsc::Receiver<LoadTestPhase>,
    pub shutdown_tx: oneshot::Sender<()>,
}

#[derive(Debug)]
pub struct LoadTestState {
    pub stats: Option<Stats>,
    stats_rx: Option<mpsc::Receiver<Stats>>,
    pub txpool_status: Vec<TxpoolHostStatus>,
    txpool_rx: Option<mpsc::Receiver<Vec<TxpoolHostStatus>>>,
    pub phase: LoadTestPhase,
    phase_rx: Option<mpsc::Receiver<LoadTestPhase>>,
    pub shutdown_tx: Option<oneshot::Sender<()>>,
    pub txpool_hosts: Vec<String>,
    pub target_tps: Option<u32>,
    pub duration: Option<Duration>,
    pub config_file: String,
}

impl LoadTestState {
    pub fn new(
        channels: LoadTestChannels,
        txpool_hosts: Vec<String>,
        target_tps: Option<u32>,
        duration: Option<Duration>,
        config_file: String,
    ) -> Self {
        Self {
            stats: None,
            stats_rx: Some(channels.stats_rx),
            txpool_status: Vec::new(),
            txpool_rx: Some(channels.txpool_rx),
            phase: LoadTestPhase::Starting,
            phase_rx: Some(channels.phase_rx),
            shutdown_tx: Some(channels.shutdown_tx),
            txpool_hosts,
            target_tps,
            duration,
            config_file,
        }
    }

    pub fn poll(&mut self) {
        // Drain stats channel, keep latest
        if let Some(ref mut rx) = self.stats_rx {
            while let Ok(stats) = rx.try_recv() {
                self.stats = Some(stats);
            }
        }

        // Drain txpool channel, keep latest
        if let Some(ref mut rx) = self.txpool_rx {
            while let Ok(status) = rx.try_recv() {
                self.txpool_status = status;
            }
        }

        // Drain phase channel, keep latest
        if let Some(ref mut rx) = self.phase_rx {
            while let Ok(phase) = rx.try_recv() {
                self.phase = phase;
            }
        }
    }
}

/// Spawn the load test lifecycle manager and txpool poller, returning a
/// [`LoadTestState`] that can be polled for updates.
pub fn activate(handle: LoadTestHandle, config_file: String) -> LoadTestState {
    let txpool_hosts = handle.txpool_hosts().to_vec();
    let target_tps = handle.target_tps();
    let duration = handle.duration();
    let http_client = handle.http_client().clone();

    let (stats_tx, stats_rx) = mpsc::channel::<Stats>(16);
    let (txpool_tx, txpool_rx) = mpsc::channel::<Vec<TxpoolHostStatus>>(16);
    let (phase_tx, phase_rx) = mpsc::channel::<LoadTestPhase>(16);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let state = LoadTestState::new(
        LoadTestChannels { stats_rx, txpool_rx, phase_rx, shutdown_tx },
        txpool_hosts.clone(),
        target_tps,
        duration,
        config_file,
    );

    let poller = handle.stats_poller();
    tokio::spawn(loadtest_manager(handle, poller, shutdown_rx, stats_tx, phase_tx));

    if !txpool_hosts.is_empty() {
        tokio::spawn(txpool_poller(http_client, txpool_hosts, txpool_tx));
    }

    state
}

/// Background task that manages the loadtest lifecycle.
async fn loadtest_manager(
    handle: LoadTestHandle,
    poller: StatsPoller,
    shutdown_rx: oneshot::Receiver<()>,
    stats_tx: mpsc::Sender<Stats>,
    phase_tx: mpsc::Sender<LoadTestPhase>,
) {
    let _ = phase_tx.send(LoadTestPhase::Running).await;

    let duration = handle.duration();

    // Single stats poll task that runs across both running and drain phases
    let poll_stats_tx = stats_tx.clone();
    let poll_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            if let Some(stats) = poller.get_stats().await
                && poll_stats_tx.send(stats).await.is_err()
            {
                break;
            }
        }
    });

    // Wait for shutdown signal (TUI quit/drop) or duration
    if let Some(d) = duration {
        tokio::select! {
            _ = tokio::time::sleep(d) => {}
            _ = shutdown_rx => {}
        }
    } else {
        let _ = shutdown_rx.await;
    }

    // Drain phase
    let _ = phase_tx.send(LoadTestPhase::Draining).await;
    handle.shutdown();

    let final_stats = handle.wait_and_drain().await;
    poll_task.abort();

    // Send final stats
    let _ = stats_tx.send(final_stats).await;
    let _ = phase_tx.send(LoadTestPhase::Complete).await;
}

/// Background task that polls `txpool_status` on management hosts.
async fn txpool_poller(
    http_client: reqwest::Client,
    hosts: Vec<String>,
    tx: mpsc::Sender<Vec<TxpoolHostStatus>>,
) {
    let providers: Vec<alloy_provider::RootProvider> = hosts
        .iter()
        .filter_map(|host| {
            let url: url::Url = host.parse().ok()?;
            let http = alloy_transport_http::Http::with_client(http_client.clone(), url);
            let client = alloy_rpc_client::RpcClient::new(http, true);
            Some(alloy_provider::RootProvider::new(client))
        })
        .collect();

    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;

        let mut statuses = Vec::with_capacity(hosts.len());
        for (host, provider) in hosts.iter().zip(providers.iter()) {
            match provider.txpool_status().await {
                Ok(status) => {
                    statuses.push(TxpoolHostStatus {
                        host: host.clone(),
                        pending: status.pending,
                        queued: status.queued,
                    });
                }
                Err(_) => {
                    statuses.push(TxpoolHostStatus { host: host.clone(), pending: 0, queued: 0 });
                }
            }
        }

        if tx.send(statuses).await.is_err() {
            break;
        }
    }
}
