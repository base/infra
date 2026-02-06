use alloy_primitives::U256;
use clap::Parser;

use crate::endpoints::EndpointDistribution;

/// Network preset for common configurations
#[derive(Debug, Clone, Copy, Default)]
pub enum NetworkPreset {
    /// Base Sepolia testnet
    Sepolia,
    /// Base Sepolia Alpha testnet
    SepoliaAlpha,
    /// Custom network (use --rpc or --rpc-endpoints)
    #[default]
    Custom,
}

impl std::str::FromStr for NetworkPreset {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sepolia" | "base-sepolia" => Ok(Self::Sepolia),
            "sepolia-alpha" | "base-sepolia-alpha" | "alpha" => Ok(Self::SepoliaAlpha),
            "custom" => Ok(Self::Custom),
            _ => Err(format!("Unknown network preset: {s}. Use 'sepolia', 'sepolia-alpha', or 'custom'")),
        }
    }
}

impl NetworkPreset {
    /// Returns the default RPC endpoint for this network preset
    pub fn default_rpc(&self) -> Option<&'static str> {
        match self {
            Self::Sepolia => Some("https://base-sepolia.cbhq.net"),
            Self::SepoliaAlpha => Some("https://base-sepolia-alpha.cbhq.net"),
            Self::Custom => None,
        }
    }

    /// Returns the chain ID for this network preset
    pub fn chain_id(&self) -> Option<u64> {
        match self {
            Self::Sepolia => Some(84532),
            Self::SepoliaAlpha => Some(84532), // Same chain, different endpoint
            Self::Custom => None,
        }
    }
}

/// Replay mode for testing caching/deduplication
#[derive(Debug, Clone, Copy, Default)]
pub enum ReplayMode {
    /// No replay - all requests are unique
    #[default]
    None,
    /// Replay same signed transaction (tests deduplication)
    SameTx,
    /// Replay same RPC method/params from different senders
    SameMethod,
}

impl std::str::FromStr for ReplayMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" | "off" | "disabled" => Ok(Self::None),
            "same-tx" | "sametx" | "tx" => Ok(Self::SameTx),
            "same-method" | "samemethod" | "method" => Ok(Self::SameMethod),
            _ => Err(format!("Unknown replay mode: {s}. Use 'none', 'same-tx', or 'same-method'")),
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "gobrr")]
#[command(
    about = "Ethereum/OP Stack load tester - derives addresses from mnemonic, funds them, and runs concurrent transactions against proxyd"
)]
pub struct Args {
    // ========== Network Configuration ==========
    
    /// Network preset (sepolia, sepolia-alpha, custom)
    #[arg(long, default_value = "custom")]
    pub network: String,

    /// Single RPC endpoint URL (mutually exclusive with --rpc-endpoints)
    #[arg(long)]
    pub rpc: Option<String>,

    /// Comma-separated list of RPC endpoints for load distribution
    #[arg(long)]
    pub rpc_endpoints: Option<String>,

    /// Endpoint distribution strategy (round-robin, random, weighted)
    #[arg(long, default_value = "round-robin")]
    pub endpoint_distribution: String,

    // ========== Wallet Configuration ==========

    /// HD wallet mnemonic for deriving sender addresses
    #[arg(long)]
    pub mnemonic: String,

    /// Funder private key (hex, with or without 0x prefix)
    #[arg(long)]
    pub funder_key: String,

    /// Amount of ETH to fund each sender (in wei)
    #[arg(long)]
    pub funding_amount: U256,

    /// Number of sender accounts to derive and use
    #[arg(long, default_value = "10")]
    pub sender_count: u32,

    /// Offset for sender derivation (skip first N addresses from mnemonic)
    #[arg(long, default_value = "0")]
    pub sender_offset: u32,

    // ========== Load Configuration ==========

    /// Maximum concurrent in-flight transactions per sender
    #[arg(long, default_value = "16")]
    pub in_flight_per_sender: u32,

    /// Maximum calldata size in bytes for large transactions
    #[arg(long, default_value = "65536")]
    pub calldata_max_size: usize,

    /// Percentage of transactions that should have large calldata (0-100)
    #[arg(long, default_value = "10")]
    pub calldata_load: u8,

    /// Test duration (e.g., "60s", "5m"). If not specified, runs until Ctrl+C
    #[arg(long)]
    pub duration: Option<String>,

    // ========== RPC Method Variation ==========

    /// Comma-separated list of RPC methods to call
    /// Default: eth_sendRawTransaction only
    /// Options: eth_sendRawTransaction, eth_getBalance, eth_call, eth_blockNumber, eth_getTransactionCount
    #[arg(long, default_value = "eth_sendRawTransaction")]
    pub rpc_methods: String,

    /// Percentage of requests that should be transactions (vs read calls) (0-100)
    /// Only applies when multiple RPC methods are specified
    #[arg(long, default_value = "80")]
    pub tx_percentage: u8,

