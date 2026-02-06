use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::info;

use crate::tracker::{Stats, TrackerEvent};

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
                if let Some(stats) = get_stats(&tracker_tx).await {
                    log_stats(&stats);
                }
            }
            _ = shutdown.recv() => {
                break;
            }
        }
    }
}

async fn get_stats(tracker_tx: &mpsc::UnboundedSender<TrackerEvent>) -> Option<Stats> {
    let (tx, rx) = oneshot::channel();
    tracker_tx.send(TrackerEvent::GetStats(tx)).ok()?;
    rx.await.ok()
}

fn log_stats(stats: &Stats) {
    info!(
        sent = stats.sent,
        confirmed = stats.confirmed,
        timed_out = stats.timed_out,
        pending = stats.pending(),
        read_calls = stats.read_calls,
        replays = stats.replays,
        total_requests = stats.total_requests(),
        avg_inclusion_ms = format!("{:.2}", stats.avg_inclusion_ms()),
        "Progress"
    );
}

/// Prints the final report
pub(crate) fn print_final_report(stats: &Stats) {
    println!();
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║                    GOBRR RESULTS                          ║");
    println!("╠═══════════════════════════════════════════════════════════╣");
    println!("║  TRANSACTIONS                                             ║");
    println!("╟───────────────────────────────────────────────────────────╢");
    println!("║  Total sent:               {:>10}                    ║", stats.sent);
    println!("║    - Small calldata:       {:>10}                    ║", stats.small_calldata_count);
    println!("║    - Large calldata:       {:>10}                    ║", stats.large_calldata_count);
    println!("║  Confirmed:                {:>10}                    ║", stats.confirmed);
    println!("║  Timed out (>60s):         {:>10}                    ║", stats.timed_out);
    println!("║  Still pending:            {:>10}                    ║", stats.pending());
    println!("╟───────────────────────────────────────────────────────────╢");
    println!("║  RPC CALLS                                                ║");
    println!("╟───────────────────────────────────────────────────────────╢");
    println!("║  Read calls:               {:>10}                    ║", stats.read_calls);
    println!("║  Replay requests:          {:>10}                    ║", stats.replays);
    println!("║  Total requests:           {:>10}                    ║", stats.total_requests());
    println!("╟───────────────────────────────────────────────────────────╢");
    println!("║  INCLUSION TIMES                                          ║");
    println!("╟───────────────────────────────────────────────────────────╢");
    
    if !stats.inclusion_times_ms.is_empty() {
        println!("║  Average:                  {:>10.2}ms                  ║", stats.avg_inclusion_ms());
        println!("║  Min:                      {:>10}ms                  ║", stats.min_inclusion_ms());
        println!("║  Max:                      {:>10}ms                  ║", stats.max_inclusion_ms());
        println!("║  P50:                      {:>10}ms                  ║", stats.p50_inclusion_ms());
        println!("║  P95:                      {:>10}ms                  ║", stats.p95_inclusion_ms());
        println!("║  P99:                      {:>10}ms                  ║", stats.p99_inclusion_ms());
    } else {
        println!("║  No transactions confirmed yet                          ║");
    }

    // Print per-endpoint stats if we have multiple endpoints
    if stats.endpoint_stats.len() > 1 {
        println!("╟───────────────────────────────────────────────────────────╢");
        println!("║  PER-ENDPOINT STATS                                       ║");
        println!("╟───────────────────────────────────────────────────────────╢");
        
        let mut endpoints: Vec<_> = stats.endpoint_stats.iter().collect();
        endpoints.sort_by(|a, b| a.0.cmp(b.0));
        
        for (endpoint, ep_stats) in endpoints {
            let truncated = if endpoint.len() > 35 {
                format!("{}...", &endpoint[..32])
            } else {
                endpoint.clone()
            };
            println!("║  {:<35}                      ║", truncated);
            println!("║    Requests: {:>8}  Txns: {:>8}  Reads: {:>8} ║", 
                ep_stats.requests, ep_stats.transactions, ep_stats.read_calls);
        }
    }

    // Print per-method stats if we have multiple methods
    if stats.method_stats.len() > 1 {
        println!("╟───────────────────────────────────────────────────────────╢");
        println!("║  PER-METHOD STATS                                         ║");
        println!("╟───────────────────────────────────────────────────────────╢");
        
        let mut methods: Vec<_> = stats.method_stats.iter().collect();
        methods.sort_by(|a, b| b.1.calls.cmp(&a.1.calls));
        
        for (method, method_stats) in methods {
            println!("║  {:<30} {:>10} calls         ║", method.method_name(), method_stats.calls);
        }
    }

    println!("╚═══════════════════════════════════════════════════════════╝");
}
