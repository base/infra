#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

// Re-export alloy JSON-RPC types
pub use alloy_json_rpc::{
    ErrorPayload, Id, Request, RequestMeta, RequestPacket, Response, ResponsePacket,
    ResponsePayload, RpcError,
};
// Re-export alloy primitives for block types
pub use alloy_primitives::{Address, B256, BlockHash, BlockNumber};

// Export our custom error type
mod error;
pub use error::{RoxyError, error_codes};
