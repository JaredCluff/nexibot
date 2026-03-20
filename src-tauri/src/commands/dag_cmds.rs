//! DAG management Tauri commands.

use chrono::Utc;
use tauri::State;
use tracing::info;
use uuid::Uuid;

use crate::dag::{DagDefinition, DagRunSummary, DagTaskDefinition};

use super::AppState;

/// Create and start a DAG run from an inline definition.
#[tauri::command]
pub async fn dag_run_create(
    name: String,
    tasks: Vec<DagTaskDefinition>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    info!(
        "[DAG_CMD] Creating run '{}' with {} tasks",
        name,
        tasks.len()
    );

    let run_id = {
        let store = state
            .dag_store
            .lock()
            .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
        store
            .create_run(&name, None, &tasks)
            .map_err(|e| e.to_string())?
    };

    state
        .dag_executor
        .start_run(run_id.clone(), state.inner().clone(), app_handle);
    Ok(run_id)
}

/// Create and start a DAG run from a saved template.
#[tauri::command]
pub async fn dag_run_from_template(
    template_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    let (name, tasks) = {
        let store = state
            .dag_store
            .lock()
            .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
        let def = store
            .get_definition(&template_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Template '{}' not found", template_id))?;
        (def.name.clone(), def.tasks.clone())
    };

    let run_id = {
        let store = state
            .dag_store
            .lock()
            .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
        store
            .create_run(&name, Some(&template_id), &tasks)
            .map_err(|e| e.to_string())?
    };

    state
        .dag_executor
        .start_run(run_id.clone(), state.inner().clone(), app_handle);
    Ok(run_id)
}

/// Get the status/summary of a DAG run.
#[tauri::command]
pub async fn dag_run_status(
    run_id: String,
    state: State<'_, AppState>,
) -> Result<DagRunSummary, String> {
    let store = state
        .dag_store
        .lock()
        .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
    store
        .get_run_summary(&run_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Run '{}' not found", run_id))
}

/// Cancel a running DAG.
#[tauri::command]
pub async fn dag_run_cancel(
    run_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    info!("[DAG_CMD] Cancelling run '{}'", run_id);
    state.dag_executor.cancel_run(&run_id, &app_handle).await
}

/// Add a task to a running DAG dynamically.
#[tauri::command]
pub async fn dag_task_add(
    run_id: String,
    task: DagTaskDefinition,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    info!("[DAG_CMD] Adding task '{}' to run '{}'", task.key, run_id);
    state
        .dag_executor
        .add_task_to_run(&run_id, &task, &app_handle)
        .await
}

/// List recent DAG runs.
#[tauri::command]
pub async fn dag_run_list(
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::dag::DagRun>, String> {
    let store = state
        .dag_store
        .lock()
        .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
    store
        .list_runs(limit.unwrap_or(20))
        .map_err(|e| e.to_string())
}

/// Get execution history for a run.
#[tauri::command]
pub async fn dag_run_history(
    run_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<crate::dag::DagHistoryEntry>, String> {
    let store = state
        .dag_store
        .lock()
        .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
    store.get_history(&run_id).map_err(|e| e.to_string())
}

/// Save a DAG definition as a reusable template.
#[tauri::command]
pub async fn dag_template_save(
    name: String,
    description: Option<String>,
    tasks: Vec<DagTaskDefinition>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let def = DagDefinition {
        id: id.clone(),
        name,
        description,
        tasks,
        is_template: true,
        created_at: now,
        updated_at: now,
    };

    let store = state
        .dag_store
        .lock()
        .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
    store.save_definition(&def).map_err(|e| e.to_string())?;

    info!("[DAG_CMD] Saved template '{}'", id);
    Ok(id)
}

/// List saved DAG templates.
#[tauri::command]
pub async fn dag_template_list(state: State<'_, AppState>) -> Result<Vec<DagDefinition>, String> {
    let store = state
        .dag_store
        .lock()
        .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
    store.list_templates().map_err(|e| e.to_string())
}

/// Delete a saved template.
#[tauri::command]
pub async fn dag_template_delete(
    template_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let store = state
        .dag_store
        .lock()
        .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
    store
        .delete_definition(&template_id)
        .map_err(|e| e.to_string())?;
    info!("[DAG_CMD] Deleted template '{}'", template_id);
    Ok(())
}
