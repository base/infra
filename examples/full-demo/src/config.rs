//! Runtime configuration generation for roxy.

use roxy_config::{
    BackendConfig, BackendGroupConfig, CacheConfig, LoadBalancerType, RoutingConfig, RoxyConfig,
    ServerConfig,
};

use crate::mock_node::MockNodeConfig;

/// Default roxy port (using 18545 to avoid conflicts with local nodes).
pub(crate) const ROXY_PORT: u16 = 18545;

/// Default chain ID (Ethereum mainnet).
pub(crate) const CHAIN_ID: u64 = 1;

/// Default initial block number.
pub(crate) const INITIAL_BLOCK: u64 = 21_000_000;

/// Demo configuration settings.
#[derive(Debug, Clone)]
pub(crate) struct DemoConfig {
    /// Port for roxy proxy.
    pub roxy_port: u16,
    /// Mock node configurations.
    pub nodes: Vec<MockNodeConfig>,
    /// Load balancer type to use.
    pub load_balancer: LoadBalancerType,
    /// Whether caching is enabled.
    pub cache_enabled: bool,
    /// Chain ID for mock nodes.
    pub chain_id: u64,
    /// Initial block number for mock nodes.
    pub initial_block: u64,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            roxy_port: ROXY_PORT,
            nodes: vec![
                MockNodeConfig::new("node-1", 9001, 10),
                MockNodeConfig::new("node-2", 9002, 50),
                MockNodeConfig::new("node-3", 9003, 100),
            ],
            load_balancer: LoadBalancerType::RoundRobin,
            cache_enabled: true,
            chain_id: CHAIN_ID,
            initial_block: INITIAL_BLOCK,
        }
    }
}

impl DemoConfig {
    /// Convert demo config to roxy config.
    pub(crate) fn to_roxy_config(&self) -> RoxyConfig {
        RoxyConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: self.roxy_port,
                ..Default::default()
            },
            backends: self
                .nodes
                .iter()
                .map(|n| BackendConfig {
                    name: n.name.clone(),
                    url: format!("http://127.0.0.1:{}", n.port),
                    weight: 1,
                    max_retries: 2,
                    timeout_ms: 5000,
                })
                .collect(),
            groups: vec![BackendGroupConfig {
                name: "demo".to_string(),
                backends: self.nodes.iter().map(|n| n.name.clone()).collect(),
                load_balancer: self.load_balancer,
            }],
            cache: CacheConfig {
                enabled: self.cache_enabled,
                memory_size: 1000,
                default_ttl_ms: 5000,
                ..Default::default()
            },
            routing: RoutingConfig {
                default_group: "demo".to_string(),
                blocked_methods: vec!["admin_addPeer".to_string()],
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
