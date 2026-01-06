//! Integration tests for RPC codec
//!
//! These tests verify the codec handles various edge cases and malformed inputs correctly.

use roxy_rpc::{JsonRpcError, ParsedRequestPacket, ParsedResponse, ParsedResponsePacket, RpcCodec};
use roxy_traits::{CodecConfig, DefaultCodecConfig};
use serde_json::value::RawValue;

// =============================================================================
// Round-trip Encoding/Decoding Tests
// =============================================================================

/// Test round-trip encoding/decoding for a simple request.
#[test]
fn test_codec_roundtrip_simple() {
    let codec = RpcCodec::default();
    let request = br#"{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}"#;

    let parsed = codec.decode(request).unwrap();

    match parsed {
        ParsedRequestPacket::Single(req) => {
            assert_eq!(req.jsonrpc, "2.0");
            assert_eq!(req.method(), "eth_blockNumber");
            assert!(!req.is_notification());
        }
        _ => panic!("expected single request"),
    }
}

/// Test round-trip with complex parameters.
#[test]
fn test_codec_roundtrip_complex_params() {
    let codec = RpcCodec::default();
    let request = br#"{
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [
            {
                "to": "0x1234567890abcdef1234567890abcdef12345678",
                "from": "0xabcdef1234567890abcdef1234567890abcdef12",
                "data": "0x70a08231000000000000000000000000abcdef1234567890abcdef1234567890abcdef12",
                "gas": "0x5208",
                "gasPrice": "0x3b9aca00"
            },
            "latest"
        ],
        "id": 42
    }"#;

    let parsed = codec.decode(request).unwrap();

    match parsed {
        ParsedRequestPacket::Single(req) => {
            assert_eq!(req.method(), "eth_call");
            assert!(req.params.is_some());

            // Verify params are preserved
            let params = req.params.as_ref().unwrap();
            let params_str = params.get();
            assert!(params_str.contains("0x1234567890abcdef1234567890abcdef12345678"));
            assert!(params_str.contains("latest"));
        }
        _ => panic!("expected single request"),
    }
}

/// Test notification (request without id).
#[test]
fn test_codec_notification() {
    let codec = RpcCodec::default();

    // Request without id
    let notification = br#"{"jsonrpc":"2.0","method":"eth_subscription","params":["0x1234"]}"#;
    let parsed = codec.decode(notification).unwrap();

    match parsed {
        ParsedRequestPacket::Single(req) => {
            assert!(req.is_notification());
        }
        _ => panic!("expected single request"),
    }

    // Request with null id
    let null_id = br#"{"jsonrpc":"2.0","method":"eth_subscription","params":[],"id":null}"#;
    let parsed = codec.decode(null_id).unwrap();

    match parsed {
        ParsedRequestPacket::Single(req) => {
            assert!(req.is_notification());
        }
        _ => panic!("expected single request"),
    }
}

// =============================================================================
// Batch Codec Tests
// =============================================================================

/// Test batch request parsing.
#[test]
fn test_batch_codec_roundtrip() {
    let codec = RpcCodec::default();
    let batch = br#"[
        {"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1},
        {"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":2},
        {"jsonrpc":"2.0","method":"eth_gasPrice","params":[],"id":3}
    ]"#;

    let parsed = codec.decode(batch).unwrap();

    match parsed {
        ParsedRequestPacket::Batch(requests) => {
            assert_eq!(requests.len(), 3);
            assert_eq!(requests[0].method(), "eth_blockNumber");
            assert_eq!(requests[1].method(), "eth_chainId");
            assert_eq!(requests[2].method(), "eth_gasPrice");
        }
        _ => panic!("expected batch request"),
    }
}

