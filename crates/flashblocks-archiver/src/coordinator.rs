use crate::database::Database;
use crate::metrics::Metrics;
use crate::parquet::ParquetWriter;
use crate::s3::S3Manager;
use crate::types::{ArchivalJob, ArchivalStatus};
use anyhow::Result;
use std::time::Instant;
use tempfile::TempDir;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Debug)]
pub struct ArchivalCoordinator {
    database: Database,
    s3_manager: S3Manager,
    retention_blocks: u64,
    block_range_size: u64,
    metrics: Metrics,
}

impl ArchivalCoordinator {
    pub fn new(
        database: Database,
        s3_manager: S3Manager,
        retention_blocks: u64,
        block_range_size: u64,
        metrics: Metrics,
    ) -> Self {
        Self {
            database,
            s3_manager,
            retention_blocks,
            block_range_size,
            metrics,
        }
    }

    pub async fn run_archival_cycle(&self) -> Result<()> {
        let cycle_start = Instant::now();
        info!(message = "Starting archival cycle");

        let result = async {
            self.create_archival_jobs().await?;
            self.process_pending_jobs().await?;
            Ok(())
        }
        .await;

        match result {
            Ok(()) => {
                self.metrics.archival_cycles_completed.increment(1);
                info!(message = "Archival cycle completed");
            }
            Err(e) => {
                self.metrics.archival_cycles_failed.increment(1);
                error!(message = "Archival cycle failed", error = %e);
                return Err(e);
            }
        }

        self.metrics
            .archival_cycle_duration
            .record(cycle_start.elapsed().as_secs_f64());

        // Update pending jobs gauge
        let pending_jobs = self.database.get_num_pending_archival_jobs().await?;
        self.metrics.archival_jobs_pending.set(pending_jobs as f64);

        Ok(())
    }

    async fn create_archival_jobs(&self) -> Result<()> {
        let latest_block = match self.database.get_global_latest_block_number().await? {
            Some(block) => block,
            None => {
                info!(message = "No data found for archival");
                return Ok(());
            }
        };

        let retention_cutoff = if latest_block > self.retention_blocks {
            latest_block - self.retention_blocks
        } else {
            info!(
                message = "Not enough blocks for archival",
                latest_block = latest_block,
                retention_blocks = self.retention_blocks
            );
            return Ok(());
        };

        let oldest_block = match self.database.get_oldest_block_number().await? {
            Some(block) => block,
            None => {
                info!(message = "No data found for archival");
                return Ok(());
            }
        };

        if oldest_block >= retention_cutoff {
            info!(
                message = "No blocks need archival",
                oldest_block = oldest_block,
                retention_cutoff = retention_cutoff
            );
            return Ok(());
        }

        info!(
            message = "Creating archival jobs",
            oldest_block = oldest_block,
            retention_cutoff = retention_cutoff,
            latest_block = latest_block,
            retention_blocks = self.retention_blocks
        );

        // Create block-aligned archival jobs for ranges that are older than retention cutoff
        // Align to block_range_size boundaries to prevent overlapping jobs from multiple instances
        let start_aligned_block = (oldest_block / self.block_range_size) * self.block_range_size;
        let end_aligned_block =
            ((retention_cutoff - 1) / self.block_range_size) * self.block_range_size;

        let mut current_block = start_aligned_block;
        while current_block <= end_aligned_block {
            let end_block = std::cmp::min(
                current_block + self.block_range_size - 1,
                retention_cutoff, // Don't go past the retention cutoff
            );

            // Only create job if this range has data to archive
            let count = self
                .database
                .count_flashblocks_in_range(current_block, end_block)
                .await?;

            if count > 0 {
                // Use idempotent job creation - handles concurrent instances gracefully
                match self
                    .database
                    .create_archival_job_idempotent(current_block, end_block)
                    .await?
                {
                    Some(job_id) => {
                        self.metrics.archival_jobs_created.increment(1);

                        info!(
                            message = "Created archival job",
                            job_id = %job_id,
                            start_block = current_block,
                            end_block = end_block,
                            flashblock_count = count
                        );
                    }
                    None => {
                        // Job already exists (created by another instance)
                        info!(
                            message = "Archival job already exists",
                            start_block = current_block,
                            end_block = end_block,
                            flashblock_count = count
                        );
                    }
                }
            }

            // Move to next block-aligned range
            current_block += self.block_range_size;
        }

        Ok(())
    }

    async fn process_pending_jobs(&self) -> Result<()> {
        let pending_jobs = self.database.get_pending_archival_jobs(10).await?; // Process up to 10 jobs at once

        for job in pending_jobs {
            if let Err(e) = self.process_single_job(job).await {
                error!(message = "Failed to process archival job", error = %e);
            }
        }

        Ok(())
    }

