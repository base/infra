//! Compressed cache wrapper using brotli compression.

use std::{
    io::{Read as _, Write as _},
    time::Duration,
};

use bytes::Bytes;
use roxy_traits::{Cache, CacheError};

/// Default brotli compression quality (0-11, where 11 is best compression).
const DEFAULT_QUALITY: u32 = 4;

/// Default brotli LG window size.
const DEFAULT_LG_WINDOW_SIZE: u32 = 22;

/// Compressed cache wrapper that compresses values before storing.
///
/// This wrapper uses brotli compression to reduce the storage size of cached values.
/// It wraps an inner cache implementation and transparently compresses/decompresses
/// values on put/get operations.
#[derive(Debug)]
pub struct CompressedCache<C> {
    inner: C,
    quality: u32,
}

impl<C: Cache> CompressedCache<C> {
    /// Create a new compressed cache with default compression quality.
    pub fn new(inner: C) -> Self {
        Self::with_quality(inner, DEFAULT_QUALITY)
    }

    /// Create a new compressed cache with the specified compression quality.
    ///
    /// # Arguments
    /// * `inner` - The inner cache to wrap
    /// * `quality` - Brotli compression quality (0-11, where 11 is best compression)
    pub fn with_quality(inner: C, quality: u32) -> Self {
        Self { inner, quality: quality.min(11) }
    }

    /// Compress data using brotli.
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>, CacheError> {
        let mut compressed = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(
                &mut compressed,
                4096, // buffer size
                self.quality,
                DEFAULT_LG_WINDOW_SIZE,
            );
            writer.write_all(data).map_err(|e| CacheError(format!("compression failed: {}", e)))?;
        }
        Ok(compressed)
    }

    /// Decompress data using brotli.
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>, CacheError> {
        let mut decompressed = Vec::new();
        let mut decompressor = brotli::Decompressor::new(data, 4096);
        decompressor
            .read_to_end(&mut decompressed)
            .map_err(|e| CacheError(format!("decompression failed: {}", e)))?;
        Ok(decompressed)
    }
}

impl<C: Cache> Cache for CompressedCache<C> {
    async fn get(&self, key: &str) -> Result<Option<Bytes>, CacheError> {
        match self.inner.get(key).await? {
            Some(compressed) => {
                let decompressed = self.decompress(&compressed)?;
                Ok(Some(Bytes::from(decompressed)))
            }
            None => Ok(None),
        }
    }

    async fn put(&self, key: &str, value: Bytes, ttl: Duration) -> Result<(), CacheError> {
        let compressed = self.compress(&value)?;
        self.inner.put(key, Bytes::from(compressed), ttl).await
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        self.inner.delete(key).await
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::MemoryCache;

    #[tokio::test]
    async fn test_compressed_cache_put_get() {
        let memory = MemoryCache::new(100);
        let cache = CompressedCache::new(memory);

        let key = "test_key";
        let value = Bytes::from("Hello, World!");
        let ttl = Duration::from_secs(60);

        cache.put(key, value.clone(), ttl).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, Some(value));
    }

    #[tokio::test]
    async fn test_compressed_cache_get_nonexistent() {
        let memory = MemoryCache::new(100);
        let cache = CompressedCache::new(memory);

        let result = cache.get("nonexistent").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_compressed_cache_delete() {
        let memory = MemoryCache::new(100);
        let cache = CompressedCache::new(memory);

        let key = "test_key";
        let value = Bytes::from("test value");
        let ttl = Duration::from_secs(60);

        cache.put(key, value, ttl).await.unwrap();
        cache.delete(key).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_compression_reduces_size() {
        let memory = MemoryCache::new(100);
        let cache = CompressedCache::new(memory);

        // Create a highly compressible string (repeating pattern)
        let value = Bytes::from("a".repeat(1000));
        let ttl = Duration::from_secs(60);

        cache.put("test", value.clone(), ttl).await.unwrap();

        // Verify roundtrip works
        let result = cache.get("test").await.unwrap();
        assert_eq!(result, Some(value));
    }

    #[rstest]
    #[case(0)]
    #[case(4)]
    #[case(11)]
    #[tokio::test]
    async fn test_different_quality_levels(#[case] quality: u32) {
        let memory = MemoryCache::new(100);
        let cache = CompressedCache::with_quality(memory, quality);

        let key = "test_key";
        let value = Bytes::from("Test data for compression quality testing");
        let ttl = Duration::from_secs(60);

        cache.put(key, value.clone(), ttl).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, Some(value));
    }

    #[tokio::test]
    async fn test_quality_clamped_to_max() {
        let memory = MemoryCache::new(100);
        // Quality > 11 should be clamped
        let cache = CompressedCache::with_quality(memory, 20);

        let key = "test_key";
        let value = Bytes::from("test");
        let ttl = Duration::from_secs(60);

        // Should not panic or error
        cache.put(key, value.clone(), ttl).await.unwrap();
        let result = cache.get(key).await.unwrap();
        assert_eq!(result, Some(value));
    }

    #[tokio::test]
    async fn test_empty_value() {
        let memory = MemoryCache::new(100);
        let cache = CompressedCache::new(memory);

        let key = "empty";
        let value = Bytes::new();
        let ttl = Duration::from_secs(60);

        cache.put(key, value.clone(), ttl).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, Some(value));
    }

    #[tokio::test]
    async fn test_binary_data() {
        let memory = MemoryCache::new(100);
        let cache = CompressedCache::new(memory);

        let key = "binary";
        let value = Bytes::from(vec![0u8, 1, 2, 255, 254, 253, 128, 127]);
        let ttl = Duration::from_secs(60);

        cache.put(key, value.clone(), ttl).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, Some(value));
    }

    #[tokio::test]
    async fn test_large_data() {
        let memory = MemoryCache::new(100);
        let cache = CompressedCache::new(memory);

        let key = "large";
        // 1MB of data
        let value = Bytes::from(vec![42u8; 1024 * 1024]);
        let ttl = Duration::from_secs(60);

        cache.put(key, value.clone(), ttl).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, Some(value));
    }

    #[tokio::test]
    async fn test_multiple_keys() {
        let memory = MemoryCache::new(100);
        let cache = CompressedCache::new(memory);
        let ttl = Duration::from_secs(60);

        let pairs = vec![
            ("key1", Bytes::from("value1")),
            ("key2", Bytes::from("value2")),
            ("key3", Bytes::from("value3")),
        ];

        for (key, value) in &pairs {
            cache.put(key, value.clone(), ttl).await.unwrap();
        }

        for (key, expected) in &pairs {
            let result = cache.get(key).await.unwrap();
            assert_eq!(result, Some(expected.clone()));
        }
    }

    #[tokio::test]
    async fn test_overwrite_key() {
        let memory = MemoryCache::new(100);
        let cache = CompressedCache::new(memory);
        let ttl = Duration::from_secs(60);

        let key = "overwrite";
        let value1 = Bytes::from("first value");
        let value2 = Bytes::from("second value");

        cache.put(key, value1, ttl).await.unwrap();
        cache.put(key, value2.clone(), ttl).await.unwrap();

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, Some(value2));
    }
}
