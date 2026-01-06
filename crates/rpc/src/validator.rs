//! Request validation chain for RPC requests.
//!
//! This module provides a flexible validation chain that can run multiple validators
//! sequentially on each incoming RPC request. Validators can pass, fail with an error,
//! or modify the request before passing it to the next validator.

use std::sync::Arc;

use derive_more::{Debug, Display, Error};

use crate::ParsedRequest;

/// Error from request validation.
#[derive(Debug, Display, Error, Clone)]
#[display("validation error: {message}")]
pub struct ValidationError {
    /// JSON-RPC error code for the validation failure.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
}

impl ValidationError {
    /// Create a new validation error.
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    /// Create a method not allowed error.
    pub fn method_not_allowed(method: &str) -> Self {
        Self::new(-32601, format!("method '{}' is not allowed", method))
    }

    /// Create an invalid params error.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message.into())
    }
}

/// Result of validation - can transform the request or reject it.
#[derive(Debug)]
pub enum ValidationResult {
    /// Request is valid, continue with (possibly modified) request.
    Valid(ParsedRequest),
    /// Request is invalid, return this error.
    Invalid(ValidationError),
}

impl ValidationResult {
    /// Returns `true` if the validation result is valid.
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid(_))
    }

    /// Returns `true` if the validation result is invalid.
    pub const fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid(_))
    }

    /// Unwraps the valid request, panicking if invalid.
    ///
    /// # Panics
    /// Panics if the result is `Invalid`.
    pub fn unwrap(self) -> ParsedRequest {
        match self {
            Self::Valid(request) => request,
            Self::Invalid(error) => panic!("called unwrap on Invalid: {}", error),
        }
    }

    /// Returns the request if valid, or `None` if invalid.
    pub fn ok(self) -> Option<ParsedRequest> {
        match self {
            Self::Valid(request) => Some(request),
            Self::Invalid(_) => None,
        }
    }

    /// Returns the error if invalid, or `None` if valid.
    pub fn err(self) -> Option<ValidationError> {
        match self {
            Self::Valid(_) => None,
            Self::Invalid(error) => Some(error),
        }
    }
}

/// Trait for request validators.
///
/// Validators are run sequentially on each request. Each validator can:
/// - Pass the request through unchanged
/// - Transform the request before passing it to the next validator
/// - Reject the request with an error
pub trait Validator: Send + Sync + 'static {
    /// Validate and optionally transform a request.
    ///
    /// Returns `ValidationResult::Valid(request)` to pass the request through
    /// (possibly modified), or `ValidationResult::Invalid(error)` to reject it.
    fn validate(&self, request: ParsedRequest) -> ValidationResult;

    /// Human-readable name for this validator.
    fn name(&self) -> &str;
}

/// A chain of validators that run in sequence.
///
/// When `validate` is called, each validator is run in order. If any validator
/// returns `Invalid`, the chain short-circuits and returns that error. Otherwise,
/// the (possibly modified) request is passed to the next validator.
#[derive(Default)]
pub struct ValidatorChain {
    validators: Vec<Arc<dyn Validator>>,
}

impl std::fmt::Debug for ValidatorChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidatorChain")
            .field("validators", &format!("{} validators", self.validators.len()))
            .finish()
    }
}

impl ValidatorChain {
    /// Create a new empty validator chain.
    pub fn new() -> Self {
        Self { validators: Vec::new() }
    }

    /// Add a validator to the chain.
    ///
    /// Validators are run in the order they are added.
    #[must_use]
    pub fn with<V: Validator>(mut self, validator: V) -> Self {
        self.validators.push(Arc::new(validator));
        self
    }

    /// Add a validator to the chain (alias for `with`).
    ///
    /// Validators are run in the order they are added.
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn add<V: Validator>(self, validator: V) -> Self {
        self.with(validator)
    }

    /// Add a pre-built `Arc<dyn Validator>` to the chain.
    #[must_use]
    pub fn with_arc(mut self, validator: Arc<dyn Validator>) -> Self {
        self.validators.push(validator);
        self
    }

    /// Add a pre-built `Arc<dyn Validator>` to the chain (alias for `with_arc`).
    #[must_use]
    pub fn add_arc(self, validator: Arc<dyn Validator>) -> Self {
        self.with_arc(validator)
    }

