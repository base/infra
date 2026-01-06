//! Bounded JSON-RPC codec with configurable limits.
//!
//! This module provides a codec for parsing and serializing JSON-RPC requests
//! and responses with configurable size and batch limits.

use bytes::Bytes;
use roxy_traits::{CodecConfig, CodecError, DefaultCodecConfig};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// A parsed JSON-RPC request that can be deserialized from raw bytes.
///
/// This is used for initial parsing and validation before forwarding.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParsedRequest {
    /// The JSON-RPC version (should be "2.0").
    pub jsonrpc: String,
    /// The method name.
    pub method: String,
    /// The request parameters (kept as raw JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Box<RawValue>>,
    /// The request ID (optional for notifications).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
}

impl ParsedRequest {
    /// Get the method name.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Check if this is a notification (no id or null id).
    pub const fn is_notification(&self) -> bool {
        self.id.is_none() || matches!(&self.id, Some(serde_json::Value::Null))
    }
}

/// A parsed request packet that can be either a single request or a batch.
#[derive(Debug, Clone)]
pub enum ParsedRequestPacket {
    /// A single request.
    Single(ParsedRequest),
    /// A batch of requests.
    Batch(Vec<ParsedRequest>),
}

impl ParsedRequestPacket {
    /// Get the number of requests in this packet.
    pub const fn len(&self) -> usize {
        match self {
            Self::Single(_) => 1,
            Self::Batch(requests) => requests.len(),
        }
    }

    /// Check if this packet is empty (only possible for malformed batches).
    pub const fn is_empty(&self) -> bool {
        match self {
            Self::Single(_) => false,
            Self::Batch(requests) => requests.is_empty(),
        }
    }

    /// Check if this is a batch request.
    pub const fn is_batch(&self) -> bool {
        matches!(self, Self::Batch(_))
    }

    /// Get an iterator over the methods in this packet.
    pub fn methods(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Self::Single(req) => Box::new(std::iter::once(req.method.as_str())),
            Self::Batch(requests) => Box::new(requests.iter().map(|r| r.method.as_str())),
        }
    }

    /// Get an iterator over the requests in this packet.
    pub fn requests(&self) -> Box<dyn Iterator<Item = &ParsedRequest> + '_> {
        match self {
            Self::Single(req) => Box::new(std::iter::once(req)),
            Self::Batch(requests) => Box::new(requests.iter()),
        }
    }
}

/// A JSON-RPC response that can be serialized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedResponse {
    /// The JSON-RPC version (always "2.0").
    pub jsonrpc: String,
    /// The response result (mutually exclusive with error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Box<RawValue>>,
    /// The response error (mutually exclusive with result).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    /// The request ID.
    pub id: serde_json::Value,
}

impl ParsedResponse {
    /// Create a successful response.
    pub fn success(id: serde_json::Value, result: Box<RawValue>) -> Self {
        Self { jsonrpc: "2.0".to_string(), result: Some(result), error: None, id }
    }

    /// Create an error response.
    pub fn error(id: serde_json::Value, error: JsonRpcError) -> Self {
        Self { jsonrpc: "2.0".to_string(), result: None, error: Some(error), id }
    }

    /// Check if this response is an error.
    pub const fn is_error(&self) -> bool {
        self.error.is_some()
    }
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// The error code.
    pub code: i64,
    /// The error message.
    pub message: String,
    /// Optional additional data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<RawValue>>,
}

impl JsonRpcError {
    /// Create a new error.
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    /// Create a new error with data.
    pub fn with_data(code: i64, message: impl Into<String>, data: Box<RawValue>) -> Self {
        Self { code, message: message.into(), data: Some(data) }
    }

    /// Parse error (-32700).
    pub fn parse_error() -> Self {
        Self::new(-32700, "Parse error")
    }

    /// Invalid request (-32600).
    pub fn invalid_request() -> Self {
        Self::new(-32600, "Invalid Request")
    }

    /// Method not found (-32601).
    pub fn method_not_found() -> Self {
        Self::new(-32601, "Method not found")
    }

