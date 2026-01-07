//! Backend group creation utilities.
//!
//! This module provides factory types for creating backend groups with load balancers
//! from configuration.

use std::{collections::HashMap, sync::Arc};

use eyre::{Result, eyre};
use roxy_backend::{BackendGroup, EmaLoadBalancer, RoundRobinBalancer};
use roxy_config::{LoadBalancerType, RoxyConfig};
use roxy_traits::{Backend, LoadBalancer};

/// Factory for creating backend groups from configuration.
///
/// # Example
///
/// ```ignore
/// use roxyproxy_cli::{BackendFactory, GroupFactory};
///
/// let backends = BackendFactory::new().create(&config)?;
/// let groups = GroupFactory::new().create(&config, &backends)?;
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct GroupFactory;

impl GroupFactory {
    /// Create a new group factory.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Create backend groups from configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - The Roxy configuration
    /// * `backends` - Map of backend names to Backend trait objects
    ///
    /// # Returns
    ///
    /// A map of group names to BackendGroup instances.
    ///
    /// # Errors
    ///
    /// Returns an error if a referenced backend doesn't exist.
    pub fn create(
        &self,
        config: &RoxyConfig,
        backends: &HashMap<String, Arc<dyn Backend>>,
    ) -> Result<HashMap<String, BackendGroup>> {
        let mut groups = HashMap::new();

        for group_config in &config.groups {
            // Collect backends for this group
            let mut group_backends = Vec::new();
            for backend_name in &group_config.backends {
                let backend = backends
                    .get(backend_name)
                    .ok_or_else(|| {
                        eyre!(
                            "backend '{}' not found for group '{}'",
                            backend_name,
                            group_config.name
                        )
                    })?
                    .clone();
                group_backends.push(backend);
            }

            // Create the appropriate load balancer
            let load_balancer =
                self.create_load_balancer(group_config.load_balancer, &group_config.name);

            let group = BackendGroup::new(group_config.name.clone(), group_backends, load_balancer);

            groups.insert(group_config.name.clone(), group);
            trace!(
                name = %group_config.name,
                backends = ?group_config.backends,
                load_balancer = %group_config.load_balancer,
                "Created backend group"
            );
        }

        Ok(groups)
    }

    /// Create a load balancer based on the configuration type.
    fn create_load_balancer(
        &self,
        lb_type: LoadBalancerType,
        group_name: &str,
    ) -> Arc<dyn LoadBalancer> {
        match lb_type {
            LoadBalancerType::Ema => Arc::new(EmaLoadBalancer),
            LoadBalancerType::RoundRobin => Arc::new(RoundRobinBalancer::new()),
            LoadBalancerType::Random => {
                warn!(
                    group = %group_name,
                    "Random load balancer not implemented, using EMA"
                );
                Arc::new(EmaLoadBalancer)
            }
            LoadBalancerType::LeastConnections => {
                warn!(
                    group = %group_name,
                    "LeastConnections load balancer not implemented, using EMA"
                );
                Arc::new(EmaLoadBalancer)
            }
        }
    }
}

/// Create backend groups with load balancers from configuration.
///
/// This is a convenience function that creates a [`GroupFactory`] and calls
/// [`GroupFactory::create`].
///
/// # Arguments
///
/// * `config` - The Roxy configuration
/// * `backends` - Map of backend names to Backend trait objects
///
/// # Returns
///
/// A map of group names to BackendGroup instances.
///
/// # Errors
///
/// Returns an error if a referenced backend doesn't exist.
pub fn create_groups(
    config: &RoxyConfig,
    backends: &HashMap<String, Arc<dyn Backend>>,
) -> Result<HashMap<String, BackendGroup>> {
    GroupFactory::new().create(config, backends)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backends::create_backends, testutils::minimal_config};

    #[test]
    fn test_create_groups() {
        let config = minimal_config();
        let backends = create_backends(&config).unwrap();
        let groups = create_groups(&config, &backends).unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups.contains_key("main"));
    }

    #[test]
    fn test_group_factory_default() {
        let factory = GroupFactory::default();
        // Just ensure it can be created
        let _ = format!("{:?}", factory);
    }
}
