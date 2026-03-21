//! Built-in nexibot_execute tool — run commands/scripts with safety layers.
//!
//! Disabled by default — users must explicitly enable in config.
//! Safety layers (defense in depth):
//! 0. Skill runtime exec gate (skill_runtime_exec_enabled flag)
//! 1. Config gate (enabled flag)
//! 2. DCG check via guardrails
//! 3. Blocked command patterns
//! 4. Allowed command whitelist (if configured)
//! 5. Execution approval check
//! 6. Docker sandbox routing
//! 7. Timeout enforcement (max 300s)
//! 8. Output truncation

use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::process::Command;
use tracing::{info, warn};

use crate::config::ExecuteConfig;
use crate::gated_shell::GatedShell;
use crate::guardrails::Guardrails;
use crate::sandbox::{SandboxConfig, SandboxFallback, docker::DockerSandbox};
use crate::sandbox::policy::{SandboxPolicy, should_sandbox};
use crate::security::credentials;
use crate::security::env_sanitize::{build_safe_env, SanitizeOptions};
use crate::security::exec_approval::ExecApprovalManager;
use crate::security::safe_bins;

/// Get the tool definition to pass to Claude.
/// Always registered so Claude knows the capability exists and can guide users to enable it.
pub fn nexibot_execute_tool_definition() -> Value {
    json!({
        "name": "nexibot_execute",
        "description": "Execute shell commands or scripts. Supports run_command (shell), run_python (Python 3), and run_node (Node.js). DISABLED by default for security — enable in settings under execute.enabled. When disabled, returns instructions for enabling. Safety features: command blocklist, Destructive Command Guard, timeout limits, output truncation.",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["run_command", "run_python", "run_node"],
                    "description": "Execution mode: run_command for shell, run_python for Python 3, run_node for Node.js"
                },
                "command": {
                    "type": "string",
                    "description": "The command to run (for run_command) or code to execute (for run_python/run_node)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 30000, max: 300000)"
                }
            },
            "required": ["action", "command"]
        }
    })
}

