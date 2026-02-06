use std::sync::Arc;

use alloy_network::{EthereumWallet, NetworkWallet, TransactionBuilder, eip2718::Encodable2718};
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::{BlockNumberOrTag, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result};
use rand::{SeedableRng, rngs::StdRng};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast, mpsc};
use tracing::{debug, info, warn};

use crate::{
    SenderId,
    blocks::OpBlock,
    client::{self, WalletProvider},
    config::{TxSelector, TxType},
};

/// Unsigned transaction content — no nonce, fees, or signature
pub(crate) struct UnsignedTx {
    pub(crate) to: Address,
    pub(crate) value: U256,
    pub(crate) input: Bytes,
    pub(crate) gas_limit: u64,
    pub(crate) tx_type: TxType,
}

/// Signed transaction ready to send
pub(crate) struct SignedTx {
    pub(crate) raw_bytes: Bytes,
    pub(crate) tx_hash: B256,
    pub(crate) nonce: u64,
    pub(crate) tx_type: TxType,
    pub(crate) unsigned: UnsignedTx,
    pub(crate) is_retry: bool,
}

/// Request from Sender to Signer to re-sign with bumped fees
pub(crate) struct ResignRequest {
    pub(crate) nonce: u64,
    pub(crate) unsigned: UnsignedTx,
}

/// Gas multiplier applied to base fee (testnet buffer)
const GAS_FEE_MULTIPLIER: u128 = 100;

/// Signer generates, signs, and forwards transactions to the sender
pub(crate) struct Signer {
    sender_id: SenderId,
    wallet: EthereumWallet,
    nonce: u64,
    sender_address: Address,
    chain_id: u64,
    max_fee_per_gas: u128,
    rate_limiter: Option<Arc<Semaphore>>,
    tx_selector: TxSelector,
    rng: StdRng,
}

impl Signer {
    /// Creates a new Signer with pre-fetched chain state
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new(
        http_client: reqwest::Client,
        rpc_url: &str,
        signer: PrivateKeySigner,
        sender_id: SenderId,
        rate_limiter: Option<Arc<Semaphore>>,
        tx_selector: TxSelector,
        chain_id: u64,
        max_fee_per_gas: u128,
    ) -> Result<Self> {
        let sender_address = signer.address();
        let wallet = EthereumWallet::from(signer);
        let provider: WalletProvider =
            client::create_wallet_provider(http_client, rpc_url, wallet.clone())?;

        // Fetch initial nonce from pending state to account for mempool transactions
        let initial_nonce = provider
            .get_transaction_count(sender_address)
            .block_id(BlockNumberOrTag::Pending.into())
            .await
            .context("Failed to get initial nonce")?;

        info!(
            sender = sender_id,
            address = %sender_address,
            initial_nonce = initial_nonce,
            "Signer started"
        );

        Ok(Self {
            sender_id,
            wallet,
            nonce: initial_nonce,
            sender_address,
            chain_id,
            max_fee_per_gas,
            rate_limiter,
            tx_selector,
            rng: StdRng::from_entropy(),
        })
    }

    /// Generates the next unsigned transaction
    fn prepare_next(&mut self) -> UnsignedTx {
        let tx_params = self.tx_selector.select(&mut self.rng).build(&mut self.rng);
        UnsignedTx {
            to: tx_params.to,
            value: tx_params.value,
            input: tx_params.input,
            gas_limit: tx_params.gas_limit,
            tx_type: tx_params.tx_type,
        }
    }

