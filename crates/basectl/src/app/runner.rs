use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use base_flashtypes::Flashblock;
use tokio::sync::{mpsc, oneshot};

use super::{
    App, Resources, ViewId,
    resources::{LoadTestChannels, LoadTestPhase, LoadTestSetup, LoadTestState, TxpoolHostStatus},
    views::create_view,
};
use crate::{
    config::ChainConfig,
    l1_client::{FullSystemConfig, fetch_full_system_config},
    rpc::{
        BacklogFetchResult, BlobSubmission, BlockDaInfo, TimestampedFlashblock,
        fetch_initial_backlog_with_progress, fetch_sync_status, run_block_fetcher,
        run_flashblock_ws, run_flashblock_ws_timestamped, run_l1_batcher_watcher,
    },
};

pub async fn run_app(config: ChainConfig) -> Result<()> {
    run_app_with_view(config, ViewId::Home).await
}

pub async fn run_app_with_view(config: ChainConfig, initial_view: ViewId) -> Result<()> {
    let mut resources = Resources::new(config.clone());

    start_background_services(&config, &mut resources);

    let app = App::new(resources, initial_view);
    app.run(create_view).await
}

/// Run load test with TUI dashboard.
/// Pre-populates the setup state so the `LoadTestView` transitions through
/// Starting → Dashboard automatically.
pub async fn run_loadtest_tui(config: ChainConfig, file: String) -> Result<()> {
    let mut resources = Resources::new(config.clone());
    start_background_services(&config, &mut resources);

    // Use the same async setup path as the TUI-initiated flow
    spawn_loadtest_setup(&mut resources, PathBuf::from(&file));

    let app = App::new(resources, ViewId::LoadTest);
    app.run(create_view).await
}

/// Run load test in headless mode with JSON output.
pub async fn run_loadtest_headless(file: String) -> Result<()> {
    let handle = gobrr::start_load_test(&file).await?;

    let duration = handle.duration();
    let poller = handle.stats_poller();

    // Spawn periodic JSON progress reporter
    let progress_poller = poller.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            if let Some(stats) = progress_poller.get_stats().await
                && let Ok(json) = serde_json::to_string(&stats)
            {
                println!("{json}");
            }
        }
    });

    // Wait for duration or Ctrl+C
    if let Some(d) = duration {
        tokio::select! {
            _ = tokio::time::sleep(d) => {
                tracing::info!("Duration elapsed");
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Ctrl+C received");
            }
        }
    } else {
        tokio::signal::ctrl_c().await.context("Failed to listen for Ctrl+C")?;
        tracing::info!("Ctrl+C received");
    }

    handle.shutdown();
    let stats = handle.wait_and_drain().await;

    // Print final JSON summary
    let json = serde_json::to_string_pretty(&stats).context("Failed to serialize final stats")?;
    println!("{json}");

    Ok(())
}

/// Spawn an async task that calls `gobrr::start_load_test` and stores the
/// result receiver in `resources.loadtest_setup`.
pub(super) fn spawn_loadtest_setup(resources: &mut Resources, config_path: PathBuf) {
    let (tx, rx) = oneshot::channel();
    let config_str = config_path.display().to_string();

    tokio::spawn(async move {
        let result = gobrr::start_load_test(&config_str).await;
        let _ = tx.send(result);
    });

    resources.loadtest_setup = Some(LoadTestSetup::Starting { config_path, result_rx: rx });
}

