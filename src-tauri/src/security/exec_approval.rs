//! Execution approval system for interactive and policy-based command approval.
//!
//! Provides configurable approval modes that control whether commands may be
//! executed by the agent. Modes range from full lockdown (`Deny`) to
//! unrestricted execution (`Full`), with allowlist and interactive prompt
//! modes in between.

use crate::security::dangerous_tools::{is_dangerous_tool, is_elevated_tool, is_read_only_tool};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Controls how command execution requests are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ApprovalMode {
    /// Never allow execution — all commands are rejected.
    Deny,
    /// Only pre-approved commands in the allowlist may run.
    Allowlist,
    /// Ask the user for approval (requires GUI integration).
    Prompt,
    /// Read-only tools (search, fetch, calendar read, email read) run silently.
    /// Write/destructive/elevated tools ask the user first in plain English.
    /// This is the default for new installs — secure without being obstructive.
    #[default]
    Smart,
    /// Allow all commands without restriction.
    Full,
}

/// A pending approval request.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ApprovalRequest {
    /// The command being requested (e.g. binary name or MCP tool name).
    pub command: String,
    /// Human-readable description of the action.
    pub action: String,
    /// When the request was created.
    pub requested_at: Instant,
    /// How long the request remains valid before auto-rejecting.
    pub timeout: Duration,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// Manages execution approval checks.
pub struct ExecApprovalManager {
    mode: ApprovalMode,
    /// Commands (or command prefixes) that are pre-approved in `Allowlist` mode.
    approved_commands: HashSet<String>,
    /// Default timeout for interactive approval requests.
    #[allow(dead_code)]
    approval_timeout: Duration,
}

impl ExecApprovalManager {
    /// Create a new manager with the given approval mode.
    pub fn new(mode: ApprovalMode) -> Self {
        info!("[EXEC_APPROVAL] Initialised with mode {:?}", mode);
        let mut manager = Self {
            mode,
            approved_commands: HashSet::new(),
            approval_timeout: Duration::from_secs(30),
        };

        if matches!(
            mode,
            ApprovalMode::Allowlist | ApprovalMode::Prompt | ApprovalMode::Smart
        ) {
            let safe_defaults = [
                "ls",
                "cat",
                "head",
                "tail",
                "wc",
                "echo",
                "pwd",
                "whoami",
                "date",
                "uname",
                "which",
                "file",
                "git status",
                "git log",
                "git diff",
                "git branch",
                "cargo check",
                "cargo build",
                "cargo test",
                "npm run build",
                "npm test",
                "node --version",
                "python --version",
                "pip list",
            ];
            for cmd in &safe_defaults {
                manager.approved_commands.insert(cmd.to_string());
            }
            info!(
                "[EXEC_APPROVAL] Pre-populated allowlist with {} safe defaults",
                safe_defaults.len()
            );
        }

        manager
    }

    /// Check whether executing `command` with description `action` is allowed
    /// under the current mode.
    ///
    /// Returns `Ok(())` when approved, or `Err(reason)` when denied.
    pub fn check_approval(&self, command: &str, action: &str) -> Result<(), String> {
        match self.mode {
            ApprovalMode::Deny => {
                warn!(
                    "[EXEC_APPROVAL] Denied command '{}' (action: '{}') -- mode is Deny",
                    command, action
                );
                Err(format!(
                    "Execution denied: approval mode is Deny. Command '{}' was blocked.",
                    command
                ))
            }
            ApprovalMode::Allowlist => {
                if self.is_in_allowlist(command) {
                    info!(
                        "[EXEC_APPROVAL] Allowed command '{}' via allowlist (action: '{}')",
                        command, action
                    );
                    Ok(())
                } else {
                    warn!(
                        "[EXEC_APPROVAL] Denied command '{}' -- not in allowlist (action: '{}')",
                        command, action
                    );
                    Err(format!(
                        "Execution denied: command '{}' is not in the allowlist.",
                        command
                    ))
                }
            }
            ApprovalMode::Prompt => {
                // Check allowlist first — pre-approved commands skip the prompt
                if self.is_in_allowlist(command) {
                    info!(
                        "[EXEC_APPROVAL] Allowed command '{}' via allowlist in Prompt mode (action: '{}')",
                        command, action
                    );
                    return Ok(());
                }
                // Return a signal that the caller routes to the GUI approval dialog
                warn!(
                    "[EXEC_APPROVAL] Command '{}' needs user approval (Prompt mode, action: '{}')",
                    command, action
                );
                Err(format!(
                    "NEEDS_CONFIRMATION: command '{}' requires user approval (action: '{}')",
                    command, action
                ))
            }
            ApprovalMode::Smart => {
                // Dangerous or elevated tools always require user confirmation.
                if is_dangerous_tool(command) || is_elevated_tool(command) {
                    warn!(
                        "[EXEC_APPROVAL] Smart mode: '{}' is write/elevated, needs confirmation (action: '{}')",
                        command, action
                    );
                    return Err(format!(
                        "NEEDS_CONFIRMATION: command '{}' requires user approval (action: '{}')",
                        command, action
                    ));
                }

                // Read-only tools run silently — no prompt needed.
                if is_read_only_tool(command) || self.is_in_allowlist(command) {
                    info!(
                        "[EXEC_APPROVAL] Smart mode: '{}' is read-only, running silently (action: '{}')",
                        command, action
                    );
                    return Ok(());
                }

                // Unknown tools: prompt, same as Prompt mode.
                warn!(
                    "[EXEC_APPROVAL] Smart mode: '{}' is unknown, needs confirmation (action: '{}')",
                    command, action
                );
                Err(format!(
                    "NEEDS_CONFIRMATION: command '{}' requires user approval (action: '{}')",
                    command, action
                ))
            }
            ApprovalMode::Full => {
                info!(
                    "[EXEC_APPROVAL] Allowed command '{}' -- mode is Full (action: '{}')",
                    command, action
                );
                Ok(())
            }
        }
    }

