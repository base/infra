#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/refcell/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::path::Path;

use eyre::{Context, bail, ensure};
use serde::{Deserialize, Serialize};

/// Default server host address.
pub const DEFAULT_HOST: &str = "0.0.0.0";
/// Default server port.
pub const DEFAULT_PORT: u16 = 8545;
/// Default maximum connections.
pub const DEFAULT_MAX_CONNECTIONS: usize = 10000;
/// Default request timeout in milliseconds.
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 30000;
/// Default maximum request size in bytes (1MB).
pub const DEFAULT_MAX_REQUEST_SIZE: usize = 1024 * 1024;
/// Default backend weight for load balancing.
pub const DEFAULT_WEIGHT: u32 = 1;
/// Default maximum retries for backend requests.
pub const DEFAULT_MAX_RETRIES: u32 = 3;
/// Default backend timeout in milliseconds.
pub const DEFAULT_BACKEND_TIMEOUT_MS: u64 = 10000;
/// Default cache memory size in entries.
pub const DEFAULT_CACHE_SIZE: usize = 10000;
/// Default cache TTL in milliseconds.
pub const DEFAULT_CACHE_TTL_MS: u64 = 5000;
/// Default metrics port.
pub const DEFAULT_METRICS_PORT: u16 = 9090;
/// Default requests per second for rate limiting.
pub const DEFAULT_REQUESTS_PER_SECOND: u64 = 1000;
/// Default burst size for rate limiting.
pub const DEFAULT_BURST_SIZE: u64 = 100;

/// Server configuration for the Roxy proxy.
///
/// Controls the HTTP server settings including binding address, connection limits,
/// and request handling parameters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ServerConfig {
    /// The host address to bind to.
    pub host: String,
    /// The port to listen on.
    pub port: u16,
    /// Maximum number of concurrent connections.
    pub max_connections: usize,
    /// Request timeout in milliseconds.
    pub request_timeout_ms: u64,
    /// Maximum request body size in bytes.
    pub max_request_size: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
            max_request_size: DEFAULT_MAX_REQUEST_SIZE,
        }
    }
}

/// Backend RPC endpoint configuration.
///
/// Defines a single upstream RPC provider that Roxy can route requests to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendConfig {
    /// Unique name for this backend.
    pub name: String,
    /// The URL of the RPC endpoint.
    pub url: String,
    /// Weight for load balancing (higher = more traffic).
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Maximum number of retry attempts.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Request timeout in milliseconds.
    #[serde(default = "default_backend_timeout")]
    pub timeout_ms: u64,
}

const fn default_weight() -> u32 {
    DEFAULT_WEIGHT
}

const fn default_max_retries() -> u32 {
    DEFAULT_MAX_RETRIES
}

const fn default_backend_timeout() -> u64 {
    DEFAULT_BACKEND_TIMEOUT_MS
}

/// Load balancer algorithm type.
///
/// Determines how requests are distributed across backends in a group.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalancerType {
    /// Exponential Moving Average based on response times.
    #[default]
    Ema,
    /// Round-robin distribution.
    RoundRobin,
    /// Random selection.
    Random,
    /// Least connections first.
    LeastConnections,
}

impl std::fmt::Display for LoadBalancerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ema => write!(f, "ema"),
            Self::RoundRobin => write!(f, "round_robin"),
            Self::Random => write!(f, "random"),
            Self::LeastConnections => write!(f, "least_connections"),
        }
    }
}

/// Backend group configuration.
///
/// Groups multiple backends together under a single name for routing purposes.
/// Requests to a group are distributed across its backends using the specified
/// load balancing algorithm.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendGroupConfig {
    /// Unique name for this group.
    pub name: String,
    /// Names of backends in this group.
    pub backends: Vec<String>,
    /// Load balancing algorithm to use.
    #[serde(default)]
    pub load_balancer: LoadBalancerType,
}

/// Cache configuration.
///
/// Controls the in-memory caching behavior for RPC responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CacheConfig {
    /// Whether caching is enabled.
    pub enabled: bool,
    /// Maximum number of entries in the cache.
    pub memory_size: usize,
    /// Default TTL for cache entries in milliseconds.
    pub default_ttl_ms: u64,
    /// TTL for finalized block data in milliseconds. None means cache forever.
    pub finalized_ttl_ms: Option<u64>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_size: DEFAULT_CACHE_SIZE,
            default_ttl_ms: DEFAULT_CACHE_TTL_MS,
            finalized_ttl_ms: None,
        }
    }
}

