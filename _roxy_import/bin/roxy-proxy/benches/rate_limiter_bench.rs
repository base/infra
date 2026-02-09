//! Benchmarks for rate limiter
#![allow(missing_docs)]

use std::time::Duration;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use roxy_rpc::{RateLimiterConfig, SlidingWindowRateLimiter};
use roxy_traits::RateLimiter;

fn bench_rate_limiter_check(c: &mut Criterion) {
    let config =
        RateLimiterConfig { requests_per_window: 1000, window_duration: Duration::from_secs(1) };
    let limiter = SlidingWindowRateLimiter::new(config);

    c.bench_function("rate_limiter_check_allowed", |b| {
        b.iter(|| {
            let _ = limiter.check(black_box("client_1"));
        })
    });
}

fn bench_rate_limiter_many_clients(c: &mut Criterion) {
    let config =
        RateLimiterConfig { requests_per_window: 1000, window_duration: Duration::from_secs(1) };
    let limiter = SlidingWindowRateLimiter::new(config);

    // Pre-populate with many clients
    for i in 0..1000 {
        let _ = limiter.check_and_record(&format!("client_{}", i));
    }

    c.bench_function("rate_limiter_many_clients", |b| {
        let mut i = 0;
        b.iter(|| {
            let client = format!("client_{}", i % 1000);
            i += 1;
            let _ = limiter.check(black_box(&client));
        })
    });
}

criterion_group!(benches, bench_rate_limiter_check, bench_rate_limiter_many_clients);
criterion_main!(benches);
