//! Integration tests for cache layer
//!
//! These tests verify the caching behavior for RPC responses.

use std::time::Duration;

use bytes::Bytes;
use roxy_cache::{CachePolicy, MemoryCache, RpcCache};
use roxy_traits::Cache;
use serde_json::json;

// =============================================================================
// Memory Cache Tests
// =============================================================================

/// Test basic cache hit/miss flow.
#[tokio::test]
async fn test_cache_hit_miss() {
    let cache = MemoryCache::new(100);

    let key = "test_key";
    let value = Bytes::from("test_value");

    // Initially miss
    let result = cache.get(key).await.unwrap();
    assert!(result.is_none(), "should miss for unknown key");

    // Store value
    cache.put(key, value.clone(), Duration::from_secs(60)).await.unwrap();

    // Now should hit
    let result = cache.get(key).await.unwrap();
    assert_eq!(result, Some(value.clone()), "should hit after put");

    // Different key should still miss
    let result = cache.get("other_key").await.unwrap();
    assert!(result.is_none(), "should miss for different key");
}

/// Test cache TTL expiration.
#[tokio::test]
async fn test_cache_ttl() {
    let cache = MemoryCache::new(100);

    let key = "expiring_key";
    let value = Bytes::from("expiring_value");

    // Store with very short TTL
    cache.put(key, value.clone(), Duration::from_millis(10)).await.unwrap();

    // Should hit immediately
    let result = cache.get(key).await.unwrap();
    assert!(result.is_some(), "should hit before expiration");

    // Wait for expiration
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Should miss after expiration
    let result = cache.get(key).await.unwrap();
    assert!(result.is_none(), "should miss after expiration");
}

/// Test cache overwrite behavior.
#[tokio::test]
async fn test_cache_overwrite() {
    let cache = MemoryCache::new(100);

    let key = "overwrite_key";
    let value1 = Bytes::from("value1");
    let value2 = Bytes::from("value2");

    // Store first value
    cache.put(key, value1.clone(), Duration::from_secs(60)).await.unwrap();

    // Overwrite with second value
    cache.put(key, value2.clone(), Duration::from_secs(60)).await.unwrap();

    // Should return second value
    let result = cache.get(key).await.unwrap();
    assert_eq!(result, Some(value2));
}

/// Test cache delete.
#[tokio::test]
async fn test_cache_delete() {
    let cache = MemoryCache::new(100);

    let key = "delete_key";
    let value = Bytes::from("delete_value");

    // Store value
    cache.put(key, value.clone(), Duration::from_secs(60)).await.unwrap();

    // Verify it exists
    assert!(cache.get(key).await.unwrap().is_some());

    // Delete
    cache.delete(key).await.unwrap();

    // Should no longer exist
    assert!(cache.get(key).await.unwrap().is_none());
}

/// Test LRU eviction.
#[tokio::test]
async fn test_cache_lru_eviction() {
    let cache = MemoryCache::new(3);

    // Fill cache to capacity
    cache.put("key1", Bytes::from("value1"), Duration::from_secs(60)).await.unwrap();
    cache.put("key2", Bytes::from("value2"), Duration::from_secs(60)).await.unwrap();
    cache.put("key3", Bytes::from("value3"), Duration::from_secs(60)).await.unwrap();

    // Add one more (should evict key1)
    cache.put("key4", Bytes::from("value4"), Duration::from_secs(60)).await.unwrap();

    // key1 should be evicted (LRU)
    assert!(cache.get("key1").await.unwrap().is_none());

    // Others should still exist
    assert!(cache.get("key2").await.unwrap().is_some());
    assert!(cache.get("key3").await.unwrap().is_some());
    assert!(cache.get("key4").await.unwrap().is_some());
}

/// Test cache with various value sizes.
#[tokio::test]
async fn test_cache_value_sizes() {
    let cache = MemoryCache::new(100);

    // Empty value
    cache.put("empty", Bytes::new(), Duration::from_secs(60)).await.unwrap();
    assert_eq!(cache.get("empty").await.unwrap(), Some(Bytes::new()));

    // Small value
    cache.put("small", Bytes::from("small"), Duration::from_secs(60)).await.unwrap();
    assert_eq!(cache.get("small").await.unwrap(), Some(Bytes::from("small")));

    // Large value (10KB)
    let large = Bytes::from(vec![0u8; 10000]);
    cache.put("large", large.clone(), Duration::from_secs(60)).await.unwrap();
    assert_eq!(cache.get("large").await.unwrap(), Some(large));
}

// =============================================================================
// RPC Cache Tests
// =============================================================================

