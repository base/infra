#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod backend;
pub use backend::{Backend, ConsensusTracker, HealthStatus, HealthTracker};

mod cache;
pub use cache::{Cache, CacheError};

mod codec;
pub use codec::{CodecConfig, CodecError, Decode, DefaultCodecConfig, Encode};

mod load_balancer;
pub use load_balancer::LoadBalancer;

mod rate_limiter;
pub use rate_limiter::{RateLimitError, RateLimitResult, RateLimiter};

mod runtime;
pub use runtime::{
    Clock, Counter, Gauge, Handle, Histogram, JoinError, MetricsRegistry, Signal, Spawner,
};
