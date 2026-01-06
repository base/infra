//! Integration tests for Roxy RPC proxy
//!
//! These tests verify that the various components of the Roxy RPC proxy
//! work together correctly.

use std::time::Duration;

use roxy_rpc::{
    MethodBlocklist, MethodRouter, ParsedRequest, ParsedRequestPacket, RouteTarget, RpcCodec,
    SlidingWindowRateLimiter, ValidationResult, ValidatorChain,
};
use roxy_traits::{DefaultCodecConfig, RateLimitResult, RateLimiter};
use serde_json::json;

/// Helper to create a test request with the given method.
fn make_request(method: &str) -> ParsedRequest {
    let params = serde_json::to_string(&json!([])).unwrap();
    let raw_params =
        serde_json::value::RawValue::from_string(params).expect("valid raw value for params");
    ParsedRequest {
        jsonrpc: "2.0".to_string(),
        method: method.to_string(),
        params: Some(raw_params),
        id: Some(serde_json::Value::Number(1.into())),
    }
}

// =============================================================================
// Request Flow Tests
// =============================================================================

/// Test that a basic request can be parsed and validated through the system.
#[test]
fn test_basic_request_flow() {
    // 1. Decode the request using the codec
    let codec = RpcCodec::default();
    let request_bytes = br#"{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}"#;

    let packet = codec.decode(request_bytes).expect("should decode valid request");

    // 2. Verify it was parsed correctly
    assert!(!packet.is_batch());
    assert_eq!(packet.len(), 1);

    match packet {
        ParsedRequestPacket::Single(ref req) => {
            assert_eq!(req.method(), "eth_blockNumber");
        }
        _ => panic!("expected single request"),
    }

    // 3. Route the request
    let router = MethodRouter::new()
        .route_prefix("eth_", RouteTarget::group("ethereum"))
        .fallback(RouteTarget::group("default"));

    for method in packet.methods() {
        let target = router.resolve(method);
        assert_eq!(target.group_name(), Some("ethereum"));
    }

    // 4. Validate the request
    let validator_chain = ValidatorChain::new();
    match packet {
        ParsedRequestPacket::Single(req) => {
            let result = validator_chain.validate(req);
            assert!(result.is_valid());
        }
        _ => {}
    }
}

/// Test the full request flow with validation.
#[test]
fn test_request_flow_with_validation() {
    let codec = RpcCodec::default();
    let request_bytes =
        br#"{"jsonrpc":"2.0","method":"eth_getBalance","params":["0x1234","latest"],"id":1}"#;

    let packet = codec.decode(request_bytes).expect("should decode valid request");

    // Create validator chain with method blocklist
    let blocklist = MethodBlocklist::new(vec!["eth_sendTransaction".to_string()]);
    let validator_chain = ValidatorChain::new().with(blocklist);

    // eth_getBalance should pass validation
    match packet {
        ParsedRequestPacket::Single(req) => {
            let result = validator_chain.validate(req);
            assert!(result.is_valid(), "eth_getBalance should be allowed");
        }
        _ => panic!("expected single request"),
    }
}

// =============================================================================
// Blocked Method Tests
// =============================================================================

/// Test that blocked methods are rejected by the validator chain.
#[test]
fn test_blocked_method_rejected() {
    let blocked_methods = vec![
        "eth_sendTransaction".to_string(),
        "debug_traceTransaction".to_string(),
        "admin_addPeer".to_string(),
    ];
    let blocklist = MethodBlocklist::new(blocked_methods);
    let validator_chain = ValidatorChain::new().with(blocklist);

    // Test blocked method
    let request = make_request("eth_sendTransaction");
    let result = validator_chain.validate(request);
    assert!(result.is_invalid(), "eth_sendTransaction should be blocked");

    // Verify error code
    if let ValidationResult::Invalid(error) = result {
        assert_eq!(error.code, -32601, "should be method not allowed error code");
        assert!(error.message.contains("eth_sendTransaction"));
    }
}

/// Test that non-blocked methods pass validation.
#[test]
fn test_non_blocked_method_allowed() {
    let blocked_methods = vec!["eth_sendTransaction".to_string()];
    let blocklist = MethodBlocklist::new(blocked_methods);
    let validator_chain = ValidatorChain::new().with(blocklist);

    // Test allowed method
    let request = make_request("eth_blockNumber");
    let result = validator_chain.validate(request);
    assert!(result.is_valid(), "eth_blockNumber should be allowed");
}

