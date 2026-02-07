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
        avg_inclusion_ms = format!("{:.2}", stats.avg_inclusion_ms()),
        tps = format!("{:.1}", stats.tps()),
        "Progress"
    );
}
