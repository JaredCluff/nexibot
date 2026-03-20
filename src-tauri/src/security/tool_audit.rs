//! Centralized Tool Execution Audit Log
//!
//! Provides a bounded, thread-safe audit log for all tool executions across
//! channels (GUI, Telegram, gateway, etc.). Entries are stored in a ring
//! buffer capped at `MAX_ENTRIES` and can be queried by recency or session.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of audit entries retained in the ring buffer.
const MAX_ENTRIES: usize = 10_000;

/// Maximum length of the input summary stored per entry.
const INPUT_SUMMARY_MAX_LEN: usize = 200;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Outcome of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolAuditResult {
    /// Tool executed successfully.
    Success,
    /// Tool execution was denied (with reason).
    Denied(String),
    /// Tool execution failed (with error message).
    Error(String),
}

impl std::fmt::Display for ToolAuditResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "Success"),
            Self::Denied(reason) => write!(f, "Denied: {}", reason),
            Self::Error(msg) => write!(f, "Error: {}", msg),
        }
    }
}

/// A single tool-execution audit entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAuditEntry {
    /// When the tool execution occurred.
    pub timestamp: DateTime<Utc>,
    /// Name of the tool that was invoked.
    pub tool_name: String,
    /// ID of the agent that requested the tool call.
    pub agent_id: String,
    /// Session in which the tool was called.
    pub session_id: String,
    /// Channel through which the request originated.
    pub channel: String,
    /// Truncated and redacted summary of the tool input (first 200 chars).
    pub input_summary: String,
    /// Outcome of the execution.
    pub result: ToolAuditResult,
    /// Wall-clock duration of the tool execution in milliseconds.
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

/// Thread-safe, bounded ring-buffer audit log for tool executions.
#[derive(Clone)]
pub struct ToolAuditLog {
    entries: Arc<RwLock<VecDeque<ToolAuditEntry>>>,
}