/// Test blocking methods via router.
#[test]
fn test_router_blocks_methods() {
    let router = MethodRouter::new()
        .route("admin_addPeer", RouteTarget::Block)
        .route_prefix("debug_", RouteTarget::Block)
        .fallback(RouteTarget::group("default"));

    assert!(router.is_blocked("admin_addPeer"));
    assert!(router.is_blocked("debug_traceCall"));
    assert!(router.is_blocked("debug_traceTransaction"));
    assert!(!router.is_blocked("eth_call"));
    assert!(!router.is_blocked("eth_blockNumber"));
}

// =============================================================================
// Batch Request Tests
// =============================================================================

/// Test batch request handling.
#[test]
fn test_batch_request_handling() {
    let codec = RpcCodec::default();
    let batch_request = br#"[
        {"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1},
        {"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":2},
        {"jsonrpc":"2.0","method":"eth_gasPrice","params":[],"id":3}
    ]"#;

    let packet = codec.decode(batch_request).expect("should decode valid batch request");

    assert!(packet.is_batch());
    assert_eq!(packet.len(), 3);

    // Verify all methods are parsed correctly
    let methods: Vec<_> = packet.methods().collect();
    assert_eq!(methods, vec!["eth_blockNumber", "eth_chainId", "eth_gasPrice"]);

    // Route each request
    let router = MethodRouter::new()
        .route_prefix("eth_", RouteTarget::group("ethereum"))
        .fallback(RouteTarget::group("default"));

    for method in packet.methods() {
        let target = router.resolve(method);
        assert_eq!(target.group_name(), Some("ethereum"));
    }
}

/// Test that batch requests can be validated individually.
#[test]
fn test_batch_request_validation() {
    let codec = RpcCodec::default();
    let batch_request = br#"[
        {"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1},
        {"jsonrpc":"2.0","method":"eth_sendTransaction","params":[],"id":2}
    ]"#;

    let packet = codec.decode(batch_request).expect("should decode valid batch request");

    let blocklist = MethodBlocklist::new(vec!["eth_sendTransaction".to_string()]);
    let validator_chain = ValidatorChain::new().with(blocklist);

    // Validate each request in the batch
    let mut results = Vec::new();
    match packet {
        ParsedRequestPacket::Batch(requests) => {
            for req in requests {
                let cloned_req = make_request(req.method());
                results.push(validator_chain.validate(cloned_req));
            }
        }
        _ => panic!("expected batch request"),
    }

    assert!(results[0].is_valid(), "eth_blockNumber should be valid");
    assert!(results[1].is_invalid(), "eth_sendTransaction should be blocked");
}

/// Test batch size limits.
#[test]
fn test_batch_size_limits() {
    let config = DefaultCodecConfig::new().with_max_batch_size(2);
    let codec = RpcCodec::new(config);

    // Batch within limit
    let small_batch = br#"[
        {"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1},
        {"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":2}
    ]"#;
    assert!(codec.decode(small_batch).is_ok());

    // Batch exceeds limit
    let large_batch = br#"[
        {"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1},
        {"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":2},
        {"jsonrpc":"2.0","method":"eth_gasPrice","params":[],"id":3}
    ]"#;
    let result = codec.decode(large_batch);
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("exceeds maximum"));
}

/// Test that batches can be disallowed.
#[test]
fn test_batch_not_allowed() {
    let config = DefaultCodecConfig::new().with_allow_batch(false);
    let codec = RpcCodec::new(config);

    let batch_request = br#"[
        {"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}
    ]"#;

    let result = codec.decode(batch_request);
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("not allowed"));
}

// =============================================================================
// Rate Limiting Tests
// =============================================================================