/// Execute the execute tool. Requires config (lock 1), guardrails (lock 2), and approval manager.
///
/// If `active_skill_env_vars` is provided, those environment variable names will be
/// resolved from the integration credential store and injected into the child process.
/// This is the skill-authorized credential injection mechanism.
pub async fn execute_execute_tool(
    input: &Value,
    config: &ExecuteConfig,
    guardrails: &mut Guardrails,
    approval_manager: Option<&ExecApprovalManager>,
    active_skill_env_vars: Option<&[String]>,
    gated_shell: Option<&GatedShell>,
    session_key: &str,
    agent_id: &str,
    sandbox_config: &SandboxConfig,
) -> String {
    // Gate 0: Skill runtime execution check
    if active_skill_env_vars.is_some() && !config.skill_runtime_exec_enabled {
        warn!("[EXECUTE] Blocked skill-initiated execution (skill_runtime_exec_enabled = false)");
        return json!({
            "error": "Skill runtime execution is disabled for safety.",
            "instructions": "To allow skills to trigger command execution, set execute.skill_runtime_exec_enabled: true in config.yaml",
            "config_path": "Settings > Code Execution > Allow Skill Execution"
        }).to_string();
    }

    // Gate 1: Config enabled check
    if !config.enabled {
        return json!({
            "error": "Code execution is disabled for safety.",
            "instructions": "To enable, add this to your NexiBot config.yaml:\n\nexecute:\n  enabled: true\n\nYou can also configure allowed_commands to restrict which commands are permitted.",
            "config_path": "Settings > Code Execution > Enable"
        }).to_string();
    }

    let action = match input.get("action").and_then(|a| a.as_str()) {
        Some(a) => a,
        None => return "Error: 'action' is required".to_string(),
    };

    let command = match input.get("command").and_then(|c| c.as_str()) {
        Some(c) if !c.trim().is_empty() => c.trim().to_string(),
        _ => return "Error: 'command' is required".to_string(),
    };

    let timeout_ms = input
        .get("timeout_ms")
        .and_then(|t| t.as_u64())
        .unwrap_or(config.default_timeout_ms)
        .min(300_000); // Hard cap at 5 minutes

    // Gate 2: DCG check via guardrails
    if config.use_dcg {
        // Pass interpreter + command to DCG without shell quoting to prevent
        // quote-escape bypass (e.g. `'; malicious_cmd #` escaping single quotes).
        // The actual execution uses safe arg passing via Command::args().
        let check_str = match action {
            "run_command" => command.clone(),
            "run_python" => format!("python3 -c [CODE] {}", command),
            "run_node" => format!("node -e [CODE] {}", command),
            _ => command.clone(),
        };

        if let Err(violations) = guardrails.check_command(&check_str) {
            warn!("[EXECUTE] Command blocked by guardrails: {:?}", violations);
            return json!({
                "error": "Command blocked by safety checks",
                "reason": format!("{:?}", violations.first()),
            })
            .to_string();
        }
    }

    // Gate 3: Blocked command patterns
    for blocked in &config.blocked_commands {
        if command.contains(blocked) {
            warn!("[EXECUTE] Command matches blocked pattern: {}", blocked);
            return json!({
                "error": "Command matches a blocked pattern",
                "blocked_pattern": blocked,
            })
            .to_string();
        }
    }

    // Gate 4: Allowed command whitelist (if configured)
    if !config.allowed_commands.is_empty() {
        let allowed = config
            .allowed_commands
            .iter()
            .any(|a| matches_allowed_command(command.as_str(), a));
        if !allowed {
            return json!({
                "error": "Command not in allowed list",
                "allowed_commands": config.allowed_commands,
                "hint": "Add the command to execute.allowed_commands in config.yaml"
            })
            .to_string();
        }
    }

    // Gate 5: Execution approval check.
    //
    // SECURITY: `None` previously allowed any command to run without policy
    // checks (e.g. in headless/autonomous mode). This is now fail-closed:
    // the absence of an approval manager is treated as a configuration error
    // and the command is denied. Callers that have already verified approval
    // through another mechanism (GUI dialog, autonomous mode gate, etc.) must
    // pass a Full-mode ExecApprovalManager instead of None.
    match approval_manager {
        Some(approval_mgr) => {
            if let Err(reason) = approval_mgr.check_approval(&command, action) {
                warn!("[EXECUTE] Command blocked by approval system: {}", reason);
                return json!({
                    "error": "Command blocked by execution approval policy",
                    "reason": reason,
                })
                .to_string();
            }
        }
        None => {
            warn!(
                "[EXECUTE] No approval manager configured — denying command '{}' (fail closed)",
                command
            );
            return json!({
                "error": "Command execution denied: no approval policy is configured.",
                "hint": "Set execute.approval_mode in config.yaml (smart/allowlist/prompt/full) to permit execution.",
            })
            .to_string();
        }
    }

    let start = Instant::now();

    info!(
        "[EXECUTE] Running ({}) with timeout {}ms: {}",
        action,
        timeout_ms,
        if command.len() > 100 {
            format!("{}...", &command[..100])
        } else {
            command.clone()
        }
    );

    // Gate 6: Docker sandbox routing
    if action == "run_command" && sandbox_config.enabled {
        let policy = config.sandbox_policy.unwrap_or(SandboxPolicy::default());
        if policy != SandboxPolicy::Never && should_sandbox(&command, &policy) {
            if DockerSandbox::is_docker_available().await {
                info!("[EXECUTE] Routing command through Docker sandbox");
                let mut sandbox = DockerSandbox::new(sandbox_config.clone());
                match sandbox.create_container().await {
                    Ok(_) => {
                        if let Err(e) = sandbox.start_container().await {
                            warn!("[EXECUTE] Sandbox container start failed: {}", e);
                            // Fall through to fallback check
                        } else {
                            let timeout = Duration::from_millis(timeout_ms);
                            let result = sandbox.exec_in_container(&command, timeout).await;
                            let _ = sandbox.stop_container().await;
                            let _ = sandbox.remove_container().await;
                            return match result {
                                Ok(exec_result) => {
                                    json!({
                                        "stdout": exec_result.stdout,
                                        "stderr": exec_result.stderr,
                                        "exit_code": exec_result.exit_code,
                                        "sandboxed": true,
                                        "timed_out": exec_result.timed_out,
                                        "duration_ms": start.elapsed().as_millis() as u64,
                                    }).to_string()
                                }
                                Err(e) => {
                                    json!({
                                        "error": format!("Sandbox execution failed: {}", e),
                                        "sandboxed": true,
                                    }).to_string()
                                }
                            };
                        }
                    }
                    Err(e) => {
                        warn!("[EXECUTE] Failed to create sandbox container: {}", e);
                        // Fall through to fallback check
                    }
                }
                // Docker failed — check fallback
                match sandbox_config.fallback {
                    SandboxFallback::Deny => {
                        warn!("[EXECUTE] Docker unavailable and fallback=Deny, blocking command");
                        return json!({
                            "error": "Command requires sandbox but Docker is unavailable",
                            "hint": "Install Docker or set sandbox.fallback_on_docker_unavailable: allow_host",
                        }).to_string();
                    }
                    SandboxFallback::AllowHost => {
                        info!("[EXECUTE] Docker unavailable, falling back to host execution");
                        // Fall through to normal execution below
                    }
                }
            } else {
                // Docker not available at all
                match sandbox_config.fallback {
                    SandboxFallback::Deny => {
                        return json!({
                            "error": "Command requires sandbox but Docker is not installed",
                            "hint": "Install Docker or set sandbox.fallback_on_docker_unavailable: allow_host",
                        }).to_string();
                    }
                    SandboxFallback::AllowHost => {
                        info!("[EXECUTE] Docker not available, falling back to host execution");
                    }
                }
            }
        }
    }

    // NexiGate: route run_command through gated shell if enabled
    if action == "run_command" {
        if let Some(shell) = gated_shell {
            if shell.is_enabled() {
                let timeout_secs = (timeout_ms / 1000).max(1);
                let result = shell
                    .execute(session_key, agent_id, &command, timeout_secs)
                    .await;
                return match result {
                    Ok(o) => o.to_json_string(),
                    Err(e) => json!({ "error": e.to_string(), "gate": "nexigate" }).to_string(),
                };
            }
        }
    }

    // Build the process command with safe binary resolution
    #[cfg(windows)]
    let (binary_name, args): (&str, Vec<&str>) = match action {
        "run_command" => ("cmd.exe", vec!["/C", &command]),
        "run_python" => ("python", vec!["-c", &command]),
        "run_node" => ("node", vec!["-e", &command]),
        _ => {
            return format!(
                "Error: Unknown action '{}'. Use run_command, run_python, or run_node.",
                action
            )
        }
    };
    #[cfg(not(windows))]
    let (binary_name, args): (&str, Vec<&str>) = match action {
        "run_command" => ("sh", vec!["-c", &command]),
        "run_python" => ("python3", vec!["-c", &command]),
        "run_node" => ("node", vec!["-e", &command]),
        _ => {
            return format!(
                "Error: Unknown action '{}'. Use run_command, run_python, or run_node.",
                action
            )
        }
    };

    // Resolve binary to absolute path in a trusted directory
    let resolved_binary = match safe_bins::validate_binary(binary_name) {
        Ok(path) => path,
        Err(e) => {
            // Fall back to resolve_binary without trust check for common shells
            match safe_bins::resolve_binary(binary_name) {
                Some(path) => {
                    warn!(
                        "[EXECUTE] Binary '{}' not in trusted dir, using resolved path: {}",
                        binary_name,
                        path.display()
                    );
                    path
                }
                None => {
                    return json!({
                        "error": format!("Binary not found: {}", e),
                        "hint": match action {
                            "run_python" => "Is Python 3 installed? Try: python3 --version",
                            "run_node" => "Is Node.js installed? Try: node --version",
                            _ => "Check that the command exists in PATH",
                        }
                    })
                    .to_string();
                }
            }
        }
    };

    let mut cmd = Command::new(&resolved_binary);
    cmd.args(&args);

    // Sanitize environment variables to prevent secret leakage
    let safe_env = build_safe_env(&SanitizeOptions::default());
    if !safe_env.blocked.is_empty() {
        info!(
            "[EXECUTE] Blocked {} sensitive env vars from child process",
            safe_env.blocked.len()
        );
    }
    cmd.env_clear();
    cmd.envs(&safe_env.allowed);

    // Skill-authorized credential injection: if the active skill declares env vars,
    // resolve them from the integration credential store and inject only those.
    if let Some(declared_vars) = active_skill_env_vars {
        if !declared_vars.is_empty() {
            let resolved = credentials::resolve_skill_env_vars(declared_vars);
            if !resolved.is_empty() {
                info!(
                    "[EXECUTE] Injecting {} skill-declared credentials into child process",
                    resolved.len()
                );
                cmd.envs(&resolved);
            }
        }
    }

    // Set working directory if configured
    if let Some(ref wd) = config.working_directory {
        cmd.current_dir(wd);
    }

    // Gate 7: Timeout enforcement
    let output = match tokio::time::timeout(Duration::from_millis(timeout_ms), cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            warn!("[EXECUTE] Process failed to start: {}", e);
            return json!({
                "error": format!("Failed to start process: {}", e),
                "hint": match action {
                    "run_python" => "Is Python 3 installed? Try: python3 --version",
                    "run_node" => "Is Node.js installed? Try: node --version",
                    _ => "Check that the command exists in PATH",
                }
            })
            .to_string();
        }
        Err(_) => {
            warn!("[EXECUTE] Command timed out after {}ms", timeout_ms);
            return json!({
                "error": format!("Command timed out after {}ms", timeout_ms),
                "timeout_ms": timeout_ms,
            })
            .to_string();
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    // Gate 8: Output truncation
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let stdout_truncated = stdout.len() > config.max_output_bytes;
    let stderr_truncated = stderr.len() > config.max_output_bytes;

    // Walk char boundaries so we never slice mid-UTF-8-character.
    let utf8_boundary = |s: &str, max: usize| -> usize {
        s.char_indices()
            .take_while(|(i, _)| *i < max)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0)
    };

    let stdout_final = if stdout_truncated {
        let b = utf8_boundary(&stdout, config.max_output_bytes);
        format!(
            "{}\n\n[Output truncated at {} bytes]",
            &stdout[..b],
            config.max_output_bytes
        )
    } else {
        stdout
    };

    let stderr_final = if stderr_truncated {
        let b = utf8_boundary(&stderr, config.max_output_bytes);
        format!(
            "{}\n\n[Stderr truncated at {} bytes]",
            &stderr[..b],
            config.max_output_bytes
        )
    } else {
        stderr
    };

    let exit_code = output.status.code().unwrap_or(-1);
    info!(
        "[EXECUTE] Completed with exit code {} in {}ms",
        exit_code, duration_ms
    );

    json!({
        "stdout": stdout_final,
        "stderr": stderr_final,
        "exit_code": exit_code,
        "duration_ms": duration_ms,
    })
    .to_string()
}

