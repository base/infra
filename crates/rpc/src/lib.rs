#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/refcell/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

#[macro_use]
extern crate tracing;

use alloy_json_rpc as _;
use roxy_types as _;

mod codec;
pub use codec::{
    JsonRpcError, ParsedRequest, ParsedRequestPacket, ParsedResponse, ParsedResponsePacket,
    RpcCodec,
};

mod rate_limiter;
pub use rate_limiter::{RateLimiterConfig, SlidingWindowRateLimiter};

pub mod router;
pub use router::{MethodRouter, RouteTarget};

mod validator;
pub use validator::{
    MaxParamsValidator, MethodAllowlist, MethodBlocklist, NoopValidator, ValidationError,
    ValidationResult, Validator, ValidatorChain,
};