/// Test basic rate limiting.
#[test]
fn test_rate_limiting() {
    use roxy_rpc::RateLimiterConfig;

    let config = RateLimiterConfig::new(3, Duration::from_secs(60));
    let limiter = SlidingWindowRateLimiter::new(config);
    let key = "test_client";

    // First 3 requests should be allowed
    assert!(limiter.check_and_record(key).is_allowed());
    assert!(limiter.check_and_record(key).is_allowed());
    assert!(limiter.check_and_record(key).is_allowed());

    // 4th request should be limited
    let result = limiter.check_and_record(key);
    assert!(result.is_limited());

    // Verify retry_after is set
    if let RateLimitResult::Limited { retry_after } = result {
        assert!(retry_after > Duration::ZERO);
        assert!(retry_after <= Duration::from_secs(60));
    }
}

/// Test rate limiting with multiple clients.
#[test]
fn test_rate_limiting_per_client() {
    use roxy_rpc::RateLimiterConfig;

    let config = RateLimiterConfig::new(2, Duration::from_secs(60));
    let limiter = SlidingWindowRateLimiter::new(config);

    // Client 1 exhausts limit
    limiter.record("client1");
    limiter.record("client1");
    assert!(limiter.check("client1").is_limited());

    // Client 2 should still have quota
    assert!(limiter.check("client2").is_allowed());
    assert!(limiter.check_and_record("client2").is_allowed());
}

/// Test rate limiter reset.
#[test]
fn test_rate_limiter_reset() {
    use roxy_rpc::RateLimiterConfig;

    let config = RateLimiterConfig::new(2, Duration::from_secs(60));
    let limiter = SlidingWindowRateLimiter::new(config);
    let key = "test_client";

    // Exhaust limit
    limiter.record(key);
    limiter.record(key);
    assert!(limiter.check(key).is_limited());

    // Reset
    limiter.reset(key);
    assert!(limiter.check(key).is_allowed());
    assert_eq!(limiter.count(key), 0);
}

// =============================================================================
// Method Routing Tests
// =============================================================================

/// Test method routing with exact matches.
#[test]
fn test_method_routing_exact_match() {
    let router = MethodRouter::new()
        .route("eth_sendRawTransaction", RouteTarget::group("sequencer"))
        .route("eth_call", RouteTarget::group("archive"))
        .route_prefix("eth_", RouteTarget::group("default"))
        .fallback(RouteTarget::group("fallback"));

    // Exact matches take precedence
    assert_eq!(router.resolve("eth_sendRawTransaction").group_name(), Some("sequencer"));
    assert_eq!(router.resolve("eth_call").group_name(), Some("archive"));

    // Prefix matches
    assert_eq!(router.resolve("eth_blockNumber").group_name(), Some("default"));
    assert_eq!(router.resolve("eth_getBalance").group_name(), Some("default"));

    // Fallback
    assert_eq!(router.resolve("web3_clientVersion").group_name(), Some("fallback"));
}

/// Test method routing with prefix priorities.
#[test]
fn test_method_routing_prefix_priority() {
    let router = MethodRouter::new()
        .route_prefix("debug_", RouteTarget::group("debug"))
        .route_prefix("debug_trace", RouteTarget::group("trace"));

    // Longer prefix should match
    assert_eq!(router.resolve("debug_traceCall").group_name(), Some("trace"));
    assert_eq!(router.resolve("debug_traceTransaction").group_name(), Some("trace"));

    // Shorter prefix for non-matching
    assert_eq!(router.resolve("debug_getRawBlock").group_name(), Some("debug"));
}

/// Test method routing with mixed blocked and allowed methods.
#[test]
fn test_method_routing_mixed() {
    let router = MethodRouter::new()
        .route_prefix("admin_", RouteTarget::Block)
        .route_prefix("eth_", RouteTarget::group("ethereum"))
        .route("debug_traceCall", RouteTarget::group("trace"))
        .fallback(RouteTarget::group("default"));

    // Admin methods blocked
    assert!(router.is_blocked("admin_addPeer"));
    assert!(router.is_blocked("admin_nodeInfo"));

    // Eth methods routed
    assert_eq!(router.resolve("eth_blockNumber").group_name(), Some("ethereum"));

    // Exact debug route
    assert_eq!(router.resolve("debug_traceCall").group_name(), Some("trace"));

    // Fallback for others
    assert_eq!(router.resolve("net_version").group_name(), Some("default"));
}

// =============================================================================
// Codec Integration Tests
// =============================================================================

