//! Gateway RPC method authorization scopes.
//!
//! Defines 5 operator scopes and maps implemented gateway methods to required scopes.
//! Default-deny: unclassified methods require admin scope.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Operator permission scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// Full administrative control — implies all other scopes.
    Admin,
    /// Read-only access to status, config, sessions.
    Read,
    /// Write access to send messages, modify config, manage sessions.
    Write,
    /// Reserved for approval RPC methods when those endpoints are enabled.
    Approvals,
    /// Reserved for pairing-management RPC methods when enabled.
    Pairing,
}

impl Scope {
    /// Check if this scope implies another scope.
    ///
    /// Admin implies all scopes. Write implies Read.
    pub fn implies(&self, other: &Scope) -> bool {
        if *self == Scope::Admin {
            return true;
        }
        if *self == Scope::Write && *other == Scope::Read {
            return true;
        }
        *self == *other
    }
}

/// Map of gateway method names to required scopes.
fn build_scope_map() -> HashMap<&'static str, Scope> {
    let mut map = HashMap::new();

    // Read-only methods
    for method in &[
        "health",
        "status",
        "get_config",
        "list_sessions",
        "list_agents",
        "list_mcp_servers",
        "get_mcp_server_status",
        "list_mcp_tools",
        "list_skills",
        "get_context_usage",
        "get_bridge_status",
        "get_voice_status",
        "get_defense_status",
        "get_guardrails_config",
        "get_audit_report",
        "get_tool_permissions",
        "list_oauth_profiles",
        "get_oauth_status",
        "list_subscriptions",
        "list_background_tasks",
        "list_scheduled_tasks",
        "list_integration_credentials",
        "get_soul_config",
        "get_agent_capabilities",
        "search_memories",
        "get_memory",
        "search_k2k",
    ] {
        map.insert(*method, Scope::Read);
    }

    // Write methods
    for method in &[
        "send_message",
        "send_message_with_events",
        "compact_conversation",
        "update_config",
        "save_config",
        "create_session",
        "delete_session",
        "save_memory",
        "delete_memory",
        "update_soul",
        "add_mcp_server",
        "remove_mcp_server",
        "connect_mcp_server",
        "disconnect_mcp_server",
        "install_skill",
        "uninstall_skill",
        "reload_skills",
        "reset_bundled_skills",
        "create_scheduled_task",
        "delete_scheduled_task",
        "start_voice_service",
        "stop_voice_service",
        "update_tool_permissions",
        "store_integration_credential",
        "delete_integration_credential",
        "submit_agent_task",
    ] {
        map.insert(*method, Scope::Write);
    }

    // NOTE: Approval/Pairing RPC methods are intentionally not mapped yet.
    // They remain admin-by-default until protocol + handlers are implemented.

    // Admin methods (explicit)
    for method in &[
        "update_guardrails",
        "run_security_audit",
        "auto_fix_finding",
        "start_bridge",
        "stop_bridge",
        "restart_bridge",
        "set_dm_policy",
        "configure_gateway",
        "shutdown",
        "install_update",
    ] {
        map.insert(*method, Scope::Admin);
    }

    map
}

fn scope_map() -> &'static HashMap<&'static str, Scope> {
    static INSTANCE: OnceLock<HashMap<&'static str, Scope>> = OnceLock::new();
    INSTANCE.get_or_init(build_scope_map)
}

/// Resolve the required scope for a gateway method.
///
/// Returns the mapped scope, or `Scope::Admin` for unclassified methods
/// (default-deny principle).
pub fn resolve_required_scope(method: &str) -> Scope {
    scope_map().get(method).copied().unwrap_or(Scope::Admin)
}

/// Check if a set of granted scopes satisfies the required scope for a method.
pub fn check_authorization(granted_scopes: &[Scope], method: &str) -> Result<(), String> {
    let required = resolve_required_scope(method);
    for scope in granted_scopes {
        if scope.implies(&required) {
            return Ok(());
        }
    }
    Err(format!(
        "Insufficient permissions: method '{}' requires {:?} scope",
        method, required
    ))
}

