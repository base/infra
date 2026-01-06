//! Integration tests for configuration
//!
//! These tests verify configuration loading, parsing, and validation.

use std::path::Path;

use roxy_config::{
    BackendConfig, BackendGroupConfig, CacheConfig, DEFAULT_BACKEND_TIMEOUT_MS, DEFAULT_BURST_SIZE,
    DEFAULT_CACHE_SIZE, DEFAULT_CACHE_TTL_MS, DEFAULT_HOST, DEFAULT_MAX_CONNECTIONS,
    DEFAULT_MAX_REQUEST_SIZE, DEFAULT_MAX_RETRIES, DEFAULT_METRICS_PORT, DEFAULT_PORT,
    DEFAULT_REQUEST_TIMEOUT_MS, DEFAULT_REQUESTS_PER_SECOND, DEFAULT_WEIGHT, LoadBalancerType,
    MetricsConfig, RateLimitConfig, RoutingConfig, RoxyConfig, ServerConfig,
};

// =============================================================================
// Configuration Loading Tests
// =============================================================================

/// Test loading a valid minimal configuration.
#[test]
fn test_load_valid_config() {
    let config = RoxyConfig::parse(
        r#"
        [server]
        port = 8545

        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"
    "#,
    )
    .unwrap();

    assert_eq!(config.server.port, 8545);
    assert_eq!(config.backends.len(), 1);
    assert_eq!(config.backends[0].name, "test");
    assert_eq!(config.groups.len(), 1);
    assert_eq!(config.groups[0].name, "main");
    assert_eq!(config.routing.default_group, "main");
}

/// Test loading a full configuration with all options.
#[test]
fn test_load_full_config() {
    let config = RoxyConfig::parse(
        r#"
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
    "#,
    )
    .unwrap();

    // Server
    assert_eq!(config.server.host, "127.0.0.1");
    assert_eq!(config.server.port, 8080);
    assert_eq!(config.server.max_connections, 5000);
    assert_eq!(config.server.request_timeout_ms, 60000);
    assert_eq!(config.server.max_request_size, 2097152);

    // Backends
    assert_eq!(config.backends.len(), 2);
    assert_eq!(config.backends[0].name, "alchemy");
    assert_eq!(config.backends[0].weight, 2);
    assert_eq!(config.backends[0].max_retries, 5);
    assert_eq!(config.backends[1].name, "infura");

    // Groups
    assert_eq!(config.groups.len(), 2);
    assert_eq!(config.groups[0].name, "primary");
    assert_eq!(config.groups[0].load_balancer, LoadBalancerType::RoundRobin);
    assert_eq!(config.groups[1].name, "archive");
    assert_eq!(config.groups[1].load_balancer, LoadBalancerType::Ema);

    // Cache
    assert!(config.cache.enabled);
    assert_eq!(config.cache.memory_size, 50000);
    assert_eq!(config.cache.default_ttl_ms, 10000);
    assert_eq!(config.cache.finalized_ttl_ms, Some(86400000));

    // Rate limit
    assert!(config.rate_limit.enabled);
    assert_eq!(config.rate_limit.requests_per_second, 500);
    assert_eq!(config.rate_limit.burst_size, 50);

    // Routing
    assert_eq!(config.routing.default_group, "primary");
    assert_eq!(config.routing.blocked_methods.len(), 2);
    assert_eq!(config.routing.routes.len(), 2);

    // Metrics
    assert!(config.metrics.enabled);
    assert_eq!(config.metrics.port, 9100);
}

// =============================================================================
// Default Value Tests
// =============================================================================

/// Test that defaults are applied correctly.
#[test]
fn test_defaults() {
    // Server defaults
    let server = ServerConfig::default();
    assert_eq!(server.host, DEFAULT_HOST);
    assert_eq!(server.port, DEFAULT_PORT);
    assert_eq!(server.max_connections, DEFAULT_MAX_CONNECTIONS);
    assert_eq!(server.request_timeout_ms, DEFAULT_REQUEST_TIMEOUT_MS);
    assert_eq!(server.max_request_size, DEFAULT_MAX_REQUEST_SIZE);

    // Cache defaults
    let cache = CacheConfig::default();
    assert!(cache.enabled);
    assert_eq!(cache.memory_size, DEFAULT_CACHE_SIZE);
    assert_eq!(cache.default_ttl_ms, DEFAULT_CACHE_TTL_MS);
    assert!(cache.finalized_ttl_ms.is_none());

    // Rate limit defaults
    let rate_limit = RateLimitConfig::default();
    assert!(!rate_limit.enabled);
    assert_eq!(rate_limit.requests_per_second, DEFAULT_REQUESTS_PER_SECOND);
    assert_eq!(rate_limit.burst_size, DEFAULT_BURST_SIZE);

    // Metrics defaults
    let metrics = MetricsConfig::default();
    assert!(!metrics.enabled);
    assert_eq!(metrics.host, DEFAULT_HOST);
    assert_eq!(metrics.port, DEFAULT_METRICS_PORT);

    // Routing defaults
    let routing = RoutingConfig::default();
    assert!(routing.routes.is_empty());
    assert!(routing.blocked_methods.is_empty());
    assert!(routing.default_group.is_empty());
}