/// Test batch response encoding.
#[test]
fn test_batch_response_encoding() {
    let codec = RpcCodec::default();

    let responses = vec![
        ParsedResponse::success(
            serde_json::Value::Number(1.into()),
            RawValue::from_string("\"0x1000\"".to_string()).unwrap(),
        ),
        ParsedResponse::error(
            serde_json::Value::Number(2.into()),
            JsonRpcError::method_not_found(),
        ),
        ParsedResponse::success(
            serde_json::Value::Number(3.into()),
            RawValue::from_string("\"0x3b9aca00\"".to_string()).unwrap(),
        ),
    ];
    let response_packet = ParsedResponsePacket::Batch(responses);

    let encoded = codec.encode_response(&response_packet).unwrap();
    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(parsed.len(), 3);

    // First response - success
    assert_eq!(parsed[0]["id"], 1);
    assert_eq!(parsed[0]["result"], "0x1000");

    // Second response - error
    assert_eq!(parsed[1]["id"], 2);
    assert_eq!(parsed[1]["error"]["code"], -32601);

    // Third response - success
    assert_eq!(parsed[2]["id"], 3);
    assert_eq!(parsed[2]["result"], "0x3b9aca00");
}

/// Test single-item batch.
#[test]
fn test_single_item_batch() {
    let codec = RpcCodec::default();
    let batch = br#"[{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}]"#;

    let parsed = codec.decode(batch).unwrap();

    // Even with one item, it should be parsed as a batch
    assert!(parsed.is_batch());
    assert_eq!(parsed.len(), 1);
}

// =============================================================================
// Error Handling Tests
// =============================================================================

/// Test various malformed inputs.
#[test]
fn test_codec_error_handling_invalid_json() {
    let codec = RpcCodec::default();

    // Not JSON at all
    let result = codec.decode(b"this is not json");
    assert!(result.is_err());

    // Truncated JSON
    let result = codec.decode(br#"{"jsonrpc":"2.0","method":"#);
    assert!(result.is_err());

    // Invalid escape sequence
    let result = codec.decode(br#"{"jsonrpc":"2.0","method":"\z","id":1}"#);
    assert!(result.is_err());
}

/// Test empty inputs.
#[test]
fn test_codec_error_handling_empty() {
    let codec = RpcCodec::default();

    // Empty bytes
    assert!(codec.decode(b"").is_err());

    // Whitespace only
    assert!(codec.decode(b"   ").is_err());

    // Empty batch
    let result = codec.decode(b"[]");
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("empty batch"));
}

/// Test invalid start characters.
#[test]
fn test_codec_error_handling_invalid_start() {
    let codec = RpcCodec::default();

    // Number
    let result = codec.decode(b"123");
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("expected"));

    // String
    let result = codec.decode(br#""hello""#);
    assert!(result.is_err());

    // Boolean
    let result = codec.decode(b"true");
    assert!(result.is_err());
}

/// Test missing required fields.
#[test]
fn test_codec_error_handling_missing_fields() {
    let codec = RpcCodec::default();

    // Missing method
    let result = codec.decode(br#"{"jsonrpc":"2.0","params":[],"id":1}"#);
    assert!(result.is_err());

    // Missing jsonrpc
    let result = codec.decode(br#"{"method":"eth_blockNumber","params":[],"id":1}"#);
    assert!(result.is_err());
}

/// Test size limit errors.
#[test]
fn test_codec_error_handling_size_limit() {
    let config = DefaultCodecConfig::new().with_max_size(50);
    let codec = RpcCodec::new(config);

    let request = br#"{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}"#;
    let result = codec.decode(request);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.0.contains("exceeds maximum"));
}

/// Test batch size limit errors.
#[test]
fn test_codec_error_handling_batch_limit() {
    let config = DefaultCodecConfig::new().with_max_batch_size(1);
    let codec = RpcCodec::new(config);

    let batch = br#"[
        {"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1},
        {"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":2}
    ]"#;
    let result = codec.decode(batch);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.0.contains("exceeds maximum"));
}

/// Test batch not allowed error.
#[test]
fn test_codec_error_handling_batch_not_allowed() {
    let config = DefaultCodecConfig::new().with_allow_batch(false);
    let codec = RpcCodec::new(config);

    let batch = br#"[{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}]"#;
    let result = codec.decode(batch);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.0.contains("not allowed"));
}

