//! RPC-specific cache with method-aware caching policies.

use std::{collections::HashMap, time::Duration};

use bytes::Bytes;
use derive_more::Debug;
use roxy_traits::{Cache, CacheError};

/// Cache policy for RPC methods.
#[derive(Debug, Clone)]
pub enum CachePolicy {
    /// Never cache this method.
    NoCache,
    /// Cache with a specific TTL.
    Ttl(Duration),
    /// Cache forever (until evicted by LRU).
    Immutable,
}

/// Very long TTL for immutable data (100 years).
const IMMUTABLE_TTL: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

/// RPC-aware cache wrapper that applies different caching policies per method.
#[derive(Debug)]
pub struct RpcCache<C> {
    /// The underlying cache implementation.
    inner: C,
    /// Method-specific caching policies.
    policies: HashMap<String, CachePolicy>,
    /// Default policy for methods not explicitly configured.
    default_policy: CachePolicy,
}

impl<C: Cache> RpcCache<C> {
    /// Create a new RPC cache with the given underlying cache.
    ///
    /// This initializes the cache with sensible default policies for common
    /// Ethereum JSON-RPC methods.
    #[must_use]
    pub fn new(inner: C) -> Self {
        let mut policies = HashMap::new();

        // Immutable data - cache forever
        policies.insert("eth_chainId".to_string(), CachePolicy::Immutable);
        policies.insert("eth_getBlockByHash".to_string(), CachePolicy::Immutable);
        policies.insert("eth_getTransactionByHash".to_string(), CachePolicy::Immutable);
        policies.insert("eth_getTransactionReceipt".to_string(), CachePolicy::Immutable);

        // Short TTL - frequently changing data
        policies.insert("eth_blockNumber".to_string(), CachePolicy::Ttl(Duration::from_secs(1)));
        policies.insert("eth_gasPrice".to_string(), CachePolicy::Ttl(Duration::from_secs(5)));

        // Never cache - state-changing or block-dependent
        policies.insert("eth_sendRawTransaction".to_string(), CachePolicy::NoCache);
        policies.insert("eth_call".to_string(), CachePolicy::NoCache);
        policies.insert("eth_estimateGas".to_string(), CachePolicy::NoCache);

        Self { inner, policies, default_policy: CachePolicy::NoCache }
    }

    /// Set a caching policy for a specific RPC method.
    #[must_use]
    pub fn with_policy(mut self, method: &str, policy: CachePolicy) -> Self {
        self.policies.insert(method.to_string(), policy);
        self
    }

    /// Set the default policy for methods not explicitly configured.
    #[must_use]
    pub const fn with_default_policy(mut self, policy: CachePolicy) -> Self {
        self.default_policy = policy;
        self
    }

    /// Get the caching policy for a method.
    pub fn get_policy(&self, method: &str) -> &CachePolicy {
        self.policies.get(method).unwrap_or(&self.default_policy)
    }

    /// Get cached response for an RPC request.
    ///
    /// Returns `None` if the method has a `NoCache` policy or if the response
    /// is not in the cache.
    pub async fn get_response(
        &self,
        method: &str,
        params: &[serde_json::Value],
    ) -> Result<Option<Bytes>, CacheError> {
        match self.get_policy(method) {
            CachePolicy::NoCache => Ok(None),
            CachePolicy::Ttl(_) | CachePolicy::Immutable => {
                let key = Self::cache_key(method, params);
                self.inner.get(&key).await
            }
        }
    }

    /// Cache an RPC response.
    ///
    /// Does nothing if the method has a `NoCache` policy.
    pub async fn put_response(
        &self,
        method: &str,
        params: &[serde_json::Value],
        response: Bytes,
    ) -> Result<(), CacheError> {
        let ttl = match self.get_policy(method) {
            CachePolicy::NoCache => return Ok(()),
            CachePolicy::Ttl(duration) => *duration,
            CachePolicy::Immutable => IMMUTABLE_TTL,
        };

        let key = Self::cache_key(method, params);
        self.inner.put(&key, response, ttl).await
    }

