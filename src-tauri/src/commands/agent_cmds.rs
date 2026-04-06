//! Tauri commands for multi-agent management.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::{error, info, warn};

use super::AppState;
use crate::agent::AgentInfo;
use crate::orchestration::SubagentResult;

/// List all configured agents.
#[tauri::command]
pub async fn list_agents(state: State<'_, AppState>) -> Result<Vec<AgentInfo>, String> {
    let mgr = state.agent_manager.read().await;
    Ok(mgr.list_agents())
}

/// Get info about a specific agent.
#[tauri::command]
pub async fn get_agent(agent_id: String, state: State<'_, AppState>) -> Result<AgentInfo, String> {
    let mgr = state.agent_manager.read().await;
    mgr.get_agent(&agent_id)
        .map(|a| AgentInfo {
            id: a.config.id.clone(),
            name: a.config.name.clone(),
            avatar: a.config.avatar.clone(),
            model: a.config.model.clone(),
            is_default: a.config.is_default,
            channel_bindings: a.config.channel_bindings.clone(),
        })
        .ok_or_else(|| format!("Agent not found: {}", agent_id))
}

/// Get the currently active GUI agent ID.
#[tauri::command]
pub async fn get_active_gui_agent(state: State<'_, AppState>) -> Result<String, String> {
    let mgr = state.agent_manager.read().await;
    Ok(mgr.active_gui_agent_id.clone())
}

/// Set the active GUI agent.
#[tauri::command]
pub async fn set_active_gui_agent(
    agent_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut mgr = state.agent_manager.write().await;
    if mgr.get_agent(&agent_id).is_none() {
        return Err(format!("Agent not found: {}", agent_id));
    }
    mgr.active_gui_agent_id = agent_id.clone();
    info!("[AGENT] Active GUI agent set to '{}'", agent_id);
    Ok(())
}

/// Request for the nexibot_orchestrate Tauri command.
#[derive(Debug, Deserialize)]
pub struct OrchestrationRequest {
    pub subtasks: Vec<OrchestrationSubtask>,
}

