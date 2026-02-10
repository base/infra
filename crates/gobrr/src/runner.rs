use std::sync::Arc;

use alloy_primitives::{Address, B256};
use alloy_provider::Provider;
use alloy_rpc_client::BatchRequest;
use alloy_rpc_types_eth::BlockNumberOrTag;
use anyhow::{Context, Result};
use tokio::sync::{Semaphore, broadcast, mpsc};
use tracing::{info, warn};

use crate::{
    SenderId,
    blocks::{BlockWatcher, OpBlock, run_block_logger},
    client::{create_provider, create_shared_client},
    config::{TestConfig, TxSelector},
    confirmer::run_confirmer,
    flashblock_watcher::run_flashblock_watcher,
    funder::Funder,
    handle::LoadTestHandle,
    sender::Sender,
    signer::{ResignRequest, SignedTx, Signer},
    stats::run_stats_reporter,
    tracker::run_tracker,
    wallet::{derive_signers, get_addresses, parse_funder_key},
};

/// Start a load test and return a handle for controlling it.
///
/// This runs the setup phase (load config, fund wallets, spawn pipeline tasks)
/// and returns a [`LoadTestHandle`] that can be used to poll stats, trigger
/// shutdown, and drain pending transactions.
pub async fn start_load_test(config_path: &str) -> Result<LoadTestHandle> {
    info!("Starting gobrr load tester");

    // Load config file
    let config = TestConfig::load(config_path)?;

    // Parse duration if specified
    let duration = config.parse_duration()?;
    if let Some(d) = &duration {
        info!(duration_secs = d.as_secs(), "Test duration configured");
    } else {
        info!("Running until Ctrl+C");
    }

    // Parse funding amount
    let funding_amount = config.parse_funding_amount()?;

    // Step 1: Parse funder key
    let funder_signer =
        parse_funder_key(&config.funder_key).context("Failed to parse funder key")?;
    info!(funder = %funder_signer.address(), "Funder wallet loaded");

    // Step 1b: Create shared HTTP client with connection pooling
    let http_client = create_shared_client();
    info!("Created shared HTTP client with connection pooling");

    // Step 1c: Fetch chain ID and base fee from RPC (once for all signers)
    let provider = create_provider(http_client.clone(), &config.rpc)?;
    let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
    let latest_block = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .context("Failed to get latest block")?
        .context("Latest block not found")?;
    let base_fee = latest_block.header.base_fee_per_gas.context("Latest block missing base fee")?;
    // Gas multiplier applied to base fee (testnet buffer)
    const GAS_FEE_MULTIPLIER: u128 = 100;
    let max_fee_per_gas = u128::from(base_fee).saturating_mul(GAS_FEE_MULTIPLIER);
    info!(chain_id, base_fee, max_fee_per_gas, "Connected to chain");

    // Step 2: Derive sender signers from mnemonic (skipping funder address if derived)
    info!(count = config.sender_count, offset = config.sender_offset, "Deriving sender wallets");
    let sender_signers = derive_signers(
        &config.mnemonic,
        config.sender_count,
        config.sender_offset,
        &[funder_signer.address()],
    )
    .context("Failed to derive sender signers")?;
    let sender_addresses = get_addresses(&sender_signers);

    // Step 2b: Clear mempools on management hosts if configured (include funder address)
    if !config.txpool_hosts.is_empty() {
        let mut addresses_to_clear = sender_addresses.clone();
        addresses_to_clear.push(funder_signer.address());
        clear_mempools(&http_client, &config.txpool_hosts, &addresses_to_clear).await;
    }

    // Step 3: Run funding phase using Funder struct
    info!("Starting funding phase");
    let mut funder = Funder::new(http_client.clone(), &config.rpc, funder_signer, chain_id)
        .await
        .context("Failed to create funder")?;
    funder.fund(&sender_addresses, funding_amount).await.context("Funding phase failed")?;

    // Build TxSelector from config
    let tx_selector = TxSelector::new(&config.transactions)?;

    // Step 4: Create tracker channel and spawn tracker task
    let (tracker_tx, tracker_rx) = mpsc::unbounded_channel();
    let tracker_handle = tokio::spawn(run_tracker(tracker_rx));

    // Step 5: Create shutdown broadcast channel
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // Step 5b: Create block event broadcast channel
    let (block_tx, _) = broadcast::channel::<OpBlock>(64);

    // Step 5c: Create confirmer pending tx channel
    let (confirmer_pending_tx, confirmer_pending_rx) = mpsc::unbounded_channel();

    // Step 5d: Create flashblock pending tx channel
    let (flashblock_pending_tx, flashblock_pending_rx) = mpsc::unbounded_channel();

    // Step 5e: Set up rate limiter if target_tps is configured
    let rate_limiter: Option<Arc<Semaphore>> = config.target_tps.map(|tps| {
        info!(target_tps = tps, "Rate limiter enabled");
        Arc::new(Semaphore::new(0))
    });

    // Spawn rate limiter replenisher task if configured
    let rate_limiter_handle =
        if let Some((tps, limiter)) = config.target_tps.zip(rate_limiter.clone()) {
            let mut replenish_shutdown = shutdown_tx.subscribe();
            let handle = tokio::spawn(async move {
                // Tick 10x per second, using fractional accumulation to support low TPS
                let permits_per_tick = tps as f64 / 10.0;
                let max_permits = (tps * 2) as usize; // cap at 2 seconds of burst
                let mut carry = 0.0;
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));

                loop {
                    tokio::select! {
                        biased;
                        _ = replenish_shutdown.recv() => break,
                        _ = interval.tick() => {
                            let available = limiter.available_permits();
                            if available < max_permits {
                                carry += permits_per_tick;
                                let mut to_add = carry.floor() as usize;
                                let max_add = max_permits - available;
                                if to_add > max_add {
                                    to_add = max_add;
                                }
                                if to_add > 0 {
                                    limiter.add_permits(to_add);
                                    carry -= to_add as f64;
                                }
                            } else {
                                carry = 0.0;
                            }
                        }
                    }
                }
            });
            Some(handle)
        } else {
            None
        };

    // Step 6: Spawn signer and sender tasks in batches to avoid overwhelming RPC
    let mut handles = Vec::new();
    let backlog_capacity = config.in_flight_per_sender as usize * 2;
    const BATCH_SIZE: usize = 50;

    let sender_signers: Vec<_> = sender_signers.into_iter().enumerate().collect();
    for batch in sender_signers.chunks(BATCH_SIZE) {
        // Spawn signer initialization tasks for this batch
        let mut init_handles = Vec::with_capacity(batch.len());
        for (i, signer) in batch.iter().cloned() {
            let sender_id = i as SenderId;
            let client = http_client.clone();
            let rpc = config.rpc.clone();
            let limiter = rate_limiter.clone();
            let selector = tx_selector.clone();

            let handle = tokio::spawn(async move {
                Signer::new(
                    client,
                    &rpc,
                    signer,
                    sender_id,
                    limiter,
                    selector,
                    chain_id,
                    max_fee_per_gas,
                )
                .await
                .map(|s| (i, s))
            });
            init_handles.push(handle);
        }

        // Wait for all signers in this batch to initialize
        for handle in init_handles {
            match handle.await {
                Ok(Ok((i, signer_instance))) => {
                    let sender_id = i as SenderId;

                    // Create channels for the pipeline
                    let (signed_tx, signed_rx) = mpsc::channel::<SignedTx>(backlog_capacity);
                    let (resign_tx, resign_rx) =
                        mpsc::channel::<ResignRequest>(config.in_flight_per_sender as usize);

                    // Spawn signer task
                    let shutdown_rx = shutdown_tx.subscribe();
                    let signer_block_rx = block_tx.subscribe();
                    let signer_handle = tokio::spawn(async move {
                        if let Err(e) = signer_instance
                            .run(resign_rx, signed_tx, signer_block_rx, shutdown_rx)
                            .await
                        {
                            tracing::error!(sender = i, error = %e, "Signer failed");
                        }
                    });
                    handles.push(signer_handle);

                    // Spawn sender task
                    let shutdown_rx = shutdown_tx.subscribe();
                    let tracker = tracker_tx.clone();
                    let confirmer = confirmer_pending_tx.clone();
                    let flashblock = flashblock_pending_tx.clone();
                    let rpc = config.rpc.clone();
                    let in_flight = config.in_flight_per_sender;
                    let client = http_client.clone();
                    let sender_handle = tokio::spawn(async move {
                        match Sender::new(
                            client, sender_id, &rpc, in_flight, tracker, confirmer, flashblock,
                            resign_tx,
                        ) {
                            Ok(sender) => {
                                if let Err(e) = sender.run(signed_rx, shutdown_rx).await {
                                    tracing::error!(sender = i, error = %e, "Sender failed");
                                }
                            }
                            Err(e) => {
                                tracing::error!(sender = i, error = %e, "Failed to create sender");
                            }
                        }
                    });
                    handles.push(sender_handle);
                }
                Ok(Err(e)) => {
                    tracing::error!(error = ?e, "Failed to create signer");
                }
                Err(e) => {
                    tracing::error!(error = %e, "Signer init task panicked");
                }
            }
        }

        info!(batch_size = batch.len(), "Initialized signer batch");
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
    let block_rpc = config.rpc.clone();
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

    // Step 7e: Spawn flashblock watcher
    let flashblock_shutdown = drain_shutdown_tx.subscribe();
    let flashblock_tracker = tracker_tx.clone();
    let flashblock_ws_url = config.flashblocks_ws.clone();
    let flashblock_handle = tokio::spawn(run_flashblock_watcher(
        flashblock_ws_url,
        flashblock_pending_rx,
        flashblock_tracker,
        flashblock_shutdown,
    ));

    info!("Load test running...");

    let txpool_hosts = config.txpool_hosts.clone();
    let target_tps = config.target_tps;

    Ok(LoadTestHandle::new(
        tracker_tx,
        shutdown_tx,
        drain_shutdown_tx,
        block_tx,
        handles,
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
    ))
}

