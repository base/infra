//! End-to-end functional test suite for Base flashblocks RPC.

mod client;
pub use client::TestClient;

pub mod harness;
pub use harness::{FlashblockHarness, FlashblocksStream, WebSocketSubscription};

mod runner;
pub use runner::{TestEvent, TestResult};

pub mod simulator;
pub use simulator::{
    PrecompileConfig, Simulator, SimulatorConfig, SimulatorConfigBuilder, encode_run_call,
};

pub mod types;
pub use types::{
    Bundle, ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, Flashblock,
    FlashblockMetadata, MeterBundleResponse, MeteredPriorityFeeResponse, OpBlock,
    TransactionResult,
};

pub mod suite;
use std::time::Duration;

use alloy_primitives::Address;
use eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};
pub use suite::{SkipFn, Test, TestCategory, TestFn, TestSuite, build_test_suite};
use tokio::sync::mpsc;

/// Configuration for starting a flash test.
///
/// Chain-level settings (RPC URL, flashblocks WS URL) come from basectl's
/// `ChainConfig` (via `-c mainnet|sepolia|devnet`). This struct holds the
/// test-specific settings that can optionally be loaded from a YAML file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlashTestConfig {
    /// Optional private key for signing transactions.
    pub private_key: Option<String>,
    /// Optional recipient address for ETH transfers.
    pub recipient: Option<Address>,
    /// Optional Simulator contract address.
    pub simulator: Option<Address>,
    /// Optional test filter (glob pattern).
    pub filter: Option<String>,
}

impl FlashTestConfig {
    /// Load configuration from a YAML file.
    pub fn load(path: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read config file: {path}"))?;
        serde_yaml::from_str(&contents).wrap_err("failed to parse config YAML")
    }
}

/// Current phase of the flash test run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashTestPhase {
    /// Tests are currently running.
    Running,
    /// All tests have completed.
    Complete,
}

impl std::fmt::Display for FlashTestPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "Running"),
            Self::Complete => write!(f, "Complete"),
        }
    }
}

/// A single test result for display.
#[derive(Debug, Clone)]
pub struct FlashTestResultEntry {
    /// Category name.
    pub category: String,
    /// Test name.
    pub name: String,
    /// Test status.
    pub status: FlashTestStatus,
    /// Duration (if completed).
    pub duration: Option<Duration>,
    /// Error message (if failed).
    pub error: Option<String>,
    /// Skip reason (if skipped).
    pub skip_reason: Option<String>,
}

/// Status of a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashTestStatus {
    /// Test is currently running.
    Running,
    /// Test passed.
    Passed,
    /// Test failed.
    Failed,
    /// Test was skipped.
    Skipped,
}

/// Aggregated state of the flash test run, polled by the TUI.
#[derive(Debug)]
pub struct FlashTestState {
    /// Individual test results.
    pub results: Vec<FlashTestResultEntry>,
    /// Current phase.
    pub phase: FlashTestPhase,
    /// Number of passed tests.
    pub passed: usize,
    /// Number of failed tests.
    pub failed: usize,
    /// Number of skipped tests.
    pub skipped: usize,
    /// Total number of tests expected (may be 0 if unknown).
    pub total: usize,
    /// RPC URL being tested.
    pub rpc_url: String,
    /// Event channel receiver.
    event_rx: Option<mpsc::Receiver<TestEvent>>,
}

impl FlashTestState {
    /// Create a new state with an event channel.
    pub const fn new(event_rx: mpsc::Receiver<TestEvent>, rpc_url: String) -> Self {
        Self {
            results: Vec::new(),
            phase: FlashTestPhase::Running,
            passed: 0,
            failed: 0,
            skipped: 0,
            total: 0,
            rpc_url,
            event_rx: Some(event_rx),
        }
    }

    /// Drain all pending events from the channel, updating state.
    pub fn poll(&mut self) {
        let Some(ref mut rx) = self.event_rx else {
            return;
        };

        while let Ok(event) = rx.try_recv() {
            match event {
                TestEvent::TestStarted { category, name } => {
                    self.results.push(FlashTestResultEntry {
                        category,
                        name,
                        status: FlashTestStatus::Running,
                        duration: None,
                        error: None,
                        skip_reason: None,
                    });
                }
                TestEvent::TestPassed { category, name, duration } => {
                    if let Some(entry) = self
                        .results
                        .iter_mut()
                        .rev()
                        .find(|e| e.category == category && e.name == name)
                    {
                        entry.status = FlashTestStatus::Passed;
                        entry.duration = Some(duration);
                    }
                    self.passed += 1;
                }
                TestEvent::TestFailed { category, name, duration, error } => {
                    if let Some(entry) = self
                        .results
                        .iter_mut()
                        .rev()
                        .find(|e| e.category == category && e.name == name)
                    {
                        entry.status = FlashTestStatus::Failed;
                        entry.duration = Some(duration);
                        entry.error = Some(error);
                    } else {
                        // Connection failure before TestStarted
                        self.results.push(FlashTestResultEntry {
                            category,
                            name,
                            status: FlashTestStatus::Failed,
                            duration: Some(duration),
                            error: Some(error),
                            skip_reason: None,
                        });
                    }
                    self.failed += 1;
                }
                TestEvent::TestSkipped { category, name, reason } => {
                    if let Some(entry) = self
                        .results
                        .iter_mut()
                        .rev()
                        .find(|e| e.category == category && e.name == name)
                    {
                        entry.status = FlashTestStatus::Skipped;
                        entry.skip_reason = Some(reason);
                    }
                    self.skipped += 1;
                }
                TestEvent::SuiteComplete { passed, failed, skipped } => {
                    self.passed = passed;
                    self.failed = failed;
                    self.skipped = skipped;
                    self.total = passed + failed + skipped;
                    self.phase = FlashTestPhase::Complete;
                }
            }
        }
    }
}

/// Handle to a running flash test.
#[derive(Debug)]
pub struct FlashTestHandle {
    /// Event channel receiver.
    pub event_rx: mpsc::Receiver<TestEvent>,
}

/// Start a flash test and return a handle for receiving events.
///
/// `rpc_url` and `flashblocks_ws_url` come from basectl's `ChainConfig`.
/// `config` holds test-specific options (keys, addresses, filter).
pub async fn start_flash_test(
    rpc_url: &str,
    flashblocks_ws_url: &str,
    config: FlashTestConfig,
) -> Result<FlashTestHandle> {
    let client = TestClient::new(
        rpc_url,
        flashblocks_ws_url,
        config.private_key.as_deref(),
        config.recipient,
        config.simulator,
    )
    .await?;

    let suite = build_test_suite();
    let filter = config.filter.clone();

    let (event_tx, event_rx) = mpsc::channel(256);

    tokio::spawn(async move {
        runner::run_tests(&client, &suite, filter.as_deref(), &event_tx).await;
    });

    Ok(FlashTestHandle { event_rx })
}