impl ToolAuditLog {
    /// Create a new, empty audit log.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(VecDeque::with_capacity(MAX_ENTRIES))),
        }
    }

    /// Record a tool execution in the audit log.
    ///
    /// If the buffer is at capacity the oldest entry is evicted.
    pub async fn log_tool_execution(&self, entry: ToolAuditEntry) {
        debug!(
            "[TOOL_AUDIT] {} tool={} agent={} session={} channel={} result={}",
            entry.timestamp,
            entry.tool_name,
            entry.agent_id,
            entry.session_id,
            entry.channel,
            entry.result
        );

        let mut buf = self.entries.write().await;
        if buf.len() >= MAX_ENTRIES {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Retrieve the most recent `count` entries (newest last).
    pub async fn get_recent_entries(&self, count: usize) -> Vec<ToolAuditEntry> {
        let buf = self.entries.read().await;
        let start = buf.len().saturating_sub(count);
        buf.iter().skip(start).cloned().collect()
    }

    /// Retrieve all entries for a given session ID.
    pub async fn get_entries_for_session(&self, session_id: &str) -> Vec<ToolAuditEntry> {
        let buf = self.entries.read().await;
        buf.iter()
            .filter(|e| e.session_id == session_id)
            .cloned()
            .collect()
    }

    /// Current number of entries in the log.
    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    /// Whether the log is empty.
    pub async fn is_empty(&self) -> bool {
        self.entries.read().await.is_empty()
    }
}

impl Default for ToolAuditLog {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Produce a redacted input summary: truncate to `INPUT_SUMMARY_MAX_LEN`
/// characters and mask anything that looks like a secret (API keys, tokens).
pub fn summarize_input(raw: &str) -> String {
    let truncated: String = raw.chars().take(INPUT_SUMMARY_MAX_LEN).collect();
    redact_secrets(&truncated)
}

/// Naive redaction of common secret patterns in a string.
fn redact_secrets(s: &str) -> String {
    let patterns = [
        "sk-ant-",
        "sk-",
        "xoxb-",
        "xoxp-",
        "ghp_",
        "gho_",
        "Bearer ",
        "token=",
        "api_key=",
        "password=",
    ];

    let mut result = s.to_string();
    for pat in &patterns {
        if let Some(pos) = result.find(pat) {
            // Skip past the pattern prefix, then find the end of the secret value
            let value_start = pos + pat.len();
            let value_rest = &result[value_start..];
            let value_end = value_rest
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',')
                .unwrap_or(value_rest.len());
            let redacted = format!("{}[REDACTED]", pat);
            result = format!(
                "{}{}{}",
                &result[..pos],
                redacted,
                &result[value_start + value_end..]
            );
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(tool: &str, session: &str, channel: &str) -> ToolAuditEntry {
        ToolAuditEntry {
            timestamp: Utc::now(),
            tool_name: tool.to_string(),
            agent_id: "agent-1".to_string(),
            session_id: session.to_string(),
            channel: channel.to_string(),
            input_summary: "test input".to_string(),
            result: ToolAuditResult::Success,
            duration_ms: 42,
        }
    }

    #[tokio::test]
    async fn test_log_and_retrieve() {
        let log = ToolAuditLog::new();
        assert!(log.is_empty().await);

        log.log_tool_execution(make_entry("fetch", "s1", "gui"))
            .await;
        log.log_tool_execution(make_entry("execute", "s1", "gui"))
            .await;
        log.log_tool_execution(make_entry("search", "s2", "telegram"))
            .await;

        assert_eq!(log.len().await, 3);

        let recent = log.get_recent_entries(2).await;
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].tool_name, "execute");
        assert_eq!(recent[1].tool_name, "search");
    }

    #[tokio::test]
    async fn test_get_entries_for_session() {
        let log = ToolAuditLog::new();

        log.log_tool_execution(make_entry("fetch", "s1", "gui"))
            .await;
        log.log_tool_execution(make_entry("execute", "s2", "telegram"))
            .await;
        log.log_tool_execution(make_entry("search", "s1", "gui"))
            .await;

        let s1_entries = log.get_entries_for_session("s1").await;
        assert_eq!(s1_entries.len(), 2);
        assert!(s1_entries.iter().all(|e| e.session_id == "s1"));

        let s2_entries = log.get_entries_for_session("s2").await;
        assert_eq!(s2_entries.len(), 1);
        assert_eq!(s2_entries[0].tool_name, "execute");

        let empty = log.get_entries_for_session("nonexistent").await;
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_ring_buffer_eviction() {
        let log = ToolAuditLog::new();

        // Fill beyond capacity
        for i in 0..MAX_ENTRIES + 50 {
            log.log_tool_execution(ToolAuditEntry {
                timestamp: Utc::now(),
                tool_name: format!("tool-{}", i),
                agent_id: "agent".to_string(),
                session_id: "s1".to_string(),
                channel: "gui".to_string(),
                input_summary: "x".to_string(),
                result: ToolAuditResult::Success,
                duration_ms: 1,
            })
            .await;
        }

        assert_eq!(log.len().await, MAX_ENTRIES);

        // Oldest entries should have been evicted; first remaining is tool-50
        let all = log.get_recent_entries(MAX_ENTRIES).await;
        assert_eq!(all.first().unwrap().tool_name, "tool-50");
        assert_eq!(
            all.last().unwrap().tool_name,
            format!("tool-{}", MAX_ENTRIES + 49)
        );
    }

    #[tokio::test]
    async fn test_get_recent_more_than_available() {
        let log = ToolAuditLog::new();
        log.log_tool_execution(make_entry("fetch", "s1", "gui"))
            .await;

        let recent = log.get_recent_entries(100).await;
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn test_summarize_input_truncation() {
        let long_input = "a".repeat(500);
        let summary = summarize_input(&long_input);
        assert_eq!(summary.len(), INPUT_SUMMARY_MAX_LEN);
    }

    #[test]
    fn test_summarize_input_redaction() {
        let raw = "fetch url with Bearer my-secret-token and more";
        let summary = summarize_input(raw);
        assert!(summary.contains("[REDACTED]"));
        assert!(!summary.contains("my-secret-token"));
    }

    #[test]
    fn test_redact_secrets_api_key() {
        let input = "key=sk-ant-abc123xyz here";
        let redacted = redact_secrets(input);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("abc123xyz"));
    }

    #[test]
    fn test_redact_secrets_no_match() {
        let input = "nothing sensitive here";
        let redacted = redact_secrets(input);
        assert_eq!(redacted, input);
    }

    #[test]
    fn test_tool_audit_result_display() {
        assert_eq!(ToolAuditResult::Success.to_string(), "Success");
        assert_eq!(
            ToolAuditResult::Denied("policy".to_string()).to_string(),
            "Denied: policy"
        );
        assert_eq!(
            ToolAuditResult::Error("timeout".to_string()).to_string(),
            "Error: timeout"
        );
    }

    #[test]
    fn test_entry_serialization() {
        let entry = make_entry("fetch", "s1", "gui");
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: ToolAuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tool_name, "fetch");
        assert_eq!(deserialized.session_id, "s1");
        assert_eq!(deserialized.channel, "gui");
    }

    #[test]
    fn test_result_serialization() {
        let results = vec![
            ToolAuditResult::Success,
            ToolAuditResult::Denied("blocked".to_string()),
            ToolAuditResult::Error("fail".to_string()),
        ];
        for result in results {
            let json = serde_json::to_string(&result).unwrap();
            let _: ToolAuditResult = serde_json::from_str(&json).unwrap();
        }
    }

    #[tokio::test]
    async fn test_denied_and_error_entries() {
        let log = ToolAuditLog::new();

        log.log_tool_execution(ToolAuditEntry {
            timestamp: Utc::now(),
            tool_name: "execute".to_string(),
            agent_id: "agent-1".to_string(),
            session_id: "s1".to_string(),
            channel: "telegram".to_string(),
            input_summary: "rm -rf /".to_string(),
            result: ToolAuditResult::Denied("destructive command blocked".to_string()),
            duration_ms: 0,
        })
        .await;

        log.log_tool_execution(ToolAuditEntry {
            timestamp: Utc::now(),
            tool_name: "fetch".to_string(),
            agent_id: "agent-1".to_string(),
            session_id: "s1".to_string(),
            channel: "gui".to_string(),
            input_summary: "http://169.254.169.254".to_string(),
            result: ToolAuditResult::Error("SSRF blocked".to_string()),
            duration_ms: 5,
        })
        .await;

        let entries = log.get_entries_for_session("s1").await;
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0].result, ToolAuditResult::Denied(_)));
        assert!(matches!(entries[1].result, ToolAuditResult::Error(_)));
    }

    #[tokio::test]
    async fn test_clone_shares_state() {
        let log1 = ToolAuditLog::new();
        let log2 = log1.clone();

        log1.log_tool_execution(make_entry("fetch", "s1", "gui"))
            .await;
        assert_eq!(log2.len().await, 1);

        log2.log_tool_execution(make_entry("search", "s2", "telegram"))
            .await;
        assert_eq!(log1.len().await, 2);
    }
}