/// Rate limiting configuration.
///
/// Controls the rate at which requests are accepted to prevent overload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RateLimitConfig {
    /// Whether rate limiting is enabled.
    pub enabled: bool,
    /// Maximum requests per second.
    pub requests_per_second: u64,
    /// Burst capacity for handling traffic spikes.
    pub burst_size: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            requests_per_second: DEFAULT_REQUESTS_PER_SECOND,
            burst_size: DEFAULT_BURST_SIZE,
        }
    }
}

/// Route configuration for a specific method or method prefix.
///
/// Maps RPC methods to backend groups or blocks them entirely.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteConfig {
    /// Method name or prefix (e.g., "eth_call" or "eth_").
    pub method: String,
    /// Target group name or "block" to reject the method.
    pub target: String,
}

/// Routing configuration.
///
/// Defines how RPC methods are routed to backend groups.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RoutingConfig {
    /// Specific route configurations.
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
    /// Methods to block entirely.
    #[serde(default)]
    pub blocked_methods: Vec<String>,
    /// Default group for methods without specific routes.
    #[serde(default)]
    pub default_group: String,
}

/// Metrics configuration.
///
/// Controls the Prometheus metrics endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MetricsConfig {
    /// Whether metrics are enabled.
    pub enabled: bool,
    /// Host address for the metrics server.
    pub host: String,
    /// Port for the metrics server.
    pub port: u16,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self { enabled: false, host: DEFAULT_HOST.to_string(), port: DEFAULT_METRICS_PORT }
    }
}

/// Root configuration for the Roxy RPC proxy.
///
/// This is the top-level configuration structure that contains all settings
/// for the proxy, including server, backends, caching, rate limiting, and routing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
#[derive(Default)]
pub struct RoxyConfig {
    /// Server configuration.
    pub server: ServerConfig,
    /// Backend configurations.
    #[serde(default)]
    pub backends: Vec<BackendConfig>,
    /// Backend group configurations.
    #[serde(default)]
    pub groups: Vec<BackendGroupConfig>,
    /// Cache configuration.
    pub cache: CacheConfig,
    /// Rate limiting configuration.
    pub rate_limit: RateLimitConfig,
    /// Routing configuration.
    pub routing: RoutingConfig,
    /// Metrics configuration.
    pub metrics: MetricsConfig,
}

impl RoxyConfig {
    /// Load configuration from a TOML file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the TOML configuration file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed, or if validation fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::Path;
    /// use roxy_config::RoxyConfig;
    ///
    /// let config = RoxyConfig::from_file(Path::new("roxy.toml")).unwrap();
    /// ```
    pub fn from_file(path: &Path) -> eyre::Result<Self> {
        let content = std::fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read config file: {}", path.display()))?;
        Self::parse(&content)
    }

