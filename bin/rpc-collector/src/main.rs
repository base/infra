//! `rpc-collector` — binary entry point.
//!
//! Parses CLI / env-var configuration, creates providers, wires up every
//! subsystem from the `rpc-collector` library crate, and runs them all
//! concurrently until a shutdown signal is received.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use alloy_primitives::Address;
use alloy_provider::{Provider, ProviderBuilder};
use anyhow::Context;
use clap::Parser;
use rpc_collector::{
    metrics::Metrics,
    services::{
        DivergenceCheckerService, FlashblockValidator, HealthChecker, HealthServer, L1Collector,
        L1CollectorConfig, L2Collector, L2CollectorConfig, MempoolListenerService, Node, Service,
        SnapshotsService, build_node_list, build_node_list_sanitized,
    },
};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use url::Url;

/// Configuration for the RPC Collector service.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "rpc-collector",
    version,
    about = "Service for collecting OPStack metrics from RPC endpoints"
)]
struct Args {
    // ── RPC URLs ──────────────────────────────────────────────
    /// RPC URL for L1 (Ethereum Mainnet).
    #[arg(long, env = "RPC_COLLECTOR_RPC_L1_URL")]
    pub l1_rpc_url: Url,

    /// RPC URL for L2 (OP Stack chain).
    #[arg(long, env = "RPC_COLLECTOR_RPC_L2_URL")]
    pub l2_rpc_url: Url,

    // ── Poll intervals ───────────────────────────────────────
    /// Interval in ms to poll L1 for new blocks.
    #[arg(long, env = "RPC_COLLECTOR_POLL_L1_INTERVAL_MS")]
    pub poll_l1_interval_ms: u64,

    /// Interval in ms to poll L2 for new blocks.
    #[arg(long, env = "RPC_COLLECTOR_POLL_L2_INTERVAL_MS")]
    pub poll_l2_interval_ms: u64,

    /// Interval in ms to poll node health checks.
    #[arg(long, env = "RPC_COLLECTOR_POLL_HEALTH_CHECK_INTERVAL_MS")]
    pub poll_health_check_interval_ms: u64,

    // ── L1 contract addresses ────────────────────────────────
    /// Address of the Optimism Portal contract on L1.
    #[arg(long, env = "RPC_COLLECTOR_OPTIMISM_PORTAL_ADDRESS")]
    pub optimism_portal_address: Address,

    /// Address of the L2 Output Oracle contract on L1 (legacy).
    #[arg(long, env = "RPC_COLLECTOR_OUTPUT_ORACLE_ADDRESS")]
    pub output_oracle_address: Option<Address>,

    /// Address of the Dispute Game Factory contract on L1.
    #[arg(long, env = "RPC_COLLECTOR_DISPUTE_GAME_FACTORY_ADDRESS")]
    pub dispute_game_factory_address: Option<Address>,

    /// Address of the L1 Standard Bridge contract.
    #[arg(long, env = "RPC_COLLECTOR_L1_STANDARD_BRIDGE_ADDRESS")]
    pub l1_standard_bridge_address: Address,

    /// Address of the L1 System Config contract.
    #[arg(long, env = "RPC_COLLECTOR_L1_SYSTEM_CONFIG_ADDRESS")]
    pub l1_system_config_address: Address,

    /// Address of the proposer on L1.
    #[arg(long, env = "RPC_COLLECTOR_ETHEREUM_PROPOSER_ADDRESS")]
    pub ethereum_proposer_address: Address,

    /// Address of the batcher on L1.
    #[arg(long, env = "RPC_COLLECTOR_ETHEREUM_BATCHER_ADDRESS")]
    pub ethereum_batcher_address: Address,

    /// Address of the batch inbox on L1.
    #[arg(long, env = "RPC_COLLECTOR_ETHEREUM_BATCH_INBOX_ADDRESS")]
    pub batch_inbox_address: Address,

    /// Address of the balance tracker contract on L1.
    #[arg(long, env = "RPC_COLLECTOR_BALANCE_TRACKER_ADDRESS")]
    pub balance_tracker_address: Option<Address>,

    // ── L2 contract addresses ────────────────────────────────
    /// Address of the fee disburser contract on L2.
    #[arg(long, env = "RPC_COLLECTOR_FEE_DISBURSER_ADDRESS")]
    pub fee_disburser_address: Option<Address>,

    // ── Node lists ───────────────────────────────────────────
    /// L1 node URLs for health checks.
    #[arg(long, env = "RPC_COLLECTOR_L1_NODES", value_delimiter = ',')]
    pub l1_nodes: Vec<String>,

    /// L2 node URLs for health checks.
    #[arg(long, env = "RPC_COLLECTOR_L2_NODES", value_delimiter = ',')]
    pub l2_nodes: Vec<String>,

