use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tracing::info;

use crate::tracker::{self, Stats, TrackerEvent};

const REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// Runs the stats reporter task
pub(crate) async fn run_stats_reporter(
    tracker_tx: mpsc::UnboundedSender<TrackerEvent>,
    mut shutdown: broadcast::Receiver<()>,
) {
    let mut interval = tokio::time::interval(REPORT_INTERVAL);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Some(stats) = tracker::get_stats(&tracker_tx).await {
                    log_stats(&stats);
                }
            }
            _ = shutdown.recv() => {
                break;
            }
        }
    }
}

fn log_stats(stats: &Stats) {
    info!(
        sent = stats.sent,
        confirmed = stats.confirmed,
        failed = stats.failed,
        timed_out = stats.timed_out,
        pending = stats.pending(),
        tps = format!("{:.1}", stats.tps()),
        "Progress"
    );

    // Log FB inclusion metrics if we have data
    if stats.fb_inclusion_count > 0 {
        info!(
            avg_ms = format!("{:.2}", stats.fb_avg_inclusion_ms()),
            min_ms = stats.fb_min_inclusion_ms(),
            max_ms = stats.fb_max_inclusion_ms(),
            p50_ms = stats.fb_percentile(50.0),
            p99_ms = stats.fb_percentile(99.0),
            count = stats.fb_inclusion_count,
            "FB Inclusion"
        );
    }

    // Log block inclusion metrics if we have data
    if stats.block_inclusion_count > 0 {
        info!(
            avg_ms = format!("{:.2}", stats.block_avg_inclusion_ms()),
            min_ms = stats.block_min_inclusion_ms(),
            max_ms = stats.block_max_inclusion_ms(),
            p50_ms = stats.block_percentile(50.0),
            p99_ms = stats.block_percentile(99.0),
            count = stats.block_inclusion_count,
            "Block Inclusion"
        );
    }
}
