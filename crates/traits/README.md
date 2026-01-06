# roxy-traits

Core traits for the Roxy RPC proxy.

## Overview

This crate defines the core abstractions used throughout the Roxy proxy.

## Traits

### Runtime

- `Spawner` - Task spawning with labels and shutdown signaling
- `Clock` - Time abstraction for testability
- `Counter`, `Histogram`, `Gauge` - Metrics primitives

### Backend

- `Backend` - RPC forwarding to upstream nodes
- `HealthTracker` - EMA-based health tracking
- `ConsensusTracker` - Byzantine-safe tip tracking

### Cache

- `Cache` - Generic cache with TTL support

### Load Balancer

- `LoadBalancer` - Backend selection and ordering for failover
