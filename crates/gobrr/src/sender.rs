use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use alloy_network::{EthereumWallet, NetworkWallet, TransactionBuilder, eip2718::Encodable2718};
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::Provider;
use alloy_rpc_client::BatchRequest;
use alloy_rpc_types_eth::{BlockNumberOrTag, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result};
use rand::{Rng, SeedableRng};
use tokio::{
    sync::{Semaphore, broadcast, mpsc},
    task::JoinSet,
};
use tracing::{debug, error, warn};

use crate::{
    calldata::{generate_large_calldata, generate_small_calldata, should_use_large_calldata},
    cli::{ReplayMode, RpcMethod},
    client::{self, Provider as OpProvider, WalletProvider},
    endpoints::SharedEndpointPool,
    tracker::TrackerEvent,
};

/// Pre-signed transaction ready to send
#[derive(Clone)]
pub(crate) struct PreparedTx {
    pub(crate) raw_bytes: Bytes,
    pub(crate) tx_hash: B256,
    pub(crate) nonce: u64,
    pub(crate) has_large_calldata: bool,
    /// Whether this is a replay of a previous transaction
    pub(crate) is_replay: bool,
}

/// A prepared RPC read call
#[derive(Clone)]
pub(crate) struct PreparedReadCall {
    pub(crate) method: RpcMethod,
    pub(crate) address: Address,
    /// Whether this is a replay of a previous call
    pub(crate) is_replay: bool,
}

/// Item in the request backlog - either a transaction or a read call
pub(crate) enum BacklogItem {
    Transaction(PreparedTx),
    ReadCall(PreparedReadCall),
}

/// Configuration for creating a Preparer
pub(crate) struct PreparerConfig {
    pub(crate) sender_index: u32,
    pub(crate) signer: PrivateKeySigner,
    pub(crate) calldata_max_size: usize,
    pub(crate) calldata_load: u8,
    pub(crate) rpc_methods: Vec<RpcMethod>,
    pub(crate) tx_percentage: u8,
    pub(crate) replay_mode: ReplayMode,
    pub(crate) replay_percentage: u8,
}

/// Maximum items to cache for replay
const REPLAY_CACHE_SIZE: usize = 100;

/// Preparer pre-signs transactions and sends them to a backlog channel
pub(crate) struct Preparer {
    sender_index: u32,
    wallet: EthereumWallet,
    nonce: Arc<AtomicU64>,
    sender_address: Address,
    chain_id: u64,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
    calldata_load: u8,
    calldata_max_size: usize,
    rpc_methods: Vec<RpcMethod>,
    tx_percentage: u8,
    replay_mode: ReplayMode,
    replay_percentage: u8,
    /// Cache of signed transactions for replay (same-tx mode)
    tx_replay_cache: VecDeque<PreparedTx>,
    /// Cache of read calls for replay (same-method mode)
    read_replay_cache: VecDeque<PreparedReadCall>,
}

impl Preparer {
    /// Creates a new Preparer by fetching chain state from the RPC
    pub(crate) async fn new(
        http_client: reqwest::Client,
        rpc_url: &str,
        config: PreparerConfig,
    ) -> Result<Self> {
        let sender_address = config.signer.address();
        let wallet = EthereumWallet::from(config.signer);
        let provider: WalletProvider =
            client::create_wallet_provider(http_client, rpc_url, wallet.clone())?;

        // Fetch initial nonce from latest confirmed state
        let initial_nonce = provider
            .get_transaction_count(sender_address)
            .block_id(BlockNumberOrTag::Latest.into())
            .await
            .context("Failed to get initial nonce")?;

        let nonce = Arc::new(AtomicU64::new(initial_nonce));

        // Get chain ID and base fee
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let base_fee = provider.get_gas_price().await.context("Failed to get gas price")?;
        let max_fee_per_gas = base_fee * 10; // 10x base fee
        let max_priority_fee_per_gas: u128 = 0;

        tracing::info!(
            sender = config.sender_index,
            address = %sender_address,
            initial_nonce = initial_nonce,
            methods = ?config.rpc_methods.iter().map(|m| m.method_name()).collect::<Vec<_>>(),
            replay_mode = ?config.replay_mode,
            replay_percentage = config.replay_percentage,
            "Preparer started"
        );

        Ok(Self {
            sender_index: config.sender_index,
            wallet,
            nonce,
            sender_address,
            chain_id,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            calldata_load: config.calldata_load,
            calldata_max_size: config.calldata_max_size,
            rpc_methods: config.rpc_methods,
            tx_percentage: config.tx_percentage,
            replay_mode: config.replay_mode,
            replay_percentage: config.replay_percentage,
            tx_replay_cache: VecDeque::with_capacity(REPLAY_CACHE_SIZE),
            read_replay_cache: VecDeque::with_capacity(REPLAY_CACHE_SIZE),
        })
    }

