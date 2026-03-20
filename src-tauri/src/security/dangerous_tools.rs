//! Dangerous Tool Registry
//!
//! Static registry of tools classified by risk level. Used by guardrails,
//! gateway mode, and the audit system to enforce tool-level access policies.
//!
//! `READ_ONLY_TOOLS` is the authoritative source for `Smart` approval mode —
//! tools listed here run silently; everything else prompts the user.
#![allow(dead_code)]

/// Tools that are read-only: they observe the world but do not change it.
/// In `Smart` approval mode these run without user confirmation.
/// Keep this list explicit and conservative — when in doubt, leave a tool out.
pub const READ_ONLY_TOOLS: &[&str] = &[
    // NexiBot built-in search and retrieval
    "nexibot_search",
    "nexibot_k2k_search",
    "nexibot_memory_search",
    "nexibot_fetch",
    // Knowledge base reads
    "kb_search",
    "kb_read",
    "kb_lookup",
    "kb_entity_lookup",
    // Calendar and contacts (read)
    "get_calendar_events",
    "list_calendar_events",
    "get_calendar_event",
    // Email (read-only)
    "get_emails",
    "list_emails",
    "get_email",
    // Contacts (read-only)
    "get_contacts",
    "list_contacts",
    // MCP generic read ops
    "mcp_read",
    "mcp_list",
    "mcp_get",
];

/// Tools that can write, delete, or execute arbitrary operations.
pub const DANGEROUS_TOOLS: &[&str] = &["nexibot_execute", "nexibot_filesystem"];

/// Default tools denied in multi-user gateway mode.
/// Runtime enforcement uses per-channel `ChannelToolPolicy` in config.yaml.
pub const GATEWAY_DENIED_TOOLS: &[&str] = &["nexibot_execute", "nexibot_filesystem"];

/// Tools that modify system behavior or identity (elevated privilege).
pub const ELEVATED_TOOLS: &[&str] = &["nexibot_settings", "nexibot_soul"];

/// Check if a tool is read-only (safe for silent execution in Smart mode).
pub fn is_read_only_tool(name: &str) -> bool {
    READ_ONLY_TOOLS.contains(&name)
}

/// Check if a tool name is classified as dangerous.
pub fn is_dangerous_tool(name: &str) -> bool {
    DANGEROUS_TOOLS.contains(&name)
}

/// Check if a tool is denied in gateway (multi-user) mode.
pub fn is_gateway_denied(name: &str) -> bool {
    GATEWAY_DENIED_TOOLS.contains(&name)
}

/// Check if a tool requires elevated privileges.
pub fn is_elevated_tool(name: &str) -> bool {
    ELEVATED_TOOLS.contains(&name)
}

/// Return a human-readable risk description for a known tool, or `None`.
pub fn get_tool_risk_description(name: &str) -> Option<&'static str> {
    match name {
        "nexibot_execute" => Some(
            "Executes arbitrary shell commands. Can modify the system, delete files, or exfiltrate data.",
        ),
        "nexibot_filesystem" => Some(
            "Reads and writes files on disk. Can access sensitive data or overwrite critical files.",
        ),
        "nexibot_settings" => Some(
            "Modifies NexiBot configuration at runtime. Can weaken security settings or change API keys.",
        ),
        "nexibot_soul" => Some(
            "Modifies the agent's identity and behavior via SOUL.md. Can alter personality and override safety guidelines.",
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_dangerous_tool() {
        assert!(is_dangerous_tool("nexibot_execute"));
        assert!(is_dangerous_tool("nexibot_filesystem"));
        assert!(!is_dangerous_tool("nexibot_memory"));
        assert!(!is_dangerous_tool("nexibot_search"));
        assert!(!is_dangerous_tool(""));
    }

    #[test]
    fn test_is_gateway_denied() {
        assert!(is_gateway_denied("nexibot_execute"));
        assert!(is_gateway_denied("nexibot_filesystem"));
        assert!(!is_gateway_denied("nexibot_settings"));
        assert!(!is_gateway_denied("nexibot_soul"));
    }

    #[test]
    fn test_is_elevated_tool() {
        assert!(is_elevated_tool("nexibot_settings"));
        assert!(is_elevated_tool("nexibot_soul"));
        assert!(!is_elevated_tool("nexibot_execute"));
        assert!(!is_elevated_tool("nexibot_memory"));
    }

    #[test]
    fn test_get_tool_risk_description_known() {
        assert!(get_tool_risk_description("nexibot_execute").is_some());
        assert!(get_tool_risk_description("nexibot_filesystem").is_some());
        assert!(get_tool_risk_description("nexibot_settings").is_some());
        assert!(get_tool_risk_description("nexibot_soul").is_some());
    }

    #[test]
    fn test_get_tool_risk_description_unknown() {
        assert!(get_tool_risk_description("nexibot_memory").is_none());
        assert!(get_tool_risk_description("unknown_tool").is_none());
        assert!(get_tool_risk_description("").is_none());
    }

    #[test]
    fn test_constants_are_non_empty() {
        assert!(!DANGEROUS_TOOLS.is_empty());
        assert!(!GATEWAY_DENIED_TOOLS.is_empty());
        assert!(!ELEVATED_TOOLS.is_empty());
    }

    #[test]
    fn test_dangerous_tools_are_gateway_denied() {
        // All dangerous tools should also be gateway-denied (superset check).
        for tool in DANGEROUS_TOOLS {
            assert!(
                is_gateway_denied(tool),
                "Dangerous tool '{}' should also be gateway-denied",
                tool
            );
        }
    }
}
