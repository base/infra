# roxy-backend

Backend implementations for the Roxy RPC proxy.

## Overview

This crate provides backend implementations for forwarding RPC requests to upstream nodes.

## Components

- `HttpBackend` - HTTP-based RPC forwarding with retry logic
- `BackendGroup` - Group of backends with load balancing and failover
- `EmaHealthTracker` - EMA-based health tracking
- `SafeTip` - Byzantine-safe tip tracker
- `EmaLoadBalancer` - Latency-based load balancer
- `RoundRobinBalancer` - Round-robin load balancer