    // ========== Replay Mode ==========

    /// Replay mode for testing caching/deduplication (none, same-tx, same-method)
    #[arg(long, default_value = "none")]
    pub replay_mode: String,

    /// Percentage of requests that should be replays (0-100)
    #[arg(long, default_value = "0")]
    pub replay_percentage: u8,
}

impl Args {
    pub fn parse_duration(&self) -> Option<std::time::Duration> {
        self.duration.as_ref().map(|d| parse_duration_string(d))
    }

    /// Parses the network preset
    pub fn parse_network(&self) -> Result<NetworkPreset, String> {
        self.network.parse()
    }

    /// Parses the endpoint distribution strategy
    pub fn parse_endpoint_distribution(&self) -> Result<EndpointDistribution, String> {
        self.endpoint_distribution.parse()
    }

    /// Parses the replay mode
    pub fn parse_replay_mode(&self) -> Result<ReplayMode, String> {
        self.replay_mode.parse()
    }

    /// Returns the effective RPC endpoint(s) based on args
    /// Priority: --rpc-endpoints > --rpc > network preset default
    pub fn effective_rpc_endpoints(&self) -> Result<String, String> {
        if let Some(endpoints) = &self.rpc_endpoints {
            return Ok(endpoints.clone());
        }

        if let Some(rpc) = &self.rpc {
            return Ok(rpc.clone());
        }

        let network = self.parse_network()?;
        network.default_rpc()
            .map(String::from)
            .ok_or_else(|| "No RPC endpoint specified. Use --rpc, --rpc-endpoints, or a network preset".to_string())
    }

    /// Parses the RPC methods list
    pub fn parse_rpc_methods(&self) -> Vec<RpcMethod> {
        self.rpc_methods
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect()
    }

    /// Returns true if we should use multi-endpoint mode
    pub fn is_multi_endpoint(&self) -> bool {
        self.rpc_endpoints
            .as_ref()
            .map(|e| e.contains(','))
            .unwrap_or(false)
    }
}

/// Supported RPC methods for load testing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RpcMethod {
    /// Send a raw transaction (write)
    SendRawTransaction,
    /// Get balance of an address (read)
    GetBalance,
    /// Call a contract (read)
    Call,
    /// Get current block number (read)
    BlockNumber,
    /// Get transaction count/nonce (read)
    GetTransactionCount,
    /// Get gas price (read)
    GasPrice,
    /// Get block by number (read)
    GetBlockByNumber,
    /// Get chain ID (read)
    ChainId,
}

impl RpcMethod {
    /// Returns true if this is a write method (transaction)
    pub fn is_write(&self) -> bool {
        matches!(self, Self::SendRawTransaction)
    }

    /// Returns the JSON-RPC method name
    pub fn method_name(&self) -> &'static str {
        match self {
            Self::SendRawTransaction => "eth_sendRawTransaction",
            Self::GetBalance => "eth_getBalance",
            Self::Call => "eth_call",
            Self::BlockNumber => "eth_blockNumber",
            Self::GetTransactionCount => "eth_getTransactionCount",
            Self::GasPrice => "eth_gasPrice",
            Self::GetBlockByNumber => "eth_getBlockByNumber",
            Self::ChainId => "eth_chainId",
        }
    }
}

impl std::str::FromStr for RpcMethod {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "").as_str() {
            "ethsendrawtransaction" | "sendrawtransaction" | "sendtx" => Ok(Self::SendRawTransaction),
            "ethgetbalance" | "getbalance" | "balance" => Ok(Self::GetBalance),
            "ethcall" | "call" => Ok(Self::Call),
            "ethblocknumber" | "blocknumber" | "blocknum" => Ok(Self::BlockNumber),
            "ethgettransactioncount" | "gettransactioncount" | "txcount" | "nonce" => Ok(Self::GetTransactionCount),
            "ethgasprice" | "gasprice" | "gas" => Ok(Self::GasPrice),
            "ethgetblockbynumber" | "getblockbynumber" | "getblock" => Ok(Self::GetBlockByNumber),
            "ethchainid" | "chainid" => Ok(Self::ChainId),
            _ => Err(format!("Unknown RPC method: {s}")),
        }
    }
}

impl std::fmt::Display for RpcMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.method_name())
    }
}

#[allow(clippy::option_if_let_else)]
fn parse_duration_string(s: &str) -> std::time::Duration {
    let s = s.trim();
    if let Some(secs) = s.strip_suffix('s') {
        std::time::Duration::from_secs(secs.parse().unwrap_or(60))
    } else if let Some(mins) = s.strip_suffix('m') {
        std::time::Duration::from_secs(mins.parse::<u64>().unwrap_or(1) * 60)
    } else if let Some(hours) = s.strip_suffix('h') {
        std::time::Duration::from_secs(hours.parse::<u64>().unwrap_or(1) * 3600)
    } else {
        std::time::Duration::from_secs(s.parse().unwrap_or(60))
    }
}
