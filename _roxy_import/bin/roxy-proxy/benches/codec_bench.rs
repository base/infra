//! Benchmarks for RPC codec
#![allow(missing_docs)]

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use roxy_rpc::{ParsedResponse, RpcCodec};
use roxy_traits::DefaultCodecConfig;

fn bench_decode_single(c: &mut Criterion) {
    let codec = RpcCodec::default();
    let request = br#"{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}"#;

    c.bench_function("decode_single_request", |b| {
        b.iter(|| {
            let _ = codec.decode(black_box(request));
        })
    });
}

fn bench_decode_batch(c: &mut Criterion) {
    let codec = RpcCodec::default();

    let mut group = c.benchmark_group("decode_batch");

    for size in [1, 5, 10, 25, 50, 100].iter() {
        let batch = create_batch(*size);
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &batch, |b, batch| {
            b.iter(|| {
                let _ = codec.decode(black_box(batch.as_bytes()));
            });
        });
    }

    group.finish();
}

fn bench_encode_response(c: &mut Criterion) {
    let codec = RpcCodec::default();
    let response = ParsedResponse::success(
        serde_json::Value::Number(1.into()),
        serde_json::value::RawValue::from_string("\"0x1234\"".to_string()).unwrap(),
    );

    c.bench_function("encode_single_response", |b| {
        b.iter(|| {
            let _ = codec.encode_single_response(black_box(&response));
        })
    });
}

fn bench_nesting_depth_validation(c: &mut Criterion) {
    let codec = RpcCodec::new(DefaultCodecConfig::new().with_max_depth(64));

    let mut group = c.benchmark_group("nesting_depth");

    for depth in [5, 10, 20, 32, 64].iter() {
        let nested = create_nested_json(*depth);
        group.bench_with_input(BenchmarkId::from_parameter(depth), &nested, |b, nested| {
            b.iter(|| {
                let _ = codec.decode(black_box(nested.as_bytes()));
            });
        });
    }

    group.finish();
}

fn create_batch(size: usize) -> String {
    let items: Vec<String> = (0..size)
        .map(|i| {
            format!(r#"{{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":{}}}"#, i)
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn create_nested_json(depth: usize) -> String {
    let mut s = String::from(r#"{"jsonrpc":"2.0","method":"test","params":"#);
    for _ in 0..depth {
        s.push('[');
    }
    for _ in 0..depth {
        s.push(']');
    }
    s.push_str(r#","id":1}"#);
    s
}

criterion_group!(
    benches,
    bench_decode_single,
    bench_decode_batch,
    bench_encode_response,
    bench_nesting_depth_validation,
);
criterion_main!(benches);
