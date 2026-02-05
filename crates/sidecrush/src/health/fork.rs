//! Fork detection health check.
//!
//! Compares local node block hashes against an external reference node
//! to detect chain forks. Ports logic from `base/agent/service/service/checker.go`.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tracing::{debug, error, info};

use crate::{
    blockbuilding_healthcheck::EthClient,
    health::{HealthCheck, HealthStatus},
};

/// Fork detection health check.
///
/// Compares block hashes between local and external nodes at the same block number.
/// If hashes differ, a fork is detected.
///
/// This check is "best effort" - if the external node is unavailable, we assume
/// no fork rather than marking ourselves unhealthy.
pub struct ForkCheck<L: EthClient, E: EthClient> {
    local_client: L,
    external_client: E,
    status: AtomicU64,
}

impl<L: EthClient, E: EthClient> ForkCheck<L, E> {
    pub fn new(local_client: L, external_client: E) -> Self {
        Self {
            local_client,
            external_client,
            // Start as healthy - fork detection is best-effort
            status: AtomicU64::new(HealthStatus::Healthy.code() as u64),
        }
    }

    /// Check if there's a fork between local and external nodes.
    /// Returns true if a fork is detected, false otherwise.
    async fn is_forked(&self) -> Option<bool> {
        // Get latest block from local node
        let local_latest = match self.local_client.latest_header().await {
            Ok(h) => h,
            Err(e) => {
                debug!(error = %e, "failed to fetch local latest block for fork check");
                return None;
            }
        };

        // Get latest block from external node
        let external_latest = match self.external_client.latest_header().await {
            Ok(h) => h,
            Err(e) => {
                // Best effort - if external node is down, assume no fork
                info!(error = %e, "unable to get latest block from external node for fork checking, assuming no fork");
                return Some(false);
            }
        };

        // Find the minimum block number (common ancestor candidate)
        let min_block = local_latest.number.min(external_latest.number);

        // Get the block at min_block from local node
        let local_block = if local_latest.number == min_block {
            local_latest
        } else {
            match self.local_client.header_by_number(min_block).await {
                Ok(h) => h,
                Err(e) => {
                    info!(error = %e, "unable to get block from local node, assuming no fork");
                    return Some(false);
                }
            }
        };

        // Get the block at min_block from external node
        let external_block = if external_latest.number == min_block {
            external_latest
        } else {
            match self.external_client.header_by_number(min_block).await {
                Ok(h) => h,
                Err(e) => {
                    info!(error = %e, "unable to get block from external node, assuming no fork");
                    return Some(false);
                }
            }
        };

        // Compare hashes
        match (local_block.hash, external_block.hash) {
            (Some(local_hash), Some(external_hash)) => {
                if local_hash == external_hash {
                    debug!(
                        block_number = min_block,
                        hash = ?local_hash,
                        "no fork detected, common ancestor found"
                    );
                    Some(false)
                } else {
                    error!(
                        block_number = min_block,
                        local_hash = ?local_hash,
                        external_hash = ?external_hash,
                        "FORK DETECTED: different hashes at same block number"
                    );
                    Some(true)
                }
            }
            _ => {
                // If either hash is missing, we can't determine fork status
                debug!("block hash missing, cannot determine fork status");
                Some(false)
            }
        }
    }
}

#[async_trait]
impl<L: EthClient + 'static, E: EthClient + 'static> HealthCheck for ForkCheck<L, E> {
    async fn check(&self) {
        let status = match self.is_forked().await {
            Some(true) => HealthStatus::Unhealthy,
            Some(false) => HealthStatus::Healthy,
            None => HealthStatus::Error,
        };
        self.status.store(status.code() as u64, Ordering::Relaxed);
    }

    fn current_status(&self) -> HealthStatus {
        HealthStatus::from_code(self.status.load(Ordering::Relaxed) as u8)
    }

    fn name(&self) -> &'static str {
        "fork_detection"
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::blockbuilding_healthcheck::HeaderSummary;

    #[derive(Clone)]
    struct MockClient {
        headers: Arc<Mutex<Vec<HeaderSummary>>>,
    }

