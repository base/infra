//! Benchmarks for cache layer
#![allow(missing_docs)]

use std::time::Duration;

use bytes::Bytes;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use roxy_cache::MemoryCache;
use roxy_traits::Cache;

fn bench_cache_get(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let cache = MemoryCache::new(10000);

    // Pre-populate cache
    rt.block_on(async {
        for i in 0..1000 {
            let key = format!("key_{}", i);
            let value = Bytes::from(format!("value_{}", i));
            cache.put(&key, value, Duration::from_secs(3600)).await.unwrap();
        }
    });

    c.bench_function("cache_get_hit", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = cache.get(black_box("key_500")).await;
        })
    });

    c.bench_function("cache_get_miss", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = cache.get(black_box("nonexistent")).await;
        })
    });
}

fn bench_cache_put(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("cache_put", |b| {
        let cache = MemoryCache::new(10000);
        let value = Bytes::from("test_value");
        let mut i = 0u64;

        b.to_async(&rt).iter(|| {
            let key = format!("key_{}", i);
            i += 1;
            let cache = &cache;
            let value = value.clone();
            async move {
                cache.put(&key, value, Duration::from_secs(3600)).await.unwrap();
            }
        })
    });
}

criterion_group!(benches, bench_cache_get, bench_cache_put);
criterion_main!(benches);
