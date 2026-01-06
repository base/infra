#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

// Placeholder imports for dependencies that will be used when this crate is fully implemented
use tokio as _;
use tracing as _;

mod memory;
pub use memory::MemoryCache;
