use std::sync::Arc;

use alloy_provider::Provider;
use anyhow::{Context, Result};
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::info;

use crate::{
    blocks::{BlockEvent, BlockWatcher, run_block_logger},
    cli::Args,
    client::{create_provider, create_shared_client},
    confirmer::run_confirmer,
    endpoints::{EndpointPool, SharedEndpointPool},
    funder::Funder,
    sender::{BacklogItem, Preparer, PreparerConfig, Sender},
    stats::{print_final_report, run_stats_reporter},
    tracker::{Stats, TrackerEvent, create_tracker_channel, run_tracker},
    wallet::{derive_signers, get_addresses, parse_funder_key},
};

/// Main entry point for the load test
pub async fn run_load_test(args: Args) -> Result<()> {
    info!("Starting gobrr load tester");

    // Parse and validate configuration
    let network = args.parse_network().map_err(|e| anyhow::anyhow!(e))?;
    let endpoint_distribution = args.parse_endpoint_distribution().map_err(|e| anyhow::anyhow!(e))?;
    let replay_mode = args.parse_replay_mode().map_err(|e| anyhow::anyhow!(e))?;
    let rpc_methods = args.parse_rpc_methods();
    
    if rpc_methods.is_empty() {
        anyhow::bail!("No valid RPC methods specified");
    }

    // Get effective RPC endpoint(s)
    let rpc_endpoints = args.effective_rpc_endpoints().map_err(|e| anyhow::anyhow!(e))?;
    
    // Create endpoint pool
    let endpoint_pool: SharedEndpointPool = Arc::new(
        EndpointPool::from_urls(&rpc_endpoints, endpoint_distribution)
    );
    
    info!(
        endpoints = endpoint_pool.len(),
        distribution = ?endpoint_distribution,
        network = ?network,
        methods = ?rpc_methods.iter().map(|m| m.method_name()).collect::<Vec<_>>(),
        replay_mode = ?replay_mode,
        "Configuration loaded"
    );

    // Parse duration if specified
    let duration = args.parse_duration();
    if let Some(d) = duration {
        info!(duration_secs = d.as_secs(), "Test duration configured");
    } else {
        info!("Running until Ctrl+C");
    }

    // Step 1: Parse funder key
    let funder_signer = parse_funder_key(&args.funder_key).context("Failed to parse funder key")?;
    info!(funder = %funder_signer.address(), "Funder wallet loaded");

    // Step 1b: Create shared HTTP client with connection pooling
    let http_client = create_shared_client();
    info!("Created shared HTTP client with connection pooling");

    // Step 1c: Fetch chain ID from RPC (use first endpoint)
    let primary_rpc = endpoint_pool.urls()[0];
    let provider = create_provider(http_client.clone(), primary_rpc)?;
    let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
    info!(chain_id, rpc = primary_rpc, "Connected to chain");

    // Step 2: Derive sender signers from mnemonic (skipping funder address if derived)
    info!(count = args.sender_count, offset = args.sender_offset, "Deriving sender wallets");
    let sender_signers = derive_signers(
        &args.mnemonic,
        args.sender_count,
        args.sender_offset,
        Some(funder_signer.address()),
    )
    .context("Failed to derive sender signers")?;
    let sender_addresses = get_addresses(&sender_signers);

    // Step 3: Run funding phase using Funder struct
    info!("Starting funding phase");
    let funder = Funder::new(http_client.clone(), primary_rpc, funder_signer, chain_id)
        .await
        .context("Failed to create funder")?;
    funder.fund(&sender_addresses, args.funding_amount).await.context("Funding phase failed")?;

    // Step 4: Create tracker channel and spawn tracker task
    let (tracker_tx, tracker_rx) = create_tracker_channel();
    let tracker_handle = tokio::spawn(run_tracker(tracker_rx));

    // Step 5: Create shutdown broadcast channel
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // Step 5b: Create block event broadcast channel
    let (block_tx, _) = broadcast::channel::<BlockEvent>(64);

    // Step 5c: Create confirmer pending tx channel
    let (confirmer_pending_tx, confirmer_pending_rx) = mpsc::unbounded_channel();

    // Step 6: Spawn preparer and sender tasks for each sender
    let mut handles = Vec::new();
    let backlog_capacity = args.in_flight_per_sender as usize * 2;

    for (i, signer) in sender_signers.into_iter().enumerate() {
        let (backlog_tx, backlog_rx) = mpsc::channel::<BacklogItem>(backlog_capacity);

        let config = PreparerConfig {
            sender_index: i as u32,
            signer,
            calldata_max_size: args.calldata_max_size,
            calldata_load: args.calldata_load,
            rpc_methods: rpc_methods.clone(),
            tx_percentage: args.tx_percentage,
            replay_mode,
            replay_percentage: args.replay_percentage,
        };

        // Create and spawn preparer task
        let shutdown_rx = shutdown_tx.subscribe();
        let client = http_client.clone();
        let rpc = primary_rpc.to_string();
        let preparer_handle = tokio::spawn(async move {
            match Preparer::new(client, &rpc, config).await {
                Ok(preparer) => {
                    if let Err(e) = preparer.run(backlog_tx, shutdown_rx).await {
                        tracing::error!(sender = i, error = %e, "Preparer failed");
                    }
                }
                Err(e) => {
                    tracing::error!(sender = i, error = %e, "Failed to create preparer");
                }
            }
        });
        handles.push(preparer_handle);

        // Create and spawn sender task
        let shutdown_rx = shutdown_tx.subscribe();
        let tracker = tracker_tx.clone();
        let confirmer = confirmer_pending_tx.clone();
        let in_flight = args.in_flight_per_sender;
        let sender_idx = i as u32;
        let client = http_client.clone();
        let pool = Arc::clone(&endpoint_pool);
        let sender_handle = tokio::spawn(async move {
            let sender = Sender::new(client, sender_idx, pool, in_flight, tracker, confirmer);
            if let Err(e) = sender.run(backlog_rx, shutdown_rx).await {
                tracing::error!(sender = i, error = %e, "Sender failed");
            }
        });
        handles.push(sender_handle);
    }

    // Step 7: Create separate shutdown for tasks that run during drain period
    let (drain_shutdown_tx, _) = broadcast::channel::<()>(1);

    // Spawn stats reporter (runs during drain to show progress)
    let stats_shutdown = drain_shutdown_tx.subscribe();
    let stats_tracker = tracker_tx.clone();
    let stats_handle = tokio::spawn(run_stats_reporter(stats_tracker, stats_shutdown));

    // Step 7b: Spawn block watcher using BlockWatcher struct
    let block_watcher_shutdown = drain_shutdown_tx.subscribe();
    let block_tx_clone = block_tx.clone();
    let block_client = http_client.clone();
    let block_rpc = primary_rpc.to_string();
    let block_handle = tokio::spawn(async move {
        match BlockWatcher::new(block_client, &block_rpc) {
            Ok(watcher) => {
                if let Err(e) = watcher.run(block_tx_clone, block_watcher_shutdown).await {
                    tracing::error!(error = %e, "Block watcher failed");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to create block watcher");
            }
        }
    });

    // Step 7c: Spawn block logger (exits when block channel closes)
    let logger_block_rx = block_tx.subscribe();
    let logger_handle = tokio::spawn(run_block_logger(logger_block_rx));

    // Step 7d: Spawn confirmer (exits when block channel closes)
    let confirmer_block_rx = block_tx.subscribe();
    let confirmer_tracker = tracker_tx.clone();
    let confirmer_handle =
        tokio::spawn(run_confirmer(confirmer_pending_rx, confirmer_block_rx, confirmer_tracker));

    // Step 8: Wait for duration or Ctrl+C
    info!("Load test running...");

    if let Some(d) = duration {
        tokio::select! {
            _ = tokio::time::sleep(d) => {
                info!("Duration elapsed");
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl+C received");
            }
        }
    } else {
        tokio::signal::ctrl_c().await.context("Failed to listen for Ctrl+C")?;
        info!("Ctrl+C received");
    }

    // Step 9: Signal shutdown to preparers and senders (stop creating new txs)
    info!("Shutting down...");
    if let Err(e) = shutdown_tx.send(()) {
        tracing::warn!(error = %e, "Failed to send shutdown signal");
    }

    // Step 10: Wait for preparer and sender tasks to complete
    // This ensures all in-flight send_and_track tasks finish before we close channels
    for (i, handle) in handles.into_iter().enumerate() {
        if let Err(e) = handle.await {
            tracing::warn!(task = i, error = %e, "Task panicked during shutdown");
        }
    }

    // Close the confirmer pending channel now that all senders have finished
    drop(confirmer_pending_tx);

    // Wait for pending transactions to confirm
    // Block watcher, logger, and confirmer continue running during this period
    // Exit when: pending == 0, or pending count unchanged for 20 seconds
    info!("Waiting for pending transactions to confirm...");
    let mut last_pending = u64::MAX;
    let mut last_change = std::time::Instant::now();
    let stall_timeout = std::time::Duration::from_secs(20);

    loop {
        let stats = get_final_stats(&tracker_tx).await;
        let pending = stats.pending();

        if pending == 0 {
            info!("All transactions confirmed or timed out");
            break;
        }

        if pending != last_pending {
            last_pending = pending;
            last_change = std::time::Instant::now();
        } else if last_change.elapsed() > stall_timeout {
            tracing::warn!(
                pending,
                "No confirmation progress for 20s, proceeding with final report"
            );
            break;
        }

        info!(pending, "Waiting for confirmations...");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    // Step 11: Now shut down the remaining tasks gracefully
    // Signal drain-phase tasks (stats reporter, block watcher) to stop
    if let Err(e) = drain_shutdown_tx.send(()) {
        tracing::warn!(error = %e, "Failed to send drain shutdown signal");
    }
    let _ = stats_handle.await;
    let _ = block_handle.await;

    // Close block channel to signal consumers (logger, confirmer) to exit
    drop(block_tx);

    // Wait for remaining tasks to complete
    let _ = logger_handle.await;
    let _ = confirmer_handle.await;

    // Step 11: Get final stats and print report
    let final_stats = get_final_stats(&tracker_tx).await;
    if let Err(e) = tracker_tx.send(TrackerEvent::Shutdown) {
        tracing::warn!(error = %e, "Failed to send shutdown to tracker");
    }
    if let Err(e) = tracker_handle.await {
        tracing::warn!(error = %e, "Tracker task panicked");
    }

    print_final_report(&final_stats);

    Ok(())
}

async fn get_final_stats(tracker_tx: &mpsc::UnboundedSender<TrackerEvent>) -> Stats {
    let (tx, rx) = oneshot::channel();
    if tracker_tx.send(TrackerEvent::GetStats(tx)).is_ok() {
        rx.await.unwrap_or_default()
    } else {
        Stats::default()
    }
}
