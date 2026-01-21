//! Block age health check strategy.
//!
//! Ports `MaxBlockAgeStrategy` from `base/agent/service/service/strategy.go`.
//! Checks if the latest block is within an acceptable age threshold.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tracing::{debug, info};

use crate::blockbuilding_healthcheck::EthClient;
use crate::health::{HealthCheck, HealthStatus};

/// Configuration for the block age health check.
#[derive(Debug, Clone)]
pub struct BlockAgeConfig {
    /// Maximum allowed block age during normal operation.
    pub max_block_age: Duration,
    /// Optional larger threshold during initial sync (predeploy).
    /// Once healthy, switches to `max_block_age`.
    pub max_block_age_predeploy: Option<Duration>,
    /// Number of blocks to wait after first healthy before considering fully healthy.
    /// Useful to ensure the node isn't just seeing a single recent block.
    pub wait_for_blocks: u64,
}

impl BlockAgeConfig {
    pub fn new(max_block_age: Duration) -> Self {
        Self {
            max_block_age,
            max_block_age_predeploy: None,
            wait_for_blocks: 0,
        }
    }

    pub fn with_predeploy(mut self, max_block_age_predeploy: Duration) -> Self {
        self.max_block_age_predeploy = Some(max_block_age_predeploy);
        self
    }

    pub fn with_wait_for_blocks(mut self, wait_for_blocks: u64) -> Self {
        self.wait_for_blocks = wait_for_blocks;
        self
    }
}

/// Block age health check.
///
/// Returns healthy if the latest block's timestamp is within the configured threshold.
/// Supports predeploy mode (larger threshold during initial sync) and wait_for_blocks
/// (require N blocks after first healthy).
///
pub struct BlockAgeCheck<C: EthClient> {
    client: C,
    config: BlockAgeConfig,
    /// Current health status
    status: AtomicU64, // Stores HealthStatus code + additional state
    /// Whether the node has ever been healthy
    has_been_healthy: AtomicBool,
    /// Whether we're still waiting for blocks after first healthy
    waiting_for_blocks: AtomicBool,
    /// Block number when first became healthy
    first_healthy_block: AtomicU64,
}

impl<C: EthClient> BlockAgeCheck<C> {
    pub fn new(client: C, config: BlockAgeConfig) -> Self {
        let waiting = config.wait_for_blocks > 0;
        Self {
            client,
            config,
            status: AtomicU64::new(HealthStatus::Error.code() as u64),
            has_been_healthy: AtomicBool::new(false),
            waiting_for_blocks: AtomicBool::new(waiting),
            first_healthy_block: AtomicU64::new(0),
        }
    }

    /// Evaluate health based on the given block timestamp and number.
    fn evaluate_health(&self, block_timestamp: u64, block_number: u64) -> HealthStatus {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();

        let block_age = Duration::from_secs(now.saturating_sub(block_timestamp));

        // Determine which threshold to use
        let threshold = if !self.has_been_healthy.load(Ordering::Relaxed) {
            self.config
                .max_block_age_predeploy
                .unwrap_or(self.config.max_block_age)
        } else {
            self.config.max_block_age
        };

        // Check if block is too old
        if block_age > threshold {
            debug!(
                block_age_secs = block_age.as_secs(),
                threshold_secs = threshold.as_secs(),
                "block too old"
            );
            return HealthStatus::Unhealthy;
        }

        // First time becoming healthy
        if !self.has_been_healthy.load(Ordering::Relaxed) {
            info!(
                block_number = block_number,
                block_age_secs = block_age.as_secs(),
                threshold_secs = threshold.as_secs(),
                "passed initial health check"
            );
            self.has_been_healthy.store(true, Ordering::Relaxed);
            self.first_healthy_block.store(block_number, Ordering::Relaxed);
        }

        // Check if we're still waiting for blocks
        if self.config.wait_for_blocks > 0 && self.waiting_for_blocks.load(Ordering::Relaxed) {
            let first_healthy = self.first_healthy_block.load(Ordering::Relaxed);
            let required_block = first_healthy + self.config.wait_for_blocks;

            if block_number < required_block {
                debug!(
                    current = block_number,
                    required = required_block,
                    "waiting for blocks"
                );
                // Still waiting - return Delayed (not Unhealthy) since the block IS fresh.
                // This passes K8s probes but signals we're in a transitional state.
                // Note: Go code has inverted logic here which appears to be a bug.
                // KALEY TODO check if this is accurate
                return HealthStatus::Delayed;
            }

            // We've reached the required block count
            info!(
                current = block_number,
                required = required_block,
                "passed block wait"
            );
            self.waiting_for_blocks.store(false, Ordering::Relaxed);
        }

        HealthStatus::Healthy
    }
}

