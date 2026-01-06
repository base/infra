#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use hyper as _;
use roxy_types as _;
use tower as _;

mod error;
pub use error::ServerError;

mod http;
pub use http::{HttpAppState, create_router, handle_rpc, health_check};

mod metrics;
pub use metrics::{
    MetricsConfig, RoxyMetrics, metrics_handler, metrics_middleware, record_backend_error,
    record_backend_latency, record_backend_request, record_cache_access, record_latency,
    record_rate_limit, record_request, record_request_result, set_active_connections,
    set_active_subscriptions, set_backend_health, set_cache_size,
};

mod state;
pub use state::ServerBuilder;

mod websocket;
// Re-export for convenience
pub use axum::Router;
pub use websocket::{
    AppState, ConnectionGuard, ConnectionTracker, SubscriptionHandle, SubscriptionManager,
    SubscriptionParams, WsError, WsNotification, WsRequest, WsResponse, ws_handler,
};
