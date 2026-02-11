//! Service trait and implementations for all rpc-collector services.
//!
//! Every long-running component of rpc-collector implements [`Service`],
//! enabling the binary to build them via a factory and spawn them uniformly.

pub mod collector;
mod divergence_checker;
mod flashblock_validator;
mod health_checker;
mod health_server;
mod mempool_listener;
mod snapshots;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// A self-contained service that can be spawned onto a [`JoinSet`].
///
/// Every long-running component of rpc-collector implements this trait,
/// enabling the binary to build them via a factory and spawn them uniformly.
pub trait Service: Send {
    /// Human-readable name, used in logs.
    fn name(&self) -> &str;

    /// Spawn this service as one or more tasks on the given [`JoinSet`].
    fn spawn(self: Box<Self>, set: &mut JoinSet<()>, cancel: CancellationToken);
}

// Re-export all public types from service modules.
pub use collector::{
    l1::{L1Collector, L1CollectorConfig},
    l2::{L2Collector, L2CollectorConfig},
};
pub use divergence_checker::DivergenceCheckerService;
pub use flashblock_validator::FlashblockValidator;
pub use health_checker::{HealthChecker, Node, build_node_list, build_node_list_sanitized};
pub use health_server::HealthServer;
pub use mempool_listener::MempoolListenerService;
pub use snapshots::SnapshotsService;
