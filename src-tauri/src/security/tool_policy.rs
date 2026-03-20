//! Per-agent tool policy with allow/deny glob patterns.
//!
//! Each agent can have a `ToolPolicy` that restricts which tools it is
//! permitted to invoke. Patterns support a simple `*` wildcard.
//! A global deny list is checked before any per-agent policy.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Tool access policy for a single agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// Identifier of the agent this policy applies to.
    pub agent_id: String,
    /// Glob patterns for tools the agent is allowed to use.
    /// An empty list means "allow nothing" (unless no denied patterns block it
    /// and there is no explicit policy -- see `is_tool_allowed`).
    pub allowed_patterns: Vec<String>,
    /// Glob patterns for tools the agent is explicitly denied.
    pub denied_patterns: Vec<String>,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// Manages per-agent tool policies and a global deny list.
pub struct ToolPolicyManager {
    policies: HashMap<String, ToolPolicy>,
    global_denied: Vec<String>,
}

impl ToolPolicyManager {
    /// Create a new, empty policy manager.
    pub fn new() -> Self {
        info!("[TOOL_POLICY] Initialised policy manager");
        Self {
            policies: HashMap::new(),
            global_denied: Vec::new(),
        }
    }

    /// Check whether `agent_id` is permitted to invoke `tool_name`.
    ///
    /// Evaluation order:
    /// 1. Global deny list -- if any pattern matches, deny.
    /// 2. Per-agent denied patterns -- if any pattern matches, deny.
    /// 3. Per-agent allowed patterns -- if any pattern matches, allow.
    /// 4. If the agent has a policy but no allowed pattern matched, deny.
    /// 5. If no policy exists for the agent, allow by default.
    pub fn is_tool_allowed(&self, agent_id: &str, tool_name: &str) -> bool {
        // 1. Global deny
        for pattern in &self.global_denied {
            if glob_match(pattern, tool_name) {
                warn!(
                    "[TOOL_POLICY] Tool '{}' denied for agent '{}' by global deny pattern '{}'",
                    tool_name, agent_id, pattern
                );
                return false;
            }
        }

        // 2-4. Per-agent policy
        if let Some(policy) = self.policies.get(agent_id) {
            // Denied patterns
            for pattern in &policy.denied_patterns {
                if glob_match(pattern, tool_name) {
                    warn!(
                        "[TOOL_POLICY] Tool '{}' denied for agent '{}' by agent deny pattern '{}'",
                        tool_name, agent_id, pattern
                    );
                    return false;
                }
            }
            // Allowed patterns
            for pattern in &policy.allowed_patterns {
                if glob_match(pattern, tool_name) {
                    return true;
                }
            }
            // Policy exists but nothing matched -- deny.
            warn!(
                "[TOOL_POLICY] Tool '{}' denied for agent '{}' -- no allowed pattern matched",
                tool_name, agent_id
            );
            return false;
        }

        // 5. No policy -- allow.
        true
    }

    /// Register or replace a policy for a given agent.
    #[allow(dead_code)]
    pub fn add_policy(&mut self, policy: ToolPolicy) {
        info!(
            "[TOOL_POLICY] Set policy for agent '{}': {} allowed, {} denied",
            policy.agent_id,
            policy.allowed_patterns.len(),
            policy.denied_patterns.len()
        );
        self.policies.insert(policy.agent_id.clone(), policy);
    }

    /// Remove the policy for `agent_id`, reverting to the default allow-all.
    #[allow(dead_code)]
    pub fn remove_policy(&mut self, agent_id: &str) {
        info!("[TOOL_POLICY] Removed policy for agent '{}'", agent_id);
        self.policies.remove(agent_id);
    }

    /// Replace the global deny list.
    #[allow(dead_code)]
    pub fn set_global_denied(&mut self, patterns: Vec<String>) {
        info!(
            "[TOOL_POLICY] Updated global deny list ({} patterns)",
            patterns.len()
        );
        self.global_denied = patterns;
    }

    /// Return how many per-agent policies are registered.
    #[allow(dead_code)]
    pub fn policy_count(&self) -> usize {
        self.policies.len()
    }
}

// ---------------------------------------------------------------------------
// Simple glob matching
// ---------------------------------------------------------------------------