    /// Sequencer node URL.
    #[arg(long, env = "RPC_COLLECTOR_L2_SEQUENCER")]
    pub l2_sequencer: Option<String>,

    /// External L2 node URLs (appended to `l2_nodes`, URLs sanitised).
    #[arg(long, env = "RPC_COLLECTOR_EXTERNAL_NODES_URL", value_delimiter = ',')]
    pub external_nodes_url: Vec<String>,

    // ── Grace periods ────────────────────────────────────────
    /// Grace period in ms for L1 nodes to be considered healthy.
    #[arg(long, env = "RPC_COLLECTOR_L1_NODES_GRACE_PERIOD_MS")]
    pub l1_nodes_grace_period_ms: u64,

    /// Grace period in ms for L2 nodes to be considered healthy.
    #[arg(long, env = "RPC_COLLECTOR_L2_NODES_GRACE_PERIOD_MS")]
    pub l2_nodes_grace_period_ms: u64,

    // ── Contract balance calls ───────────────────────────────
    /// L1 contract balance calls: `metric|address|calldata` triples.
    #[arg(long, env = "RPC_COLLECTOR_L1_CONTRACT_BALANCE_CALLS", value_delimiter = ',')]
    pub l1_contract_balance_calls: Vec<String>,

    /// L2 contract balance calls: `metric|address|calldata` triples.
    #[arg(long, env = "RPC_COLLECTOR_L2_CONTRACT_BALANCE_CALLS", value_delimiter = ',')]
    pub l2_contract_balance_calls: Vec<String>,

    // ── Flashblocks ──────────────────────────────────────────
    /// WebSocket URL for the Flashblock proxy.
    #[arg(long, env = "RPC_COLLECTOR_FLASHBLOCK_WEBSOCKET_PROXY_URL")]
    pub flashblock_websocket_url: Option<String>,

    /// Flashblock rollup-boost WebSocket URLs.
    #[arg(long, env = "RPC_COLLECTOR_FLASHBLOCK_WEBSOCKET_RB_URLS", value_delimiter = ',')]
    pub flashblock_rb_urls: Vec<String>,

    /// Flashblock builder WebSocket URLs.
    #[arg(long, env = "RPC_COLLECTOR_FLASHBLOCK_WEBSOCKET_BUILDER_URLS", value_delimiter = ',')]
    pub flashblock_builder_urls: Vec<String>,

    // ── Mempool ──────────────────────────────────────────────
    /// RPC URL for Geth mempool monitoring.
    #[arg(long, env = "RPC_COLLECTOR_GETH_MEMPOOL")]
    pub geth_mempool_rpc_url: Option<String>,

    /// RPC URL for Reth mempool monitoring.
    #[arg(long, env = "RPC_COLLECTOR_RETH_MEMPOOL")]
    pub reth_mempool_rpc_url: Option<String>,

    /// Interval in ms to poll mempool metrics.
    #[arg(long, env = "RPC_COLLECTOR_POLL_MEMPOOL_INTERVAL_MS", default_value = "5000")]
    pub poll_mempool_interval_ms: u64,

    // ── Divergence checker ───────────────────────────────────
    /// Geth node URL for divergence checking.
    #[arg(long, env = "RPC_COLLECTOR_DIVERGENCE_GETH_NODE")]
    pub divergence_geth_node: Option<String>,

    /// Reth node URL for divergence checking.
    #[arg(long, env = "RPC_COLLECTOR_DIVERGENCE_RETH_NODE")]
    pub divergence_reth_node: Option<String>,

    /// Grace period for Geth node sync in ms.
    #[arg(long, env = "RPC_COLLECTOR_DIVERGENCE_GETH_GRACE_PERIOD_MS", default_value = "30000")]
    pub divergence_geth_grace_period_ms: u64,

    /// Interval in ms to poll divergence checker.
    #[arg(long, env = "RPC_COLLECTOR_DIVERGENCE_POLL_INTERVAL_MS", default_value = "5000")]
    pub divergence_poll_interval_ms: u64,

    // ── Snapshots ────────────────────────────────────────────
    /// S3 bucket URLs for snapshot monitoring.
    #[arg(long, env = "RPC_COLLECTOR_SNAPSHOT_BUCKETS", value_delimiter = ',')]
    pub snapshot_buckets: Vec<String>,

    // ── Observability / infra ────────────────────────────────
    /// Address for the health check HTTP server.
    #[arg(long, env = "RPC_COLLECTOR_HEALTH_ADDR", default_value = "0.0.0.0:3001")]
    pub health_addr: SocketAddr,

