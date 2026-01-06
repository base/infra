//! Load balancer implementations.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use roxy_traits::{Backend, LoadBalancer};

/// EMA-based load balancer - lower latency = higher priority.
#[derive(Debug, Default)]
pub struct EmaLoadBalancer;

impl LoadBalancer for EmaLoadBalancer {
    fn select(&self, backends: &[Arc<dyn Backend>]) -> Option<Arc<dyn Backend>> {
        self.select_ordered(backends).into_iter().next()
    }

    fn select_ordered(&self, backends: &[Arc<dyn Backend>]) -> Vec<Arc<dyn Backend>> {
        let mut healthy: Vec<_> = backends.iter().filter(|b| b.is_healthy()).cloned().collect();
        let mut unhealthy: Vec<_> = backends.iter().filter(|b| !b.is_healthy()).cloned().collect();

        healthy.sort_by_key(|b| b.latency_ema());
        unhealthy.sort_by_key(|b| b.latency_ema());

        healthy.into_iter().chain(unhealthy).collect()
    }
}

/// Round-robin load balancer.
#[derive(Debug)]
pub struct RoundRobinBalancer {
    index: AtomicUsize,
}

impl RoundRobinBalancer {
    /// Create a new round-robin balancer.
    pub const fn new() -> Self {
        Self { index: AtomicUsize::new(0) }
    }
}

impl Default for RoundRobinBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer for RoundRobinBalancer {
    fn select(&self, backends: &[Arc<dyn Backend>]) -> Option<Arc<dyn Backend>> {
        let healthy: Vec<_> = backends.iter().filter(|b| b.is_healthy()).collect();
        if healthy.is_empty() {
            return None;
        }

        let idx = self.index.fetch_add(1, Ordering::Relaxed);
        Some(healthy[idx % healthy.len()].clone())
    }

    fn select_ordered(&self, backends: &[Arc<dyn Backend>]) -> Vec<Arc<dyn Backend>> {
        let healthy: Vec<_> = backends.iter().filter(|b| b.is_healthy()).cloned().collect();
        let idx = self.index.load(Ordering::Relaxed);

        let mut result = Vec::with_capacity(healthy.len());
        for i in 0..healthy.len() {
            result.push(healthy[(idx + i) % healthy.len()].clone());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use alloy_json_rpc::{RequestPacket, ResponsePacket};
    use async_trait::async_trait;
    use roxy_traits::HealthStatus;
    use roxy_types::RoxyError;

    use super::*;

    /// Mock backend for testing.
    struct MockBackend {
        name: String,
        healthy: bool,
        latency: Duration,
    }

    impl MockBackend {
        fn create(name: &str, healthy: bool, latency_ms: u64) -> Arc<dyn Backend> {
            Arc::new(Self {
                name: name.to_string(),
                healthy,
                latency: Duration::from_millis(latency_ms),
            })
        }
    }

    impl std::fmt::Debug for MockBackend {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockBackend")
                .field("name", &self.name)
                .field("healthy", &self.healthy)
                .finish()
        }
    }

    #[async_trait]
    impl Backend for MockBackend {
        fn name(&self) -> &str {
            &self.name
        }

        fn rpc_url(&self) -> &str {
            "http://mock"
        }

        async fn forward(&self, _request: RequestPacket) -> Result<ResponsePacket, RoxyError> {
            unimplemented!("mock backend")
        }

        fn health_status(&self) -> HealthStatus {
            if self.healthy {
                HealthStatus::Healthy
            } else {
                HealthStatus::Unhealthy { error_rate: 1.0 }
            }
        }

        fn latency_ema(&self) -> Duration {
            self.latency
        }
    }

    #[test]
    fn test_ema_load_balancer_empty() {
        let lb = EmaLoadBalancer;
        let backends: Vec<Arc<dyn Backend>> = vec![];

        assert!(lb.select(&backends).is_none());
        assert!(lb.select_ordered(&backends).is_empty());
    }

    #[test]
    fn test_ema_load_balancer_single() {
        let lb = EmaLoadBalancer;
        let backends = vec![MockBackend::create("b1", true, 100)];

        let selected = lb.select(&backends);
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name(), "b1");
    }

    #[test]
    fn test_ema_load_balancer_prefers_lower_latency() {
        let lb = EmaLoadBalancer;
        let backends = vec![
            MockBackend::create("slow", true, 500),
            MockBackend::create("fast", true, 50),
            MockBackend::create("medium", true, 200),
        ];

        let ordered = lb.select_ordered(&backends);
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].name(), "fast");
        assert_eq!(ordered[1].name(), "medium");
        assert_eq!(ordered[2].name(), "slow");
    }

    #[test]
    fn test_ema_load_balancer_healthy_before_unhealthy() {
        let lb = EmaLoadBalancer;
        let backends = vec![
            MockBackend::create("unhealthy_fast", false, 10),
            MockBackend::create("healthy_slow", true, 500),
        ];

        let ordered = lb.select_ordered(&backends);
        assert_eq!(ordered.len(), 2);
        // Healthy should come first even with higher latency
        assert_eq!(ordered[0].name(), "healthy_slow");
        assert_eq!(ordered[1].name(), "unhealthy_fast");
    }

    #[test]
    fn test_round_robin_empty() {
        let lb = RoundRobinBalancer::new();
        let backends: Vec<Arc<dyn Backend>> = vec![];

        assert!(lb.select(&backends).is_none());
    }

    #[test]
    fn test_round_robin_rotates() {
        let lb = RoundRobinBalancer::new();
        let backends = vec![
            MockBackend::create("b1", true, 100),
            MockBackend::create("b2", true, 100),
            MockBackend::create("b3", true, 100),
        ];

        let s1 = lb.select(&backends).unwrap();
        let s2 = lb.select(&backends).unwrap();
        let s3 = lb.select(&backends).unwrap();
        let s4 = lb.select(&backends).unwrap();

        // Should rotate through backends
        assert_eq!(s1.name(), "b1");
        assert_eq!(s2.name(), "b2");
        assert_eq!(s3.name(), "b3");
        assert_eq!(s4.name(), "b1"); // Wraps around
    }

    #[test]
    fn test_round_robin_skips_unhealthy() {
        let lb = RoundRobinBalancer::new();
        let backends = vec![
            MockBackend::create("healthy1", true, 100),
            MockBackend::create("unhealthy", false, 100),
            MockBackend::create("healthy2", true, 100),
        ];

        // Only healthy backends should be selected
        let s1 = lb.select(&backends).unwrap();
        let s2 = lb.select(&backends).unwrap();
        let s3 = lb.select(&backends).unwrap();

        // Should only cycle between healthy1 and healthy2
        assert!(s1.name() == "healthy1" || s1.name() == "healthy2");
        assert!(s2.name() == "healthy1" || s2.name() == "healthy2");
        assert_ne!(s1.name(), s2.name());
        assert_eq!(s1.name(), s3.name()); // Should wrap around
    }
}
