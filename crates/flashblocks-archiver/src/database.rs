use crate::types::{ArchivalJob, ArchivalStatus, Flashblock, FlashblockMessage, Transaction};
use alloy_primitives::keccak256;
use anyhow::Result;
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    pub fn get_pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn new(database_url: &str, max_connections: u32) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(database_url)
            .await?;

        info!(message = "Connected to database");
        Ok(Self { pool })
    }

    pub async fn run_migrations(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        info!(message = "Database migrations completed");
        Ok(())
    }

    pub async fn get_or_create_builder(&self, url: &str, name: Option<&str>) -> Result<Uuid> {
        let id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO builders (url, name) 
            VALUES ($1, $2) 
            ON CONFLICT (url) 
            DO UPDATE SET 
                name = EXCLUDED.name,
                updated_at = NOW()
            RETURNING id
            "#,
        )
        .bind(url)
        .bind(name)
        .fetch_one(&self.pool)
        .await?;

        Ok(id)
    }

    pub async fn store_flashblock(
        &self,
        builder_id: Uuid,
        payload: &FlashblockMessage,
    ) -> Result<Uuid> {
        let flashblock_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO flashblocks (
                builder_id, payload_id, flashblock_index, block_number
            ) VALUES (
                $1, $2, $3, $4
            ) 
            ON CONFLICT (builder_id, payload_id, flashblock_index) 
            DO UPDATE SET 
                block_number = EXCLUDED.block_number,
                received_at = NOW()
            RETURNING id
            "#,
        )
        .bind(builder_id)
        .bind(payload.payload_id.to_string())
        .bind(payload.index as i64)
        .bind(payload.metadata.block_number as i64)
        .fetch_one(&self.pool)
        .await?;

        for (tx_index, tx_data) in payload.diff.transactions.iter().enumerate() {
            let tx_hash = keccak256(tx_data);
            self.store_transaction(
                flashblock_id,
                builder_id,
                &payload.payload_id.to_string(),
                payload.index as i64,
                payload.metadata.block_number as i64,
                tx_data.as_ref(),
                &format!("{:#x}", tx_hash),
                tx_index as i32,
            )
            .await?;
        }

        Ok(flashblock_id)
    }

    #[allow(clippy::too_many_arguments)]
    async fn store_transaction(
        &self,
        flashblock_id: Uuid,
        builder_id: Uuid,
        payload_id: &str,
        flashblock_index: i64,
        block_number: i64,
        tx_data: &[u8],
        tx_hash: &str,
        tx_index: i32,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO transactions (
                flashblock_id, builder_id, payload_id, flashblock_index, 
                block_number, tx_data, tx_hash, tx_index
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (flashblock_id, tx_index) 
            DO UPDATE SET 
                tx_data = EXCLUDED.tx_data,
                tx_hash = EXCLUDED.tx_hash,
                created_at = NOW()
            "#,
        )
        .bind(flashblock_id)
        .bind(builder_id)
        .bind(payload_id)
        .bind(flashblock_index)
        .bind(block_number)
        .bind(tx_data)
        .bind(tx_hash)
        .bind(tx_index)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_flashblocks_by_block_number(
        &self,
        block_number: u64,
    ) -> Result<Vec<Flashblock>> {
        let flashblocks = sqlx::query_as::<_, Flashblock>(
            "SELECT * FROM flashblocks WHERE block_number = $1 ORDER BY flashblock_index",
        )
        .bind(block_number as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(flashblocks)
    }

    pub async fn get_latest_block_number(&self, builder_id: Uuid) -> Result<Option<u64>> {
        let result: (Option<i64>,) = sqlx::query_as(
            "SELECT MAX(block_number) as max_block FROM flashblocks WHERE builder_id = $1",
        )
        .bind(builder_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(result.0.map(|b| b as u64))
    }

    // Archival methods

    pub async fn create_archival_job(&self, start_block: u64, end_block: u64) -> Result<Uuid> {
        let job_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO archival_jobs (start_block, end_block, status)
            VALUES ($1, $2, $3)
            RETURNING id
            "#,
        )
        .bind(start_block as i64)
        .bind(end_block as i64)
        .bind(ArchivalStatus::Pending.to_string())
        .fetch_one(&self.pool)
        .await?;

        Ok(job_id)
    }

    pub async fn get_pending_archival_jobs(&self, limit: i64) -> Result<Vec<ArchivalJob>> {
        let jobs = sqlx::query_as::<_, ArchivalJob>(
            r#"
            SELECT * FROM archival_jobs 
            WHERE status = 'pending' 
            ORDER BY created_at ASC 
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(jobs)
    }

    pub async fn update_archival_job_status(
        &self,
        job_id: Uuid,
        status: ArchivalStatus,
        s3_path: Option<&str>,
        archived_count: Option<i64>,
        error_message: Option<&str>,
    ) -> Result<()> {
        // Simple approach - update all fields
        sqlx::query(
            r#"
            UPDATE archival_jobs 
            SET status = $2, 
                updated_at = NOW(),
                completed_at = CASE WHEN $2 = 'completed' THEN NOW() ELSE completed_at END,
                s3_path = COALESCE($3, s3_path),
                archived_count = COALESCE($4, archived_count),
                error_message = COALESCE($5, error_message)
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(status.to_string())
        .bind(s3_path)
        .bind(archived_count)
        .bind(error_message)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn acquire_archival_lock(&self, job_id: Uuid) -> Result<bool> {
        let result = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
            .bind(job_id.as_u128() as i64)
            .fetch_one(&self.pool)
            .await?;

        Ok(result)
    }

    pub async fn release_archival_lock(&self, job_id: Uuid) -> Result<()> {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(job_id.as_u128() as i64)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn get_flashblocks_with_transactions(
        &self,
        start_block: u64,
        end_block: u64,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<(Flashblock, Vec<Transaction>)>> {
        let flashblocks = sqlx::query_as::<_, Flashblock>(
            r#"
            SELECT * FROM flashblocks 
            WHERE block_number >= $1 AND block_number <= $2
            ORDER BY block_number ASC, flashblock_index ASC
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(start_block as i64)
        .bind(end_block as i64)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::new();
        for flashblock in flashblocks {
            let transactions = sqlx::query_as::<_, Transaction>(
                "SELECT * FROM transactions WHERE flashblock_id = $1 ORDER BY tx_index ASC",
            )
            .bind(flashblock.id)
            .fetch_all(&self.pool)
            .await?;

            results.push((flashblock, transactions));
        }

        Ok(results)
    }

    pub async fn delete_archived_data(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> Result<(i64, i64)> {
        let mut tx = self.pool.begin().await?;

        let transaction_result = sqlx::query(
            r#"
            DELETE FROM transactions 
            WHERE block_number >= $1 AND block_number <= $2
            "#,
        )
        .bind(start_block as i64)
        .bind(end_block as i64)
        .execute(&mut *tx)
        .await?;

        let transaction_count = transaction_result.rows_affected() as i64;

        let flashblock_result = sqlx::query(
            r#"
            DELETE FROM flashblocks 
            WHERE block_number >= $1 AND block_number <= $2
            "#,
        )
        .bind(start_block as i64)
        .bind(end_block as i64)
        .execute(&mut *tx)
        .await?;

        let flashblock_count = flashblock_result.rows_affected() as i64;

        tx.commit().await?;

        Ok((flashblock_count, transaction_count))
    }

    pub async fn get_oldest_block_number(&self) -> Result<Option<u64>> {
        let result: (Option<i64>,) =
            sqlx::query_as("SELECT MIN(block_number) as min_block FROM flashblocks")
                .fetch_one(&self.pool)
                .await?;

        Ok(result.0.map(|b| b as u64))
    }

    pub async fn count_flashblocks_in_range(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM flashblocks WHERE block_number >= $1 AND block_number <= $2",
        )
        .bind(start_block as i64)
        .bind(end_block as i64)
        .fetch_one(&self.pool)
        .await?;

        Ok(count)
    }
}
