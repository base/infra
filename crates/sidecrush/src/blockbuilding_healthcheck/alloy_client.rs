use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_types_eth::BlockId;
use async_trait::async_trait;

use super::{EthClient, HeaderSummary};

#[derive(Clone)]
pub struct AlloyEthClient {
    provider: RootProvider,
}

impl AlloyEthClient {
    pub fn new_http(url: &str) -> anyhow::Result<Self> {
        let provider =
            ProviderBuilder::new().disable_recommended_fillers().connect_http(url.parse()?);
        Ok(Self { provider })
    }

    async fn get_block_header(
        &self,
        block_id: BlockId,
    ) -> Result<HeaderSummary, Box<dyn std::error::Error + Send + Sync>> {
        let block = self
            .provider
            .get_block(block_id)
            .hashes()
            .await?
            .ok_or_else(|| format!("block {:?} not found", block_id))?;

        let number: u64 = block.header.number;
        let timestamp_unix_seconds: u64 = block.header.timestamp;
        let transaction_count: usize = block.transactions.len();
        let hash: [u8; 32] = block.header.hash.into();

        Ok(HeaderSummary { number, timestamp_unix_seconds, transaction_count, hash: Some(hash) })
    }
}

#[async_trait]
impl EthClient for AlloyEthClient {
    async fn latest_header(
        &self,
    ) -> Result<HeaderSummary, Box<dyn std::error::Error + Send + Sync>> {
        self.get_block_header(BlockId::latest()).await
    }

    async fn header_by_number(
        &self,
        number: u64,
    ) -> Result<HeaderSummary, Box<dyn std::error::Error + Send + Sync>> {
        self.get_block_header(BlockId::number(number)).await
    }
}
