//! Health check abstractions for sidecrushd.
//!
//! This module provides the core `HealthCheck` trait that all health checks implement,
//! along with the shared `HealthStatus` enum used across the daemon.

use async_trait::async_trait;

/// Health status codes matching the existing blockbuilding healthcheck values.
/// These map to HTTP status codes for K8s probes:
/// - Healthy/Delayed → 200 OK
/// - Unhealthy/Error → 503 Service Unavailable
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Node is healthy and producing blocks on time
    Healthy = 0,
    /// Block production is slightly delayed but within tolerance
    Delayed = 1,
    /// Node is unhealthy (block too old)
    Unhealthy = 2,
    /// Error fetching block or other failure
    Error = 3,
}

impl HealthStatus {
    /// Convert from the u8 code stored in AtomicU8
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Healthy,
            1 => Self::Delayed,
            2 => Self::Unhealthy,
            _ => Self::Error,
        }
    }

    /// Get the numeric code for this status
    pub fn code(&self) -> u8 {
        *self as u8
    }

    /// Returns true if this status should return HTTP 200 for K8s probes.
    /// Both Healthy and Delayed are considered "healthy enough" for probes.
    pub fn is_healthy_for_probe(&self) -> bool {
        matches!(self, Self::Healthy | Self::Delayed)
    }
}

/// Trait for all health checks in sidecrushd.
///
/// All implementations use interior mutability (atomics) so that:
/// - The polling loop can call `check()` without holding a mutable reference
/// - HTTP handlers can call `current_status()` concurrently
///
/// This allows the health engine to share check instances with the HTTP server
/// via `Arc<dyn HealthCheck>` without locks.
#[async_trait]
pub trait HealthCheck: Send + Sync {
    /// Run the health check and update internal state.
    ///
    /// Takes `&self` (not `&mut self`) - implementations must use interior
    /// mutability (AtomicU8, AtomicBool, Mutex) for any state updates.
    async fn check(&self);

    /// Return the current status from internal atomic state.
    /// This is a cheap read that doesn't perform any I/O.
    fn current_status(&self) -> HealthStatus;

    /// Human-readable name for logging and metrics.
    fn name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_status_code_roundtrip() {
        assert_eq!(HealthStatus::from_code(0), HealthStatus::Healthy);
        assert_eq!(HealthStatus::from_code(1), HealthStatus::Delayed);
        assert_eq!(HealthStatus::from_code(2), HealthStatus::Unhealthy);
        assert_eq!(HealthStatus::from_code(3), HealthStatus::Error);
        // Unknown codes map to Error
        assert_eq!(HealthStatus::from_code(99), HealthStatus::Error);
    }

    #[test]
    fn health_status_is_healthy_for_probe() {
        assert!(HealthStatus::Healthy.is_healthy_for_probe());
        assert!(HealthStatus::Delayed.is_healthy_for_probe());
        assert!(!HealthStatus::Unhealthy.is_healthy_for_probe());
        assert!(!HealthStatus::Error.is_healthy_for_probe());
    }

    #[test]
    fn health_status_code() {
        assert_eq!(HealthStatus::Healthy.code(), 0);
        assert_eq!(HealthStatus::Delayed.code(), 1);
        assert_eq!(HealthStatus::Unhealthy.code(), 2);
        assert_eq!(HealthStatus::Error.code(), 3);
    }
}
