#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

// Placeholder imports for dependencies that will be used when this crate is implemented
use alloy_json_rpc as _;
use axum as _;
use hyper as _;
use roxy_backend as _;
use roxy_traits as _;
use roxy_types as _;
use serde as _;
use serde_json as _;
use tokio as _;
use tower as _;
use tracing as _;

// Server modules will be added here
