#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/refcell/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use tokio as _;
use tracing as _;

mod compressed;
pub use compressed::CompressedCache;

mod fallback;
pub use fallback::FallbackCache;

mod memory;
pub use memory::MemoryCache;

mod redis;
pub use redis::RedisCache;

mod rmap;
pub use rmap::RMap;

mod rpc_cache;
pub use rpc_cache::{CachePolicy, RpcCache};
