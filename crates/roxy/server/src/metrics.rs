//! Prometheus metrics exporter for Roxy proxy.
//!
//! This module provides metrics collection and export functionality using
//! the [`metrics`] crate with Prometheus as the export format.
//!
//! # Metrics Naming Convention
//!
//! All metrics use the `roxy_` prefix and follow Prometheus naming conventions:
//! - Counters end with `_total`
//! - Durations use `_ms` suffix for milliseconds
//! - Gauges use descriptive names without suffixes
//!
//! # Example
//!
//! ```ignore
//! use roxy_server::metrics::{RoxyMetrics, record_request, record_latency};
//!
//! // Initialize metrics system
//! let metrics = RoxyMetrics::new()?;
//!
//! // Record metrics
//! record_request("eth_call");
//! record_latency("eth_call", 15.5);
//!
//! // Get Prometheus output
//! let output = metrics.render();
//! ```

use std::sync::Arc;

use axum::{extract::State, http::header::CONTENT_TYPE, response::IntoResponse};
use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

// ============================================================================
// Metrics Core
// ============================================================================

/// Metrics for Roxy proxy.
///
/// This struct manages the Prometheus metrics recorder and provides
/// the ability to render metrics in Prometheus text format.
#[derive(Debug, Clone)]
pub struct RoxyMetrics {
    handle: PrometheusHandle,
}

impl RoxyMetrics {
    /// Initialize the metrics system.
    ///
    /// This installs the Prometheus recorder as the global metrics recorder.
    /// Only one recorder can be installed per process.
    ///
    /// # Errors
    ///
    /// Returns an error if the recorder has already been installed.
    pub fn new() -> eyre::Result<Self> {
        let handle = PrometheusBuilder::new().install_recorder()?;

        Ok(Self { handle })
    }

    /// Get the Prometheus scrape output.
    ///
    /// Returns all collected metrics in Prometheus text exposition format.
    #[must_use]
    pub fn render(&self) -> String {
        self.handle.render()
    }
}

// ============================================================================
// Request Metrics
// ============================================================================

/// Record an incoming request.
///
/// Increments the `roxy_requests_total` counter with the given method label.
pub fn record_request(method: &str) {
    counter!("roxy_requests_total", "method" => method.to_string()).increment(1);
}

/// Record request latency.
///
/// Records the request duration in milliseconds to the `roxy_request_duration_ms` histogram.
pub fn record_latency(method: &str, duration_ms: f64) {
    histogram!("roxy_request_duration_ms", "method" => method.to_string()).record(duration_ms);
}

/// Record request result.
///
/// Increments the `roxy_request_results_total` counter with method and status labels.
pub fn record_request_result(method: &str, status: &str) {
    counter!(
        "roxy_request_results_total",
        "method" => method.to_string(),
        "status" => status.to_string()
    )
    .increment(1);
}

// ============================================================================
// Backend Metrics
// ============================================================================

/// Record backend request.
///
/// Increments the `roxy_backend_requests_total` counter with backend and group labels.
pub fn record_backend_request(backend: &str, group: &str) {
    counter!(
        "roxy_backend_requests_total",
        "backend" => backend.to_string(),
        "group" => group.to_string()
    )
    .increment(1);
}

/// Record backend latency.
///
/// Records the backend request duration in milliseconds to the `roxy_backend_duration_ms` histogram.
pub fn record_backend_latency(backend: &str, duration_ms: f64) {
    histogram!("roxy_backend_duration_ms", "backend" => backend.to_string()).record(duration_ms);
}

/// Record backend error.
///
/// Increments the `roxy_backend_errors_total` counter with backend and error type labels.
pub fn record_backend_error(backend: &str, error_type: &str) {
    counter!(
        "roxy_backend_errors_total",
        "backend" => backend.to_string(),
        "error_type" => error_type.to_string()
    )
    .increment(1);
}

/// Update backend health status.
///
/// Sets the `roxy_backend_healthy` gauge to 1.0 if healthy, 0.0 otherwise.
pub fn set_backend_health(backend: &str, healthy: bool) {
    gauge!("roxy_backend_healthy", "backend" => backend.to_string()).set(if healthy {
        1.0
    } else {
        0.0
    });
}

// ============================================================================
// Cache Metrics
// ============================================================================

/// Record cache hit/miss.
///
/// Increments either `roxy_cache_hits_total` or `roxy_cache_misses_total`
/// depending on whether the access was a hit or miss.
pub fn record_cache_access(hit: bool) {
    if hit {
        counter!("roxy_cache_hits_total").increment(1);
    } else {
        counter!("roxy_cache_misses_total").increment(1);
    }
}

/// Update cache size.
///
/// Sets the `roxy_cache_entries` gauge to the current number of cache entries.
pub fn set_cache_size(entries: usize) {
    gauge!("roxy_cache_entries").set(entries as f64);
}

