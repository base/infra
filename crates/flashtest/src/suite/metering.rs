//! Tests for metering RPC endpoints.
//!
//! These tests require the node to support the `base_meterBundle` RPC method.
//! Not all nodes have this.

use alloy_eips::BlockNumberOrTag;
use alloy_network::TransactionBuilder;
use alloy_primitives::{Address, B256, U256};
use eyre::{Result, ensure, eyre};
use op_alloy_rpc_types::OpTransactionRequest;

use crate::{
    TestClient,
    simulator::{SimulatorConfigBuilder, encode_run_call},
    suite::{Test, TestCategory, skip_if_no_signer_or_recipient, skip_if_no_signer_or_simulator},
    types::Bundle,
};

/// Check if the node supports metering RPC methods.
async fn check_metering_support(client: &TestClient) -> Option<String> {
    let bundle = Bundle { block_number: 1, ..Default::default() };
    match client.meter_bundle(bundle).await {
        Ok(_) => None,
        Err(e) => {
            let err_str = format!("{e:?}");
            if err_str.contains("-32601") || err_str.contains("Method not found") {
                Some("Node does not support base_meterBundle RPC method".to_string())
            } else {
                None
            }
        }
    }
}

/// Build the metering test category.
pub(crate) fn category() -> TestCategory {
    TestCategory {
        name: "metering".to_string(),
        description: Some(
            "Bundle metering and priority fee estimation tests (requires base_meterBundle support)"
                .to_string(),
        ),
        tests: vec![
            Test {
                name: "meter_bundle_empty".to_string(),
                description: Some("Meter an empty bundle".to_string()),
                run: Box::new(|client| Box::pin(test_meter_bundle_empty(client))),
                skip_if: Some(Box::new(|client| Box::pin(check_metering_support(client)))),
            },
            Test {
                name: "meter_bundle_state_block".to_string(),
                description: Some("Verify metering returns valid state block".to_string()),
                run: Box::new(|client| Box::pin(test_meter_bundle_state_block(client))),
                skip_if: Some(Box::new(|client| Box::pin(check_metering_support(client)))),
            },
            Test {
                name: "meter_bundle_with_transaction".to_string(),
                description: Some("Meter a bundle with a real transaction".to_string()),
                run: Box::new(|client| Box::pin(test_meter_bundle_with_transaction(client))),
                skip_if: Some(Box::new(|client| {
                    Box::pin(async move {
                        if let Some(reason) = check_metering_support(client).await {
                            return Some(reason);
                        }
                        skip_if_no_signer_or_recipient(client)
                    })
                })),
            },
            Test {
                name: "meter_bundle_flashblock_index".to_string(),
                description: Some(
                    "Verify metering returns flashblock index when available".to_string(),
                ),
                run: Box::new(|client| Box::pin(test_meter_bundle_flashblock_index(client))),
                skip_if: Some(Box::new(|client| Box::pin(check_metering_support(client)))),
            },
            Test {
                name: "meter_bundle_state_root_timing".to_string(),
                description: Some(
                    "Verify metering returns state root timing (ETH transfer)".to_string(),
                ),
                run: Box::new(|client| Box::pin(test_meter_bundle_state_root_timing(client))),
                skip_if: Some(Box::new(|client| {
                    Box::pin(async move {
                        if let Some(reason) = check_metering_support(client).await {
                            return Some(reason);
                        }
                        skip_if_no_signer_or_recipient(client)
                    })
                })),
            },
            Test {
                name: "meter_bundle_state_root_timing_simulator".to_string(),
                description: Some(
                    "Verify metering returns state root timing (Simulator contract)".to_string(),
                ),
                run: Box::new(|client| {
                    Box::pin(test_meter_bundle_state_root_timing_simulator(client))
                }),
                skip_if: Some(Box::new(|client| {
                    Box::pin(async move {
                        if let Some(reason) = check_metering_support(client).await {
                            return Some(reason);
                        }
                        skip_if_no_signer_or_simulator(client)
                    })
                })),
            },
            Test {
                name: "meter_bundle_high_state_root_time".to_string(),
                description: Some(
                    "Meter bundle with high state root time (~400 accounts)".to_string(),
                ),
                run: Box::new(|client| Box::pin(test_meter_bundle_high_state_root_time(client))),
                skip_if: Some(Box::new(|client| {
                    Box::pin(async move {
                        if let Some(reason) = check_metering_support(client).await {
                            return Some(reason);
                        }
                        skip_if_no_signer_or_simulator(client)
                    })
                })),
            },
            Test {
                name: "meter_bundle_high_execution_time".to_string(),
                description: Some(
                    "Meter bundle with high execution time (~20k bn256Add calls)".to_string(),
                ),
                run: Box::new(|client| Box::pin(test_meter_bundle_high_execution_time(client))),
                skip_if: Some(Box::new(|client| {
                    Box::pin(async move {
                        if let Some(reason) = check_metering_support(client).await {
                            return Some(reason);
                        }
                        skip_if_no_signer_or_simulator(client)
                    })
                })),
            },
            Test {
                name: "metered_priority_fee".to_string(),
                description: Some("Test base_meteredPriorityFeePerGas endpoint".to_string()),
                run: Box::new(|client| Box::pin(test_metered_priority_fee(client))),
                skip_if: Some(Box::new(|client| Box::pin(check_metering_support(client)))),
            },
        ],
    }
}

