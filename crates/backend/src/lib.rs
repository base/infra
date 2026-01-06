#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

// Placeholder import for tracing (will be used when logging is added)
use tracing as _;

mod connection;
pub use connection::{ConnectionConfig, ConnectionState, ConnectionStateMachine};

mod group;
pub use group::{BackendGroup, BackendResponse};

mod health;
pub use health::{EmaHealthTracker, HealthConfig};

mod http;
pub use http::{BackendConfig, HttpBackend};

mod load_balancer;
pub use load_balancer::{EmaLoadBalancer, RoundRobinBalancer};

mod safe_tip;
pub use safe_tip::SafeTip;
