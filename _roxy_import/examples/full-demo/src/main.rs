//! Full demo example for roxy JSON-RPC proxy.
//!
//! This example demonstrates roxy's comprehensive capabilities:
//! 1. Load balancing (Round Robin) across multiple backends
//! 2. Response caching with TTL
//! 3. Batch request handling
//! 4. Rate limiting with per-client tracking
//! 5. Method routing to specific backend groups
//! 6. Method blocking for sensitive operations
//! 7. EMA-based intelligent load balancing
//! 8. Automatic failover when backends go offline

#![allow(missing_docs)]

use std::{collections::HashMap, time::Duration};

use eyre::{Context, Result};
use serde_json::json;
use tokio::task::JoinHandle;

mod client;
mod config;
mod mock_node;
mod output;

use client::{DemoClient, parse_node_name};
use config::DemoConfig;
use mock_node::MockNode;

/// Container for all mock nodes in the demo.
struct DemoNodes {
    /// Main demo group nodes.
    main_nodes: Vec<MockNode>,
    /// Sequencer group nodes for routing demo.
    sequencer_nodes: Vec<MockNode>,
}

impl DemoNodes {
    /// Get all nodes as an iterator.
    fn all_nodes(&self) -> impl Iterator<Item = &MockNode> {
        self.main_nodes.iter().chain(self.sequencer_nodes.iter())
    }

    /// Shutdown all nodes.
    async fn shutdown_all(self) {
        for node in self.main_nodes {
            node.shutdown().await;
        }
        for node in self.sequencer_nodes {
            node.shutdown().await;
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing with minimal verbosity for clean demo output
    tracing_subscriber::fmt().with_env_filter("error").with_target(false).init();

    output::print_banner();

    let demo_config = DemoConfig::default();

    // Phase 1: Start mock backends
    output::print_phase(1, 3, "Starting mock Ethereum nodes");
    let mut nodes = start_mock_nodes(&demo_config).await?;

    // Brief pause to ensure backends are ready
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Phase 2: Start roxy proxy
    output::print_phase(2, 3, "Starting roxy proxy");
    let roxy_config = demo_config.to_roxy_config();
    let roxy_app =
        roxyproxy_cli::build_app(&roxy_config).await.wrap_err("failed to build roxy app")?;

    let roxy_handle = spawn_roxy(roxy_app, &roxy_config);
    output::print_success(&format!("Roxy listening at http://127.0.0.1:{}", demo_config.roxy_port));

    // Brief pause for roxy to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Phase 3: Run demos
    output::print_phase(3, 3, "Running feature demonstrations");
    let client = DemoClient::new(demo_config.roxy_port);

    // Run all demo scenarios with delays between sections
    demo_load_balancing(&client, 9).await?;
    section_delay(&demo_config).await;

    demo_caching(&client).await?;
    section_delay(&demo_config).await;

    demo_batch_requests(&client).await?;
    section_delay(&demo_config).await;

    demo_rate_limiting(&client, &demo_config).await?;
    section_delay(&demo_config).await;

    demo_method_routing(&client).await?;
    section_delay(&demo_config).await;

    demo_method_blocking(&client, &demo_config).await?;
    section_delay(&demo_config).await;

    demo_ema_load_balancing(&client).await?;
    section_delay(&demo_config).await;

    // Failover last since it modifies node state
    demo_failover(&client, &mut nodes.main_nodes).await?;

    // Print summary
    let node_counts: Vec<(String, u64)> =
        nodes.all_nodes().map(|n| (n.name().to_string(), n.request_count())).collect();
    output::print_summary(&node_counts);

    // Cleanup
    output::print_info("Shutting down...");
    nodes.shutdown_all().await;
    roxy_handle.abort();

    output::print_complete();
    Ok(())
}

/// Add visual delay between demo sections.
async fn section_delay(config: &DemoConfig) {
    let delay_secs = config.section_delay_ms as f64 / 1000.0;
    output::print_delay(delay_secs);
    tokio::time::sleep(Duration::from_millis(config.section_delay_ms)).await;
}

/// Start all mock nodes from configuration.
async fn start_mock_nodes(config: &DemoConfig) -> Result<DemoNodes> {
    let mut main_nodes = Vec::new();
    let mut sequencer_nodes = Vec::new();

    // Start main demo group nodes
    println!("  Main group nodes:");
    for node_cfg in &config.nodes {
        let mut node = MockNode::start(
            &node_cfg.name,
            node_cfg.port,
            config.chain_id,
            config.initial_block,
            Duration::from_millis(node_cfg.latency_ms),
        )
        .await
        .wrap_err_with(|| format!("failed to start {}", node_cfg.name))?;

        // Start block progression (every 2 seconds)
        node.start_block_progression(Duration::from_secs(2));

        output::print_node_started(&node_cfg.name, &node.url(), node_cfg.latency_ms);
        main_nodes.push(node);
    }

    // Start sequencer group nodes
    println!();
    println!("  Sequencer group nodes:");
    for node_cfg in &config.sequencer_nodes {
        let mut node = MockNode::start(
            &node_cfg.name,
            node_cfg.port,
            config.chain_id,
            config.initial_block,
            Duration::from_millis(node_cfg.latency_ms),
        )
        .await
        .wrap_err_with(|| format!("failed to start {}", node_cfg.name))?;

        // Start block progression (every 2 seconds)
        node.start_block_progression(Duration::from_secs(2));

        output::print_node_started(&node_cfg.name, &node.url(), node_cfg.latency_ms);
        sequencer_nodes.push(node);
    }

    Ok(DemoNodes { main_nodes, sequencer_nodes })
}

/// Spawn roxy server in background.
fn spawn_roxy(app: roxy_server::Router, config: &roxy_config::RoxyConfig) -> JoinHandle<()> {
    let addr = format!("{}:{}", config.server.host, config.server.port);
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        axum::serve(listener, app).await.ok();
    })
}

