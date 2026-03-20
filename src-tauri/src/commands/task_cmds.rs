//! Background task management commands

use tauri::State;

use super::AppState;
use crate::task_manager::BackgroundTask;

/// List all background tasks
#[tauri::command]
pub async fn list_background_tasks(
    state: State<'_, AppState>,
) -> Result<Vec<BackgroundTask>, String> {
    let tm = state.task_manager.read().await;
    Ok(tm.list_tasks())
}