    /// Invalid params (-32602).
    pub fn invalid_params() -> Self {
        Self::new(-32602, "Invalid params")
    }

    /// Internal error (-32603).
    pub fn internal_error() -> Self {
        Self::new(-32603, "Internal error")
    }
}

/// A parsed response packet that can be either a single response or a batch.
#[derive(Debug, Clone)]
pub enum ParsedResponsePacket {
    /// A single response.
    Single(ParsedResponse),
    /// A batch of responses.
    Batch(Vec<ParsedResponse>),
}

/// Bounded JSON-RPC codec with configurable limits.
///
/// This codec enforces size limits, nesting depth, and batch size constraints
/// when parsing JSON-RPC requests.
///
/// # Example
///
/// ```
/// use roxy_rpc::RpcCodec;
/// use roxy_traits::DefaultCodecConfig;
///
/// let codec = RpcCodec::new(DefaultCodecConfig::new());
///
/// // Parse a single request
/// let request_bytes = br#"{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}"#;
/// let packet = codec.decode(request_bytes).unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct RpcCodec<C: CodecConfig = DefaultCodecConfig> {
    config: C,
}

impl<C: CodecConfig> RpcCodec<C> {
    /// Create a new RPC codec with the given configuration.
    pub const fn new(config: C) -> Self {
        Self { config }
    }

    /// Get a reference to the codec configuration.
    pub const fn config(&self) -> &C {
        &self.config
    }

    /// Parse raw bytes into a ParsedRequestPacket, enforcing size limits.
    ///
    /// This parses the JSON and validates structure. Use this when you need
    /// to inspect the request method or parameters.
    ///
    /// # Errors
    ///
    /// Returns a `CodecError` if:
    /// - The input exceeds the maximum size limit
    /// - The JSON is malformed
    /// - Batch requests are not allowed but a batch is received
    /// - The batch size exceeds the maximum
    pub fn decode(&self, bytes: &[u8]) -> Result<ParsedRequestPacket, CodecError> {
        // Check size limit
        self.validate_size(bytes.len())?;

        // Check nesting depth before full parse
        self.validate_nesting_depth(bytes)?;

        // Parse the JSON to determine if it's a batch or single request
        let packet = self.parse_request_packet(bytes)?;

        // Validate batch constraints
        self.validate_batch_size(&packet)?;

        Ok(packet)
    }

    /// Decode from a `Bytes` buffer.
    ///
    /// This is a convenience method that delegates to [`Self::decode`].
    pub fn decode_bytes(&self, bytes: &Bytes) -> Result<ParsedRequestPacket, CodecError> {
        self.decode(bytes.as_ref())
    }

    /// Serialize a ParsedResponsePacket to bytes.
    ///
    /// # Errors
    ///
    /// Returns a `CodecError` if serialization fails.
    pub fn encode_response(&self, response: &ParsedResponsePacket) -> Result<Bytes, CodecError> {
        let json = match response {
            ParsedResponsePacket::Single(r) => serde_json::to_vec(r),
            ParsedResponsePacket::Batch(rs) => serde_json::to_vec(rs),
        }
        .map_err(|e| CodecError::new(format!("encode error: {e}")))?;
        Ok(Bytes::from(json))
    }

    /// Serialize a single ParsedResponse to bytes.
    ///
    /// # Errors
    ///
    /// Returns a `CodecError` if serialization fails.
    pub fn encode_single_response(&self, response: &ParsedResponse) -> Result<Bytes, CodecError> {
        let json = serde_json::to_vec(response)
            .map_err(|e| CodecError::new(format!("encode error: {e}")))?;
        Ok(Bytes::from(json))
    }

    /// Encode a serde-serializable value to bytes.
    ///
    /// # Errors
    ///
    /// Returns a `CodecError` if serialization fails.
    pub fn encode_value<T: Serialize>(&self, value: &T) -> Result<Bytes, CodecError> {
        let json =
            serde_json::to_vec(value).map_err(|e| CodecError::new(format!("encode error: {e}")))?;
        Ok(Bytes::from(json))
    }

