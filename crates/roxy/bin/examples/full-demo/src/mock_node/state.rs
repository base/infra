//! Simulated blockchain state for mock Ethereum nodes.

use std::{
    collections::HashMap,
    sync::{
        RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

/// Simulated blockchain state for a single mock node.
///
/// This state tracks block progression, account balances, and health status
/// for demonstrating roxy's load balancing and failover capabilities.
#[derive(Debug)]
pub(crate) struct NodeState {
    /// Node identifier (e.g., "node-1", "node-2").
    pub name: String,
    /// Current block number - increments over time.
    block_number: AtomicU64,
    /// Account balances: address (lowercase hex) -> wei value.
    balances: RwLock<HashMap<String, String>>,
    /// Chain ID for this simulated network.
    pub chain_id: u64,
    /// Simulated response latency.
    pub latency: Duration,
    /// Whether node is "healthy" (can be toggled for failover demo).
    healthy: AtomicBool,
    /// Request counter for this node.
    request_count: AtomicU64,
}

impl NodeState {
    /// Create a new node state with the given parameters.
    pub(crate) fn new(name: &str, chain_id: u64, initial_block: u64, latency: Duration) -> Self {
        let mut balances = HashMap::new();
        // Pre-populate some balances for demo
        balances.insert(
            "0x742d35cc6634c0532925a3b844bc9e7595f1e3b8".to_string(),
            "0x56bc75e2d63100000".to_string(), // 100 ETH
        );
        balances.insert(
            "0xde0b295669a9fd93d5f28d9ec85e40f4cb697bae".to_string(),
            "0x21e19e0c9bab2400000".to_string(), // 10000 ETH
        );

        Self {
            name: name.to_string(),
            block_number: AtomicU64::new(initial_block),
            balances: RwLock::new(balances),
            chain_id,
            latency,
            healthy: AtomicBool::new(true),
            request_count: AtomicU64::new(0),
        }
    }

    /// Increment block number and return the new value.
    pub(crate) fn advance_block(&self) -> u64 {
        self.block_number.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Get current block number as a hex string.
    pub(crate) fn block_number(&self) -> String {
        format!("0x{:x}", self.block_number.load(Ordering::SeqCst))
    }

    /// Get current block number as u64.
    pub(crate) fn block_number_u64(&self) -> u64 {
        self.block_number.load(Ordering::SeqCst)
    }

    /// Get chain ID as a hex string.
    pub(crate) fn chain_id_hex(&self) -> String {
        format!("0x{:x}", self.chain_id)
    }

    /// Get balance for an address as hex string.
    pub(crate) fn get_balance(&self, address: &str) -> String {
        let balances = self.balances.read().unwrap();
        let normalized = address.to_lowercase();
        balances.get(&normalized).cloned().unwrap_or_else(|| "0x0".to_string())
    }

    /// Set balance for an address.
    #[allow(dead_code)]
    pub(crate) fn set_balance(&self, address: &str, balance: &str) {
        let mut balances = self.balances.write().unwrap();
        balances.insert(address.to_lowercase(), balance.to_string());
    }

    /// Toggle health status for failover demo.
    pub(crate) fn set_healthy(&self, healthy: bool) {
        self.healthy.store(healthy, Ordering::SeqCst);
    }

    /// Check if node is healthy.
    pub(crate) fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::SeqCst)
    }

    /// Increment and return request count.
    pub(crate) fn record_request(&self) -> u64 {
        self.request_count.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Get total request count.
    pub(crate) fn request_count(&self) -> u64 {
        self.request_count.load(Ordering::SeqCst)
    }

    /// Get client version string identifying this node.
    pub(crate) fn client_version(&self) -> String {
        format!("MockNode/{}/v1.0.0", self.name)
    }

    /// Get network version (same as chain_id for simplicity).
    pub(crate) fn net_version(&self) -> String {
        self.chain_id.to_string()
    }

    /// Get simulated gas price as hex.
    pub(crate) fn gas_price(&self) -> String {
        // 20 Gwei
        "0x4a817c800".to_string()
    }
}
