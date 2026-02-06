use std::time::Duration;

use alloy_network::EthereumWallet;
use alloy_provider::{
    Identity, ProviderBuilder, RootProvider,
    fillers::{FillProvider, JoinFill, WalletFiller},
};
use alloy_rpc_client::RpcClient;
use alloy_transport_http::Http;
use anyhow::{Context, Result};
use op_alloy_network::Optimism;

/// Concrete provider type for read-only operations on Optimism
pub(crate) type Provider = RootProvider<Optimism>;

/// Concrete provider type with wallet for signing transactions on Optimism
pub(crate) type WalletProvider =
    FillProvider<JoinFill<Identity, WalletFiller<EthereumWallet>>, Provider, Optimism>;

/// Creates a shared HTTP client with connection pooling configured to reduce DNS pressure.
pub(crate) fn create_shared_client() -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(100)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()
        .expect("Failed to build HTTP client")
}

/// Creates a provider without a wallet for read-only operations
pub(crate) fn create_provider(http_client: reqwest::Client, rpc_url: &str) -> Result<Provider> {
    let url: url::Url = rpc_url.parse().context("Invalid RPC URL")?;
    let http = Http::with_client(http_client, url);
    let rpc_client = RpcClient::new(http, true);
    Ok(RootProvider::new(rpc_client))
}

/// Creates a provider with an Ethereum wallet for signing transactions
pub(crate) fn create_wallet_provider(
    http_client: reqwest::Client,
    rpc_url: &str,
    wallet: EthereumWallet,
) -> Result<WalletProvider> {
    let url: url::Url = rpc_url.parse().context("Invalid RPC URL")?;
    let http = Http::with_client(http_client, url);
    let rpc_client = RpcClient::new(http, true);
    let root: Provider = RootProvider::new(rpc_client);
    // Use filler() to add only the wallet filler without default fillers
    Ok(ProviderBuilder::default().filler(WalletFiller::new(wallet)).connect_provider(root))
}
