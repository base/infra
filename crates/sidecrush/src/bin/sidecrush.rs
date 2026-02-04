//! Sidecrush - Unified sidecar daemon for Base nodes.
//!
//! Subcommands:
//! - `daemon` - Run the health check daemon with HTTP server
//! - `init` - Initialize node (snapshots, JWT) [future]
//! - `ctl` - Control operations (transfer leader, etc) [future]

use std::net::SocketAddr;
use std::sync::atomic::AtomicU8;
use std::sync::Arc;

use cadence::{StatsdClient, UdpMetricSink};
use clap::{Parser, Subcommand};
use std::net::UdpSocket;
use tokio::net::TcpListener;
use tracing::Level;

use sidecrush::api::{create_router, AppState};
use sidecrush::blockbuilding_healthcheck::{
    alloy_client::AlloyEthClient, BlockProductionHealthChecker, HealthcheckConfig, Node,
};
use sidecrush::health::HealthStatus;
use sidecrush::metrics::HealthcheckMetrics;

#[derive(Parser, Debug)]
#[command(name = "sidecrush", author, version, about = "Unified sidecar daemon for Base nodes")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the health check daemon with HTTP server
    Daemon(DaemonArgs),
    /// Initialize node (snapshots, JWT) - not yet implemented
    Init(InitArgs),
    /// Control operations (transfer leader, etc) - not yet implemented
    Ctl(CtlArgs),
}

#[derive(Parser, Debug)]
struct DaemonArgs {
    // --- HTTP Server ---
    /// Port for the HTTP health server
    #[arg(long, env = "SIDECRUSH_LISTEN_PORT", default_value_t = 3000)]
    listen_port: u16,

    /// KALEY TODO check if this is needed/wanted
    /// Enable debug endpoints (/debug/override/health) for runtime health override.
    /// Not present in Go Health Service; added for operational convenience.
    /// Use with caution in production - allows bypassing health checks without restart.
    #[arg(long, env = "SIDECRUSH_ENABLE_DEBUG_ENDPOINTS", default_value_t = false)]
    enable_debug_endpoints: bool,

    // --- Node Configuration ---
    /// Ethereum node HTTP RPC URL
    #[arg(long, env = "SIDECRUSH_NODE_URL")]
    node_url: Option<String>,

    /// Poll interval in milliseconds
    #[arg(long, env = "SIDECRUSH_POLL_INTERVAL_MS", default_value_t = 1000)]
    poll_interval_ms: u64,

    /// Grace period in milliseconds before considering delayed
    #[arg(long, env = "SIDECRUSH_GRACE_PERIOD_MS", default_value_t = 2000)]
    grace_period_ms: u64,

    /// Threshold in milliseconds to consider unhealthy/stalled
    #[arg(long, env = "SIDECRUSH_UNHEALTHY_THRESHOLD_MS", default_value_t = 3000)]
    unhealthy_threshold_ms: u64,

    /// Treat node as a new instance on startup (suppresses initial errors until healthy)
    #[arg(long, env = "SIDECRUSH_NEW_INSTANCE", default_value_t = true)]
    new_instance: bool,

    // --- Logging ---
    /// Log level
    #[arg(long, env = "SIDECRUSH_LOG_LEVEL", default_value_t = Level::INFO)]
    log_level: Level,

    /// Log format (text|json)
    #[arg(long, env = "SIDECRUSH_LOG_FORMAT", default_value = "json")]
    log_format: String,
}

impl DaemonArgs {
    /// Resolve node URL with backward-compatible fallback to old env vars.
    fn resolve_node_url(&self) -> String {
        if let Some(ref url) = self.node_url {
            return url.clone();
        }
        // Backward compatibility: check old env vars
        if let Ok(url) = std::env::var("NODE_URL") {
            return url;
        }
        if let Ok(url) = std::env::var("BBHC_SIDECAR_GETH_RPC") {
            return url;
        }
        // Default
        "http://localhost:8545".to_string()
    }

    /// Resolve poll interval with backward-compatible fallback.
    fn resolve_poll_interval(&self) -> u64 {
        if let Ok(val) = std::env::var("BBHC_SIDECAR_POLL_INTERVAL_MS") {
            if let Ok(ms) = val.parse() {
                return ms;
            }
        }
        self.poll_interval_ms
    }

    /// Resolve grace period with backward-compatible fallback.
    fn resolve_grace_period(&self) -> u64 {
        if let Ok(val) = std::env::var("BBHC_SIDECAR_GRACE_PERIOD_MS") {
            if let Ok(ms) = val.parse() {
                return ms;
            }
        }
        self.grace_period_ms
    }

    /// Resolve unhealthy threshold with backward-compatible fallback.
    fn resolve_unhealthy_threshold(&self) -> u64 {
        if let Ok(val) = std::env::var("BBHC_SIDECAR_UNHEALTHY_NODE_THRESHOLD_MS") {
            if let Ok(ms) = val.parse() {
                return ms;
            }
        }
        self.unhealthy_threshold_ms
    }
}

#[derive(Parser, Debug)]
struct InitArgs {
    /// Placeholder for future init subcommand
    #[arg(long)]
    placeholder: Option<String>,
}

#[derive(Parser, Debug)]
struct CtlArgs {
    /// Placeholder for future ctl subcommand
    #[arg(long)]
    placeholder: Option<String>,
}

fn init_logging(log_format: &str, log_level: Level) {
    if log_format.to_lowercase() == "json" {
        let _ = tracing_subscriber::fmt()
            .json()
            .with_max_level(log_level)
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_max_level(log_level)
            .try_init();
    }
}