/// Test backend defaults.
#[test]
fn test_backend_defaults() {
    let config = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"
    "#,
    )
    .unwrap();

    let backend = &config.backends[0];
    assert_eq!(backend.weight, DEFAULT_WEIGHT);
    assert_eq!(backend.max_retries, DEFAULT_MAX_RETRIES);
    assert_eq!(backend.timeout_ms, DEFAULT_BACKEND_TIMEOUT_MS);
}

/// Test group defaults.
#[test]
fn test_group_defaults() {
    let config = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"
    "#,
    )
    .unwrap();

    let group = &config.groups[0];
    assert_eq!(group.load_balancer, LoadBalancerType::Ema);
}

// =============================================================================
// Validation Error Tests
// =============================================================================

/// Test validation error for no backends.
#[test]
fn test_validation_no_backends() {
    let config = RoxyConfig::default();
    let result = config.validate();

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("at least one backend"));
}

/// Test validation error for unknown backend in group.
#[test]
fn test_validation_unknown_backend_in_group() {
    let result = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test", "unknown"]

        [routing]
        default_group = "main"
    "#,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown backend"));
}

/// Test validation error for unknown default group.
#[test]
fn test_validation_unknown_default_group() {
    let result = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "nonexistent"
    "#,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("default group"));
}

/// Test validation error for duplicate backend names.
#[test]
fn test_validation_duplicate_backend_names() {
    let result = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[backends]]
        name = "test"
        url = "http://localhost:8547"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"
    "#,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("duplicate backend name"));
}

/// Test validation error for duplicate group names.
#[test]
fn test_validation_duplicate_group_names() {
    let result = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"
    "#,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("duplicate group name"));
}

/// Test validation error for empty backend URL.
#[test]
fn test_validation_empty_backend_url() {
    let result = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = ""

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"
    "#,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("empty URL"));
}

/// Test validation error for invalid route target.
#[test]
fn test_validation_invalid_route_target() {
    let result = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"

        [[routing.routes]]
        method = "eth_call"
        target = "nonexistent"
    "#,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown group"));
}

/// Test that "block" is a valid route target.
#[test]
fn test_validation_block_route_target() {
    let config = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"

        [[routing.routes]]
        method = "debug_traceTransaction"
        target = "block"
    "#,
    )
    .unwrap();

    assert_eq!(config.routing.routes[0].target, "block");
}

/// Test validation with no groups but empty default.
#[test]
fn test_validation_no_groups_empty_default() {
    let config = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"
    "#,
    )
    .unwrap();

    // Should be valid - no groups and empty default is allowed
    assert!(config.groups.is_empty());
    assert!(config.routing.default_group.is_empty());
}

/// Test validation with no groups but non-empty default.
#[test]
fn test_validation_no_groups_nonempty_default() {
    let result = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [routing]
        default_group = "main"
    "#,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("no groups are configured"));
}

// =============================================================================
// Load Balancer Type Tests
// =============================================================================

/// Test load balancer type parsing.
#[test]
fn test_load_balancer_types() {
    let types = [
        ("ema", LoadBalancerType::Ema),
        ("round_robin", LoadBalancerType::RoundRobin),
        ("random", LoadBalancerType::Random),
        ("least_connections", LoadBalancerType::LeastConnections),
    ];

    for (input, expected) in types {
        let toml = format!(
            r#"
            [[backends]]
            name = "test"
            url = "http://localhost:8546"

            [[groups]]
            name = "main"
            backends = ["test"]
            load_balancer = "{}"

            [routing]
            default_group = "main"
        "#,
            input
        );

        let config = RoxyConfig::parse(&toml).unwrap();
        assert_eq!(config.groups[0].load_balancer, expected);
    }
}

/// Test load balancer type display.
#[test]
fn test_load_balancer_type_display() {
    assert_eq!(LoadBalancerType::Ema.to_string(), "ema");
    assert_eq!(LoadBalancerType::RoundRobin.to_string(), "round_robin");
    assert_eq!(LoadBalancerType::Random.to_string(), "random");
    assert_eq!(LoadBalancerType::LeastConnections.to_string(), "least_connections");
}

