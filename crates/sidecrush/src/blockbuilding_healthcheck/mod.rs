use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::time::{interval, timeout};
use tracing::{debug, error, info};

use crate::health::{HealthCheck, HealthStatus};
use crate::metrics::HealthcheckMetrics;

pub mod alloy_client;

#[derive(Debug, Clone)]
pub struct HealthcheckConfig {
    pub poll_interval_ms: u64,
    pub grace_period_ms: u64,
    pub unhealthy_node_threshold_ms: u64,
}

impl HealthcheckConfig {
    pub fn new(
        poll_interval_ms: u64,
        grace_period_ms: u64,
        unhealthy_node_threshold_ms: u64,
    ) -> Self {
        Self {
            poll_interval_ms,
            grace_period_ms,
            unhealthy_node_threshold_ms,
        }
    }
}

/// Represents a node being health-checked.
#[derive(Debug)]
pub struct Node {
    pub url: String,
    /// Tracks whether this is a new instance that hasn't been healthy yet.
    /// Uses AtomicBool for interior mutability.
    is_new_instance: AtomicBool,
}

impl Node {
    pub fn new(url: impl Into<String>, is_new_instance: bool) -> Self {
        Self {
            url: url.into(),
            is_new_instance: AtomicBool::new(is_new_instance),
        }
    }

    /// Check if this node is still considered a new instance
    pub fn is_new_instance(&self) -> bool {
        self.is_new_instance.load(Ordering::Relaxed)
    }

    /// Mark this node as no longer a new instance (it has become healthy)
    pub fn mark_healthy(&self) {
        self.is_new_instance.store(false, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone)]
pub struct HeaderSummary {
    pub number: u64,
    pub timestamp_unix_seconds: u64,
    pub transaction_count: usize,
    /// Block hash for fork detection
    pub hash: Option<[u8; 32]>,
}

#[async_trait]
pub trait EthClient: Send + Sync {
    /// Get the latest block header
    async fn latest_header(
        &self,
    ) -> Result<HeaderSummary, Box<dyn std::error::Error + Send + Sync>>;

    /// Get a block header by number (for fork detection)
    async fn header_by_number(
        &self,
        number: u64,
    ) -> Result<HeaderSummary, Box<dyn std::error::Error + Send + Sync>>;
}

/// Health checker for block production on a sequencer node.
/// 
/// Monitors block age and classifies health as:
/// - Healthy: Block age within grace period
/// - Delayed: Block age above grace period but below unhealthy threshold
/// - Unhealthy: Block age above unhealthy threshold
/// - Error: Failed to fetch block header
#[derive(Debug)]
pub struct BlockProductionHealthChecker<C: EthClient> {
    pub node: Node,
    pub client: C,
    pub config: HealthcheckConfig,
    status_code: Arc<AtomicU8>,
    metrics: HealthcheckMetrics,
}

impl<C: EthClient> BlockProductionHealthChecker<C> {
    pub fn new(
        node: Node,
        client: C,
        config: HealthcheckConfig,
        metrics: HealthcheckMetrics,
    ) -> Self {
        // default to healthy until first classification
        let initial_status: u8 = HealthStatus::Healthy.code();
        Self {
            node,
            client,
            config,
            status_code: Arc::new(AtomicU8::new(initial_status)),
            metrics,
        }
    }

    /// Returns a clone of the status code Arc for sharing with other components
    pub fn status_handle(&self) -> Arc<AtomicU8> {
        self.status_code.clone()
    }