    /// `StatsD` host for metric emission.
    #[arg(long, env = "RPC_COLLECTOR_STATSD_HOST", default_value = "127.0.0.1")]
    pub statsd_host: String,

    /// `StatsD` port for metric emission.
    #[arg(long, env = "RPC_COLLECTOR_STATSD_PORT", default_value = "8125")]
    pub statsd_port: u16,

    /// `StatsD` metric prefix.
    #[arg(long, env = "RPC_COLLECTOR_STATSD_PREFIX", default_value = "")]
    pub statsd_prefix: String,

    /// Log level.
    #[arg(long, env = "RPC_COLLECTOR_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// Log format: `json` or `text`.
    #[arg(long, env = "RPC_COLLECTOR_LOG_FORMAT", default_value = "text")]
    pub log_format: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let cfg = Args::parse();

    init_tracing(&cfg.log_level, &cfg.log_format);
    info!(version = env!("CARGO_PKG_VERSION"), "starting rpc-collector");

    let m = Metrics::new(&cfg.statsd_host, cfg.statsd_port, &cfg.statsd_prefix)
        .context("failed to create StatsD client")?;

    let cancel = CancellationToken::new();
    let l1 = Arc::new(ProviderBuilder::new().connect_http(cfg.l1_rpc_url.clone()));
    let l2 = Arc::new(ProviderBuilder::new().connect_http(cfg.l2_rpc_url.clone()));

    let services = build_services(&cfg, &l1, &l2, &m)?;

    let mut set = JoinSet::new();
    info!(count = services.len(), "spawning services");
    for svc in services {
        info!(name = svc.name(), "spawning service");
        svc.spawn(&mut set, cancel.clone());
    }

    info!("all services started — waiting for shutdown signal");
    await_shutdown(cancel, set).await;

    info!("rpc-collector shut down");
    Ok(())
}