async fn test_meter_bundle_empty(client: &TestClient) -> Result<()> {
    let bundle = Bundle { block_number: 1, ..Default::default() };

    let response = client.meter_bundle(bundle).await?;

    ensure!(response.results.is_empty(), "Empty bundle should have no results");
    ensure!(response.total_gas_used == 0, "Empty bundle should use 0 gas");

    tracing::debug!(state_block = response.state_block_number, "Metered empty bundle");

    Ok(())
}

async fn test_meter_bundle_state_block(client: &TestClient) -> Result<()> {
    let bundle = Bundle { block_number: 1, ..Default::default() };

    let response = client.meter_bundle(bundle).await?;

    tracing::debug!(state_block = response.state_block_number, "State block from metering");

    let pending_block = client
        .get_block_by_number(BlockNumberOrTag::Pending)
        .await?
        .ok_or_else(|| eyre!("Pending block unavailable to validate state block"))?;

    let latest_block = client
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| eyre!("Latest block unavailable to validate state block"))?;

    ensure!(
        response.state_block_number <= pending_block.header.number,
        "State block should not be ahead of pending: metering returned {}, pending is {}",
        response.state_block_number,
        pending_block.header.number
    );

    ensure!(
        response.state_block_number >= latest_block.header.number,
        "State block should not be older than latest: metering returned {}, latest is {}",
        response.state_block_number,
        latest_block.header.number
    );

    let block =
        client.get_block_by_number(BlockNumberOrTag::Number(response.state_block_number)).await?;
    if block.is_none() && response.state_block_number < pending_block.header.number {
        return Err(eyre!(
            "State block {} not retrievable even though it should be canonical (latest={}, pending={})",
            response.state_block_number,
            latest_block.header.number,
            pending_block.header.number
        ));
    }

    Ok(())
}

async fn test_meter_bundle_with_transaction(client: &TestClient) -> Result<()> {
    let from = client.signer_address().ok_or_else(|| eyre::eyre!("No signer"))?;
    let to = client.recipient().ok_or_else(|| eyre::eyre!("No recipient configured"))?;

    let latest_block = client
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| eyre::eyre!("No latest block"))?;
    let block_number = latest_block.header.number + 1;

    let nonce = client.peek_nonce().await?;

    let mut tx_request = OpTransactionRequest::default()
        .from(from)
        .to(to)
        .value(U256::from(1_000_000_000_000_000u64))
        .nonce(nonce)
        .gas_limit(21_000)
        .max_fee_per_gas(1_000_000_000)
        .max_priority_fee_per_gas(1_000_000);
    tx_request.set_chain_id(client.chain_id());

    let (tx_bytes, tx_hash) = client.sign_transaction(tx_request)?;

    let bundle = Bundle { txs: vec![tx_bytes], block_number, ..Default::default() };

    let response = client.meter_bundle(bundle).await?;

    ensure!(response.results.len() == 1, "Should have 1 result, got {}", response.results.len());
    ensure!(response.total_gas_used > 0, "Should use some gas");

    let tx_result = &response.results[0];
    ensure!(tx_result.tx_hash == tx_hash, "Transaction hash should match");
    ensure!(tx_result.from_address == from, "From address should match");
    ensure!(tx_result.gas_used == 21000, "ETH transfer should use 21000 gas");

    tracing::info!(
        tx_hash = ?tx_hash,
        gas_used = response.total_gas_used,
        execution_time_us = response.total_execution_time_us,
        state_root_time_us = response.state_root_time_us,
        "Metered bundle with transaction"
    );

    Ok(())
}

