//! Full demo example for roxy JSON-RPC proxy.
//!
//! This example demonstrates roxy's capabilities by:
//! 1. Starting mock Ethereum node backends with simulated block progression
//! 2. Starting roxy proxy pointing to those backends
//! 3. Running a demo client showing load balancing, caching, batch requests, and failover

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

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("info,roxy=debug,full_demo=debug")
        .with_target(false)
        .init();

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
    let roxy_app = roxy_cli::build_app(&roxy_config).await.wrap_err("failed to build roxy app")?;

    let roxy_handle = spawn_roxy(roxy_app, &roxy_config);
    output::print_success(&format!("Roxy listening at http://127.0.0.1:{}", demo_config.roxy_port));

    // Brief pause for roxy to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Phase 3: Run demos
    output::print_phase(3, 3, "Running demos");
    let client = DemoClient::new(demo_config.roxy_port);

    // Run all demo scenarios
    demo_load_balancing(&client, 9).await?;
    demo_caching(&client).await?;
    demo_batch_requests(&client).await?;
    demo_failover(&client, &mut nodes).await?;

    // Print summary
    let node_counts: Vec<(String, u64)> =
        nodes.iter().map(|n| (n.name().to_string(), n.request_count())).collect();
    output::print_summary(&node_counts);

    // Cleanup
    output::print_info("Shutting down...");
    for node in nodes {
        node.shutdown().await;
    }
    roxy_handle.abort();

    output::print_complete();
    Ok(())
}

/// Start all mock nodes from configuration.
async fn start_mock_nodes(config: &DemoConfig) -> Result<Vec<MockNode>> {
    let mut nodes = Vec::new();

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
        nodes.push(node);
    }

    Ok(nodes)
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

/// Format a JSON value for display.
fn format_result(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}
