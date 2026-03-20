//! Guardrails management commands

use crate::guardrails::{get_security_level_warnings, GuardrailsConfig, SecurityLevel};
use tauri::State;
use tracing::warn;

use super::AppState;

/// Get current guardrails configuration
#[tauri::command]
pub async fn get_guardrails_config(state: State<'_, AppState>) -> Result<GuardrailsConfig, String> {
    let guardrails = state.guardrails.read().await;
    Ok(guardrails.get_config().clone())
}

/// Update guardrails configuration
#[tauri::command]
pub async fn update_guardrails_config(
    state: State<'_, AppState>,
    new_config: GuardrailsConfig,
) -> Result<(), String> {
    let previous = {
        let config = state.config.read().await;
        config.guardrails.clone()
    };

    // Persist first so runtime and config stay aligned across restarts.
    {
        let mut config = state.config.write().await;
        config.guardrails = new_config.clone();
        if let Err(e) = config.save() {
            config.guardrails = previous.clone();
            return Err(e.to_string());
        }
    }

    // Apply to runtime guardrails. If this fails, roll back persisted config.
    let mut guardrails = state.guardrails.write().await;
    if let Err(e) = guardrails.update_config(new_config) {
        drop(guardrails);
        let mut config = state.config.write().await;
        config.guardrails = previous;
        if let Err(rollback_err) = config.save() {
            warn!(
                "[GUARDRAILS] Failed to persist rollback after runtime update error: {}",
                rollback_err
            );
        }
        return Err(e.to_string());
    }

    let _ = state.config_changed.send(());
    Ok(())
}

/// Check if a command is safe to execute
#[tauri::command]
pub async fn check_command_safety(
    state: State<'_, AppState>,
    command: String,
) -> Result<(), Vec<String>> {
    let mut guardrails = state.guardrails.write().await;
    guardrails
        .check_command(&command)
        .map_err(|violations| violations.iter().map(|v| format!("{:?}", v)).collect())
}

/// Get security level warnings
#[tauri::command]
pub fn get_security_warnings(level: SecurityLevel) -> Vec<String> {
    get_security_level_warnings(level)
}

/// Execute a shell command with guardrails protection
#[tauri::command]
pub async fn execute_command_safe(
    state: State<'_, AppState>,
    command: String,
) -> Result<String, String> {
    // Check command safety before execution
    let mut guardrails = state.guardrails.write().await;
    if let Err(violations) = guardrails.check_command(&command) {
        let error_messages: Vec<String> = violations.iter().map(|v| {
            match v {
                crate::guardrails::GuardrailViolation::DangerousCommand { command, reason, severity } => {
                    format!("BLOCKED: Dangerous command detected\n\nCommand: {}\n\nReason: {}\n\nSeverity: {:?}", command, reason, severity)
                }
                _ => format!("{:?}", v),
            }
        }).collect();

        return Err(error_messages.join("\n\n"));
    }
    drop(guardrails);

    // This endpoint validates commands against guardrails only — it does not
    // execute them. Actual execution goes through the LLM tool loop's execute
    // tool, which applies additional gates (hard guards, DCG, exec approval).
    Ok("Command passed guardrails safety checks. Use the execute tool to run it.".to_string())
}
