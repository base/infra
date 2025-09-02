use crate::coordinator::ArchivalCoordinator;
use crate::database::Database;
use crate::metrics::Metrics;
use crate::s3::S3Manager;
use crate::websocket::WebSocketPool;
use crate::{cli::FlashblocksArchiverArgs, FlashblockMessage};
use anyhow::Result;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::time::interval;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Debug)]
pub struct FlashblocksArchiver {
    args: FlashblocksArchiverArgs,
    database: Database,
    builder_ids: HashMap<String, Uuid>,
    metrics: Metrics,
    retention_coordinator: Option<ArchivalCoordinator>,
}

impl FlashblocksArchiver {
    pub async fn new(args: FlashblocksArchiverArgs) -> Result<Self> {
        let database = Database::new(&args.database_url, args.database_max_connections).await?;

        database.run_migrations().await?;

        let builders = args.parse_builders()?;
        let mut builder_ids = HashMap::new();
        for builder_config in &builders {
            let builder_id = database
                .get_or_create_builder(builder_config.url.as_ref(), Some(&builder_config.name))
                .await?;
            builder_ids.insert(builder_config.name.clone(), builder_id);
        }

        // Initialize metrics first
        let metrics = Metrics::default();

        // Initialize retention coordinator if enabled
        let retention_coordinator = if args.retention_enabled {
            if let Some(bucket_name) = &args.s3_bucket_name {
                info!(message = "Initializing retention coordinator", bucket = %bucket_name);

                let s3_manager = S3Manager::new(
                    bucket_name.clone(),
                    args.s3_region.clone(),
                    args.s3_key_prefix.clone(),
                )
                .await?;

                let coordinator = ArchivalCoordinator::new(
                    database.clone(),
                    s3_manager,
                    args.retention_period_days,
                    args.block_range_size,
                    metrics.clone(),
                );

                Some(coordinator)
            } else {
                warn!(
                    message = "Retention enabled but no S3 bucket configured, disabling retention"
                );
                None
            }
        } else {
            info!(message = "Data retention disabled");
            None
        };

        Ok(Self {
            args,
            database,
            builder_ids,
            metrics,
            retention_coordinator,
        })
    }

    pub async fn run(&self) -> Result<()> {
        let builders = self.args.parse_builders()?;
        info!(
            message = "Starting FlashblocksArchiver",
            builders_count = builders.len(),
            retention_enabled = self.retention_coordinator.is_some()
        );

        if builders.is_empty() {
            warn!(message = "No builders configured, archiver will not collect any data");
            return Ok(());
        }

        let ws_pool = WebSocketPool::new(builders);
        let mut receiver = ws_pool.start().await?;

        let mut batch = Vec::with_capacity(self.args.batch_size);
        let mut flush_interval = interval(Duration::from_secs(self.args.flush_interval_seconds));

        // Start retention background task if coordinator is available
        let mut retention_interval = if self.retention_coordinator.is_some() {
            Some(interval(Duration::from_secs(
                self.args.archive_interval_hours * 3600,
            )))
        } else {
            None
        };

        if self.retention_coordinator.is_some() {
            info!(
                message = "Retention background task enabled",
                interval_hours = self.args.archive_interval_hours,
                retention_period_days = self.args.retention_period_days,
                block_range_size = self.args.block_range_size
            );
        }

        info!(message = "FlashblocksArchiver started, listening for flashblock messages");

        loop {
            tokio::select! {
                message = receiver.recv() => {
                    match message {
                        Some((builder_name, payload)) => {
                            if let Some(builder_id) = self.builder_ids.get(&builder_name) {
                                batch.push((*builder_id, payload));

                                if batch.len() >= self.args.batch_size {
                                    if let Err(e) = self.flush_batch(&mut batch) {
                                        error!(message = "Failed to flush batch", error = %e);
                                        self.metrics.flush_batch_error.increment(1);
                                    }
                                }
                            } else {
                                warn!(message = "Received message from unknown builder", builder_name = %builder_name);
                            }
                        }
                        None => {
                            info!(message = "All WebSocket connections closed, flushing remaining data");
                            if !batch.is_empty() {
                                if let Err(e) = self.flush_batch(&mut batch) {
                                    error!(message = "Failed to flush final batch", error = %e);
                                }
                            }
                            break;
                        }
                    }
                }

                // Periodically flush messages even if batch isn't full
                _ = flush_interval.tick() => {
                    if !batch.is_empty() {
                        if let Err(e) = self.flush_batch(&mut batch) {
                            error!(message = "Failed to flush batch on timer", error = %e);
                        }
                    }
                }

                // Run retention archival process periodically
                _ = async {
                    match retention_interval.as_mut() {
                        Some(interval) => {
                            interval.tick().await;
                        }
                        None => {
                            std::future::pending::<()>().await;
                        }
                    }
                }, if retention_interval.is_some() => {
                    if let Some(ref coordinator) = self.retention_coordinator {
                        info!(message = "Running scheduled retention archival cycle");
                        if let Err(e) = coordinator.run_archival_cycle().await {
                            error!(message = "Retention archival cycle failed", error = %e);
                        }
                    }
                }
            }
        }

        info!(message = "FlashblocksArchiver stopped");
        Ok(())
    }

    fn flush_batch(&self, batch: &mut Vec<(Uuid, FlashblockMessage)>) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }

        info!(
            message = "Flushing batch of flashblock messages",
            batch_size = batch.len()
        );

        for (builder_id, payload) in batch.drain(..) {
            let database = self.database.get_pool().clone();
            let metrics = self.metrics.clone();

            tokio::spawn(async move {
                let start = Instant::now();
                let db = Database::from_pool(database);
                if let Err(e) = db.store_flashblock(builder_id, &payload).await {
                    error!(message = "Failed to store flashblock", block_number = payload.metadata.block_number, index = payload.index, error = %e);
                }
                metrics.store_flashblock_duration.record(start.elapsed());
            });
        }

        Ok(())
    }
}