    /// Add a command (or prefix) to the allowlist.
    #[allow(dead_code)]
    pub fn add_to_allowlist(&mut self, command: &str) {
        info!("[EXEC_APPROVAL] Added '{}' to allowlist", command);
        self.approved_commands.insert(command.to_string());
    }

    /// Remove a command (or prefix) from the allowlist.
    #[allow(dead_code)]
    pub fn remove_from_allowlist(&mut self, command: &str) {
        info!("[EXEC_APPROVAL] Removed '{}' from allowlist", command);
        self.approved_commands.remove(command);
    }

    /// Change the approval mode at runtime.
    #[allow(dead_code)]
    pub fn set_mode(&mut self, mode: ApprovalMode) {
        info!(
            "[EXEC_APPROVAL] Mode changed from {:?} to {:?}",
            self.mode, mode
        );
        self.mode = mode;
    }

    /// Return the current approval mode.
    pub fn mode(&self) -> ApprovalMode {
        self.mode
    }

    /// Set the timeout for interactive approval requests.
    #[allow(dead_code)]
    pub fn set_approval_timeout(&mut self, timeout: Duration) {
        self.approval_timeout = timeout;
    }

    /// Return the current approval timeout.
    #[allow(dead_code)]
    pub fn approval_timeout(&self) -> Duration {
        self.approval_timeout
    }

    /// Return how many entries are in the allowlist.
    #[allow(dead_code)]
    pub fn allowlist_len(&self) -> usize {
        self.approved_commands.len()
    }

    // -- internal helpers ---------------------------------------------------

