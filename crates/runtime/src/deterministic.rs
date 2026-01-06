//! Deterministic runtime for testing.
//!
//! This module provides a deterministic runtime implementation that allows
//! for controlled time advancement, making tests reproducible and fast.

use std::{
    collections::BinaryHeap,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use derive_more::Debug;
use roxy_traits::{Clock, Handle, Signal, Spawner};
use tokio::sync::{oneshot, watch};

/// A pending timer in the deterministic runtime.
struct PendingTimer {
    deadline: Instant,
    tx: oneshot::Sender<()>,
}

impl Eq for PendingTimer {}

impl PartialEq for PendingTimer {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
    }
}

impl Ord for PendingTimer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse order for min-heap behavior
        other.deadline.cmp(&self.deadline)
    }
}

impl PartialOrd for PendingTimer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Shared state for the deterministic runtime.
#[derive(Debug)]
struct DeterministicState {
    /// The current simulated time.
    #[debug("{:?}", now)]
    now: Instant,
    /// Pending timers (min-heap by deadline).
    #[debug("{} pending timers", timers.len())]
    timers: BinaryHeap<PendingTimer>,
}

/// A deterministic runtime context for testing.
///
/// This context provides:
/// - Controllable time advancement via `advance_time`
/// - Reproducible task execution
/// - Fast tests without real sleeps
#[derive(Clone, Debug)]
pub struct DeterministicContext {
    label: String,
    #[debug(skip)]
    state: Arc<Mutex<DeterministicState>>,
    #[debug(skip)]
    stop_tx: Arc<watch::Sender<Option<i32>>>,
    #[debug(skip)]
    stop_rx: watch::Receiver<Option<i32>>,
}

impl DeterministicContext {
    /// Create a new deterministic context.
    pub fn new() -> Self {
        let (stop_tx, stop_rx) = watch::channel(None);
        Self {
            label: String::new(),
            state: Arc::new(Mutex::new(DeterministicState {
                now: Instant::now(),
                timers: BinaryHeap::new(),
            })),
            stop_tx: Arc::new(stop_tx),
            stop_rx,
        }
    }

    /// Advance time by the given duration.
    ///
    /// This wakes up any timers that have expired.
    pub fn advance_time(&self, duration: Duration) {
        let mut state = self.state.lock().unwrap();
        state.now += duration;
        let now = state.now;

        // Wake up all expired timers
        while let Some(timer) = state.timers.peek() {
            if timer.deadline <= now {
                let timer = state.timers.pop().unwrap();
                let _ = timer.tx.send(());
            } else {
                break;
            }
        }
    }

    /// Advance time to the next pending timer.
    ///
    /// Returns `true` if a timer was advanced to, `false` if no timers are pending.
    pub fn advance_to_next_timer(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if let Some(timer) = state.timers.pop() {
            state.now = timer.deadline;
            let _ = timer.tx.send(());
            true
        } else {
            false
        }
    }

    /// Get the number of pending timers.
    pub fn pending_timer_count(&self) -> usize {
        self.state.lock().unwrap().timers.len()
    }

    /// Get the simulated current time.
    pub fn current_time(&self) -> Instant {
        self.state.lock().unwrap().now
    }
}

impl Default for DeterministicContext {
    fn default() -> Self {
        Self::new()
    }
}

impl Spawner for DeterministicContext {
    fn spawn<F, Fut, T>(&self, f: F) -> Handle<T>
    where
        F: FnOnce(Self) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let ctx = self.clone();
        Handle::new(tokio::spawn(f(ctx)))
    }

    fn with_label(&self, label: &str) -> Self {
        Self {
            label: if self.label.is_empty() {
                label.to_string()
            } else {
                format!("{}:{}", self.label, label)
            },
            state: self.state.clone(),
            stop_tx: self.stop_tx.clone(),
            stop_rx: self.stop_rx.clone(),
        }
    }

    async fn stop(&self, code: i32, timeout: Option<Duration>) {
        let _ = self.stop_tx.send(Some(code));
        if let Some(timeout) = timeout {
            self.advance_time(timeout);
        }
    }

    async fn stopped(&self) -> Signal {
        let mut rx = self.stop_rx.clone();
        loop {
            if let Some(code) = *rx.borrow() {
                return Signal::Closed(code);
            }
            if rx.changed().await.is_err() {
                return Signal::Closed(-1);
            }
        }
    }
}

