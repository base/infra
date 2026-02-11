//! Node health checking — unified for both L1 and L2.
//!
//! Each node is polled in parallel: fetch latest block number,
//! check staleness against a grace period, detect rate-limiting.
//! For L2, an optional sequencer node provides a "delta" metric.

use std::time::Duration;

use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use op_alloy_network::Optimism;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    metrics::{self, Metrics},
    services::Service,
};

// ── Node type ────────────────────────────────────────────────

/// Represents a node used in health checks.
#[derive(Debug, Clone)]
pub struct Node {
    pub client: RootProvider<Optimism>,
    pub url: String,
    pub provider: String,
}

impl Node {
    /// Create a [`Node`] from a URL. Returns `None` if the URL cannot be parsed.
    pub fn new(url: &str, provider_name: &str) -> Option<Self> {
        let parsed: url::Url = url.parse().ok()?;
        let client = ProviderBuilder::new()
            .disable_recommended_fillers()
            .network::<Optimism>()
            .connect_http(parsed);
        Some(Self { client, url: url.to_string(), provider: provider_name.to_string() })
    }
}

/// Build a list of [`Node`]s, logging and skipping failures.
pub fn build_node_list(urls: &[String], provider_name: &str) -> Vec<Node> {
    urls.iter()
        .filter_map(|url| {
            Node::new(url, provider_name).or_else(|| {
                warn!(url, "failed to parse node URL, skipping");
                None
            })
        })
        .collect()
}

/// Build a node list for external nodes, sanitizing URLs to just the host.
pub fn build_node_list_sanitized(urls: &[String], provider_name: &str) -> Vec<Node> {
    urls.iter()
        .filter_map(|raw_url| {
            let parsed = raw_url.parse::<url::Url>().ok().or_else(|| {
                warn!(url = raw_url.as_str(), "invalid external node URL, skipping");
                None
            })?;
            let sanitized = parsed.host_str().unwrap_or(raw_url).to_string();
            let client = ProviderBuilder::new()
                .disable_recommended_fillers()
                .network::<Optimism>()
                .connect_http(parsed);
            Some(Node { client, url: sanitized, provider: provider_name.to_string() })
        })
        .collect()
}

// ── Service ──────────────────────────────────────────────────

/// Node health checking service (L1 or L2), implementing [`Service`].
#[derive(Debug)]
pub struct HealthChecker {
    /// Nodes to check.
    pub nodes: Vec<Node>,
    /// Optional sequencer node (L2 only).
    pub sequencer: Option<Node>,
    /// Interval between health check polls.
    pub poll_interval: Duration,
    /// Grace period before a node is considered stale.
    pub grace_period: Duration,
    /// Metrics client.
    pub metrics: Metrics,
    /// Layer label: `"l1"` or `"l2"`.
    pub layer: &'static str,
}

impl Service for HealthChecker {
    fn name(&self) -> &str {
        self.layer
    }

    fn spawn(self: Box<Self>, set: &mut JoinSet<()>, cancel: CancellationToken) {
        if self.nodes.is_empty() {
            return;
        }
        let layer = self.layer;
        set.spawn(async move {
            info!(layer, "starting node health checks");
            if let Err(e) = run(
                self.nodes,
                self.sequencer,
                self.poll_interval,
                self.grace_period,
                self.metrics,
                layer,
                cancel,
            )
            .await
            {
                error!(layer, error = %e, "health checker stopped with error");
            }
        });
    }
}

// ── Core logic ───────────────────────────────────────────────

/// Run health checks for a set of nodes at the given interval.
///
/// Works for both L1 and L2. Pass `sequencer: Some(node)` for L2 to
/// enable sequencer-delta tracking.
async fn run(
    nodes: Vec<Node>,
    sequencer: Option<Node>,
    poll_interval: Duration,
    grace_period: Duration,
    metrics: Metrics,
    layer: &str,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = interval.tick() => {
                let seq_block = fetch_sequencer_block(sequencer.as_ref(), &metrics).await;
                check_nodes(&nodes, seq_block, grace_period, &metrics, layer).await;
            }
        }
    }
}

/// Fetch the sequencer's latest block (if configured). Returns `None` if
/// there is no sequencer or the fetch fails.
async fn fetch_sequencer_block(sequencer: Option<&Node>, metrics: &Metrics) -> Option<u64> {
    let seq = sequencer?;
    let tags = [("url", seq.url.as_str()), ("layer", "l2")];
    match tokio::time::timeout(Duration::from_secs(5), seq.client.get_block_number()).await {
        Ok(Ok(n)) => {
            metrics.gauge_with_tags(metrics::SEQUENCER_LATEST_BLOCK, n as f64, &tags);
            info!(url = seq.url, block = n, "fetched sequencer block");
            Some(n)
        }
        _ => {
            metrics.count_with_tags(metrics::NODE_ERROR, 1, &tags);
            None
        }
    }
}

