//! Tokio-based runtime implementation.

use derive_more::Debug;
use roxy_traits::{Clock, Handle, Signal, Spawner};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Tokio-based runtime context.
#[derive(Clone, Debug)]
pub struct TokioContext {
    label: String,
    #[debug(skip)]
    stop_tx: Arc<watch::Sender<Option<i32>>>,
    #[debug(skip)]
    stop_rx: watch::Receiver<Option<i32>>,
}

impl TokioContext {
    /// Create a new Tokio context.
    pub fn new() -> Self {
        let (stop_tx, stop_rx) = watch::channel(None);
        Self {
            label: String::new(),
            stop_tx: Arc::new(stop_tx),
            stop_rx,
        }
    }
}

impl Default for TokioContext {
    fn default() -> Self {
        Self::new()
    }
}

impl Spawner for TokioContext {
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
            stop_tx: self.stop_tx.clone(),
            stop_rx: self.stop_rx.clone(),
        }
    }

    async fn stop(&self, code: i32, timeout: Option<Duration>) {
        let _ = self.stop_tx.send(Some(code));
        if let Some(timeout) = timeout {
            tokio::time::sleep(timeout).await;
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

impl Clock for TokioContext {
    fn now(&self) -> Instant {
        Instant::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}
