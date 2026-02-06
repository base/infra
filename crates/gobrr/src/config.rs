use std::{fmt, time::Duration};

use alloy_primitives::{Address, Bytes, U256};
use anyhow::{Context, Result, bail};
use rand::{Rng, distributions::WeightedIndex, prelude::Distribution};
use serde::{Deserialize, Serialize};

/// Strongly-typed transaction category used across the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum TxType {
    EthSend,
    EthSendCalldata,
}

impl fmt::Display for TxType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EthSend => write!(f, "eth_send"),
            Self::EthSendCalldata => write!(f, "eth_send_calldata"),
        }
    }
}

/// Gas limit for simple ETH transfer (no calldata)
const GAS_LIMIT_SIMPLE: u64 = 21_000;

/// Gas per calldata byte. OP Stack L2s charge ~41 gas/byte due to L1 data costs.
/// We use 48 for safety margin.
const GAS_PER_CALLDATA_BYTE: u64 = 48;

/// Computes gas limit for calldata transactions: 21000 base + `GAS_PER_CALLDATA_BYTE` per byte
const fn compute_gas_limit(calldata_len: usize) -> u64 {
    21_000 + (calldata_len as u64) * GAS_PER_CALLDATA_BYTE
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum TxKind {
    #[serde(rename = "eth_send")]
    EthSend,
    #[serde(rename = "eth_send_calldata")]
    EthSendCalldata { max_size: usize },
}

pub(crate) struct TxParams {
    pub(crate) to: Address,
    pub(crate) value: U256,
    pub(crate) input: Bytes,
    pub(crate) gas_limit: u64,
    pub(crate) tx_type: TxType,
}

impl TxKind {
    pub(crate) fn build(&self, rng: &mut impl Rng) -> TxParams {
        match self {
            Self::EthSend => TxParams {
                to: Address::ZERO,
                value: U256::ZERO,
                input: Bytes::new(),
                gas_limit: GAS_LIMIT_SIMPLE,
                tx_type: TxType::EthSend,
            },
            Self::EthSendCalldata { max_size } => {
                let input = generate_calldata(rng, *max_size);
                let gas_limit = compute_gas_limit(input.len());
                TxParams {
                    to: Address::ZERO,
                    value: U256::ZERO,
                    input,
                    gas_limit,
                    tx_type: TxType::EthSendCalldata,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedTxKind {
    pub weight: u32,
    #[serde(flatten)]
    pub(crate) kind: TxKind,
}

#[derive(Clone)]
pub(crate) struct TxSelector {
    kinds: Vec<TxKind>,
    dist: WeightedIndex<u32>,
}

impl TxSelector {
    pub(crate) fn new(weighted: &[WeightedTxKind]) -> Result<Self> {
        if weighted.is_empty() {
            bail!("transactions list must not be empty");
        }
        let weights: Vec<u32> = weighted.iter().map(|w| w.weight).collect();
        if weights.contains(&0) {
            bail!("all transaction weights must be > 0");
        }
        let dist = WeightedIndex::new(&weights).context("invalid weights")?;
        let kinds = weighted.iter().map(|w| w.kind.clone()).collect();
        Ok(Self { kinds, dist })
    }

    pub(crate) fn select(&self, rng: &mut impl Rng) -> &TxKind {
        &self.kinds[self.dist.sample(rng)]
    }
}

const fn default_sender_count() -> u32 {
    10
}

const fn default_in_flight() -> u32 {
    16
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConfig {
    pub rpc: String,
    pub mnemonic: String,
    pub funder_key: String,
    pub funding_amount: String,
    #[serde(default = "default_sender_count")]
    pub sender_count: u32,
    #[serde(default)]
    pub sender_offset: u32,
    #[serde(default = "default_in_flight")]
    pub in_flight_per_sender: u32,
    pub duration: Option<String>,
    pub target_tps: Option<u32>,
    #[serde(default)]
    pub txpool_hosts: Vec<String>,
    /// WebSocket URL for flashblocks. FB inclusion times are tracked via this stream.
    pub flashblocks_ws: String,
    pub transactions: Vec<WeightedTxKind>,
}

impl TestConfig {
    pub fn load(path: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {path}"))?;
        serde_yaml::from_str(&contents).context("failed to parse config YAML")
    }

    pub fn parse_duration(&self) -> Result<Option<Duration>> {
        self.duration
            .as_ref()
            .map(|d| {
                humantime::parse_duration(d.trim())
                    .with_context(|| format!("invalid duration: {d}"))
            })
            .transpose()
    }

    pub fn parse_funding_amount(&self) -> Result<U256> {
        self.funding_amount.parse::<U256>().context("invalid funding_amount")
    }
}

/// Generates random calldata of the specified size.
/// Uses high-entropy random bytes that are uncompressible.
pub(crate) fn generate_calldata(rng: &mut impl Rng, size: usize) -> Bytes {
    let data: Vec<u8> = (0..size).map(|_| rng.r#gen()).collect();
    Bytes::from(data)
}
