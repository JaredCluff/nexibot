//! Shared workspace for inter-agent data sharing within an orchestration run.
//!
//! Provides a key-value store scoped per orchestration, allowing subagents
//! to share intermediate results with each other and the parent orchestrator.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::info;

/// A single entry in the shared workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub value: serde_json::Value,
    pub written_by: String,
    #[serde(skip)]
    pub written_at: Option<Instant>,
    pub ttl_seconds: Option<u64>,
}

/// Shared workspace for inter-agent data sharing.
pub struct SharedWorkspace {
    /// Data keyed by "orchestration_id:key".
    data: HashMap<String, WorkspaceEntry>,
    max_entries: usize,
    max_value_bytes: usize,
}

impl SharedWorkspace {
    pub fn new() -> Self {
        Self::with_limits(1000, 64 * 1024)
    }

    pub fn with_limits(max_entries: usize, max_value_bytes: usize) -> Self {
        info!(
            "[WORKSPACE] Shared workspace initialized (max_entries={}, max_value_bytes={})",
            max_entries, max_value_bytes
        );
        Self {
            data: HashMap::new(),
            max_entries,
            max_value_bytes,
        }
    }

    /// Scoped key: "orchestration_id:key".
    fn scoped_key(orchestration_id: &str, key: &str) -> String {
        format!("{}:{}", orchestration_id, key)
    }

    /// Write or overwrite an entry.
    pub fn put(
        &mut self,
        orchestration_id: &str,
        key: &str,
        value: serde_json::Value,
        agent_name: &str,
        ttl: Option<Duration>,
    ) -> Result<(), String> {
        // Validate key
        if key.is_empty() || key.len() > 256 {
            return Err("Key must be 1-256 characters".to_string());
        }

        // Validate value size
        let value_bytes = serde_json::to_string(&value)
            .map_err(|e| format!("Failed to serialize value: {}", e))?;
        if value_bytes.len() > self.max_value_bytes {
            return Err(format!(
                "Value exceeds max size: {} > {} bytes",
                value_bytes.len(),
                self.max_value_bytes
            ));
        }

        let scoped = Self::scoped_key(orchestration_id, key);

        // For new entries: lazily evict expired entries when near capacity so
        // that a workspace full of expired-but-not-yet-cleaned entries doesn't
        // permanently block writes.
        if !self.data.contains_key(&scoped) && self.data.len() >= self.max_entries {
            self.clear_expired();
            if self.data.len() >= self.max_entries {
                return Err(format!(
                    "Workspace full: {} entries (max {})",
                    self.data.len(),
                    self.max_entries
                ));
            }
        }

        let entry = WorkspaceEntry {
            value,
            written_by: agent_name.to_string(),
            written_at: Some(Instant::now()),
            ttl_seconds: ttl.map(|d| d.as_secs()),
        };

        self.data.insert(scoped, entry);
        Ok(())
    }

    /// Read an entry. Returns None if not found or expired.
    pub fn get(&self, orchestration_id: &str, key: &str) -> Option<&WorkspaceEntry> {
        let scoped = Self::scoped_key(orchestration_id, key);
        self.data.get(&scoped).and_then(|entry| {
            // Check TTL
            if let (Some(written_at), Some(ttl_secs)) = (entry.written_at, entry.ttl_seconds) {
                if written_at.elapsed() > Duration::from_secs(ttl_secs) {
                    return None;
                }
            }
            Some(entry)
        })
    }

    /// List all keys for an orchestration (optionally filtered by prefix).
    pub fn list_keys(&self, orchestration_id: &str, prefix: Option<&str>) -> Vec<String> {
        let scope_prefix = format!("{}:", orchestration_id);
        self.data
            .keys()
            .filter_map(|k| {
                k.strip_prefix(&scope_prefix).and_then(|rest| {
                    if let Some(p) = prefix {
                        if rest.starts_with(p) {
                            Some(rest.to_string())
                        } else {
                            None
                        }
                    } else {
                        Some(rest.to_string())
                    }
                })
            })
            .collect()
    }

    /// Delete a single entry.
    #[allow(dead_code)]
    pub fn delete(&mut self, orchestration_id: &str, key: &str) -> bool {
        let scoped = Self::scoped_key(orchestration_id, key);
        self.data.remove(&scoped).is_some()
    }

    /// Clear all entries for a given orchestration.
    pub fn clear_orchestration(&mut self, orchestration_id: &str) {
        let prefix = format!("{}:", orchestration_id);
        let before = self.data.len();
        self.data.retain(|k, _| !k.starts_with(&prefix));
        let removed = before - self.data.len();
        if removed > 0 {
            info!(
                "[WORKSPACE] Cleared {} entries for orchestration '{}'",
                removed, orchestration_id
            );
        }
    }

