//! DAG execution engine — runs tasks in dependency order with retries.

use std::sync::Arc;
use std::time::Duration;

use std::sync::Mutex;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::commands::AppState;
use crate::subagent_executor::SubagentExecutor;

use super::store::DagStore;
use super::{DagRunStatus, DagTaskDefinition, DagTaskStatus};

/// DAG executor — manages background execution of DAG runs.
pub struct DagExecutor {
    store: Arc<Mutex<DagStore>>,
    subagent_executor: Arc<RwLock<SubagentExecutor>>,
}

impl DagExecutor {
    pub fn new(
        store: Arc<Mutex<DagStore>>,
        subagent_executor: Arc<RwLock<SubagentExecutor>>,
    ) -> Self {
        Self {
            store,
            subagent_executor,
        }
    }

    /// Start executing a DAG run in the background.
    /// Returns immediately; execution proceeds asynchronously.
    pub fn start_run(&self, run_id: String, state: AppState, app_handle: tauri::AppHandle) {
        let store = self.store.clone();
        let executor = self.subagent_executor.clone();

        tokio::spawn(async move {
            if let Err(e) = execution_loop(&store, &executor, &run_id, &state, &app_handle).await {
                error!("[DAG_EXECUTOR] Run '{}' failed: {}", run_id, e);
                {
                    let s = store.lock().unwrap_or_else(|e| e.into_inner());
                    let _ =
                        s.update_run_status(&run_id, &DagRunStatus::Failed, Some(&e.to_string()));
                    let _ = s.add_history(&run_id, None, "run_failed", Some(&e.to_string()));
                } // MutexGuard dropped before await
                emit_event(
                    &app_handle,
                    "dag:run-completed",
                    &serde_json::json!({
                        "run_id": run_id,
                        "status": "failed",
                        "error": e.to_string(),
                    }),
                );
                let notify_msg = format!(
                    "❌ DAG run `{}` failed: {}",
                    &run_id[..8.min(run_id.len())],
                    e
                );
                let attempted = state.notification_dispatcher.broadcast(&notify_msg).await;
                if attempted {
                    // Mark as notified so heartbeat catch-up skips it.
                    let s = store.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = s.mark_notification_sent(&run_id);
                } else {
                    warn!(
                        "[DAG_EXECUTOR] No notification delivery targets available for failed run {}; leaving notification unsent",
                        run_id
                    );
                }
            }
        });
    }

    /// Add a task to a running DAG.
    pub async fn add_task_to_run(
        &self,
        run_id: &str,
        task_def: &DagTaskDefinition,
        app_handle: &tauri::AppHandle,
    ) -> Result<String, String> {
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let task_id = store
            .add_task_to_run(run_id, task_def)
            .map_err(|e| e.to_string())?;

        emit_event(
            app_handle,
            "dag:task-added",
            &serde_json::json!({
                "run_id": run_id,
                "task_id": task_id,
                "task_key": task_def.key,
            }),
        );

        Ok(task_id)
    }

    /// Cancel a running DAG.
    pub async fn cancel_run(
        &self,
        run_id: &str,
        app_handle: &tauri::AppHandle,
    ) -> Result<(), String> {
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        store
            .cancel_pending_tasks(run_id)
            .map_err(|e| e.to_string())?;
        store
            .update_run_status(run_id, &DagRunStatus::Cancelled, None)
            .map_err(|e| e.to_string())?;
        store
            .add_history(run_id, None, "run_cancelled", None)
            .map_err(|e| e.to_string())?;

        emit_event(
            app_handle,
            "dag:run-completed",
            &serde_json::json!({
                "run_id": run_id,
                "status": "cancelled",
            }),
        );

        Ok(())
    }
}