/// A single subtask in an orchestration request.
#[derive(Debug, Clone, Deserialize)]
pub struct OrchestrationSubtask {
    pub id: String,
    pub agent: String,
    pub task: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Response from the orchestration command.
#[derive(Debug, Serialize)]
pub struct OrchestrationResponse {
    pub results: Vec<SubtaskResult>,
    pub workspace_id: String,
    pub total_elapsed_ms: u64,
}

/// Result of a single subtask.
#[derive(Debug, Serialize)]
pub struct SubtaskResult {
    pub id: String,
    pub agent: String,
    pub output: String,
    pub success: bool,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}

/// Trigger a multi-agent orchestration from the frontend.
#[tauri::command]
pub async fn nexibot_orchestrate(
    request: OrchestrationRequest,
    state: State<'_, AppState>,
) -> Result<OrchestrationResponse, String> {
    info!(
        "[ORCHESTRATE] Received orchestration request with {} subtasks",
        request.subtasks.len()
    );

    let start = std::time::Instant::now();
    let workspace_id = uuid::Uuid::new_v4().to_string();

    // Clone state to break the non-Send State<'_> borrow
    let state_owned = state.inner().clone();
    let results = execute_orchestration(&request.subtasks, &workspace_id, &state_owned).await;

    Ok(OrchestrationResponse {
        results,
        workspace_id,
        total_elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

/// Wrapper to make a non-Send future usable with `tokio::spawn`.
/// SAFETY: The wrapped future must not hold non-Send references across await points.
/// In our case, `execute_single_subtask` only uses owned `AppState` (which is Clone/Send)
/// and owned `String` values, so this is safe.
struct SendFuture<F>(F);
// SAFETY: execute_single_subtask uses only owned data (cloned AppState, owned Strings).
// It never holds a non-Send reference like &Window across an await point.
unsafe impl<F: std::future::Future> Send for SendFuture<F> {}
impl<F: std::future::Future> std::future::Future for SendFuture<F> {
    type Output = F::Output;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // SAFETY: Pin projection is safe — SendFuture is a transparent wrapper.
        let inner = unsafe { self.map_unchecked_mut(|s| &mut s.0) };
        inner.poll(cx)
    }
}

/// Execute orchestration subtasks respecting dependencies.
/// Independent subtasks run in parallel; dependent ones wait.
pub(crate) async fn execute_orchestration(
    subtasks: &[OrchestrationSubtask],
    workspace_id: &str,
    state: &AppState,
) -> Vec<SubtaskResult> {
    let mut results: Vec<SubtaskResult> = Vec::new();
    let mut completed_ids: HashMap<String, SubtaskResult> = HashMap::new();

    // Simple topological execution: iterate rounds until all done.
    let mut remaining: Vec<OrchestrationSubtask> = subtasks.to_vec();
    let max_rounds = subtasks.len() + 1; // Prevent infinite loops

    for round in 0..max_rounds {
        if remaining.is_empty() {
            break;
        }

        // Find subtasks whose dependencies are all satisfied.
        let (ready, not_ready): (Vec<_>, Vec<_>) = remaining.into_iter().partition(|st| {
            st.depends_on
                .iter()
                .all(|dep| completed_ids.contains_key(dep))
        });

        remaining = not_ready;

        if ready.is_empty() {
            // Circular dependency or missing dependency — fail remaining.
            warn!(
                "[ORCHESTRATE] Round {}: no runnable subtasks, {} remaining (possible circular dependency)",
                round,
                remaining.len()
            );
            for st in &remaining {
                let result = SubtaskResult {
                    id: st.id.clone(),
                    agent: st.agent.clone(),
                    output: String::new(),
                    success: false,
                    elapsed_ms: 0,
                    error: Some(
                        "Dependency deadlock: required subtasks never completed".to_string(),
                    ),
                };
                completed_ids.insert(st.id.clone(), result);
            }
            break;
        }

        info!(
            "[ORCHESTRATE] Round {}: executing {} parallel subtasks",
            round,
            ready.len()
        );

        // Execute ready subtasks in parallel using SendFuture.
        let mut handles = Vec::new();
        for subtask in ready {
            let state_clone = state.clone();
            let ws_id = workspace_id.to_string();

            handles.push(tokio::spawn(SendFuture(async move {
                execute_single_subtask(&subtask, &ws_id, &state_clone).await
            })));
        }

        // Collect results.
        for handle in handles {
            match handle.await {
                Ok(result) => {
                    completed_ids.insert(result.id.clone(), result);
                }
                Err(e) => {
                    error!("[ORCHESTRATE] Subtask join error: {}", e);
                }
            }
        }
    }

    // Build final results in original order.
    // Subtasks that panicked or were cancelled get an explicit failure entry so
    // callers can detect partial failures rather than receiving a silently-shorter list.
    for st in subtasks {
        if let Some(result) = completed_ids.remove(&st.id) {
            results.push(result);
        } else {
            results.push(SubtaskResult {
                id: st.id.clone(),
                agent: st.agent.clone(),
                output: String::new(),
                success: false,
                elapsed_ms: 0,
                error: Some("Subtask did not return a result (task panicked or was cancelled)".to_string()),
            });
        }
    }

    // Clean up workspace for this orchestration.
    {
        let mut ws = state.shared_workspace.write().await;
        ws.clear_orchestration(workspace_id);
    }

    results
}

/// Execute a single subtask using the SubagentExecutor.
async fn execute_single_subtask(
    subtask: &OrchestrationSubtask,
    workspace_id: &str,
    state: &AppState,
) -> SubtaskResult {
    // Check circuit breaker
    {
        let mut cb = state.circuit_breaker.write().await;
        if !cb.allow_call(&subtask.agent) {
            return SubtaskResult {
                id: subtask.id.clone(),
                agent: subtask.agent.clone(),
                output: String::new(),
                success: false,
                elapsed_ms: 0,
                error: Some(format!(
                    "Circuit breaker open for agent '{}' — too many recent failures",
                    subtask.agent
                )),
            };
        }
    }

    // Look up the agent config.
    let agent_config = {
        let mgr = state.agent_manager.read().await;
        mgr.get_agent(&subtask.agent).map(|a| a.config.clone())
    };

    let agent_config = match agent_config {
        Some(c) => c,
        None => {
            return SubtaskResult {
                id: subtask.id.clone(),
                agent: subtask.agent.clone(),
                output: String::new(),
                success: false,
                elapsed_ms: 0,
                error: Some(format!("Agent not found: {}", subtask.agent)),
            };
        }
    };

    // Register spawn in orchestration manager.
    let spawn_id = {
        let mut orch = state.orchestration_manager.write().await;
        match orch.spawn_subagent(None, &subtask.agent, &subtask.task, None) {
            Ok(id) => {
                let _ = orch.mark_running(&id);
                id
            }
            Err(e) => {
                return SubtaskResult {
                    id: subtask.id.clone(),
                    agent: subtask.agent.clone(),
                    output: String::new(),
                    success: false,
                    elapsed_ms: 0,
                    error: Some(format!("Failed to spawn: {}", e)),
                };
            }
        }
    };

    // Execute via SubagentExecutor.
    let executor = state.subagent_executor.read().await;
    let exec_result = executor
        .execute(
            &agent_config,
            &subtask.task,
            "orchestration",
            state,
            Some(workspace_id),
        )
        .await;

    // Update orchestration manager and circuit breaker.
    // Capture worktree info before marking the spawn complete so we can clean up
    // after the lock is released (worktree removal is async and must not hold the lock).
    let worktree_cleanup = {
        let mut orch = state.orchestration_manager.write().await;
        // Grab worktree path/branch while the spawn is still in active_spawns.
        let wt_info = orch.get_spawn(&spawn_id).and_then(|s| {
            s.worktree_path.as_ref().and_then(|p| {
                s.worktree_branch.as_ref().map(|b| (p.clone(), b.clone()))
            })
        });
        if exec_result.success {
            let _ = orch.mark_completed(
                &spawn_id,
                SubagentResult {
                    text: exec_result.output.clone(),
                    tool_calls_made: exec_result.tool_calls_made,
                    model_used: exec_result.model_used.clone(),
                    duration_ms: exec_result.elapsed_ms,
                },
            );
        } else {
            let _ = orch.mark_failed(
                &spawn_id,
                exec_result.error.as_deref().unwrap_or("Unknown error"),
            );
        }
        wt_info
    };

    // Remove the agent's worktree (if any) now that execution is complete.
    if let Some((wt_path, wt_branch)) = worktree_cleanup {
        if let Some(git_root) = crate::tools::worktree::find_git_root(&wt_path).await {
            crate::tools::worktree::remove_agent_worktree(&git_root, &wt_path, &wt_branch).await;
        } else {
            warn!("[AGENT] Could not find git root for worktree cleanup: {}", wt_path.display());
        }
    }

    // Update circuit breaker.
    {
        let mut cb = state.circuit_breaker.write().await;
        if exec_result.success {
            cb.record_success(&subtask.agent);
        } else {
            cb.record_failure(&subtask.agent);
        }
    }

    // Write result to shared workspace.
    {
        let mut ws = state.shared_workspace.write().await;
        let _ = ws.put(
            workspace_id,
            &format!("result/{}", subtask.id),
            serde_json::json!({
                "output": exec_result.output,
                "success": exec_result.success,
            }),
            &subtask.agent,
            None,
        );
    }

    SubtaskResult {
        id: subtask.id.clone(),
        agent: subtask.agent.clone(),
        output: exec_result.output,
        success: exec_result.success,
        elapsed_ms: exec_result.elapsed_ms,
        error: exec_result.error,
    }
}

/// Execute the `nexibot_orchestrate` tool call (called from the tool loop).
pub(crate) async fn execute_orchestrate_tool(
    input: &serde_json::Value,
    state: &AppState,
) -> String {
    let subtasks_val = match input.get("subtasks") {
        Some(v) => v,
        None => return r#"{"error": "Missing required field: subtasks"}"#.to_string(),
    };

    let subtasks: Vec<OrchestrationSubtask> = match serde_json::from_value(subtasks_val.clone()) {
        Ok(s) => s,
        Err(e) => return format!(r#"{{"error": "Invalid subtasks format: {}"}}"#, e),
    };

    if subtasks.is_empty() {
        return r#"{"error": "subtasks array must not be empty"}"#.to_string();
    }

    let workspace_id = uuid::Uuid::new_v4().to_string();
    let results = execute_orchestration(&subtasks, &workspace_id, state).await;

    serde_json::json!({
        "workspace_id": workspace_id,
        "results": results.iter().map(|r| serde_json::json!({
            "id": r.id,
            "agent": r.agent,
            "output": r.output,
            "success": r.success,
            "elapsed_ms": r.elapsed_ms,
            "error": r.error,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}
