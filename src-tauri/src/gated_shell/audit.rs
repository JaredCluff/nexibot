//! Audit log and debug frame types for NexiGate.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::filter::FilterEvent;
use super::policy::PolicyAction;

/// Maximum audit entries kept in memory per session before oldest are dropped.
const DEFAULT_MAX_ENTRIES: usize = 10_000;

/// Maximum raw_output bytes kept when debug_mode is false.
const TRUNCATE_RAW_OUTPUT_BYTES: usize = 200;

/// A single recorded shell command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique entry ID.
    pub id: String,
    /// Wall-clock time in milliseconds since UNIX epoch.
    pub timestamp_ms: u64,
    pub session_id: String,
    pub agent_id: String,
    /// Raw command sent by the agent (may contain proxy tokens).
    pub raw_command: String,
    /// Command after inbound filter (what PTY actually received).
    pub filtered_command: String,
    /// Raw PTY output before outbound filter. Truncated to 200 chars when debug_mode=false.
    pub raw_output: String,
    /// Filtered output returned to the agent.
    pub filtered_output: String,
    /// Filter events that occurred during this execution.
    pub filter_events: Vec<FilterEvent>,
    /// Process exit code (None if timed out or session closed).
    pub exit_code: Option<i32>,
    /// Wall-clock duration of the command execution.
    pub duration_ms: u64,
    /// Policy decision for this command.
    pub policy_action: PolicyAction,
    /// Total bytes substituted in inbound direction.
    pub bytes_filtered_inbound: usize,
    /// Total bytes substituted in outbound direction.
    pub bytes_filtered_outbound: usize,
}

/// Parameters for building an AuditEntry.
pub struct AuditEntryBuilder {
    pub session_id: String,
    pub agent_id: String,
    pub raw_command: String,
    pub filtered_command: String,
    pub raw_output: String,
    pub filtered_output: String,
    pub filter_events: Vec<FilterEvent>,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub policy_action: PolicyAction,
    pub debug_mode: bool,
}

impl AuditEntryBuilder {
    pub fn build(self) -> AuditEntry {
        let bytes_inbound: usize = self
            .filter_events
            .iter()
            .filter(|e| e.direction == super::filter::FilterDirection::Inbound)
            .map(|e| e.length)
            .sum();
        let bytes_outbound: usize = self
            .filter_events
            .iter()
            .filter(|e| e.direction == super::filter::FilterDirection::Outbound)
            .map(|e| e.length)
            .sum();

        let raw_output = if self.debug_mode {
            self.raw_output.clone()
        } else {
            truncate_str(&self.raw_output, TRUNCATE_RAW_OUTPUT_BYTES)
        };

        AuditEntry {
            id: Uuid::new_v4().to_string(),
            timestamp_ms: now_ms(),
            session_id: self.session_id,
            agent_id: self.agent_id,
            raw_command: self.raw_command,
            filtered_command: self.filtered_command,
            raw_output,
            filtered_output: self.filtered_output,
            filter_events: self.filter_events,
            exit_code: self.exit_code,
            duration_ms: self.duration_ms,
            policy_action: self.policy_action,
            bytes_filtered_inbound: bytes_inbound,
            bytes_filtered_outbound: bytes_outbound,
        }
    }
}

/// In-memory audit log, bounded at `max_entries`.
///
/// Oldest entries are evicted when the cap is reached.
pub struct AuditLog {
    entries: VecDeque<AuditEntry>,
    max_entries: usize,
}

impl AuditLog {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_entries.min(1024)),
            max_entries,
        }
    }

    pub fn push(&mut self, entry: AuditEntry) {
        if self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// Return the last `limit` entries (most recent first).
    pub fn recent(&self, limit: usize) -> Vec<AuditEntry> {
        self.entries.iter().rev().take(limit).cloned().collect()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn truncate_str(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    // Truncate at a char boundary
    let mut end = max_bytes;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}…[truncated]", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gated_shell::filter::FilterDirection;

    fn make_entry(debug: bool) -> AuditEntry {
        AuditEntryBuilder {
            session_id: "sess-1".to_string(),
            agent_id: "agent-1".to_string(),
            raw_command: "echo REAL_SECRET".to_string(),
            filtered_command: "echo REAL_SECRET".to_string(),
            raw_output: "REAL_SECRET\n".to_string(),
            filtered_output: "PROXY_TOKEN\n".to_string(),
            filter_events: vec![FilterEvent {
                direction: FilterDirection::Outbound,
                proxy_key: "PROXY_TOKEN".to_string(),
                key_format: "generic".to_string(),
                position: 0,
                length: 11,
                timestamp_ms: now_ms(),
            }],
            exit_code: Some(0),
            duration_ms: 42,
            policy_action: PolicyAction::Allow,
            debug_mode: debug,
        }
        .build()
    }

    #[test]
    fn test_debug_mode_raw_output_kept() {
        let entry = make_entry(true);
        assert_eq!(entry.raw_output, "REAL_SECRET\n");
    }

    #[test]
    fn test_non_debug_mode_raw_output_truncated() {
        // raw_output is 12 chars, under 200 so not truncated, but if we use a long string:
        let mut builder = AuditEntryBuilder {
            session_id: "s".to_string(),
            agent_id: "a".to_string(),
            raw_command: "cmd".to_string(),
            filtered_command: "cmd".to_string(),
            raw_output: "x".repeat(300),
            filtered_output: "y".repeat(300),
            filter_events: vec![],
            exit_code: Some(0),
            duration_ms: 1,
            policy_action: PolicyAction::Allow,
            debug_mode: false,
        };
        let entry = builder.build();
        assert!(
            entry.raw_output.len() <= 220,
            "should be truncated: {}",
            entry.raw_output.len()
        );
    }

    #[test]
    fn test_audit_log_evicts_oldest() {
        let mut log = AuditLog::new(3);
        for i in 0..5 {
            log.push(
                AuditEntryBuilder {
                    session_id: format!("s{}", i),
                    agent_id: "a".to_string(),
                    raw_command: format!("cmd{}", i),
                    filtered_command: format!("cmd{}", i),
                    raw_output: "".to_string(),
                    filtered_output: "".to_string(),
                    filter_events: vec![],
                    exit_code: Some(0),
                    duration_ms: 1,
                    policy_action: PolicyAction::Allow,
                    debug_mode: false,
                }
                .build(),
            );
        }
        assert_eq!(log.len(), 3);
        let recent = log.recent(3);
        // Most recent is cmd4
        assert_eq!(recent[0].raw_command, "cmd4");
        assert_eq!(recent[2].raw_command, "cmd2");
    }
}