async fn test_meter_bundle_flashblock_index(client: &TestClient) -> Result<()> {
    let bundle = Bundle { block_number: 1, ..Default::default() };

    let response = client.meter_bundle(bundle).await?;

    if let Some(index) = response.state_flashblock_index {
        tracing::info!(
            flashblock_index = index,
            state_block = response.state_block_number,
            "Metering used flashblock state"
        );
    } else {
        tracing::info!(
            state_block = response.state_block_number,
            "Metering used canonical block state (no flashblocks available)"
        );
    }

    Ok(())
}

async fn test_meter_bundle_state_root_timing(client: &TestClient) -> Result<()> {
    let from = client.signer_address().ok_or_else(|| eyre::eyre!("No signer"))?;
    let to = client.recipient().ok_or_else(|| eyre::eyre!("No recipient configured"))?;

    let latest_block = client
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| eyre::eyre!("No latest block"))?;
    let block_number = latest_block.header.number + 1;

    let nonce = client.peek_nonce().await?;

    let mut tx_request = OpTransactionRequest::default()
        .from(from)
        .to(to)
        .value(U256::from(1_000_000_000_000_000u64))
        .nonce(nonce)
        .gas_limit(21_000)
        .max_fee_per_gas(1_000_000_000)
        .max_priority_fee_per_gas(1_000_000);
    tx_request.set_chain_id(client.chain_id());

    let (tx_bytes, _) = client.sign_transaction(tx_request)?;

    let bundle = Bundle { txs: vec![tx_bytes], block_number, ..Default::default() };

    let response = client.meter_bundle(bundle).await?;

    tracing::info!(
        state_root_time_us = response.state_root_time_us,
        total_execution_time_us = response.total_execution_time_us,
        "State root timing from ETH transfer"
    );

    ensure!(
        response.total_gas_used == 21000,
        "ETH transfer should use exactly 21000 gas, got {}",
        response.total_gas_used
    );

    Ok(())
}

async fn test_meter_bundle_state_root_timing_simulator(client: &TestClient) -> Result<()> {
    let from = client.signer_address().ok_or_else(|| eyre::eyre!("No signer"))?;
    let simulator_addr =
        client.simulator().ok_or_else(|| eyre::eyre!("No simulator configured"))?;

    let latest_block = client
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| eyre::eyre!("No latest block"))?;
    let block_number = latest_block.header.number + 1;

    let nonce = client.peek_nonce().await?;

    let config = SimulatorConfigBuilder::new().create_accounts(10).create_storage(50).build();
    let calldata = encode_run_call(&config);

    let mut tx_request = OpTransactionRequest::default()
        .from(from)
        .to(simulator_addr)
        .input(calldata.into())
        .nonce(nonce)
        .gas_limit(2_000_000)
        .max_fee_per_gas(1_000_000_000)
        .max_priority_fee_per_gas(1_000_000);
    tx_request.set_chain_id(client.chain_id());

    let (tx_bytes, _) = client.sign_transaction(tx_request)?;

    let bundle = Bundle { txs: vec![tx_bytes], block_number, ..Default::default() };

    let response = client.meter_bundle(bundle).await?;

    tracing::info!(
        state_root_time_us = response.state_root_time_us,
        total_execution_time_us = response.total_execution_time_us,
        total_gas_used = response.total_gas_used,
        "State root timing from Simulator bundle"
    );

    ensure!(
        response.total_gas_used > 21000,
        "Simulator bundle should use more gas than simple transfer, got {}",
        response.total_gas_used
    );

    ensure!(
        response.total_execution_time_us > 0,
        "Execution time should be positive for simulator workload"
    );

    Ok(())
}

