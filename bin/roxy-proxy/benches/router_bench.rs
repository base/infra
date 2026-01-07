//! Benchmarks for method router
#![allow(missing_docs)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use roxy_rpc::{MethodRouter, RouteTarget};

fn bench_router_exact_match(c: &mut Criterion) {
    let mut router = MethodRouter::new();

    // Add 100 exact routes
    for i in 0..100 {
        router = router.route(&format!("method_{}", i), RouteTarget::Group(format!("group_{}", i)));
    }

    c.bench_function("router_exact_match", |b| {
        b.iter(|| {
            let _ = router.resolve(black_box("method_50"));
        })
    });
}

fn bench_router_prefix_match(c: &mut Criterion) {
    let router = MethodRouter::new()
        .route_prefix("eth_", RouteTarget::Group("eth".to_string()))
        .route_prefix("debug_", RouteTarget::Group("debug".to_string()))
        .route_prefix("net_", RouteTarget::Group("net".to_string()))
        .route_prefix("web3_", RouteTarget::Group("web3".to_string()));

    c.bench_function("router_prefix_match", |b| {
        b.iter(|| {
            let _ = router.resolve(black_box("eth_getBalance"));
        })
    });
}

fn bench_router_fallback(c: &mut Criterion) {
    let router = MethodRouter::new()
        .route_prefix("eth_", RouteTarget::Group("eth".to_string()))
        .fallback(RouteTarget::Default);

    c.bench_function("router_fallback", |b| {
        b.iter(|| {
            let _ = router.resolve(black_box("unknown_method"));
        })
    });
}

criterion_group!(
    benches,
    bench_router_exact_match,
    bench_router_prefix_match,
    bench_router_fallback
);
criterion_main!(benches);