    impl MockClient {
        fn new(headers: Vec<HeaderSummary>) -> Self {
            Self { headers: Arc::new(Mutex::new(headers)) }
        }
    }

    #[async_trait]
    impl EthClient for MockClient {
        async fn latest_header(
            &self,
        ) -> Result<HeaderSummary, Box<dyn std::error::Error + Send + Sync>> {
            let headers = self.headers.lock().unwrap();
            headers.last().cloned().ok_or_else(|| "no headers".into())
        }

        async fn header_by_number(
            &self,
            number: u64,
        ) -> Result<HeaderSummary, Box<dyn std::error::Error + Send + Sync>> {
            let headers = self.headers.lock().unwrap();
            headers
                .iter()
                .find(|h| h.number == number)
                .cloned()
                .ok_or_else(|| format!("block {} not found", number).into())
        }
    }

    fn make_header(number: u64, hash: [u8; 32]) -> HeaderSummary {
        HeaderSummary {
            number,
            timestamp_unix_seconds: 1000 + number,
            transaction_count: 0,
            hash: Some(hash),
        }
    }

    #[tokio::test]
    async fn no_fork_when_hashes_match() {
        let hash1 = [1u8; 32];
        let hash2 = [2u8; 32];
        let hash3 = [3u8; 32];

        let local = MockClient::new(vec![
            make_header(1, hash1),
            make_header(2, hash2),
            make_header(3, hash3),
        ]);
        let external = MockClient::new(vec![
            make_header(1, hash1),
            make_header(2, hash2),
            make_header(3, hash3),
        ]);

        let check = ForkCheck::new(local, external);
        check.check().await;
        assert_eq!(check.current_status(), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn fork_detected_when_hashes_differ() {
        let hash1 = [1u8; 32];
        let hash2_local = [2u8; 32];
        let hash2_external = [99u8; 32]; // Different hash at block 2

        let local = MockClient::new(vec![make_header(1, hash1), make_header(2, hash2_local)]);
        let external = MockClient::new(vec![make_header(1, hash1), make_header(2, hash2_external)]);

        let check = ForkCheck::new(local, external);
        check.check().await;
        assert_eq!(check.current_status(), HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn uses_min_block_when_external_is_behind() {
        let hash1 = [1u8; 32];
        let hash2 = [2u8; 32];
        let hash3 = [3u8; 32];

        // Local is at block 3, external is at block 2
        let local = MockClient::new(vec![
            make_header(1, hash1),
            make_header(2, hash2),
            make_header(3, hash3),
        ]);
        let external = MockClient::new(vec![make_header(1, hash1), make_header(2, hash2)]);

        let check = ForkCheck::new(local, external);
        check.check().await;
        // Should compare at block 2 (min) and find no fork
        assert_eq!(check.current_status(), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn uses_min_block_when_local_is_behind() {
        let hash1 = [1u8; 32];
        let hash2 = [2u8; 32];
        let hash3 = [3u8; 32];

        // Local is at block 2, external is at block 3
        let local = MockClient::new(vec![make_header(1, hash1), make_header(2, hash2)]);
        let external = MockClient::new(vec![
            make_header(1, hash1),
            make_header(2, hash2),
            make_header(3, hash3),
        ]);

        let check = ForkCheck::new(local, external);
        check.check().await;
        // Should compare at block 2 (min) and find no fork
        assert_eq!(check.current_status(), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn fork_at_earlier_block() {
        let hash1 = [1u8; 32];
        let hash2_local = [2u8; 32];
        let hash2_external = [99u8; 32]; // Fork at block 2
        let hash3_local = [3u8; 32];
        let hash3_external = [100u8; 32];

        // Local is at block 3, external is at block 2 with different hash
        let local = MockClient::new(vec![
            make_header(1, hash1),
            make_header(2, hash2_local),
            make_header(3, hash3_local),
        ]);
        let external = MockClient::new(vec![
            make_header(1, hash1),
            make_header(2, hash2_external),
            make_header(3, hash3_external),
        ]);

        let check = ForkCheck::new(local, external);
        check.check().await;
        // Should compare at block 3 and detect fork
        assert_eq!(check.current_status(), HealthStatus::Unhealthy);
    }
}