    /// Parse raw bytes into a ParsedRequestPacket.
    fn parse_request_packet(&self, bytes: &[u8]) -> Result<ParsedRequestPacket, CodecError> {
        // Trim leading whitespace to check if it starts with [ (batch) or { (single)
        let trimmed = bytes.iter().skip_while(|b| b.is_ascii_whitespace()).copied().next();

        match trimmed {
            Some(b'[') => {
                // Batch request
                let requests: Vec<ParsedRequest> = serde_json::from_slice(bytes)
                    .map_err(|e| CodecError::new(format!("invalid JSON-RPC batch: {e}")))?;
                Ok(ParsedRequestPacket::Batch(requests))
            }
            Some(b'{') => {
                // Single request
                let request: ParsedRequest = serde_json::from_slice(bytes)
                    .map_err(|e| CodecError::new(format!("invalid JSON-RPC request: {e}")))?;
                Ok(ParsedRequestPacket::Single(request))
            }
            Some(c) => Err(CodecError::new(format!(
                "invalid JSON-RPC: expected '{{' or '[', got '{}'",
                c as char
            ))),
            None => Err(CodecError::new("empty request")),
        }
    }

    /// Validate that the request size is within limits.
    fn validate_size(&self, size: usize) -> Result<(), CodecError> {
        let max_size = self.config.max_size();
        if size > max_size {
            return Err(CodecError::new(format!("request size {size} exceeds maximum {max_size}")));
        }
        Ok(())
    }

