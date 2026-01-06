//! Connection state machine for backend lifecycle management.

use std::time::{Duration, Instant};

/// Connection state for a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Backend is connected and healthy.
    Connected,
    /// Backend is connecting (initial or reconnecting).
    Connecting {
        /// Number of connection attempts made.
        attempts: u32,
    },
    /// Backend is disconnected due to errors.
    Disconnected {
        /// When the backend was disconnected.
        since: Instant,
        /// Number of consecutive failures.
        failures: u32,
    },
    /// Backend is temporarily banned.
    Banned {
        /// When the ban expires.
        until: Instant,
        /// Reason for the ban.
        failures: u32,
    },
}

impl ConnectionState {
    /// Check if the backend should accept requests.
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Connected)
    }

    /// Check if the backend should attempt reconnection.
    pub fn should_reconnect(&self) -> bool {
        match self {
            Self::Disconnected { .. } => true,
            Self::Banned { until, .. } => Instant::now() >= *until,
            _ => false,
        }
    }
}

/// Configuration for connection state machine.
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// Base delay for exponential backoff.
    pub base_delay: Duration,
    /// Maximum delay for exponential backoff.
    pub max_delay: Duration,
    /// Maximum number of retries before banning.
    pub max_retries: u32,
    /// Duration of a ban.
    pub ban_duration: Duration,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            max_retries: 5,
            ban_duration: Duration::from_secs(60),
        }
    }
}

/// Connection state machine.
#[derive(Debug)]
pub struct ConnectionStateMachine {
    state: ConnectionState,
    config: ConnectionConfig,
}

impl ConnectionStateMachine {
    /// Create a new connection state machine.
    pub const fn new(config: ConnectionConfig) -> Self {
        Self {
            state: ConnectionState::Connecting { attempts: 0 },
            config,
        }
    }

    /// Get the current state.
    pub const fn state(&self) -> ConnectionState {
        self.state
    }

    /// Check if the backend is available for requests.
    pub const fn is_available(&self) -> bool {
        self.state.is_available()
    }

    /// Record a successful connection/request.
    pub const fn on_success(&mut self) {
        self.state = ConnectionState::Connected;
    }

    /// Record a connection/request failure.
    pub fn on_failure(&mut self) {
        self.state = match self.state {
            ConnectionState::Connected => ConnectionState::Disconnected {
                since: Instant::now(),
                failures: 1,
            },
            ConnectionState::Connecting { attempts } => {
                let new_attempts = attempts + 1;
                if new_attempts >= self.config.max_retries {
                    ConnectionState::Banned {
                        until: Instant::now() + self.config.ban_duration,
                        failures: new_attempts,
                    }
                } else {
                    ConnectionState::Connecting {
                        attempts: new_attempts,
                    }
                }
            }
            ConnectionState::Disconnected { failures, .. } => {
                let new_failures = failures + 1;
                if new_failures >= self.config.max_retries {
                    ConnectionState::Banned {
                        until: Instant::now() + self.config.ban_duration,
                        failures: new_failures,
                    }
                } else {
                    ConnectionState::Disconnected {
                        since: Instant::now(),
                        failures: new_failures,
                    }
                }
            }
            ConnectionState::Banned { failures, .. } => {
                // Still banned, increment failures
                ConnectionState::Banned {
                    until: Instant::now() + self.config.ban_duration,
                    failures: failures + 1,
                }
            }
        };
    }

    /// Get the backoff duration before next retry.
    pub fn backoff_duration(&self) -> Duration {
        match self.state {
            ConnectionState::Connecting { attempts } | ConnectionState::Disconnected { failures: attempts, .. } => {
                let delay = self
                    .config
                    .base_delay
                    .saturating_mul(2u32.saturating_pow(attempts));
                delay.min(self.config.max_delay)
            }
            ConnectionState::Banned { until, .. } => {
                until.saturating_duration_since(Instant::now())
            }
            ConnectionState::Connected => Duration::ZERO,
        }
    }

    /// Attempt to transition from banned/disconnected to connecting.
    pub fn try_reconnect(&mut self) -> bool {
        if self.state.should_reconnect() {
            self.state = ConnectionState::Connecting { attempts: 0 };
            true
        } else {
            false
        }
    }

    /// Reset the state machine to initial connecting state.
    pub const fn reset(&mut self) {
        self.state = ConnectionState::Connecting { attempts: 0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let sm = ConnectionStateMachine::new(ConnectionConfig::default());
        assert!(matches!(sm.state(), ConnectionState::Connecting { attempts: 0 }));
        assert!(!sm.is_available());
    }

    #[test]
    fn test_success_transitions_to_connected() {
        let mut sm = ConnectionStateMachine::new(ConnectionConfig::default());
        sm.on_success();
        assert!(matches!(sm.state(), ConnectionState::Connected));
        assert!(sm.is_available());
    }

    #[test]
    fn test_failure_from_connected() {
        let mut sm = ConnectionStateMachine::new(ConnectionConfig::default());
        sm.on_success();
        sm.on_failure();
        assert!(matches!(sm.state(), ConnectionState::Disconnected { failures: 1, .. }));
        assert!(!sm.is_available());
    }

    #[test]
    fn test_repeated_failures_lead_to_ban() {
        let config = ConnectionConfig {
            max_retries: 3,
            ..Default::default()
        };
        let mut sm = ConnectionStateMachine::new(config);

        sm.on_failure(); // attempts: 1
        sm.on_failure(); // attempts: 2
        sm.on_failure(); // attempts: 3 -> banned

        assert!(matches!(sm.state(), ConnectionState::Banned { .. }));
    }

    #[test]
    fn test_backoff_exponential() {
        let config = ConnectionConfig {
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            max_retries: 10,
            ..Default::default()
        };
        let mut sm = ConnectionStateMachine::new(config);

        // First attempt: 100ms
        assert_eq!(sm.backoff_duration(), Duration::from_millis(100));

        sm.on_failure();
        // Second attempt: 200ms
        assert_eq!(sm.backoff_duration(), Duration::from_millis(200));

        sm.on_failure();
        // Third attempt: 400ms
        assert_eq!(sm.backoff_duration(), Duration::from_millis(400));
    }

    #[test]
    fn test_backoff_capped_at_max() {
        let config = ConnectionConfig {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(5),
            max_retries: 10,
            ..Default::default()
        };
        let mut sm = ConnectionStateMachine::new(config);

        // Trigger many failures
        for _ in 0..5 {
            sm.on_failure();
        }

        // Should be capped at max_delay
        assert!(sm.backoff_duration() <= Duration::from_secs(5));
    }

    #[test]
    fn test_reset() {
        let mut sm = ConnectionStateMachine::new(ConnectionConfig::default());
        sm.on_success();
        sm.on_failure();
        sm.on_failure();

        sm.reset();

        assert!(matches!(sm.state(), ConnectionState::Connecting { attempts: 0 }));
    }
}