/// Test nesting depth limit errors.
#[test]
fn test_codec_error_handling_depth_limit() {
    let config = DefaultCodecConfig::new().with_max_depth(3);
    let codec = RpcCodec::new(config);

    // 4 levels of nesting (including outer object)
    let deep = br#"{"jsonrpc":"2.0","method":"test","params":[[[1]]],"id":1}"#;
    let result = codec.decode(deep);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.0.contains("nesting depth"));
}

// =============================================================================
// Response Encoding Tests
// =============================================================================

/// Test success response encoding.
#[test]
fn test_response_encoding_success() {
    let codec = RpcCodec::default();

    let response = ParsedResponse::success(
        serde_json::Value::Number(42.into()),
        RawValue::from_string("\"0xdeadbeef\"".to_string()).unwrap(),
    );

    let encoded = codec.encode_single_response(&response).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], 42);
    assert_eq!(parsed["result"], "0xdeadbeef");
    assert!(parsed.get("error").is_none());
}

/// Test error response encoding.
#[test]
fn test_response_encoding_error() {
    let codec = RpcCodec::default();

    let response = ParsedResponse::error(
        serde_json::Value::String("request-1".to_string()),
        JsonRpcError::new(-32000, "Execution reverted"),
    );

    let encoded = codec.encode_single_response(&response).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], "request-1");
    assert_eq!(parsed["error"]["code"], -32000);
    assert_eq!(parsed["error"]["message"], "Execution reverted");
    assert!(parsed.get("result").is_none());
}

/// Test error response with data.
#[test]
fn test_response_encoding_error_with_data() {
    let codec = RpcCodec::default();

    let error = JsonRpcError::with_data(
        -32000,
        "Execution reverted",
        RawValue::from_string(r#"{"revertReason":"0x08c379a0..."}"#.to_string()).unwrap(),
    );
    let response = ParsedResponse::error(serde_json::Value::Number(1.into()), error);

    let encoded = codec.encode_single_response(&response).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(parsed["error"]["code"], -32000);
    assert!(parsed["error"]["data"]["revertReason"].is_string());
}

/// Test all standard JSON-RPC error codes.
#[test]
fn test_standard_error_codes() {
    assert_eq!(JsonRpcError::parse_error().code, -32700);
    assert_eq!(JsonRpcError::invalid_request().code, -32600);
    assert_eq!(JsonRpcError::method_not_found().code, -32601);
    assert_eq!(JsonRpcError::invalid_params().code, -32602);
    assert_eq!(JsonRpcError::internal_error().code, -32603);
}

// =============================================================================
// ID Type Tests
// =============================================================================

/// Test various ID types.
#[test]
fn test_id_types() {
    let codec = RpcCodec::default();

    // Numeric ID
    let numeric = br#"{"jsonrpc":"2.0","method":"test","params":[],"id":42}"#;
    let parsed = codec.decode(numeric).unwrap();
    match parsed {
        ParsedRequestPacket::Single(req) => {
            assert_eq!(req.id, Some(serde_json::Value::Number(42.into())));
        }
        _ => panic!("expected single"),
    }

    // String ID
    let string = br#"{"jsonrpc":"2.0","method":"test","params":[],"id":"request-1"}"#;
    let parsed = codec.decode(string).unwrap();
    match parsed {
        ParsedRequestPacket::Single(req) => {
            assert_eq!(req.id, Some(serde_json::Value::String("request-1".to_string())));
        }
        _ => panic!("expected single"),
    }

    // Large numeric ID
    let large = br#"{"jsonrpc":"2.0","method":"test","params":[],"id":9007199254740991}"#;
    let parsed = codec.decode(large).unwrap();
    match parsed {
        ParsedRequestPacket::Single(req) => {
            assert!(req.id.is_some());
        }
        _ => panic!("expected single"),
    }
}

// =============================================================================
// Edge Case Tests
// =============================================================================

/// Test whitespace handling.
#[test]
fn test_whitespace_handling() {
    let codec = RpcCodec::default();

    // Leading whitespace
    let leading = b"  \n\t{\"jsonrpc\":\"2.0\",\"method\":\"test\",\"params\":[],\"id\":1}";
    assert!(codec.decode(leading).is_ok());

    // Trailing whitespace
    let trailing = b"{\"jsonrpc\":\"2.0\",\"method\":\"test\",\"params\":[],\"id\":1}  \n";
    assert!(codec.decode(trailing).is_ok());

    // Whitespace in JSON (between fields)
    let internal = br#"{
        "jsonrpc" : "2.0" ,
        "method"  : "test" ,
        "params"  : [ ] ,
        "id"      : 1
    }"#;
    assert!(codec.decode(internal).is_ok());
}

