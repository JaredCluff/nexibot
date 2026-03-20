//! Tauri commands for log management: viewing, filtering, level control, export.

use tauri::State;

use crate::commands::AppState;
use crate::logging;

/// Retrieve recent log entries from the in-memory ring buffer.
///
/// - `count`: max number of entries to return (0 = all buffered entries)
/// - `level`: optional filter by level (e.g. "WARN", "ERROR")
/// - `subsystem`: optional filter by subsystem tag (e.g. "TELEGRAM", "BRIDGE")
#[tauri::command]
pub async fn get_logs(
    state: State<'_, AppState>,
    count: Option<usize>,
    level: Option<String>,
    subsystem: Option<String>,
) -> Result<Vec<logging::LogEntry>, String> {
    let log_state = state
        .log_state
        .as_ref()
        .ok_or_else(|| "Logging not initialized".to_string())?;

    Ok(log_state
        .ring_buffer
        .recent(count.unwrap_or(200), level.as_deref(), subsystem.as_deref()))
}

/// Get the current log level.
#[tauri::command]
pub async fn get_log_level(state: State<'_, AppState>) -> Result<String, String> {
    let log_state = state
        .log_state
        .as_ref()
        .ok_or_else(|| "Logging not initialized".to_string())?;

    Ok(logging::get_level(log_state))
}

/// Change the log level at runtime (e.g., "debug", "info", "warn", "error").
#[tauri::command]
pub async fn set_log_level(state: State<'_, AppState>, level: String) -> Result<String, String> {
    let log_state = state
        .log_state
        .as_ref()
        .ok_or_else(|| "Logging not initialized".to_string())?;

    logging::set_level(log_state, &level)?;
    tracing::info!("[LOGGING] Log level changed to: {}", level);

    Ok(format!("Log level set to {}", level))
}

/// Clear all entries from the in-memory log ring buffer.
#[tauri::command]
pub async fn clear_logs(state: State<'_, AppState>) -> Result<String, String> {
    let log_state = state
        .log_state
        .as_ref()
        .ok_or_else(|| "Logging not initialized".to_string())?;

    log_state.ring_buffer.clear();
    Ok("Log buffer cleared".to_string())
}

/// Export recent logs as a JSON string (for saving to file from the frontend).
#[tauri::command]
pub async fn export_logs(
    state: State<'_, AppState>,
    count: Option<usize>,
) -> Result<String, String> {
    let log_state = state
        .log_state
        .as_ref()
        .ok_or_else(|| "Logging not initialized".to_string())?;

    let entries = log_state.ring_buffer.recent(count.unwrap_or(0), None, None);
    serde_json::to_string_pretty(&entries).map_err(|e| format!("Serialization error: {}", e))
}

/// Get the path to the log directory (for the frontend to show / open).
#[tauri::command]
pub async fn get_log_dir() -> Result<String, String> {
    let dir = logging::default_log_dir();
    Ok(dir.to_string_lossy().to_string())
}