    /// Generate a cache key from method and params.
    ///
    /// The key format is `{method}:{params_json}` where params_json is the
    /// compact JSON representation of the params array.
    fn cache_key(method: &str, params: &[serde_json::Value]) -> String {
        let params_json = serde_json::to_string(params).unwrap_or_else(|_| "[]".to_string());
        format!("{}:{}", method, params_json)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::*;
    use crate::MemoryCache;

    /// Create a test RpcCache with a MemoryCache backend.
    fn test_cache() -> RpcCache<MemoryCache> {
        RpcCache::new(MemoryCache::new(100))
    }

    /// Test cache key generation produces expected format.
    #[rstest]
    #[case::no_params("eth_blockNumber", &[], "eth_blockNumber:[]")]
    #[case::single_param("eth_getBalance", &[json!("0x1234")], "eth_getBalance:[\"0x1234\"]")]
    #[case::multiple_params(
        "eth_call",
        &[json!({"to": "0x1234"}), json!("latest")],
        "eth_call:[{\"to\":\"0x1234\"},\"latest\"]"
    )]
    #[case::numeric_param("eth_getBlockByNumber", &[json!(123)], "eth_getBlockByNumber:[123]")]
    fn test_cache_key_format(
        #[case] method: &str,
        #[case] params: &[serde_json::Value],
        #[case] expected: &str,
    ) {
        let key = RpcCache::<MemoryCache>::cache_key(method, params);
        assert_eq!(key, expected);
    }

    /// Test default policies are correctly set.
    #[rstest]
    #[case::chain_id("eth_chainId", true)]
    #[case::block_by_hash("eth_getBlockByHash", true)]
    #[case::tx_by_hash("eth_getTransactionByHash", true)]
    #[case::tx_receipt("eth_getTransactionReceipt", true)]
    fn test_immutable_policies(#[case] method: &str, #[case] _is_immutable: bool) {
        let cache = test_cache();
        assert!(
            matches!(cache.get_policy(method), CachePolicy::Immutable),
            "Expected {} to have Immutable policy",
            method
        );
    }

    /// Test TTL policies are correctly set.
    #[rstest]
    #[case::block_number("eth_blockNumber", 1)]
    #[case::gas_price("eth_gasPrice", 5)]
    fn test_ttl_policies(#[case] method: &str, #[case] expected_secs: u64) {
        let cache = test_cache();
        match cache.get_policy(method) {
            CachePolicy::Ttl(duration) => {
                assert_eq!(
                    duration.as_secs(),
                    expected_secs,
                    "Expected {} to have TTL of {} seconds",
                    method,
                    expected_secs
                );
            }
            _ => panic!("Expected {} to have TTL policy", method),
        }
    }

    /// Test NoCache policies are correctly set.
    #[rstest]
    #[case::send_raw_tx("eth_sendRawTransaction")]
    #[case::eth_call("eth_call")]
    #[case::estimate_gas("eth_estimateGas")]
    fn test_no_cache_policies(#[case] method: &str) {
        let cache = test_cache();
        assert!(
            matches!(cache.get_policy(method), CachePolicy::NoCache),
            "Expected {} to have NoCache policy",
            method
        );
    }

    /// Test that unknown methods use default policy.
    #[rstest]
    #[case::unknown("eth_unknownMethod")]
    #[case::custom("custom_rpcMethod")]
    fn test_unknown_method_uses_default(#[case] method: &str) {
        let cache = test_cache();
        assert!(
            matches!(cache.get_policy(method), CachePolicy::NoCache),
            "Expected unknown method {} to use default NoCache policy",
            method
        );
    }

    /// Test with_policy overrides existing policy.
    #[test]
    fn test_with_policy_override() {
        let cache = test_cache().with_policy("eth_call", CachePolicy::Ttl(Duration::from_secs(10)));
        match cache.get_policy("eth_call") {
            CachePolicy::Ttl(duration) => {
                assert_eq!(duration.as_secs(), 10);
            }
            _ => panic!("Expected eth_call to have TTL policy after override"),
        }
    }

    /// Test with_default_policy changes default for unknown methods.
    #[test]
    fn test_with_default_policy() {
        let cache = test_cache().with_default_policy(CachePolicy::Ttl(Duration::from_secs(30)));
        match cache.get_policy("eth_unknownMethod") {
            CachePolicy::Ttl(duration) => {
                assert_eq!(duration.as_secs(), 30);
            }
            _ => panic!("Expected unknown method to use new default TTL policy"),
        }
    }

    /// Test get_response returns None for NoCache methods.
    #[tokio::test]
    async fn test_get_response_no_cache_method() {
        let cache = test_cache();
        let result = cache.get_response("eth_sendRawTransaction", &[json!("0x1234")]).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    /// Test put_response does nothing for NoCache methods.
    #[tokio::test]
    async fn test_put_response_no_cache_method() {
        let cache = test_cache();
        let response = Bytes::from_static(b"test response");

        // Put should succeed (but do nothing)
        let put_result =
            cache.put_response("eth_sendRawTransaction", &[json!("0x1234")], response).await;
        assert!(put_result.is_ok());

        // Get should still return None
        let get_result = cache.get_response("eth_sendRawTransaction", &[json!("0x1234")]).await;
        assert!(get_result.is_ok());
        assert!(get_result.unwrap().is_none());
    }

    /// Test caching works for cacheable methods.
    #[tokio::test]
    async fn test_cache_round_trip_ttl_method() {
        let cache = test_cache();
        let params = vec![json!("0x1234")];
        let response = Bytes::from_static(b"block number result");

        // Initially empty
        let get_result = cache.get_response("eth_blockNumber", &params).await;
        assert!(get_result.unwrap().is_none());

        // Put response
        cache.put_response("eth_blockNumber", &params, response.clone()).await.unwrap();

        // Should now return cached value
        let get_result = cache.get_response("eth_blockNumber", &params).await;
        assert_eq!(get_result.unwrap(), Some(response));
    }

    /// Test caching works for immutable methods.
    #[tokio::test]
    async fn test_cache_round_trip_immutable_method() {
        let cache = test_cache();
        let params = vec![json!("0xabc123"), json!(true)];
        let response = Bytes::from_static(b"block data");

        // Initially empty
        let get_result = cache.get_response("eth_getBlockByHash", &params).await;
        assert!(get_result.unwrap().is_none());

        // Put response
        cache.put_response("eth_getBlockByHash", &params, response.clone()).await.unwrap();

        // Should now return cached value
        let get_result = cache.get_response("eth_getBlockByHash", &params).await;
        assert_eq!(get_result.unwrap(), Some(response));
    }

    /// Test different params produce different cache keys.
    #[tokio::test]
    async fn test_different_params_different_keys() {
        let cache = test_cache();
        let params1 = vec![json!("0x1111")];
        let params2 = vec![json!("0x2222")];
        let response1 = Bytes::from_static(b"response1");
        let response2 = Bytes::from_static(b"response2");

        cache.put_response("eth_chainId", &params1, response1.clone()).await.unwrap();
        cache.put_response("eth_chainId", &params2, response2.clone()).await.unwrap();

        let get1 = cache.get_response("eth_chainId", &params1).await.unwrap();
        let get2 = cache.get_response("eth_chainId", &params2).await.unwrap();

        assert_eq!(get1, Some(response1));
        assert_eq!(get2, Some(response2));
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

    /// Test RpcCache Debug implementation.
    #[test]
    fn test_rpc_cache_debug() {
        let cache = test_cache();
        let debug_str = format!("{:?}", cache);
        assert!(debug_str.contains("RpcCache"));
        assert!(debug_str.contains("inner"));
        assert!(debug_str.contains("policies"));
    }
}
