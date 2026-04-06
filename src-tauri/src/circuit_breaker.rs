//! Circuit breaker pattern for subagent execution.
//!
//! Prevents cascading failures by tracking per-agent failure rates
//! and temporarily disabling agents that fail repeatedly.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{info, warn};

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — calls are allowed.
    Closed,
    /// Tripped — all calls are immediately rejected.
    Open,
    /// Probing — allow one call to test recovery.
    HalfOpen,
}

/// Per-agent circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    pub state: CircuitState,
    pub failure_count: u32,
    pub success_count: u32,
    failure_threshold: u32,
    last_failure: Option<Instant>,
    last_state_change: Instant,
    recovery_timeout: Duration,
    /// True when a probe call has been dispatched in HalfOpen state.
    /// Blocks subsequent calls until the probe resolves.
    probe_in_flight: bool,
}

impl CircuitBreaker {
    fn new(failure_threshold: u32, recovery_timeout: Duration) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            failure_threshold,
            last_failure: None,
            last_state_change: Instant::now(),
            recovery_timeout,
            probe_in_flight: false,
        }
    }

    /// Check if a call is allowed. If the circuit is Open and the recovery
    /// timeout has elapsed, transition to HalfOpen and allow one probe call.
    pub fn allow_call(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(last) = self.last_failure {
                    if last.elapsed() >= self.recovery_timeout {
                        self.state = CircuitState::HalfOpen;
                        self.probe_in_flight = false;
                        self.last_state_change = Instant::now();
                        true
                    } else {
                        false
                    }
                } else {
                    // No failure recorded but somehow Open — reset to Closed.
                    self.state = CircuitState::Closed;
                    self.last_state_change = Instant::now();
                    true
                }
            }
            CircuitState::HalfOpen => {
                // Allow exactly one probe call; block all subsequent calls until
                // the probe resolves via record_success() or record_failure().
                if self.probe_in_flight {
                    false
                } else {
                    self.probe_in_flight = true;
                    true
                }
            }
        }
    }

    /// Record a successful call.
    pub fn record_success(&mut self) {
        self.success_count += 1;
        match self.state {
            CircuitState::HalfOpen => {
                // Probe succeeded — close the circuit.
                self.state = CircuitState::Closed;
                self.failure_count = 0;
                self.probe_in_flight = false;
                self.last_state_change = Instant::now();
            }
            CircuitState::Closed => {
                // Reset failure count on success.
                self.failure_count = 0;
            }
            CircuitState::Open => {
                // Shouldn't happen, but handle gracefully.
                self.state = CircuitState::Closed;
                self.failure_count = 0;
                self.last_state_change = Instant::now();
            }
        }
    }

    /// Record a failed call.
    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        self.last_failure = Some(Instant::now());

        match self.state {
            CircuitState::Closed => {
                if self.failure_count >= self.failure_threshold {
                    self.state = CircuitState::Open;
                    self.last_state_change = Instant::now();
                }
            }
            CircuitState::HalfOpen => {
                // Probe failed — trip back to Open.
                self.state = CircuitState::Open;
                self.probe_in_flight = false;
                self.last_state_change = Instant::now();
            }
            CircuitState::Open => {
                // Already open, just update timestamp.
            }
        }
    }
}

/// Configuration for the circuit breaker registry.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before tripping.
    pub failure_threshold: u32,
    /// How long to wait before probing after tripping.
    pub recovery_timeout: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(60),
        }
    }
}

/// Maximum number of circuit breakers retained in the registry.
const MAX_CIRCUIT_BREAKERS: usize = 1000;

/// Registry of circuit breakers keyed by agent ID.
pub struct CircuitBreakerRegistry {
    breakers: HashMap<String, CircuitBreaker>,
    config: CircuitBreakerConfig,
}

