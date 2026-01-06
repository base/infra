#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

// Placeholder imports for dependencies that will be used when this crate is fully implemented
use alloy_json_rpc as _;
use roxy_types as _;
use tracing as _;

mod codec;
pub use codec::{
    JsonRpcError, ParsedRequest, ParsedRequestPacket, ParsedResponse, ParsedResponsePacket,
    RpcCodec,
};

mod rate_limiter;
pub use rate_limiter::{RateLimiterConfig, SlidingWindowRateLimiter};

mod router;
pub use router::{MethodRouter, RouteTarget};

mod validator;
pub use validator::{
    MaxParamsValidator, MethodAllowlist, MethodBlocklist, NoopValidator, ValidationError,
    ValidationResult, Validator, ValidatorChain,
};