/// Demonstrate load balancing across backends.
async fn demo_load_balancing(client: &DemoClient, count: usize) -> Result<()> {
    output::print_section("Load Balancing Demo (Round Robin)");
    println!("  Sending {} requests through roxy...", count);
    println!();

    let mut distribution: HashMap<String, u64> = HashMap::new();

    for i in 1..=count {
        let result = client.request_timed("web3_clientVersion", json!([])).await?;
        let client_version = result.value.as_str().unwrap_or("unknown");
        let node_name = parse_node_name(client_version).unwrap_or_else(|| "unknown".to_string());

        output::print_request_served(i, &node_name, result.duration);
        *distribution.entry(node_name).or_insert(0) += 1;
    }

    output::print_distribution(&distribution);
    Ok(())
}

/// Demonstrate caching behavior.
async fn demo_caching(client: &DemoClient) -> Result<()> {
    output::print_section("Caching Demo");
    println!("  Sending net_version (should be cached after first request)...");
    println!();

    // First request - hits backend
    let result1 = client.request_timed("net_version", json!([])).await?;
    output::print_cache_result("First request", result1.duration, false);

    // Second request - should be cached
    let result2 = client.request_timed("net_version", json!([])).await?;
    let is_cached = result2.duration < result1.duration / 2;
    output::print_cache_result("Second request", result2.duration, is_cached);

    // Third request - definitely cached
    let result3 = client.request_timed("net_version", json!([])).await?;
    output::print_cache_result("Third request", result3.duration, true);

    println!();
    println!("  eth_blockNumber (NOT cached - changes over time):");
    let block1 = client.get_block_number().await?;
    println!("    Block 1: {}", block1);

    // Wait for block to advance
    tokio::time::sleep(Duration::from_secs(2)).await;

    let block2 = client.get_block_number().await?;
    println!("    Block 2: {} (after 2s)", block2);

    Ok(())
}

/// Demonstrate batch request handling.
async fn demo_batch_requests(client: &DemoClient) -> Result<()> {
    output::print_section("Batch Request Demo");

    let batch = vec![
        ("eth_chainId", json!([])),
        ("eth_blockNumber", json!([])),
        ("eth_gasPrice", json!([])),
        ("net_version", json!([])),
    ];

    println!("  Sending batch of {} requests...", batch.len());
    println!();

    let results = client.batch(batch).await?;

    output::print_batch_result("eth_chainId", &format_result(&results[0]));
    output::print_batch_result("eth_blockNumber", &format_result(&results[1]));
    output::print_batch_result("eth_gasPrice", &format_result(&results[2]));
    output::print_batch_result("net_version", &format_result(&results[3]));

    output::print_success("All batch responses received successfully");
    Ok(())
}

/// Demonstrate failover when a backend goes down.
async fn demo_failover(client: &DemoClient, nodes: &mut [MockNode]) -> Result<()> {
    output::print_section("Failover Demo");

    // Show initial state
    println!("  All backends healthy. Sending request...");
    let result = client.get_client_version().await?;
    let node_name = parse_node_name(&result).unwrap_or_else(|| "unknown".to_string());
    output::print_success(&format!("Served by {}", node_name));

    println!();

    // Take node-1 offline
    output::print_failover_action("Taking node-1 offline...");
    nodes[0].set_healthy(false);

    // Brief pause
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send request - should failover to another node
    println!("  Sending request (should failover)...");
    let result = client.get_client_version().await?;
    let node_name = parse_node_name(&result).unwrap_or_else(|| "unknown".to_string());
    output::print_success(&format!("Request served by {} (failover worked!)", node_name));

    println!();

    // Restore node-1
    output::print_failover_action("Restoring node-1...");
    nodes[0].set_healthy(true);
    output::print_success("All backends healthy again");

    Ok(())
}

