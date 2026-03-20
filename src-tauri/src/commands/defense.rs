//! Defense pipeline and tool permissions commands

use std::collections::HashMap;
use tauri::State;

use crate::defense::DefenseStatus;
use crate::guardrails::ServerPermissions;

use super::AppState;

/// Get defense pipeline status
#[tauri::command]
pub async fn get_defense_status(state: State<'_, AppState>) -> Result<DefenseStatus, String> {
    let defense = state.defense_pipeline.read().await;
    Ok(defense.get_status())
}

/// Get per-server tool permissions
#[tauri::command]
pub async fn get_tool_permissions(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let guardrails = state.guardrails.read().await;
    serde_json::to_value(guardrails.get_server_permissions()).map_err(|e| e.to_string())
}

/// Update tool permissions for a server
#[tauri::command]
pub async fn update_tool_permissions(
    state: State<'_, AppState>,
    permissions: HashMap<String, ServerPermissions>,
) -> Result<(), String> {
    let mut guardrails = state.guardrails.write().await;
    let previous_guardrails_config = guardrails.get_config().clone();
    let mut next = previous_guardrails_config.clone();
    next.server_permissions = permissions.clone();
    guardrails.update_config(next).map_err(|e| e.to_string())?;
    drop(guardrails);

    // Keep persisted config in sync with runtime guardrails.
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.guardrails.server_permissions = permissions;
    if let Err(e) = config.save() {
        *config = previous_config;
        drop(config);

        let mut guardrails = state.guardrails.write().await;
        let _ = guardrails.update_config(previous_guardrails_config);
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    Ok(())
}
