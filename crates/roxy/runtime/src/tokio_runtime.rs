//! Tokio-based runtime implementation.

use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

use derive_more::Debug;
use roxy_traits::{Clock, Handle, Signal, Spawner};
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
    #[must_use]
    pub fn new() -> Self {
        let (stop_tx, stop_rx) = watch::channel(None);
        Self { label: String::new(), stop_tx: Arc::new(stop_tx), stop_rx }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_context() {
        let ctx = TokioContext::new();
        assert!(ctx.label.is_empty());
    }

    #[test]
    fn test_default_creates_context() {
        let ctx = TokioContext::default();
        assert!(ctx.label.is_empty());
    }

    #[tokio::test]
    async fn test_spawn_returns_result() {
        let ctx = TokioContext::new();
        let handle = ctx.spawn(|_| async { 42 });
        let result = handle.join().await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_spawn_receives_context() {
        let ctx = TokioContext::new().with_label("parent");
        let handle = ctx.spawn(|c| async move { c.label.clone() });
        let result = handle.join().await.unwrap();
        assert_eq!(result, "parent");
    }

    #[tokio::test]
    async fn test_spawn_multiple_tasks() {
        let ctx = TokioContext::new();

        let h1 = ctx.spawn(|_| async { 1 });
        let h2 = ctx.spawn(|_| async { 2 });
        let h3 = ctx.spawn(|_| async { 3 });

        let r1 = h1.join().await.unwrap();
        let r2 = h2.join().await.unwrap();
        let r3 = h3.join().await.unwrap();

        assert_eq!(r1 + r2 + r3, 6);
    }

    #[test]
    fn test_with_label_single() {
        let ctx = TokioContext::new();
        let labeled = ctx.with_label("test");
        assert_eq!(labeled.label, "test");
    }

    #[test]
    fn test_with_label_nested() {
        let ctx = TokioContext::new();
        let first = ctx.with_label("parent");
        let second = first.with_label("child");
        assert_eq!(second.label, "parent:child");
    }

    #[test]
    fn test_with_label_deeply_nested() {
        let ctx = TokioContext::new();
        let labeled = ctx.with_label("a").with_label("b").with_label("c");
        assert_eq!(labeled.label, "a:b:c");
    }

    #[test]
    fn test_with_label_preserves_stop_channel() {
        let ctx = TokioContext::new();
        let labeled = ctx.with_label("test");
        assert!(Arc::ptr_eq(&ctx.stop_tx, &labeled.stop_tx));
    }

    #[tokio::test]
    async fn test_stop_signals_stopped() {
        let ctx = TokioContext::new();
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

    #[tokio::test]
    async fn test_stop_with_different_codes() {
        for code in [0, 1, -1, 100, i32::MAX, i32::MIN] {
            let ctx = TokioContext::new();
            let ctx_clone = ctx.clone();

            let handle = ctx.spawn(|c| async move {
                match c.stopped().await {
                    Signal::Closed(c) => c,
                    Signal::Open => -999,
                }
            });

            ctx_clone.stop(code, None).await;

            let result = handle.join().await.unwrap();
            assert_eq!(result, code);
        }
    }

    #[tokio::test]
    async fn test_stop_propagates_to_labeled_context() {
        let ctx = TokioContext::new();
        let labeled = ctx.with_label("child");

        let handle = labeled.spawn(|c| async move {
            match c.stopped().await {
                Signal::Closed(code) => code,
                Signal::Open => -999,
            }
        });

        ctx.stop(123, None).await;

        let result = handle.join().await.unwrap();
        assert_eq!(result, 123);
    }

    #[tokio::test]
    async fn test_now_returns_increasing_time() {
        let ctx = TokioContext::new();
        let t1 = ctx.now();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let t2 = ctx.now();
        assert!(t2 > t1);
    }

    #[tokio::test]
    async fn test_sleep_waits_minimum_duration() {
        let ctx = TokioContext::new();
        let sleep_duration = Duration::from_millis(50);

        let start = Instant::now();
        ctx.sleep(sleep_duration).await;
        let elapsed = start.elapsed();

        assert!(elapsed >= sleep_duration);
    }

    #[tokio::test]
    async fn test_clone_shares_state() {
        let ctx1 = TokioContext::new();
        let ctx2 = ctx1.clone();
        assert!(Arc::ptr_eq(&ctx1.stop_tx, &ctx2.stop_tx));
    }

    #[tokio::test]
    async fn test_debug_impl() {
        let ctx = TokioContext::new().with_label("test");
        let debug_str = format!("{:?}", ctx);
        assert!(debug_str.contains("TokioContext"));
        assert!(debug_str.contains("test"));
    }
}