    /// Parse configuration from a TOML string.
    ///
    /// # Arguments
    ///
    /// * `s` - TOML configuration string.
    ///
    /// # Errors
    ///
    /// Returns an error if the string cannot be parsed or if validation fails.
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_config::RoxyConfig;
    ///
    /// let toml = r#"
    /// [[backends]]
    /// name = "primary"
    /// url = "https://eth.example.com"
    ///
    /// [[groups]]
    /// name = "main"
    /// backends = ["primary"]
    ///
    /// [routing]
    /// default_group = "main"
    /// "#;
    ///
    /// let config = RoxyConfig::parse(toml).unwrap();
    /// ```
    pub fn parse(s: &str) -> eyre::Result<Self> {
        let config: Self = toml::from_str(s).wrap_err("failed to parse TOML configuration")?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration.
    ///
    /// Checks that:
    /// - At least one backend is configured
    /// - All backend names in groups exist
    /// - The default group exists
    /// - Port numbers are valid (non-zero)
    /// - Backend names and group names are unique
    ///
    /// # Errors
    ///
    /// Returns an error describing any validation failures.
    pub fn validate(&self) -> eyre::Result<()> {
        // Check at least one backend is configured
        ensure!(!self.backends.is_empty(), "at least one backend must be configured");

        // Check for duplicate backend names
        let mut backend_names = std::collections::HashSet::new();
        for backend in &self.backends {
            ensure!(
                backend_names.insert(&backend.name),
                "duplicate backend name: {}",
                backend.name
            );
        }

        // Check for duplicate group names
        let mut group_names = std::collections::HashSet::new();
        for group in &self.groups {
            ensure!(group_names.insert(&group.name), "duplicate group name: {}", group.name);
        }

        // Check all backend names in groups exist
        for group in &self.groups {
            for backend_name in &group.backends {
                ensure!(
                    backend_names.contains(backend_name),
                    "group '{}' references unknown backend: {}",
                    group.name,
                    backend_name
                );
            }
        }

        // Check default group exists (if groups are configured)
        if !self.groups.is_empty() {
            ensure!(
                group_names.contains(&self.routing.default_group),
                "default group '{}' does not exist",
                self.routing.default_group
            );
        } else if !self.routing.default_group.is_empty() {
            bail!(
                "default group '{}' specified but no groups are configured",
                self.routing.default_group
            );
        }

        // Check route targets exist
        for route in &self.routing.routes {
            if route.target != "block" && !group_names.contains(&route.target) {
                bail!(
                    "route for method '{}' references unknown group: {}",
                    route.method,
                    route.target
                );
            }
        }

        // Validate port numbers
        ensure!(self.server.port > 0, "server port must be greater than 0");

        if self.metrics.enabled {
            ensure!(self.metrics.port > 0, "metrics port must be greater than 0");
        }

        // Validate backend URLs are non-empty
        for backend in &self.backends {
            ensure!(!backend.url.is_empty(), "backend '{}' has empty URL", backend.name);
        }

        Ok(())
    }

    /// Serialize the configuration to a TOML string.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_toml(&self) -> eyre::Result<String> {
        toml::to_string_pretty(self).wrap_err("failed to serialize configuration to TOML")
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Creates a minimal valid configuration for testing.
    fn minimal_config() -> RoxyConfig {
        RoxyConfig {
            backends: vec![BackendConfig {
                name: "primary".to_string(),
                url: "https://eth.example.com".to_string(),
                weight: DEFAULT_WEIGHT,
                max_retries: DEFAULT_MAX_RETRIES,
                timeout_ms: DEFAULT_BACKEND_TIMEOUT_MS,
            }],
            groups: vec![BackendGroupConfig {
                name: "main".to_string(),
                backends: vec!["primary".to_string()],
                load_balancer: LoadBalancerType::Ema,
            }],
            routing: RoutingConfig { default_group: "main".to_string(), ..Default::default() },
            ..Default::default()
        }
    }

    #[rstest]
    fn test_parse_minimal_config() {
        let toml = r#"
[[backends]]
name = "primary"
url = "https://eth.example.com"

[[groups]]
name = "main"
backends = ["primary"]

[routing]
default_group = "main"
"#;

        let config = RoxyConfig::parse(toml).unwrap();
        assert_eq!(config.backends.len(), 1);
        assert_eq!(config.backends[0].name, "primary");
        assert_eq!(config.backends[0].weight, DEFAULT_WEIGHT);
        assert_eq!(config.server.port, DEFAULT_PORT);
    }

    #[rstest]
    fn test_parse_full_config() {
        let toml = r#"
[server]
host = "127.0.0.1"
port = 8080
max_connections = 5000
request_timeout_ms = 60000
max_request_size = 2097152

[[backends]]
name = "alchemy"
url = "https://eth-mainnet.g.alchemy.com/v2/key"
weight = 2
max_retries = 5
timeout_ms = 15000

[[backends]]
name = "infura"
url = "https://mainnet.infura.io/v3/key"
weight = 1
max_retries = 3
timeout_ms = 10000

[[groups]]
name = "primary"
backends = ["alchemy", "infura"]
load_balancer = "round_robin"

[[groups]]
name = "archive"
backends = ["alchemy"]
load_balancer = "ema"

[cache]
enabled = true
memory_size = 50000
default_ttl_ms = 10000
finalized_ttl_ms = 86400000

[rate_limit]
enabled = true
requests_per_second = 500
burst_size = 50

[routing]
default_group = "primary"
blocked_methods = ["debug_traceTransaction", "trace_block"]

[[routing.routes]]
method = "eth_call"
target = "primary"

[[routing.routes]]
method = "eth_getStorageAt"
target = "archive"

[metrics]
enabled = true
host = "0.0.0.0"
port = 9100
"#;

        let config = RoxyConfig::parse(toml).unwrap();

        // Server
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.server.max_connections, 5000);

        // Backends
        assert_eq!(config.backends.len(), 2);
        assert_eq!(config.backends[0].name, "alchemy");
        assert_eq!(config.backends[0].weight, 2);
        assert_eq!(config.backends[1].name, "infura");

        // Groups
        assert_eq!(config.groups.len(), 2);
        assert_eq!(config.groups[0].load_balancer, LoadBalancerType::RoundRobin);

        // Cache
        assert!(config.cache.enabled);
        assert_eq!(config.cache.memory_size, 50000);
        assert_eq!(config.cache.finalized_ttl_ms, Some(86400000));

        // Rate limit
        assert!(config.rate_limit.enabled);
        assert_eq!(config.rate_limit.requests_per_second, 500);

        // Routing
        assert_eq!(config.routing.default_group, "primary");
        assert_eq!(config.routing.blocked_methods.len(), 2);
        assert_eq!(config.routing.routes.len(), 2);

        // Metrics
        assert!(config.metrics.enabled);
        assert_eq!(config.metrics.port, 9100);
    }

