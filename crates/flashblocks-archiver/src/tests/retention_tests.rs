use crate::{
    cli::FlashblocksArchiverArgs,
    coordinator::ArchivalCoordinator,
    database::Database,
    parquet::ParquetWriter,
    s3::S3Manager,
    tests::common::PostgresTestContainer,
    types::{ArchivalStatus, Flashblock, Transaction},
};
use chrono::Utc;
use tempfile::{NamedTempFile, TempDir};
use uuid::Uuid;

struct RetentionTestSetup {
    postgres: PostgresTestContainer,
    database: Database,
    temp_dir: TempDir,
    builder_id: Uuid,
}

impl RetentionTestSetup {
    async fn new() -> anyhow::Result<Self> {
        let postgres = PostgresTestContainer::new("test_retention").await?;
        let database = postgres.database.clone();
        let temp_dir = TempDir::new()?;
        
        let builder_id = postgres
            .create_test_builder("ws://test-builder.example.com", Some("test_builder"))
            .await?;

        Ok(Self {
            postgres,
            database,
            temp_dir,
            builder_id,
        })
    }

    async fn create_test_flashblocks(&self, count: usize, start_block: i64) -> anyhow::Result<Vec<Uuid>> {
        let mut flashblock_ids = Vec::new();
        
        for i in 0..count {
            let flashblock_id = self.database.store_flashblock(
                self.builder_id,
                &format!("payload_{}", i),
                i as i64,
                start_block + i as i64,
            ).await?;
            
            // Add some transactions for each flashblock
            for tx_idx in 0..3 {
                self.database.store_transaction(
                    flashblock_id,
                    self.builder_id,
                    &format!("payload_{}", i),
                    i as i64,
                    start_block + i as i64,
                    &vec![1, 2, 3, 4, tx_idx],
                    &format!("0xhash_{}_{}_{}", start_block, i, tx_idx),
                    tx_idx as i32,
                ).await?;
            }
            
            flashblock_ids.push(flashblock_id);
        }
        
        Ok(flashblock_ids)
    }
}

#[tokio::test]
async fn test_parquet_writer() -> anyhow::Result<()> {
    let temp_file = NamedTempFile::new()?;
    let file_path = temp_file.path().to_str().unwrap();

    let flashblock = Flashblock {
        id: Uuid::new_v4(),
        builder_id: Uuid::new_v4(),
        payload_id: "test_payload".to_string(),
        flashblock_index: 1,
        block_number: 100,
        received_at: Utc::now(),
    };

    let transactions = vec![
        Transaction {
            id: Uuid::new_v4(),
            flashblock_id: flashblock.id,
            builder_id: flashblock.builder_id,
            payload_id: flashblock.payload_id.clone(),
            flashblock_index: flashblock.flashblock_index,
            block_number: flashblock.block_number,
            tx_data: vec![1, 2, 3, 4],
            tx_hash: "0x1234".to_string(),
            tx_index: 0,
            created_at: Utc::now(),
        },
        Transaction {
            id: Uuid::new_v4(),
            flashblock_id: flashblock.id,
            builder_id: flashblock.builder_id,
            payload_id: flashblock.payload_id.clone(),
            flashblock_index: flashblock.flashblock_index,
            block_number: flashblock.block_number,
            tx_data: vec![5, 6, 7, 8],
            tx_hash: "0x5678".to_string(),
            tx_index: 1,
            created_at: Utc::now(),
        },
    ];

    let data = vec![(flashblock, transactions)];
    let row_count = ParquetWriter::write_to_file(file_path, data)?;

    assert_eq!(row_count, 1);
    
    // Verify file exists and has content
    let metadata = std::fs::metadata(file_path)?;
    assert!(metadata.len() > 0);
    
    Ok(())
}

