//! Redis cache implementation.

use std::time::Duration;

use bytes::Bytes;
use redis::{AsyncCommands, Client};
use roxy_traits::{Cache, CacheError};

/// Redis-based cache implementation.
///
/// Uses Redis for distributed caching with TTL support via SETEX.
#[derive(Debug, Clone)]
pub struct RedisCache {
    client: Client,
}

impl RedisCache {
    /// Create a new Redis cache with the given connection URL.
    ///
    /// # Arguments
    ///
    /// * `url` - Redis connection URL (e.g., "redis://127.0.0.1:6379")
    ///
    /// # Errors
    ///
    /// Returns a `CacheError` if the URL is invalid or connection cannot be established.
    pub fn new(url: &str) -> Result<Self, CacheError> {
        let client =
            Client::open(url).map_err(|e| CacheError(format!("failed to create client: {e}")))?;
        Ok(Self { client })
    }

    /// Get an async connection from the client.
    async fn get_connection(&self) -> Result<redis::aio::MultiplexedConnection, CacheError> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| CacheError(format!("connection error: {e}")))
    }
}

impl Cache for RedisCache {
    async fn get(&self, key: &str) -> Result<Option<Bytes>, CacheError> {
        let mut conn = self.get_connection().await?;

        let result: Option<Vec<u8>> =
            conn.get(key).await.map_err(|e| CacheError(format!("get error: {e}")))?;

        Ok(result.map(Bytes::from))
    }

    async fn put(&self, key: &str, value: Bytes, ttl: Duration) -> Result<(), CacheError> {
        let mut conn = self.get_connection().await?;

        // Convert TTL to seconds (minimum 1 second)
        let ttl_secs = ttl.as_secs().max(1);

        // Use SETEX for atomic set with expiration
        conn.set_ex::<_, _, ()>(key, value.as_ref(), ttl_secs)
            .await
            .map_err(|e| CacheError(format!("put error: {e}")))?;

        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        let mut conn = self.get_connection().await?;

        conn.del::<_, ()>(key).await.map_err(|e| CacheError(format!("delete error: {e}")))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redis_cache_new_valid_url() {
        let cache = RedisCache::new("redis://127.0.0.1:6379");
        assert!(cache.is_ok());
    }

    #[test]
    fn test_redis_cache_new_invalid_url() {
        let cache = RedisCache::new("not-a-valid-url");
        assert!(cache.is_err());
    }

    #[test]
    fn test_redis_cache_debug() {
        let cache = RedisCache::new("redis://127.0.0.1:6379").unwrap();
        let debug_str = format!("{:?}", cache);
        assert!(debug_str.contains("RedisCache"));
    }

    /// Integration tests that require a running Redis instance.
    /// Run with: cargo test --package roxy-cache -- --ignored
    mod integration {
        use super::*;

        const REDIS_URL: &str = "redis://127.0.0.1:6379";

        #[tokio::test]
        #[ignore]
        async fn test_redis_cache_put_and_get() {
            let cache = RedisCache::new(REDIS_URL).expect("Failed to create Redis cache");

            let key = "test_key_put_get";
            let value = Bytes::from("test_value");
            let ttl = Duration::from_secs(60);

            // Put value
            cache.put(key, value.clone(), ttl).await.expect("Failed to put value");

            // Get value
            let result = cache.get(key).await.expect("Failed to get value");
            assert_eq!(result, Some(value));

            // Cleanup
            cache.delete(key).await.expect("Failed to delete value");
        }

        #[tokio::test]
        #[ignore]
        async fn test_redis_cache_get_nonexistent() {
            let cache = RedisCache::new(REDIS_URL).expect("Failed to create Redis cache");

            let key = "nonexistent_key_12345";
            let result = cache.get(key).await.expect("Failed to get value");
            assert_eq!(result, None);
        }

        #[tokio::test]
        #[ignore]
        async fn test_redis_cache_delete() {
            let cache = RedisCache::new(REDIS_URL).expect("Failed to create Redis cache");

            let key = "test_key_delete";
            let value = Bytes::from("test_value");
            let ttl = Duration::from_secs(60);

            // Put value
            cache.put(key, value, ttl).await.expect("Failed to put value");

            // Delete value
            cache.delete(key).await.expect("Failed to delete value");

            // Verify deletion
            let result = cache.get(key).await.expect("Failed to get value");
            assert_eq!(result, None);
        }

        #[tokio::test]
        #[ignore]
        async fn test_redis_cache_ttl_expiration() {
            let cache = RedisCache::new(REDIS_URL).expect("Failed to create Redis cache");

            let key = "test_key_ttl";
            let value = Bytes::from("test_value");
            let ttl = Duration::from_secs(1);

            // Put value with 1 second TTL
            cache.put(key, value.clone(), ttl).await.expect("Failed to put value");

            // Verify value exists
            let result = cache.get(key).await.expect("Failed to get value");
            assert_eq!(result, Some(value));

            // Wait for TTL to expire
            tokio::time::sleep(Duration::from_secs(2)).await;

            // Verify value expired
            let result = cache.get(key).await.expect("Failed to get value");
            assert_eq!(result, None);
        }

        #[tokio::test]
        #[ignore]
        async fn test_redis_cache_overwrite() {
            let cache = RedisCache::new(REDIS_URL).expect("Failed to create Redis cache");

            let key = "test_key_overwrite";
            let value1 = Bytes::from("value1");
            let value2 = Bytes::from("value2");
            let ttl = Duration::from_secs(60);

            // Put first value
            cache.put(key, value1, ttl).await.expect("Failed to put value");

            // Overwrite with second value
            cache.put(key, value2.clone(), ttl).await.expect("Failed to put value");

            // Get value - should be second value
            let result = cache.get(key).await.expect("Failed to get value");
            assert_eq!(result, Some(value2));

            // Cleanup
            cache.delete(key).await.expect("Failed to delete value");
        }
    }
}
