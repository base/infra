use std::path::PathBuf;

use alloy_primitives::Address;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    pub name: String,
    pub rpc: Url,
    pub flashblocks_ws: Url,
    pub l1_rpc: Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_node_rpc: Option<Url>,
    #[serde(with = "address_serde")]
    pub system_config: Address,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "option_address_serde")]
    pub batcher_address: Option<Address>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ChainConfigOverride {
    name: Option<String>,
    rpc: Option<Url>,
    flashblocks_ws: Option<Url>,
    l1_rpc: Option<Url>,
    op_node_rpc: Option<Url>,
    #[serde(default, with = "option_address_serde")]
    system_config: Option<Address>,
    #[serde(default, with = "option_address_serde")]
    batcher_address: Option<Address>,
}

mod address_serde {
    use std::str::FromStr;

    use alloy_primitives::Address;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(address: &Address, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{address:#x}"))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Address, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Address::from_str(&s).map_err(serde::de::Error::custom)
    }
}

mod option_address_serde {
    use std::str::FromStr;

    use alloy_primitives::Address;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(address: &Option<Address>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match address {
            Some(addr) => serializer.serialize_str(&format!("{addr:#x}")),
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<Address>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        opt.map_or_else(
            || Ok(None),
            |s| Address::from_str(&s).map(Some).map_err(serde::de::Error::custom),
        )
    }
}

impl ChainConfig {
    pub fn mainnet() -> Self {
        Self {
            name: "mainnet".to_string(),
            rpc: Url::parse("https://mainnet.base.org").unwrap(),
            flashblocks_ws: Url::parse("wss://mainnet.flashblocks.base.org/ws").unwrap(),
            l1_rpc: Url::parse("https://ethereum-rpc.publicnode.com").unwrap(),
            op_node_rpc: None,
            system_config: "0x73a79Fab69143498Ed3712e519A88a918e1f4072".parse().unwrap(),
            batcher_address: Some("0x5050F69a9786F081509234F1a7F4684b5E5b76C9".parse().unwrap()),
        }
    }

    pub fn sepolia() -> Self {
        Self {
            name: "sepolia".to_string(),
            rpc: Url::parse("https://sepolia.base.org").unwrap(),
            flashblocks_ws: Url::parse("wss://sepolia.flashblocks.base.org/ws").unwrap(),
            l1_rpc: Url::parse("https://ethereum-sepolia-rpc.publicnode.com").unwrap(),
            op_node_rpc: None,
            system_config: "0xf272670eb55e895584501d564AfEB048bEd26194".parse().unwrap(),
            batcher_address: Some("0x6CDEbe940BC0F26850285cacA097C11c33103E47".parse().unwrap()),
        }
    }

    /// Returns a devnet configuration for local development.
    ///
    /// This attempts to read dynamic addresses from the devnet rollup.json file
    /// at `../base/base/.devnet/l2/configs/rollup.json` (relative to the repo root).
    /// If the file is not found, placeholder addresses are used.
    ///
    /// Note: Devnet addresses are regenerated each time the devnet is started,
    /// so this config may need to be refreshed after devnet restarts.
    pub fn devnet() -> Self {
        // Try to read system_config and batcher from rollup.json if it exists
        let (system_config, batcher_address) = Self::load_devnet_addresses().unwrap_or_else(|| {
            // Placeholder addresses - will be updated when devnet is running
            ("0x0000000000000000000000000000000000000000".parse().unwrap(), None)
        });

        Self {
            name: "devnet".to_string(),
            rpc: Url::parse("http://localhost:7545").unwrap(),
            flashblocks_ws: Url::parse("ws://localhost:7111").unwrap(),
            l1_rpc: Url::parse("http://localhost:4545").unwrap(),
            op_node_rpc: Some(Url::parse("http://localhost:7549").unwrap()),
            system_config,
            batcher_address,
        }
    }