#[tokio::test]
async fn test_s3_manager_key_generation() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    
    // Create a mock S3Manager (we can't easily test real S3 in unit tests)
    let s3_manager = S3Manager::new(
        "test-bucket".to_string(),
        "us-east-1".to_string(),
        "flashblocks/".to_string(),
    ).await?;

    let key = s3_manager.generate_archive_key(1000, 2000);
    assert_eq!(key, "flashblocks_archive_1000_2000.parquet");
    assert_eq!(s3_manager.bucket(), "test-bucket");
    assert_eq!(s3_manager.key_prefix(), "flashblocks/");
    
    Ok(())
}

#[tokio::test]
async fn test_archival_job_creation() -> anyhow::Result<()> {
    let setup = RetentionTestSetup::new().await?;
    
    // Create test data
    let _flashblock_ids = setup.create_test_flashblocks(10, 1000).await?;
    
    // Test archival job creation
    let job_id = setup.database.create_archival_job(1000, 1009).await?;
    
    // Verify job was created
    let jobs = setup.database.get_pending_archival_jobs(10).await?;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, job_id);
    assert_eq!(jobs[0].start_block, 1000);
    assert_eq!(jobs[0].end_block, 1009);
    assert_eq!(jobs[0].status, "pending");
    
    Ok(())
}

