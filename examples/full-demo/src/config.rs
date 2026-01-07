//! Runtime configuration generation for roxy.

use roxy_config::{
    BackendConfig, BackendGroupConfig, CacheConfig, LoadBalancerType, RateLimitConfig, RouteConfig,
    RoutingConfig, RoxyConfig, ServerConfig,
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
    /// Mock node configurations for the main demo group.
    pub nodes: Vec<MockNodeConfig>,
    /// Sequencer node configurations for the sequencer group (method routing demo).
    pub sequencer_nodes: Vec<MockNodeConfig>,
    /// Load balancer type to use.
    pub load_balancer: LoadBalancerType,
    /// Whether caching is enabled.
    pub cache_enabled: bool,
    /// Chain ID for mock nodes.
    pub chain_id: u64,
    /// Initial block number for mock nodes.
    pub initial_block: u64,
    /// Whether rate limiting is enabled.
    pub rate_limit_enabled: bool,
    /// Requests per second limit for rate limiting demo.
    pub requests_per_second: u64,
    /// Methods to block.
    pub blocked_methods: Vec<String>,
    /// Delay between demo sections in milliseconds.
    pub section_delay_ms: u64,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            roxy_port: ROXY_PORT,
            // Main demo group - 3 nodes with varying latency for load balancing demo
            nodes: vec![
                MockNodeConfig::new("node-1", 9001, 10),
                MockNodeConfig::new("node-2", 9002, 50),
                MockNodeConfig::new("node-3", 9003, 100),
            ],
            // Sequencer group - 2 fast nodes for method routing demo
            sequencer_nodes: vec![
                MockNodeConfig::new("sequencer-1", 9011, 5),
                MockNodeConfig::new("sequencer-2", 9012, 5),
            ],
            load_balancer: LoadBalancerType::RoundRobin,
            cache_enabled: true,
            chain_id: CHAIN_ID,
            initial_block: INITIAL_BLOCK,
            // Rate limiting: 50 requests per second
            // High enough for normal demos, but we can still trigger it with rapid requests
            rate_limit_enabled: true,
            requests_per_second: 50,
            // Block admin and debug methods
            blocked_methods: vec![
                "admin_addPeer".to_string(),
                "admin_removePeer".to_string(),
                "debug_traceTransaction".to_string(),
            ],
            // 1.5 second delay between sections
            section_delay_ms: 1500,
        }
    }
}

impl DemoConfig {
    /// Convert demo config to roxy config.
    pub(crate) fn to_roxy_config(&self) -> RoxyConfig {
        // Build backends list from both node groups
        let mut backends: Vec<BackendConfig> = self
            .nodes
            .iter()
            .map(|n| BackendConfig {
                name: n.name.clone(),
                url: format!("http://127.0.0.1:{}", n.port),
                weight: 1,
                max_retries: 2,
                timeout_ms: 5000,
            })
            .collect();

        // Add sequencer backends
        backends.extend(self.sequencer_nodes.iter().map(|n| BackendConfig {
            name: n.name.clone(),
            url: format!("http://127.0.0.1:{}", n.port),
            weight: 1,
            max_retries: 2,
            timeout_ms: 5000,
        }));

        // Create two groups: demo and sequencer
        let groups = vec![
            BackendGroupConfig {
                name: "demo".to_string(),
                backends: self.nodes.iter().map(|n| n.name.clone()).collect(),
                load_balancer: self.load_balancer,
            },
            BackendGroupConfig {
                name: "sequencer".to_string(),
                backends: self.sequencer_nodes.iter().map(|n| n.name.clone()).collect(),
                load_balancer: LoadBalancerType::RoundRobin,
            },
        ];

        // Route eth_sendRawTransaction to sequencer group
        let routes = vec![RouteConfig {
            method: "eth_sendRawTransaction".to_string(),
            target: "sequencer".to_string(),
        }];

        RoxyConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: self.roxy_port,
                ..Default::default()
            },
            backends,
            groups,
            cache: CacheConfig {
                enabled: self.cache_enabled,
                memory_size: 1000,
                default_ttl_ms: 5000,
                ..Default::default()
            },
            rate_limit: RateLimitConfig {
                enabled: self.rate_limit_enabled,
                requests_per_second: self.requests_per_second,
                burst_size: self.requests_per_second,
            },
            routing: RoutingConfig {
                default_group: "demo".to_string(),
                blocked_methods: self.blocked_methods.clone(),
                routes,
            },
            ..Default::default()
        }
    }
}
