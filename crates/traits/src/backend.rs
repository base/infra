//! Backend traits for RPC forwarding and health tracking.

use std::time::{Duration, Instant};

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use alloy_primitives::BlockNumber;
use async_trait::async_trait;
use roxy_types::RoxyError;

/// Health status of a backend.
#[derive(Debug, Clone, Copy)]
pub enum HealthStatus {
    /// Backend is healthy.
    Healthy,
    /// Backend is degraded with high latency.
    Degraded {
        /// Current latency EMA.
        latency_ema: Duration,
    },
    /// Backend is unhealthy with high error rate.
    Unhealthy {
        /// Current error rate (0.0 to 1.0).
        error_rate: f64,
    },
    /// Backend is temporarily banned.
    Banned {
        /// Time until the ban expires.
        until: Instant,
    },
}

/// Core backend trait for RPC forwarding.
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    /// Backend identifier.
    fn name(&self) -> &str;

    /// RPC endpoint URL.
    fn rpc_url(&self) -> &str;

    /// Forward RPC request packet (single or batch).
    async fn forward(&self, request: RequestPacket) -> Result<ResponsePacket, RoxyError>;

    /// Current health status.
    fn health_status(&self) -> HealthStatus;

    /// Latency EMA for load balancing.
    fn latency_ema(&self) -> Duration;

    /// Whether backend should receive requests.
    fn is_healthy(&self) -> bool {
        matches!(self.health_status(), HealthStatus::Healthy | HealthStatus::Degraded { .. })
    }
}

/// Health tracking with EMA.
pub trait HealthTracker: Send + Sync {
    /// Record a request result.
    fn record(&mut self, duration: Duration, success: bool);

    /// Get latency EMA.
    fn latency_ema(&self) -> Duration;

    /// Get error rate (0.0 to 1.0).
    fn error_rate(&self) -> f64;

    /// Get current health status.
    fn status(&self) -> HealthStatus;
}

/// Consensus tracking across backends.
pub trait ConsensusTracker: Send + Sync {
    /// Update a backend's reported block.
    fn update(&mut self, backend: &str, height: BlockNumber);

    /// Get the latest reported block (any backend).
    fn latest(&self) -> BlockNumber;

    /// Get the safe block (majority agree).
    fn safe(&self) -> BlockNumber;

    /// Get the finalized block (Byzantine-safe, f+1 agree).
    fn finalized(&self) -> BlockNumber;
}