    /// Runs the signer loop
    pub(crate) async fn run(
        mut self,
        mut resign_rx: mpsc::Receiver<ResignRequest>,
        signed_tx: mpsc::Sender<SignedTx>,
        mut block_rx: broadcast::Receiver<OpBlock>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<()> {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.recv() => {
                    debug!(sender = self.sender_id, "Signer shutting down");
                    break;
                }
                Some(req) = resign_rx.recv() => {
                    if let Err(e) = self.handle_resign(req, &signed_tx).await {
                        warn!(sender = self.sender_id, error = %e, "Failed to re-sign transaction");
                    }
                }
                result = block_rx.recv() => {
                    match result {
                        Ok(block) => self.on_new_block(&block),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            debug!(sender = self.sender_id, missed = n, "Signer lagged behind block events");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            debug!(sender = self.sender_id, "Block channel closed in signer");
                        }
                    }
                }
                permit = self.acquire_rate_permit() => {
                    match permit {
                        Ok(rate_permit) => {
                            // Default branch: generate and sign a new transaction
                            let unsigned = self.prepare_next();
                            if let Err(e) = self.handle_sign(unsigned, &signed_tx, rate_permit).await
                            {
                                warn!(
                                    sender = self.sender_id,
                                    error = %e,
                                    "Failed to sign transaction"
                                );
                            }
                        }
                        Err(e) => {
                            warn!(
                                sender = self.sender_id,
                                error = %e,
                                "Rate limiter closed"
                            );
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Acquire a rate-limiter permit if configured.
    async fn acquire_rate_permit(&self) -> Result<Option<OwnedSemaphorePermit>> {
        let Some(limiter) = &self.rate_limiter else {
            return Ok(None);
        };
        let permit = Arc::clone(limiter).acquire_owned().await.context("Rate limiter closed")?;
        Ok(Some(permit))
    }

    /// Update gas fee from a new block's base fee
    fn on_new_block(&mut self, block: &OpBlock) {
        if let Some(base_fee) = block.header.inner.base_fee_per_gas {
            let new_max_fee = u128::from(base_fee).saturating_mul(GAS_FEE_MULTIPLIER);
            if new_max_fee != self.max_fee_per_gas {
                debug!(
                    sender = self.sender_id,
                    old_max_fee = self.max_fee_per_gas,
                    new_max_fee,
                    block = block.header.number,
                    "Gas price updated from block"
                );
                self.max_fee_per_gas = new_max_fee;
            }
        }
    }

    /// Assigns nonce, signs, and forwards to sender
    async fn handle_sign(
        &mut self,
        unsigned: UnsignedTx,
        signed_tx: &mpsc::Sender<SignedTx>,
        rate_permit: Option<OwnedSemaphorePermit>,
    ) -> Result<()> {
        let nonce = self.nonce;
        self.nonce += 1;

        let tx = self.build_and_sign(&unsigned, nonce).await?;
        let signed = SignedTx {
            raw_bytes: tx.0,
            tx_hash: tx.1,
            nonce,
            tx_type: unsigned.tx_type,
            unsigned,
            is_retry: false,
        };

        signed_tx.send(signed).await.context("Sender channel closed")?;

        if let Some(permit) = rate_permit {
            std::mem::forget(permit);
        }
        Ok(())
    }

    /// Re-signs a transaction with bumped gas fees
    async fn handle_resign(
        &mut self,
        req: ResignRequest,
        signed_tx: &mpsc::Sender<SignedTx>,
    ) -> Result<()> {
        // Bump max fee by 20%
        self.max_fee_per_gas = self.max_fee_per_gas * 120 / 100;
        debug!(
            sender = self.sender_id,
            bumped_max_fee = self.max_fee_per_gas,
            "Gas price bumped 20% for re-sign"
        );

        let tx = self.build_and_sign(&req.unsigned, req.nonce).await?;
        let signed = SignedTx {
            raw_bytes: tx.0,
            tx_hash: tx.1,
            nonce: req.nonce,
            tx_type: req.unsigned.tx_type,
            unsigned: req.unsigned,
            is_retry: true,
        };

        signed_tx.send(signed).await.context("Sender channel closed")?;
        Ok(())
    }

    /// Builds a `TransactionRequest` and signs it with the wallet
    async fn build_and_sign(&self, unsigned: &UnsignedTx, nonce: u64) -> Result<(Bytes, B256)> {
        let tx = TransactionRequest::default()
            .with_to(unsigned.to)
            .with_value(unsigned.value)
            .with_input(unsigned.input.clone())
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_max_fee_per_gas(self.max_fee_per_gas)
            .with_max_priority_fee_per_gas(0)
            .with_gas_limit(unsigned.gas_limit)
            .with_from(self.sender_address);

        let tx_envelope = <EthereumWallet as NetworkWallet<alloy_network::Ethereum>>::sign_request(
            &self.wallet,
            tx,
        )
        .await
        .context("Failed to sign transaction")?;

        let tx_hash = *tx_envelope.tx_hash();
        let raw_bytes = Bytes::from(tx_envelope.encoded_2718());

        Ok((raw_bytes, tx_hash))
    }
}
