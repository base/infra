//! Health engine that aggregates multiple health checks.
//!
//! The engine runs checks on a configurable interval and maintains
//! an aggregated health status that can be read without locks.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::interval;
use tracing::{debug, info, warn};

use super::{HealthCheck, HealthStatus};

/// Node role for role-aware health check configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// HA sequencer node with conductor
    Sequencer,
    /// Block builder node
    Builder,
    /// RPC node serving external requests
    Rpc,
}

impl Role {
    /// Parse role from string (case-insensitive).
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "sequencer" => Some(Self::Sequencer),
            "builder" => Some(Self::Builder),
            "rpc" => Some(Self::Rpc),
            _ => None,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sequencer => write!(f, "sequencer"),
            Self::Builder => write!(f, "builder"),
            Self::Rpc => write!(f, "rpc"),
        }
    }
}

/// Health engine that aggregates multiple health checks.
///
/// The engine maintains an atomic aggregated status that is the "worst"
/// status across all checks. This allows the HTTP server to read status
/// without blocking.
pub struct HealthEngine {
    checks: Vec<Arc<dyn HealthCheck>>,
    /// Aggregated status across all checks
    status: Arc<AtomicU8>,
    /// Force healthy override - when set, always returns healthy
    force_healthy: AtomicBool,
    /// The role this engine is configured for
    role: Role,
    /// Poll interval for running checks
    poll_interval: Duration,
}

impl HealthEngine {
    /// Create a new health engine with the given role.
    pub fn new(role: Role, poll_interval: Duration) -> Self {
        Self {
            checks: Vec::new(),
            status: Arc::new(AtomicU8::new(HealthStatus::Healthy.code())),
            force_healthy: AtomicBool::new(false),
            role,
            poll_interval,
        }
    }

    /// Add a health check to the engine.
    pub fn add_check(&mut self, check: Arc<dyn HealthCheck>) {
        info!(check_name = check.name(), role = %self.role, "adding health check");
        self.checks.push(check);
    }

    /// Get a handle to the aggregated status for sharing with HTTP server.
    pub fn status_handle(&self) -> Arc<AtomicU8> {
        self.status.clone()
    }

    /// Set the force healthy override.
    pub fn set_force_healthy(&self, force: bool) {
        if force {
            warn!("forcing health status to healthy - use with caution");
        } else {
            info!("disabling force healthy override");
        }
        self.force_healthy.store(force, Ordering::Relaxed);
    }

    /// Get the current force healthy state.
    pub fn is_force_healthy(&self) -> bool {
        self.force_healthy.load(Ordering::Relaxed)
    }

    /// Get the current aggregated status.
    pub fn current_status(&self) -> HealthStatus {
        if self.force_healthy.load(Ordering::Relaxed) {
            return HealthStatus::Healthy;
        }
        HealthStatus::from_code(self.status.load(Ordering::Relaxed))
    }

    /// Run all checks once and update the aggregated status.
    pub async fn run_checks(&self) {
        if self.force_healthy.load(Ordering::Relaxed) {
            self.status
                .store(HealthStatus::Healthy.code(), Ordering::Relaxed);
            return;
        }

        let mut worst_status = HealthStatus::Healthy;

        for check in &self.checks {
            check.check().await;
            let status = check.current_status();

            debug!(
                check_name = check.name(),
                status = ?status,
                "check completed"
            );

            // Track worst status (higher code = worse)
            if status.code() > worst_status.code() {
                worst_status = status;
            }
        }

        self.status.store(worst_status.code(), Ordering::Relaxed);

        debug!(aggregated_status = ?worst_status, "health check cycle complete");
    }

    /// Run the health check polling loop.
    /// This runs indefinitely until the future is dropped.
    pub async fn run(&self) {
        info!(
            role = %self.role,
            poll_interval_ms = self.poll_interval.as_millis(),
            check_count = self.checks.len(),
            "starting health engine"
        );

        let mut ticker = interval(self.poll_interval);

        loop {
            ticker.tick().await;
            self.run_checks().await;
        }
    }

