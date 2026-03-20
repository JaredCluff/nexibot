//! Agent control and emergency killswitch system.
//!
//! Provides real-time control over agent operations:
//! - Emergency stop: Immediately halt all processing
//! - Pause mode: Queue messages without processing
//! - Resume: Return to normal operation
//! - Audit logging of all state changes

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};

/// Agent control state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentState {
    /// Normal operation - process all messages
    Running,
    /// Paused - queue messages, don't process
    Paused,
    /// Emergency stop - reject all messages
    Stopped,
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentState::Running => write!(f, "running"),
            AgentState::Paused => write!(f, "paused"),
            AgentState::Stopped => write!(f, "stopped"),
        }
    }
}

/// Agent control manager
pub struct AgentControl {
    state: Arc<std::sync::Mutex<AgentState>>,
    stopped_at: Arc<std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
    paused_at: Arc<std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
}

impl AgentControl {
    /// Create a new agent control manager
    pub fn new() -> Self {
        Self {
            state: Arc::new(std::sync::Mutex::new(AgentState::Running)),
            stopped_at: Arc::new(std::sync::Mutex::new(None)),
            paused_at: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Get current agent state
    pub fn get_state(&self) -> AgentState {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Emergency stop the agent (instant, no questions)
    pub fn emergency_stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let old_state = *state;
        *state = AgentState::Stopped;

        let mut stopped_at = self.stopped_at.lock().unwrap_or_else(|e| e.into_inner());
        *stopped_at = Some(chrono::Utc::now());

        // Clear pause time since we're stopping
        let mut paused_at = self.paused_at.lock().unwrap_or_else(|e| e.into_inner());
        *paused_at = None;

        warn!(
            "[KILLSWITCH] Emergency stop activated: {} → {}",
            old_state,
            AgentState::Stopped
        );
    }

    /// Pause the agent (queue messages, don't process).
    ///
    /// No-ops if an emergency stop is active — the stopped state takes precedence
    /// and cannot be overridden by a normal pause call.  Call `resume()` first to
    /// clear the emergency stop before pausing.
    pub fn pause(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());

        if *state == AgentState::Stopped {
            warn!(
                "[KILLSWITCH] pause() ignored — emergency stop is active. \
                 Call resume() first to clear the emergency stop."
            );
            return;
        }

        let old_state = *state;
        *state = AgentState::Paused;
        drop(state);

        let mut paused_at = self.paused_at.lock().unwrap_or_else(|e| e.into_inner());
        *paused_at = Some(chrono::Utc::now());

        info!(
            "[KILLSWITCH] Agent paused: {} → {}",
            old_state,
            AgentState::Paused
        );
    }

    /// Resume normal operation
    pub fn resume(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let old_state = *state;
        *state = AgentState::Running;

        let mut stopped_at = self.stopped_at.lock().unwrap_or_else(|e| e.into_inner());
        *stopped_at = None;

        let mut paused_at = self.paused_at.lock().unwrap_or_else(|e| e.into_inner());
        *paused_at = None;

        info!(
            "[KILLSWITCH] Agent resumed: {} → {}",
            old_state,
            AgentState::Running
        );
    }

    /// Check if agent can process messages
    #[allow(dead_code)]
    pub fn can_process(&self) -> bool {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) == AgentState::Running
    }

    /// Check if agent is stopped
    #[allow(dead_code)]
    pub fn is_stopped(&self) -> bool {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) == AgentState::Stopped
    }

    /// Check if agent is paused
    #[allow(dead_code)]
    pub fn is_paused(&self) -> bool {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) == AgentState::Paused
    }

    /// Get duration stopped (if stopped)
    #[allow(dead_code)]
    pub fn stopped_duration(&self) -> Option<chrono::Duration> {
        let stopped_at = self.stopped_at.lock().unwrap_or_else(|e| e.into_inner());
        stopped_at.map(|t| chrono::Utc::now().signed_duration_since(t))
    }

    /// Get duration paused (if paused)
    #[allow(dead_code)]
    pub fn paused_duration(&self) -> Option<chrono::Duration> {
        let paused_at = self.paused_at.lock().unwrap_or_else(|e| e.into_inner());
        paused_at.map(|t| chrono::Utc::now().signed_duration_since(t))
    }

    /// Get status info for UI
    pub fn get_status(&self) -> AgentStatusInfo {
        let state = self.get_state();
        let stopped_at = *self.stopped_at.lock().unwrap_or_else(|e| e.into_inner());
        let paused_at = *self.paused_at.lock().unwrap_or_else(|e| e.into_inner());

        AgentStatusInfo {
            state: state.to_string(),
            is_stopped: state == AgentState::Stopped,
            is_paused: state == AgentState::Paused,
            is_running: state == AgentState::Running,
            stopped_at: stopped_at.map(|t| t.to_rfc3339()),
            paused_at: paused_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Status info for UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusInfo {
    pub state: String,
    pub is_stopped: bool,
    pub is_paused: bool,
    pub is_running: bool,
    pub stopped_at: Option<String>,
    pub paused_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_control_lifecycle() {
        let control = AgentControl::new();

        // Start in running state
        assert_eq!(control.get_state(), AgentState::Running);
        assert!(control.can_process());

        // Emergency stop
        control.emergency_stop();
        assert_eq!(control.get_state(), AgentState::Stopped);
        assert!(!control.can_process());
        assert!(control.is_stopped());

        // pause() while stopped must be a no-op (does not override the emergency stop)
        control.pause();
        assert_eq!(
            control.get_state(),
            AgentState::Stopped,
            "pause() must not override emergency stop"
        );

        // Resume clears the emergency stop
        control.resume();
        assert_eq!(control.get_state(), AgentState::Running);
        assert!(control.can_process());

        // Pause from running state works normally
        control.pause();
        assert_eq!(control.get_state(), AgentState::Paused);
        assert!(!control.can_process());
        assert!(control.is_paused());

        // Resume again
        control.resume();
        assert_eq!(control.get_state(), AgentState::Running);
    }

    #[test]
    fn test_emergency_stop_clears_pause() {
        let control = AgentControl::new();

        control.pause();
        assert!(control.is_paused());

        control.emergency_stop();
        assert!(control.is_stopped());
        assert!(!control.is_paused());
    }

    #[test]
    fn test_status_info() {
        let control = AgentControl::new();

        let status = control.get_status();
        assert_eq!(status.state, "running");
        assert!(status.is_running);
        assert!(!status.is_stopped);
        assert!(!status.is_paused);

        control.emergency_stop();
        let status = control.get_status();
        assert_eq!(status.state, "stopped");
        assert!(status.is_stopped);
        assert!(status.stopped_at.is_some());
    }
}
