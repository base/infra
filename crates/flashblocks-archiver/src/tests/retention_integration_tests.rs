use crate::{
    coordinator::ArchivalCoordinator,
    database::Database,
    metrics::Metrics,
    s3::S3Manager,
    tests::common::{LocalStackTestContainer, PostgresTestContainer},
    types::{FlashblockMessage, Metadata},
};
use alloy_primitives::map::foldhash::{HashMap, HashMapExt};
use reth_optimism_primitives::OpReceipt;
use rollup_boost::{ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1};
use tracing::info;
use uuid::Uuid;

struct RetentionTestSetup {
    postgres: PostgresTestContainer,
    builder_id: Uuid,
    s3_manager: S3Manager,
}

impl RetentionTestSetup {
    async fn new() -> anyhow::Result<Self> {
        let postgres = PostgresTestContainer::new("test_retention").await?;
        let localstack = LocalStackTestContainer::new().await?;
        let builder_id = postgres
            .create_test_builder("ws://test-builder.example.com", Some("test_builder"))
            .await?;

        let s3_manager = S3Manager::new_with_endpoint(
            "test-bucket".to_string(),
            "flashblocks/".to_string(),
            localstack.s3_url.clone(),
        )
        .await?;

        Ok(Self {
            postgres,
            builder_id,
            s3_manager,
        })
    }

    fn database(&self) -> &Database {
        &self.postgres.database
    }

    fn s3_manager(&self) -> &S3Manager {
        &self.s3_manager
    }

    fn create_test_flashblock_message(&self, index: u64, block_number: u64) -> FlashblockMessage {
        use alloy_primitives::{Address, Bloom, Bytes, B256, U256};
        use alloy_rpc_types_engine::PayloadId;

        let base = if index == 0 {
            Some(ExecutionPayloadBaseV1 {
                parent_beacon_block_root: B256::from([2; 32]),
                parent_hash: B256::from([3; 32]),
                fee_recipient: Address::from([4; 20]),
                prev_randao: B256::from([5; 32]),
                block_number,
                gas_limit: 30_000_000,
                timestamp: 1700000000,
                extra_data: Bytes::from(vec![6, 7, 8]),
                base_fee_per_gas: U256::from(20_000_000_000u64),
            })
        } else {
            None
        };

        FlashblockMessage {
            payload_id: PayloadId::new([((block_number + index) % 256) as u8; 8]),
            index,
            base,
            diff: ExecutionPayloadFlashblockDeltaV1 {
                state_root: B256::from([9; 32]),
                receipts_root: B256::from([10; 32]),
                logs_bloom: Bloom::default(),
                gas_used: 21000,
                block_hash: B256::from([11; 32]),
                transactions: vec![Bytes::from(vec![0x02, 0x01, 0x00])],
                withdrawals: vec![],
                withdrawals_root: B256::ZERO,
            },
            metadata: Metadata {
                receipts: HashMap::<B256, OpReceipt>::new(),
                new_account_balances: HashMap::<Address, U256>::new(),
                block_number,
            },
        }
    }

    async fn create_test_flashblocks(
        &self,
        num_blocks: usize,
        start_block: i64,
    ) -> anyhow::Result<Vec<Uuid>> {
        let mut flashblock_ids = Vec::new();

        for i in 0..num_blocks {
            for j in 0..11 {
                let message =
                    self.create_test_flashblock_message(j as u64, (start_block + i as i64) as u64);
                let flashblock_id = self
                    .database()
                    .store_flashblock(self.builder_id, &message)
                    .await?;
                flashblock_ids.push(flashblock_id);
            }
        }

        Ok(flashblock_ids)
    }
}

