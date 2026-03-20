//! Scheduler Tauri commands

use tauri::State;
use tracing::info;

use crate::config::ScheduledTask;
use crate::scheduler::TaskExecutionResult;

use super::AppState;

/// List all scheduled tasks.
#[tauri::command]
pub async fn list_scheduled_tasks(
    state: State<'_, AppState>,
) -> Result<Vec<ScheduledTask>, String> {
    let config = state.config.read().await;
    Ok(config.scheduled_tasks.tasks.clone())
}

/// Add a scheduled task.
#[tauri::command]
pub async fn add_scheduled_task(
    state: State<'_, AppState>,
    name: String,
    schedule: String,
    prompt: String,
) -> Result<ScheduledTask, String> {
    let task = ScheduledTask {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.clone(),
        schedule,
        prompt,
        enabled: true,
        run_if_missed: false,
        last_run: None,
    };

    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.scheduled_tasks.tasks.push(task.clone());
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());

    info!("[SCHEDULER] Added task '{}': {}", name, task.id);
    Ok(task)
}

/// Remove a scheduled task by ID.
#[tauri::command]
pub async fn remove_scheduled_task(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    let before = config.scheduled_tasks.tasks.len();
    config.scheduled_tasks.tasks.retain(|t| t.id != task_id);

    if config.scheduled_tasks.tasks.len() == before {
        return Err(format!("Task not found: {}", task_id));
    }

    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[SCHEDULER] Removed task: {}", task_id);
    Ok(())
}

/// Update a scheduled task (partial update).
#[tauri::command]
pub async fn update_scheduled_task(
    state: State<'_, AppState>,
    task_id: String,
    name: Option<String>,
    schedule: Option<String>,
    prompt: Option<String>,
    enabled: Option<bool>,
    run_if_missed: Option<bool>,
) -> Result<ScheduledTask, String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    let task = config
        .scheduled_tasks
        .tasks
        .iter_mut()
        .find(|t| t.id == task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;

    if let Some(n) = name {
        task.name = n;
    }
    if let Some(s) = schedule {
        task.schedule = s;
    }
    if let Some(p) = prompt {
        task.prompt = p;
    }
    if let Some(e) = enabled {
        task.enabled = e;
    }
    if let Some(r) = run_if_missed {
        task.run_if_missed = r;
    }

    let updated = task.clone();
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[SCHEDULER] Updated task: {}", task_id);
    Ok(updated)
}

/// Get recent scheduler execution results.
#[tauri::command]
pub async fn get_scheduler_results(
    state: State<'_, AppState>,
) -> Result<Vec<TaskExecutionResult>, String> {
    Ok(state.scheduler.get_results().await)
}

/// Trigger a scheduled task immediately.
#[tauri::command]
pub async fn trigger_scheduled_task(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<TaskExecutionResult, String> {
    state.scheduler.execute_task(&task_id).await
}

/// Get scheduler enabled status.
#[tauri::command]
pub async fn get_scheduler_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state.config.read().await;
    Ok(config.scheduled_tasks.enabled)
}

/// Set scheduler enabled status.
#[tauri::command]
pub async fn set_scheduler_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.scheduled_tasks.enabled = enabled;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[SCHEDULER] Scheduler enabled: {}", enabled);
    Ok(())
}