impl Clock for DeterministicContext {
    fn now(&self) -> Instant {
        self.state.lock().unwrap().now
    }

    async fn sleep(&self, duration: Duration) {
        let (tx, rx) = oneshot::channel();
        let deadline = {
            let mut state = self.state.lock().unwrap();
            let deadline = state.now + duration;
            state.timers.push(PendingTimer { deadline, tx });
            deadline
        };

        // Wait for the timer to be triggered by advance_time
        // or return immediately if deadline has already passed
        {
            let state = self.state.lock().unwrap();
            if state.now >= deadline {
                return;
            }
        }

        // Wait for the timer to fire
        let _ = rx.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_deterministic_time_advancement() {
        let ctx = DeterministicContext::new();
        let start = ctx.now();

        ctx.advance_time(Duration::from_secs(10));

        let elapsed = ctx.now().duration_since(start);
        assert_eq!(elapsed, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn test_deterministic_sleep_wakes_on_advance() {
        let ctx = DeterministicContext::new();
        let ctx_clone = ctx.clone();

        // Spawn a task that sleeps
        let handle = ctx.spawn(|c| async move {
            c.sleep(Duration::from_secs(5)).await;
            42
        });

        // Give the spawned task time to register its timer
        tokio::task::yield_now().await;

        assert_eq!(ctx_clone.pending_timer_count(), 1);

        // Advance time past the sleep duration
        ctx_clone.advance_time(Duration::from_secs(6));

        // The task should complete
        let result = handle.join().await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_advance_to_next_timer() {
        let ctx = DeterministicContext::new();
        let ctx_clone = ctx.clone();

        // Spawn a task that sleeps for 100 seconds
        let handle = ctx.spawn(|c| async move {
            c.sleep(Duration::from_secs(100)).await;
            "done"
        });

        // Give the spawned task time to register its timer
        tokio::task::yield_now().await;

        // Advance directly to the next timer
        let advanced = ctx_clone.advance_to_next_timer();
        assert!(advanced);

        let result = handle.join().await.unwrap();
        assert_eq!(result, "done");
    }

    #[tokio::test]
    async fn test_multiple_timers_in_order() {
        let ctx = DeterministicContext::new();
        let ctx_clone = ctx.clone();
        let order = Arc::new(Mutex::new(Vec::new()));

        let order1 = order.clone();
        let _h1 = ctx.spawn(|c| async move {
            c.sleep(Duration::from_secs(10)).await;
            order1.lock().unwrap().push(1);
        });

        let order2 = order.clone();
        let _h2 = ctx.spawn(|c| async move {
            c.sleep(Duration::from_secs(5)).await;
            order2.lock().unwrap().push(2);
        });

        let order3 = order.clone();
        let _h3 = ctx.spawn(|c| async move {
            c.sleep(Duration::from_secs(15)).await;
            order3.lock().unwrap().push(3);
        });

        // Give spawned tasks time to register timers
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        assert_eq!(ctx_clone.pending_timer_count(), 3);

        // Advance time past all sleeps
        ctx_clone.advance_time(Duration::from_secs(20));

        // Give tasks time to complete
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        // Timers should fire in order: 5s, 10s, 15s
        let completed = order.lock().unwrap();
        assert_eq!(*completed, vec![2, 1, 3]);
    }

    #[tokio::test]
    async fn test_with_label() {
        let ctx = DeterministicContext::new();
        let labeled = ctx.with_label("test");
        let nested = labeled.with_label("nested");

        // Labels should concatenate
        assert!(nested.label.contains("test"));
        assert!(nested.label.contains("nested"));
    }

    #[tokio::test]
    async fn test_stop_signal() {
        let ctx = DeterministicContext::new();
        let ctx_clone = ctx.clone();

        let handle = ctx.spawn(|c| async move {
            match c.stopped().await {
                Signal::Closed(code) => code,
                Signal::Open => -999,
            }
        });

        ctx_clone.stop(42, None).await;

        let result = handle.join().await.unwrap();
        assert_eq!(result, 42);
    }
}
