use std::{path::Path, sync::Arc, time::Duration};

use cadence::{CountedExt, Gauged, StatsdClient};
use tokio::time::interval;
use tracing::{debug, info};

use crate::blockbuilding_healthcheck::EthClient;

// Metric names matching Go Health Service
const METRIC_VOLUME_FREE_BYTES: &str = "node_service.agent.volume_free_bytes";
const METRIC_VOLUME_TOTAL_BYTES: &str = "node_service.agent.volume_total_bytes";
const METRIC_LATEST_BLOCK_NUMBER: &str = "latest_block_number";

// Provisioning gate file - metrics are only collected after node is ready
const READY_FILE: &str = "/app/assets/ready";

/// Metrics client wrapper for block building health checks.
/// Emits metrics every 2 seconds via status heartbeat (independent of poll frequency).
#[derive(Clone, Debug)]
pub struct HealthcheckMetrics {
    /// Prefixed client for base.blocks.* metrics
    client: Arc<StatsdClient>,
    /// Unprefixed client for node_service.agent.* metrics
    volume_client: Arc<StatsdClient>,
}

impl HealthcheckMetrics {
    pub fn new(client: StatsdClient, volume_client: StatsdClient) -> Self {
        Self { client: Arc::new(client), volume_client: Arc::new(volume_client) }
    }

    /// Increment status_healthy counter (2s heartbeat).
    pub fn increment_status_healthy(&self) {
        let _ = self.client.incr("healthy");
    }

    /// Increment status_delayed counter (2s heartbeat).
    pub fn increment_status_delayed(&self) {
        let _ = self.client.incr("delayed");
    }

    /// Increment status_unhealthy counter (2s heartbeat).
    pub fn increment_status_unhealthy(&self) {
        let _ = self.client.incr("unhealthy");
    }

    /// Increment status_error counter (2s heartbeat).
    pub fn increment_status_error(&self) {
        let _ = self.client.incr("error");
    }

    /// Emit volume free bytes gauge (no prefix, matches Go)
    /// Emits: node_service.agent.volume_free_bytes
    pub fn gauge_volume_free(&self, bytes: u64) {
        let _ = self.volume_client.gauge(METRIC_VOLUME_FREE_BYTES, bytes);
    }

    /// Emit volume total bytes gauge (no prefix, matches Go)
    /// Emits: node_service.agent.volume_total_bytes
    pub fn gauge_volume_total(&self, bytes: u64) {
        let _ = self.volume_client.gauge(METRIC_VOLUME_TOTAL_BYTES, bytes);
    }

    /// Emit latest block number gauge
    /// Emits: base.blocks.latest_block_number
    pub fn gauge_latest_block(&self, block_num: u64) {
        let _ = self.client.gauge(METRIC_LATEST_BLOCK_NUMBER, block_num);
    }

    /// Get the underlying statsd client Arc for sharing
    pub fn client(&self) -> Arc<StatsdClient> {
        self.client.clone()
    }
}

/// Platform metrics collector.
///
/// Collects system-level metrics (volume stats, block number) on a 60-second interval.
pub struct PlatformMetrics<C: EthClient> {
    metrics: HealthcheckMetrics,
    eth_client: C,
    data_dir: String,
}

impl<C: EthClient> PlatformMetrics<C> {
    pub fn new(metrics: HealthcheckMetrics, eth_client: C, data_dir: String) -> Self {
        Self { metrics, eth_client, data_dir }
    }

    /// Check if node is provisioned (ready file exists).
    fn is_provisioned(&self) -> bool {
        Path::new(READY_FILE).exists()
    }

    /// Collect and emit volume metrics (Linux-specific).
    #[cfg(target_os = "linux")]
    fn collect_volume_metrics(&self) {
        use std::{ffi::CString, mem::MaybeUninit};

        let path = match CString::new(self.data_dir.as_str()) {
            Ok(p) => p,
            Err(e) => {
                debug!(error = %e, "failed to create CString for data_dir");
                return;
            }
        };

        let mut stat: MaybeUninit<libc::statvfs> = MaybeUninit::uninit();
        let result = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };

        if result != 0 {
            debug!("statvfs failed");
            return;
        }

        let stat = unsafe { stat.assume_init() };
        let avail = stat.f_bavail * stat.f_bsize;
        let total = stat.f_blocks * stat.f_bsize;

        self.metrics.gauge_volume_free(avail);
        self.metrics.gauge_volume_total(total);
    }

    /// No-op on non-Linux platforms
    #[cfg(not(target_os = "linux"))]
    fn collect_volume_metrics(&self) {
        debug!("volume metrics not supported on this platform");
    }

    /// Collect and emit latest block number.
    async fn collect_block_metrics(&self) {
        match self.eth_client.latest_header().await {
            Ok(header) => {
                self.metrics.gauge_latest_block(header.number);
                debug!(block_number = header.number, "collected block metric");
            }
            Err(e) => {
                // Skip on failure - don't emit partial/stale data
                debug!(error = %e, "failed to fetch block for metrics");
            }
        }
    }

    /// Collect all platform metrics once.
    async fn collect(&self) {
        if !self.is_provisioned() {
            debug!("not collecting metrics, node is not provisioned");
            return;
        }

        self.collect_volume_metrics();
        self.collect_block_metrics().await;
    }

    /// Run the platform metrics collection loop (60-second interval).
    pub async fn run(&self) {
        info!(data_dir = %self.data_dir, "starting platform metrics collector");

        // Collect immediately on startup
        self.collect().await;

        let mut ticker = interval(Duration::from_secs(60));
        loop {
            ticker.tick().await;
            self.collect().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{net::UdpSocket, sync::Mutex};

    use async_trait::async_trait;
    use cadence::UdpMetricSink;

    use super::*;
    use crate::blockbuilding_healthcheck::HeaderSummary;

    struct MockClient {
        header: Mutex<HeaderSummary>,
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

    fn mock_metrics() -> HealthcheckMetrics {
        // Prefixed client for health counters
        let socket1 = UdpSocket::bind("0.0.0.0:0").unwrap();
        socket1.set_nonblocking(true).unwrap();
        let sink1 = UdpMetricSink::from("127.0.0.1:8125", socket1).unwrap();
        let client = StatsdClient::from_sink("base.blocks", sink1);

        // Unprefixed client for volume metrics
        let socket2 = UdpSocket::bind("0.0.0.0:0").unwrap();
        socket2.set_nonblocking(true).unwrap();
        let sink2 = UdpMetricSink::from("127.0.0.1:8125", socket2).unwrap();
        let volume_client = StatsdClient::from_sink("", sink2);

        HealthcheckMetrics::new(client, volume_client)
    }

    #[test]
    fn healthcheck_metrics_methods_dont_panic() {
        let metrics = mock_metrics();
        metrics.increment_status_healthy();
        metrics.increment_status_delayed();
        metrics.increment_status_unhealthy();
        metrics.increment_status_error();
        metrics.gauge_volume_free(1000);
        metrics.gauge_volume_total(2000);
        metrics.gauge_latest_block(12345);
    }

    #[tokio::test]
    async fn platform_metrics_collect_doesnt_panic() {
        let metrics = mock_metrics();
        let client = MockClient {
            header: Mutex::new(HeaderSummary {
                number: 100,
                timestamp_unix_seconds: 1234567890,
                transaction_count: 5,
                hash: None,
            }),
        };
        let collector = PlatformMetrics::new(metrics, client, "/tmp".to_string());

        // This should not panic even if not provisioned
        collector.collect().await;
    }
}