    /// Runs the preparer loop, sending prepared items to the backlog channel
    pub(crate) async fn run(
        mut self,
        backlog_tx: mpsc::Sender<BacklogItem>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<()> {
        let mut rng = rand::rngs::StdRng::from_entropy();
        
        loop {
            tokio::select! {
                biased;
                _ = shutdown.recv() => {
                    debug!(sender = self.sender_index, "Preparer shutting down");
                    break;
                }
                result = self.prepare_and_send(&backlog_tx, &mut rng) => {
                    if let Err(e) = result {
                        warn!(sender = self.sender_index, error = %e, "Failed to prepare request");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        }

        Ok(())
    }

    /// Prepares the next item and sends it to the backlog
    async fn prepare_and_send(
        &mut self,
        backlog_tx: &mpsc::Sender<BacklogItem>,
        rng: &mut rand::rngs::StdRng,
    ) -> Result<()> {
        let item = self.prepare_next_item(rng).await?;
        backlog_tx.send(item).await.context("Backlog channel closed")?;
        Ok(())
    }

    /// Check if we should replay instead of creating a new request
    fn should_replay(&self, rng: &mut rand::rngs::StdRng) -> bool {
        if matches!(self.replay_mode, ReplayMode::None) {
            return false;
        }
        if self.replay_percentage == 0 {
            return false;
        }
        rng.gen_range(0..100) < self.replay_percentage
    }

    /// Decides what type of request to prepare based on configuration
    async fn prepare_next_item(&mut self, rng: &mut rand::rngs::StdRng) -> Result<BacklogItem> {
        // Check if we should replay
        if self.should_replay(rng) {
            if let Some(item) = self.get_replay_item(rng) {
                return Ok(item);
            }
            // No cached item available, fall through to create new
        }

        // Check if we should send a transaction or a read call
        let should_send_tx = self.should_send_transaction(rng);
        
        if should_send_tx {
            let tx = self.prepare_next_tx().await?;
            
            // Cache for potential replay (in same-tx mode)
            if matches!(self.replay_mode, ReplayMode::SameTx) {
                self.cache_tx(&tx);
            }
            
            Ok(BacklogItem::Transaction(tx))
        } else {
            let read_call = self.prepare_read_call(rng);
            
            // Cache for potential replay (in same-method mode)
            if matches!(self.replay_mode, ReplayMode::SameMethod) {
                self.cache_read_call(&read_call);
            }
            
            Ok(BacklogItem::ReadCall(read_call))
        }
    }

    /// Get a replay item from cache
    fn get_replay_item(&self, rng: &mut rand::rngs::StdRng) -> Option<BacklogItem> {
        match self.replay_mode {
            ReplayMode::None => None,
            ReplayMode::SameTx => {
                if self.tx_replay_cache.is_empty() {
                    return None;
                }
                let idx = rng.gen_range(0..self.tx_replay_cache.len());
                let mut tx = self.tx_replay_cache[idx].clone();
                tx.is_replay = true;
                Some(BacklogItem::Transaction(tx))
            }
            ReplayMode::SameMethod => {
                if self.read_replay_cache.is_empty() {
                    return None;
                }
                let idx = rng.gen_range(0..self.read_replay_cache.len());
                let mut call = self.read_replay_cache[idx].clone();
                call.is_replay = true;
                Some(BacklogItem::ReadCall(call))
            }
        }
    }

    /// Cache a transaction for potential replay
    fn cache_tx(&mut self, tx: &PreparedTx) {
        if self.tx_replay_cache.len() >= REPLAY_CACHE_SIZE {
            self.tx_replay_cache.pop_front();
        }
        self.tx_replay_cache.push_back(tx.clone());
    }

    /// Cache a read call for potential replay
    fn cache_read_call(&mut self, call: &PreparedReadCall) {
        if self.read_replay_cache.len() >= REPLAY_CACHE_SIZE {
            self.read_replay_cache.pop_front();
        }
        self.read_replay_cache.push_back(call.clone());
    }

    /// Determines if we should send a transaction based on tx_percentage
    fn should_send_transaction(&self, rng: &mut rand::rngs::StdRng) -> bool {
        // If we only have SendRawTransaction, always send transactions
        if self.rpc_methods.len() == 1 && self.rpc_methods[0] == RpcMethod::SendRawTransaction {
            return true;
        }
        
        // If SendRawTransaction is not in methods, never send transactions
        if !self.rpc_methods.contains(&RpcMethod::SendRawTransaction) {
            return false;
        }

        // Otherwise, use tx_percentage
        rng.gen_range(0..100) < self.tx_percentage
    }

    /// Prepares a read call
    fn prepare_read_call(&self, rng: &mut rand::rngs::StdRng) -> PreparedReadCall {
        // Filter to only read methods
        let read_methods: Vec<_> = self.rpc_methods.iter()
            .filter(|m| !m.is_write())
            .collect();
        
        let method = if read_methods.is_empty() {
            // Fallback to GetBalance if no read methods configured
            RpcMethod::GetBalance
        } else {
            **read_methods.get(rng.gen_range(0..read_methods.len())).unwrap()
        };

        PreparedReadCall {
            method,
            address: self.sender_address,
            is_replay: false,
        }
    }

    /// Builds and signs the next transaction
    async fn prepare_next_tx(&self) -> Result<PreparedTx> {
        let current_nonce = self.nonce.fetch_add(1, Ordering::SeqCst);
        let has_large_calldata = should_use_large_calldata(self.calldata_load);

        let calldata = if has_large_calldata {
            generate_large_calldata(self.calldata_max_size)
        } else {
            generate_small_calldata()
        };

        // Build EIP-1559 transaction to address 0x0
        let gas_limit = if has_large_calldata { 5_000_000 } else { 100_000 };
        let tx = TransactionRequest::default()
            .with_to(Address::ZERO)
            .with_value(U256::ZERO)
            .with_input(calldata)
            .with_nonce(current_nonce)
            .with_chain_id(self.chain_id)
            .with_max_fee_per_gas(self.max_fee_per_gas)
            .with_max_priority_fee_per_gas(self.max_priority_fee_per_gas)
            .with_gas_limit(gas_limit)
            .with_from(self.sender_address);

        // Sign the transaction using the wallet
        let tx_envelope = <EthereumWallet as NetworkWallet<alloy_network::Ethereum>>::sign_request(
            &self.wallet,
            tx,
        )
        .await
        .context("Failed to sign transaction")?;

        let tx_hash = *tx_envelope.tx_hash();
        let raw_bytes = Bytes::from(tx_envelope.encoded_2718());

        Ok(PreparedTx { 
            raw_bytes, 
            tx_hash, 
            nonce: current_nonce, 
            has_large_calldata,
            is_replay: false,
        })
    }
}

/// Minimum batch size for transaction batching
const MIN_BATCH_SIZE: usize = 1;
/// Maximum batch size for transaction batching
const MAX_BATCH_SIZE: usize = 5;
/// Timeout to flush partial batches
const BATCH_FLUSH_TIMEOUT: Duration = Duration::from_millis(50);

/// Sender sends pre-signed transactions in batches with multi-endpoint support
pub(crate) struct Sender {
    sender_index: u32,
    http_client: reqwest::Client,
    endpoint_pool: SharedEndpointPool,
    semaphore: Arc<Semaphore>,
    tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
    confirmer_tx: mpsc::UnboundedSender<B256>,
}

impl Sender {
    /// Creates a new Sender with multi-endpoint support
    pub(crate) fn new(
        http_client: reqwest::Client,
        sender_index: u32,
        endpoint_pool: SharedEndpointPool,
        in_flight_limit: u32,
        tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
        confirmer_tx: mpsc::UnboundedSender<B256>,
    ) -> Self {
        let semaphore = Arc::new(Semaphore::new(in_flight_limit as usize));

        Self { 
            sender_index, 
            http_client, 
            endpoint_pool, 
            semaphore, 
            tracker_tx, 
            confirmer_tx 
        }
    }

    /// Creates a provider for the selected endpoint
    fn create_provider_for_endpoint(&self, endpoint_url: &str) -> Result<OpProvider> {
        client::create_provider(self.http_client.clone(), endpoint_url)
    }

    /// Spawns a task to send a batch of items
    async fn spawn_batch(&self, tasks: &mut JoinSet<()>, batch: Vec<BacklogItem>) -> Result<()> {
        let permits = acquire_batch_permits(&self.semaphore, batch.len()).await?;
        
        // Select endpoint for this batch
        let endpoint = self.endpoint_pool.select();
        let endpoint_url = endpoint.url.clone();
        let endpoint_name = endpoint.name.clone();
        
        let provider = self.create_provider_for_endpoint(&endpoint_url)?;
        let tracker = self.tracker_tx.clone();
        let confirmer = self.confirmer_tx.clone();
        let idx = self.sender_index;
        
        tasks.spawn(async move {
            send_batch(idx, batch, provider, tracker, confirmer, endpoint_name).await;
            drop(permits);
        });
        Ok(())
    }

    /// Runs the sender loop, receiving prepared items and sending them in batches
    pub(crate) async fn run(
        self,
        mut backlog_rx: mpsc::Receiver<BacklogItem>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<()> {
        let mut tasks: JoinSet<()> = JoinSet::new();
        let mut batch_buffer: Vec<BacklogItem> = Vec::with_capacity(MAX_BATCH_SIZE);
        let mut rng = rand::rngs::StdRng::from_entropy();

        debug!(sender = self.sender_index, endpoints = self.endpoint_pool.len(), "Sender started with batching enabled");

        // Determine batch size for current batch
        let mut current_batch_target: usize = rng.gen_range(MIN_BATCH_SIZE..=MAX_BATCH_SIZE);

        loop {
            tokio::select! {
                biased;
                _ = shutdown.recv() => {
                    debug!(sender = self.sender_index, "Sender shutting down");
                    // Flush any remaining items in the buffer
                    if !batch_buffer.is_empty() {
                        let batch = std::mem::take(&mut batch_buffer);
                        if let Err(e) = self.spawn_batch(&mut tasks, batch).await {
                            error!(sender = self.sender_index, error = %e, "Failed to spawn final batch");
                        }
                    }
                    // Wait for all spawned tasks to complete
                    while tasks.join_next().await.is_some() {}
                    break;
                }
                Some(item) = backlog_rx.recv() => {
                    batch_buffer.push(item);

                    if batch_buffer.len() >= current_batch_target {
                        let batch = std::mem::take(&mut batch_buffer);
                        if let Err(e) = self.spawn_batch(&mut tasks, batch).await {
                            error!(sender = self.sender_index, error = %e, "Failed to spawn batch");
                        }

                        // Pick a new random batch size for next batch
                        current_batch_target = rng.gen_range(MIN_BATCH_SIZE..=MAX_BATCH_SIZE);
                    }
                }
                _ = tokio::time::sleep(BATCH_FLUSH_TIMEOUT), if !batch_buffer.is_empty() => {
                    // Flush partial batch on timeout
                    let batch = std::mem::take(&mut batch_buffer);
                    if let Err(e) = self.spawn_batch(&mut tasks, batch).await {
                        error!(sender = self.sender_index, error = %e, "Failed to spawn timeout batch");
                    }

                    // Pick a new random batch size for next batch
                    current_batch_target = rng.gen_range(MIN_BATCH_SIZE..=MAX_BATCH_SIZE);
                }
            }
        }

        Ok(())
    }
}

/// Acquire multiple permits for a batch
async fn acquire_batch_permits(
    semaphore: &Arc<Semaphore>,
    count: usize,
) -> Result<Vec<tokio::sync::OwnedSemaphorePermit>> {
    let mut permits = Vec::with_capacity(count);
    for _ in 0..count {
        permits.push(Arc::clone(semaphore).acquire_owned().await?);
    }
    Ok(permits)
}

/// Send a batch of items using JSON-RPC batching
async fn send_batch(
    sender_index: u32,
    batch: Vec<BacklogItem>,
    provider: OpProvider,
    tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
    confirmer_tx: mpsc::UnboundedSender<B256>,
    endpoint_name: Option<String>,
) {
    let batch_size = batch.len();
    debug!(sender = sender_index, batch_size, endpoint = ?endpoint_name, "Sending request batch");

    // Build batch request using BatchRequest directly
    let mut batch_req = BatchRequest::new(provider.client());
    
    // Track what we're sending
    enum PendingRequest {
        Tx { tx: PreparedTx, fut: alloy_rpc_client::Waiter<B256> },
        ReadCall { method: RpcMethod, is_replay: bool, fut: alloy_rpc_client::Waiter<serde_json::Value> },
    }
    
    let mut pending = Vec::with_capacity(batch.len());

    for item in batch {
        match item {
            BacklogItem::Transaction(tx) => {
                match batch_req.add_call::<_, B256>("eth_sendRawTransaction", &(tx.raw_bytes.clone(),)) {
                    Ok(fut) => pending.push(PendingRequest::Tx { tx, fut }),
                    Err(e) => {
                        error!(sender = sender_index, tx_hash = %tx.tx_hash, error = ?e, "Failed to add tx to batch");
                    }
                }
            }
            BacklogItem::ReadCall(read_call) => {
                let params = build_read_call_params(&read_call);
                match batch_req.add_call::<_, serde_json::Value>(read_call.method.method_name(), &params) {
                    Ok(fut) => pending.push(PendingRequest::ReadCall { 
                        method: read_call.method, 
                        is_replay: read_call.is_replay,
                        fut 
                    }),
                    Err(e) => {
                        error!(sender = sender_index, method = %read_call.method, error = ?e, "Failed to add read call to batch");
                    }
                }
            }
        }
    }

    // Send the batch request (single HTTP request for all items)
    if let Err(e) = batch_req.send().await {
        error!(sender = sender_index, batch_size, error = ?e, "Failed to send batch request");
        return;
    }

    // Collect results
    for pending_req in pending {
        match pending_req {
            PendingRequest::Tx { tx, fut } => {
                match fut.await {
                    Ok(_) => {
                        // Track replays separately
                        if tx.is_replay {
                            if let Err(e) = tracker_tx.send(TrackerEvent::ReplaySent {
                                is_tx: true,
                                endpoint: endpoint_name.clone(),
                            }) {
                                warn!(tx_hash = %tx.tx_hash, error = %e, "Failed to send ReplaySent to tracker");
                            }
                        } else {
                            // Notify tracker that tx was sent
                            if let Err(e) = tracker_tx.send(TrackerEvent::TxSent {
                                tx_hash: tx.tx_hash,
                                has_large_calldata: tx.has_large_calldata,
                                endpoint: endpoint_name.clone(),
                            }) {
                                warn!(tx_hash = %tx.tx_hash, error = %e, "Failed to send TxSent to tracker");
                            }

                            // Send tx hash to confirmer after successful send (only non-replays)
                            if let Err(e) = confirmer_tx.send(tx.tx_hash) {
                                warn!(tx_hash = %tx.tx_hash, error = %e, "Failed to send tx hash to confirmer");
                            }
                        }

                        debug!(
                            sender = sender_index,
                            tx_hash = %tx.tx_hash,
                            nonce = tx.nonce,
                            is_replay = tx.is_replay,
                            "Transaction sent via batch"
                        );
                    }
                    Err(e) => {
                        // Replays are expected to fail (already known), so log at debug level
                        if tx.is_replay {
                            debug!(
                                sender = sender_index,
                                nonce = tx.nonce,
                                tx_hash = %tx.tx_hash,
                                error = ?e,
                                "Replay tx failed (expected)"
                            );
                        } else {
                            error!(
                                sender = sender_index,
                                nonce = tx.nonce,
                                tx_hash = %tx.tx_hash,
                                error = ?e,
                                "Batch tx failed"
                            );
                        }
                    }
                }
            }
            PendingRequest::ReadCall { method, is_replay, fut } => {
                match fut.await {
                    Ok(_) => {
                        if is_replay {
                            if let Err(e) = tracker_tx.send(TrackerEvent::ReplaySent {
                                is_tx: false,
                                endpoint: endpoint_name.clone(),
                            }) {
                                warn!(method = %method, error = %e, "Failed to send ReplaySent to tracker");
                            }
                        } else {
                            // Notify tracker that read call completed
                            if let Err(e) = tracker_tx.send(TrackerEvent::RpcCallSent {
                                method,
                                endpoint: endpoint_name.clone(),
                            }) {
                                warn!(method = %method, error = %e, "Failed to send RpcCallSent to tracker");
                            }
                        }

                        debug!(sender = sender_index, method = %method, is_replay, "Read call completed");
                    }
                    Err(e) => {
                        error!(sender = sender_index, method = %method, error = ?e, "Read call failed");
                    }
                }
            }
        }
    }
}

/// Builds parameters for a read call based on the method type
fn build_read_call_params(read_call: &PreparedReadCall) -> serde_json::Value {
    match read_call.method {
        RpcMethod::GetBalance => {
            serde_json::json!([format!("{:?}", read_call.address), "latest"])
        }
        RpcMethod::GetTransactionCount => {
            serde_json::json!([format!("{:?}", read_call.address), "latest"])
        }
        RpcMethod::BlockNumber => {
            serde_json::json!([])
        }
        RpcMethod::GasPrice => {
            serde_json::json!([])
        }
        RpcMethod::ChainId => {
            serde_json::json!([])
        }
        RpcMethod::GetBlockByNumber => {
            serde_json::json!(["latest", false])
        }
        RpcMethod::Call => {
            // Simple eth_call to check balance (no data)
            serde_json::json!([{
                "to": format!("{:?}", read_call.address),
                "data": "0x"
            }, "latest"])
        }
        RpcMethod::SendRawTransaction => {
            // This shouldn't happen for read calls
            serde_json::json!([])
        }
    }
}