#[async_trait]
impl<C: EthClient + 'static> HealthCheck for BlockAgeCheck<C> {
    async fn check(&self) {
        let result = self.client.latest_header().await;

        let status = match result {
            Ok(header) => self.evaluate_health(header.timestamp_unix_seconds, header.number),
            Err(e) => {
                debug!(error = %e, "failed to fetch latest header");
                HealthStatus::Error
            }
        };

        self.status.store(status.code() as u64, Ordering::Relaxed);
    }

    fn current_status(&self) -> HealthStatus {
        HealthStatus::from_code(self.status.load(Ordering::Relaxed) as u8)
    }

    fn name(&self) -> &'static str {
        "block_age"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockbuilding_healthcheck::HeaderSummary;
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
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[tokio::test]
    async fn healthy_when_block_is_fresh() {
        let header = Arc::new(Mutex::new(HeaderSummary {
            number: 100,
            timestamp_unix_seconds: now_secs(),
            transaction_count: 0,
        }));
        let client = MockClient { header };
        let config = BlockAgeConfig::new(Duration::from_secs(30));
        let check = BlockAgeCheck::new(client, config);

        check.check().await;
        assert_eq!(check.current_status(), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn unhealthy_when_block_is_old() {
        let header = Arc::new(Mutex::new(HeaderSummary {
            number: 100,
            timestamp_unix_seconds: now_secs() - 60, // 60 seconds old
            transaction_count: 0,
        }));
        let client = MockClient { header };
        let config = BlockAgeConfig::new(Duration::from_secs(30)); // 30 second threshold
        let check = BlockAgeCheck::new(client, config);

        check.check().await;
        assert_eq!(check.current_status(), HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn predeploy_uses_larger_threshold() {
        let header = Arc::new(Mutex::new(HeaderSummary {
            number: 100,
            timestamp_unix_seconds: now_secs() - 120, // 2 minutes old
            transaction_count: 0,
        }));
        let client = MockClient { header };
        let config = BlockAgeConfig::new(Duration::from_secs(30))
            .with_predeploy(Duration::from_secs(300)); // 5 minute predeploy threshold
        let check = BlockAgeCheck::new(client, config);

        // Should be healthy because we're in predeploy mode with larger threshold
        check.check().await;
        assert_eq!(check.current_status(), HealthStatus::Healthy);
        assert!(check.has_been_healthy.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn wait_for_blocks_returns_delayed() {
        let header = Arc::new(Mutex::new(HeaderSummary {
            number: 100,
            timestamp_unix_seconds: now_secs(),
            transaction_count: 0,
        }));
        let client = MockClient {
            header: header.clone(),
        };
        let config = BlockAgeConfig::new(Duration::from_secs(30)).with_wait_for_blocks(10);
        let check = BlockAgeCheck::new(client, config);

        // First check - block is fresh, but we're waiting for more blocks
        // Returns Delayed (not Unhealthy) since the block IS fresh
        check.check().await;
        assert_eq!(check.current_status(), HealthStatus::Delayed);
        assert!(check.has_been_healthy.load(Ordering::Relaxed));
        assert!(check.waiting_for_blocks.load(Ordering::Relaxed));

        // Advance block number but not enough - still Delayed
        header.lock().unwrap().number = 105;
        header.lock().unwrap().timestamp_unix_seconds = now_secs();
        check.check().await;
        assert_eq!(check.current_status(), HealthStatus::Delayed);

        // Advance to required block - now fully Healthy
        header.lock().unwrap().number = 110;
        header.lock().unwrap().timestamp_unix_seconds = now_secs();
        check.check().await;
        assert_eq!(check.current_status(), HealthStatus::Healthy);
        assert!(!check.waiting_for_blocks.load(Ordering::Relaxed));
    }
}