    /// Attempts to load devnet addresses from the rollup.json file.
    fn load_devnet_addresses() -> Option<(Address, Option<Address>)> {
        // Try common locations for the devnet rollup.json
        let possible_paths = [
            // Relative to infra repo when running from project root
            PathBuf::from("../base/base/.devnet/l2/configs/rollup.json"),
            // Absolute path from home directory
            dirs::home_dir()?.join("base/base/.devnet/l2/configs/rollup.json"),
        ];

        for path in &possible_paths {
            if let Ok(contents) = std::fs::read_to_string(path)
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents)
            {
                let system_config = json
                    .get("l1_system_config_address")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Address>().ok())?;

                let batcher_address = json
                    .get("genesis")
                    .and_then(|g| g.get("system_config"))
                    .and_then(|sc| sc.get("batcherAddr"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Address>().ok());

                return Some((system_config, batcher_address));
            }
        }

        None
    }

    /// Load config by name or path
    ///
    /// Resolution order:
    /// 1. Built-in config as base (if name matches "mainnet", "sepolia", or "devnet")
    /// 2. User config at ~/.base/config/<name>.yaml merged on top
    /// 3. Or treat as standalone file path
    pub fn load(name_or_path: &str) -> Result<Self> {
        let base_config = match name_or_path {
            "mainnet" => Some(Self::mainnet()),
            "sepolia" => Some(Self::sepolia()),
            "devnet" => Some(Self::devnet()),
            _ => None,
        };

        if let Some(config_dir) = Self::config_dir() {
            let user_config_path = config_dir.join(format!("{name_or_path}.yaml"));
            if user_config_path.exists() {
                return base_config.map_or_else(
                    || Self::load_from_file(&user_config_path),
                    |base| Self::load_and_merge(&user_config_path, base),
                );
            }
        }

        if let Some(config) = base_config {
            return Ok(config);
        }

        let path = PathBuf::from(name_or_path);
        if path.exists() {
            return Self::load_from_file(&path);
        }

        anyhow::bail!(
            "Config '{name_or_path}' not found. Expected built-in name (mainnet, sepolia, devnet), \
             user config at ~/.base/config/{name_or_path}.yaml, or a valid file path."
        )
    }

    fn load_from_file(path: &PathBuf) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: Self = serde_yaml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(config)
    }

    fn load_and_merge(path: &PathBuf, base: Self) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let overrides: ChainConfigOverride = serde_yaml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(Self {
            name: overrides.name.unwrap_or(base.name),
            rpc: overrides.rpc.unwrap_or(base.rpc),
            flashblocks_ws: overrides.flashblocks_ws.unwrap_or(base.flashblocks_ws),
            l1_rpc: overrides.l1_rpc.unwrap_or(base.l1_rpc),
            op_node_rpc: overrides.op_node_rpc.or(base.op_node_rpc),
            system_config: overrides.system_config.unwrap_or(base.system_config),
            batcher_address: overrides.batcher_address.or(base.batcher_address),
        })
    }

    fn config_dir() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".base").join("config"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_configs() {
        let mainnet = ChainConfig::load("mainnet").unwrap();
        assert_eq!(mainnet.name, "mainnet");
        assert!(mainnet.rpc.as_str().contains("mainnet"));

        let sepolia = ChainConfig::load("sepolia").unwrap();
        assert_eq!(sepolia.name, "sepolia");
        assert!(sepolia.rpc.as_str().contains("sepolia"));

        let devnet = ChainConfig::load("devnet").unwrap();
        assert_eq!(devnet.name, "devnet");
        assert!(devnet.rpc.as_str().contains("localhost"));
        assert_eq!(devnet.rpc.as_str(), "http://localhost:7545/");
        assert_eq!(devnet.flashblocks_ws.as_str(), "ws://localhost:7111/");
        assert_eq!(devnet.l1_rpc.as_str(), "http://localhost:4545/");
        assert!(devnet.op_node_rpc.is_some());
        assert_eq!(devnet.op_node_rpc.unwrap().as_str(), "http://localhost:7549/");
    }

    #[test]
    fn test_unknown_config() {
        let result = ChainConfig::load("nonexistent");
        assert!(result.is_err());
    }
}