/// Test RPC cache with different policies.
#[tokio::test]
async fn test_rpc_cache_policies() {
    let memory_cache = MemoryCache::new(100);
    let rpc_cache = RpcCache::new(memory_cache);

    // Test immutable policy (eth_chainId)
    assert!(matches!(rpc_cache.get_policy("eth_chainId"), CachePolicy::Immutable));

    // Test TTL policy (eth_blockNumber)
    assert!(matches!(rpc_cache.get_policy("eth_blockNumber"), CachePolicy::Ttl(_)));

    // Test no-cache policy (eth_sendRawTransaction)
    assert!(matches!(rpc_cache.get_policy("eth_sendRawTransaction"), CachePolicy::NoCache));

    // Test default policy for unknown method
    assert!(matches!(rpc_cache.get_policy("unknown_method"), CachePolicy::NoCache));
}

/// Test RPC cache get/put with cacheable method.
#[tokio::test]
async fn test_rpc_cache_get_put_cacheable() {
    let memory_cache = MemoryCache::new(100);
    let rpc_cache = RpcCache::new(memory_cache);

    let method = "eth_chainId";
    let params = vec![];
    let response = Bytes::from(r#"{"jsonrpc":"2.0","result":"0x1","id":1}"#);

    // Initially empty
    let result = rpc_cache.get_response(method, &params).await.unwrap();
    assert!(result.is_none());

    // Put response
    rpc_cache.put_response(method, &params, response.clone()).await.unwrap();

    // Should now return cached value
    let result = rpc_cache.get_response(method, &params).await.unwrap();
    assert_eq!(result, Some(response));
}

/// Test RPC cache with non-cacheable method.
#[tokio::test]
async fn test_rpc_cache_non_cacheable() {
    let memory_cache = MemoryCache::new(100);
    let rpc_cache = RpcCache::new(memory_cache);

    let method = "eth_sendRawTransaction";
    let params = vec![json!("0x...")];
    let response = Bytes::from(r#"{"jsonrpc":"2.0","result":"0xhash","id":1}"#);

    // Put should succeed but not actually cache
    rpc_cache.put_response(method, &params, response.clone()).await.unwrap();

    // Get should still return None (NoCache policy)
    let result = rpc_cache.get_response(method, &params).await.unwrap();
    assert!(result.is_none());
}

/// Test RPC cache with custom policy.
#[tokio::test]
async fn test_rpc_cache_custom_policy() {
    let memory_cache = MemoryCache::new(100);
    let rpc_cache = RpcCache::new(memory_cache)
        .with_policy("eth_call", CachePolicy::Ttl(Duration::from_secs(10)))
        .with_default_policy(CachePolicy::Ttl(Duration::from_secs(5)));

    // eth_call now has TTL policy
    assert!(matches!(rpc_cache.get_policy("eth_call"), CachePolicy::Ttl(_)));

    // Unknown method uses new default
    match rpc_cache.get_policy("custom_method") {
        CachePolicy::Ttl(duration) => {
            assert_eq!(duration.as_secs(), 5);
        }
        _ => panic!("expected TTL policy"),
    }
}

/// Test RPC cache with different params.
#[tokio::test]
async fn test_rpc_cache_different_params() {
    let memory_cache = MemoryCache::new(100);
    let rpc_cache = RpcCache::new(memory_cache);

    let method = "eth_chainId";
    let params1 = vec![json!("0x1111")];
    let params2 = vec![json!("0x2222")];
    let response1 = Bytes::from("response1");
    let response2 = Bytes::from("response2");

    // Store different responses for different params
    rpc_cache.put_response(method, &params1, response1.clone()).await.unwrap();
    rpc_cache.put_response(method, &params2, response2.clone()).await.unwrap();

    // Should get correct response for each params
    let result1 = rpc_cache.get_response(method, &params1).await.unwrap();
    let result2 = rpc_cache.get_response(method, &params2).await.unwrap();

    assert_eq!(result1, Some(response1));
    assert_eq!(result2, Some(response2));
}

/// Test RPC cache TTL expiration.
#[tokio::test]
async fn test_rpc_cache_ttl_expiration() {
    let memory_cache = MemoryCache::new(100);
    let rpc_cache = RpcCache::new(memory_cache)
        .with_policy("test_method", CachePolicy::Ttl(Duration::from_millis(10)));

    let method = "test_method";
    let params = vec![];
    let response = Bytes::from("test_response");

    // Store response
    rpc_cache.put_response(method, &params, response.clone()).await.unwrap();

    // Should hit immediately
    assert!(rpc_cache.get_response(method, &params).await.unwrap().is_some());

    // Wait for expiration
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Should miss after expiration
    assert!(rpc_cache.get_response(method, &params).await.unwrap().is_none());
}

// =============================================================================
// Cache Policy Tests
// =============================================================================

/// Test default cache policies for common methods.
#[test]
fn test_default_cache_policies() {
    let memory_cache = MemoryCache::new(100);
    let rpc_cache = RpcCache::new(memory_cache);

    // Immutable methods (cache forever)
    let immutable_methods = [
        "eth_chainId",
        "eth_getBlockByHash",
        "eth_getTransactionByHash",
        "eth_getTransactionReceipt",
    ];

    for method in &immutable_methods {
        assert!(
            matches!(rpc_cache.get_policy(method), CachePolicy::Immutable),
            "expected {} to be immutable",
            method
        );
    }

    // TTL methods (cache with expiration)
    let ttl_methods = ["eth_blockNumber", "eth_gasPrice"];

    for method in &ttl_methods {
        assert!(
            matches!(rpc_cache.get_policy(method), CachePolicy::Ttl(_)),
            "expected {} to have TTL",
            method
        );
    }

    // NoCache methods (never cache)
    let no_cache_methods = ["eth_sendRawTransaction", "eth_call", "eth_estimateGas"];

    for method in &no_cache_methods {
        assert!(
            matches!(rpc_cache.get_policy(method), CachePolicy::NoCache),
            "expected {} to be no-cache",
            method
        );
    }
}

/// Test CachePolicy Debug implementation.
#[test]
fn test_cache_policy_debug() {
    let no_cache = CachePolicy::NoCache;
    let ttl = CachePolicy::Ttl(Duration::from_secs(10));
    let immutable = CachePolicy::Immutable;

    assert!(format!("{:?}", no_cache).contains("NoCache"));
    assert!(format!("{:?}", ttl).contains("Ttl"));
    assert!(format!("{:?}", immutable).contains("Immutable"));
}

// =============================================================================
// Cache Integration Scenarios
// =============================================================================

/// Test simulated RPC caching workflow.
#[tokio::test]
async fn test_cache_workflow_simulation() {
    let memory_cache = MemoryCache::new(1000);
    let rpc_cache = RpcCache::new(memory_cache);

    // Simulate eth_chainId request
    let chain_id_params = vec![];
    let chain_id_key = "eth_chainId";

    // First request - cache miss
    let result = rpc_cache.get_response(chain_id_key, &chain_id_params).await.unwrap();
    assert!(result.is_none());

    // "Fetch" from backend and cache
    let chain_id_response = Bytes::from(r#"{"jsonrpc":"2.0","result":"0x1","id":1}"#);
    rpc_cache
        .put_response(chain_id_key, &chain_id_params, chain_id_response.clone())
        .await
        .unwrap();

    // Second request - cache hit
    let result = rpc_cache.get_response(chain_id_key, &chain_id_params).await.unwrap();
    assert_eq!(result, Some(chain_id_response));

    // Simulate eth_getBalance request (no cache by default)
    let balance_params = vec![json!("0x1234"), json!("latest")];
    let balance_key = "eth_getBalance";

    // Try to cache
    let balance_response = Bytes::from(r#"{"jsonrpc":"2.0","result":"0x1000","id":1}"#);
    rpc_cache.put_response(balance_key, &balance_params, balance_response.clone()).await.unwrap();

    // Should still miss (unknown method uses NoCache default)
    let result = rpc_cache.get_response(balance_key, &balance_params).await.unwrap();
    assert!(result.is_none());
}

/// Test multiple concurrent cache operations.
#[tokio::test]
async fn test_concurrent_cache_operations() {
    use std::sync::Arc;

    let memory_cache = Arc::new(MemoryCache::new(100));

    // Spawn multiple tasks accessing the cache
    let mut handles = Vec::new();

    for i in 0..10 {
        let cache = Arc::clone(&memory_cache);
        handles.push(tokio::spawn(async move {
            let key = format!("key_{}", i);
            let value = Bytes::from(format!("value_{}", i));

            cache.put(&key, value.clone(), Duration::from_secs(60)).await.unwrap();

            let result = cache.get(&key).await.unwrap();
            assert_eq!(result, Some(value));
        }));
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all keys are still accessible
    for i in 0..10 {
        let key = format!("key_{}", i);
        let expected = Some(Bytes::from(format!("value_{}", i)));
        assert_eq!(memory_cache.get(&key).await.unwrap(), expected);
    }
}

/// Test cache with JSON-RPC response data.
#[tokio::test]
async fn test_cache_json_rpc_responses() {
    let memory_cache = MemoryCache::new(100);
    let rpc_cache = RpcCache::new(memory_cache);

    // Block number response
    let block_number_response = Bytes::from(r#"{"jsonrpc":"2.0","result":"0x10d4f","id":1}"#);
    rpc_cache.put_response("eth_blockNumber", &[], block_number_response.clone()).await.unwrap();

    // Block by hash response
    let block_by_hash_params = vec![json!("0xabc123"), json!(true)];
    let block_response = Bytes::from(
        r#"{"jsonrpc":"2.0","result":{"number":"0x10d4f","hash":"0xabc123","transactions":[]},"id":1}"#,
    );
    rpc_cache
        .put_response("eth_getBlockByHash", &block_by_hash_params, block_response.clone())
        .await
        .unwrap();

    // Verify both are cached correctly
    let result = rpc_cache.get_response("eth_blockNumber", &[]).await.unwrap();
    assert_eq!(result, Some(block_number_response));

    let result = rpc_cache.get_response("eth_getBlockByHash", &block_by_hash_params).await.unwrap();
    assert_eq!(result, Some(block_response));
}
