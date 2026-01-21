//! Health check wrappers for modifying check behavior.
//!
//! Ports wrapper strategies from `base/agent/service/service/strategy.go`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;

use crate::health::{HealthCheck, HealthStatus};

/// Wrapper that latches to healthy once the inner check passes.
///
/// Once the wrapped health check returns healthy, this wrapper will always
/// return healthy, even if the inner check later fails. This is used for
/// K8s startup probes to prevent pods from being killed during temporary
/// block production delays after initial sync.
///
/// Ports `OnceHealthyAlwaysHealthyStrategy` from Go.
pub struct OnceHealthyAlwaysHealthy<C: HealthCheck> {
    inner: C,
    /// Whether we've ever been healthy
    previously_healthy: AtomicBool,
}

impl<C: HealthCheck> OnceHealthyAlwaysHealthy<C> {
    pub fn new(inner: C) -> Self {
        info!(inner = inner.name(), "starting once-healthy-always-healthy wrapper");
        Self {
            inner,
            previously_healthy: AtomicBool::new(false),
        }
    }

    /// Check if the wrapper has latched to healthy.
    pub fn has_been_healthy(&self) -> bool {
        self.previously_healthy.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl<C: HealthCheck + 'static> HealthCheck for OnceHealthyAlwaysHealthy<C> {
    async fn check(&self) {
        // Always run the inner check to keep its state updated
        self.inner.check().await;

        let inner_status = self.inner.current_status();
        let was_healthy = self.previously_healthy.load(Ordering::Relaxed);

        if inner_status.is_healthy_for_probe() && !was_healthy {
            info!(inner = self.inner.name(), "passed initial health check, latching to healthy");
            self.previously_healthy.store(true, Ordering::Relaxed);
        } else if !inner_status.is_healthy_for_probe() && was_healthy {
            info!(
                inner = self.inner.name(),
                inner_status = ?inner_status,
                "inner check unhealthy but returning healthy (latched)"
            );
        }
    }

    fn current_status(&self) -> HealthStatus {
        // If we've ever been healthy, always return healthy
        if self.previously_healthy.load(Ordering::Relaxed) {
            return HealthStatus::Healthy;
        }

        // Otherwise, return the inner status
        self.inner.current_status()
    }

    fn name(&self) -> &'static str {
        "once_healthy_always_healthy"
    }
}

/// A health check that can be shared across threads.
///
/// Wraps any HealthCheck in an Arc for use in multiple contexts
/// (e.g., both health engine and HTTP server).
pub struct SharedHealthCheck {
    inner: Arc<dyn HealthCheck>,
}

impl SharedHealthCheck {
    pub fn new<C: HealthCheck + 'static>(check: C) -> Self {
        Self {
            inner: Arc::new(check),
        }
    }

    pub fn from_arc(inner: Arc<dyn HealthCheck>) -> Self {
        Self { inner }
    }
}

impl Clone for SharedHealthCheck {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[async_trait]
impl HealthCheck for SharedHealthCheck {
    async fn check(&self) {
        self.inner.check().await;
    }

    fn current_status(&self) -> HealthStatus {
        self.inner.current_status()
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU8;

    /// A simple mock health check for testing
    struct MockHealthCheck {
        status: AtomicU8,
        name: &'static str,
    }

    impl MockHealthCheck {
        fn new(name: &'static str) -> Self {
            Self {
                status: AtomicU8::new(HealthStatus::Unhealthy.code()),
                name,
            }
        }

        fn set_status(&self, status: HealthStatus) {
            self.status.store(status.code(), Ordering::Relaxed);
        }
    }

    #[async_trait]
    impl HealthCheck for MockHealthCheck {
        async fn check(&self) {
            // No-op, status is set externally
        }

        fn current_status(&self) -> HealthStatus {
            HealthStatus::from_code(self.status.load(Ordering::Relaxed))
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    #[tokio::test]
    async fn latches_to_healthy() {
        let inner = MockHealthCheck::new("test");
        let wrapper = OnceHealthyAlwaysHealthy::new(inner);

        // Initially unhealthy
        assert_eq!(wrapper.current_status(), HealthStatus::Unhealthy);
        assert!(!wrapper.has_been_healthy());

        // Set inner to healthy and check
        wrapper.inner.set_status(HealthStatus::Healthy);
        wrapper.check().await;

        assert_eq!(wrapper.current_status(), HealthStatus::Healthy);
        assert!(wrapper.has_been_healthy());

        // Set inner back to unhealthy - wrapper should still return healthy
        wrapper.inner.set_status(HealthStatus::Unhealthy);
        wrapper.check().await;

        assert_eq!(wrapper.current_status(), HealthStatus::Healthy);
        assert!(wrapper.has_been_healthy());
    }

    #[tokio::test]
    async fn delayed_counts_as_healthy_for_latch() {
        let inner = MockHealthCheck::new("test");
        let wrapper = OnceHealthyAlwaysHealthy::new(inner);

        // Delayed should trigger the latch
        wrapper.inner.set_status(HealthStatus::Delayed);
        wrapper.check().await;

        assert_eq!(wrapper.current_status(), HealthStatus::Healthy);
        assert!(wrapper.has_been_healthy());
    }

    #[tokio::test]
    async fn error_does_not_latch() {
        let inner = MockHealthCheck::new("test");
        let wrapper = OnceHealthyAlwaysHealthy::new(inner);

        // Error should not trigger latch
        wrapper.inner.set_status(HealthStatus::Error);
        wrapper.check().await;

        assert_eq!(wrapper.current_status(), HealthStatus::Error);
        assert!(!wrapper.has_been_healthy());
    }
}