    /// Remove expired entries across all orchestrations.
    pub fn clear_expired(&mut self) {
        let before = self.data.len();
        self.data.retain(|_, entry| {
            if let (Some(written_at), Some(ttl_secs)) = (entry.written_at, entry.ttl_seconds) {
                written_at.elapsed() <= Duration::from_secs(ttl_secs)
            } else {
                true // No TTL means no expiration
            }
        });
        let removed = before - self.data.len();
        if removed > 0 {
            info!("[WORKSPACE] Cleared {} expired entries", removed);
        }
    }

    /// Get the total number of entries.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Check if the workspace is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// Tool definition for `nexibot_workspace_read`.
pub fn nexibot_workspace_read_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "nexibot_workspace_read",
        "description": "Read a value from the shared workspace. Use this to access data written by other agents during orchestration.",
        "input_schema": {
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "The key to read"
                },
                "list_keys": {
                    "type": "boolean",
                    "description": "If true, list all available keys instead of reading a value"
                },
                "prefix": {
                    "type": "string",
                    "description": "Optional prefix filter when listing keys"
                }
            },
            "required": ["key"]
        }
    })
}

/// Tool definition for `nexibot_workspace_write`.
pub fn nexibot_workspace_write_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "nexibot_workspace_write",
        "description": "Write a value to the shared workspace. Use this to share data with other agents during orchestration.",
        "input_schema": {
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "The key to write"
                },
                "value": {
                    "description": "The value to store (any JSON value)"
                },
                "ttl_seconds": {
                    "type": "integer",
                    "description": "Optional time-to-live in seconds"
                }
            },
            "required": ["key", "value"]
        }
    })
}

/// Execute the workspace read tool.
pub fn execute_workspace_read(
    workspace: &SharedWorkspace,
    orchestration_id: &str,
    input: &serde_json::Value,
) -> String {
    let list_keys = input
        .get("list_keys")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if list_keys {
        let prefix = input.get("prefix").and_then(|v| v.as_str());
        let keys = workspace.list_keys(orchestration_id, prefix);
        serde_json::json!({ "keys": keys }).to_string()
    } else {
        let key = match input.get("key").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => return r#"{"error": "Missing required field: key"}"#.to_string(),
        };
        match workspace.get(orchestration_id, key) {
            Some(entry) => serde_json::json!({
                "key": key,
                "value": entry.value,
                "written_by": entry.written_by,
            })
            .to_string(),
            None => serde_json::json!({
                "key": key,
                "value": null,
                "error": "Key not found"
            })
            .to_string(),
        }
    }
}

