//! Tauri commands for the memory dreaming engine.
use serde_json::Value;
use tauri::State;

use crate::commands::AppState;
use crate::memory_dreaming::{DreamLogEntry, DreamStatus};

/// Get current dreaming engine status.
#[tauri::command]
pub async fn get_dream_status(state: State<'_, AppState>) -> Result<DreamStatus, String> {
    let status = state.dreaming_engine.status.read().await;
    Ok(status.clone())
}

/// Get the dream cycle log (up to last 100 entries).
#[tauri::command]
pub async fn get_dream_log(state: State<'_, AppState>) -> Result<Vec<DreamLogEntry>, String> {
    let log = state.dreaming_engine.log.read().await;
    Ok(log.clone())
}

/// Manually trigger a dream cycle (runs in background).
#[tauri::command]
pub async fn trigger_dream_cycle(state: State<'_, AppState>) -> Result<Value, String> {
    let engine = state.dreaming_engine.clone();
    let advanced_memory = state.advanced_memory_manager.clone();
    let claude = state.claude_client.clone();
    tokio::spawn(async move {
        let client = claude.read().await;
        match engine.run_cycle(&advanced_memory, Some(&*client)).await {
            Ok(entry) => {
                tracing::info!("[DREAMING] Manual cycle complete: {}ms", entry.duration_ms);
            }
            Err(e) => {
                tracing::warn!("[DREAMING] Manual cycle error: {}", e);
            }
        }
    });
    Ok(serde_json::json!({ "status": "started" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_dream_status_result_serializable() {
        let status = DreamStatus::default();
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"phase\""), "json: {}", json);
    }
}
