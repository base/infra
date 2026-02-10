use std::time::Duration;

use alloy_consensus::TxEnvelope;
use alloy_network::{EthereumWallet, ReceiptResponse, TransactionBuilder, eip2718::Encodable2718};
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::{Provider, WalletProvider as AlloyWalletProvider};
use alloy_rpc_client::BatchRequest;
use alloy_rpc_types_eth::{BlockNumberOrTag, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result};
use futures_util::stream::{self, StreamExt};
use tracing::{info, warn};

use crate::client::{WalletProvider, create_wallet_provider};

/// Timeout for waiting for a funding transaction receipt
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(60);

/// Number of requests per JSON-RPC batch
const BATCH_SIZE: usize = 10;
/// Number of batches to run concurrently
const CONCURRENT_BATCHES: usize = 10;
/// Maximum number of outstanding funding transactions before waiting for confirmation
const MAX_OUTSTANDING_TXS: usize = 100;

/// Handles funding sender accounts to a target balance
pub(crate) struct Funder {
    provider: WalletProvider,
    chain_id: u64,
    nonce: u64,
}

impl Funder {
    /// Creates a new Funder with the given HTTP client, RPC URL, signer, and chain ID
    pub(crate) async fn new(
        http_client: reqwest::Client,
        rpc_url: &str,
        signer: PrivateKeySigner,
        chain_id: u64,
    ) -> Result<Self> {
        let funder_address = signer.address();
        let wallet = EthereumWallet::from(signer);
        let provider = create_wallet_provider(http_client, rpc_url, wallet)?;

        let funder_balance =
            provider.get_balance(funder_address).await.context("Failed to get funder balance")?;

        // Get initial nonce from pending state
        let nonce = provider
            .get_transaction_count(funder_address)
            .block_id(BlockNumberOrTag::Pending.into())
            .await
            .context("Failed to get initial nonce")?;

        info!(
            funder = %funder_address,
            balance = %funder_balance,
            nonce,
            "Funder account"
        );

        Ok(Self { provider, chain_id, nonce })
    }

    /// Funds all sender addresses to the target balance using batched RPC calls
    pub(crate) async fn fund(&mut self, addresses: &[Address], target_balance: U256) -> Result<()> {
        // Step 1: Batch get all balances (10 batches of 10 concurrently)
        info!(count = addresses.len(), "Fetching balances for all senders");
        let balances = get_balances_batch(&self.provider, addresses).await?;

        // Step 2: Identify accounts needing funding, assign nonces
        let mut to_fund: Vec<(usize, Address, U256, u64)> = Vec::new();
        for (i, (&addr, &balance)) in addresses.iter().zip(&balances).enumerate() {
            if balance >= target_balance {
                info!(sender = i, address = %addr, balance = %balance, "Already funded");
                continue;
            }
            let amount_needed = target_balance - balance;
            info!(
                sender = i,
                address = %addr,
                current = %balance,
                needed = %amount_needed,
                "Needs funding"
            );
            to_fund.push((i, addr, amount_needed, self.nonce));
            self.nonce += 1;
        }

        if to_fund.is_empty() {
            info!("All senders already funded");
            return Ok(());
        }

        // Estimate fees once for all transactions
        let fees =
            self.provider.estimate_eip1559_fees().await.context("Failed to estimate fees")?;
        let max_fee = fees.max_fee_per_gas;

        // Step 3: Process in waves of MAX_OUTSTANDING_TXS, waiting for confirmation between waves
        let total = to_fund.len();
        let waves: Vec<Vec<(usize, Address, U256, u64)>> =
            to_fund.chunks(MAX_OUTSTANDING_TXS).map(|c| c.to_vec()).collect();

        let wallet = self.provider.wallet();
        let chain_id = self.chain_id;
        let provider = &self.provider;
        let poll_interval = Duration::from_millis(500);

        for (wave_idx, wave) in waves.iter().enumerate() {
            info!(
                wave = wave_idx + 1,
                wave_size = wave.len(),
                total,
                "Sending funding wave"
            );

            // Send this wave in batches concurrently
            let chunks: Vec<Vec<(usize, Address, U256, u64)>> =
                wave.chunks(BATCH_SIZE).map(|c| c.to_vec()).collect();

            let batch_results: Vec<Result<Vec<(usize, Address, B256)>>> = stream::iter(chunks)
                .map(|chunk| async move {
                    send_funding_batch(provider, wallet, chain_id, max_fee, chunk).await
                })
                .buffer_unordered(CONCURRENT_BATCHES)
                .collect()
                .await;

            // Collect sent transactions for this wave
            let mut sent_txs: Vec<(usize, Address, B256)> = Vec::new();
            for result in batch_results {
                match result {
                    Ok(txs) => sent_txs.extend(txs),
                    Err(e) => {
                        warn!(error = %e, "Batch funding failed");
                        return Err(e).context("Failed to send funding batch");
                    }
                }
            }

            if sent_txs.is_empty() {
                continue;
            }

            // Wait for all transactions in this wave to confirm before sending the next wave
            info!(
                wave = wave_idx + 1,
                count = sent_txs.len(),
                "Waiting for funding wave to confirm"
            );
            let start = std::time::Instant::now();

            for (i, address, tx_hash) in &sent_txs {
                loop {
                    if start.elapsed() > RECEIPT_TIMEOUT {
                        anyhow::bail!(
                            "Timeout waiting for funding receipt for sender {i} ({}s)",
                            RECEIPT_TIMEOUT.as_secs()
                        );
                    }

                    match self.provider.get_transaction_receipt(*tx_hash).await {
                        Ok(Some(receipt)) => {
                            if receipt.status() {
                                info!(
                                    sender = i,
                                    address = %address,
                                    tx_hash = %tx_hash,
                                    "Funding confirmed"
                                );
                            } else {
                                anyhow::bail!("Funding transaction failed for sender {i}");
                            }
                            break;
                        }
                        Ok(None) => {
                            tokio::time::sleep(poll_interval).await;
                        }
                        Err(e) => {
                            warn!(sender = i, tx_hash = %tx_hash, error = %e, "Error fetching receipt, retrying");
                            tokio::time::sleep(poll_interval).await;
                        }
                    }
                }
            }

            info!(
                wave = wave_idx + 1,
                count = sent_txs.len(),
                "Funding wave confirmed"
            );
        }

        info!("All senders funded successfully");
        Ok(())
    }
}

