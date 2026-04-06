//! Soul management commands

use crate::soul::{Soul, SoulTemplate};

/// Get the current active soul
#[tauri::command]
pub async fn get_soul() -> Result<Soul, String> {
    tokio::task::spawn_blocking(Soul::load)
        .await
        .map_err(|e| format!("Task panicked: {}", e))?
        .map_err(|e| e.to_string())
}

/// List available soul templates
#[tauri::command]
pub async fn list_soul_templates() -> Result<Vec<SoulTemplate>, String> {
    tokio::task::spawn_blocking(Soul::list_templates)
        .await
        .map_err(|e| format!("Task panicked: {}", e))?
        .map_err(|e| e.to_string())
}

/// Load a soul template as the active soul
#[tauri::command]
pub async fn load_soul_template(template_name: String) -> Result<Soul, String> {
    tokio::task::spawn_blocking(move || Soul::load_template(&template_name))
        .await
        .map_err(|e| format!("Task panicked: {}", e))?
        .map_err(|e| e.to_string())
}

/// Update the active soul content
#[tauri::command]
pub async fn update_soul(new_content: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let mut soul = Soul::load().map_err(|e| e.to_string())?;
        soul.update(new_content).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Task panicked: {}", e))?
}
