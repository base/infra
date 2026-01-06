//! Fallback cache wrapper that tries a primary cache first, then falls back to a secondary cache.

use std::time::Duration;

use bytes::Bytes;
use derive_more::Debug;
use roxy_traits::{Cache, CacheError};

/// A cache wrapper that tries a primary cache first, then falls back to a secondary cache.
///
/// On `get`: tries primary first, if miss or error, tries fallback.
/// On `put`: writes to both caches (write-through).
/// On `delete`: deletes from both caches.
#[derive(Debug)]
pub struct FallbackCache<P, F> {
    primary: P,
    fallback: F,
}

impl<P: Cache, F: Cache> FallbackCache<P, F> {
    /// Create a new fallback cache with the given primary and fallback caches.
    pub const fn new(primary: P, fallback: F) -> Self {
        Self { primary, fallback }
    }

    /// Returns a reference to the primary cache.
    pub const fn primary(&self) -> &P {
        &self.primary
    }

    /// Returns a reference to the fallback cache.
    pub const fn fallback(&self) -> &F {
        &self.fallback
    }
}

impl<P: Cache, F: Cache> Cache for FallbackCache<P, F> {
    async fn get(&self, key: &str) -> Result<Option<Bytes>, CacheError> {
        // Try primary first
        match self.primary.get(key).await {
            Ok(Some(value)) => return Ok(Some(value)),
            Ok(None) => {
                // Primary miss, try fallback
            }
            Err(_) => {
                // Primary error, try fallback
            }
        }

        // Try fallback
        self.fallback.get(key).await
    }

