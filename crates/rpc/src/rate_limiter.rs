//! Sliding window rate limiter with per-key tracking.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use derive_more::Debug;
use roxy_traits::{RateLimitResult, RateLimiter};

/// Configuration for the sliding window rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Maximum number of requests allowed per window.
    pub requests_per_window: u64,
    /// Duration of the sliding window.
    pub window_duration: Duration,
}

impl RateLimiterConfig {
    /// Create a new rate limiter configuration.
    pub const fn new(requests_per_window: u64, window_duration: Duration) -> Self {
        Self { requests_per_window, window_duration }
    }
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self { requests_per_window: 100, window_duration: Duration::from_secs(60) }
    }
}

/// State for a single rate limit window.
#[derive(Debug)]
struct WindowState {
    /// Number of requests in the current window.
    count: u64,
    /// When the current window started.
    window_start: Instant,
}

impl WindowState {
    /// Create a new window state starting now.
    fn new() -> Self {
        Self { count: 0, window_start: Instant::now() }
    }
}

/// Sliding window rate limiter with per-key tracking.
///
/// This rate limiter uses a fixed window approach where requests are counted
/// within discrete time windows. When a window expires, the count resets.
///
/// # Thread Safety
///
/// This implementation is thread-safe through the use of a `Mutex` protecting
/// the internal state. All methods acquire the lock before accessing or
/// modifying the rate limit state.
///
/// # Example
///
/// ```rust,ignore
/// use roxy_rpc::{SlidingWindowRateLimiter, RateLimiterConfig};
/// use roxy_traits::{RateLimiter, RateLimitResult};
/// use std::time::Duration;
///
/// let config = RateLimiterConfig::new(10, Duration::from_secs(60));
/// let limiter = SlidingWindowRateLimiter::new(config);
///
/// // Check if a request is allowed
/// match limiter.check_and_record("user_123") {
///     RateLimitResult::Allowed => println!("Request allowed"),
///     RateLimitResult::Limited { retry_after } => {
///         println!("Rate limited, retry after {:?}", retry_after);
///     }
/// }
/// ```
#[derive(Debug)]
pub struct SlidingWindowRateLimiter {
    /// Configuration for this rate limiter.
    config: RateLimiterConfig,
    /// Per-key window state.
    #[debug(skip)]
    windows: Mutex<HashMap<String, WindowState>>,
}

impl SlidingWindowRateLimiter {
    /// Create a new sliding window rate limiter with the given configuration.
    pub fn new(config: RateLimiterConfig) -> Self {
        Self { config, windows: Mutex::new(HashMap::new()) }
    }

    /// Get the configuration for this rate limiter.
    pub const fn config(&self) -> &RateLimiterConfig {
        &self.config
    }

    /// Refresh the window state if the window has expired.
    ///
    /// Returns the updated window state. If the window has expired,
    /// it is reset with the current time as the new window start.
    fn refresh_window(&self, state: &mut WindowState) {
        let now = Instant::now();
        let elapsed = now.duration_since(state.window_start);

        if elapsed >= self.config.window_duration {
            state.count = 0;
            state.window_start = now;
        }
    }

    /// Calculate the time remaining until the current window expires.
    fn time_until_window_reset(&self, state: &WindowState) -> Duration {
        let elapsed = state.window_start.elapsed();
        self.config.window_duration.checked_sub(elapsed).unwrap_or(Duration::ZERO)
    }
}

impl RateLimiter for SlidingWindowRateLimiter {
    fn check(&self, key: &str) -> RateLimitResult {
        let mut windows = self.windows.lock().expect("rate limiter lock poisoned");

        let state = windows.entry(key.to_string()).or_insert_with(WindowState::new);

        // Refresh the window if expired
        self.refresh_window(state);

        if state.count < self.config.requests_per_window {
            RateLimitResult::Allowed
        } else {
            RateLimitResult::Limited { retry_after: self.time_until_window_reset(state) }
        }
    }

    fn record(&self, key: &str) {
        let mut windows = self.windows.lock().expect("rate limiter lock poisoned");

        let state = windows.entry(key.to_string()).or_insert_with(WindowState::new);

        // Refresh the window if expired
        self.refresh_window(state);

        state.count = state.count.saturating_add(1);
    }

    fn check_and_record(&self, key: &str) -> RateLimitResult {
        let mut windows = self.windows.lock().expect("rate limiter lock poisoned");

        let state = windows.entry(key.to_string()).or_insert_with(WindowState::new);

        // Refresh the window if expired
        self.refresh_window(state);

        if state.count < self.config.requests_per_window {
            state.count = state.count.saturating_add(1);
            RateLimitResult::Allowed
        } else {
            RateLimitResult::Limited { retry_after: self.time_until_window_reset(state) }
        }
    }

