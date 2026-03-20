//! Autonomous mode management commands

use crate::config::AutonomousModeConfig;
use tauri::State;

use super::AppState;

/// Get current autonomous mode configuration
#[tauri::command]
pub async fn get_autonomous_config(
    state: State<'_, AppState>,
) -> Result<AutonomousModeConfig, String> {
    let config = state.config.read().await;
    Ok(config.autonomous_mode.clone())
}

/// Update autonomous mode configuration
#[tauri::command]
pub async fn update_autonomous_config(
    state: State<'_, AppState>,
    new_config: AutonomousModeConfig,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.autonomous_mode = new_config;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    // Signal config change for hot reload
    let _ = state.config_changed.send(());
    Ok(())
}