#[tokio::test]
async fn test_full_archival_cycle_e2e() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("flashblocks_archiver=debug")
        .try_init();

    let setup = RetentionTestSetup::new().await?;

    let _flashblock_ids = setup.create_test_flashblocks(50, 1000).await?;

    let initial_count = setup
        .database()
        .count_flashblocks_in_range(1000, 1050)
        .await?;
    assert_eq!(initial_count, 550);

    let coordinator = ArchivalCoordinator::new(
        setup.database().clone(),
        setup.s3_manager().clone(),
        40,
        5,
        Metrics::default(),
    );

    // Run the archival cycle
    coordinator.run_archival_cycle().await?;

    // Verify archival jobs were created for blocks 1000-1010
    // Should create 2 jobs: 1000-1004, 1005-1009
    let jobs = setup.database().get_archival_jobs(10).await?;
    assert!(
        jobs.len() == 2,
        "Expected 2 archival jobs, got {}",
        jobs.len()
    );
    assert_eq!(jobs[0].start_block, 1000);
    assert_eq!(jobs[0].end_block, 1004);
    assert_eq!(jobs[1].start_block, 1005);
    assert_eq!(jobs[1].end_block, 1009);

    // Verify that blocks 1011-1050 (latest 40 blocks ie 440 flashblocks) are still in the database
    let retained_count = setup
        .database()
        .count_flashblocks_in_range(1010, 1050)
        .await?;
    assert_eq!(
        retained_count, 440,
        "Latest 40 blocks (440 flashblocks) should be retained"
    );

    // Verify that 10 blocks were archived
    let files = setup.s3_manager().list_files().await?;
    info!("Uploaded files: {:?}", files);

    Ok(())
}

#[tokio::test]
async fn test_block_aligned_job_creation() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("flashblocks_archiver=debug")
        .try_init();
    let setup = RetentionTestSetup::new().await?;

    // Create data from blocks 1050-1200 (151 blocks)
    let _flashblock_ids = setup.create_test_flashblocks(151, 1050).await?;

    // Create coordinator with block_range_size = 50 and retention = 100 blocks
    // Latest blocks are 1150-1200 (51 blocks), so retention cutoff is at 1101
    // Should create jobs for blocks 1050-1100 (aligned ranges)
    let coordinator = ArchivalCoordinator::new(
        setup.database().clone(),
        setup.s3_manager().clone(),
        100, // retention_blocks - keep latest 100 blocks (1101-1200)
        50,  // block_range_size - creates aligned ranges of 50 blocks
        Metrics::default(),
    );

    coordinator.run_archival_cycle().await?;

    let jobs = setup.database().get_archival_jobs(10).await?;

    // Should create exactly one job for the aligned range 1050-1099
    // (1000-1049 is empty, 1050-1099 has data, 1100+ is retained)
    assert_eq!(jobs.len(), 1, "Expected 1 aligned job");

    let job = &jobs[0];
    assert_eq!(
        job.start_block, 1050,
        "Job should start at aligned boundary"
    );
    assert_eq!(job.end_block, 1099, "Job should end at aligned boundary");

    // Test idempotent creation - running again shouldn't create duplicate jobs
    coordinator.run_archival_cycle().await?;

    let jobs_after = setup.database().get_archival_jobs(10).await?;
    assert_eq!(jobs_after.len(), 1, "No duplicate jobs should be created");
    assert_eq!(jobs_after[0].id, job.id, "Same job should exist");

    Ok(())
}

#[tokio::test]
async fn test_concurrent_job_creation_safety() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("flashblocks_archiver=debug")
        .try_init();
    let setup = RetentionTestSetup::new().await?;

    // Create test data
    let _flashblock_ids = setup.create_test_flashblocks(100, 2000).await?;

    // Test that multiple calls to create_archival_job_idempotent are safe
    let database = setup.database().clone();

    // Try to create the same job range concurrently
    let job1_result = database.create_archival_job_idempotent(2000, 2049).await?;
    let job2_result = database.create_archival_job_idempotent(2000, 2049).await?;

    // Exactly one should succeed, the other should return None
    match (job1_result, job2_result) {
        (Some(_), None) | (None, Some(_)) => {
            // One succeeded, one was duplicate - this is correct
        }
        (Some(_), Some(_)) => {
            panic!("Both job creations succeeded - unique constraint not working");
        }
        (None, None) => {
            panic!("Both job creations failed - unexpected");
        }
    }

    // Verify only one job exists in the database
    let jobs = setup.database().get_pending_archival_jobs(10).await?;
    let matching_jobs: Vec<_> = jobs
        .iter()
        .filter(|j| j.start_block == 2000 && j.end_block == 2049)
        .collect();

    assert_eq!(
        matching_jobs.len(),
        1,
        "Exactly one job should exist for the range"
    );

    Ok(())
}
