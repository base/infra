use alloy_network::{EthereumWallet, ReceiptResponse, TransactionBuilder};
use alloy_primitives::{Address, U256};
use alloy_provider::{PendingTransactionBuilder, Provider};
use alloy_rpc_types_eth::BlockNumberOrTag;
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result};
use op_alloy_network::Optimism;
use tracing::{info, warn};

use crate::client::{WalletProvider, create_wallet_provider};

const MAX_RETRIES: u32 = 10;
const FEE_BUMP_PERCENT: u128 = 100;
const MAX_FEE_MULTIPLIER: u128 = 1000;

/// Handles funding sender accounts to a target balance
pub(crate) struct Funder {
    provider: WalletProvider,
    funder_address: Address,
    chain_id: u64,
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

        info!(
            funder = %funder_address,
            balance = %funder_balance,
            "Funder account"
        );

        Ok(Self { provider, funder_address, chain_id })
    }

    /// Funds all sender addresses to the target balance
    pub(crate) async fn fund(&self, addresses: &[Address], target_balance: U256) -> Result<()> {
        let mut pending_txs = Vec::new();

        for (i, &address) in addresses.iter().enumerate() {
            let current_balance =
                self.provider.get_balance(address).await.context("Failed to get sender balance")?;

            if current_balance >= target_balance {
                info!(
                    sender = i,
                    address = %address,
                    balance = %current_balance,
                    "Already funded"
                );
                continue;
            }

            let amount_needed = target_balance - current_balance;
            info!(
                sender = i,
                address = %address,
                current = %current_balance,
                needed = %amount_needed,
                "Funding sender"
            );

            let pending = self.send_with_retry(address, amount_needed).await?;
            pending_txs.push((i, address, pending));
        }

        // Wait for all funding transactions to confirm
        for (i, address, pending) in pending_txs {
            let receipt = pending.get_receipt().await.context("Failed to get funding receipt")?;

            if receipt.status() {
                info!(
                    sender = i,
                    address = %address,
                    tx_hash = %receipt.transaction_hash(),
                    "Funding confirmed"
                );
            } else {
                anyhow::bail!("Funding transaction failed for sender {i}");
            }
        }

        info!("All senders funded successfully");
        Ok(())
    }

    /// Sends a funding transaction with retry logic for underpriced transactions
    async fn send_with_retry(
        &self,
        to: Address,
        amount: U256,
    ) -> Result<PendingTransactionBuilder<Optimism>> {
        let mut last_error = None;
        let mut fee_multiplier = 100u128;

        // Get nonce for the funder account
        let nonce = self
            .provider
            .get_transaction_count(self.funder_address)
            .block_id(BlockNumberOrTag::Pending.into())
            .await
            .context("Failed to get nonce")?;

        for attempt in 0..MAX_RETRIES {
            // Bump fees on retries (up to MAX_FEE_MULTIPLIER)
            if attempt > 0 {
                fee_multiplier = (fee_multiplier + FEE_BUMP_PERCENT).min(MAX_FEE_MULTIPLIER);
                warn!(attempt, fee_multiplier, "Retrying with bumped fees");
            }

            let fees = self
                .provider
                .estimate_eip1559_fees()
                .await
                .context("Failed to estimate fees")?;
            let max_fee = fees.max_fee_per_gas.saturating_mul(fee_multiplier) / 100;
            let priority_fee = fees.max_priority_fee_per_gas.saturating_mul(fee_multiplier) / 100;

            let tx = TransactionRequest::default()
                .with_to(to)
                .with_value(amount)
                .with_nonce(nonce)
                .with_chain_id(self.chain_id)
                .with_gas_limit(21_000) // Standard gas limit for simple ETH transfer
                .with_max_fee_per_gas(max_fee)
                .with_max_priority_fee_per_gas(priority_fee);

            match self.provider.send_transaction(tx.into()).await {
                Ok(pending) => return Ok(pending),
                Err(e) => {
                    let err_str = e.to_string();
                    if err_str.contains("underpriced") || err_str.contains("replacement") {
                        warn!(attempt, error = %e, "Transaction underpriced, retrying with higher fee");
                        last_error = Some(e);
                        continue;
                    }
                    return Err(e).context("Failed to send funding transaction");
                }
            }
        }

        Err(last_error
            .map(|e| anyhow::anyhow!(e))
            .unwrap_or_else(|| anyhow::anyhow!("Max retries exceeded")))
        .context("Failed to send funding transaction after retries")
    }
}