    async fn put(&self, key: &str, value: Bytes, ttl: Duration) -> Result<(), CacheError> {
        // Write-through: write to both caches
        // We attempt both writes and return the first error if any
        let primary_result = self.primary.put(key, value.clone(), ttl).await;
        let fallback_result = self.fallback.put(key, value, ttl).await;

        // Return the first error, preferring primary error if both fail
        primary_result?;
        fallback_result?;

        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        // Delete from both caches
        let primary_result = self.primary.delete(key).await;
        let fallback_result = self.fallback.delete(key).await;

        // Return the first error, preferring primary error if both fail
        primary_result?;
        fallback_result?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use rstest::rstest;

    use super::*;

    /// A mock cache for testing that can be configured to succeed or fail.
    #[derive(std::fmt::Debug)]
    struct MockCache {
        data: Mutex<HashMap<String, Bytes>>,
        should_fail: bool,
    }

    impl MockCache {
        fn new() -> Self {
            Self { data: Mutex::new(HashMap::new()), should_fail: false }
        }

        fn failing() -> Self {
            Self { data: Mutex::new(HashMap::new()), should_fail: true }
        }

        fn with_data(key: &str, value: Bytes) -> Self {
            let mut data = HashMap::new();
            data.insert(key.to_string(), value);
            Self { data: Mutex::new(data), should_fail: false }
        }

        fn contains_key(&self, key: &str) -> bool {
            self.data.lock().unwrap().contains_key(key)
        }
    }

    impl Cache for MockCache {
        async fn get(&self, key: &str) -> Result<Option<Bytes>, CacheError> {
            if self.should_fail {
                return Err(CacheError("mock cache error".to_string()));
            }
            let data = self.data.lock().unwrap();
            Ok(data.get(key).cloned())
        }

        async fn put(&self, key: &str, value: Bytes, _ttl: Duration) -> Result<(), CacheError> {
            if self.should_fail {
                return Err(CacheError("mock cache error".to_string()));
            }
            let mut data = self.data.lock().unwrap();
            data.insert(key.to_string(), value);
            Ok(())
        }

        async fn delete(&self, key: &str) -> Result<(), CacheError> {
            if self.should_fail {
                return Err(CacheError("mock cache error".to_string()));
            }
            let mut data = self.data.lock().unwrap();
            data.remove(key);
            Ok(())
        }
    }

    #[rstest]
    #[tokio::test]
    async fn test_get_from_primary_hit() {
        let primary = MockCache::with_data("key", Bytes::from("primary_value"));
        let fallback = MockCache::with_data("key", Bytes::from("fallback_value"));
        let cache = FallbackCache::new(primary, fallback);

        let result = cache.get("key").await.unwrap();
        assert_eq!(result, Some(Bytes::from("primary_value")));
    }

    #[rstest]
    #[tokio::test]
    async fn test_get_from_fallback_on_primary_miss() {
        let primary = MockCache::new();
        let fallback = MockCache::with_data("key", Bytes::from("fallback_value"));
        let cache = FallbackCache::new(primary, fallback);

        let result = cache.get("key").await.unwrap();
        assert_eq!(result, Some(Bytes::from("fallback_value")));
    }

    #[rstest]
    #[tokio::test]
    async fn test_get_from_fallback_on_primary_error() {
        let primary = MockCache::failing();
        let fallback = MockCache::with_data("key", Bytes::from("fallback_value"));
        let cache = FallbackCache::new(primary, fallback);

        let result = cache.get("key").await.unwrap();
        assert_eq!(result, Some(Bytes::from("fallback_value")));
    }

    #[rstest]
    #[tokio::test]
    async fn test_get_miss_both_caches() {
        let primary = MockCache::new();
        let fallback = MockCache::new();
        let cache = FallbackCache::new(primary, fallback);

        let result = cache.get("key").await.unwrap();
        assert_eq!(result, None);
    }

    #[rstest]
    #[tokio::test]
    async fn test_get_error_when_both_fail() {
        let primary = MockCache::failing();
        let fallback = MockCache::failing();
        let cache = FallbackCache::new(primary, fallback);

        let result = cache.get("key").await;
        assert!(result.is_err());
    }

    #[rstest]
    #[tokio::test]
    async fn test_put_writes_to_both() {
        let primary = MockCache::new();
        let fallback = MockCache::new();
        let cache = FallbackCache::new(primary, fallback);

        cache.put("key", Bytes::from("value"), Duration::from_secs(60)).await.unwrap();

        // Verify both caches have the value
        assert!(cache.primary().contains_key("key"));
        assert!(cache.fallback().contains_key("key"));
    }

    #[rstest]
    #[tokio::test]
    async fn test_put_fails_if_primary_fails() {
        let primary = MockCache::failing();
        let fallback = MockCache::new();
        let cache = FallbackCache::new(primary, fallback);

        let result = cache.put("key", Bytes::from("value"), Duration::from_secs(60)).await;
        assert!(result.is_err());
    }

    #[rstest]
    #[tokio::test]
    async fn test_put_fails_if_fallback_fails() {
        let primary = MockCache::new();
        let fallback = MockCache::failing();
        let cache = FallbackCache::new(primary, fallback);

        let result = cache.put("key", Bytes::from("value"), Duration::from_secs(60)).await;
        assert!(result.is_err());
    }

    #[rstest]
    #[tokio::test]
    async fn test_delete_removes_from_both() {
        let primary = MockCache::with_data("key", Bytes::from("value"));
        let fallback = MockCache::with_data("key", Bytes::from("value"));
        let cache = FallbackCache::new(primary, fallback);

        cache.delete("key").await.unwrap();

        // Verify both caches no longer have the value
        assert!(!cache.primary().contains_key("key"));
        assert!(!cache.fallback().contains_key("key"));
    }

    #[rstest]
    #[tokio::test]
    async fn test_delete_fails_if_primary_fails() {
        let primary = MockCache::failing();
        let fallback = MockCache::new();
        let cache = FallbackCache::new(primary, fallback);

        let result = cache.delete("key").await;
        assert!(result.is_err());
    }

    #[rstest]
    #[tokio::test]
    async fn test_delete_fails_if_fallback_fails() {
        let primary = MockCache::new();
        let fallback = MockCache::failing();
        let cache = FallbackCache::new(primary, fallback);

        let result = cache.delete("key").await;
        assert!(result.is_err());
    }

    #[rstest]
    #[tokio::test]
    async fn test_accessors() {
        let primary = MockCache::with_data("p", Bytes::from("primary"));
        let fallback = MockCache::with_data("f", Bytes::from("fallback"));
        let cache = FallbackCache::new(primary, fallback);

        // Test that accessors work
        assert!(cache.primary().contains_key("p"));
        assert!(cache.fallback().contains_key("f"));
    }

    #[rstest]
    fn test_debug_impl() {
        let primary = MockCache::new();
        let fallback = MockCache::new();
        let cache = FallbackCache::new(primary, fallback);

        let debug_str = format!("{:?}", cache);
        assert!(debug_str.contains("FallbackCache"));
    }
}