/// Validate that a session's scopes permit calling the given method.
///
/// Returns `Ok(())` if authorized, or `Err` with a JSON-serializable error
/// message suitable for sending back to the client over the WebSocket.
///
/// This is the primary entry-point for scope enforcement on every incoming
/// RPC call and should be invoked **before** dispatching the method handler.
pub fn validate_scope(method: &str, session_scopes: &[Scope]) -> Result<(), ScopeError> {
    let required = resolve_required_scope(method);
    for scope in session_scopes {
        if scope.implies(&required) {
            return Ok(());
        }
    }
    Err(ScopeError {
        method: method.to_string(),
        required,
        granted: session_scopes.to_vec(),
    })
}

/// Error returned when a session's scopes are insufficient for a method.
#[derive(Debug, Clone)]
pub struct ScopeError {
    /// The method the client tried to call.
    pub method: String,
    /// The scope required by that method.
    pub required: Scope,
    /// The scopes the session was granted at authentication time.
    pub granted: Vec<Scope>,
}

impl std::fmt::Display for ScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Permission denied: method '{}' requires {:?} scope, session has {:?}",
            self.method, self.required, self.granted
        )
    }
}

impl std::error::Error for ScopeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_admin_implies_all() {
        assert!(Scope::Admin.implies(&Scope::Read));
        assert!(Scope::Admin.implies(&Scope::Write));
        assert!(Scope::Admin.implies(&Scope::Approvals));
        assert!(Scope::Admin.implies(&Scope::Pairing));
        assert!(Scope::Admin.implies(&Scope::Admin));
    }

    #[test]
    fn test_write_implies_read() {
        assert!(Scope::Write.implies(&Scope::Read));
        assert!(!Scope::Write.implies(&Scope::Admin));
        assert!(!Scope::Write.implies(&Scope::Approvals));
    }

    #[test]
    fn test_read_does_not_imply_write() {
        assert!(!Scope::Read.implies(&Scope::Write));
        assert!(!Scope::Read.implies(&Scope::Admin));
    }

    #[test]
    fn test_known_method_scopes() {
        assert_eq!(resolve_required_scope("health"), Scope::Read);
        assert_eq!(resolve_required_scope("send_message"), Scope::Write);
        assert_eq!(resolve_required_scope("shutdown"), Scope::Admin);
    }

    #[test]
    fn test_unknown_method_defaults_to_admin() {
        assert_eq!(resolve_required_scope("some_unknown_rpc"), Scope::Admin);
    }

    #[test]
    fn test_check_authorization_success() {
        assert!(check_authorization(&[Scope::Admin], "send_message").is_ok());
        assert!(check_authorization(&[Scope::Write], "send_message").is_ok());
        assert!(check_authorization(&[Scope::Read], "health").is_ok());
    }

    #[test]
    fn test_check_authorization_failure() {
        assert!(check_authorization(&[Scope::Read], "send_message").is_err());
        assert!(check_authorization(&[Scope::Write], "shutdown").is_err());
        assert!(check_authorization(&[Scope::Pairing], "update_config").is_err());
    }

    #[test]
    fn test_validate_scope_success() {
        assert!(validate_scope("health", &[Scope::Read]).is_ok());
        assert!(validate_scope("send_message", &[Scope::Write]).is_ok());
        assert!(validate_scope("shutdown", &[Scope::Admin]).is_ok());
        // Admin implies everything
        assert!(validate_scope("send_message", &[Scope::Admin]).is_ok());
        assert!(validate_scope("health", &[Scope::Admin]).is_ok());
        // Write implies Read
        assert!(validate_scope("health", &[Scope::Write]).is_ok());
    }

    #[test]
    fn test_validate_scope_failure() {
        let err = validate_scope("send_message", &[Scope::Read]).unwrap_err();
        assert_eq!(err.method, "send_message");
        assert_eq!(err.required, Scope::Write);
        assert_eq!(err.granted, vec![Scope::Read]);
    }

    #[test]
    fn test_validate_scope_unknown_method_requires_admin() {
        // Unknown methods default to Admin scope
        assert!(validate_scope("unknown_rpc", &[Scope::Write]).is_err());
        assert!(validate_scope("unknown_rpc", &[Scope::Admin]).is_ok());
    }

    #[test]
    fn test_validate_scope_empty_scopes_denied() {
        assert!(validate_scope("health", &[]).is_err());
        assert!(validate_scope("send_message", &[]).is_err());
    }

    #[test]
    fn test_scope_error_display() {
        let err = validate_scope("shutdown", &[Scope::Read]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("shutdown"));
        assert!(msg.contains("Admin"));
        assert!(msg.contains("Read"));
    }
}