// ============================================================================
// Rate Limiting Metrics
// ============================================================================

/// Record rate limit check.
///
/// Increments either `roxy_rate_limit_allowed_total` or `roxy_rate_limit_rejected_total`
/// depending on whether the request was allowed or rejected.
pub fn record_rate_limit(allowed: bool, client: &str) {
    if allowed {
        counter!("roxy_rate_limit_allowed_total").increment(1);
    } else {
        counter!(
            "roxy_rate_limit_rejected_total",
            "client" => client.to_string()
        )
        .increment(1);
    }
}

// ============================================================================
// Connection Metrics
// ============================================================================

/// Update active connections count.
///
/// Sets the `roxy_connections_active` gauge for both HTTP and WebSocket connection types.
pub fn set_active_connections(http: usize, websocket: usize) {
    gauge!("roxy_connections_active", "type" => "http").set(http as f64);
    gauge!("roxy_connections_active", "type" => "websocket").set(websocket as f64);
}

/// Update active subscriptions count.
///
/// Sets the `roxy_subscriptions_active` gauge to the current number of active subscriptions.
pub fn set_active_subscriptions(count: usize) {
    gauge!("roxy_subscriptions_active").set(count as f64);
}

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for metrics.
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// Whether metrics collection is enabled.
    pub enabled: bool,
    /// The endpoint path for Prometheus scraping.
    pub endpoint: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self { enabled: true, endpoint: "/metrics".to_string() }
    }
}

// ============================================================================
// HTTP Handler
// ============================================================================

/// Handler for /metrics endpoint.
///
/// Returns metrics in Prometheus text exposition format.
pub async fn metrics_handler(State(metrics): State<Arc<RoxyMetrics>>) -> impl IntoResponse {
    let body = metrics.render();
    ([(CONTENT_TYPE, "text/plain; version=0.0.4")], body)
}

// ============================================================================
// Middleware
// ============================================================================

/// Middleware to record request metrics.
///
/// This middleware records:
/// - Request count (`roxy_requests_total`)
/// - Request latency (`roxy_request_duration_ms`)
/// - Request results (`roxy_request_results_total`)
pub async fn metrics_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let start = std::time::Instant::now();
    let method = extract_method_from_request(&request);

    record_request(&method);

    let response = next.run(request).await;

    let duration = start.elapsed().as_secs_f64() * 1000.0;
    record_latency(&method, duration);

    let status = if response.status().is_success() { "success" } else { "error" };
    record_request_result(&method, status);

    response
}

/// Extract RPC method from request.
///
/// This is best-effort since we can't read the body in middleware.
/// Returns "rpc" as a fallback.
fn extract_method_from_request<B>(_request: &axum::http::Request<B>) -> String {
    // Try to extract method from parsed body or default to "unknown"
    // This is best-effort since we can't read the body in middleware
    "rpc".to_string()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that metrics config has sensible defaults.
    #[test]
    fn test_metrics_config_default() {
        let config = MetricsConfig::default();
        assert!(config.enabled);
        assert_eq!(config.endpoint, "/metrics");
    }

    /// Test that extract_method returns default value.
    #[test]
    fn test_extract_method_default() {
        let request = axum::http::Request::builder().uri("/").body(()).unwrap();
        let method = extract_method_from_request(&request);
        assert_eq!(method, "rpc");
    }

    // Note: Testing actual metrics recording requires a recorder to be installed.
    // The following tests verify the metrics functions can be called without panicking
    // when no recorder is installed (they become no-ops).

    #[test]
    fn test_record_request_no_panic() {
        record_request("eth_call");
    }

    #[test]
    fn test_record_latency_no_panic() {
        record_latency("eth_call", 15.5);
    }

    #[test]
    fn test_record_request_result_no_panic() {
        record_request_result("eth_call", "success");
    }

    #[test]
    fn test_record_backend_request_no_panic() {
        record_backend_request("backend1", "group1");
    }

    #[test]
    fn test_record_backend_latency_no_panic() {
        record_backend_latency("backend1", 25.0);
    }

    #[test]
    fn test_record_backend_error_no_panic() {
        record_backend_error("backend1", "timeout");
    }

    #[test]
    fn test_set_backend_health_no_panic() {
        set_backend_health("backend1", true);
        set_backend_health("backend1", false);
    }

    #[test]
    fn test_record_cache_access_no_panic() {
        record_cache_access(true);
        record_cache_access(false);
    }

    #[test]
    fn test_set_cache_size_no_panic() {
        set_cache_size(100);
    }

    #[test]
    fn test_record_rate_limit_no_panic() {
        record_rate_limit(true, "client1");
        record_rate_limit(false, "client1");
    }

    #[test]
    fn test_set_active_connections_no_panic() {
        set_active_connections(10, 5);
    }

    #[test]
    fn test_set_active_subscriptions_no_panic() {
        set_active_subscriptions(20);
    }
}
