//! Bridge management commands

use tauri::State;
use tracing::info;

use crate::bridge::BridgeStatus;

use super::AppState;

/// Get bridge service status
#[tauri::command]
pub async fn get_bridge_status(state: State<'_, AppState>) -> Result<BridgeStatus, String> {
    Ok(state.bridge_manager.read().await.get_status().await)
}

/// Start bridge service
#[tauri::command]
pub async fn start_bridge(state: State<'_, AppState>) -> Result<(), String> {
    info!("[BRIDGE] Starting bridge service via command");
    state
        .bridge_manager
        .read()
        .await
        .start()
        .await
        .map_err(|e| e.to_string())
}

/// Stop bridge service
#[tauri::command]
pub async fn stop_bridge(state: State<'_, AppState>) -> Result<(), String> {
    info!("[BRIDGE] Stopping bridge service via command");
    state
        .bridge_manager
        .read()
        .await
        .stop()
        .await
        .map_err(|e| e.to_string())
}

/// Restart bridge service
#[tauri::command]
pub async fn restart_bridge(state: State<'_, AppState>) -> Result<(), String> {
    info!("[BRIDGE] Restarting bridge service via command");
    state
        .bridge_manager
        .read()
        .await
        .restart()
        .await
        .map_err(|e| e.to_string())
}

/// Install bridge dependencies
#[tauri::command]
pub async fn install_bridge(state: State<'_, AppState>) -> Result<(), String> {
    info!("[BRIDGE] Installing bridge dependencies via command");
    state
        .bridge_manager
        .read()
        .await
        .install_dependencies()
        .await
        .map_err(|e| e.to_string())
}

/// Check bridge health
#[tauri::command]
pub async fn check_bridge_health(state: State<'_, AppState>) -> Result<String, String> {
    state
        .bridge_manager
        .read()
        .await
        .check_health()
        .await
        .map(|h| format!("{} v{}", h.service, h.version))
        .map_err(|e| e.to_string())
}

/// Ensure bridge is running
#[tauri::command]
pub async fn ensure_bridge_running(state: State<'_, AppState>) -> Result<(), String> {
    info!("[BRIDGE] Ensuring bridge is running via command");
    state
        .bridge_manager
        .read()
        .await
        .ensure_running()
        .await
        .map_err(|e| e.to_string())
}