    async fn process_single_job(&self, job: ArchivalJob) -> Result<()> {
        let job_start = Instant::now();
        let job_id = job.id;
        let start_block = job.start_block as u64;
        let end_block = job.end_block as u64;

        info!(
            message = "Processing archival job",
            job_id = %job_id,
            start_block = start_block,
            end_block = end_block
        );

        // Try to acquire lock for this job
        let lock_acquired = self.database.acquire_archival_lock(job_id).await?;

        if !lock_acquired {
            info!(
                message = "Could not acquire lock for job, skipping",
                job_id = %job_id
            );
            return Ok(());
        }

        // Update job status to processing
        self.database
            .update_archival_job_status(job_id, ArchivalStatus::Processing, None, None, None)
            .await?;

        let result = self
            .archive_block_range(job_id, start_block, end_block)
            .await;

        // Always release the lock
        if let Err(e) = self.database.release_archival_lock(job_id).await {
            error!(message = "Failed to release archival lock", job_id = %job_id, error = %e);
        }

        match result {
            Ok((s3_path, archived_count)) => {
                self.database
                    .update_archival_job_status(
                        job_id,
                        ArchivalStatus::Completed,
                        Some(&s3_path),
                        Some(archived_count),
                        None,
                    )
                    .await?;

                self.metrics.archival_jobs_completed.increment(1);
                self.metrics
                    .flashblocks_archived_count
                    .record(archived_count as f64);
                self.metrics
                    .archival_job_duration
                    .record(job_start.elapsed().as_secs_f64());

                info!(
                    message = "Successfully completed archival job",
                    job_id = %job_id,
                    s3_path = %s3_path,
                    archived_count = archived_count
                );
            }
            Err(e) => {
                self.database
                    .update_archival_job_status(
                        job_id,
                        ArchivalStatus::Failed,
                        None,
                        None,
                        Some(&e.to_string()),
                    )
                    .await?;

                self.metrics.archival_jobs_failed.increment(1);

                error!(
                    message = "Archival job failed",
                    job_id = %job_id,
                    error = %e
                );
            }
        }

        Ok(())
    }

    async fn archive_block_range(
        &self,
        _job_id: Uuid,
        start_block: u64,
        end_block: u64,
    ) -> Result<(String, i64)> {
        info!(
            message = "Archiving block range",
            start_block = start_block,
            end_block = end_block
        );

        // Create temporary directory for parquet file
        let temp_dir = TempDir::new()?;
        let archive_key = self.s3_manager.generate_archive_key(start_block, end_block);
        let temp_file_path = temp_dir.path().join(&archive_key);

        // Check if file already exists in S3
        if self.s3_manager.file_exists(&archive_key).await? {
            warn!(
                message = "Archive file already exists in S3",
                s3_key = %archive_key
            );
            return Ok((archive_key, 0));
        }

        let chunk_size = 1000;
        let mut offset = 0;
        let mut all_data = Vec::new();
        let mut total_count = 0i64;

        loop {
            let chunk = self
                .database
                .get_flashblocks_with_transactions(start_block, end_block, chunk_size, offset)
                .await?;

            if chunk.is_empty() {
                break;
            }

            let chunk_len = chunk.len();
            total_count += chunk_len as i64;
            all_data.extend(chunk);
            offset += chunk_size;

            info!(
                message = "Loaded chunk for archival",
                chunk_size = chunk_len,
                total_loaded = all_data.len()
            );
        }

        if all_data.is_empty() {
            info!(
                message = "No data found for block range",
                start_block = start_block,
                end_block = end_block
            );
            return Ok((archive_key, 0));
        }

        let parquet_start = Instant::now();
        let parquet_path = temp_file_path.to_str().unwrap();

        let rows_written = match ParquetWriter::write_to_file(parquet_path, all_data) {
            Ok(rows) => {
                self.metrics
                    .parquet_creation_duration
                    .record(parquet_start.elapsed().as_secs_f64());
                self.metrics.parquet_rows_written.record(rows as f64);

                // Record file size
                if let Ok(metadata) = std::fs::metadata(parquet_path) {
                    self.metrics
                        .parquet_file_size_bytes
                        .record(metadata.len() as f64);
                }

                rows
            }
            Err(e) => {
                self.metrics.parquet_creation_errors.increment(1);
                return Err(e);
            }
        };

        info!(
            message = "Created Parquet file",
            file_path = %parquet_path,
            rows_written = rows_written
        );

        let s3_start = Instant::now();
        let file_size = std::fs::metadata(parquet_path)?.len();

        let s3_key = match self
            .s3_manager
            .upload_file(parquet_path, &archive_key)
            .await
        {
            Ok(key) => {
                self.metrics
                    .s3_upload_duration
                    .record(s3_start.elapsed().as_secs_f64());
                self.metrics.s3_upload_size_bytes.record(file_size as f64);
                self.metrics.s3_uploads_completed.increment(1);
                self.metrics.total_data_archived_bytes.increment(file_size);
                key
            }
            Err(e) => {
                self.metrics.s3_uploads_failed.increment(1);
                return Err(e);
            }
        };

        let (deleted_flashblocks, deleted_transactions) = self
            .database
            .delete_archived_data(start_block, end_block)
            .await?;

        self.metrics
            .transactions_archived_count
            .record(deleted_transactions as f64);

        // std::fs::remove_file(parquet_path)?;

        info!(
            message = "Deleted archived data from database",
            deleted_flashblocks = deleted_flashblocks,
            deleted_transactions = deleted_transactions
        );

        Ok((s3_key, total_count))
    }
}
