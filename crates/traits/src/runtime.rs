//! Runtime traits for task spawning, time, and metrics.

use std::{
    future::Future,
    time::{Duration, Instant},
};

use derive_more::{Debug, Display, Error, From};

/// Handle to a spawned task.
#[derive(Debug)]
#[debug("Handle {{ .. }}")]
#[must_use = "task handles should be awaited or stored"]
pub struct Handle<T> {
    inner: tokio::task::JoinHandle<T>,
}

impl<T> Handle<T> {
    /// Create a new handle from a tokio join handle.
    pub const fn new(inner: tokio::task::JoinHandle<T>) -> Self {
        Self { inner }
    }

    /// Wait for the task to complete.
    pub async fn join(self) -> Result<T, JoinError> {
        self.inner.await.map_err(JoinError)
    }

    /// Abort the task.
    pub fn abort(&self) {
        self.inner.abort();
    }
}

/// Error returned when joining a task fails.
#[derive(Debug, Display, Error, From)]
#[display("join error: {_0}")]
pub struct JoinError(pub tokio::task::JoinError);

/// Shutdown signal.
#[derive(Clone, Debug)]
pub enum Signal {
    /// The system is running.
    Open,
    /// The system is shutting down with the given exit code.
    Closed(i32),
}

/// Core runtime trait for task spawning.
pub trait Spawner: Clone + Send + Sync + 'static {
    /// Spawn a new async task.
    fn spawn<F, Fut, T>(&self, f: F) -> Handle<T>
    where
        F: FnOnce(Self) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static;

    /// Spawn with a label for metrics/tracing.
    #[must_use = "with_label returns a new labeled spawner, the original is unchanged"]
    fn with_label(&self, label: &str) -> Self;

    /// Signal shutdown with exit code.
    fn stop(&self, code: i32, timeout: Option<Duration>) -> impl Future<Output = ()> + Send;

    /// Check if shutdown was requested.
    fn stopped(&self) -> impl Future<Output = Signal> + Send;
}

/// Clock abstraction for time operations.
pub trait Clock: Send + Sync + 'static {
    /// Get the current instant.
    fn now(&self) -> Instant;

    /// Sleep for the given duration.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

/// Metrics counter.
pub trait Counter: Send + Sync + 'static {
    /// Increment the counter by 1.
    fn inc(&self);

    /// Increment the counter by the given value.
    fn inc_by(&self, value: u64);
}

/// Metrics histogram.
pub trait Histogram: Send + Sync + 'static {
    /// Record a value in the histogram.
    fn record(&self, value: f64);
}

/// Metrics gauge.
pub trait Gauge: Send + Sync + 'static {
    /// Set the gauge to the given value.
    fn set(&self, value: f64);

    /// Increment the gauge by 1.
    fn inc(&self);

    /// Decrement the gauge by 1.
    fn dec(&self);
}

/// Metrics registry for creating and registering metrics.
pub trait MetricsRegistry: Send + Sync + 'static {
    /// Counter type produced by this registry.
    type Counter: Counter;
    /// Histogram type produced by this registry.
    type Histogram: Histogram;
    /// Gauge type produced by this registry.
    type Gauge: Gauge;

    /// Create and register a counter.
    fn counter(&self, name: &str, help: &str) -> Self::Counter;

    /// Create and register a histogram with the given buckets.
    fn histogram(&self, name: &str, help: &str, buckets: &[f64]) -> Self::Histogram;

    /// Create and register a gauge.
    fn gauge(&self, name: &str, help: &str) -> Self::Gauge;
}
