//! Snapshots collector — periodically polls S3 bucket endpoints to check
//! snapshot freshness, age, and size.

use std::time::Duration;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use super::Service;
use crate::metrics::{self, Metrics};

// ── Service ──────────────────────────────────────────────────

/// Snapshot monitoring service, implementing [`Service`].
#[derive(Debug)]
pub struct SnapshotsService {
    /// S3 bucket URLs to monitor.
    pub buckets: Vec<String>,
    /// Interval between snapshot checks.
    pub poll_interval: Duration,
    /// Metrics client.
    pub metrics: Metrics,
}

impl Service for SnapshotsService {
    fn name(&self) -> &str {
        "snapshots"
    }

    fn spawn(self: Box<Self>, set: &mut JoinSet<()>, cancel: CancellationToken) {
        if self.buckets.is_empty() {
            info!("no snapshot buckets configured, skipping snapshots collector");
            return;
        }
        set.spawn(async move {
            info!("starting snapshots collector");
            if let Err(e) = run(self.buckets, self.poll_interval, self.metrics, cancel).await {
                error!(error = %e, "snapshots collector failed");
            }
        });
    }
}

// ── Core logic ───────────────────────────────────────────────

/// Run the snapshots collector loop.
async fn run(
    buckets: Vec<String>,
    poll_interval: Duration,
    metrics: Metrics,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let http_client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;

    // Record once immediately, then enter the polling loop.
    record_metrics_in_parallel(&http_client, &buckets, &metrics).await;

    let mut interval = tokio::time::interval(poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the first tick since we already recorded above.
    interval.tick().await;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = interval.tick() => {
                record_metrics_in_parallel(&http_client, &buckets, &metrics).await;
            }
        }
    }
}

/// Fetch snapshot data for every bucket in parallel.
async fn record_metrics_in_parallel(
    http_client: &reqwest::Client,
    buckets: &[String],
    metrics: &Metrics,
) {
    let mut set = tokio::task::JoinSet::new();

    for bucket in buckets {
        let client = http_client.clone();
        let m = metrics.clone();
        let bucket_url = bucket.clone();

        set.spawn(async move {
            check_bucket(&client, &bucket_url, &m).await;
        });
    }

    while set.join_next().await.is_some() {}
}

/// Extract a 10-digit Unix timestamp from a snapshot filename.
///
/// Looks for a 10-digit number immediately before `.tar.gz` or `.tar.zst`.
/// e.g. `snapshot-1234567890.tar.zst` → `Some(1234567890)`.
fn parse_snapshot_timestamp(filename: &str) -> Option<u64> {
    let stem = filename.strip_suffix(".tar.gz").or_else(|| filename.strip_suffix(".tar.zst"))?;
    // The timestamp is the last 10 chars of the stem (or follows a separator).
    let digits: String = stem.chars().rev().take_while(|c| c.is_ascii_digit()).collect();
    let digits: String = digits.chars().rev().collect();
    if digits.len() == 10 { digits.parse().ok() } else { None }
}

/// Check a single S3 bucket endpoint for its latest snapshot.
async fn check_bucket(http_client: &reqwest::Client, bucket_url: &str, metrics: &Metrics) {
    let tags: [(&str, &str); 1] = [("bucket", bucket_url)];

    info!(bucket = bucket_url, "fetching snapshot information");

    // 1. Fetch the "latest" file to get the snapshot filename.
    let latest_url = format!("{bucket_url}/latest");
    let res = match http_client.get(&latest_url).send().await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, bucket = bucket_url, "error fetching snapshot latest");
            metrics.incr_with_tags(metrics::SNAPSHOT_ERROR, &tags);
            return;
        }
    };

    if !res.status().is_success() {
        error!(
            bucket = bucket_url,
            status = %res.status(),
            "non-200 response fetching snapshot latest"
        );
        metrics.incr_with_tags(metrics::SNAPSHOT_ERROR, &tags);
        return;
    }

    let body = match res.text().await {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, bucket = bucket_url, "error reading latest file body");
            metrics.incr_with_tags(metrics::SNAPSHOT_ERROR, &tags);
            return;
        }
    };

    let file_name = body.trim();

    // 2. Parse the 10-digit Unix timestamp from the filename.
    let timestamp = match parse_snapshot_timestamp(file_name) {
        Some(t) => t,
        None => {
            error!(bucket = bucket_url, file = file_name, "invalid snapshot filename");
            metrics.incr_with_tags(metrics::SNAPSHOT_ERROR, &tags);
            return;
        }
    };

    // 3. HEAD request to get the file size.
    let file_url = format!("{bucket_url}/{file_name}");
    let head_res = match http_client.head(&file_url).send().await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, bucket = bucket_url, "error fetching snapshot HEAD");
            metrics.incr_with_tags(metrics::SNAPSHOT_ERROR, &tags);
            return;
        }
    };

    if !head_res.status().is_success() {
        error!(
            bucket = bucket_url,
            status = %head_res.status(),
            "non-200 response for snapshot HEAD"
        );
        metrics.incr_with_tags(metrics::SNAPSHOT_ERROR, &tags);
        return;
    }

    let content_length = head_res.content_length().unwrap_or(0);

    // 4. Compute age and emit metrics.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let age_secs = now.saturating_sub(timestamp) as f64;

    metrics.gauge_with_tags(metrics::SNAPSHOT_SUCCESS, timestamp as f64, &tags);
    metrics.gauge_with_tags(metrics::SNAPSHOT_AGE, age_secs, &tags);
    metrics.gauge_with_tags(metrics::SNAPSHOT_SIZE, content_length as f64, &tags);

    info!(
        bucket = bucket_url,
        file = file_name,
        timestamp = timestamp,
        size = content_length,
        "fetched snapshot information"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_snapshot_timestamp() {
        assert_eq!(parse_snapshot_timestamp("snapshot-1234567890.tar.zst"), Some(1234567890));
        assert_eq!(parse_snapshot_timestamp("snapshot-1234567890.tar.gz"), Some(1234567890));
        assert_eq!(parse_snapshot_timestamp("1234567890.tar.zst"), Some(1234567890));
        assert_eq!(parse_snapshot_timestamp("bad.tar.zst"), None);
        assert_eq!(parse_snapshot_timestamp("snapshot-123.tar.zst"), None); // too short
        assert_eq!(parse_snapshot_timestamp("snapshot.txt"), None);
    }
}