/// Test codec round-trip encoding/decoding.
#[test]
fn test_codec_roundtrip() {
    use roxy_rpc::{JsonRpcError, ParsedResponse};
    use serde_json::value::RawValue;

    let codec = RpcCodec::default();

    // Test request parsing
    let request = br#"{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}"#;
    let parsed = codec.decode(request).unwrap();

    match parsed {
        ParsedRequestPacket::Single(req) => {
            assert_eq!(req.method(), "eth_blockNumber");
            assert_eq!(req.jsonrpc, "2.0");
            assert!(!req.is_notification());
        }
        _ => panic!("expected single request"),
    }

    // Test response encoding
    let response = ParsedResponse::success(
        serde_json::Value::Number(1.into()),
        RawValue::from_string("\"0x1234\"".to_string()).unwrap(),
    );

    let encoded = codec.encode_single_response(&response).unwrap();
    let parsed_response: serde_json::Value = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(parsed_response["jsonrpc"], "2.0");
    assert_eq!(parsed_response["id"], 1);
    assert_eq!(parsed_response["result"], "0x1234");

    // Test error response encoding
    let error_response = ParsedResponse::error(
        serde_json::Value::Number(1.into()),
        JsonRpcError::method_not_found(),
    );

    let encoded_error = codec.encode_single_response(&error_response).unwrap();
    let parsed_error: serde_json::Value = serde_json::from_slice(&encoded_error).unwrap();

    assert_eq!(parsed_error["error"]["code"], -32601);
    assert_eq!(parsed_error["error"]["message"], "Method not found");
}

/// Test batch codec round-trip.
#[test]
fn test_batch_codec_roundtrip() {
    use roxy_rpc::{ParsedResponse, ParsedResponsePacket};
    use serde_json::value::RawValue;

    let codec = RpcCodec::default();

    // Parse batch request
    let batch = br#"[
        {"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1},
        {"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":2}
    ]"#;

    let parsed = codec.decode(batch).unwrap();
    assert!(matches!(parsed, ParsedRequestPacket::Batch(_)));
    assert_eq!(parsed.len(), 2);

    // Encode batch response
    let responses = vec![
        ParsedResponse::success(
            serde_json::Value::Number(1.into()),
            RawValue::from_string("\"0x1000\"".to_string()).unwrap(),
        ),
        ParsedResponse::success(
            serde_json::Value::Number(2.into()),
            RawValue::from_string("\"0x1\"".to_string()).unwrap(),
        ),
    ];
    let response_packet = ParsedResponsePacket::Batch(responses);

    let encoded = codec.encode_response(&response_packet).unwrap();
    let parsed_responses: Vec<serde_json::Value> = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(parsed_responses.len(), 2);
    assert_eq!(parsed_responses[0]["id"], 1);
    assert_eq!(parsed_responses[0]["result"], "0x1000");
    assert_eq!(parsed_responses[1]["id"], 2);
    assert_eq!(parsed_responses[1]["result"], "0x1");
}

/// Test codec error handling for malformed input.
#[test]
fn test_codec_error_handling() {
    let codec = RpcCodec::default();

    // Invalid JSON
    let invalid_json = b"not valid json";
    assert!(codec.decode(invalid_json).is_err());

    // Empty input
    let empty = b"";
    assert!(codec.decode(empty).is_err());

    // Missing required fields
    let missing_method = br#"{"jsonrpc":"2.0","params":[],"id":1}"#;
    assert!(codec.decode(missing_method).is_err());

    // Empty batch
    let empty_batch = b"[]";
    assert!(codec.decode(empty_batch).is_err());
}

/// Test codec size limits.
#[test]
fn test_codec_size_limits() {
    let config = DefaultCodecConfig::new().with_max_size(100);
    let codec = RpcCodec::new(config);

    // Small request should work
    let small = br#"{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}"#;
    assert!(codec.decode(small).is_ok());

    // Large request should fail
    let large = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_blockNumber","params":["{:0<200}"],"id":1}}"#,
        ""
    );
    let result = codec.decode(large.as_bytes());
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("exceeds maximum"));
}