impl CircuitBreakerRegistry {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        info!(
            "[CIRCUIT_BREAKER] Registry initialized (threshold={}, recovery={}s)",
            config.failure_threshold,
            config.recovery_timeout.as_secs()
        );
        Self {
            breakers: HashMap::new(),
            config,
        }
    }

    /// Check if a call to the given agent is allowed.
    pub fn allow_call(&mut self, agent_id: &str) -> bool {
        let breaker = self
            .breakers
            .entry(agent_id.to_string())
            .or_insert_with(|| {
                CircuitBreaker::new(self.config.failure_threshold, self.config.recovery_timeout)
            });

        let allowed = breaker.allow_call();
        if !allowed {
            warn!(
                "[CIRCUIT_BREAKER] Call to agent '{}' blocked (state={:?}, failures={})",
                agent_id, breaker.state, breaker.failure_count
            );
        }

        // Evict Closed breakers with zero failures if registry is over capacity
        if self.breakers.len() > MAX_CIRCUIT_BREAKERS {
            self.breakers
                .retain(|_, b| !(b.state == CircuitState::Closed && b.failure_count == 0));
        }

        allowed
    }

    /// Record a successful call for the given agent.
    pub fn record_success(&mut self, agent_id: &str) {
        if let Some(breaker) = self.breakers.get_mut(agent_id) {
            let was_half_open = breaker.state == CircuitState::HalfOpen;
            breaker.record_success();
            if was_half_open {
                info!(
                    "[CIRCUIT_BREAKER] Agent '{}' recovered (HalfOpen -> Closed)",
                    agent_id
                );
            }
        }
    }

    /// Record a failed call for the given agent.
    pub fn record_failure(&mut self, agent_id: &str) {
        let breaker = self
            .breakers
            .entry(agent_id.to_string())
            .or_insert_with(|| {
                CircuitBreaker::new(self.config.failure_threshold, self.config.recovery_timeout)
            });

        let old_state = breaker.state;
        breaker.record_failure();

        if old_state != breaker.state {
            warn!(
                "[CIRCUIT_BREAKER] Agent '{}' state changed: {:?} -> {:?} (failures={})",
                agent_id, old_state, breaker.state, breaker.failure_count
            );
        }
    }

    /// Get the current state for an agent (defaults to Closed if not tracked).
    #[allow(dead_code)]
    pub fn get_state(&self, agent_id: &str) -> CircuitState {
        self.breakers
            .get(agent_id)
            .map(|b| b.state)
            .unwrap_or(CircuitState::Closed)
    }

    /// Reset the breaker for a specific agent.
    #[allow(dead_code)]
    pub fn reset(&mut self, agent_id: &str) {
        self.breakers.remove(agent_id);
        info!("[CIRCUIT_BREAKER] Reset breaker for agent '{}'", agent_id);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_millis(100),
        }
    }

    #[test]
    fn test_closed_allows_calls() {
        let mut registry = CircuitBreakerRegistry::new(test_config());
        assert!(registry.allow_call("agent-a"));
        assert!(registry.allow_call("agent-a"));
    }

    #[test]
    fn test_trips_open_after_threshold() {
        let mut registry = CircuitBreakerRegistry::new(test_config());

        registry.record_failure("agent-a");
        registry.record_failure("agent-a");
        assert!(registry.allow_call("agent-a")); // Still closed (2 < 3)

        registry.record_failure("agent-a");
        assert!(!registry.allow_call("agent-a")); // Now open (3 >= 3)
        assert_eq!(registry.get_state("agent-a"), CircuitState::Open);
    }

    #[test]
    fn test_success_resets_failure_count() {
        let mut registry = CircuitBreakerRegistry::new(test_config());

        registry.record_failure("agent-a");
        registry.record_failure("agent-a");
        registry.record_success("agent-a");

        // Failure count should be reset
        registry.record_failure("agent-a");
        registry.record_failure("agent-a");
        assert!(registry.allow_call("agent-a")); // Still closed (2 < 3)
    }

    #[test]
    fn test_recovery_after_timeout() {
        let mut registry = CircuitBreakerRegistry::new(test_config());

        // Trip the breaker
        for _ in 0..3 {
            registry.record_failure("agent-a");
        }
        assert!(!registry.allow_call("agent-a"));

        // Wait for recovery timeout
        std::thread::sleep(Duration::from_millis(150));

        // Should transition to HalfOpen and allow one call
        assert!(registry.allow_call("agent-a"));
        assert_eq!(registry.get_state("agent-a"), CircuitState::HalfOpen);
    }

    #[test]
    fn test_half_open_success_closes() {
        let mut registry = CircuitBreakerRegistry::new(test_config());

        // Trip and recover
        for _ in 0..3 {
            registry.record_failure("agent-a");
        }
        std::thread::sleep(Duration::from_millis(150));
        assert!(registry.allow_call("agent-a")); // HalfOpen

        registry.record_success("agent-a");
        assert_eq!(registry.get_state("agent-a"), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_failure_reopens() {
        let mut registry = CircuitBreakerRegistry::new(test_config());

        // Trip and recover
        for _ in 0..3 {
            registry.record_failure("agent-a");
        }
        std::thread::sleep(Duration::from_millis(150));
        assert!(registry.allow_call("agent-a")); // HalfOpen

        registry.record_failure("agent-a");
        assert_eq!(registry.get_state("agent-a"), CircuitState::Open);
    }

    #[test]
    fn test_independent_agents() {
        let mut registry = CircuitBreakerRegistry::new(test_config());

        // Trip agent-a
        for _ in 0..3 {
            registry.record_failure("agent-a");
        }
        assert!(!registry.allow_call("agent-a"));

        // agent-b should be unaffected
        assert!(registry.allow_call("agent-b"));
    }

    #[test]
    fn test_reset() {
        let mut registry = CircuitBreakerRegistry::new(test_config());

        for _ in 0..3 {
            registry.record_failure("agent-a");
        }
        assert!(!registry.allow_call("agent-a"));

        registry.reset("agent-a");
        assert!(registry.allow_call("agent-a"));
        assert_eq!(registry.get_state("agent-a"), CircuitState::Closed);
    }

    #[test]
    fn test_default_state_is_closed() {
        let registry = CircuitBreakerRegistry::new(test_config());
        assert_eq!(registry.get_state("unknown-agent"), CircuitState::Closed);
    }
}
