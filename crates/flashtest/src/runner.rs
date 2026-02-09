//! Test runner adapted for channel-based output.

use std::time::{Duration, Instant};

use alloy_eips::BlockNumberOrTag;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::{
    TestClient,
    suite::{Test, TestSuite},
};

/// Result of running a single test.
#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    /// Name of the test.
    pub name: String,
    /// Category the test belongs to.
    pub category: String,
    /// Whether the test passed.
    pub passed: bool,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Error message if the test failed.
    pub error: Option<String>,
    /// Whether the test was skipped.
    pub skipped: bool,
    /// Reason for skipping if applicable.
    pub skip_reason: Option<String>,
}

/// Events emitted during test execution.
#[derive(Debug, Clone)]
pub enum TestEvent {
    /// A test is about to start.
    TestStarted {
        /// Category name.
        category: String,
        /// Test name.
        name: String,
    },
    /// A test passed.
    TestPassed {
        /// Category name.
        category: String,
        /// Test name.
        name: String,
        /// How long the test took.
        duration: Duration,
    },
    /// A test failed.
    TestFailed {
        /// Category name.
        category: String,
        /// Test name.
        name: String,
        /// How long the test took.
        duration: Duration,
        /// Error description.
        error: String,
    },
    /// A test was skipped.
    TestSkipped {
        /// Category name.
        category: String,
        /// Test name.
        name: String,
        /// Why it was skipped.
        reason: String,
    },
    /// The entire suite finished.
    SuiteComplete {
        /// Number of passing tests.
        passed: usize,
        /// Number of failing tests.
        failed: usize,
        /// Number of skipped tests.
        skipped: usize,
    },
}

/// Run tests, emitting events via the channel.
pub(crate) async fn run_tests(
    client: &TestClient,
    suite: &TestSuite,
    filter: Option<&str>,
    event_tx: &mpsc::Sender<TestEvent>,
) {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    // Check connection first
    match client.get_block_by_number(BlockNumberOrTag::Latest).await {
        Ok(Some(_)) | Ok(None) => {}
        Err(e) => {
            let _ = event_tx
                .send(TestEvent::TestFailed {
                    category: "setup".to_string(),
                    name: "connection".to_string(),
                    duration: Duration::ZERO,
                    error: e.to_string(),
                })
                .await;
            failed += 1;
            let _ = event_tx.send(TestEvent::SuiteComplete { passed, failed, skipped }).await;
            return;
        }
    }

    for category in &suite.categories {
        let category_tests: Vec<&Test> =
            category.tests.iter().filter(|t| matches_filter(&t.name, filter)).collect();

        if category_tests.is_empty() {
            continue;
        }

        for test in category_tests {
            let _ = event_tx
                .send(TestEvent::TestStarted {
                    category: category.name.clone(),
                    name: test.name.clone(),
                })
                .await;

            let result = run_single_test(client, &category.name, test).await;

            if result.skipped {
                skipped += 1;
                let _ = event_tx
                    .send(TestEvent::TestSkipped {
                        category: category.name.clone(),
                        name: test.name.clone(),
                        reason: result.skip_reason.unwrap_or_default(),
                    })
                    .await;
            } else if result.passed {
                passed += 1;
                let _ = event_tx
                    .send(TestEvent::TestPassed {
                        category: category.name.clone(),
                        name: test.name.clone(),
                        duration: Duration::from_millis(result.duration_ms),
                    })
                    .await;
            } else {
                failed += 1;
                let _ = event_tx
                    .send(TestEvent::TestFailed {
                        category: category.name.clone(),
                        name: test.name.clone(),
                        duration: Duration::from_millis(result.duration_ms),
                        error: result.error.unwrap_or_default(),
                    })
                    .await;
            }
        }
    }

    let _ = event_tx.send(TestEvent::SuiteComplete { passed, failed, skipped }).await;
}

/// Run a single test.
async fn run_single_test(client: &TestClient, category: &str, test: &Test) -> TestResult {
    let start = Instant::now();

    // Check skip condition
    if let Some(ref skip_fn) = test.skip_if
        && let Some(reason) = skip_fn(client).await
    {
        return TestResult {
            name: test.name.clone(),
            category: category.to_string(),
            passed: true,
            duration_ms: start.elapsed().as_millis() as u64,
            error: None,
            skipped: true,
            skip_reason: Some(reason),
        };
    }

    // Run the test
    let result = tokio::time::timeout(Duration::from_secs(30), (test.run)(client)).await;

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(())) => TestResult {
            name: test.name.clone(),
            category: category.to_string(),
            passed: true,
            duration_ms,
            error: None,
            skipped: false,
            skip_reason: None,
        },
        Ok(Err(e)) => TestResult {
            name: test.name.clone(),
            category: category.to_string(),
            passed: false,
            duration_ms,
            error: Some(format!("{e:#}")),
            skipped: false,
            skip_reason: None,
        },
        Err(_) => TestResult {
            name: test.name.clone(),
            category: category.to_string(),
            passed: false,
            duration_ms,
            error: Some("Test timed out after 30s".to_string()),
            skipped: false,
            skip_reason: None,
        },
    }
}

/// Check if test name matches filter.
fn matches_filter(name: &str, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(f) => {
            if f.contains('*') {
                let parts: Vec<&str> = f.split('*').collect();
                let mut remaining = name;
                for (i, part) in parts.iter().enumerate() {
                    if part.is_empty() {
                        continue;
                    }
                    if i == 0 {
                        if !remaining.starts_with(part) {
                            return false;
                        }
                        remaining = &remaining[part.len()..];
                    } else if i == parts.len() - 1 {
                        if !remaining.ends_with(part) {
                            return false;
                        }
                    } else if let Some(pos) = remaining.find(part) {
                        remaining = &remaining[pos + part.len()..];
                    } else {
                        return false;
                    }
                }
                true
            } else {
                name.contains(f)
            }
        }
    }
}