/// Test Unicode method names.
#[test]
fn test_unicode_method_names() {
    let codec = RpcCodec::default();

    // Method with unicode
    let unicode = br#"{"jsonrpc":"2.0","method":"test_\u65e5\u672c\u8a9e","params":[],"id":1}"#;
    let result = codec.decode(unicode);
    assert!(result.is_ok());
}

/// Test escaped characters in strings.
#[test]
fn test_escaped_characters() {
    let codec = RpcCodec::default();

    // Escaped quotes in params
    let escaped = br#"{"jsonrpc":"2.0","method":"test","params":["escaped \" quote"],"id":1}"#;
    let result = codec.decode(escaped);
    assert!(result.is_ok());

    // Other escape sequences
    let other = br#"{"jsonrpc":"2.0","method":"test","params":["tab\ttab\nnewline"],"id":1}"#;
    let result = codec.decode(other);
    assert!(result.is_ok());
}

/// Test ParsedRequestPacket utility methods.
#[test]
fn test_parsed_request_packet_methods() {
    let codec = RpcCodec::default();

    // Single request
    let single = br#"{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}"#;
    let packet = codec.decode(single).unwrap();

    assert!(!packet.is_batch());
    assert!(!packet.is_empty());
    assert_eq!(packet.len(), 1);

    let methods: Vec<_> = packet.methods().collect();
    assert_eq!(methods, vec!["eth_blockNumber"]);

    let requests: Vec<_> = packet.requests().collect();
    assert_eq!(requests.len(), 1);

    // Batch request
    let batch = br#"[
        {"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1},
        {"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":2}
    ]"#;
    let packet = codec.decode(batch).unwrap();

    assert!(packet.is_batch());
    assert!(!packet.is_empty());
    assert_eq!(packet.len(), 2);

    let methods: Vec<_> = packet.methods().collect();
    assert_eq!(methods, vec!["eth_blockNumber", "eth_chainId"]);
}

/// Test ParsedResponse utility methods.
#[test]
fn test_parsed_response_methods() {
    // Success response
    let success = ParsedResponse::success(
        serde_json::Value::Number(1.into()),
        RawValue::from_string("true".to_string()).unwrap(),
    );
    assert!(!success.is_error());

    // Error response
    let error =
        ParsedResponse::error(serde_json::Value::Number(1.into()), JsonRpcError::internal_error());
    assert!(error.is_error());
}

/// Test codec config accessor.
#[test]
fn test_codec_config_accessor() {
    let config = DefaultCodecConfig::new()
        .with_max_size(512)
        .with_max_depth(8)
        .with_max_batch_size(5)
        .with_allow_batch(false);
    let codec = RpcCodec::new(config);

    assert_eq!(codec.config().max_size(), 512);
    assert_eq!(codec.config().max_depth(), 8);
    assert_eq!(codec.config().max_batch_size(), 5);
    assert!(!codec.config().allow_batch());
}

/// Test Bytes decoding convenience method.
#[test]
fn test_decode_bytes() {
    use bytes::Bytes;

    let codec = RpcCodec::default();
    let bytes = Bytes::from_static(br#"{"jsonrpc":"2.0","method":"test","params":[],"id":1}"#);

    let result = codec.decode_bytes(&bytes);
    assert!(result.is_ok());
}
