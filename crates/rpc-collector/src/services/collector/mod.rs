//! Core collector: block-polling loop that dispatches blocks to
//! [`MetricRecorder`](crate::recorders::MetricRecorder) implementations.
//!
//! The [`l1`] and [`l2`] submodules provide [`L1Collector`](l1::L1Collector)
//! and [`L2Collector`](l2::L2Collector), which wrap this shared [`Collector`]
//! and implement the [`Service`](crate::services::Service) trait.

pub mod l1;
pub mod l2;

use std::{sync::Arc, time::Duration};

use alloy_provider::Provider;
use alloy_rpc_types_eth::Block;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{metrics::Metrics, recorders::MetricRecorder};

// ── Collector struct ─────────────────────────────────────────

/// Shared block-polling collector logic.
///
/// It fetches blocks sequentially starting from the chain tip and dispatches
/// every block to all registered [`MetricRecorder`]s in parallel.
///
struct Collector<P> {
    provider: P,
    poll_interval: Duration,
    recorders: Vec<Arc<dyn MetricRecorder>>,
    metrics: Metrics,
    label: &'static str,
    /// Metric name for the "current block" gauge (e.g. `base.l2.collector.block`).
    block_metric: &'static str,
}

impl<P> std::fmt::Debug for Collector<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Collector")
            .field("poll_interval", &self.poll_interval)
            .field("recorders", &self.recorders.len())
            .field("label", &self.label)
            .field("block_metric", &self.block_metric)
            .finish()
    }
}

