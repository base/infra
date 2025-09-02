use anyhow::Result;
use clap::Parser;
use url::Url;

#[derive(Parser, Debug, Clone)]
#[command(name = "flashblocks-archiver")]
#[command(
    about = "Archives flashblock messages from multiple builders to PostgreSQL, and periodically dumps Parquet files to S3"
)]
pub struct FlashblocksArchiverArgs {
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgresql://localhost/flashblocks_archiver",
        help = "Database URL"
    )]
    pub database_url: String,

    #[arg(
        long,
        env = "DATABASE_MAX_CONNECTIONS",
        default_value = "10",
        help = "Maximum database connections"
    )]
    pub database_max_connections: u32,

    #[arg(
        long,
        env = "DATABASE_CONNECT_TIMEOUT_SECONDS",
        default_value = "30",
        help = "Database connection timeout in seconds"
    )]
    pub database_connect_timeout_seconds: u64,

    #[arg(
        long,
        env = "FLASHBLOCKS_WEBSOCKET_URLS",
        value_delimiter = ',',
        help = "Comma-separated list of builder WebSocket URLs"
    )]
    pub builder_urls: Vec<String>,

    #[arg(
        long,
        env = "FLASHBLOCKS_RECONNECT_DELAY_SECONDS",
        default_value = "5",
        help = "WebSocket reconnect delay in seconds"
    )]
    pub reconnect_delay_seconds: u64,

    #[arg(
        long,
        env = "ARCHIVER_BUFFER_SIZE",
        default_value = "1000",
        help = "Message buffer size"
    )]
    pub buffer_size: usize,

    #[arg(
        long,
        env = "ARCHIVER_BATCH_SIZE",
        default_value = "100",
        help = "Batch size for database writes"
    )]
    pub batch_size: usize,

    #[arg(
        long,
        env = "ARCHIVER_FLUSH_INTERVAL_SECONDS",
        default_value = "5",
        help = "Flush interval in seconds"
    )]
    pub flush_interval_seconds: u64,

    // Retention settings
    #[arg(
        long,
        env = "RETENTION_ENABLED",
        default_value = "true",
        help = "Enable data retention/archival"
    )]
    pub retention_enabled: bool,

    #[arg(
        long,
        env = "RETENTION_PERIOD_DAYS",
        default_value = "30",
        help = "Number of days to keep data in PostgreSQL before archiving to S3"
    )]
    pub retention_period_days: u64,

    #[arg(
        long,
        env = "RETENTION_ARCHIVE_INTERVAL_HOURS",
        default_value = "6",
        help = "How often to run archival process in hours"
    )]
    pub archive_interval_hours: u64,

    #[arg(
        long,
        env = "RETENTION_BLOCK_RANGE_SIZE",
        default_value = "21600",
        help = "Number of blocks to archive in each job (21600 = 6 hours on Base)"
    )]
    pub block_range_size: u64,

    #[arg(
        long,
        env = "S3_BUCKET_NAME",
        help = "S3 bucket name for archival storage"
    )]
    pub s3_bucket_name: Option<String>,

    #[arg(
        long,
        env = "S3_REGION",
        default_value = "us-east-1",
        help = "S3 region"
    )]
    pub s3_region: String,

    #[arg(
        long,
        env = "S3_KEY_PREFIX",
        default_value = "flashblocks/",
        help = "S3 key prefix for archived files"
    )]
    pub s3_key_prefix: String,
}

#[derive(Debug, Clone)]
pub struct BuilderConfig {
    pub name: String,
    pub url: Url,
    pub reconnect_delay_seconds: u64,
}

impl FlashblocksArchiverArgs {
    pub fn parse_builders(&self) -> Result<Vec<BuilderConfig>> {
        self.builder_urls
            .iter()
            .enumerate()
            .map(|(i, url)| {
                Ok(BuilderConfig {
                    name: format!("builder_{}", i),
                    url: Url::parse(url.trim())?,
                    reconnect_delay_seconds: self.reconnect_delay_seconds,
                })
            })
            .collect::<Result<Vec<_>>>()
    }
}