    #[rstest]
    fn test_defaults() {
        let server = ServerConfig::default();
        assert_eq!(server.host, DEFAULT_HOST);
        assert_eq!(server.port, DEFAULT_PORT);
        assert_eq!(server.max_connections, DEFAULT_MAX_CONNECTIONS);

        let cache = CacheConfig::default();
        assert!(cache.enabled);
        assert_eq!(cache.memory_size, DEFAULT_CACHE_SIZE);
        assert!(cache.finalized_ttl_ms.is_none());

        let rate_limit = RateLimitConfig::default();
        assert!(!rate_limit.enabled);

        let metrics = MetricsConfig::default();
        assert!(!metrics.enabled);
    }

    #[rstest]
    fn test_validation_no_backends() {
        let config = RoxyConfig::default();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one backend"));
    }

    #[rstest]
    fn test_validation_unknown_backend_in_group() {
        let config = RoxyConfig {
            backends: vec![BackendConfig {
                name: "primary".to_string(),
                url: "https://eth.example.com".to_string(),
                weight: 1,
                max_retries: 3,
                timeout_ms: 10000,
            }],
            groups: vec![BackendGroupConfig {
                name: "main".to_string(),
                backends: vec!["primary".to_string(), "unknown".to_string()],
                load_balancer: LoadBalancerType::Ema,
            }],
            routing: RoutingConfig { default_group: "main".to_string(), ..Default::default() },
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown backend"));
    }

    #[rstest]
    fn test_validation_unknown_default_group() {
        let config = RoxyConfig {
            backends: vec![BackendConfig {
                name: "primary".to_string(),
                url: "https://eth.example.com".to_string(),
                weight: 1,
                max_retries: 3,
                timeout_ms: 10000,
            }],
            groups: vec![BackendGroupConfig {
                name: "main".to_string(),
                backends: vec!["primary".to_string()],
                load_balancer: LoadBalancerType::Ema,
            }],
            routing: RoutingConfig {
                default_group: "nonexistent".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("default group"));
    }

    #[rstest]
    fn test_validation_duplicate_backend_names() {
        let config = RoxyConfig {
            backends: vec![
                BackendConfig {
                    name: "primary".to_string(),
                    url: "https://eth1.example.com".to_string(),
                    weight: 1,
                    max_retries: 3,
                    timeout_ms: 10000,
                },
                BackendConfig {
                    name: "primary".to_string(),
                    url: "https://eth2.example.com".to_string(),
                    weight: 1,
                    max_retries: 3,
                    timeout_ms: 10000,
                },
            ],
            groups: vec![BackendGroupConfig {
                name: "main".to_string(),
                backends: vec!["primary".to_string()],
                load_balancer: LoadBalancerType::Ema,
            }],
            routing: RoutingConfig { default_group: "main".to_string(), ..Default::default() },
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("duplicate backend name"));
    }

    #[rstest]
    fn test_validation_empty_backend_url() {
        let config = RoxyConfig {
            backends: vec![BackendConfig {
                name: "primary".to_string(),
                url: "".to_string(),
                weight: 1,
                max_retries: 3,
                timeout_ms: 10000,
            }],
            groups: vec![BackendGroupConfig {
                name: "main".to_string(),
                backends: vec!["primary".to_string()],
                load_balancer: LoadBalancerType::Ema,
            }],
            routing: RoutingConfig { default_group: "main".to_string(), ..Default::default() },
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty URL"));
    }

    #[rstest]
    fn test_validation_invalid_route_target() {
        let config = RoxyConfig {
            backends: vec![BackendConfig {
                name: "primary".to_string(),
                url: "https://eth.example.com".to_string(),
                weight: 1,
                max_retries: 3,
                timeout_ms: 10000,
            }],
            groups: vec![BackendGroupConfig {
                name: "main".to_string(),
                backends: vec!["primary".to_string()],
                load_balancer: LoadBalancerType::Ema,
            }],
            routing: RoutingConfig {
                default_group: "main".to_string(),
                routes: vec![RouteConfig {
                    method: "eth_call".to_string(),
                    target: "nonexistent".to_string(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown group"));
    }

    #[rstest]
    fn test_validation_block_route_target_allowed() {
        let config = RoxyConfig {
            backends: vec![BackendConfig {
                name: "primary".to_string(),
                url: "https://eth.example.com".to_string(),
                weight: 1,
                max_retries: 3,
                timeout_ms: 10000,
            }],
            groups: vec![BackendGroupConfig {
                name: "main".to_string(),
                backends: vec!["primary".to_string()],
                load_balancer: LoadBalancerType::Ema,
            }],
            routing: RoutingConfig {
                default_group: "main".to_string(),
                routes: vec![RouteConfig {
                    method: "debug_traceTransaction".to_string(),
                    target: "block".to_string(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(config.validate().is_ok());
    }

    #[rstest]
    fn test_round_trip_serialization() {
        let config = minimal_config();

        let toml_str = config.to_toml().unwrap();
        let parsed: RoxyConfig = RoxyConfig::parse(&toml_str).unwrap();

        assert_eq!(config, parsed);
    }

    #[rstest]
    fn test_load_balancer_type_display() {
        assert_eq!(LoadBalancerType::Ema.to_string(), "ema");
        assert_eq!(LoadBalancerType::RoundRobin.to_string(), "round_robin");
        assert_eq!(LoadBalancerType::Random.to_string(), "random");
        assert_eq!(LoadBalancerType::LeastConnections.to_string(), "least_connections");
    }

    #[rstest]
    #[case("ema", LoadBalancerType::Ema)]
    #[case("round_robin", LoadBalancerType::RoundRobin)]
    #[case("random", LoadBalancerType::Random)]
    #[case("least_connections", LoadBalancerType::LeastConnections)]
    fn test_load_balancer_type_parsing(#[case] input: &str, #[case] expected: LoadBalancerType) {
        let toml = format!(
            r#"
[[backends]]
name = "primary"
url = "https://eth.example.com"

[[groups]]
name = "main"
backends = ["primary"]
load_balancer = "{}"

[routing]
default_group = "main"
"#,
            input
        );

        let config = RoxyConfig::parse(&toml).unwrap();
        assert_eq!(config.groups[0].load_balancer, expected);
    }

    #[rstest]
    fn test_parse_invalid_toml() {
        let invalid = "this is not valid toml [[[";
        let result = RoxyConfig::parse(invalid);
        assert!(result.is_err());
    }

    #[rstest]
    fn test_from_file_nonexistent() {
        let result = RoxyConfig::from_file(Path::new("/nonexistent/path/config.toml"));
        assert!(result.is_err());
    }

    #[rstest]
    fn test_valid_minimal_config() {
        let config = minimal_config();
        assert!(config.validate().is_ok());
    }

    #[rstest]
    fn test_no_groups_with_empty_default() {
        // When no groups are configured and default_group is empty, it should pass
        let config = RoxyConfig {
            backends: vec![BackendConfig {
                name: "primary".to_string(),
                url: "https://eth.example.com".to_string(),
                weight: 1,
                max_retries: 3,
                timeout_ms: 10000,
            }],
            groups: vec![],
            routing: RoutingConfig { default_group: String::new(), ..Default::default() },
            ..Default::default()
        };

        assert!(config.validate().is_ok());
    }

    #[rstest]
    fn test_no_groups_with_nonempty_default() {
        // When no groups are configured but default_group is specified, it should fail
        let config = RoxyConfig {
            backends: vec![BackendConfig {
                name: "primary".to_string(),
                url: "https://eth.example.com".to_string(),
                weight: 1,
                max_retries: 3,
                timeout_ms: 10000,
            }],
            groups: vec![],
            routing: RoutingConfig { default_group: "main".to_string(), ..Default::default() },
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no groups are configured"));
    }
}