    pub fn spawn_status_emitter(&self, period_ms: u64) -> tokio::task::JoinHandle<()> {
        let status = self.status_code.clone();
        let metrics = self.metrics.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(period_ms));
            loop {
                ticker.tick().await;
                let code = status.load(Ordering::Relaxed);
                match code {
                    0 => metrics.increment_status_healthy(),
                    1 => metrics.increment_status_delayed(),
                    2 => metrics.increment_status_unhealthy(),
                    _ => metrics.increment_status_error(),
                }
            }
        })
    }

    /// Run a single health check iteration.
    pub async fn run_health_check(&self) {
        let url = &self.node.url;
        let is_new = self.node.is_new_instance();

        debug!(sequencer = %url, "checking block production health");

        // Enforce a 2s timeout on header fetch
        let header_result = timeout(Duration::from_secs(2), self.client.latest_header()).await;

        let latest = match header_result {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => {
                if is_new {
                    debug!(sequencer = %url, error = %e, "waiting for node to become healthy");
                } else {
                    error!(sequencer = %url, error = %e, "failed to fetch block");
                }
                self.status_code
                    .store(HealthStatus::Error.code(), Ordering::Relaxed);
                return;
            }
            Err(_elapsed) => {
                if is_new {
                    debug!(sequencer = %url, "waiting for node to become healthy (timeout)");
                } else {
                    error!(sequencer = %url, "failed to fetch block (timeout)");
                }
                self.status_code
                    .store(HealthStatus::Error.code(), Ordering::Relaxed);
                return;
            }
        };

        // Compute block age
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs();
        let block_age_ms = now_secs.saturating_sub(latest.timestamp_unix_seconds) * 1000;

        let unhealthy_ms = self.config.unhealthy_node_threshold_ms;
        let grace_ms = self.config.grace_period_ms;

        let state = if is_new {
            HealthStatus::Healthy
        } else if block_age_ms >= unhealthy_ms {
            HealthStatus::Unhealthy
        } else if block_age_ms > grace_ms {
            HealthStatus::Delayed
        } else {
            HealthStatus::Healthy
        };
        self.status_code.store(state.code(), Ordering::Relaxed);

        if block_age_ms > grace_ms {
            if is_new {
                // Suppress delayed/unhealthy while new instance catches up
            } else if block_age_ms >= unhealthy_ms {
                error!(
                    blockNumber = latest.number,
                    sequencer = %url,
                    age_ms = block_age_ms,
                    "block production unhealthy"
                );
            } else {
                info!(
                    blockNumber = latest.number,
                    sequencer = %url,
                    age_ms = block_age_ms,
                    "delayed block production detected"
                );
            }
        } else {
            if is_new {
                self.node.mark_healthy();
                info!(
                    blockNumber = latest.number,
                    sequencer = %url,
                    "node becoming healthy"
                );
            }
            debug!(
                blockNumber = latest.number,
                sequencer = %url,
                "block production healthy"
            );
        }
    }

    pub async fn poll_for_health_checks(&self) {
        let mut ticker = interval(Duration::from_millis(self.config.poll_interval_ms));
        loop {
            ticker.tick().await;
            self.run_health_check().await;
        }
    }
}

