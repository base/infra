//! Backend creation utilities.
//!
//! This module provides factory types for creating HTTP backends from configuration.

use std::{collections::HashMap, sync::Arc, time::Duration};

use eyre::Result;
use roxy_backend::{BackendConfig as HttpBackendConfig, HttpBackend};
use roxy_config::RoxyConfig;
use roxy_traits::Backend;

/// Factory for creating HTTP backends from configuration.
///
/// # Example
///
/// ```ignore
/// use roxyproxy_cli::BackendFactory;
///
/// let factory = BackendFactory::new().with_batch_size(200);
/// let backends = factory.create(&config)?;
/// ```
#[derive(Debug, Clone)]
pub struct BackendFactory {
    default_batch_size: usize,
}

impl Default for BackendFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendFactory {
    /// Create a new backend factory with default settings.
    #[must_use]
    pub const fn new() -> Self {
        Self { default_batch_size: 100 }
    }

    /// Set the default batch size for backends.
    #[must_use]
    pub const fn with_batch_size(mut self, size: usize) -> Self {
        self.default_batch_size = size;
        self
    }

    /// Create backends from configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if backend creation fails.
    pub fn create(&self, config: &RoxyConfig) -> Result<HashMap<String, Arc<dyn Backend>>> {
        let mut backends = HashMap::new();

        for backend_config in &config.backends {
            let http_config = HttpBackendConfig {
                timeout: Duration::from_millis(backend_config.timeout_ms),
                max_retries: backend_config.max_retries,
                max_batch_size: self.default_batch_size,
            };

            let backend = HttpBackend::new(
                backend_config.name.clone(),
                backend_config.url.clone(),
                http_config,
            );

            backends.insert(backend_config.name.clone(), Arc::new(backend) as Arc<dyn Backend>);
            trace!(name = %backend_config.name, url = %backend_config.url, "Created backend");
        }

        Ok(backends)
    }
}

/// Create HTTP backends from configuration.
///
/// This is a convenience function that creates a [`BackendFactory`] and calls
/// [`BackendFactory::create`].
///
/// # Arguments
///
/// * `config` - The Roxy configuration
///
/// # Returns
///
/// A map of backend names to Backend trait objects.
///
/// # Errors
///
/// Returns an error if backend creation fails.
pub fn create_backends(config: &RoxyConfig) -> Result<HashMap<String, Arc<dyn Backend>>> {
    BackendFactory::new().create(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::minimal_config;

    #[test]
    fn test_create_backends() {
        let config = minimal_config();
        let backends = create_backends(&config).unwrap();
        assert_eq!(backends.len(), 1);
        assert!(backends.contains_key("primary"));
    }

    #[test]
    fn test_backend_factory_with_batch_size() {
        let factory = BackendFactory::new().with_batch_size(200);
        assert_eq!(factory.default_batch_size, 200);
    }

    #[test]
    fn test_backend_factory_default() {
        let factory = BackendFactory::default();
        assert_eq!(factory.default_batch_size, 100);
    }
}