    /// Validate that the JSON nesting depth is within limits.
    ///
    /// This performs a quick scan of the input to estimate nesting depth
    /// without fully parsing the JSON.
    fn validate_nesting_depth(&self, bytes: &[u8]) -> Result<(), CodecError> {
        let max_depth = self.config.max_depth();
        let mut depth = 0usize;
        let mut max_seen = 0usize;
        let mut in_string = false;
        let mut escape_next = false;

        for &byte in bytes {
            if escape_next {
                escape_next = false;
                continue;
            }

            match byte {
                b'\\' if in_string => {
                    escape_next = true;
                }
                b'"' => {
                    in_string = !in_string;
                }
                b'[' | b'{' if !in_string => {
                    depth += 1;
                    max_seen = max_seen.max(depth);
                    if max_seen > max_depth {
                        return Err(CodecError::new(format!(
                            "nesting depth exceeds maximum {max_depth}"
                        )));
                    }
                }
                b']' | b'}' if !in_string => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Validate that the request respects batch size limits.
    fn validate_batch_size(&self, packet: &ParsedRequestPacket) -> Result<(), CodecError> {
        match packet {
            ParsedRequestPacket::Single(_) => Ok(()),
            ParsedRequestPacket::Batch(requests) => {
                // Check if batch is allowed
                if !self.config.allow_batch() {
                    return Err(CodecError::new("batch requests are not allowed"));
                }

                // Check batch size
                let max_batch = self.config.max_batch_size();
                let batch_len = requests.len();
                if batch_len > max_batch {
                    return Err(CodecError::new(format!(
                        "batch size {batch_len} exceeds maximum {max_batch}"
                    )));
                }

                // Empty batch is invalid per JSON-RPC spec
                if requests.is_empty() {
                    return Err(CodecError::new("empty batch request"));
                }

                Ok(())
            }
        }
    }
}

impl Default for RpcCodec<DefaultCodecConfig> {
    fn default() -> Self {
        Self::new(DefaultCodecConfig::new())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Helper to create a valid single JSON-RPC request.
    fn single_request() -> &'static [u8] {
        br#"{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}"#
    }

    /// Helper to create a batch JSON-RPC request with the given number of items.
    fn batch_request(count: usize) -> Vec<u8> {
        let items: Vec<String> = (0..count)
            .map(|i| {
                format!(r#"{{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":{i}}}"#)
            })
            .collect();
        format!("[{}]", items.join(",")).into_bytes()
    }

    /// Helper to create a deeply nested JSON structure.
    fn nested_json(depth: usize) -> Vec<u8> {
        let mut s = String::new();
        s.push_str(r#"{"jsonrpc":"2.0","method":"test","params":"#);
        for _ in 0..depth {
            s.push('[');
        }
        for _ in 0..depth {
            s.push(']');
        }
        s.push_str(r#","id":1}"#);
        s.into_bytes()
    }

    // === Construction tests ===

    #[test]
    fn test_codec_new() {
        let config = DefaultCodecConfig::new();
        let codec = RpcCodec::new(config);
        assert_eq!(codec.config().max_size(), 1024 * 1024);
    }

    #[test]
    fn test_codec_default() {
        let codec = RpcCodec::default();
        assert_eq!(codec.config().max_size(), 1024 * 1024);
        assert!(codec.config().allow_batch());
    }

    // === Decode single request tests ===

    #[test]
    fn test_decode_single_request() {
        let codec = RpcCodec::default();
        let result = codec.decode(single_request());
        assert!(result.is_ok());

        let packet = result.unwrap();
        assert!(matches!(packet, ParsedRequestPacket::Single(_)));
        assert!(!packet.is_batch());
        assert_eq!(packet.len(), 1);
    }

    #[test]
    fn test_decode_single_request_method() {
        let codec = RpcCodec::default();
        let result = codec.decode(single_request()).unwrap();

        match result {
            ParsedRequestPacket::Single(req) => {
                assert_eq!(req.method(), "eth_blockNumber");
                assert!(!req.is_notification());
            }
            _ => panic!("expected single request"),
        }
    }

    #[test]
    fn test_decode_bytes() {
        let codec = RpcCodec::default();
        let bytes = Bytes::from_static(single_request());
        let result = codec.decode_bytes(&bytes);
        assert!(result.is_ok());
    }

    // === Decode batch request tests ===

    #[test]
    fn test_decode_batch_request() {
        let codec = RpcCodec::default();
        let batch = batch_request(3);
        let result = codec.decode(&batch);
        assert!(result.is_ok());

        let packet = result.unwrap();
        assert!(packet.is_batch());
        assert_eq!(packet.len(), 3);
    }

    #[test]
    fn test_decode_empty_batch_fails() {
        let codec = RpcCodec::default();
        let empty_batch = b"[]";
        let result = codec.decode(empty_batch);
        assert!(result.is_err());
        assert!(result.unwrap_err().0.contains("empty batch"));
    }

    // === ParsedRequest tests ===

    #[test]
    fn test_parsed_request_notification() {
        let codec = RpcCodec::default();

        // Request without id
        let notification = br#"{"jsonrpc":"2.0","method":"test","params":[]}"#;
        let result = codec.decode(notification).unwrap();
        match result {
            ParsedRequestPacket::Single(req) => {
                assert!(req.is_notification());
            }
            _ => panic!("expected single"),
        }

        // Request with null id
        let null_id = br#"{"jsonrpc":"2.0","method":"test","params":[],"id":null}"#;
        let result = codec.decode(null_id).unwrap();
        match result {
            ParsedRequestPacket::Single(req) => {
                assert!(req.is_notification());
            }
            _ => panic!("expected single"),
        }
    }

    #[test]
    fn test_parsed_request_methods_iterator() {
        let codec = RpcCodec::default();

        // Single request
        let single = codec.decode(single_request()).unwrap();
        let methods: Vec<_> = single.methods().collect();
        assert_eq!(methods, vec!["eth_blockNumber"]);

        // Batch request
        let batch_bytes = br#"[
            {"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1},
            {"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":2}
        ]"#;
        let batch = codec.decode(batch_bytes.as_slice()).unwrap();
        let methods: Vec<_> = batch.methods().collect();
        assert_eq!(methods, vec!["eth_blockNumber", "eth_chainId"]);
    }

    #[test]
    fn test_parsed_request_requests_iterator() {
        let codec = RpcCodec::default();

        // Single request
        let single = codec.decode(single_request()).unwrap();
        let reqs: Vec<_> = single.requests().collect();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method(), "eth_blockNumber");

        // Batch request
        let batch = codec.decode(&batch_request(3)).unwrap();
        let reqs: Vec<_> = batch.requests().collect();
        assert_eq!(reqs.len(), 3);
    }

    // === Size limit tests ===

    #[rstest]
    #[case::within_limit(100, 50, true)]
    #[case::at_limit(100, 100, true)]
    #[case::over_limit(100, 101, false)]
    fn test_size_validation(
        #[case] max_size: usize,
        #[case] input_size: usize,
        #[case] should_pass: bool,
    ) {
        let config = DefaultCodecConfig::new().with_max_size(max_size);
        let codec = RpcCodec::new(config);

        // Create a request with the specified size (approximately)
        let mut request = single_request().to_vec();
        if input_size > request.len() {
            // Pad with whitespace (valid JSON can have leading/trailing whitespace)
            let padding = input_size - request.len();
            let mut padded = vec![b' '; padding];
            padded.extend(&request);
            request = padded;
        }

        let result = codec.decode(&request[..input_size.min(request.len())]);

        if should_pass {
            // Size validation should pass (JSON parse may still fail if truncated)
            // We only check that size validation itself works
            let is_size_error =
                result.as_ref().err().is_some_and(|e| e.0.contains("exceeds maximum"));
            assert!(!is_size_error, "should not fail size validation");
        } else {
            assert!(result.is_err());
            assert!(result.unwrap_err().0.contains("exceeds maximum"));
        }
    }

    #[test]
    fn test_size_limit_error_message() {
        let config = DefaultCodecConfig::new().with_max_size(10);
        let codec = RpcCodec::new(config);
        let result = codec.decode(single_request());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.0.contains("exceeds maximum 10"));
    }

    // === Batch size limit tests ===

    #[rstest]
    #[case::within_limit(10, 5, true)]
    #[case::at_limit(10, 10, true)]
    #[case::over_limit(10, 11, false)]
    fn test_batch_size_validation(
        #[case] max_batch: usize,
        #[case] batch_size: usize,
        #[case] should_pass: bool,
    ) {
        let config =
            DefaultCodecConfig::new().with_max_size(1024 * 1024).with_max_batch_size(max_batch);
        let codec = RpcCodec::new(config);

        let batch = batch_request(batch_size);
        let result = codec.decode(&batch);

        if should_pass {
            assert!(result.is_ok(), "expected success: {:?}", result.err());
        } else {
            assert!(result.is_err());
            assert!(result.unwrap_err().0.contains("exceeds maximum"));
        }
    }

    // === Batch allowed tests ===

    #[test]
    fn test_batch_not_allowed() {
        let config = DefaultCodecConfig::new().with_allow_batch(false);
        let codec = RpcCodec::new(config);

        let batch = batch_request(2);
        let result = codec.decode(&batch);
        assert!(result.is_err());
        assert!(result.unwrap_err().0.contains("not allowed"));
    }

    #[test]
    fn test_single_request_when_batch_not_allowed() {
        let config = DefaultCodecConfig::new().with_allow_batch(false);
        let codec = RpcCodec::new(config);

        let result = codec.decode(single_request());
        assert!(result.is_ok());
    }

    // === Nesting depth tests ===

    #[rstest]
    #[case::within_limit(10, 5, true)]
    #[case::at_limit(10, 9, true)] // nested_json adds 1 for outer object, so 9+1=10
    #[case::over_limit(10, 10, false)] // 10+1=11 exceeds limit of 10
    fn test_nesting_depth_validation(
        #[case] max_depth: usize,
        #[case] actual_depth: usize,
        #[case] should_pass: bool,
    ) {
        let config = DefaultCodecConfig::new().with_max_size(1024 * 1024).with_max_depth(max_depth);
        let codec = RpcCodec::new(config);

        let nested = nested_json(actual_depth);
        let result = codec.decode(&nested);

        if should_pass {
            assert!(result.is_ok(), "expected success: {:?}", result.err());
        } else {
            assert!(result.is_err());
            assert!(result.unwrap_err().0.contains("nesting depth"));
        }
    }

    #[test]
    fn test_nesting_in_string_ignored() {
        let config = DefaultCodecConfig::new().with_max_depth(5);
        let codec = RpcCodec::new(config);

        // Brackets inside strings should not count towards nesting depth
        let request =
            br#"{"jsonrpc":"2.0","method":"test","params":["[[[[[[[[[[]]]]]]]]]]"],"id":1}"#;
        let result = codec.decode(request);
        assert!(result.is_ok());
    }

    #[test]
    fn test_escaped_quotes_in_string() {
        let config = DefaultCodecConfig::new().with_max_depth(5);
        let codec = RpcCodec::new(config);

        // Escaped quotes should not toggle string mode
        let request =
            br#"{"jsonrpc":"2.0","method":"test","params":["escaped \" quote [[["],"id":1}"#;
        let result = codec.decode(request);
        assert!(result.is_ok());
    }

    // === Invalid JSON tests ===

    #[rstest]
    #[case::missing_brace(b"{\"jsonrpc\":\"2.0\"")]
    #[case::invalid_json(b"not json at all")]
    #[case::empty(b"")]
    fn test_invalid_json(#[case] input: &[u8]) {
        let codec = RpcCodec::default();
        let result = codec.decode(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_start_character() {
        let codec = RpcCodec::default();
        let result = codec.decode(b"123");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.0.contains("expected '{'"));
    }

    // === Encode tests ===

    #[test]
    fn test_encode_single_response() {
        let codec = RpcCodec::default();

        let response = ParsedResponse::success(
            serde_json::Value::Number(1.into()),
            RawValue::from_string("\"0x1234\"".to_string()).unwrap(),
        );

        let result = codec.encode_single_response(&response);
        assert!(result.is_ok());

        let bytes = result.unwrap();
        assert!(!bytes.is_empty());

        // Verify it's valid JSON
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["result"], "0x1234");
        assert_eq!(parsed["jsonrpc"], "2.0");
    }

    #[test]
    fn test_encode_error_response() {
        let codec = RpcCodec::default();

        let response = ParsedResponse::error(
            serde_json::Value::Number(1.into()),
            JsonRpcError::method_not_found(),
        );

        let result = codec.encode_single_response(&response);
        assert!(result.is_ok());

        let bytes = result.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["error"]["code"], -32601);
        assert_eq!(parsed["error"]["message"], "Method not found");
    }

    #[test]
    fn test_encode_batch_response() {
        let codec = RpcCodec::default();

        let responses = vec![
            ParsedResponse::success(
                serde_json::Value::Number(1.into()),
                RawValue::from_string("\"0x1\"".to_string()).unwrap(),
            ),
            ParsedResponse::success(
                serde_json::Value::Number(2.into()),
                RawValue::from_string("\"0x2\"".to_string()).unwrap(),
            ),
        ];
        let packet = ParsedResponsePacket::Batch(responses);

        let result = codec.encode_response(&packet);
        assert!(result.is_ok());

        let bytes = result.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_encode_value() {
        let codec = RpcCodec::default();

        #[derive(Serialize)]
        struct TestValue {
            name: String,
            value: i32,
        }

        let value = TestValue { name: "test".to_string(), value: 42 };

        let result = codec.encode_value(&value);
        assert!(result.is_ok());

        let bytes = result.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["name"], "test");
        assert_eq!(parsed["value"], 42);
    }

    // === Response helper tests ===

    #[test]
    fn test_response_is_error() {
        let success = ParsedResponse::success(
            serde_json::Value::Number(1.into()),
            RawValue::from_string("true".to_string()).unwrap(),
        );
        assert!(!success.is_error());

        let error = ParsedResponse::error(
            serde_json::Value::Number(1.into()),
            JsonRpcError::internal_error(),
        );
        assert!(error.is_error());
    }

    #[test]
    fn test_json_rpc_error_constructors() {
        assert_eq!(JsonRpcError::parse_error().code, -32700);
        assert_eq!(JsonRpcError::invalid_request().code, -32600);
        assert_eq!(JsonRpcError::method_not_found().code, -32601);
        assert_eq!(JsonRpcError::invalid_params().code, -32602);
        assert_eq!(JsonRpcError::internal_error().code, -32603);
    }

    #[test]
    fn test_json_rpc_error_with_data() {
        let error = JsonRpcError::with_data(
            -32000,
            "Custom error",
            RawValue::from_string("{\"detail\":\"info\"}".to_string()).unwrap(),
        );
        assert_eq!(error.code, -32000);
        assert_eq!(error.message, "Custom error");
        assert!(error.data.is_some());
    }

    // === ID type tests ===

    #[test]
    fn test_decode_various_id_types() {
        let codec = RpcCodec::default();

        // Numeric ID
        let numeric = br#"{"jsonrpc":"2.0","method":"test","params":[],"id":42}"#;
        let result = codec.decode(numeric).unwrap();
        match result {
            ParsedRequestPacket::Single(req) => {
                assert_eq!(req.id, Some(serde_json::Value::Number(42.into())));
            }
            _ => panic!("expected single"),
        }

        // String ID
        let string_id = br#"{"jsonrpc":"2.0","method":"test","params":[],"id":"abc-123"}"#;
        let result = codec.decode(string_id).unwrap();
        match result {
            ParsedRequestPacket::Single(req) => {
                assert_eq!(req.id, Some(serde_json::Value::String("abc-123".to_string())));
            }
            _ => panic!("expected single"),
        }

        // Null ID - serde with #[serde(default)] treats null as missing,
        // so we check is_notification() which treats both as notifications
        let null_id = br#"{"jsonrpc":"2.0","method":"test","params":[],"id":null}"#;
        let result = codec.decode(null_id).unwrap();
        match result {
            ParsedRequestPacket::Single(req) => {
                // Null ID is treated as a notification
                assert!(req.is_notification());
            }
            _ => panic!("expected single"),
        }
    }

    // === Config access tests ===

    #[test]
    fn test_config_accessor() {
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

    // === Edge case tests ===

    #[test]
    fn test_whitespace_around_request() {
        let codec = RpcCodec::default();
        let with_whitespace =
            b"  \n\t{\"jsonrpc\":\"2.0\",\"method\":\"test\",\"params\":[],\"id\":1}  \n";
        let result = codec.decode(with_whitespace);
        assert!(result.is_ok());
    }

    #[test]
    fn test_batch_with_one_item() {
        let codec = RpcCodec::default();
        let single_item_batch = batch_request(1);
        let result = codec.decode(&single_item_batch);
        assert!(result.is_ok());

        match result.unwrap() {
            ParsedRequestPacket::Batch(requests) => {
                assert_eq!(requests.len(), 1);
            }
            _ => panic!("expected batch"),
        }
    }

    #[test]
    fn test_packet_is_empty() {
        let codec = RpcCodec::default();

        let single = codec.decode(single_request()).unwrap();
        assert!(!single.is_empty());

        let batch = codec.decode(&batch_request(3)).unwrap();
        assert!(!batch.is_empty());
    }

    // === Serialization round-trip tests ===

    #[test]
    fn test_request_serialization_round_trip() {
        let codec = RpcCodec::default();
        let original =
            br#"{"jsonrpc":"2.0","method":"eth_call","params":[{"to":"0x1234"},"latest"],"id":1}"#;

        let parsed = codec.decode(original).unwrap();
        match parsed {
            ParsedRequestPacket::Single(req) => {
                assert_eq!(req.method(), "eth_call");
                // Serialize back
                let serialized = codec.encode_value(&req).unwrap();
                // Parse again
                let reparsed = codec.decode(&serialized).unwrap();
                match reparsed {
                    ParsedRequestPacket::Single(req2) => {
                        assert_eq!(req2.method(), "eth_call");
                    }
                    _ => panic!("expected single"),
                }
            }
            _ => panic!("expected single"),
        }
    }
}