/// Build all services from configuration and shared dependencies.
///
/// Returns a `Vec<Box<dyn Service>>` that `main` can spawn uniformly.
pub(crate) fn build_services<P: Provider + Clone + Send + Sync + 'static>(
    cfg: &Args,
    l1: &Arc<P>,
    l2: &Arc<P>,
    m: &Metrics,
) -> anyhow::Result<Vec<Box<dyn Service>>> {
    let mut services: Vec<Box<dyn Service>> = vec![];

    // ── Health server ─────────────────────────────────────
    let health_server = HealthServer { addr: cfg.health_addr };
    services.push(Box::new(health_server));

    // ── L2 metrics collector ──────────────────────────────
    let l2_collector = L2Collector::new(
        L2CollectorConfig {
            poll_interval: Duration::from_millis(cfg.poll_l2_interval_ms),
            fee_disburser_address: cfg.fee_disburser_address,
            l1_system_config_address: cfg.l1_system_config_address,
            contract_balance_calls: cfg.l2_contract_balance_calls.clone(),
        },
        Arc::clone(l2),
        m.clone(),
    )?;
    services.push(Box::new(l2_collector));

    // ── L1 metrics collector ──────────────────────────────
    let l1_collector = L1Collector::new(
        L1CollectorConfig {
            poll_interval: Duration::from_millis(cfg.poll_l1_interval_ms),
            optimism_portal_address: cfg.optimism_portal_address,
            output_oracle_address: cfg.output_oracle_address,
            dispute_game_factory_address: cfg.dispute_game_factory_address,
            l1_standard_bridge_address: cfg.l1_standard_bridge_address,
            ethereum_proposer_address: cfg.ethereum_proposer_address,
            ethereum_batcher_address: cfg.ethereum_batcher_address,
            batch_inbox_address: cfg.batch_inbox_address,
            balance_tracker_address: cfg.balance_tracker_address,
            contract_balance_calls: cfg.l1_contract_balance_calls.clone(),
        },
        Arc::clone(l1),
        m.clone(),
    )?;
    services.push(Box::new(l1_collector));

    // ── L1 health checks ─────────────────────────────
    let l1_health_checker = HealthChecker {
        nodes: build_node_list(&cfg.l1_nodes, "internal"),
        sequencer: None,
        poll_interval: Duration::from_millis(cfg.poll_health_check_interval_ms),
        grace_period: Duration::from_millis(cfg.l1_nodes_grace_period_ms),
        metrics: m.clone(),
        layer: "l1",
    };
    services.push(Box::new(l1_health_checker));

    // ── L2 health checks ─────────────────────────────────
    let mut l2_nodes = build_node_list(&cfg.l2_nodes, "internal");
    l2_nodes.extend(build_node_list_sanitized(&cfg.external_nodes_url, "external"));
    let sequencer = cfg.l2_sequencer.as_deref().and_then(|url| {
        Node::new(url, "sequencer").or_else(|| {
            warn!("failed to parse sequencer URL");
            None
        })
    });
    let l2_health_checker = HealthChecker {
        nodes: l2_nodes,
        sequencer,
        poll_interval: Duration::from_millis(cfg.poll_health_check_interval_ms),
        grace_period: Duration::from_millis(cfg.l2_nodes_grace_period_ms),
        metrics: m.clone(),
        layer: "l2",
    };
    services.push(Box::new(l2_health_checker));

    // ── Flashblock validators ─────────────────────────────
    let fb_urls = cfg
        .flashblock_websocket_url
        .iter()
        .map(|u| (u.clone(), "websocket-proxy".to_string()))
        .chain(
            cfg.flashblock_rb_urls.iter().enumerate().map(|(i, u)| (u.clone(), format!("rb-{i}"))),
        )
        .chain(
            cfg.flashblock_builder_urls
                .iter()
                .enumerate()
                .map(|(i, u)| (u.clone(), format!("builder-{i}"))),
        );
    for (url, name) in fb_urls {
        info!(url, name, "registering flashblock validator");
        let flashblock_validator = FlashblockValidator::new(
            name,
            url,
            Arc::clone(l2),
            Duration::from_millis(cfg.poll_l2_interval_ms),
            m.clone(),
        );
        services.push(Box::new(flashblock_validator));
    }

    // ── Mempool listener ──────────────────────────────────
    match (cfg.geth_mempool_rpc_url.as_deref(), cfg.reth_mempool_rpc_url.as_deref()) {
        (Some(g), Some(r)) => {
            let geth = ProviderBuilder::new().connect_http(g.parse::<Url>()?);
            let reth = ProviderBuilder::new().connect_http(r.parse::<Url>()?);
            let mempool_listener = MempoolListenerService {
                geth,
                reth,
                poll_interval: Duration::from_millis(cfg.poll_mempool_interval_ms),
                metrics: m.clone(),
                listener_name: "mempool-listener".to_string(),
            };
            services.push(Box::new(mempool_listener));
        }
        (None, None) => info!("no mempool RPC URLs provided, skipping"),
        (None, _) => warn!("geth mempool URL missing, skipping mempool listener"),
        (_, None) => warn!("reth mempool URL missing, skipping mempool listener"),
    }

    // ── Divergence checker ────────────────────────────────
    if let (Some(g), Some(r)) =
        (cfg.divergence_geth_node.as_deref(), cfg.divergence_reth_node.as_deref())
    {
        let geth = ProviderBuilder::new().connect_http(g.parse::<Url>()?);
        let reth = ProviderBuilder::new().connect_http(r.parse::<Url>()?);
        let divergence_checker = DivergenceCheckerService {
            geth,
            reth,
            poll_interval: Duration::from_millis(cfg.divergence_poll_interval_ms),
            geth_grace_period: Duration::from_millis(cfg.divergence_geth_grace_period_ms),
            metrics: m.clone(),
            checker_name: "divergence-checker".to_string(),
        };
        services.push(Box::new(divergence_checker));
    } else {
        info!("divergence checker not configured, skipping");
    }

    // ── Snapshots ─────────────────────────────────────────
    let snapshots_service = SnapshotsService {
        buckets: cfg.snapshot_buckets.clone(),
        poll_interval: Duration::from_secs(3600),
        metrics: m.clone(),
    };
    services.push(Box::new(snapshots_service));

    Ok(services)
}

/// Initialise `tracing` with the given level and format (`json` or `text`).
fn init_tracing(level: &str, format: &str) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));

    match format {
        "json" => {
            tracing_subscriber::fmt().with_env_filter(filter).json().init();
        }
        _ => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }
}

/// Wait for SIGINT / SIGTERM, cancel all tasks, and drain the join set.
async fn await_shutdown(cancel: CancellationToken, mut set: JoinSet<()>) {
    use tokio::signal;

    tokio::select! {
        _ = signal::ctrl_c() => {
            info!("received SIGINT, shutting down");
        }
        _ = async {
            #[cfg(unix)]
            {
                let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
                    .expect("failed to register SIGTERM handler");
                sigterm.recv().await;
            }
            #[cfg(not(unix))]
            {
                std::future::pending::<()>().await;
            }
        } => {
            info!("received SIGTERM, shutting down");
        }
    }

    cancel.cancel();

    // Give tasks 15 seconds to finish.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);

    loop {
        tokio::select! {
            result = set.join_next() => {
                match result {
                    None => break,
                    Some(Ok(())) => {}
                    Some(Err(e)) => {
                        error!(error = %e, "task panicked during shutdown");
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                warn!("timeout waiting for tasks to shut down");
                set.abort_all();
                break;
            }
        }
    }
}