    /// Get the role this engine is configured for.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Get the number of registered checks.
    pub fn check_count(&self) -> usize {
        self.checks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::AtomicU8;

    struct MockCheck {
        name: &'static str,
        status: AtomicU8,
    }

    impl MockCheck {
        fn new(name: &'static str, status: HealthStatus) -> Self {
            Self {
                name,
                status: AtomicU8::new(status.code()),
            }
        }

        fn set_status(&self, status: HealthStatus) {
            self.status.store(status.code(), Ordering::Relaxed);
        }
    }

    #[async_trait]
    impl HealthCheck for MockCheck {
        async fn check(&self) {
            // Status is already set, nothing to do
        }

        fn current_status(&self) -> HealthStatus {
            HealthStatus::from_code(self.status.load(Ordering::Relaxed))
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    #[tokio::test]
    async fn aggregates_worst_status() {
        let mut engine = HealthEngine::new(Role::Rpc, Duration::from_secs(1));

        let check1 = Arc::new(MockCheck::new("check1", HealthStatus::Healthy));
        let check2 = Arc::new(MockCheck::new("check2", HealthStatus::Delayed));
        let check3 = Arc::new(MockCheck::new("check3", HealthStatus::Healthy));

        engine.add_check(check1);
        engine.add_check(check2);
        engine.add_check(check3);

        engine.run_checks().await;

        // Delayed is the worst status
        assert_eq!(engine.current_status(), HealthStatus::Delayed);
    }

    #[tokio::test]
    async fn unhealthy_overrides_delayed() {
        let mut engine = HealthEngine::new(Role::Rpc, Duration::from_secs(1));

        let check1 = Arc::new(MockCheck::new("check1", HealthStatus::Delayed));
        let check2 = Arc::new(MockCheck::new("check2", HealthStatus::Unhealthy));

        engine.add_check(check1);
        engine.add_check(check2);

        engine.run_checks().await;

        assert_eq!(engine.current_status(), HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn error_is_worst() {
        let mut engine = HealthEngine::new(Role::Rpc, Duration::from_secs(1));

        let check1 = Arc::new(MockCheck::new("check1", HealthStatus::Unhealthy));
        let check2 = Arc::new(MockCheck::new("check2", HealthStatus::Error));

        engine.add_check(check1);
        engine.add_check(check2);

        engine.run_checks().await;

        assert_eq!(engine.current_status(), HealthStatus::Error);
    }

    #[tokio::test]
    async fn force_healthy_overrides_all() {
        let mut engine = HealthEngine::new(Role::Rpc, Duration::from_secs(1));

        let check = Arc::new(MockCheck::new("check1", HealthStatus::Unhealthy));
        engine.add_check(check);

        engine.run_checks().await;
        assert_eq!(engine.current_status(), HealthStatus::Unhealthy);

        // Enable force healthy
        engine.set_force_healthy(true);
        assert!(engine.is_force_healthy());

        engine.run_checks().await;
        assert_eq!(engine.current_status(), HealthStatus::Healthy);

        // Disable force healthy
        engine.set_force_healthy(false);
        assert!(!engine.is_force_healthy());

        engine.run_checks().await;
        assert_eq!(engine.current_status(), HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn empty_engine_is_healthy() {
        let engine = HealthEngine::new(Role::Rpc, Duration::from_secs(1));
        engine.run_checks().await;
        assert_eq!(engine.current_status(), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn status_handle_is_shared() {
        let mut engine = HealthEngine::new(Role::Rpc, Duration::from_secs(1));
        let handle = engine.status_handle();

        let check = Arc::new(MockCheck::new("check1", HealthStatus::Delayed));
        engine.add_check(check);

        engine.run_checks().await;

        // Both engine and handle should see the same status
        assert_eq!(engine.current_status(), HealthStatus::Delayed);
        assert_eq!(
            HealthStatus::from_code(handle.load(Ordering::Relaxed)),
            HealthStatus::Delayed
        );
    }

    #[test]
    fn role_from_str() {
        assert_eq!(Role::from_str("sequencer"), Some(Role::Sequencer));
        assert_eq!(Role::from_str("SEQUENCER"), Some(Role::Sequencer));
        assert_eq!(Role::from_str("builder"), Some(Role::Builder));
        assert_eq!(Role::from_str("rpc"), Some(Role::Rpc));
        assert_eq!(Role::from_str("RPC"), Some(Role::Rpc));
        assert_eq!(Role::from_str("unknown"), None);
    }

    #[test]
    fn role_display() {
        assert_eq!(format!("{}", Role::Sequencer), "sequencer");
        assert_eq!(format!("{}", Role::Builder), "builder");
        assert_eq!(format!("{}", Role::Rpc), "rpc");
    }
}