impl<P: Provider + Clone + Send + Sync + 'static> Collector<P> {
    fn new(
        provider: P,
        poll_interval: Duration,
        recorders: Vec<Arc<dyn MetricRecorder>>,
        metrics: Metrics,
        label: &'static str,
        block_metric: &'static str,
    ) -> Self {
        Self { provider, poll_interval, recorders, metrics, label, block_metric }
    }

    /// Human-readable label for this collector (e.g. `"l1"`, `"l2"`).
    const fn label(&self) -> &'static str {
        self.label
    }

    /// Spawn the collector polling loop as a task on the given [`JoinSet`].
    ///
    /// This is the shared spawn implementation used by both
    /// [`L1Collector`](l1::L1Collector) and [`L2Collector`](l2::L2Collector).
    fn spawn(self, set: &mut JoinSet<()>, cancel: CancellationToken) {
        let label = self.label;
        set.spawn(async move {
            info!(label, "starting metrics collector");
            if let Err(e) = self.run(cancel).await {
                error!(label, error = %e, "metrics collector stopped with error");
            } else {
                info!(label, "metrics collector stopped gracefully");
            }
        });
    }

    /// Run the polling loop until the cancellation token fires.
    async fn run(self, cancel: CancellationToken) -> anyhow::Result<()> {
        let mut current_block: Option<u64> = None;
        let mut interval = tokio::time::interval(self.poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!(label = self.label, "collector shutting down");
                    return Ok(());
                }
                _ = interval.tick() => {
                    // If we don't know the start block yet, fetch the latest.
                    if current_block.is_none() {
                        match self.provider.get_block_number().await {
                            Ok(n) => current_block = Some(n),
                            Err(e) => {
                                warn!(label = self.label, error = %e, "failed to get latest block number");
                                continue;
                            }
                        }
                    }

                    // Drain all available blocks.
                    loop {
                        let block_num = current_block.unwrap();

                        let block = match self
                            .provider
                            .get_block_by_number(block_num.into())
                            .full()
                            .await
                        {
                            Ok(Some(b)) => b,
                            Ok(None) => break,  // block not yet available
                            Err(e) => {
                                info!(
                                    label = self.label,
                                    block = block_num,
                                    error = %e,
                                    "unable to fetch block"
                                );
                                break;
                            }
                        };

                        info!(label = self.label, block = block_num, "fetched block");

                        // Emit the collector-progress gauge.
                        let _ = self.metrics.gauge(self.block_metric, block_num as f64);

                        self.record_all(&block).await;

                        current_block = Some(block_num + 1);
                    }
                }
            }
        }
    }

    /// Dispatch all recorders in parallel for a single block.
    async fn record_all(&self, block: &Block) {
        let mut set = JoinSet::new();

        for recorder in &self.recorders {
            let recorder = Arc::clone(recorder);
            let block = block.clone();
            let metrics = self.metrics.clone();
            let label = self.label;

            set.spawn(async move {
                if let Err(e) = recorder.record(&block, &metrics).await {
                    error!(
                        label,
                        recorder = recorder.name(),
                        error = %e,
                        "metric recorder failed"
                    );
                }
            });
        }

        // Await all.
        while set.join_next().await.is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use alloy_provider::ProviderBuilder;
    use async_trait::async_trait;

    use super::*;

    /// A test recorder that counts how many times `record` was called.
    #[derive(Debug)]
    struct CountingRecorder {
        count: AtomicUsize,
    }

    impl CountingRecorder {
        fn new() -> Self {
            Self { count: AtomicUsize::new(0) }
        }
        fn calls(&self) -> usize {
            self.count.load(AtomicOrdering::SeqCst)
        }
    }

    #[async_trait]
    impl MetricRecorder for CountingRecorder {
        fn name(&self) -> &'static str {
            "counting_recorder"
        }
        async fn record(&self, _block: &Block, _metrics: &Metrics) -> anyhow::Result<()> {
            self.count.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn collector_cancellation() {
        // Create a collector pointing at a non-existent endpoint.
        // It will fail to connect, but should still exit cleanly on cancel.
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .connect_http("http://127.0.0.1:1".parse().unwrap());

        let collector = Collector::new(
            provider,
            Duration::from_millis(50),
            vec![],
            Metrics::noop(),
            "test",
            "test.block",
        );

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { collector.run(cancel_clone).await });

        // Give it a moment to start, then cancel.
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();

        // Task should complete without panicking.
        let result = handle.await;
        assert!(result.is_ok(), "collector task panicked");
        assert!(result.unwrap().is_ok(), "collector returned an error");
    }

    #[tokio::test]
    async fn collector_processes_blocks() {
        use std::sync::atomic::AtomicU64;

        use axum::{Json, Router, routing::post};

        // A minimal JSON-RPC handler that returns canned responses:
        // - eth_blockNumber → 1
        // - eth_getBlockByNumber → a minimal block for block 1, then null
        async fn rpc_handler(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
            let id = body.get("id").cloned().unwrap_or_else(|| serde_json::json!(1));
            let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");

            let result = match method {
                "eth_blockNumber" => serde_json::json!("0x1"),
                "eth_getBlockByNumber" => {
                    let count = CALL_COUNT.fetch_add(1, AtomicOrdering::SeqCst);
                    if count == 0 {
                        // Return a minimal block for block 1.
                        serde_json::json!({
                            "hash": "0x0000000000000000000000000000000000000000000000000000000000000001",
                            "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                            "number": "0x1",
                            "timestamp": "0x0",
                            "gasLimit": "0x0",
                            "gasUsed": "0x0",
                            "miner": "0x0000000000000000000000000000000000000000",
                            "extraData": "0x",
                            "baseFeePerGas": "0x0",
                            "stateRoot": "0x0000000000000000000000000000000000000000000000000000000000000000",
                            "transactionsRoot": "0x0000000000000000000000000000000000000000000000000000000000000000",
                            "receiptsRoot": "0x0000000000000000000000000000000000000000000000000000000000000000",
                            "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
                            "difficulty": "0x0",
                            "nonce": "0x0000000000000000",
                            "sha3Uncles": "0x0000000000000000000000000000000000000000000000000000000000000000",
                            "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                            "transactions": [],
                            "size": "0x0",
                            "totalDifficulty": "0x0",
                            "uncles": []
                        })
                    } else {
                        serde_json::Value::Null
                    }
                }
                _ => serde_json::Value::Null,
            };

            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }))
        }

        // Use a thread_local-like static for the call counter in the handler.
        static CALL_COUNT: AtomicU64 = AtomicU64::new(0);
        CALL_COUNT.store(0, AtomicOrdering::SeqCst);

        // Start mock RPC server.
        let app = Router::new().route("/", post(rpc_handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_cancel = CancellationToken::new();
        let server_cancel_clone = server_cancel.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(server_cancel_clone.cancelled_owned())
                .await
                .unwrap();
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Build a counting recorder.
        let recorder = Arc::new(CountingRecorder::new());
        let recorder_ref = Arc::clone(&recorder);

        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .connect_http(format!("http://{addr}").parse().unwrap());

        let collector = Collector::new(
            provider,
            Duration::from_millis(50),
            vec![recorder_ref as Arc<dyn MetricRecorder>],
            Metrics::noop(),
            "test",
            "test.block",
        );

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { collector.run(cancel_clone).await });

        // Wait for the collector to process at least one block.
        tokio::time::sleep(Duration::from_millis(500)).await;
        cancel.cancel();

        let result = handle.await;
        assert!(result.is_ok(), "collector task panicked");

        // The recorder should have been called at least once.
        assert!(
            recorder.calls() >= 1,
            "recorder was called {} times, expected >= 1",
            recorder.calls()
        );

        server_cancel.cancel();
    }
}
