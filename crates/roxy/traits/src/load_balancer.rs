//! Load balancer traits for backend selection.

use std::sync::Arc;

use crate::backend::Backend;

/// Load balancer trait for selecting backends.
pub trait LoadBalancer: Send + Sync {
    /// Select a single backend.
    fn select(&self, backends: &[Arc<dyn Backend>]) -> Option<Arc<dyn Backend>>;

    /// Order backends by preference for failover.
    fn select_ordered(&self, backends: &[Arc<dyn Backend>]) -> Vec<Arc<dyn Backend>>;
}
