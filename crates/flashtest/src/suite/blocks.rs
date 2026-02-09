//! Tests for block retrieval and state visibility.

use std::time::Duration;

use alloy_eips::BlockNumberOrTag;
use alloy_primitives::U256;
use eyre::{Result, ensure};

use crate::{
    TestClient,
    harness::FlashblockHarness,
    suite::{Test, TestCategory, skip_if_no_signer_or_recipient},
};

/// Build the blocks test category.
pub(crate) fn category() -> TestCategory {
    TestCategory {
        name: "blocks".to_string(),
        description: Some("Block retrieval and pending state tests".to_string()),
        tests: vec![
            Test {
                name: "get_latest_block".to_string(),
                description: Some("Verify we can retrieve the latest block".to_string()),
                run: Box::new(|client| Box::pin(test_get_latest_block(client))),
                skip_if: None,
            },
            Test {
                name: "get_pending_block".to_string(),
                description: Some("Verify we can retrieve the pending block".to_string()),
                run: Box::new(|client| Box::pin(test_get_pending_block(client))),
                skip_if: None,
            },
            Test {
                name: "pending_block_number_gt_latest".to_string(),
                description: Some("Pending block number should be >= latest".to_string()),
                run: Box::new(|client| Box::pin(test_pending_block_number(client))),
                skip_if: None,
            },
            Test {
                name: "flashblock_balance_change".to_string(),
                description: Some(
                    "Send tx and verify balance change visible in pending state within same block"
                        .to_string(),
                ),
                run: Box::new(|client| Box::pin(test_flashblock_balance_change(client))),
                skip_if: Some(Box::new(|client| {
                    Box::pin(async move { skip_if_no_signer_or_recipient(client) })
                })),
            },
            Test {
                name: "flashblock_nonce_change".to_string(),
                description: Some(
                    "Send tx and verify nonce change visible in pending state within same block"
                        .to_string(),
                ),
                run: Box::new(|client| Box::pin(test_flashblock_nonce_change(client))),
                skip_if: Some(Box::new(|client| {
                    Box::pin(async move { skip_if_no_signer_or_recipient(client) })
                })),
            },
        ],
    }
}

async fn test_get_latest_block(client: &TestClient) -> Result<()> {
    let block = client.get_block_by_number(BlockNumberOrTag::Latest).await?;
    ensure!(block.is_some(), "Latest block should exist");

    let block = block.unwrap();
    tracing::debug!(number = block.header.number, "Got latest block");

    Ok(())
}

async fn test_get_pending_block(client: &TestClient) -> Result<()> {
    let block = client.get_block_by_number(BlockNumberOrTag::Pending).await?;
    ensure!(block.is_some(), "Pending block should exist");

    let block = block.unwrap();
    tracing::debug!(number = block.header.number, "Got pending block");

    Ok(())
}

async fn test_pending_block_number(client: &TestClient) -> Result<()> {
    let latest = client
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| eyre::eyre!("No latest block"))?;

    let pending = client
        .get_block_by_number(BlockNumberOrTag::Pending)
        .await?
        .ok_or_else(|| eyre::eyre!("No pending block"))?;

    ensure!(
        pending.header.number >= latest.header.number,
        "Pending block number ({}) should be >= latest ({})",
        pending.header.number,
        latest.header.number
    );

    Ok(())
}

async fn test_flashblock_balance_change(client: &TestClient) -> Result<()> {
    let from = client.signer_address().ok_or_else(|| eyre::eyre!("No signer"))?;

    let mut harness = FlashblockHarness::new(client).await?;
    let block_number = harness.block_number();

    let balance_before = client.get_balance(from, BlockNumberOrTag::Pending).await?;
    tracing::debug!(?balance_before, block = block_number, "Balance before tx");

    let value = U256::from(1u64);
    let recipient = client.recipient().ok_or_else(|| eyre::eyre!("No recipient configured"))?;
    let (tx_bytes, tx_hash) = client.create_transfer(recipient, value, None).await?;

    tracing::info!(?tx_hash, "Sending transaction");
    client.send_raw_transaction(tx_bytes).await?;

    harness.wait_for_tx(tx_hash, Duration::from_secs(10)).await?;

    let balance_after = client.get_balance(from, BlockNumberOrTag::Pending).await?;
    tracing::debug!(?balance_after, "Balance after flashblock tx");

    ensure!(
        balance_after < balance_before,
        "Pending balance should decrease after flashblock tx: before={}, after={}",
        balance_before,
        balance_after
    );

    harness.assert_same_block(block_number)?;

    tracing::info!(
        block = block_number,
        flashblocks = harness.flashblock_count(),
        "Balance change verified in pending state within flashblock window"
    );

    harness.close().await?;
    Ok(())
}

async fn test_flashblock_nonce_change(client: &TestClient) -> Result<()> {
    let from = client.signer_address().ok_or_else(|| eyre::eyre!("No signer"))?;

    let mut harness = FlashblockHarness::new(client).await?;
    let block_number = harness.block_number();

    let nonce_before = client.get_transaction_count(from, BlockNumberOrTag::Pending).await?;
    tracing::debug!(nonce_before, block = block_number, "Nonce before tx");

    let value = U256::from(1u64);
    let recipient = client.recipient().ok_or_else(|| eyre::eyre!("No recipient configured"))?;
    let (tx_bytes, tx_hash) = client.create_transfer(recipient, value, Some(nonce_before)).await?;

    tracing::info!(?tx_hash, "Sending transaction");
    client.send_raw_transaction(tx_bytes).await?;

    harness.wait_for_tx(tx_hash, Duration::from_secs(10)).await?;

    let nonce_after = client.get_transaction_count(from, BlockNumberOrTag::Pending).await?;
    tracing::debug!(nonce_after, "Nonce after flashblock tx");

    ensure!(
        nonce_after == nonce_before + 1,
        "Pending nonce should increment after flashblock tx: before={}, after={}",
        nonce_before,
        nonce_after
    );

    harness.assert_same_block(block_number)?;

    tracing::info!(
        block = block_number,
        flashblocks = harness.flashblock_count(),
        "Nonce change verified in pending state within flashblock window"
    );

    harness.close().await?;
    Ok(())
}
