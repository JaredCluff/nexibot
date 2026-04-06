//! Tauri commands for autonomous agent management.
//!
//! Exposes run_agent, plan_agent, get_agent_run_status, list_agent_defs, and save_agent_def.
//! Agent definitions (WorkflowSpecs) are stored as DAG templates with a special marker in the
//! description field so they appear in both the DAG template list and the agent list.

use std::collections::HashMap;
use std::sync::Arc;

use tauri::State;
use tracing::{error, info};
use uuid::Uuid;

use super::AppState;
use crate::agent_engine::capability_dispatch::{LocalCapabilityDispatch, is_local_capability};
use crate::agent_engine::planner::AgentPlanner;
use crate::agent_engine::workflow_executor::WorkflowExecutor;
use crate::agent_engine::workflow_spec::{AgentRunStatus, AgentSummary, WorkflowSpec};

// ── Active run registry ───────────────────────────────────────────────────────

/// Maximum number of terminal (completed/failed/cancelled) runs retained.
const MAX_FINISHED_AGENT_RUNS: usize = 500;

/// In-memory registry of active and recently completed agent runs.
/// Stored in AppState as `agent_run_registry`.
pub struct AgentRunRegistry {
    runs: HashMap<String, AgentRunStatus>,
}

impl AgentRunRegistry {
    pub fn new() -> Self {
        Self {
            runs: HashMap::new(),
        }
    }

    pub fn insert(&mut self, run: AgentRunStatus) {
        self.runs.insert(run.run_id.clone(), run);
        self.evict_finished();
    }

    pub fn get(&self, run_id: &str) -> Option<&AgentRunStatus> {
        self.runs.get(run_id)
    }

    pub fn update(&mut self, run: AgentRunStatus) {
        self.runs.insert(run.run_id.clone(), run);
        self.evict_finished();
    }

    /// Evict oldest terminal runs when over the cap.
    fn evict_finished(&mut self) {
        let terminal = ["completed", "failed", "cancelled"];
        let finished_count = self.runs.values()
            .filter(|r| terminal.contains(&r.status.as_str()))
            .count();
        if finished_count > MAX_FINISHED_AGENT_RUNS {
            let mut finished: Vec<_> = self.runs.iter()
                .filter(|(_, r)| terminal.contains(&r.status.as_str()))
                .map(|(k, r)| (k.clone(), r.started_at.unwrap_or(0)))
                .collect();
            finished.sort_by_key(|(_, ts)| *ts);
            let evict_count = finished_count - MAX_FINISHED_AGENT_RUNS;
            for (id, _) in finished.into_iter().take(evict_count) {
                self.runs.remove(&id);
            }
        }
    }
}

// ── Spec-is-local check ───────────────────────────────────────────────────────