    /// Run all validators on a request.
    ///
    /// Returns `Valid` with the (possibly modified) request if all validators pass,
    /// or `Invalid` with the first error encountered.
    pub fn validate(&self, request: ParsedRequest) -> ValidationResult {
        let mut current_request = request;

        for validator in &self.validators {
            match validator.validate(current_request) {
                ValidationResult::Valid(modified_request) => {
                    current_request = modified_request;
                }
                ValidationResult::Invalid(error) => {
                    tracing::debug!(
                        validator = validator.name(),
                        error = %error,
                        "request rejected by validator"
                    );
                    return ValidationResult::Invalid(error);
                }
            }
        }

        ValidationResult::Valid(current_request)
    }

    /// Returns the number of validators in the chain.
    pub fn len(&self) -> usize {
        self.validators.len()
    }

    /// Returns `true` if the chain has no validators.
    pub fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }
}

/// Validator that blocks specific methods.
///
/// Requests for methods in the blocklist will be rejected with a "method not allowed" error.
#[derive(Debug, Clone)]
pub struct MethodBlocklist {
    blocked: Vec<String>,
}

impl MethodBlocklist {
    /// Create a new method blocklist validator.
    pub const fn new(methods: Vec<String>) -> Self {
        Self { blocked: methods }
    }

    /// Check if a method is blocked.
    pub fn is_blocked(&self, method: &str) -> bool {
        self.blocked.iter().any(|m| m == method)
    }
}

impl Validator for MethodBlocklist {
    fn validate(&self, request: ParsedRequest) -> ValidationResult {
        if self.is_blocked(request.method()) {
            ValidationResult::Invalid(ValidationError::method_not_allowed(request.method()))
        } else {
            ValidationResult::Valid(request)
        }
    }

    fn name(&self) -> &str {
        "MethodBlocklist"
    }
}

/// Validator that only allows specific methods.
///
/// Requests for methods not in the allowlist will be rejected with a "method not allowed" error.
#[derive(Debug, Clone)]
pub struct MethodAllowlist {
    allowed: Vec<String>,
}

impl MethodAllowlist {
    /// Create a new method allowlist validator.
    pub const fn new(methods: Vec<String>) -> Self {
        Self { allowed: methods }
    }

    /// Check if a method is allowed.
    pub fn is_allowed(&self, method: &str) -> bool {
        self.allowed.iter().any(|m| m == method)
    }
}

impl Validator for MethodAllowlist {
    fn validate(&self, request: ParsedRequest) -> ValidationResult {
        if self.is_allowed(request.method()) {
            ValidationResult::Valid(request)
        } else {
            ValidationResult::Invalid(ValidationError::method_not_allowed(request.method()))
        }
    }

    fn name(&self) -> &str {
        "MethodAllowlist"
    }
}

/// Validator that rejects requests with too many parameters.
#[derive(Debug, Clone)]
pub struct MaxParamsValidator {
    max_params: usize,
}

impl MaxParamsValidator {
    /// Create a new max params validator.
    pub const fn new(max_params: usize) -> Self {
        Self { max_params }
    }
}

impl Validator for MaxParamsValidator {
    fn validate(&self, request: ParsedRequest) -> ValidationResult {
        // Get params count from the raw params
        let param_count = request.params.as_ref().map_or(0, |raw| {
            serde_json::from_str::<serde_json::Value>(raw.get()).ok().map_or(0, |value| {
                value
                    .as_array()
                    .map_or_else(|| if value.is_null() { 0 } else { 1 }, std::vec::Vec::len)
            })
        });

        if param_count > self.max_params {
            ValidationResult::Invalid(ValidationError::invalid_params(format!(
                "too many parameters: got {}, max {}",
                param_count, self.max_params
            )))
        } else {
            ValidationResult::Valid(request)
        }
    }

    fn name(&self) -> &str {
        "MaxParamsValidator"
    }
}

/// A no-op validator that always passes requests through unchanged.
///
/// Useful for testing or as a placeholder.
#[derive(Debug, Clone, Default)]
pub struct NoopValidator;

impl NoopValidator {
    /// Create a new no-op validator.
    pub const fn new() -> Self {
        Self
    }
}

impl Validator for NoopValidator {
    fn validate(&self, request: ParsedRequest) -> ValidationResult {
        ValidationResult::Valid(request)
    }

    fn name(&self) -> &str {
        "NoopValidator"
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::{json, value::RawValue};

    use super::*;

    /// Create a test request with the given method and params.
    fn make_request(method: &str, params: serde_json::Value) -> ParsedRequest {
        let params_str = serde_json::to_string(&params).expect("valid params");
        let raw_params = RawValue::from_string(params_str).expect("valid raw value");
        ParsedRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(raw_params),
            id: Some(serde_json::Value::Number(1.into())),
        }
    }