/// Execute the workspace write tool.
pub fn execute_workspace_write(
    workspace: &mut SharedWorkspace,
    orchestration_id: &str,
    agent_name: &str,
    input: &serde_json::Value,
) -> String {
    let key = match input.get("key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => return r#"{"error": "Missing required field: key"}"#.to_string(),
    };
    let value = match input.get("value") {
        Some(v) => v.clone(),
        None => return r#"{"error": "Missing required field: value"}"#.to_string(),
    };
    let ttl = input
        .get("ttl_seconds")
        .and_then(|v| v.as_u64())
        .map(Duration::from_secs);

    match workspace.put(orchestration_id, key, value, agent_name, ttl) {
        Ok(()) => serde_json::json!({ "success": true, "key": key }).to_string(),
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_and_get() {
        let mut ws = SharedWorkspace::new();
        ws.put(
            "orch-1",
            "result",
            serde_json::json!("hello"),
            "agent-a",
            None,
        )
        .unwrap();

        let entry = ws.get("orch-1", "result").unwrap();
        assert_eq!(entry.value, serde_json::json!("hello"));
        assert_eq!(entry.written_by, "agent-a");
    }

    #[test]
    fn test_scoping() {
        let mut ws = SharedWorkspace::new();
        ws.put("orch-1", "key", serde_json::json!(1), "a", None)
            .unwrap();
        ws.put("orch-2", "key", serde_json::json!(2), "b", None)
            .unwrap();

        assert_eq!(ws.get("orch-1", "key").unwrap().value, serde_json::json!(1));
        assert_eq!(ws.get("orch-2", "key").unwrap().value, serde_json::json!(2));
    }

    #[test]
    fn test_overwrite() {
        let mut ws = SharedWorkspace::new();
        ws.put("orch-1", "key", serde_json::json!(1), "a", None)
            .unwrap();
        ws.put("orch-1", "key", serde_json::json!(2), "b", None)
            .unwrap();

        let entry = ws.get("orch-1", "key").unwrap();
        assert_eq!(entry.value, serde_json::json!(2));
        assert_eq!(entry.written_by, "b");
    }

    #[test]
    fn test_list_keys() {
        let mut ws = SharedWorkspace::new();
        ws.put("orch-1", "result/a", serde_json::json!(1), "a", None)
            .unwrap();
        ws.put("orch-1", "result/b", serde_json::json!(2), "b", None)
            .unwrap();
        ws.put("orch-1", "config", serde_json::json!(3), "c", None)
            .unwrap();

        let all = ws.list_keys("orch-1", None);
        assert_eq!(all.len(), 3);

        let results = ws.list_keys("orch-1", Some("result/"));
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_delete() {
        let mut ws = SharedWorkspace::new();
        ws.put("orch-1", "key", serde_json::json!(1), "a", None)
            .unwrap();
        assert!(ws.delete("orch-1", "key"));
        assert!(ws.get("orch-1", "key").is_none());
    }

    #[test]
    fn test_clear_orchestration() {
        let mut ws = SharedWorkspace::new();
        ws.put("orch-1", "a", serde_json::json!(1), "a", None)
            .unwrap();
        ws.put("orch-1", "b", serde_json::json!(2), "a", None)
            .unwrap();
        ws.put("orch-2", "c", serde_json::json!(3), "a", None)
            .unwrap();

        ws.clear_orchestration("orch-1");
        assert_eq!(ws.len(), 1);
        assert!(ws.get("orch-2", "c").is_some());
    }

    #[test]
    fn test_ttl_expiration() {
        let mut ws = SharedWorkspace::new();
        // Use 1-second TTL to avoid flakiness from thread scheduling
        ws.put(
            "orch-1",
            "temp",
            serde_json::json!("ephemeral"),
            "a",
            Some(Duration::from_secs(1)),
        )
        .unwrap();

        assert!(ws.get("orch-1", "temp").is_some());
        std::thread::sleep(Duration::from_millis(1100));
        assert!(ws.get("orch-1", "temp").is_none());
    }

    #[test]
    fn test_clear_expired() {
        let mut ws = SharedWorkspace::new();
        ws.put(
            "orch-1",
            "temp",
            serde_json::json!(1),
            "a",
            Some(Duration::from_secs(1)),
        )
        .unwrap();
        ws.put("orch-1", "perm", serde_json::json!(2), "a", None)
            .unwrap();

        std::thread::sleep(Duration::from_millis(1100));
        ws.clear_expired();

        assert_eq!(ws.len(), 1);
        assert!(ws.get("orch-1", "perm").is_some());
    }

    #[test]
    fn test_max_entries() {
        let mut ws = SharedWorkspace::with_limits(2, 64 * 1024);
        ws.put("o", "a", serde_json::json!(1), "a", None).unwrap();
        ws.put("o", "b", serde_json::json!(2), "a", None).unwrap();

        let result = ws.put("o", "c", serde_json::json!(3), "a", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("full"));
    }

    #[test]
    fn test_max_value_bytes() {
        let mut ws = SharedWorkspace::with_limits(100, 10); // 10 byte max
        let result = ws.put(
            "o",
            "k",
            serde_json::json!("a very long value here"),
            "a",
            None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds max size"));
    }

    #[test]
    fn test_tool_definitions() {
        let read = nexibot_workspace_read_tool_definition();
        assert_eq!(read["name"], "nexibot_workspace_read");

        let write = nexibot_workspace_write_tool_definition();
        assert_eq!(write["name"], "nexibot_workspace_write");
    }

    #[test]
    fn test_execute_read_write() {
        let mut ws = SharedWorkspace::new();

        let write_result = execute_workspace_write(
            &mut ws,
            "orch-1",
            "test-agent",
            &serde_json::json!({ "key": "data", "value": {"score": 42} }),
        );
        assert!(write_result.contains("success"));

        let read_result =
            execute_workspace_read(&ws, "orch-1", &serde_json::json!({ "key": "data" }));
        assert!(read_result.contains("42"));
    }

    #[test]
    fn test_execute_list_keys() {
        let mut ws = SharedWorkspace::new();
        ws.put("orch-1", "a", serde_json::json!(1), "a", None)
            .unwrap();

        let result = execute_workspace_read(
            &ws,
            "orch-1",
            &serde_json::json!({ "key": "", "list_keys": true }),
        );
        assert!(result.contains("keys"));
    }
}
