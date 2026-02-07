use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use base_flashtypes::Flashblock;
use tokio::sync::{mpsc, oneshot};

use super::{App, Resources, ViewId, resources::LoadTestSetup, views::create_view};
use crate::{
    config::ChainConfig,
    l1_client::{FullSystemConfig, fetch_full_system_config},
    rpc::{
        BacklogFetchResult, BlobSubmission, BlockDaInfo, TimestampedFlashblock,
        fetch_initial_backlog_with_progress, fetch_sync_status, run_block_fetcher,
        run_flashblock_ws, run_flashblock_ws_timestamped, run_l1_batcher_watcher,
    },
};

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

/// Given a successful `LoadTestHandle`, activate the load test via gobrr's
/// orchestrator and store the resulting state.
pub(super) fn activate_loadtest(
    resources: &mut Resources,
    handle: gobrr::LoadTestHandle,
    config_file: String,
) {
    resources.loadtest = Some(gobrr::activate(handle, config_file));
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