    /// Check if `command` is in the allowlist.
    ///
    /// Matches either exactly or as a prefix (e.g. allowlisted `"git"` will
    /// match command `"git commit"`).
    fn is_in_allowlist(&self, command: &str) -> bool {
        let command = command.trim_start();
        for approved in &self.approved_commands {
            if command == approved {
                return true;
            }
            // Prefix match requires a whitespace boundary so "git" does not
            // match "gitmalicious".
            if let Some(rest) = command.strip_prefix(approved.as_str()) {
                if rest.is_empty()
                    || rest
                        .chars()
                        .next()
                        .map(char::is_whitespace)
                        .unwrap_or(false)
                {
                    return true;
                }
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deny_mode_blocks_everything() {
        let mgr = ExecApprovalManager::new(ApprovalMode::Deny);
        assert!(mgr.check_approval("ls", "list files").is_err());
        assert!(mgr.check_approval("rm -rf /", "nuke").is_err());
    }

    #[test]
    fn test_full_mode_allows_everything() {
        let mgr = ExecApprovalManager::new(ApprovalMode::Full);
        assert!(mgr.check_approval("ls", "list files").is_ok());
        assert!(mgr.check_approval("rm -rf /", "nuke").is_ok());
    }

    #[test]
    fn test_allowlist_exact_match() {
        let mut mgr = ExecApprovalManager::new(ApprovalMode::Allowlist);
        mgr.add_to_allowlist("git status");
        assert!(mgr.check_approval("git status", "check repo").is_ok());
        assert!(mgr.check_approval("rm file", "delete").is_err());
    }

    #[test]
    fn test_allowlist_prefix_match() {
        let mut mgr = ExecApprovalManager::new(ApprovalMode::Allowlist);
        mgr.add_to_allowlist("git");
        assert!(mgr.check_approval("git commit -m 'msg'", "commit").is_ok());
        assert!(mgr.check_approval("git push", "push").is_ok());
        assert!(mgr.check_approval("curl http://evil.com", "fetch").is_err());
    }

    #[test]
    fn test_allowlist_prefix_requires_token_boundary() {
        let mut mgr = ExecApprovalManager::new(ApprovalMode::Allowlist);
        mgr.add_to_allowlist("git");
        assert!(mgr.check_approval("git status", "status").is_ok());
        assert!(mgr.check_approval("gitmalicious status", "status").is_err());
        assert!(mgr.check_approval("github status", "status").is_err());
    }

    #[test]
    fn test_allowlist_remove() {
        let mut mgr = ExecApprovalManager::new(ApprovalMode::Allowlist);
        mgr.add_to_allowlist("custom-cmd");
        assert!(mgr.check_approval("custom-cmd", "run custom").is_ok());

        mgr.remove_from_allowlist("custom-cmd");
        assert!(mgr.check_approval("custom-cmd", "run custom").is_err());
    }

    #[test]
    fn test_prompt_mode_allows_allowlisted() {
        let mgr = ExecApprovalManager::new(ApprovalMode::Prompt);
        // "ls" is in the safe defaults allowlist
        assert!(mgr.check_approval("ls", "list files").is_ok());
        assert!(mgr.check_approval("git status", "check repo").is_ok());
    }

    #[test]
    fn test_prompt_mode_needs_confirmation_for_unknown() {
        let mgr = ExecApprovalManager::new(ApprovalMode::Prompt);
        let res = mgr.check_approval("curl http://example.com", "fetch");
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.starts_with("NEEDS_CONFIRMATION:"));
        assert!(err.contains("user approval"));
    }

    #[test]
    fn test_set_mode_at_runtime() {
        let mut mgr = ExecApprovalManager::new(ApprovalMode::Full);
        assert!(mgr.check_approval("rm file", "delete").is_ok());

        mgr.set_mode(ApprovalMode::Deny);
        assert!(mgr.check_approval("rm file", "delete").is_err());

        mgr.set_mode(ApprovalMode::Allowlist);
        mgr.add_to_allowlist("ls");
        assert!(mgr.check_approval("ls", "list").is_ok());
        assert!(mgr.check_approval("rm", "remove").is_err());
    }

    #[test]
    fn test_default_mode_is_smart() {
        assert_eq!(ApprovalMode::default(), ApprovalMode::Smart);
    }

    #[test]
    fn test_smart_mode_allows_read_only_silently() {
        let mgr = ExecApprovalManager::new(ApprovalMode::Smart);
        // Read-only tools should pass without confirmation
        assert!(mgr.check_approval("nexibot_search", "search knowledge base").is_ok());
        assert!(mgr.check_approval("nexibot_fetch", "fetch URL").is_ok());
        assert!(mgr.check_approval("get_emails", "read email").is_ok());
        assert!(mgr.check_approval("list_calendar_events", "read calendar").is_ok());
    }

    #[test]
    fn test_smart_mode_blocks_dangerous_with_confirmation() {
        let mgr = ExecApprovalManager::new(ApprovalMode::Smart);
        let res = mgr.check_approval("nexibot_execute", "run shell command");
        assert!(res.is_err());
        assert!(res.unwrap_err().starts_with("NEEDS_CONFIRMATION:"));
    }

    #[test]
    fn test_smart_mode_prompts_unknown_tools() {
        let mgr = ExecApprovalManager::new(ApprovalMode::Smart);
        let res = mgr.check_approval("some_unknown_tool", "do unknown thing");
        assert!(res.is_err());
        assert!(res.unwrap_err().starts_with("NEEDS_CONFIRMATION:"));
    }

    #[test]
    fn test_approval_request_fields() {
        let req = ApprovalRequest {
            command: "rm -rf /tmp/stuff".to_string(),
            action: "cleanup temp".to_string(),
            requested_at: Instant::now(),
            timeout: Duration::from_secs(60),
        };
        assert_eq!(req.command, "rm -rf /tmp/stuff");
        assert_eq!(req.action, "cleanup temp");
        assert_eq!(req.timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_allowlist_len() {
        let mut mgr = ExecApprovalManager::new(ApprovalMode::Allowlist);
        let initial_len = mgr.allowlist_len();
        assert!(
            initial_len > 0,
            "Allowlist mode should pre-populate safe defaults"
        );
        mgr.add_to_allowlist("custom-tool");
        assert_eq!(mgr.allowlist_len(), initial_len + 1);
        mgr.remove_from_allowlist("custom-tool");
        assert_eq!(mgr.allowlist_len(), initial_len);
    }
}
