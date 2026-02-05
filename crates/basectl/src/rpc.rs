use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::BlockNumberOrTag;
use anyhow::Result;

const DEFAULT_ELASTICITY: u64 = 6;

/// Fetch the EIP-1559 elasticity multiplier from the L2 block extraData.
/// Falls back to default (6) if extraData is not in Holocene format.
pub async fn fetch_elasticity(rpc_url: &str) -> Result<u64> {
    let provider = ProviderBuilder::new().connect(rpc_url).await?;

    let block = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| anyhow::anyhow!("No block found"))?;

    let extra_data = &block.header.extra_data;

    // Holocene format: version(1) + denominator(4) + elasticity(4) = 9 bytes
    if extra_data.len() >= 9 && extra_data[0] == 0 {
        let elasticity = u32::from_be_bytes([
            extra_data[5],
            extra_data[6],
            extra_data[7],
            extra_data[8],
        ]);
        Ok(elasticity as u64)
    } else {
        // Pre-Holocene or invalid format, use default
        Ok(DEFAULT_ELASTICITY)
    }
}