// Implement the HealthCheck trait for BlockProductionHealthChecker
#[async_trait]
impl<C: EthClient + 'static> HealthCheck for BlockProductionHealthChecker<C> {
    async fn check(&self) {
        self.run_health_check().await;
    }

    fn current_status(&self) -> HealthStatus {
        HealthStatus::from_code(self.status_code.load(Ordering::Relaxed))
    }

    fn name(&self) -> &'static str {
        "block_production"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct MockClient {
        header: Arc<Mutex<HeaderSummary>>,
    }

    #[async_trait]
    impl EthClient for MockClient {
        async fn latest_header(
            &self,
        ) -> Result<HeaderSummary, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.header.lock().unwrap().clone())
        }

        async fn header_by_number(
            &self,
            _number: u64,
        ) -> Result<HeaderSummary, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.header.lock().unwrap().clone())
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs()
    }

    fn mock_metrics() -> HealthcheckMetrics {
        use cadence::{StatsdClient, UdpMetricSink};
        use std::net::UdpSocket;
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let sink = UdpMetricSink::from("127.0.0.1:8125", socket).unwrap();
        let statsd_client = StatsdClient::from_sink("test", sink);
        HealthcheckMetrics::new(statsd_client)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn healthy_new_block_emits_healthy() {
        let cfg = HealthcheckConfig::new(1_000, 5_000, 15_000);
        let start = now_secs();
        let shared_header = Arc::new(Mutex::new(HeaderSummary {
            number: 1,
            timestamp_unix_seconds: start,
            transaction_count: 5,
            hash: None,
        }));
        let client = MockClient {
            header: shared_header.clone(),
        };
        let node = Node::new("http://localhost:8545", false);
        let metrics = mock_metrics();
        let checker = BlockProductionHealthChecker::new(node, client, cfg, metrics);

        checker.run_health_check().await;
        assert_eq!(
            checker.current_status(),
            HealthStatus::Healthy
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn delayed_new_block_emits_delayed() {
        let grace_ms = 5_000u64;
        let cfg = HealthcheckConfig::new(1_000, grace_ms, 15_000);
        let start = now_secs();
        let shared_header = Arc::new(Mutex::new(HeaderSummary {
            number: 1,
            timestamp_unix_seconds: start,
            transaction_count: 5,
            hash: None,
        }));
        let client = MockClient {
            header: shared_header.clone(),
        };
        let node = Node::new("http://localhost:8545", false);
        let metrics = mock_metrics();
        let checker = BlockProductionHealthChecker::new(node, client, cfg, metrics);

        // First healthy block
        checker.run_health_check().await;
        assert_eq!(
            checker.current_status(),
            HealthStatus::Healthy
        );

        // Next block arrives but is delayed beyond grace
        let delayed_ts = start.saturating_sub((grace_ms / 1000) + 1);
        *shared_header.lock().unwrap() = HeaderSummary {
            number: 2,
            timestamp_unix_seconds: delayed_ts,
            transaction_count: 5,
            hash: None,
        };
        checker.run_health_check().await;
        assert_eq!(
            checker.current_status(),
            HealthStatus::Delayed
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unhealthy_same_block_triggers_single_stall_emit() {
        let unhealthy_ms = 15_000u64;
        let cfg = HealthcheckConfig::new(1_000, 5_000, unhealthy_ms);
        let start = now_secs();
        let shared_header = Arc::new(Mutex::new(HeaderSummary {
            number: 10,
            timestamp_unix_seconds: start,
            transaction_count: 5,
            hash: None,
        }));
        let client = MockClient {
            header: shared_header.clone(),
        };
        let node = Node::new("http://localhost:8545", false);
        let metrics = mock_metrics();
        let checker = BlockProductionHealthChecker::new(node, client, cfg, metrics);

        // First observation (healthy)
        checker.run_health_check().await;
        assert_eq!(
            checker.current_status(),
            HealthStatus::Healthy
        );

        // Same head, but now sufficiently old to be unhealthy -> emits stall once
        let unhealthy_ts = start.saturating_sub((unhealthy_ms / 1000) + 1);
        *shared_header.lock().unwrap() = HeaderSummary {
            number: 10,
            timestamp_unix_seconds: unhealthy_ts,
            transaction_count: 5,
            hash: None,
        };
        checker.run_health_check().await;
        assert_eq!(
            checker.current_status(),
            HealthStatus::Unhealthy
        );

        // Re-run again with same head: should not re-emit; flag remains set
        checker.run_health_check().await;
        assert_eq!(
            checker.current_status(),
            HealthStatus::Unhealthy
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn health_check_trait_implementation() {
        // Test that the HealthCheck trait is properly implemented
        let cfg = HealthcheckConfig::new(1_000, 5_000, 15_000);
        let start = now_secs();
        let shared_header = Arc::new(Mutex::new(HeaderSummary {
            number: 1,
            timestamp_unix_seconds: start,
            transaction_count: 5,
            hash: None,
        }));
        let client = MockClient {
            header: shared_header.clone(),
        };
        let node = Node::new("http://localhost:8545", false);
        let metrics = mock_metrics();
        let checker = BlockProductionHealthChecker::new(node, client, cfg, metrics);

        // Use trait methods
        assert_eq!(checker.name(), "block_production");
        
        // Initial status should be Healthy
        assert_eq!(checker.current_status(), HealthStatus::Healthy);

        // Run check via trait method
        checker.check().await;
        assert_eq!(checker.current_status(), HealthStatus::Healthy);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn new_instance_becomes_healthy() {
        let cfg = HealthcheckConfig::new(1_000, 5_000, 15_000);
        let start = now_secs();
        let shared_header = Arc::new(Mutex::new(HeaderSummary {
            number: 1,
            timestamp_unix_seconds: start,
            transaction_count: 5,
            hash: None,
        }));
        let client = MockClient {
            header: shared_header.clone(),
        };
        // Start as new instance
        let node = Node::new("http://localhost:8545", true);
        let metrics = mock_metrics();
        let checker = BlockProductionHealthChecker::new(node, client, cfg, metrics);

        // New instance should be marked healthy even if block is old
        assert!(checker.node.is_new_instance());
        
        checker.check().await;
        assert_eq!(checker.current_status(), HealthStatus::Healthy);
        
        // After first healthy check, should no longer be new instance
        assert!(!checker.node.is_new_instance());
    }
}
