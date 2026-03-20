//! Heartbeat management commands

use tauri::State;
use tracing::info;

use crate::heartbeat::{HeartbeatConfig, HeartbeatResult};

use super::AppState;

/// Start the heartbeat loop
#[tauri::command]
pub async fn start_heartbeat(state: State<'_, AppState>) -> Result<(), String> {
    info!("Starting heartbeat");
    state
        .heartbeat_manager
        .start()
        .await
        .map_err(|e| e.to_string())
}

/// Stop the heartbeat loop
#[tauri::command]
pub async fn stop_heartbeat(state: State<'_, AppState>) -> Result<(), String> {
    info!("Stopping heartbeat");
    state
        .heartbeat_manager
        .stop()
        .await
        .map_err(|e| e.to_string())
}

/// Get current heartbeat configuration
#[tauri::command]
pub async fn get_heartbeat_config(state: State<'_, AppState>) -> Result<HeartbeatConfig, String> {
    Ok(state.heartbeat_manager.get_config().await)
}

/// Update heartbeat configuration
#[tauri::command]
pub async fn update_heartbeat_config(
    state: State<'_, AppState>,
    config: HeartbeatConfig,
) -> Result<(), String> {
    state
        .heartbeat_manager
        .update_config(config)
        .await
        .map_err(|e| e.to_string())
}

/// Check if heartbeat is running
#[tauri::command]
pub async fn is_heartbeat_running(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.heartbeat_manager.is_running().await)
}

/// Manually trigger a heartbeat
#[tauri::command]
pub async fn trigger_heartbeat(state: State<'_, AppState>) -> Result<HeartbeatResult, String> {
    state
        .heartbeat_manager
        .trigger_now()
        .await
        .map_err(|e| e.to_string())
}