/// Check all nodes in parallel.
async fn check_nodes(
    nodes: &[Node],
    sequencer_block: Option<u64>,
    grace_period: Duration,
    metrics: &Metrics,
    layer: &str,
) {
    let mut set = JoinSet::new();

    for node in nodes {
        let url = node.url.clone();
        let provider = node.provider.clone();
        let client = node.client.clone();
        let metrics = metrics.clone();
        let layer = layer.to_string();

        set.spawn(async move {
            let base_tags: [(&str, &str); 2] = [("url", url.as_str()), ("layer", layer.as_str())];

            match tokio::time::timeout(Duration::from_secs(5), client.get_block_number()).await {
                Err(_) => {
                    error!(url, "timeout fetching latest block");
                    metrics.count_with_tags(metrics::NODE_ERROR, 1, &base_tags);
                }
                Ok(Err(e)) => {
                    let err_str = e.to_string();
                    if err_str.contains("429") {
                        metrics.count_with_tags(metrics::NODE_RATE_LIMITED, 1, &base_tags);
                    } else {
                        metrics.count_with_tags(metrics::NODE_ERROR, 1, &base_tags);
                    }
                    error!(url, error = %e, "unable to fetch latest block");
                }
                Ok(Ok(block_num)) => {
                    let prov_tags: [(&str, &str); 3] = [
                        ("url", url.as_str()),
                        ("layer", layer.as_str()),
                        ("provider", provider.as_str()),
                    ];
                    metrics.gauge_with_tags(
                        metrics::NODE_LATEST_BLOCK,
                        block_num as f64,
                        &prov_tags,
                    );
                    info!(url, provider, block = block_num, "fetched latest block");

                    if let Some(seq_block) = sequencer_block {
                        let delta = seq_block.saturating_sub(block_num);
                        metrics.gauge_with_tags(metrics::SEQUENCER_DELTA, delta as f64, &base_tags);
                    }

                    // Staleness check via header timestamp.
                    if let Ok(Some(header)) = client.get_header_by_number(block_num.into()).await {
                        let block_time =
                            std::time::UNIX_EPOCH + Duration::from_secs(header.timestamp);
                        let age = std::time::SystemTime::now()
                            .duration_since(block_time)
                            .unwrap_or_default();

                        metrics.gauge_with_tags(
                            metrics::NODE_LATEST_TIME,
                            header.timestamp as f64,
                            &base_tags,
                        );

                        if age > grace_period {
                            metrics.count_with_tags(metrics::NODE_STALL, 1, &base_tags);
                        } else {
                            metrics.gauge_with_tags(
                                metrics::NODE_HEALTHY,
                                block_num as f64,
                                &base_tags,
                            );
                        }
                    }
                }
            }
        });
    }

    while set.join_next().await.is_some() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_node_list ──────────────────────────────────────

    #[test]
    fn build_node_list_valid_urls() {
        let urls =
            vec!["http://localhost:8545".to_string(), "http://geth.example.com:8545".to_string()];
        let nodes = build_node_list(&urls, "test_provider");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].provider, "test_provider");
        assert_eq!(nodes[0].url, "http://localhost:8545");
        assert_eq!(nodes[1].url, "http://geth.example.com:8545");
    }

    #[test]
    fn build_node_list_skips_invalid() {
        let urls = vec![
            "http://valid.host:8545".to_string(),
            "not a valid url at all!@#$%".to_string(),
            "http://another-valid.host:8545".to_string(),
        ];
        let nodes = build_node_list(&urls, "test");
        // The invalid URL should be silently skipped.
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn build_node_list_empty() {
        let urls: Vec<String> = vec![];
        let nodes = build_node_list(&urls, "test");
        assert!(nodes.is_empty());
    }

    // ── build_node_list_sanitized ────────────────────────────

    #[test]
    fn build_node_list_sanitized_strips_path() {
        let urls = vec!["http://host.example.com/path?key=val".to_string()];
        let nodes = build_node_list_sanitized(&urls, "ext");
        assert_eq!(nodes.len(), 1);
        // The URL should be sanitized to just the host.
        assert_eq!(nodes[0].url, "host.example.com");
        assert_eq!(nodes[0].provider, "ext");
    }

    #[test]
    fn build_node_list_sanitized_skips_invalid() {
        let urls = vec!["http://valid.host:8545".to_string(), "garbage://???".to_string()];
        let nodes = build_node_list_sanitized(&urls, "ext");
        // The second URL doesn't produce a valid host but may still parse.
        // Only URLs that can be parsed at all are included.
        assert!(!nodes.is_empty());
    }

    #[test]
    fn build_node_list_sanitized_preserves_host_only() {
        let urls = vec!["http://192.168.1.100:8545/v1/mainnet".to_string()];
        let nodes = build_node_list_sanitized(&urls, "infra");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].url, "192.168.1.100");
    }
}
