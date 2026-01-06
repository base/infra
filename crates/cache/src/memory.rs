//! In-memory LRU cache implementation.

use bytes::Bytes;
use lru::LruCache;
use roxy_traits::{Cache, CacheError};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1000).unwrap());
        Self {
            cache: Mutex::new(LruCache::new(cap)),
        }
    }
}

impl std::fmt::Debug for MemoryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryCache").finish_non_exhaustive()
    }
}

impl Cache for MemoryCache {
    async fn get(&self, key: &str) -> Result<Option<Bytes>, CacheError> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|e| CacheError(format!("lock poisoned: {}", e)))?;

        if let Some(entry) = cache.get(key) {
            if entry.expires_at > Instant::now() {
                return Ok(Some(entry.value.clone()));
            }
            cache.pop(key);
        }

        Ok(None)
    }

    async fn put(&self, key: &str, value: Bytes, ttl: Duration) -> Result<(), CacheError> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|e| CacheError(format!("lock poisoned: {}", e)))?;

        let entry = CacheEntry {
            value,
            expires_at: Instant::now() + ttl,
        };

        cache.put(key.to_string(), entry);
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|e| CacheError(format!("lock poisoned: {}", e)))?;

        cache.pop(key);
        Ok(())
    }
}