/// Given a successful `LoadTestHandle`, create channels, build `LoadTestState`,
/// and spawn `loadtest_manager` + `txpool_poller`.
pub(super) fn activate_loadtest(
    resources: &mut Resources,
    handle: gobrr::LoadTestHandle,
    config_file: String,
) {
    let management_hosts = handle.management_hosts().to_vec();
    let target_tps = handle.target_tps();
    let duration = handle.duration();
    let http_client = handle.http_client().clone();

    let (stats_tx, stats_rx) = mpsc::channel::<gobrr::Stats>(16);
    let (txpool_tx, txpool_rx) = mpsc::channel::<Vec<TxpoolHostStatus>>(16);
    let (phase_tx, phase_rx) = mpsc::channel::<LoadTestPhase>(16);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    resources.loadtest = Some(LoadTestState::new(
        LoadTestChannels { stats_rx, txpool_rx, phase_rx, shutdown_tx },
        management_hosts.clone(),
        target_tps,
        duration,
        config_file,
    ));

    let poller = handle.stats_poller();
    tokio::spawn(loadtest_manager(handle, poller, shutdown_rx, stats_tx, phase_tx));

    if !management_hosts.is_empty() {
        tokio::spawn(txpool_poller(http_client, management_hosts, txpool_tx));
    }
}

/// Background task that manages the loadtest lifecycle.
async fn loadtest_manager(
    handle: gobrr::LoadTestHandle,
    poller: gobrr::StatsPoller,
    shutdown_rx: oneshot::Receiver<()>,
    stats_tx: mpsc::Sender<gobrr::Stats>,
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
    use alloy_provider::ext::TxPoolApi;

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

fn start_background_services(config: &ChainConfig, resources: &mut Resources) {
    let (fb_tx, fb_rx) = mpsc::channel::<TimestampedFlashblock>(100);
    let (da_fb_tx, da_fb_rx) = mpsc::channel::<Flashblock>(100);
    let (sync_tx, sync_rx) = mpsc::channel::<u64>(10);
    let (backlog_tx, backlog_rx) = mpsc::channel::<BacklogFetchResult>(100);
    let (block_req_tx, block_req_rx) = mpsc::channel::<u64>(100);
    let (block_res_tx, block_res_rx) = mpsc::channel::<BlockDaInfo>(100);
    let (blob_tx, blob_rx) = mpsc::channel::<BlobSubmission>(100);

    resources.flash.set_channel(fb_rx);
    resources.da.set_channels(da_fb_rx, sync_rx, backlog_rx, block_req_tx, block_res_rx, blob_rx);

    let ws_url = config.flashblocks_ws.to_string();
    let ws_url2 = config.flashblocks_ws.to_string();

    tokio::spawn(async move {
        let _ = run_flashblock_ws_timestamped(ws_url, fb_tx).await;
    });

    tokio::spawn(async move {
        let _ = run_flashblock_ws(ws_url2, da_fb_tx).await;
    });

    let rpc_url = config.rpc.to_string();
    tokio::spawn(async move {
        run_block_fetcher(rpc_url, block_req_rx, block_res_tx).await;
    });

    if let Some(batcher_addr) = config.batcher_address {
        let l1_rpc = config.l1_rpc.to_string();
        tokio::spawn(async move {
            run_l1_batcher_watcher(l1_rpc, batcher_addr, blob_tx).await;
        });
    }

    if let Some(ref op_node_rpc) = config.op_node_rpc {
        let l2_rpc = config.rpc.to_string();
        let rpc_url = op_node_rpc.to_string();
        tokio::spawn(async move {
            fetch_initial_backlog_with_progress(l2_rpc, rpc_url, backlog_tx).await;
        });
    }

    if let Some(ref op_node_rpc) = config.op_node_rpc {
        let rpc_url = op_node_rpc.to_string();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            loop {
                interval.tick().await;
                if let Ok(status) = fetch_sync_status(&rpc_url).await
                    && sync_tx.send(status.safe_l2.number).await.is_err()
                {
                    break;
                }
            }
        });
    }

    let (sys_config_tx, sys_config_rx) = mpsc::channel::<FullSystemConfig>(1);
    resources.set_sys_config_channel(sys_config_rx);

    let l1_rpc = config.l1_rpc.to_string();
    let system_config_addr = config.system_config;
    tokio::spawn(async move {
        if let Ok(cfg) = fetch_full_system_config(&l1_rpc, system_config_addr).await {
            let _ = sys_config_tx.send(cfg).await;
        }
    });
}
