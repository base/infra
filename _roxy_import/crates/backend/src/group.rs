//! Backend group with failover.

use std::sync::Arc;

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use derive_more::Debug;
use roxy_traits::{Backend, LoadBalancer};
use roxy_types::RoxyError;

/// Response from a backend group.
#[derive(Debug)]
pub struct BackendResponse {
    /// The response packet.
    pub response: ResponsePacket,
    /// Name of the backend that served the request.
    pub served_by: String,
}

/// A group of backends with load balancing and failover.
#[derive(Debug)]
pub struct BackendGroup {
    name: String,
    #[debug("{} backends", backends.len())]
    backends: Vec<Arc<dyn Backend>>,
    #[debug(skip)]
    load_balancer: Arc<dyn LoadBalancer>,
}

impl BackendGroup {
    /// Create a new backend group.
    #[must_use]
    pub fn new(
        name: String,
        backends: Vec<Arc<dyn Backend>>,
        load_balancer: Arc<dyn LoadBalancer>,
    ) -> Self {
        Self { name, backends, load_balancer }
    }

    /// Get the group name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Forward a request with failover.
    pub async fn forward(&self, request: RequestPacket) -> Result<BackendResponse, RoxyError> {
        let ordered = self.load_balancer.select_ordered(&self.backends);

        if ordered.is_empty() {
            return Err(RoxyError::NoHealthyBackends);
        }

        for backend in ordered {
            match backend.forward(request.clone()).await {
                Ok(response) => {
                    return Ok(BackendResponse { response, served_by: backend.name().to_string() });
                }
                Err(e) if e.should_failover() => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        Err(RoxyError::NoHealthyBackends)
    }
}
