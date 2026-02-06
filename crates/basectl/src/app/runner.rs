use std::time::Duration;

use anyhow::Result;
use base_flashtypes::Flashblock;
use chrono::Local;
use gobrr::LoadTestPhase;
use tokio::sync::mpsc;

use super::{App, Resources, ViewId, views::create_view};
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
pub async fn run_loadtest_tui(config: ChainConfig, file: String) -> Result<()> {
    let mut resources = Resources::new(config.clone());
    start_background_services(&config, &mut resources);

    let handle = gobrr::start_load_test(&file).await?;
    activate_loadtest(&mut resources, handle, file);

    let app = App::new(resources, ViewId::LoadTest);
    app.run(create_view).await
}

/// Given a successful `LoadTestHandle`, activate the load test via gobrr's
/// orchestrator and store the resulting state.
fn activate_loadtest(
    resources: &mut Resources,
    handle: gobrr::LoadTestHandle,
    config_file: String,
) {
    resources.loadtest = Some(gobrr::activate(handle, config_file));
}

/// Run load test with text logs output (for headless/CI environments).
pub async fn run_loadtest_logs(config: ChainConfig, file: String) -> Result<()> {
    let mut resources = Resources::new(config.clone());
    start_background_services(&config, &mut resources);

    let handle = gobrr::start_load_test(&file).await?;
    activate_loadtest(&mut resources, handle, file);

    let mut poll_interval = tokio::time::interval(Duration::from_millis(100));
    let mut print_interval = tokio::time::interval(Duration::from_secs(2));
    let mut last_failed = 0u64;
    let mut last_failure_reasons: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();

    println!("Load test started. Press Ctrl+C to stop.\n");

    loop {
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                println!("\nReceived Ctrl+C, shutting down...");
                break;
            }
            _ = poll_interval.tick() => {
                // Poll all resources to drain channels
                resources.flash.poll();
                resources.da.poll();
                resources.poll_sys_config();
                if let Some(ref mut lt) = resources.loadtest {
                    lt.poll();

                    // Check for new errors and print immediately
                    if let Some(ref stats) = lt.stats
                        && stats.failed > last_failed
                    {
                        // Find new failure reasons
                        for (reason, &count) in &stats.failure_reasons {
                            let prev_count =
                                last_failure_reasons.get(reason).copied().unwrap_or(0);
                            if count > prev_count {
                                println!("[ERROR] {reason} (total: {count})");
                            }
                        }
                        last_failed = stats.failed;
                        last_failure_reasons = stats.failure_reasons.clone();
                    }
                }
            }
            _ = print_interval.tick() => {
                print_loadtest_summary(&resources);

                // Check if complete
                if let Some(ref lt) = resources.loadtest
                    && lt.phase == LoadTestPhase::Complete
                {
                    println!("\nLoad test complete.");
                    break;
                }
            }
        }
    }

    // Trigger shutdown if not already done
    if let Some(ref mut lt) = resources.loadtest
        && let Some(tx) = lt.shutdown_tx.take()
    {
        let _ = tx.send(());
    }

    // Wait a bit for final stats
    tokio::time::sleep(Duration::from_millis(500)).await;
    if let Some(ref mut lt) = resources.loadtest {
        lt.poll();
    }
    print_loadtest_summary(&resources);

    Ok(())
}

fn print_loadtest_summary(resources: &Resources) {
    let Some(lt) = &resources.loadtest else {
        println!("No load test active");
        return;
    };

    let now = Local::now();
    let elapsed = lt.stats.as_ref().map_or(0.0, |s| s.elapsed_secs);
    let elapsed_str = format_elapsed_secs(elapsed);
    let duration_str =
        lt.duration.map_or_else(|| "indefinite".to_string(), |d| format!("{}s", d.as_secs()));

    let phase_str = format!("{}", lt.phase);

    println!(
        "[{}] LoadTest Status (elapsed: {} / {}) - {}",
        now.format("%Y-%m-%d %H:%M:%S"),
        elapsed_str,
        duration_str,
        phase_str
    );
    println!("{}", "─".repeat(60));

    // Block info from flash state (use cumulative values for the block)
    if let Some(entry) = resources.flash.entries.front() {
        let gas_pct = if entry.gas_limit > 0 {
            (entry.cumulative_gas_used as f64 / entry.gas_limit as f64 * 100.0) as u64
        } else {
            0
        };
        println!(
            "Block: {} | Gas: {}/{} ({}%) | Txs: {}",
            entry.block_number,
            format_gas(entry.cumulative_gas_used),
            format_gas(entry.gas_limit),
            gas_pct,
            entry.cumulative_tx_count
        );
    } else {
        println!("Block: N/A");
    }

    // Mempool / txpool status
    if !lt.txpool_status.is_empty() {
        println!("\nMempool:");
        for status in &lt.txpool_status {
            let host_display = status
                .host
                .strip_prefix("http://")
                .or_else(|| status.host.strip_prefix("https://"))
                .unwrap_or(&status.host);
            println!("  {}  pending={}  queued={}", host_display, status.pending, status.queued);
        }
    }

    // Stats
    if let Some(stats) = &lt.stats {
        println!("\nThroughput:");
        println!("  Send TPS: {:.1}  |  Confirmed TPS: {:.1}", stats.tps(), stats.confirmed_tps());

        println!("\nTransactions:");
        println!(
            "  Sent: {}  Confirmed: {}  Pending: {}  Failed: {}  Timed Out: {}",
            stats.sent,
            stats.confirmed,
            stats.pending(),
            stats.failed,
            stats.timed_out
        );

        if stats.fb_inclusion_count > 0 {
            println!("\nFB Inclusion (flashblocks):");
            println!(
                "  P50: {}ms  P95: {}ms  P99: {}ms",
                stats.fb_percentile(50.0),
                stats.fb_percentile(95.0),
                stats.fb_percentile(99.0)
            );
        }

        if stats.block_inclusion_count > 0 {
            println!("\nBlock Inclusion (RPC):");
            println!(
                "  P50: {}ms  P95: {}ms  P99: {}ms",
                stats.block_percentile(50.0),
                stats.block_percentile(95.0),
                stats.block_percentile(99.0)
            );
        }

        if !stats.failure_reasons.is_empty() {
            let total_errors: u64 = stats.failure_reasons.values().sum();
            println!("\nErrors: {total_errors}");
            for (reason, count) in &stats.failure_reasons {
                let display_reason = if reason.len() > 30 { &reason[..30] } else { reason };
                println!("  {display_reason}: {count}");
            }
        }
    } else {
        println!("\nWaiting for stats...");
    }

    println!();
}

fn format_elapsed_secs(secs: f64) -> String {
    let total = secs as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m{seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

fn format_gas(gas: u64) -> String {
    if gas >= 1_000_000_000 {
        format!("{:.1}B", gas as f64 / 1_000_000_000.0)
    } else if gas >= 1_000_000 {
        format!("{:.1}M", gas as f64 / 1_000_000.0)
    } else if gas >= 1_000 {
        format!("{:.0}K", gas as f64 / 1_000.0)
    } else {
        gas.to_string()
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
        if let Err(e) = run_flashblock_ws_timestamped(ws_url.clone(), fb_tx).await {
            tracing::warn!(url = %ws_url, error = %e, "Flashblocks timestamped websocket disconnected");
        }
    });

    tokio::spawn(async move {
        if let Err(e) = run_flashblock_ws(ws_url2.clone(), da_fb_tx).await {
            tracing::warn!(url = %ws_url2, error = %e, "Flashblocks websocket disconnected");
        }
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