/// Clears pending transactions for all sender addresses on each management host.
/// Calls `txpool_removeSender` in a JSON-RPC batch for each host. Errors are logged but don't fail the test.
async fn clear_mempools(
    http_client: &reqwest::Client,
    txpool_hosts: &[String],
    addresses: &[Address],
) {
    info!(
        hosts = txpool_hosts.len(),
        addresses = addresses.len(),
        "Clearing mempools on management hosts"
    );

    for host in txpool_hosts {
        let provider = match create_provider(http_client.clone(), host) {
            Ok(p) => p,
            Err(e) => {
                warn!(host, error = %e, "Failed to create provider for management host");
                continue;
            }
        };

        let mut batch = BatchRequest::new(provider.client());
        let mut futures = Vec::with_capacity(addresses.len());
        let mut addrs = Vec::with_capacity(addresses.len());

        for addr in addresses {
            match batch.add_call::<_, Vec<B256>>("txpool_removeSender", &(*addr,)) {
                Ok(fut) => {
                    futures.push(fut);
                    addrs.push(*addr);
                }
                Err(e) => {
                    warn!(host, address = %addr, error = ?e, "Failed to add removeSender to batch");
                }
            }
        }

        if let Err(e) = batch.send().await {
            warn!(host, error = ?e, "Failed to send mempool clear batch");
            continue;
        }

        let mut total_removed = 0usize;
        for (fut, addr) in futures.into_iter().zip(addrs) {
            match fut.await {
                Ok(removed) => {
                    total_removed += removed.len();
                    if !removed.is_empty() {
                        info!(
                            host,
                            address = %addr,
                            removed = removed.len(),
                            "Cleared pending txs from mempool"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        host,
                        address = %addr,
                        error = %e,
                        "Failed to clear mempool for sender"
                    );
                }
            }
        }

        info!(host, total_removed, "Mempool clear complete for host");
    }
}
