# Roxy

An extensible and modular RPC request router and proxy service built in Rust.

[![CI](https://github.com/refcell/roxy/actions/workflows/ci.yml/badge.svg)](https://github.com/refcell/roxy/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/roxy-proxy.svg)](https://crates.io/crates/roxy-proxy)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## Demo

<https://github.com/user-attachments/assets/3c3fa289-5a12-41ff-b556-e52e4cd1f04d>

## Overview

Roxy is a JSON-RPC proxy that sits between clients and upstream RPC backends. It distributes requests across multiple backends using exponential moving average (EMA) based health tracking to route traffic toward healthier endpoints. Responses can be cached in a tiered system with an in-memory LRU cache and optional Redis backing. Rate limiting uses a sliding window algorithm to control request throughput. The server accepts both HTTP and WebSocket connections and exposes Prometheus metrics for observability.

## Installation

```bash
cargo install roxy-proxy
```

## Usage

Run the proxy with a configuration file:

```bash
roxy-proxy --config roxy.toml
```

Validate configuration without starting the server:

```bash
roxy-proxy --config roxy.toml --check
```

## Configuration

Create a TOML configuration file:

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

| Section      | Description                                    |
|--------------|------------------------------------------------|
| server       | Bind address, port, connection limits          |
| backends     | Upstream RPC endpoints with timeout and retry  |
| groups       | Backend groups with load balancing strategy    |
| cache        | Memory size and TTL settings                   |
| rate_limit   | Requests per second and burst limits           |
| routing      | Method routing rules and blocked methods       |
| metrics      | Prometheus metrics endpoint                    |

## Acknowledgments

Roxy's design draws inspiration from the [commonwarexyz/monorepo](https://github.com/commonwarexyz/monorepo).

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
