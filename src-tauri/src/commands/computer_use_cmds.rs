//! Computer Use and accessibility commands

use crate::native_control;

/// Check if accessibility permissions are granted (macOS)
#[tauri::command]
pub async fn check_accessibility_permissions() -> Result<bool, String> {
    native_control::check_accessibility_permissions().map_err(|e| e.to_string())
}

/// Request accessibility permissions (macOS — opens System Preferences)
#[tauri::command]
pub async fn request_accessibility_permissions() -> Result<(), String> {
    native_control::request_accessibility_permissions().map_err(|e| e.to_string())
}