    /// Create a test request with the given method and no params.
    fn make_request_no_params(method: &str) -> ParsedRequest {
        make_request(method, json!([]))
    }

    // ============ ValidationError Tests ============

    #[test]
    fn test_validation_error_new() {
        let err = ValidationError::new(-32600, "test error");
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "test error");
    }

    #[test]
    fn test_validation_error_method_not_allowed() {
        let err = ValidationError::method_not_allowed("eth_forbidden");
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("eth_forbidden"));
    }

    #[test]
    fn test_validation_error_invalid_params() {
        let err = ValidationError::invalid_params("missing required param");
        assert_eq!(err.code, -32602);
        assert_eq!(err.message, "missing required param");
    }

    #[test]
    fn test_validation_error_display() {
        let err = ValidationError::new(-32600, "test error");
        let display = format!("{}", err);
        assert!(display.contains("validation error"));
        assert!(display.contains("test error"));
    }

    #[test]
    fn test_validation_error_debug() {
        let err = ValidationError::new(-32600, "test error");
        let debug = format!("{:?}", err);
        assert!(debug.contains("ValidationError"));
    }

    // ============ ValidationResult Tests ============

    #[test]
    fn test_validation_result_is_valid() {
        let request = make_request_no_params("eth_blockNumber");
        let result = ValidationResult::Valid(request);
        assert!(result.is_valid());
        assert!(!result.is_invalid());
    }

    #[test]
    fn test_validation_result_is_invalid() {
        let err = ValidationError::new(-32600, "test");
        let result = ValidationResult::Invalid(err);
        assert!(!result.is_valid());
        assert!(result.is_invalid());
    }

    #[test]
    fn test_validation_result_unwrap_valid() {
        let request = make_request_no_params("eth_blockNumber");
        let result = ValidationResult::Valid(request);
        let unwrapped = result.unwrap();
        assert_eq!(unwrapped.method(), "eth_blockNumber");
    }

    #[test]
    #[should_panic(expected = "called unwrap on Invalid")]
    fn test_validation_result_unwrap_invalid_panics() {
        let err = ValidationError::new(-32600, "test");
        let result = ValidationResult::Invalid(err);
        let _ = result.unwrap();
    }

    #[test]
    fn test_validation_result_ok() {
        let request = make_request_no_params("eth_blockNumber");
        let result = ValidationResult::Valid(request);
        assert!(result.ok().is_some());

        let err = ValidationError::new(-32600, "test");
        let result = ValidationResult::Invalid(err);
        assert!(result.ok().is_none());
    }

    #[test]
    fn test_validation_result_err() {
        let request = make_request_no_params("eth_blockNumber");
        let result = ValidationResult::Valid(request);
        assert!(result.err().is_none());

        let err = ValidationError::new(-32600, "test");
        let result = ValidationResult::Invalid(err);
        assert!(result.err().is_some());
    }

    // ============ ValidatorChain Tests ============

    #[test]
    fn test_validator_chain_new_is_empty() {
        let chain = ValidatorChain::new();
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
    }

    #[test]
    fn test_validator_chain_default() {
        let chain = ValidatorChain::default();
        assert!(chain.is_empty());
    }

    #[test]
    fn test_validator_chain_add() {
        let chain = ValidatorChain::new().add(NoopValidator::new()).add(NoopValidator::new());
        assert_eq!(chain.len(), 2);
        assert!(!chain.is_empty());
    }

    #[test]
    fn test_validator_chain_add_arc() {
        let validator: Arc<dyn Validator> = Arc::new(NoopValidator::new());
        let chain = ValidatorChain::new().add_arc(validator);
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn test_validator_chain_empty_passes_all() {
        let chain = ValidatorChain::new();
        let request = make_request_no_params("eth_blockNumber");
        let result = chain.validate(request);
        assert!(result.is_valid());
    }

    #[test]
    fn test_validator_chain_noop_passes() {
        let chain = ValidatorChain::new().add(NoopValidator::new()).add(NoopValidator::new());
        let request = make_request_no_params("eth_blockNumber");
        let result = chain.validate(request);
        assert!(result.is_valid());
    }

    #[test]
    fn test_validator_chain_debug() {
        let chain = ValidatorChain::new().add(NoopValidator::new());
        let debug = format!("{:?}", chain);
        assert!(debug.contains("ValidatorChain"));
        assert!(debug.contains("1 validators"));
    }

    // ============ MethodBlocklist Tests ============

    #[rstest]
    #[case::single_blocked("eth_sendTransaction", vec!["eth_sendTransaction".to_string()], true)]
    #[case::not_blocked("eth_blockNumber", vec!["eth_sendTransaction".to_string()], false)]
    #[case::multiple_blocked("eth_sendRawTransaction", vec!["eth_sendTransaction".to_string(), "eth_sendRawTransaction".to_string()], true)]
    #[case::empty_blocklist("eth_anything", vec![], false)]
    fn test_method_blocklist_is_blocked(
        #[case] method: &str,
        #[case] blocked: Vec<String>,
        #[case] expected_blocked: bool,
    ) {
        let blocklist = MethodBlocklist::new(blocked);
        assert_eq!(blocklist.is_blocked(method), expected_blocked);
    }

    #[rstest]
    #[case::allowed_method("eth_blockNumber", vec!["eth_sendTransaction".to_string()], true)]
    #[case::blocked_method("eth_sendTransaction", vec!["eth_sendTransaction".to_string()], false)]
    fn test_method_blocklist_validate(
        #[case] method: &str,
        #[case] blocked: Vec<String>,
        #[case] expected_valid: bool,
    ) {
        let blocklist = MethodBlocklist::new(blocked);
        let request = make_request_no_params(method);
        let result = blocklist.validate(request);
        assert_eq!(result.is_valid(), expected_valid);
    }

    #[test]
    fn test_method_blocklist_name() {
        let blocklist = MethodBlocklist::new(vec![]);
        assert_eq!(blocklist.name(), "MethodBlocklist");
    }

    #[test]
    fn test_method_blocklist_debug() {
        let blocklist = MethodBlocklist::new(vec!["eth_send".to_string()]);
        let debug = format!("{:?}", blocklist);
        assert!(debug.contains("MethodBlocklist"));
        assert!(debug.contains("eth_send"));
    }

    // ============ MethodAllowlist Tests ============

    #[rstest]
    #[case::allowed("eth_blockNumber", vec!["eth_blockNumber".to_string()], true)]
    #[case::not_allowed("eth_sendTransaction", vec!["eth_blockNumber".to_string()], false)]
    #[case::multiple_allowed("eth_chainId", vec!["eth_blockNumber".to_string(), "eth_chainId".to_string()], true)]
    #[case::empty_allowlist("eth_anything", vec![], false)]
    fn test_method_allowlist_is_allowed(
        #[case] method: &str,
        #[case] allowed: Vec<String>,
        #[case] expected_allowed: bool,
    ) {
        let allowlist = MethodAllowlist::new(allowed);
        assert_eq!(allowlist.is_allowed(method), expected_allowed);
    }

    #[rstest]
    #[case::allowed_method("eth_blockNumber", vec!["eth_blockNumber".to_string()], true)]
    #[case::not_allowed_method("eth_sendTransaction", vec!["eth_blockNumber".to_string()], false)]
    fn test_method_allowlist_validate(
        #[case] method: &str,
        #[case] allowed: Vec<String>,
        #[case] expected_valid: bool,
    ) {
        let allowlist = MethodAllowlist::new(allowed);
        let request = make_request_no_params(method);
        let result = allowlist.validate(request);
        assert_eq!(result.is_valid(), expected_valid);
    }

    #[test]
    fn test_method_allowlist_name() {
        let allowlist = MethodAllowlist::new(vec![]);
        assert_eq!(allowlist.name(), "MethodAllowlist");
    }

    #[test]
    fn test_method_allowlist_debug() {
        let allowlist = MethodAllowlist::new(vec!["eth_call".to_string()]);
        let debug = format!("{:?}", allowlist);
        assert!(debug.contains("MethodAllowlist"));
        assert!(debug.contains("eth_call"));
    }

    // ============ MaxParamsValidator Tests ============

    #[rstest]
    #[case::zero_params(0, json!([]), true)]
    #[case::under_limit(3, json!(["a", "b"]), true)]
    #[case::at_limit(2, json!(["a", "b"]), true)]
    #[case::over_limit(1, json!(["a", "b"]), false)]
    #[case::null_params(0, json!(null), true)]
    fn test_max_params_validator(
        #[case] max: usize,
        #[case] params: serde_json::Value,
        #[case] expected_valid: bool,
    ) {
        let validator = MaxParamsValidator::new(max);
        let request = make_request("eth_test", params);
        let result = validator.validate(request);
        assert_eq!(result.is_valid(), expected_valid, "max={}, valid={}", max, expected_valid);
    }

    #[test]
    fn test_max_params_validator_name() {
        let validator = MaxParamsValidator::new(10);
        assert_eq!(validator.name(), "MaxParamsValidator");
    }

    #[test]
    fn test_max_params_validator_debug() {
        let validator = MaxParamsValidator::new(5);
        let debug = format!("{:?}", validator);
        assert!(debug.contains("MaxParamsValidator"));
        assert!(debug.contains("5"));
    }

    // ============ NoopValidator Tests ============

    #[test]
    fn test_noop_validator_always_passes() {
        let validator = NoopValidator::new();
        let request = make_request("any_method", json!(["any", "params"]));
        let result = validator.validate(request);
        assert!(result.is_valid());
    }

    #[test]
    fn test_noop_validator_name() {
        let validator = NoopValidator::new();
        assert_eq!(validator.name(), "NoopValidator");
    }

    #[test]
    fn test_noop_validator_default() {
        let validator = NoopValidator::default();
        let request = make_request_no_params("eth_test");
        let result = validator.validate(request);
        assert!(result.is_valid());
    }

    // ============ Chain Integration Tests ============

    #[test]
    fn test_chain_blocklist_then_allowlist() {
        // Blocklist takes precedence as it comes first
        let chain = ValidatorChain::new()
            .add(MethodBlocklist::new(vec!["eth_sendTransaction".to_string()]))
            .add(MethodAllowlist::new(vec![
                "eth_blockNumber".to_string(),
                "eth_sendTransaction".to_string(),
            ]));

        // eth_blockNumber should pass both
        let request = make_request_no_params("eth_blockNumber");
        assert!(chain.validate(request).is_valid());

        // eth_sendTransaction is blocked before allowlist check
        let request = make_request_no_params("eth_sendTransaction");
        assert!(chain.validate(request).is_invalid());

        // eth_call would fail at allowlist
        let request = make_request_no_params("eth_call");
        assert!(chain.validate(request).is_invalid());
    }

    #[test]
    fn test_chain_with_max_params() {
        let chain = ValidatorChain::new()
            .add(MethodAllowlist::new(vec!["eth_call".to_string()]))
            .add(MaxParamsValidator::new(2));

        // Valid method with valid params
        let request = make_request("eth_call", json!(["0x1", "latest"]));
        assert!(chain.validate(request).is_valid());

        // Valid method with too many params
        let request = make_request("eth_call", json!(["0x1", "latest", "extra"]));
        assert!(chain.validate(request).is_invalid());

        // Invalid method (fails before param check)
        let request = make_request("eth_forbidden", json!([]));
        assert!(chain.validate(request).is_invalid());
    }

    #[test]
    fn test_chain_short_circuits_on_first_error() {
        // This test ensures that once a validator fails, subsequent validators are not called
        let chain = ValidatorChain::new()
            .add(MethodBlocklist::new(vec!["eth_blocked".to_string()]))
            .add(MaxParamsValidator::new(0)); // Would fail if it were reached with any params

        // This request has params but is blocked before param check
        let request = make_request("eth_blocked", json!(["param1", "param2"]));
        let result = chain.validate(request);
        assert!(result.is_invalid());

        // Error should be from blocklist, not max params
        let err = result.err().unwrap();
        assert_eq!(err.code, -32601); // method not allowed, not invalid params
    }

    #[rstest]
    #[case::read_methods(
        vec!["eth_blockNumber", "eth_chainId", "eth_getBalance"],
        "eth_blockNumber",
        true
    )]
    #[case::write_methods_blocked(
        vec!["eth_blockNumber", "eth_chainId", "eth_getBalance"],
        "eth_sendTransaction",
        false
    )]
    fn test_allowlist_common_patterns(
        #[case] allowed: Vec<&str>,
        #[case] method: &str,
        #[case] expected_valid: bool,
    ) {
        let allowlist = MethodAllowlist::new(allowed.into_iter().map(String::from).collect());
        let request = make_request_no_params(method);
        let result = allowlist.validate(request);
        assert_eq!(result.is_valid(), expected_valid);
    }

    #[rstest]
    #[case::block_admin(
        vec!["admin_addPeer", "admin_removePeer", "admin_nodeInfo"],
        "admin_addPeer",
        false
    )]
    #[case::allow_standard(
        vec!["admin_addPeer", "admin_removePeer", "admin_nodeInfo"],
        "eth_blockNumber",
        true
    )]
    fn test_blocklist_common_patterns(
        #[case] blocked: Vec<&str>,
        #[case] method: &str,
        #[case] expected_valid: bool,
    ) {
        let blocklist = MethodBlocklist::new(blocked.into_iter().map(String::from).collect());
        let request = make_request_no_params(method);
        let result = blocklist.validate(request);
        assert_eq!(result.is_valid(), expected_valid);
    }
}