/// Main execution loop for a DAG run.
async fn execution_loop(
    store: &Arc<Mutex<DagStore>>,
    executor: &Arc<RwLock<SubagentExecutor>>,
    run_id: &str,
    state: &AppState,
    app_handle: &tauri::AppHandle,
) -> Result<(), anyhow::Error> {
    let start = std::time::Instant::now();

    // Mark run as running
    {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.update_run_status(run_id, &DagRunStatus::Running, None)?;
        s.add_history(run_id, None, "run_started", None)?;
    }

    let task_count = {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.get_tasks_for_run(run_id)?.len()
    };

    emit_event(
        app_handle,
        "dag:run-started",
        &serde_json::json!({
            "run_id": run_id,
            "task_count": task_count,
        }),
    );

    loop {
        // Get runnable tasks
        let runnable = {
            let s = store.lock().unwrap_or_else(|e| e.into_inner());
            s.get_runnable_tasks(run_id)?
        };

        if runnable.is_empty() {
            // Check if we're done or deadlocked
            let all_terminal = {
                let s = store.lock().unwrap_or_else(|e| e.into_inner());
                s.all_tasks_terminal(run_id)?
            };
            if all_terminal {
                break;
            }

            // Check for running tasks (might complete and unblock others)
            let tasks = {
                let s = store.lock().unwrap_or_else(|e| e.into_inner());
                s.get_tasks_for_run(run_id)?
            };
            // A task in Retrying status is sleeping before its next attempt — it is
            // not Running yet, but it will transition back to Pending shortly.  Treat
            // Retrying the same as Running for the deadlock check so we don't false-fire.
            let has_running_or_retrying = tasks
                .iter()
                .any(|t| t.status == DagTaskStatus::Running || t.status == DagTaskStatus::Retrying);
            if !has_running_or_retrying {
                // Deadlock: no runnable, no running/retrying, but not all terminal
                return Err(anyhow::anyhow!(
                    "DAG deadlock: no runnable or running tasks, but {} tasks remain non-terminal",
                    tasks
                        .iter()
                        .filter(|t| !matches!(
                            t.status,
                            DagTaskStatus::Completed
                                | DagTaskStatus::Failed
                                | DagTaskStatus::Cancelled
                        ))
                        .count()
                ));
            }
            // Wait for running tasks to complete
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        // Execute runnable tasks in parallel
        let mut handles = Vec::new();

        for task in runnable {
            // Wait for executor capacity
            while !executor.read().await.can_accept() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }

            // Collect dependency results for context
            let dep_context = {
                let s = store.lock().unwrap_or_else(|e| e.into_inner());
                let all_tasks = s.get_tasks_for_run(run_id)?;
                let mut context = String::new();
                for dep_key in &task.depends_on {
                    if let Some(dep) = all_tasks.iter().find(|t| t.task_key == *dep_key) {
                        if let Some(ref output) = dep.output {
                            let preview = if output.len() > 2000 {
                                let b = output
                                    .char_indices()
                                    .take_while(|(i, _)| *i < 2000)
                                    .last()
                                    .map(|(i, c)| i + c.len_utf8())
                                    .unwrap_or(0);
                                format!("{}...[truncated]", &output[..b])
                            } else {
                                output.clone()
                            };
                            context.push_str(&format!("- {}: {}\n", dep_key, preview));
                        }
                    }
                }
                context
            };

            // Build task description with dependency context
            let full_description = if dep_context.is_empty() {
                task.description.clone()
            } else {
                format!(
                    "Context from completed dependencies:\n{}\nYour task: {}",
                    dep_context, task.description
                )
            };

            // Mark task as running
            {
                let s = store.lock().unwrap_or_else(|e| e.into_inner());
                s.update_task_status(&task.id, &DagTaskStatus::Running, None, None)?;
                s.add_history(run_id, Some(&task.id), "task_started", Some(&task.task_key))?;
            }

            emit_event(
                app_handle,
                "dag:task-started",
                &serde_json::json!({
                    "run_id": run_id,
                    "task_id": task.id,
                    "task_key": task.task_key,
                    "agent_id": task.agent_id,
                }),
            );

            // Get agent config
            let agent_config = {
                let mgr = state.agent_manager.read().await;
                mgr.get_agent(&task.agent_id)
                    .map(|a| a.config.clone())
                    .or_else(|| mgr.default_agent().map(|a| a.config.clone()))
            };

            let Some(agent_config) = agent_config else {
                let err_msg = format!("Agent '{}' not found", task.agent_id);
                let s = store.lock().unwrap_or_else(|e| e.into_inner());
                s.update_task_status(&task.id, &DagTaskStatus::Failed, None, Some(&err_msg))?;
                s.add_history(run_id, Some(&task.id), "task_failed", Some(&err_msg))?;
                emit_event(
                    app_handle,
                    "dag:task-failed",
                    &serde_json::json!({
                        "run_id": run_id,
                        "task_id": task.id,
                        "task_key": task.task_key,
                        "error": err_msg,
                        "will_retry": false,
                    }),
                );
                continue;
            };

            let store_clone = store.clone();
            let state_clone = state.clone();
            let app_handle_clone = app_handle.clone();
            let run_id_owned = run_id.to_string();
            let task_id = task.id.clone();
            let task_key = task.task_key.clone();
            let max_retries = task.max_retries;
            let retry_delay_ms = task.retry_delay_ms;
            let attempt = task.attempt;

            const DAG_TASK_TIMEOUT_SECS: u64 = 600; // 10 minutes per task
            let executor_clone = executor.clone();
            let handle = tokio::spawn(async move {
                let exec = executor_clone.read().await;
                let session_key = format!("dag_{}", run_id_owned);
                let session_id = format!("dag_{}", &run_id_owned[..8]);
                let exec_future = exec.execute(
                    &agent_config,
                    &full_description,
                    &session_key,
                    &state_clone,
                    Some(&session_id),
                );
                let timeout_result = tokio::time::timeout(
                    Duration::from_secs(DAG_TASK_TIMEOUT_SECS),
                    exec_future,
                ).await;
                drop(exec);
                let result = match timeout_result {
                    Ok(r) => r,
                    Err(_) => {
                        let timeout_msg = format!("Task timed out after {}s", DAG_TASK_TIMEOUT_SECS);
                        warn!("[DAG_EXECUTOR] Task '{}' {}", task_key, timeout_msg);
                        {
                            let s = store_clone.lock().unwrap_or_else(|e| e.into_inner());
                            let _ = s.update_task_status(&task_id, &DagTaskStatus::Failed, None, Some(&timeout_msg));
                            let _ = s.add_history(&run_id_owned, Some(&task_id), "task_failed", Some(&timeout_msg));
                        }
                        emit_event(&app_handle_clone, "dag:task-failed", &serde_json::json!({ "run_id": run_id_owned, "task_id": task_id, "task_key": task_key, "error": timeout_msg, "will_retry": false }));
                        return;
                    }
                };

                if result.success {
                    let output_preview = if result.output.len() > 200 {
                        let b = result.output
                            .char_indices()
                            .take_while(|(i, _)| *i < 200)
                            .last()
                            .map(|(i, c)| i + c.len_utf8())
                            .unwrap_or(0);
                        format!("{}...", &result.output[..b])
                    } else {
                        result.output.clone()
                    };
                    {
                        let s = store_clone.lock().unwrap_or_else(|e| e.into_inner());
                        let _ = s.update_task_status(
                            &task_id,
                            &DagTaskStatus::Completed,
                            Some(&result.output),
                            None,
                        );
                        let _ = s.add_history(
                            &run_id_owned,
                            Some(&task_id),
                            "task_completed",
                            Some(&output_preview),
                        );
                    } // MutexGuard dropped before await

                    // Write result to shared workspace
                    {
                        let mut ws = state_clone.shared_workspace.write().await;
                        let _ = ws.put(
                            &format!("dag_{}", &run_id_owned[..8]),
                            &format!("result/{}", task_key),
                            serde_json::Value::String(result.output.clone()),
                            "dag_executor",
                            None,
                        );
                    }

                    emit_event(
                        &app_handle_clone,
                        "dag:task-completed",
                        &serde_json::json!({
                            "run_id": run_id_owned,
                            "task_id": task_id,
                            "task_key": task_key,
                            "output_preview": output_preview,
                        }),
                    );

                    info!(
                        "[DAG_EXECUTOR] Task '{}' completed ({} chars, {}ms)",
                        task_key,
                        result.output.len(),
                        result.elapsed_ms
                    );
                } else {
                    let err_msg = result.error.unwrap_or_else(|| "Unknown error".to_string());

                    // Check retry policy.
                    // `attempt` was already incremented to the current attempt number by
                    // update_task_status(Running), so use it directly — do not add 1 again.
                    let current_attempt = attempt;
                    if current_attempt <= max_retries {
                        let delay = retry_delay_ms * 2u64.pow(current_attempt.saturating_sub(1));
                        warn!(
                            "[DAG_EXECUTOR] Task '{}' failed (attempt {}/{}), retrying in {}ms: {}",
                            task_key, current_attempt, max_retries, delay, err_msg
                        );

                        {
                            let s = store_clone.lock().unwrap_or_else(|e| e.into_inner());
                            let _ = s.update_task_status(
                                &task_id,
                                &DagTaskStatus::Retrying,
                                None,
                                None,
                            );
                            let _ = s.add_history(
                                &run_id_owned,
                                Some(&task_id),
                                "task_retrying",
                                Some(&format!(
                                    "attempt {}/{}: {}",
                                    current_attempt, max_retries, err_msg
                                )),
                            );
                        } // MutexGuard dropped before await

                        emit_event(
                            &app_handle_clone,
                            "dag:task-retrying",
                            &serde_json::json!({
                                "run_id": run_id_owned,
                                "task_id": task_id,
                                "task_key": task_key,
                                "attempt": current_attempt,
                                "max_retries": max_retries,
                                "delay_ms": delay,
                            }),
                        );

                        // Wait and reset to pending for next iteration
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        {
                            let s = store_clone.lock().unwrap_or_else(|e| e.into_inner());
                            let _ =
                                s.update_task_status(&task_id, &DagTaskStatus::Pending, None, None);
                        }
                    } else {
                        error!(
                            "[DAG_EXECUTOR] Task '{}' failed permanently: {}",
                            task_key, err_msg
                        );

                        {
                            let s = store_clone.lock().unwrap_or_else(|e| e.into_inner());
                            let _ = s.update_task_status(
                                &task_id,
                                &DagTaskStatus::Failed,
                                None,
                                Some(&err_msg),
                            );
                            let _ = s.add_history(
                                &run_id_owned,
                                Some(&task_id),
                                "task_failed",
                                Some(&err_msg),
                            );
                        }

                        emit_event(
                            &app_handle_clone,
                            "dag:task-failed",
                            &serde_json::json!({
                                "run_id": run_id_owned,
                                "task_id": task_id,
                                "task_key": task_key,
                                "error": err_msg,
                                "will_retry": false,
                            }),
                        );
                    }
                }
            });

            handles.push(handle);
        }

        // Wait for all tasks in this round to complete
        for handle in handles {
            if let Err(e) = handle.await {
                error!("[DAG] Task thread panicked during round execution: {}", e);
            }
        }

        // Emit progress
        let (completed, total, running, failed) = {
            let s = store.lock().unwrap_or_else(|e| e.into_inner());
            let tasks = s.get_tasks_for_run(run_id)?;
            let completed = tasks
                .iter()
                .filter(|t| t.status == DagTaskStatus::Completed)
                .count();
            let total = tasks.len();
            let running = tasks
                .iter()
                .filter(|t| t.status == DagTaskStatus::Running)
                .count();
            let failed = tasks
                .iter()
                .filter(|t| t.status == DagTaskStatus::Failed)
                .count();
            (completed, total, running, failed)
        };

        emit_event(
            app_handle,
            "dag:progress",
            &serde_json::json!({
                "run_id": run_id,
                "completed": completed,
                "total": total,
                "running": running,
                "failed": failed,
            }),
        );
    }

    // Determine final status
    let has_failures = {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.has_failed_tasks(run_id)?
    };

    let final_status = if has_failures {
        DagRunStatus::Failed
    } else {
        DagRunStatus::Completed
    };

    {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.update_run_status(run_id, &final_status, None)?;
        s.add_history(
            run_id,
            None,
            "run_completed",
            Some(&format!("status={}", final_status)),
        )?;
    }

    let elapsed_ms = start.elapsed().as_millis();
    emit_event(
        app_handle,
        "dag:run-completed",
        &serde_json::json!({
            "run_id": run_id,
            "status": final_status.to_string(),
            "total_elapsed_ms": elapsed_ms,
        }),
    );

    info!(
        "[DAG_EXECUTOR] Run '{}' completed with status '{}' in {}ms",
        run_id, final_status, elapsed_ms
    );

    // Notify all configured channels that this DAG run finished.
    let notify_msg = if has_failures {
        format!(
            "❌ DAG run `{}` finished with failures after {}ms",
            &run_id[..8.min(run_id.len())],
            elapsed_ms
        )
    } else {
        format!(
            "✅ DAG run `{}` completed successfully in {}ms",
            &run_id[..8.min(run_id.len())],
            elapsed_ms
        )
    };
    let attempted = state.notification_dispatcher.broadcast(&notify_msg).await;
    if attempted {
        // Mark the run's notification as sent so the heartbeat catch-up scan skips it.
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        let _ = s.mark_notification_sent(run_id);
    } else {
        warn!(
            "[DAG_EXECUTOR] No notification delivery targets available for completed run {}; leaving notification unsent",
            run_id
        );
    }

    Ok(())
}

/// Emit a Tauri event (fire-and-forget).
fn emit_event(app_handle: &tauri::AppHandle, event: &str, payload: &serde_json::Value) {
    use tauri::Emitter;
    if let Err(e) = app_handle.emit(event, payload.clone()) {
        warn!(
            "[DAG_EXECUTOR] Failed to emit '{}' event to UI: {}",
            event, e
        );
    }
}
