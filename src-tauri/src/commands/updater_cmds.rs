//! Tauri commands for auto-update functionality.

use serde::{Deserialize, Serialize};
use tauri_plugin_updater::UpdaterExt;
use tracing::info;

/// Information about an available update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub date: Option<String>,
    pub body: Option<String>,
}

/// Get the current app version (from Cargo.toml at build time).
#[tauri::command]
pub fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Check for available updates.
#[tauri::command]
pub async fn check_for_updates(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    info!("[UPDATER] Checking for updates...");

    let updater = app.updater_builder().build().map_err(|e| e.to_string())?;

    match updater.check().await {
        Ok(Some(update)) => {
            info!("[UPDATER] Update available: {}", update.version);
            Ok(Some(UpdateInfo {
                version: update.version.clone(),
                date: update.date.map(|d| d.to_string()),
                body: update.body.clone(),
            }))
        }
        Ok(None) => {
            info!("[UPDATER] No updates available");
            Ok(None)
        }
        Err(e) => {
            tracing::warn!("[UPDATER] Update check failed: {}", e);
            Err(e.to_string())
        }
    }
}

/// Download and install the available update.
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    info!("[UPDATER] Installing update...");

    let updater = app.updater_builder().build().map_err(|e| e.to_string())?;

    match updater.check().await {
        Ok(Some(update)) => {
            info!("[UPDATER] Downloading update {}...", update.version);
            update
                .download_and_install(|_, _| {}, || {})
                .await
                .map_err(|e| e.to_string())?;
            info!("[UPDATER] Update installed, restart required");
            Ok(())
        }
        Ok(None) => Err("No update available".to_string()),
        Err(e) => Err(e.to_string()),
    }
}