/// Fetches balances for all addresses using batched RPC calls with concurrency
async fn get_balances_batch(provider: &WalletProvider, addresses: &[Address]) -> Result<Vec<U256>> {
    let chunks: Vec<&[Address]> = addresses.chunks(BATCH_SIZE).collect();

    // Use buffered (not buffer_unordered) to preserve order
    let results: Vec<Result<Vec<U256>>> = stream::iter(chunks)
        .map(|chunk| async move {
            let mut batch = BatchRequest::new(provider.client());
            let mut futures = Vec::with_capacity(chunk.len());

            for &addr in chunk {
                match batch.add_call::<_, U256>("eth_getBalance", &(addr, "latest")) {
                    Ok(fut) => futures.push(fut),
                    Err(e) => {
                        return Err(anyhow::anyhow!("Failed to add balance call to batch: {e}"));
                    }
                }
            }

            batch.send().await.context("Failed to send balance batch request")?;

            let mut balances = Vec::with_capacity(futures.len());
            for fut in futures {
                let balance = fut.await.context("Failed to get balance from batch")?;
                balances.push(balance);
            }

            Ok(balances)
        })
        .buffered(CONCURRENT_BATCHES)
        .collect()
        .await;

    let mut all_balances = Vec::with_capacity(addresses.len());
    for result in results {
        all_balances.extend(result?);
    }

    Ok(all_balances)
}

/// Sends a batch of funding transactions using JSON-RPC batching
async fn send_funding_batch(
    provider: &WalletProvider,
    wallet: &EthereumWallet,
    chain_id: u64,
    max_fee: u128,
    batch_items: Vec<(usize, Address, U256, u64)>,
) -> Result<Vec<(usize, Address, B256)>> {
    // Sign all transactions first
    let mut signed_txs: Vec<(usize, Address, Bytes, B256)> = Vec::with_capacity(batch_items.len());

    for (i, to, amount, nonce) in batch_items {
        let tx = TransactionRequest::default()
            .with_to(to)
            .with_value(amount)
            .with_nonce(nonce)
            .with_chain_id(chain_id)
            .with_gas_limit(21_000)
            .with_max_fee_per_gas(max_fee)
            .with_max_priority_fee_per_gas(0);

        let tx_envelope: TxEnvelope = tx.build(wallet).await.context("Failed to sign tx")?;
        let tx_hash = *tx_envelope.tx_hash();
        let raw_bytes = Bytes::from(tx_envelope.encoded_2718());

        signed_txs.push((i, to, raw_bytes, tx_hash));
    }

    // Build and send batch request
    let mut batch = BatchRequest::new(provider.client());
    let mut queued: Vec<(usize, Address, B256, _)> = Vec::with_capacity(signed_txs.len());

    for (i, addr, raw_bytes, tx_hash) in signed_txs {
        match batch.add_call::<_, B256>("eth_sendRawTransaction", &(raw_bytes,)) {
            Ok(fut) => queued.push((i, addr, tx_hash, fut)),
            Err(e) => {
                warn!(sender = i, address = %addr, error = ?e, "Failed to add tx to batch");
                return Err(anyhow::anyhow!("Failed to add tx to batch: {e}"));
            }
        }
    }

    if queued.is_empty() {
        return Ok(Vec::new());
    }

    batch.send().await.context("Failed to send funding batch request")?;

    // Collect results
    let mut results = Vec::with_capacity(queued.len());
    for (i, addr, tx_hash, fut) in queued {
        match fut.await {
            Ok(_returned_hash) => {
                results.push((i, addr, tx_hash));
            }
            Err(e) => {
                warn!(sender = i, address = %addr, tx_hash = %tx_hash, error = %e, "Tx failed in batch");
                return Err(anyhow::anyhow!("Tx {tx_hash} failed in batch: {e}"));
            }
        }
    }

    Ok(results)
}