/// Create a StatsD client with the given prefix and standard Codeflow tags.
fn create_statsd_client_with_prefix(prefix: &str) -> StatsdClient {
    let statsd_host = std::env::var("DD_AGENT_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let statsd_addr = format!("{}:8125", statsd_host);

    let socket = UdpSocket::bind("0.0.0.0:0").expect("failed to bind UDP socket");
    socket
        .set_nonblocking(true)
        .expect("failed to set socket nonblocking");
    let sink =
        UdpMetricSink::from(statsd_addr.as_str(), socket).expect("failed to create StatsD sink");

    let config_name =
        std::env::var("CODEFLOW_CONFIG_NAME").unwrap_or_else(|_| "unknown".to_string());
    let environment =
        std::env::var("CODEFLOW_ENVIRONMENT").unwrap_or_else(|_| "unknown".to_string());
    let project_name =
        std::env::var("CODEFLOW_PROJECT_NAME").unwrap_or_else(|_| "unknown".to_string());
    let service_name =
        std::env::var("CODEFLOW_SERVICE_NAME").unwrap_or_else(|_| "unknown".to_string());

    StatsdClient::builder(prefix, sink)
        .with_tag("configname", &config_name)
        .with_tag("environment", &environment)
        .with_tag("projectname", &project_name)
        .with_tag("servicename", &service_name)
        .build()
}

/// Create both StatsD clients needed for metrics.
fn create_statsd_clients() -> (StatsdClient, StatsdClient) {
    let statsd_host = std::env::var("DD_AGENT_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    tracing::info!(address = %format!("{}:8125", statsd_host), "Connecting to StatsD agent");

    // Prefixed client for base.blocks.* metrics (health counters, latest_block_number)
    let client = create_statsd_client_with_prefix("base.blocks");
    // Unprefixed client for node_service.agent.* metrics (volume stats)
    let volume_client = create_statsd_client_with_prefix("");

    let config_name =
        std::env::var("CODEFLOW_CONFIG_NAME").unwrap_or_else(|_| "unknown".to_string());
    let environment =
        std::env::var("CODEFLOW_ENVIRONMENT").unwrap_or_else(|_| "unknown".to_string());
    let project_name =
        std::env::var("CODEFLOW_PROJECT_NAME").unwrap_or_else(|_| "unknown".to_string());
    let service_name =
        std::env::var("CODEFLOW_SERVICE_NAME").unwrap_or_else(|_| "unknown".to_string());

    tracing::info!(
        configname = %config_name,
        environment = %environment,
        projectname = %project_name,
        servicename = %service_name,
        "Initialized StatsD clients with tags"
    );

    (client, volume_client)
}

async fn run_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    init_logging(&args.log_format, args.log_level);

    let (client, volume_client) = create_statsd_clients();
    let metrics = HealthcheckMetrics::new(client, volume_client);

    // Resolve configuration with backward compatibility
    let node_url = args.resolve_node_url();
    let poll_interval_ms = args.resolve_poll_interval();
    let grace_period_ms = args.resolve_grace_period();
    let unhealthy_threshold_ms = args.resolve_unhealthy_threshold();

    tracing::info!(
        node_url = %node_url,
        poll_interval_ms = poll_interval_ms,
        grace_period_ms = grace_period_ms,
        unhealthy_threshold_ms = unhealthy_threshold_ms,
        listen_port = args.listen_port,
        "Starting sidecrush daemon"
    );

    let node = Node::new(node_url.clone(), args.new_instance);
    let client = AlloyEthClient::new_http(&node_url)?;
    let config = HealthcheckConfig::new(poll_interval_ms, grace_period_ms, unhealthy_threshold_ms);

    let checker = BlockProductionHealthChecker::new(node, client, config, metrics);

    // Get the status handle to share with HTTP server
    let health_status = checker.status_handle();
    // For startup health, we use the same status for now (will be wrapped with OnceHealthyAlwaysHealthy later)
    let startup_status = Arc::new(AtomicU8::new(HealthStatus::Healthy.code()));

    // Spawn decoupled status emitter at 2s cadence. TODO: make config var?
    let _status_emitter = checker.spawn_status_emitter(2000);

    // Create HTTP server
    let app_state = AppState::new(health_status, startup_status);
    let app = create_router(app_state, args.enable_debug_endpoints);
    let addr = SocketAddr::from(([0, 0, 0, 0], args.listen_port));

    tracing::info!(address = %addr, "Starting HTTP server");
    let listener = TcpListener::bind(addr).await?;

    // Setup graceful shutdown
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    // Spawn signal handler
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Shutdown signal received");
        let _ = shutdown_tx.send(());
    });

    // Run health check polling in background
    let health_check_handle = tokio::spawn(async move {
        checker.poll_for_health_checks().await;
    });

    // Run HTTP server with graceful shutdown
    let server = axum::serve(listener, app).with_graceful_shutdown(async {
        shutdown_rx.await.ok();
        tracing::info!("Shutting down HTTP server");
    });

    // Wait for server to complete
    if let Err(e) = server.await {
        tracing::error!(error = %e, "HTTP server error");
    }

    // Abort background tasks
    health_check_handle.abort();

    tracing::info!("Sidecrush daemon shutdown complete");
    Ok(())
}

async fn run_init(_args: InitArgs) -> anyhow::Result<()> {
    tracing::warn!("Init subcommand not yet implemented");
    Ok(())
}

async fn run_ctl(_args: CtlArgs) -> anyhow::Result<()> {
    tracing::warn!("Ctl subcommand not yet implemented");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon(args) => run_daemon(args).await,
        Commands::Init(args) => run_init(args).await,
        Commands::Ctl(args) => run_ctl(args).await,
    }
}