/// Match `pattern` against `value` using a simple `*` wildcard.
///
/// Supported cases:
/// - `"*"` matches everything.
/// - `"prefix*"` matches values starting with `prefix`.
/// - `"*suffix"` matches values ending with `suffix`.
/// - `"prefix*suffix"` matches values starting with `prefix` and ending with
///   `suffix`.
/// - No `*` -- exact match.
/// - Multiple `*`s: each segment between wildcards must appear in order.
fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == value;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    let mut remaining = value;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            // First segment: must be a prefix.
            if !remaining.starts_with(part) {
                return false;
            }
            remaining = &remaining[part.len()..];
        } else if i == parts.len() - 1 {
            // Last segment: must be a suffix.
            if !remaining.ends_with(part) {
                return false;
            }
            remaining = &remaining[..remaining.len() - part.len()];
        } else {
            // Middle segment: must appear somewhere.
            match remaining.find(part) {
                Some(pos) => remaining = &remaining[pos + part.len()..],
                None => return false,
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- glob_match --------------------------------------------------------

    #[test]
    fn test_glob_exact() {
        assert!(glob_match("hello", "hello"));
        assert!(!glob_match("hello", "world"));
    }

    #[test]
    fn test_glob_star_only() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn test_glob_prefix() {
        assert!(glob_match("nexibot_*", "nexibot_execute"));
        assert!(glob_match("nexibot_*", "nexibot_"));
        assert!(!glob_match("nexibot_*", "other_execute"));
    }

    #[test]
    fn test_glob_suffix() {
        assert!(glob_match("*_execute", "nexibot_execute"));
        assert!(!glob_match("*_execute", "nexibot_read"));
    }

    #[test]
    fn test_glob_prefix_and_suffix() {
        assert!(glob_match("nexi*cute", "nexibot_execute"));
        assert!(!glob_match("nexi*cute", "nexibot_read"));
    }

    #[test]
    fn test_glob_multiple_stars() {
        assert!(glob_match("a*b*c", "aXbYc"));
        assert!(glob_match("a*b*c", "abc"));
        assert!(!glob_match("a*b*c", "aXYc"));
    }

    // -- ToolPolicyManager -------------------------------------------------

    #[test]
    fn test_no_policy_allows_all() {
        let mgr = ToolPolicyManager::new();
        assert!(mgr.is_tool_allowed("agent_1", "any_tool"));
    }

    #[test]
    fn test_global_deny() {
        let mut mgr = ToolPolicyManager::new();
        mgr.set_global_denied(vec!["dangerous_*".to_string()]);
        assert!(!mgr.is_tool_allowed("agent_1", "dangerous_rm"));
        assert!(mgr.is_tool_allowed("agent_1", "safe_read"));
    }

    #[test]
    fn test_agent_allowed_patterns() {
        let mut mgr = ToolPolicyManager::new();
        mgr.add_policy(ToolPolicy {
            agent_id: "agent_1".to_string(),
            allowed_patterns: vec!["read_*".to_string(), "list_*".to_string()],
            denied_patterns: vec![],
        });
        assert!(mgr.is_tool_allowed("agent_1", "read_file"));
        assert!(mgr.is_tool_allowed("agent_1", "list_dir"));
        assert!(!mgr.is_tool_allowed("agent_1", "write_file"));
    }

    #[test]
    fn test_agent_denied_overrides_allowed() {
        let mut mgr = ToolPolicyManager::new();
        mgr.add_policy(ToolPolicy {
            agent_id: "agent_1".to_string(),
            allowed_patterns: vec!["*".to_string()],
            denied_patterns: vec!["exec_*".to_string()],
        });
        assert!(mgr.is_tool_allowed("agent_1", "read_file"));
        assert!(!mgr.is_tool_allowed("agent_1", "exec_command"));
    }

    #[test]
    fn test_global_deny_overrides_agent_allow() {
        let mut mgr = ToolPolicyManager::new();
        mgr.set_global_denied(vec!["nuclear_*".to_string()]);
        mgr.add_policy(ToolPolicy {
            agent_id: "agent_1".to_string(),
            allowed_patterns: vec!["*".to_string()],
            denied_patterns: vec![],
        });
        assert!(!mgr.is_tool_allowed("agent_1", "nuclear_launch"));
    }

    #[test]
    fn test_remove_policy_reverts_to_allow_all() {
        let mut mgr = ToolPolicyManager::new();
        mgr.add_policy(ToolPolicy {
            agent_id: "agent_1".to_string(),
            allowed_patterns: vec!["read_*".to_string()],
            denied_patterns: vec![],
        });
        assert!(!mgr.is_tool_allowed("agent_1", "write_file"));

        mgr.remove_policy("agent_1");
        assert!(mgr.is_tool_allowed("agent_1", "write_file"));
    }

    #[test]
    fn test_policy_count() {
        let mut mgr = ToolPolicyManager::new();
        assert_eq!(mgr.policy_count(), 0);
        mgr.add_policy(ToolPolicy {
            agent_id: "a".to_string(),
            allowed_patterns: vec![],
            denied_patterns: vec![],
        });
        mgr.add_policy(ToolPolicy {
            agent_id: "b".to_string(),
            allowed_patterns: vec![],
            denied_patterns: vec![],
        });
        assert_eq!(mgr.policy_count(), 2);
    }
}