/// Returns `true` if every step in the spec can be handled locally.
fn spec_is_local(spec: &WorkflowSpec) -> bool {
    spec.steps
        .iter()
        .all(|s| is_local_capability(&s.capability))
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Run a workflow spec locally or delegate to the backend agent-engine.
///
/// - `spec_json` — a serialised `WorkflowSpec`.  Pass `null` / empty string to
///   use `goal` for LLM-based planning instead.
/// - `goal` — natural language goal (triggers the LLM planner when `spec_json` is absent).
/// - `inputs` — input variables injected into the execution context.
/// - `run_local` — force local execution even for remote capabilities.
///
/// Returns a `run_id` that can be polled via `get_agent_run_status`.
#[tauri::command]
pub async fn run_agent(
    spec_json: Option<String>,
    goal: Option<String>,
    inputs: Option<serde_json::Value>,
    run_local: Option<bool>,
    state: State<'_, AppState>,
    _app_handle: tauri::AppHandle,
) -> Result<String, String> {
    // 1. Obtain a WorkflowSpec
    let mut spec: WorkflowSpec = if let Some(ref json) = spec_json {
        if json.trim().is_empty() || json.trim() == "null" {
            plan_from_goal(goal, &state).await?
        } else {
            serde_json::from_str(json).map_err(|e| format!("Invalid spec_json: {}", e))?
        }
    } else {
        plan_from_goal(goal, &state).await?
    };

    // 2. Merge provided inputs into spec.inputs
    if let Some(extra) = inputs {
        if let serde_json::Value::Object(map) = extra {
            for (k, v) in map {
                spec.inputs.insert(k, v);
            }
        }
    }

    let run_id = Uuid::new_v4().to_string();
    // Sanitize spec.name against log injection (strip newlines/carriage returns)
    let safe_spec_name: String = spec.name.chars().filter(|&c| c != '\n' && c != '\r').collect();
    info!(
        "[AGENT_CMD] Starting run '{}' for spec '{}' ({} steps)",
        run_id, safe_spec_name, spec.steps.len()
    );

    // 3. Decide local vs remote execution
    let force_local = run_local.unwrap_or(false);
    let use_local = force_local || spec_is_local(&spec);

    if use_local {
        // Local execution via WorkflowExecutor
        let dispatch = build_dispatch(&state).await;
        let executor = WorkflowExecutor::new(Arc::new(dispatch));
        let run_id_clone = run_id.clone();
        let spec_clone = spec.clone();
        let state_inner = state.inner().clone();

        // Register a pending run
        {
            let mut reg = state.agent_run_registry.write().await;
            reg.insert(AgentRunStatus {
                run_id: run_id.clone(),
                status: "running".to_string(),
                step_results: HashMap::new(),
                started_at: Some(chrono::Utc::now().timestamp_millis()),
                completed_at: None,
                error: None,
            });
        }

        tokio::spawn(async move {
            let result = executor
                .execute(&run_id_clone, &spec_clone)
                .await;

            // Store final status
            let mut reg = state_inner.agent_run_registry.write().await;
            reg.update(result);
        });
    } else {
        // Remote execution via backend agent-engine
        let run_id_clone = run_id.clone();
        let state_inner = state.inner().clone();
        let spec_clone = spec.clone();

        {
            let mut reg = state.agent_run_registry.write().await;
            reg.insert(AgentRunStatus {
                run_id: run_id.clone(),
                status: "delegated".to_string(),
                step_results: HashMap::new(),
                started_at: Some(chrono::Utc::now().timestamp_millis()),
                completed_at: None,
                error: None,
            });
        }

        tokio::spawn(async move {
            match delegate_to_agent_engine(&run_id_clone, &spec_clone, &state_inner).await {
                Ok(remote_run_id) => {
                    info!(
                        "[AGENT_CMD] Delegated run '{}' to agent-engine as '{}'",
                        run_id_clone, remote_run_id
                    );
                }
                Err(e) => {
                    error!(
                        "[AGENT_CMD] Failed to delegate run '{}' to agent-engine: {}",
                        run_id_clone, e
                    );
                    let mut reg = state_inner.agent_run_registry.write().await;
                    reg.update(AgentRunStatus {
                        run_id: run_id_clone.clone(),
                        status: "failed".to_string(),
                        step_results: HashMap::new(),
                        started_at: None,
                        completed_at: Some(chrono::Utc::now().timestamp_millis()),
                        error: Some(e),
                    });
                }
            }
        });
    }

    Ok(run_id)
}

/// Plan a workflow from a natural language goal (returns spec JSON for user review).
#[tauri::command]
pub async fn plan_agent(
    goal: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    info!("[AGENT_CMD] Planning workflow for goal: '{}'", goal);
    let spec = plan_from_goal(Some(goal), &state).await?;
    serde_json::to_string_pretty(&spec).map_err(|e| e.to_string())
}

/// Get status of a running or completed agent run.
#[tauri::command]
pub async fn get_agent_run_status(
    run_id: String,
    state: State<'_, AppState>,
) -> Result<AgentRunStatus, String> {
    let reg = state.agent_run_registry.read().await;
    reg.get(&run_id)
        .cloned()
        .ok_or_else(|| format!("No agent run found with id '{}'", run_id))
}

/// List saved agent definitions (stored as DAG templates with spec_json in description).
#[tauri::command]
pub async fn list_agent_defs(state: State<'_, AppState>) -> Result<Vec<AgentSummary>, String> {
    let store = state
        .dag_store
        .lock()
        .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
    let templates = store.list_templates().map_err(|e| e.to_string())?;

    let summaries: Vec<AgentSummary> = templates
        .into_iter()
        .filter(|t| {
            t.description
                .as_deref()
                .map(|d| d.starts_with("[agent_def]"))
                .unwrap_or(false)
        })
        .map(|t| AgentSummary {
            agent_id: t.id.clone(),
            name: t.name.clone(),
            description: t
                .description
                .as_deref()
                .map(|d| d.trim_start_matches("[agent_def] ").to_string()),
            created_at: t.created_at.timestamp_millis(),
            updated_at: t.updated_at.timestamp_millis(),
        })
        .collect();

    Ok(summaries)
}

/// Save a WorkflowSpec as a reusable agent definition.
///
/// Returns the `agent_id` (same as DAG template ID).
#[tauri::command]
pub async fn save_agent_def(
    name: String,
    spec_json: String,
    description: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    // Validate the spec
    let spec: WorkflowSpec =
        serde_json::from_str(&spec_json).map_err(|e| format!("Invalid spec_json: {}", e))?;

    let agent_id = if spec.id.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        spec.id.clone()
    };

    // Store as a DAG definition (template), with spec JSON in description
    // We can't store arbitrary JSON in the existing schema, so we store the spec
    // as a single-task DAG template with a tagged description.
    use crate::dag::{DagDefinition, DagTaskDefinition};
    let now = chrono::Utc::now();
    let desc = format!(
        "[agent_def] {}",
        description.unwrap_or_else(|| spec.description.clone().unwrap_or_default())
    );

    // Store spec JSON inside the task description for retrieval.
    let task = DagTaskDefinition {
        key: "spec".to_string(),
        agent_id: "agent_engine".to_string(),
        description: spec_json.clone(),
        depends_on: vec![],
        max_retries: 0,
        retry_delay_ms: 1000,
    };

    let def = DagDefinition {
        id: agent_id.clone(),
        name: name.clone(),
        description: Some(desc),
        tasks: vec![task],
        is_template: true,
        created_at: now,
        updated_at: now,
    };

    let store = state
        .dag_store
        .lock()
        .map_err(|e| format!("Failed to lock DAG store: {}", e))?;
    store.save_definition(&def).map_err(|e| e.to_string())?;

    info!(
        "[AGENT_CMD] Saved agent definition '{}' (id={})",
        name, agent_id
    );
    Ok(agent_id)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Build a `LocalCapabilityDispatch` from AppState (async, reads sandbox config from config).
async fn build_dispatch(state: &AppState) -> LocalCapabilityDispatch {
    let sandbox_config = {
        let c = state.config.read().await;
        c.sandbox.clone()
    };
    LocalCapabilityDispatch::new(
        std::sync::Arc::clone(&state.k2k_client),
        std::sync::Arc::clone(&state.claude_client),
        sandbox_config,
    )
}

/// Plan a WorkflowSpec from an optional natural language goal.
async fn plan_from_goal(
    goal: Option<String>,
    state: &AppState,
) -> Result<WorkflowSpec, String> {
    let goal = goal
        .filter(|g| !g.trim().is_empty())
        .ok_or("Either spec_json or goal must be provided")?;

    let planner = AgentPlanner::new(
        std::sync::Arc::clone(&state.claude_client),
        std::sync::Arc::clone(&state.k2k_client),
    );

    planner.plan(&goal).await.map_err(|e| e.to_string())
}

/// Delegate a WorkflowSpec to the backend agent-engine via HTTP.
///
/// POST http://agent-engine:8019/agents/runs  (or k2k-router as proxy)
/// Returns the remote run_id.
async fn delegate_to_agent_engine(
    local_run_id: &str,
    spec: &WorkflowSpec,
    state: &AppState,
) -> Result<String, String> {
    // Resolve agent-engine URL from config (falls back to default docker DNS name)
    let agent_engine_url = {
        let cfg = state.config.read().await;
        cfg.agent_engine_url
            .clone()
            .unwrap_or_else(|| "http://agent-engine:8019".to_string())
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let payload = serde_json::json!({
        "local_run_id": local_run_id,
        "spec": spec,
    });

    let resp = client
        .post(format!("{}/agents/runs", agent_engine_url))
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("agent-engine POST failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("agent-engine returned {}: {}", status, body));
    }

    let resp_json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse agent-engine response: {}", e))?;

    let remote_run_id = resp_json["run_id"]
        .as_str()
        .unwrap_or(local_run_id)
        .to_string();

    Ok(remote_run_id)
}
