# Roxy

An extensible and modular RPC request router and proxy service built in Rust.

## Features

- Load balancing across multiple RPC backends with EMA-based health tracking
- Tiered caching with in-memory LRU and Redis support
- Sliding window rate limiting
- HTTP and WebSocket support
- Method routing and request validation
- Prometheus metrics

## Repository Structure

```
bin/
  roxy/            Entry point for the Roxy binary
crates/
  backend/         HTTP backends with health tracking and load balancing
  cache/           Memory, Redis, and fallback cache implementations
  cli/             CLI definition and application builder
  config/          TOML configuration parsing and validation
  rpc/             JSON-RPC codec, routing, validation, and rate limiting
  runtime/         Tokio and deterministic runtime implementations
  server/          HTTP and WebSocket server with metrics
  test-utils/      Mock backends, fixtures, and async test helpers
  traits/          Core trait definitions for all components
  types/           Error types and alloy re-exports
```

## Prerequisites

- Rust 1.88+

## Getting Started

Clone the repository:

```bash
git clone https://github.com/refcell/roxy
cd roxy
```

Build:

```bash
cargo build --release
```

## Running Roxy

Create a configuration file:

```toml
[[backends]]
name = "primary"
url = "https://eth-mainnet.example.com"

[[backends]]
name = "fallback"
url = "https://eth-mainnet-fallback.example.com"

[[groups]]
name = "main"
backends = ["primary", "fallback"]
load_balancer = "ema"

[routing]
default_group = "main"

[cache]
enabled = true
memory_size = 10000

[server]
host = "0.0.0.0"
port = 8545
```

Run the proxy:

```bash
./target/release/roxy --config roxy.toml
```

Validate configuration without starting:

```bash
./target/release/roxy --config roxy.toml --check
```

## Configuration

| Section      | Description                                    |
|--------------|------------------------------------------------|
| server       | Bind address, port, connection limits          |
| backends     | Upstream RPC endpoints with timeout and retry  |
| groups       | Backend groups with load balancing strategy    |
| cache        | Memory size and TTL settings                   |
| rate_limit   | Requests per second and burst limits           |
| routing      | Method routing rules and blocked methods       |
| metrics      | Prometheus metrics endpoint                    |

## License

MIT
