#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use roxy_rpc::RpcCodec;
use roxy_traits::DefaultCodecConfig;

#[derive(Arbitrary, Debug)]
struct FuzzInput {
    requests: Vec<FuzzRequest>,
}

#[derive(Arbitrary, Debug)]
struct FuzzRequest {
    method: String,
    params: Option<String>,
    id: Option<u64>,
}

impl FuzzInput {
    fn to_json(&self) -> String {
        if self.requests.is_empty() {
            return "[]".to_string();
        }

        if self.requests.len() == 1 {
            return self.requests[0].to_json();
        }

        let items: Vec<String> = self.requests.iter().map(|r| r.to_json()).collect();
        format!("[{}]", items.join(","))
    }
}

impl FuzzRequest {
    fn to_json(&self) -> String {
        let id_str = self.id.map(|i| format!(",\"id\":{}", i)).unwrap_or_default();
        let params_str = self
            .params
            .as_ref()
            .map(|p| format!(",\"params\":{}", p))
            .unwrap_or_default();

        format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"{}\"{}{}}}",
            self.method, params_str, id_str
        )
    }
}

fuzz_target!(|input: FuzzInput| {
    let codec = RpcCodec::new(
        DefaultCodecConfig::new()
            .with_max_size(1024 * 1024)
            .with_max_depth(64)
            .with_max_batch_size(100),
    );

    let json = input.to_json();
    let _ = codec.decode(json.as_bytes());
});