/// Test codec nesting depth limits.
#[test]
fn test_codec_depth_limits() {
    let config = DefaultCodecConfig::new().with_max_depth(5);
    let codec = RpcCodec::new(config);

    // Shallow nesting should work
    let shallow = br#"{"jsonrpc":"2.0","method":"test","params":[[1]],"id":1}"#;
    assert!(codec.decode(shallow).is_ok());

    // Deep nesting should fail
    let deep = br#"{"jsonrpc":"2.0","method":"test","params":[[[[[[[[[[]]]]]]]]]],"id":1}"#;
    let result = codec.decode(deep);
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("nesting depth"));
}

// =============================================================================
// End-to-End Integration Tests
// =============================================================================

/// Test a complete request processing pipeline.
#[test]
fn test_complete_request_pipeline() {
    use roxy_rpc::{
        MaxParamsValidator, MethodAllowlist, ParsedResponse, ParsedResponsePacket,
        RateLimiterConfig,
    };
    use serde_json::value::RawValue;

    // 1. Setup components
    let codec = RpcCodec::default();

    let allowed_methods = vec![
        "eth_blockNumber".to_string(),
        "eth_chainId".to_string(),
        "eth_getBalance".to_string(),
        "eth_call".to_string(),
    ];
    let allowlist = MethodAllowlist::new(allowed_methods);
    let max_params = MaxParamsValidator::new(10);
    let validator_chain = ValidatorChain::new().with(allowlist).with(max_params);

    let router = MethodRouter::new()
        .route("eth_call", RouteTarget::group("archive"))
        .route_prefix("eth_", RouteTarget::group("ethereum"))
        .fallback(RouteTarget::group("default"));

    let rate_limiter_config = RateLimiterConfig::new(100, Duration::from_secs(60));
    let rate_limiter = SlidingWindowRateLimiter::new(rate_limiter_config);

    // 2. Process a request
    let request_bytes =
        br#"{"jsonrpc":"2.0","method":"eth_getBalance","params":["0x1234","latest"],"id":1}"#;

    // Decode
    let packet = codec.decode(request_bytes).expect("decode should succeed");

    // Check rate limit
    let client_key = "192.168.1.1";
    let rate_result = rate_limiter.check_and_record(client_key);
    assert!(rate_result.is_allowed());

    // Validate
    match packet {
        ParsedRequestPacket::Single(req) => {
            let validation_result = validator_chain.validate(req.clone());
            assert!(validation_result.is_valid());

            // Route
            let target = router.resolve(req.method());
            assert_eq!(target.group_name(), Some("ethereum"));
        }
        _ => panic!("expected single request"),
    }

    // 3. Encode response
    let response = ParsedResponse::success(
        serde_json::Value::Number(1.into()),
        RawValue::from_string("\"0x1000000000000000\"".to_string()).unwrap(),
    );
    let response_packet = ParsedResponsePacket::Single(response);
    let encoded = codec.encode_response(&response_packet).expect("encode should succeed");
    assert!(!encoded.is_empty());
}

/// Test pipeline with rejected request.
#[test]
fn test_pipeline_rejected_request() {
    use roxy_rpc::MethodAllowlist;

    let codec = RpcCodec::default();

    let allowed_methods = vec!["eth_blockNumber".to_string()];
    let allowlist = MethodAllowlist::new(allowed_methods);
    let validator_chain = ValidatorChain::new().with(allowlist);

    // Request for disallowed method
    let request_bytes = br#"{"jsonrpc":"2.0","method":"eth_sendTransaction","params":[],"id":1}"#;

    let packet = codec.decode(request_bytes).expect("decode should succeed");

    match packet {
        ParsedRequestPacket::Single(req) => {
            let result = validator_chain.validate(req);
            assert!(result.is_invalid());

            if let ValidationResult::Invalid(error) = result {
                assert_eq!(error.code, -32601);
            }
        }
        _ => panic!("expected single request"),
    }
}

/// Test pipeline with rate limited request.
#[test]
fn test_pipeline_rate_limited() {
    use roxy_rpc::RateLimiterConfig;

    let rate_limiter_config = RateLimiterConfig::new(1, Duration::from_secs(60));
    let rate_limiter = SlidingWindowRateLimiter::new(rate_limiter_config);

    let client_key = "192.168.1.1";

    // First request allowed
    assert!(rate_limiter.check_and_record(client_key).is_allowed());

    // Second request rate limited
    let result = rate_limiter.check(client_key);
    assert!(result.is_limited());
}