async fn test_meter_bundle_high_state_root_time(client: &TestClient) -> Result<()> {
    let from = client.signer_address().ok_or_else(|| eyre::eyre!("No signer"))?;
    let simulator_addr =
        client.simulator().ok_or_else(|| eyre::eyre!("No simulator configured"))?;

    let latest_block = client
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| eyre::eyre!("No latest block"))?;
    let block_number = latest_block.header.number + 1;

    let nonce = client.peek_nonce().await?;

    let config = SimulatorConfigBuilder::new().create_accounts(400).build();
    let calldata = encode_run_call(&config);

    let mut tx_request = OpTransactionRequest::default()
        .from(from)
        .to(simulator_addr)
        .input(calldata.into())
        .nonce(nonce)
        .gas_limit(16_500_000)
        .max_fee_per_gas(1_000_000_000)
        .max_priority_fee_per_gas(1_000_000);
    tx_request.set_chain_id(client.chain_id());

    let (tx_bytes, _) = client.sign_transaction(tx_request)?;
    let bundle = Bundle { txs: vec![tx_bytes], block_number, ..Default::default() };

    let response = client.meter_bundle(bundle).await?;

    tracing::info!(
        state_root_time_us = response.state_root_time_us,
        total_execution_time_us = response.total_execution_time_us,
        total_gas_used = response.total_gas_used,
        accounts_created = 400,
        "High state root time test"
    );

    ensure!(
        response.total_gas_used < 16_500_000,
        "Transaction should not hit gas limit (got {}), reduce account count",
        response.total_gas_used
    );

    ensure!(
        response.total_gas_used >= 8_000_000,
        "Expected significant gas usage for 400 account creations, got {}",
        response.total_gas_used
    );

    ensure!(response.total_execution_time_us > 0, "Execution time should be positive");

    Ok(())
}

async fn test_meter_bundle_high_execution_time(client: &TestClient) -> Result<()> {
    let from = client.signer_address().ok_or_else(|| eyre::eyre!("No signer"))?;
    let simulator_addr =
        client.simulator().ok_or_else(|| eyre::eyre!("No simulator configured"))?;

    let latest_block = client
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| eyre::eyre!("No latest block"))?;
    let block_number = latest_block.header.number + 1;

    let nonce = client.peek_nonce().await?;

    let bn256_add = Address::from_word(B256::from(U256::from(6)));
    let config = SimulatorConfigBuilder::new().precompile_calls(15_000, bn256_add).build();
    let calldata = encode_run_call(&config);

    let mut tx_request = OpTransactionRequest::default()
        .from(from)
        .to(simulator_addr)
        .input(calldata.into())
        .nonce(nonce)
        .gas_limit(16_000_000)
        .max_fee_per_gas(1_000_000_000)
        .max_priority_fee_per_gas(1_000_000);
    tx_request.set_chain_id(client.chain_id());

    let (tx_bytes, _) = client.sign_transaction(tx_request)?;
    let bundle = Bundle { txs: vec![tx_bytes], block_number, ..Default::default() };

    let response = client.meter_bundle(bundle).await?;

    tracing::info!(
        state_root_time_us = response.state_root_time_us,
        total_execution_time_us = response.total_execution_time_us,
        total_gas_used = response.total_gas_used,
        precompile_calls = 15_000,
        "High execution time test"
    );

    ensure!(
        response.total_gas_used < 16_000_000,
        "Transaction should not hit gas limit (got {}), reduce precompile calls",
        response.total_gas_used
    );

    ensure!(
        response.total_gas_used >= 6_000_000,
        "Expected significant gas usage for 15k precompile calls, got {}",
        response.total_gas_used
    );

    ensure!(response.total_execution_time_us > 0, "Execution time should be positive");

    ensure!(
        response.total_execution_time_us >= response.state_root_time_us,
        "For precompile-heavy workloads, execution time ({} us) should be >= state root time ({} us)",
        response.total_execution_time_us,
        response.state_root_time_us
    );

    Ok(())
}

async fn test_metered_priority_fee(client: &TestClient) -> Result<()> {
    let bundle = Bundle { block_number: 1, ..Default::default() };

    match client.metered_priority_fee(bundle).await {
        Ok(response) => {
            tracing::debug!(
                blocks_sampled = response.blocks_sampled,
                priority_fee = ?response.priority_fee,
                "Got metered priority fee"
            );
            Ok(())
        }
        Err(e) => {
            let err_str = format!("{e:?}");
            if err_str.contains("cache") || err_str.contains("empty") || err_str.contains("no data")
            {
                tracing::warn!("Metered priority fee not available (no cache data): {}", e);
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}
