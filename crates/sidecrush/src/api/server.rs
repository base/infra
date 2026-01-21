//! Axum HTTP server for health endpoints.
//!
//! Provides K8s-compatible health probe endpoints:
//! - `/_health` - Main health endpoint (aggregated status)
//! - `/startup-health` - Startup probe (once healthy, always healthy)
//! - `/liveness-health` - Liveness probe (checks RPC connectivity)
//!
//! Optional debug endpoints (gated by `enable_debug_endpoints`):
//! - `/debug/override/health` - Force healthy status
//! - `/debug/override/health/reset` - Reset override

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc,
};
use tracing::debug;

use crate::health::HealthStatus;

/// Shared application state for HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    /// Main health status (aggregated from all checks)
    pub health_status: Arc<AtomicU8>,
    /// Startup health status (once healthy, stays healthy)
    pub startup_status: Arc<AtomicU8>,
    /// Force healthy override for debugging.
    /// Note: The Go Health Service only has a startup-time `--force-healthy` flag.
    /// We add runtime toggle capability as an operational improvement, but gate it
    /// behind `--enable-debug-endpoints` to avoid accidental exposure in production.
    pub force_healthy: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(health_status: Arc<AtomicU8>, startup_status: Arc<AtomicU8>) -> Self {
        Self {
            health_status,
            startup_status,
            force_healthy: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a simple AppState with fresh atomics (for testing or simple usage)
    pub fn new_simple() -> Self {
        Self {
            health_status: Arc::new(AtomicU8::new(HealthStatus::Healthy.code())),
            startup_status: Arc::new(AtomicU8::new(HealthStatus::Healthy.code())),
            force_healthy: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Create the axum router with health endpoints.
///
/// # Arguments
/// * `state` - Shared application state
/// * `enable_debug_endpoints` - If true, registers `/debug/override/health` endpoints.
///   These allow runtime toggling of force-healthy status without container restart.
///   The Go Health Service only supports a startup-time `--force-healthy` flag;
///   these endpoints are a sidecrush-specific improvement for operational convenience.
pub fn create_router(state: AppState, enable_debug_endpoints: bool) -> Router {
    let router = Router::new()
        .route("/_health", get(health_handler))
        .route("/startup-health", get(startup_handler))
        .route("/liveness-health", get(liveness_handler));

    let router = if enable_debug_endpoints {
        tracing::warn!("Debug endpoints enabled - /debug/override/health routes are active");
        router
            .route("/debug/override/health", get(override_healthy))
            .route("/debug/override/health/reset", get(override_reset))
    } else {
        router
    };

    router.with_state(state)
}

/// Main health endpoint - returns aggregated health status.
/// Returns 200 if Healthy or Delayed, 503 otherwise.
async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    // Check force override first
    if state.force_healthy.load(Ordering::Relaxed) {
        debug!("/_health returning 200 (force override)");
        return StatusCode::OK;
    }

    let code = state.health_status.load(Ordering::Relaxed);
    let status = HealthStatus::from_code(code);

    if status.is_healthy_for_probe() {
        debug!(status = ?status, "/_health returning 200");
        StatusCode::OK
    } else {
        debug!(status = ?status, "/_health returning 503");
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Startup probe endpoint - once healthy, always healthy.
/// Used for K8s startup probes to allow slow initial sync.
async fn startup_handler(State(state): State<AppState>) -> impl IntoResponse {
    // Check force override first
    if state.force_healthy.load(Ordering::Relaxed) {
        debug!("/startup-health returning 200 (force override)");
        return StatusCode::OK;
    }

    let code = state.startup_status.load(Ordering::Relaxed);
    let status = HealthStatus::from_code(code);

    if status.is_healthy_for_probe() {
        debug!(status = ?status, "/startup-health returning 200");
        StatusCode::OK
    } else {
        debug!(status = ?status, "/startup-health returning 503");
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Liveness probe endpoint - basic connectivity check.
/// For now, returns the same as health. Future: check RPC connectivity.
async fn liveness_handler(State(state): State<AppState>) -> impl IntoResponse {
    // Check force override first
    if state.force_healthy.load(Ordering::Relaxed) {
        debug!("/liveness-health returning 200 (force override)");
        return StatusCode::OK;
    }

    // For liveness, we use the main health status
    // Future enhancement: check execution/consensus client URLs for 5xx errors
    let code = state.health_status.load(Ordering::Relaxed);
    let status = HealthStatus::from_code(code);

    if status.is_healthy_for_probe() {
        debug!(status = ?status, "/liveness-health returning 200");
        StatusCode::OK
    } else {
        debug!(status = ?status, "/liveness-health returning 503");
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Debug endpoint to force healthy status.
async fn override_healthy(State(state): State<AppState>) -> impl IntoResponse {
    state.force_healthy.store(true, Ordering::Relaxed);
    tracing::warn!("Health override enabled - forcing healthy status");
    "Health override enabled"
}

/// Debug endpoint to reset health override.
async fn override_reset(State(state): State<AppState>) -> impl IntoResponse {
    state.force_healthy.store(false, Ordering::Relaxed);
    tracing::info!("Health override reset");
    "Health override reset"
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    fn test_state() -> AppState {
        AppState::new_simple()
    }

    #[tokio::test]
    async fn health_endpoint_returns_200_when_healthy() {
        let state = test_state();
        let app = create_router(state, false);

        let response = app
            .oneshot(Request::builder().uri("/_health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_endpoint_returns_503_when_unhealthy() {
        let state = test_state();
        state
            .health_status
            .store(HealthStatus::Unhealthy.code(), Ordering::Relaxed);
        let app = create_router(state, false);

        let response = app
            .oneshot(Request::builder().uri("/_health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn health_endpoint_returns_200_when_delayed() {
        let state = test_state();
        state
            .health_status
            .store(HealthStatus::Delayed.code(), Ordering::Relaxed);
        let app = create_router(state, false);

        let response = app
            .oneshot(Request::builder().uri("/_health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn force_override_returns_200_regardless_of_status() {
        let state = test_state();
        state
            .health_status
            .store(HealthStatus::Error.code(), Ordering::Relaxed);
        // Manually set force_healthy (simulating what the debug endpoint would do)
        state.force_healthy.store(true, Ordering::Relaxed);
        let app = create_router(state, false);

        let response = app
            .oneshot(Request::builder().uri("/_health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn startup_endpoint_independent_of_health() {
        let state = test_state();
        // Set main health to unhealthy but startup to healthy
        state
            .health_status
            .store(HealthStatus::Unhealthy.code(), Ordering::Relaxed);
        state
            .startup_status
            .store(HealthStatus::Healthy.code(), Ordering::Relaxed);
        let app = create_router(state, false);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/startup-health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn debug_endpoints_disabled_by_default() {
        let state = test_state();
        let app = create_router(state, false);

        // Debug endpoint should return 404 when disabled
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/debug/override/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn override_endpoints_work_when_enabled() {
        let state = test_state();
        state
            .health_status
            .store(HealthStatus::Error.code(), Ordering::Relaxed);

        // First check it's unhealthy
        let app = create_router(state.clone(), true);
        let response = app
            .oneshot(Request::builder().uri("/_health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        // Enable override via debug endpoint
        let app = create_router(state.clone(), true);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/debug/override/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Now health should be OK
        let app = create_router(state.clone(), true);
        let response = app
            .oneshot(Request::builder().uri("/_health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Reset override
        let app = create_router(state.clone(), true);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/debug/override/health/reset")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Should be unhealthy again
        let app = create_router(state, true);
        let response = app
            .oneshot(Request::builder().uri("/_health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