// =============================================================================
// Serialization Tests
// =============================================================================

/// Test round-trip serialization.
#[test]
fn test_round_trip_serialization() {
    let config = RoxyConfig {
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
    };

    let toml_str = config.to_toml().unwrap();
    let parsed = RoxyConfig::parse(&toml_str).unwrap();

    assert_eq!(config, parsed);
}

/// Test serialization produces valid TOML.
#[test]
fn test_serialization_format() {
    let config = RoxyConfig::parse(
        r#"
        [server]
        port = 8545

        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"
    "#,
    )
    .unwrap();

    let toml_str = config.to_toml().unwrap();

    // Verify it contains expected sections
    assert!(toml_str.contains("[server]"));
    assert!(toml_str.contains("[[backends]]"));
    assert!(toml_str.contains("[[groups]]"));
    assert!(toml_str.contains("[routing]"));
}

// =============================================================================
// File Loading Tests
// =============================================================================

/// Test loading from non-existent file.
#[test]
fn test_from_file_nonexistent() {
    let result = RoxyConfig::from_file(Path::new("/nonexistent/path/config.toml"));
    assert!(result.is_err());
}

// =============================================================================
// Parse Error Tests
// =============================================================================

/// Test parsing invalid TOML.
#[test]
fn test_parse_invalid_toml() {
    let invalid = "this is not valid toml [[[";
    let result = RoxyConfig::parse(invalid);
    assert!(result.is_err());
}

/// Test parsing TOML with wrong types.
#[test]
fn test_parse_wrong_types() {
    let result = RoxyConfig::parse(
        r#"
        [server]
        port = "not a number"
    "#,
    );
    assert!(result.is_err());
}

// =============================================================================
// Edge Case Tests
// =============================================================================

/// Test config with multiple backends in group.
#[test]
fn test_multiple_backends_in_group() {
    let config = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "backend1"
        url = "http://localhost:8546"

        [[backends]]
        name = "backend2"
        url = "http://localhost:8547"

        [[backends]]
        name = "backend3"
        url = "http://localhost:8548"

        [[groups]]
        name = "main"
        backends = ["backend1", "backend2", "backend3"]

        [routing]
        default_group = "main"
    "#,
    )
    .unwrap();

    assert_eq!(config.backends.len(), 3);
    assert_eq!(config.groups[0].backends.len(), 3);
}

/// Test config with multiple groups.
#[test]
fn test_multiple_groups() {
    let config = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "backend1"
        url = "http://localhost:8546"

        [[backends]]
        name = "backend2"
        url = "http://localhost:8547"

        [[groups]]
        name = "primary"
        backends = ["backend1"]

        [[groups]]
        name = "secondary"
        backends = ["backend2"]

        [[groups]]
        name = "all"
        backends = ["backend1", "backend2"]

        [routing]
        default_group = "primary"
    "#,
    )
    .unwrap();

    assert_eq!(config.groups.len(), 3);
}

/// Test config with multiple routes.
#[test]
fn test_multiple_routes() {
    let config = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"
        blocked_methods = ["admin_addPeer", "admin_removePeer"]

        [[routing.routes]]
        method = "eth_sendRawTransaction"
        target = "main"

        [[routing.routes]]
        method = "eth_call"
        target = "main"

        [[routing.routes]]
        method = "debug_traceTransaction"
        target = "block"
    "#,
    )
    .unwrap();

    assert_eq!(config.routing.routes.len(), 3);
    assert_eq!(config.routing.blocked_methods.len(), 2);
}

/// Test Debug implementations.
#[test]
fn test_debug_implementations() {
    let config = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"
    "#,
    )
    .unwrap();

    // Just verify Debug doesn't panic
    let _ = format!("{:?}", config);
    let _ = format!("{:?}", config.server);
    let _ = format!("{:?}", config.backends[0]);
    let _ = format!("{:?}", config.groups[0]);
    let _ = format!("{:?}", config.cache);
    let _ = format!("{:?}", config.rate_limit);
    let _ = format!("{:?}", config.routing);
    let _ = format!("{:?}", config.metrics);
}

/// Test Clone implementations.
#[test]
fn test_clone_implementations() {
    let config = RoxyConfig::parse(
        r#"
        [[backends]]
        name = "test"
        url = "http://localhost:8546"

        [[groups]]
        name = "main"
        backends = ["test"]

        [routing]
        default_group = "main"
    "#,
    )
    .unwrap();

    let cloned = config.clone();
    assert_eq!(config, cloned);
}
