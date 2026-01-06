//! HTTP backend implementation.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use async_trait::async_trait;
use roxy_traits::{Backend, HealthStatus, HealthTracker};
use roxy_types::RoxyError;
use tokio::sync::RwLock;

use crate::health::EmaHealthTracker;

/// Configuration for HTTP backend.
#[derive(Debug, Clone)]
pub struct BackendConfig {
    /// Request timeout.
    pub timeout: Duration,
    /// Maximum retry attempts.
    pub max_retries: u32,
    /// Maximum batch size.
    pub max_batch_size: usize,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(30), max_retries: 3, max_batch_size: 100 }
    }
}

/// HTTP backend for RPC forwarding.
#[derive(Debug)]
pub struct HttpBackend {
    name: String,
    rpc_url: String,
    client: reqwest::Client,
    health: Arc<RwLock<EmaHealthTracker>>,
    config: BackendConfig,
}

impl HttpBackend {
    /// Create a new HTTP backend.
    pub fn new(name: String, rpc_url: String, config: BackendConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("failed to build HTTP client");

        Self {
            name,
            rpc_url,
            client,
            health: Arc::new(RwLock::new(EmaHealthTracker::new(Default::default()))),
            config,
        }
    }

    async fn do_forward(&self, request: &RequestPacket) -> Result<ResponsePacket, RoxyError> {
        let start = Instant::now();

        let response = self
            .client
            .post(&self.rpc_url)
            .json(request)
            .send()
            .await
            .map_err(|_| RoxyError::BackendOffline { backend: self.name.clone() })?;

        let duration = start.elapsed();
        let success = response.status().is_success();

        self.health.write().await.record(duration, success);

        if !success {
            return Err(RoxyError::BackendOffline { backend: self.name.clone() });
        }

        response
            .json()
            .await
            .map_err(|e| RoxyError::Internal(format!("failed to parse response: {}", e)))
    }
}

#[async_trait]
impl Backend for HttpBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    async fn forward(&self, request: RequestPacket) -> Result<ResponsePacket, RoxyError> {
        let mut last_error = None;

        for attempt in 0..=self.config.max_retries {
            match self.do_forward(&request).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < self.config.max_retries {
                        let backoff = Duration::from_millis((2u64.pow(attempt) * 100).min(3000));
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }

        Err(last_error.unwrap())
    }

    fn health_status(&self) -> HealthStatus {
        self.health.try_read().map(|h| h.status()).unwrap_or(HealthStatus::Healthy)
    }

    fn latency_ema(&self) -> Duration {
        self.health.try_read().map(|h| h.latency_ema()).unwrap_or(Duration::ZERO)
    }
}
