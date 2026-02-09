#![no_main]

use libfuzzer_sys::fuzz_target;
use roxy_rpc::RpcCodec;
use roxy_traits::DefaultCodecConfig;

fuzz_target!(|data: &[u8]| {
    let codec = RpcCodec::new(
        DefaultCodecConfig::new()
            .with_max_size(1024 * 1024)
            .with_max_depth(64)
            .with_max_batch_size(100),
    );

    // The codec should never panic on any input
    let _ = codec.decode(data);
});