fn matches_allowed_command(command: &str, allowed_entry: &str) -> bool {
    let command = command.trim_start();
    if command == allowed_entry {
        return true;
    }
    if let Some(rest) = command.strip_prefix(allowed_entry) {
        return rest.is_empty()
            || rest
                .chars()
                .next()
                .map(char::is_whitespace)
                .unwrap_or(false);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guardrails::GuardrailsConfig;
    use crate::security::exec_approval::{ApprovalMode, ExecApprovalManager};

    fn enabled_config() -> ExecuteConfig {
        ExecuteConfig {
            enabled: true,
            ..ExecuteConfig::default()
        }
    }

    fn echo_input() -> Value {
        json!({
            "action": "run_command",
            "command": "echo hello"
        })
    }

    /// A command NOT in the allowlist safe defaults (e.g., curl).
    fn non_allowlisted_input() -> Value {
        json!({
            "action": "run_command",
            "command": "curl --version"
        })
    }

    #[tokio::test]
    async fn test_allowlist_blocks_non_safe_command() {
        let config = enabled_config();
        let mut guardrails = Guardrails::new(GuardrailsConfig::default());
        let approval = ExecApprovalManager::new(ApprovalMode::Allowlist);

        let result = execute_execute_tool(
            &non_allowlisted_input(),
            &config,
            &mut guardrails,
            Some(&approval),
            None,
            None,
            "test-session",
            "test-agent",
            &SandboxConfig::default(),
        )
        .await;

        assert!(
            result.contains("not in the allowlist"),
            "Allowlist mode should block 'curl': {}",
            result
        );
    }

    #[test]
    fn test_matches_allowed_command_requires_token_boundary() {
        assert!(matches_allowed_command("git status", "git"));
        assert!(matches_allowed_command("git status --short", "git status"));
        assert!(!matches_allowed_command("gitmalicious status", "git"));
        assert!(!matches_allowed_command("git-status", "git"));
    }

    #[tokio::test]
    async fn test_allowed_commands_boundary_blocks_prefix_lookalike() {
        let mut config = enabled_config();
        config.allowed_commands = vec!["git".into()];
        config.blocked_commands.clear();
        config.use_dcg = false;
        let mut guardrails = Guardrails::new(GuardrailsConfig::default());
        let input = json!({
            "action": "run_command",
            "command": "gitmalicious status"
        });

        let result = execute_execute_tool(
            &input,
            &config,
            &mut guardrails,
            None,
            None,
            None,
            "test-session",
            "test-agent",
            &SandboxConfig::default(),
        )
        .await;

        assert!(
            result.contains("Command not in allowed list"),
            "Prefix lookalike should not pass execute.allowed_commands: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_none_approval_fails_closed() {
        let config = enabled_config();
        let mut guardrails = Guardrails::new(GuardrailsConfig::default());

        // Passing None for approval_manager must now fail closed.
        let result = execute_execute_tool(
            &echo_input(),
            &config,
            &mut guardrails,
            None,
            None,
            None,
            "test-session",
            "test-agent",
            &SandboxConfig::default(),
        )
        .await;

        // Must be denied — no unchecked execution without a configured policy.
        assert!(
            result.contains("no approval policy is configured") || result.contains("denied"),
            "None approval manager must fail closed, not allow execution: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_none_approval_blocks_any_command() {
        let config = enabled_config();
        let mut guardrails = Guardrails::new(GuardrailsConfig::default());

        // Even non-allowlisted commands must be denied without a policy (fail closed).
        let result = execute_execute_tool(
            &non_allowlisted_input(),
            &config,
            &mut guardrails,
            None,
            None,
            None,
            "test-session",
            "test-agent",
            &SandboxConfig::default(),
        )
        .await;

        assert!(
            result.contains("no approval policy is configured") || result.contains("denied"),
            "None approval manager must block all commands (fail closed): {}",
            result
        );
    }

    #[tokio::test]
    async fn test_full_mode_allows_any_command() {
        let config = enabled_config();
        let mut guardrails = Guardrails::new(GuardrailsConfig::default());
        let approval = ExecApprovalManager::new(ApprovalMode::Full);

        let result = execute_execute_tool(
            &non_allowlisted_input(),
            &config,
            &mut guardrails,
            Some(&approval),
            None,
            None,
            "test-session",
            "test-agent",
            &SandboxConfig::default(),
        )
        .await;

        assert!(
            !result.contains("not in the allowlist"),
            "Full mode should allow any command: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_disabled_config_blocks_execution() {
        let mut config = ExecuteConfig::default();
        config.enabled = false; // explicitly disable
        let mut guardrails = Guardrails::new(GuardrailsConfig::default());

        let result = execute_execute_tool(
            &echo_input(),
            &config,
            &mut guardrails,
            None,
            None,
            None,
            "test-session",
            "test-agent",
            &SandboxConfig::default(),
        )
        .await;

        assert!(
            result.contains("Code execution is disabled"),
            "Disabled config should block: {}",
            result
        );
    }
}
