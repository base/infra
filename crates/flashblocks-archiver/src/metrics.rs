use metrics::{Counter, Gauge, Histogram};
use metrics_derive::Metrics;

#[derive(Metrics, Clone)]
#[metrics(scope = "flashblocks_archiver")]
pub struct Metrics {
    #[metric(describe = "Time taken to store a batch of flashblocks in the database")]
    pub store_flashblock_duration: Histogram,

    #[metric(describe = "Count of errors when flushing a batch of flashblocks")]
    pub flush_batch_error: Counter,

    #[metric(describe = "Time taken to complete an archival cycle")]
    pub archival_cycle_duration: Histogram,

    #[metric(describe = "Number of archival cycles completed successfully")]
    pub archival_cycles_completed: Counter,

    #[metric(describe = "Number of archival cycles that failed")]
    pub archival_cycles_failed: Counter,

    #[metric(describe = "Number of archival jobs created")]
    pub archival_jobs_created: Counter,

    #[metric(describe = "Number of archival jobs completed successfully")]
    pub archival_jobs_completed: Counter,

    #[metric(describe = "Number of archival jobs that failed")]
    pub archival_jobs_failed: Counter,

    #[metric(describe = "Number of pending archival jobs")]
    pub archival_jobs_pending: Gauge,

    #[metric(describe = "Time taken to process a single archival job")]
    pub archival_job_duration: Histogram,

    #[metric(describe = "Number of flashblocks archived per job")]
    pub flashblocks_archived_count: Histogram,

    #[metric(describe = "Number of transactions archived per job")]
    pub transactions_archived_count: Histogram,

    #[metric(describe = "Time taken to create a parquet file")]
    pub parquet_creation_duration: Histogram,

    #[metric(describe = "Size of parquet files created in bytes")]
    pub parquet_file_size_bytes: Histogram,

    #[metric(describe = "Number of rows written to parquet files")]
    pub parquet_rows_written: Histogram,

    #[metric(describe = "Number of parquet file creation errors")]
    pub parquet_creation_errors: Counter,

    #[metric(describe = "Time taken to upload files to S3")]
    pub s3_upload_duration: Histogram,

    #[metric(describe = "Size of files uploaded to S3 in bytes")]
    pub s3_upload_size_bytes: Histogram,

    #[metric(describe = "Number of successful S3 uploads")]
    pub s3_uploads_completed: Counter,

    #[metric(describe = "Number of failed S3 uploads")]
    pub s3_uploads_failed: Counter,

    #[metric(describe = "Total size of data archived in bytes")]
    pub total_data_archived_bytes: Counter,
}