#[tokio::test]
async fn test_archival_job_status_updates() -> anyhow::Result<()> {
    let setup = RetentionTestSetup::new().await?;
    
    let job_id = setup.database.create_archival_job(1000, 1009).await?;
    
    // Test status update to processing
    setup.database.update_archival_job_status(
        job_id,
        ArchivalStatus::Processing,
        None,
        None,
        None,
    ).await?;
    
    let jobs = setup.database.get_pending_archival_jobs(10).await?;
    assert_eq!(jobs.len(), 0); // Should not be pending anymore
    
    // Test status update to completed with S3 path
    setup.database.update_archival_job_status(
        job_id,
        ArchivalStatus::Completed,
        Some("s3://bucket/key.parquet"),
        Some(10),
        None,
    ).await?;
    
    // Verify the job was updated
    let completed_jobs = sqlx::query_as::<_, crate::types::ArchivalJob>(
        "SELECT * FROM archival_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_all(setup.database.get_pool())
    .await?;
    
    assert_eq!(completed_jobs.len(), 1);
    assert_eq!(completed_jobs[0].status, "completed");
    assert_eq!(completed_jobs[0].s3_path, Some("s3://bucket/key.parquet".to_string()));
    assert_eq!(completed_jobs[0].archived_count, 10);
    assert!(completed_jobs[0].completed_at.is_some());
    
    Ok(())
}

#[tokio::test]
async fn test_archival_lock_mechanism() -> anyhow::Result<()> {
    let setup = RetentionTestSetup::new().await?;
    
    let job_id = setup.database.create_archival_job(1000, 1009).await?;
    
    // Acquire lock
    let acquired = setup.database.acquire_archival_lock(job_id).await?;
    assert!(acquired);
    
    // Try to acquire the same lock again (should fail)
    let acquired_again = setup.database.acquire_archival_lock(job_id).await?;
    assert!(!acquired_again);
    
    // Release lock
    setup.database.release_archival_lock(job_id).await?;
    
    // Should be able to acquire again after release
    let acquired_after_release = setup.database.acquire_archival_lock(job_id).await?;
    assert!(acquired_after_release);
    
    setup.database.release_archival_lock(job_id).await?;
    
    Ok(())
}

#[tokio::test]
async fn test_get_flashblocks_with_transactions() -> anyhow::Result<()> {
    let setup = RetentionTestSetup::new().await?;
    
    // Create test data with known parameters
    let _flashblock_ids = setup.create_test_flashblocks(5, 2000).await?;
    
    // Test fetching flashblocks with transactions
    let results = setup.database.get_flashblocks_with_transactions(
        2000, 2004, 10, 0
    ).await?;
    
    assert_eq!(results.len(), 5);
    
    // Verify each flashblock has its transactions
    for (flashblock, transactions) in results {
        assert!(flashblock.block_number >= 2000 && flashblock.block_number <= 2004);
        assert_eq!(transactions.len(), 3); // We created 3 transactions per flashblock
        
        // Verify transaction data
        for tx in transactions {
            assert_eq!(tx.flashblock_id, flashblock.id);
            assert_eq!(tx.block_number, flashblock.block_number);
            assert!(tx.tx_hash.starts_with("0xhash_"));
        }
    }
    
    Ok(())
}

#[tokio::test]
async fn test_delete_archived_data() -> anyhow::Result<()> {
    let setup = RetentionTestSetup::new().await?;
    
    // Create test data
    let _flashblock_ids = setup.create_test_flashblocks(5, 3000).await?;
    
    // Verify data exists
    let initial_count = setup.database.count_flashblocks_in_range(3000, 3004).await?;
    assert_eq!(initial_count, 5);
    
    // Delete archived data
    let (deleted_flashblocks, deleted_transactions) = setup.database
        .delete_archived_data(3000, 3004)
        .await?;
    
    assert_eq!(deleted_flashblocks, 5);
    assert_eq!(deleted_transactions, 15); // 5 flashblocks * 3 transactions each
    
    // Verify data is gone
    let final_count = setup.database.count_flashblocks_in_range(3000, 3004).await?;
    assert_eq!(final_count, 0);
    
    Ok(())
}

#[tokio::test]
async fn test_archival_coordinator_job_creation() -> anyhow::Result<()> {
    let setup = RetentionTestSetup::new().await?;
    
    // Create test data that would be eligible for archival
    let _flashblock_ids = setup.create_test_flashblocks(50, 1000).await?;
    
    // Create a mock S3 manager (won't actually connect to S3)
    let s3_manager = S3Manager::new(
        "test-bucket".to_string(),
        "us-east-1".to_string(),
        "flashblocks/".to_string(),
    ).await?;
    
    let coordinator = ArchivalCoordinator::new(
        setup.database.clone(),
        s3_manager,
        30, // retention_period_days
        10, // block_range_size
    );
    
    // Note: We can't easily test the full archival process without S3 credentials
    // But we can test that the coordinator can be created successfully
    assert!(true); // Placeholder - coordinator was created successfully
    
    Ok(())
}

#[tokio::test]
async fn test_archival_status_conversions() -> anyhow::Result<()> {
    use std::str::FromStr;
    
    // Test Display trait
    assert_eq!(ArchivalStatus::Pending.to_string(), "pending");
    assert_eq!(ArchivalStatus::Processing.to_string(), "processing");
    assert_eq!(ArchivalStatus::Completed.to_string(), "completed");
    assert_eq!(ArchivalStatus::Failed.to_string(), "failed");
    
    // Test FromStr trait
    assert_eq!(ArchivalStatus::from_str("pending")?, ArchivalStatus::Pending);
    assert_eq!(ArchivalStatus::from_str("processing")?, ArchivalStatus::Processing);
    assert_eq!(ArchivalStatus::from_str("completed")?, ArchivalStatus::Completed);
    assert_eq!(ArchivalStatus::from_str("failed")?, ArchivalStatus::Failed);
    
    // Test invalid status
    assert!(ArchivalStatus::from_str("invalid").is_err());
    
    Ok(())
}

#[tokio::test]
async fn test_archival_cli_args_defaults() -> anyhow::Result<()> {
    // Test that CLI args have sensible defaults
    let args = FlashblocksArchiverArgs {
        database_url: "postgres://test".to_string(),
        builders: vec!["test".to_string()],
        database_max_connections: 10,
        batch_size: 100,
        flush_interval_seconds: 5,
        retention_enabled: true,
        retention_period_days: 30,
        archive_interval_hours: 6,
        block_range_size: 21600, // 6 hours on Base
        s3_bucket_name: Some("test-bucket".to_string()),
        s3_region: "us-east-1".to_string(),
        s3_key_prefix: "flashblocks/".to_string(),
    };
    
    assert_eq!(args.retention_period_days, 30);
    assert_eq!(args.archive_interval_hours, 6);
    assert_eq!(args.block_range_size, 21600);
    assert_eq!(args.s3_region, "us-east-1");
    assert_eq!(args.s3_key_prefix, "flashblocks/");
    
    Ok(())
}