//! In-memory LRU cache implementation.

use std::{
    num::NonZeroUsize,
    sync::Mutex,
    time::{Duration, Instant},
};

use bytes::Bytes;
use lru::LruCache;
use roxy_traits::{Cache, CacheError};

/// Entry in the memory cache.
struct CacheEntry {
    value: Bytes,
    expires_at: Instant,
}

/// In-memory LRU cache.
pub struct MemoryCache {
    cache: Mutex<LruCache<String, CacheEntry>>,
}

impl MemoryCache {
    /// Create a new memory cache with the given capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1000).unwrap());
        Self { cache: Mutex::new(LruCache::new(cap)) }
    }
}

impl std::fmt::Debug for MemoryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryCache").finish_non_exhaustive()
    }
}

impl Cache for MemoryCache {
    async fn get(&self, key: &str) -> Result<Option<Bytes>, CacheError> {
        let mut cache =
            self.cache.lock().map_err(|e| CacheError(format!("lock poisoned: {}", e)))?;

        if let Some(entry) = cache.get(key) {
            if entry.expires_at > Instant::now() {
                return Ok(Some(entry.value.clone()));
            }
            cache.pop(key);
        }

        Ok(None)
    }

    async fn put(&self, key: &str, value: Bytes, ttl: Duration) -> Result<(), CacheError> {
        let mut cache =
            self.cache.lock().map_err(|e| CacheError(format!("lock poisoned: {}", e)))?;

        let entry = CacheEntry { value, expires_at: Instant::now() + ttl };

        cache.put(key.to_string(), entry);
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        let mut cache =
            self.cache.lock().map_err(|e| CacheError(format!("lock poisoned: {}", e)))?;

        cache.pop(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[tokio::test]
    async fn test_put_and_get() {
        let cache = MemoryCache::new(100);
        let key = "test_key";
        let value = Bytes::from("test_value");

        cache.put(key, value.clone(), Duration::from_secs(60)).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, Some(value));
    }

    #[tokio::test]
    async fn test_get_missing_key() {
        let cache = MemoryCache::new(100);
        let result = cache.get("nonexistent").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_delete() {
        let cache = MemoryCache::new(100);
        let key = "test_key";
        let value = Bytes::from("test_value");

        cache.put(key, value, Duration::from_secs(60)).await.unwrap();
        cache.delete(key).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_expiration() {
        let cache = MemoryCache::new(100);
        let key = "test_key";
        let value = Bytes::from("test_value");

        // Set with very short TTL
        cache.put(key, value, Duration::from_millis(1)).await.unwrap();

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(10)).await;

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_overwrite() {
        let cache = MemoryCache::new(100);
        let key = "test_key";
        let value1 = Bytes::from("value1");
        let value2 = Bytes::from("value2");

        cache.put(key, value1, Duration::from_secs(60)).await.unwrap();
        cache.put(key, value2.clone(), Duration::from_secs(60)).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, Some(value2));
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let cache = MemoryCache::new(2); // Capacity of 2

        cache.put("key1", Bytes::from("value1"), Duration::from_secs(60)).await.unwrap();
        cache.put("key2", Bytes::from("value2"), Duration::from_secs(60)).await.unwrap();
        cache.put("key3", Bytes::from("value3"), Duration::from_secs(60)).await.unwrap();

        // key1 should be evicted (LRU)
        assert_eq!(cache.get("key1").await.unwrap(), None);
        assert!(cache.get("key2").await.unwrap().is_some());
        assert!(cache.get("key3").await.unwrap().is_some());
    }

    #[rstest]
    #[case::empty_value("key", Bytes::new())]
    #[case::small_value("key", Bytes::from("small"))]
    #[case::large_value("key", Bytes::from(vec![0u8; 10000]))]
    #[tokio::test]
    async fn test_various_value_sizes(#[case] key: &str, #[case] value: Bytes) {
        let cache = MemoryCache::new(100);

        cache.put(key, value.clone(), Duration::from_secs(60)).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, Some(value));
    }

    #[test]
    fn test_debug_impl() {
        let cache = MemoryCache::new(100);
        let debug_str = format!("{:?}", cache);
        assert!(debug_str.contains("MemoryCache"));
    }
}