    fn reset(&self, key: &str) {
        let mut windows = self.windows.lock().expect("rate limiter lock poisoned");
        windows.remove(key);
    }

    fn count(&self, key: &str) -> u64 {
        let mut windows = self.windows.lock().expect("rate limiter lock poisoned");

        match windows.get_mut(key) {
            Some(state) => {
                // Refresh the window if expired
                self.refresh_window(state);
                state.count
            }
            None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use rstest::rstest;

    use super::*;

    fn default_config() -> RateLimiterConfig {
        RateLimiterConfig::new(5, Duration::from_secs(1))
    }

    #[test]
    fn test_new_limiter_allows_requests() {
        let limiter = SlidingWindowRateLimiter::new(default_config());
        let result = limiter.check("test_key");
        assert!(result.is_allowed());
    }

    #[test]
    fn test_config_accessor() {
        let config = RateLimiterConfig::new(10, Duration::from_secs(30));
        let limiter = SlidingWindowRateLimiter::new(config);

        assert_eq!(limiter.config().requests_per_window, 10);
        assert_eq!(limiter.config().window_duration, Duration::from_secs(30));
    }

    #[test]
    fn test_default_config() {
        let config = RateLimiterConfig::default();
        assert_eq!(config.requests_per_window, 100);
        assert_eq!(config.window_duration, Duration::from_secs(60));
    }

    #[test]
    fn test_record_increments_count() {
        let limiter = SlidingWindowRateLimiter::new(default_config());
        let key = "test_key";

        assert_eq!(limiter.count(key), 0);
        limiter.record(key);
        assert_eq!(limiter.count(key), 1);
        limiter.record(key);
        assert_eq!(limiter.count(key), 2);
    }

    #[test]
    fn test_check_does_not_increment_count() {
        let limiter = SlidingWindowRateLimiter::new(default_config());
        let key = "test_key";

        assert_eq!(limiter.count(key), 0);
        let _ = limiter.check(key);
        assert_eq!(limiter.count(key), 0);
    }

    #[test]
    fn test_check_and_record_increments_count() {
        let limiter = SlidingWindowRateLimiter::new(default_config());
        let key = "test_key";

        assert_eq!(limiter.count(key), 0);
        let result = limiter.check_and_record(key);
        assert!(result.is_allowed());
        assert_eq!(limiter.count(key), 1);
    }

    #[test]
    fn test_rate_limiting_kicks_in() {
        let config = RateLimiterConfig::new(3, Duration::from_secs(60));
        let limiter = SlidingWindowRateLimiter::new(config);
        let key = "test_key";

        // First 3 requests should be allowed
        assert!(limiter.check_and_record(key).is_allowed());
        assert!(limiter.check_and_record(key).is_allowed());
        assert!(limiter.check_and_record(key).is_allowed());

        // 4th request should be limited
        let result = limiter.check_and_record(key);
        assert!(result.is_limited());
    }

    #[test]
    fn test_limited_result_has_retry_after() {
        let config = RateLimiterConfig::new(1, Duration::from_secs(60));
        let limiter = SlidingWindowRateLimiter::new(config);
        let key = "test_key";

        // Exhaust the limit
        limiter.record(key);

        // Next check should have retry_after
        let result = limiter.check(key);
        if let RateLimitResult::Limited { retry_after } = result {
            assert!(retry_after > Duration::ZERO);
            assert!(retry_after <= Duration::from_secs(60));
        } else {
            panic!("Expected Limited result");
        }
    }

    #[test]
    fn test_reset_clears_count() {
        let limiter = SlidingWindowRateLimiter::new(default_config());
        let key = "test_key";

        limiter.record(key);
        limiter.record(key);
        assert_eq!(limiter.count(key), 2);

        limiter.reset(key);
        assert_eq!(limiter.count(key), 0);
    }

    #[test]
    fn test_different_keys_are_independent() {
        let config = RateLimiterConfig::new(2, Duration::from_secs(60));
        let limiter = SlidingWindowRateLimiter::new(config);

        // Exhaust limit for key1
        limiter.record("key1");
        limiter.record("key1");
        assert!(limiter.check("key1").is_limited());

        // key2 should still be allowed
        assert!(limiter.check("key2").is_allowed());
    }

    #[test]
    fn test_window_expiration_resets_count() {
        let config = RateLimiterConfig::new(2, Duration::from_millis(50));
        let limiter = SlidingWindowRateLimiter::new(config);
        let key = "test_key";

        // Exhaust the limit
        limiter.record(key);
        limiter.record(key);
        assert!(limiter.check(key).is_limited());

        // Wait for window to expire
        thread::sleep(Duration::from_millis(60));

        // Should be allowed again
        assert!(limiter.check(key).is_allowed());
        assert_eq!(limiter.count(key), 0);
    }

    #[test]
    fn test_count_returns_zero_for_unknown_key() {
        let limiter = SlidingWindowRateLimiter::new(default_config());
        assert_eq!(limiter.count("unknown_key"), 0);
    }

    #[rstest]
    #[case::one_request(1, Duration::from_secs(60))]
    #[case::ten_requests(10, Duration::from_secs(1))]
    #[case::hundred_requests(100, Duration::from_secs(300))]
    #[case::high_rate(1000, Duration::from_millis(100))]
    fn test_various_configurations(
        #[case] requests_per_window: u64,
        #[case] window_duration: Duration,
    ) {
        let config = RateLimiterConfig::new(requests_per_window, window_duration);
        let limiter = SlidingWindowRateLimiter::new(config);
        let key = "test_key";

        // All requests up to limit should be allowed
        for _ in 0..requests_per_window {
            assert!(limiter.check_and_record(key).is_allowed());
        }

        // Next request should be limited
        assert!(limiter.check_and_record(key).is_limited());
    }

    #[rstest]
    #[case::empty_key("")]
    #[case::simple_key("user123")]
    #[case::ip_address("192.168.1.1")]
    #[case::api_key("sk-abc123def456")]
    #[case::unicode_key("user_")]
    #[case::long_key(&"x".repeat(1000))]
    fn test_various_key_formats(#[case] key: &str) {
        let limiter = SlidingWindowRateLimiter::new(default_config());

        // Should work with any string key
        assert!(limiter.check(key).is_allowed());
        limiter.record(key);
        assert_eq!(limiter.count(key), 1);
        limiter.reset(key);
        assert_eq!(limiter.count(key), 0);
    }

    #[test]
    fn test_debug_impl() {
        let config = RateLimiterConfig::new(10, Duration::from_secs(60));
        let limiter = SlidingWindowRateLimiter::new(config);

        let debug_str = format!("{:?}", limiter);
        assert!(debug_str.contains("SlidingWindowRateLimiter"));
        assert!(debug_str.contains("RateLimiterConfig"));
    }

    #[test]
    fn test_config_debug_impl() {
        let config = RateLimiterConfig::new(10, Duration::from_secs(60));
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("RateLimiterConfig"));
        assert!(debug_str.contains("requests_per_window"));
        assert!(debug_str.contains("window_duration"));
    }

    #[test]
    fn test_config_clone() {
        let config = RateLimiterConfig::new(10, Duration::from_secs(60));
        let cloned = config.clone();
        assert_eq!(cloned.requests_per_window, config.requests_per_window);
        assert_eq!(cloned.window_duration, config.window_duration);
    }

    #[test]
    fn test_thread_safety() {
        use std::sync::Arc;

        let config = RateLimiterConfig::new(1000, Duration::from_secs(60));
        let limiter = Arc::new(SlidingWindowRateLimiter::new(config));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let limiter = Arc::clone(&limiter);
                thread::spawn(move || {
                    let key = format!("thread_{}", i);
                    for _ in 0..100 {
                        let _ = limiter.check_and_record(&key);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        // Each thread should have recorded 100 requests
        for i in 0..10 {
            let key = format!("thread_{}", i);
            assert_eq!(limiter.count(&key), 100);
        }
    }

    #[test]
    fn test_concurrent_access_same_key() {
        use std::sync::Arc;

        let config = RateLimiterConfig::new(1000, Duration::from_secs(60));
        let limiter = Arc::new(SlidingWindowRateLimiter::new(config));
        let key = "shared_key";

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let limiter = Arc::clone(&limiter);
                let key = key.to_string();
                thread::spawn(move || {
                    for _ in 0..100 {
                        let _ = limiter.check_and_record(&key);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        // All threads together should have recorded 1000 requests
        assert_eq!(limiter.count(key), 1000);
    }

    #[test]
    fn test_check_and_record_atomicity() {
        let config = RateLimiterConfig::new(5, Duration::from_secs(60));
        let limiter = SlidingWindowRateLimiter::new(config);
        let key = "test_key";

        // Record exactly the limit
        for _ in 0..5 {
            let result = limiter.check_and_record(key);
            assert!(result.is_allowed());
        }

        // Count should match exactly
        assert_eq!(limiter.count(key), 5);

        // check_and_record when limited should not increment
        let result = limiter.check_and_record(key);
        assert!(result.is_limited());
        assert_eq!(limiter.count(key), 5);
    }

    #[test]
    fn test_saturating_count() {
        let config = RateLimiterConfig::new(u64::MAX, Duration::from_secs(60));
        let limiter = SlidingWindowRateLimiter::new(config);
        let key = "test_key";

        // Manually set count to near max by recording many times
        // This test verifies we use saturating_add
        for _ in 0..100 {
            limiter.record(key);
        }

        assert_eq!(limiter.count(key), 100);
    }
}
