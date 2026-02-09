//! Cache traits for RPC response caching.

use std::time::Duration;

use bytes::Bytes;
use derive_more::{Debug, Display, Error};

/// Error type for cache operations.
#[derive(Debug, Display, Error)]
#[display("cache error: {_0}")]
#[error(ignore)]
pub struct CacheError(pub String);

/// Core cache trait for get/put/delete operations.
pub trait Cache: Send + Sync + 'static {
    /// Get a value from the cache.
    fn get(
        &self,
        key: &str,
    ) -> impl std::future::Future<Output = Result<Option<Bytes>, CacheError>> + Send;

    /// Put a value into the cache with a TTL.
    fn put(
        &self,
        key: &str,
        value: Bytes,
        ttl: Duration,
    ) -> impl std::future::Future<Output = Result<(), CacheError>> + Send;

    /// Delete a value from the cache.
    fn delete(&self, key: &str)
    -> impl std::future::Future<Output = Result<(), CacheError>> + Send;
}