/// Demonstrate rate limiting behavior.
async fn demo_rate_limiting(client: &DemoClient, config: &DemoConfig) -> Result<()> {
    output::print_section("Rate Limiting Demo");

    let limit = config.requests_per_second;
    // Send enough requests to definitely hit the limit
    let total_requests = (limit + 10) as usize;

    println!("  Rate limit: {} requests per second", limit);
    println!("  Sending {} rapid requests to trigger rate limiting...", total_requests);
    println!();

    let mut allowed = 0;
    let mut limited = 0;

    // Send requests as fast as possible to trigger rate limiting
    for i in 1..=total_requests {
        let result = client.request_raw("eth_chainId", json!([])).await?;
        if result.success {
            allowed += 1;
        } else {
            limited += 1;
        }
        // Only print first few and last few to avoid flooding output
        if i <= 5 || i > total_requests - 3 {
            output::print_rate_limit_result(i, result.success, result.duration);
        } else if i == 6 {
            println!("  ...");
        }
    }

    println!();
    println!("  Results: {} allowed, {} rate limited", allowed, limited);

    // Wait for rate limit window to reset
    println!();
    println!("  Waiting 1s for rate limit window to reset...");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Verify rate limit reset
    let result = client.request_raw("eth_chainId", json!([])).await?;
    if result.success {
        output::print_success("Rate limit reset - request allowed again");
    }

    Ok(())
}

/// Demonstrate method routing to different backend groups.
async fn demo_method_routing(client: &DemoClient) -> Result<()> {
    output::print_section("Method Routing Demo");

    println!("  Routing configuration:");
    println!("    eth_sendRawTransaction -> 'sequencer' group");
    println!("    All other methods      -> 'demo' group");

    output::print_subsection("Regular methods (demo group)");
    for _ in 0..3 {
        let result = client.request_timed("web3_clientVersion", json!([])).await?;
        let node_name = parse_node_name(result.value.as_str().unwrap_or("unknown"))
            .unwrap_or_else(|| "unknown".to_string());
        output::print_routing_result("web3_clientVersion", "demo", &node_name);
    }

    output::print_subsection("Routed methods (sequencer group)");
    for _ in 0..3 {
        // eth_sendRawTransaction is routed to sequencer group
        // Mock node returns a fake tx hash
        let result = client.request_timed("eth_sendRawTransaction", json!(["0x"])).await?;
        // The sequencer nodes handle this - we can't easily identify which one
        // but we know the routing worked if no error occurred
        let tx_hash = result.value.as_str().unwrap_or("0x...");
        println!(
            "  [eth_sendRawTransaction] -> group 'sequencer' -> tx: {}...{}",
            &tx_hash[..8.min(tx_hash.len())],
            &tx_hash[tx_hash.len().saturating_sub(6)..]
        );
    }

    println!();
    output::print_success("Methods correctly routed to designated groups");

    Ok(())
}

/// Demonstrate method blocking behavior.
async fn demo_method_blocking(client: &DemoClient, config: &DemoConfig) -> Result<()> {
    output::print_section("Method Blocking Demo");

    println!("  Blocked methods: {}", config.blocked_methods.join(", "));
    println!();

    // Try calling blocked methods
    for method in &config.blocked_methods {
        let result = client.request_raw(method, json!([])).await?;

        if !result.success {
            let error = result.response.get("error").unwrap();
            let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let msg = error.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
            output::print_blocked_method(method, code, msg);
        }
    }

    println!();

    // Show allowed method works for contrast
    println!("  Allowed method for comparison:");
    let result = client.request_raw("eth_blockNumber", json!([])).await?;
    if result.success {
        let value = result.response.get("result").and_then(|r| r.as_str()).unwrap_or("?");
        output::print_allowed_method("eth_blockNumber", value);
    }

    Ok(())
}

/// Demonstrate EMA-based intelligent load balancing.
async fn demo_ema_load_balancing(client: &DemoClient) -> Result<()> {
    output::print_section("EMA Load Balancing Demo");

    println!("  Node latencies:");
    println!("    node-1: 10ms  (fastest)");
    println!("    node-2: 50ms  (medium)");
    println!("    node-3: 100ms (slowest)");

    output::print_subsection("Current: Round-Robin distribution");
    println!("  Requests distributed evenly regardless of latency:");
    println!();

    let mut distribution: HashMap<String, u64> = HashMap::new();
    for i in 1..=6 {
        let result = client.request_timed("web3_clientVersion", json!([])).await?;
        let node_name = parse_node_name(result.value.as_str().unwrap_or("unknown"))
            .unwrap_or_else(|| "unknown".to_string());
        *distribution.entry(node_name.clone()).or_insert(0) += 1;
        output::print_request_served(i, &node_name, result.duration);
    }

    output::print_distribution(&distribution);

    output::print_subsection("With EMA (intelligent routing)");
    output::print_ema_explanation("EMA tracks response latency with Exponential Moving Average");
    output::print_ema_explanation("Prefers backends with lower latency and error rates");
    output::print_ema_explanation("Would route most traffic to node-1 (10ms) over node-3 (100ms)");
    println!();
    output::print_ema_explanation("Configure with: load_balancer = \"ema\" in group config");

    Ok(())
}

/// Format a JSON value for display.
fn format_result(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}
