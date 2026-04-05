//! Multi-agent coordination and knowledge push Tauri commands

use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::{error, info, warn};

use super::AppState;
use crate::k2k_client::StoreInfo;

// ============================================================================
// Tier Promotion / Approval / Research / KB Browse types
// ============================================================================

/// A pending approval request for a knowledge contribution.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub contribution_id: String,
    pub article_id: String,
    pub title: String,
    pub target_tier: u8,
    pub justification: String,
    pub submitted_by: String,
    pub submitted_at: String,
    pub status: String,
}

/// Response type for listing knowledge items.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnowledgeListResponse {
    pub items: Vec<KnowledgeItem>,
    pub total: usize,
    pub page: u32,
    pub page_size: u32,
}

/// A single knowledge item in a list or detail response.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnowledgeItem {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: Vec<String>,
    pub tier: u8,
    pub store_id: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentCapabilityInfo {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskSubmitResult {
    pub task_id: String,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskPollResult {
    pub task_id: String,
    pub status: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub progress: Option<u8>,
}

/// List capabilities from the local System Agent
#[tauri::command]
pub async fn list_agent_capabilities(
    state: State<'_, AppState>,
) -> Result<Vec<AgentCapabilityInfo>, String> {
    info!("[K2K] Listing agent capabilities");

    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config.k2k.local_agent_url.clone();
    drop(config);

    let k2k = state.k2k_client.read().await;
    let client_guard = k2k.get_client().await;
    let client = match client_guard.as_ref() {
        Some(c) => c,
        None => return Err("K2K client not initialized".to_string()),
    };

    match client.list_capabilities(&base_url).await {
        Ok(response) => {
            let caps = response
                .capabilities
                .into_iter()
                .map(|c| AgentCapabilityInfo {
                    id: c.id,
                    name: c.name,
                    category: format!("{:?}", c.category),
                    description: c.description,
                    version: c.version,
                })
                .collect();
            Ok(caps)
        }
        Err(e) => {
            error!("[K2K] Failed to list capabilities: {}", e);
            Err(e.to_string())
        }
    }
}

/// Submit a task to the local System Agent
#[tauri::command]
pub async fn submit_agent_task(
    capability_id: String,
    input: serde_json::Value,
    context: Option<String>,
    state: State<'_, AppState>,
) -> Result<TaskSubmitResult, String> {
    info!("[K2K] Submitting task for capability: {}", capability_id);

    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config.k2k.local_agent_url.clone();
    let client_id = config.k2k.client_id.clone();
    drop(config);

    let k2k = state.k2k_client.read().await;
    let client_guard = k2k.get_client().await;
    let client = match client_guard.as_ref() {
        Some(c) => c,
        None => return Err("K2K client not initialized".to_string()),
    };

    let request = k2k::TaskRequest {
        capability_id,
        input,
        requesting_node_id: client_id,
        client_id: String::new(),
        timeout_seconds: Some(300),
        context,
        priority: "normal".to_string(),
        trace_id: None,
    };

    match client.submit_task(&base_url, &request).await {
        Ok(response) => Ok(TaskSubmitResult {
            task_id: response.task_id,
            status: format!("{:?}", response.status),
            error: None,
        }),
        Err(e) => {
            error!("[K2K] Failed to submit task: {}", e);
            Ok(TaskSubmitResult {
                task_id: String::new(),
                status: "failed".to_string(),
                error: Some(e.to_string()),
            })
        }
    }
}

/// Poll task status from the local System Agent
#[tauri::command]
pub async fn poll_agent_task(
    task_id: String,
    state: State<'_, AppState>,
) -> Result<TaskPollResult, String> {
    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config.k2k.local_agent_url.clone();
    drop(config);

    let k2k = state.k2k_client.read().await;
    let client_guard = k2k.get_client().await;
    let client = match client_guard.as_ref() {
        Some(c) => c,
        None => return Err("K2K client not initialized".to_string()),
    };

    match client.poll_task(&base_url, &task_id, "nexibot").await {
        Ok(response) => Ok(TaskPollResult {
            task_id: response.task_id,
            status: format!("{:?}", response.status),
            result: response.result.map(|r| r.data),
            error: response.error,
            progress: response.progress,
        }),
        Err(e) => {
            error!("[K2K] Failed to poll task: {}", e);
            Err(e.to_string())
        }
    }
}

// ============================================================================
// Knowledge Push Commands
// ============================================================================

/// Push a new knowledge article to the System Agent.
///
/// `store_id` defaults to the first available personal store when `None`.
/// Returns the article ID assigned by the System Agent.
#[tauri::command]
pub async fn push_knowledge(
    title: String,
    content: String,
    tags: Vec<String>,
    source_url: Option<String>,
    store_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    info!("[K2K] push_knowledge: title='{}'", title);

    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    drop(config);

    let k2k = state.k2k_client.read().await;

    // Resolve the target store: use the provided store_id, or fall back to the
    // first personal store returned by the System Agent.
    let resolved_store_id = if let Some(id) = store_id {
        id
    } else {
        let stores = k2k.list_stores().await.map_err(|e| {
            error!("[K2K] push_knowledge: failed to list stores: {}", e);
            e.to_string()
        })?;
        stores
            .into_iter()
            .find(|s| s.store_type == "personal")
            .map(|s| s.id)
            .ok_or_else(|| "No personal store found on System Agent".to_string())?
    };

    k2k.create_article(
        &title,
        &content,
        tags,
        source_url.as_deref(),
        &resolved_store_id,
    )
    .await
    .map_err(|e| {
        error!("[K2K] push_knowledge failed: {}", e);
        e.to_string()
    })
}

/// Update an existing knowledge article on the System Agent (partial update).
///
/// Only fields that are `Some` are sent in the update payload.
#[tauri::command]
pub async fn update_knowledge(
    article_id: String,
    title: Option<String>,
    content: Option<String>,
    tags: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!("[K2K] update_knowledge: article_id='{}'", article_id);

    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    drop(config);

    let k2k = state.k2k_client.read().await;
    k2k.update_article(
        &article_id,
        title.as_deref(),
        content.as_deref(),
        tags,
    )
    .await
    .map_err(|e| {
        error!("[K2K] update_knowledge failed: {}", e);
        e.to_string()
    })
}

/// List all knowledge stores available on the System Agent.
#[tauri::command]
pub async fn list_knowledge_stores(
    state: State<'_, AppState>,
) -> Result<Vec<StoreInfo>, String> {
    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    drop(config);

    let k2k = state.k2k_client.read().await;
    k2k.list_stores().await.map_err(|e| {
        error!("[K2K] list_knowledge_stores failed: {}", e);
        e.to_string()
    })
}

// ============================================================================
// Web Search via K2K
// ============================================================================

/// A single web search result returned by the System Agent's web_search capability.
#[derive(Debug, Serialize, Deserialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}

/// Response from a web search performed via K2K.
#[derive(Debug, Serialize, Deserialize)]
pub struct WebSearchResponse {
    pub results: Vec<WebSearchResult>,
    pub total: usize,
    pub query: String,
    pub provider: String,
}

/// Search the web via the local System Agent's K2K `web_search` capability.
///
/// Submits a `web_search` task to the System Agent, polls until complete (up to 30 s),
/// and returns structured results to the NexiBot frontend.
#[tauri::command]
pub async fn search_web_via_k2k(
    query: String,
    max_results: Option<u32>,
    state: State<'_, AppState>,
) -> Result<WebSearchResponse, String> {
    info!("[K2K] Web search via K2K: \"{}\"", query);

    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config.k2k.local_agent_url.clone();
    let client_id = config.k2k.client_id.clone();
    drop(config);

    let k2k = state.k2k_client.read().await;
    let client_guard = k2k.get_client().await;
    let client = match client_guard.as_ref() {
        Some(c) => c,
        None => return Err("K2K client not initialized".to_string()),
    };

    // Build task input
    let mut input = serde_json::json!({ "query": query });
    if let Some(n) = max_results {
        input["max_results"] = serde_json::json!(n);
    }

    let request = k2k::TaskRequest {
        capability_id: "web_search".to_string(),
        input,
        requesting_node_id: client_id,
        client_id: String::new(),
        timeout_seconds: Some(30),
        context: None,
        priority: "normal".to_string(),
        trace_id: None,
    };

    // Submit task
    let submit_resp = client
        .submit_task(&base_url, &request)
        .await
        .map_err(|e| {
            error!("[K2K] Failed to submit web_search task: {}", e);
            e.to_string()
        })?;

    let task_id = submit_resp.task_id;

    // Poll until complete (up to 30 iterations × 1 s = 30 s)
    let max_polls = 30_u32;
    for attempt in 0..max_polls {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        match client.poll_task(&base_url, &task_id, "nexibot").await {
            Ok(status_resp) => {
                let status_str = format!("{:?}", status_resp.status);
                match status_str.as_str() {
                    "Completed" => {
                        let data = status_resp
                            .result
                            .map(|r| r.data)
                            .unwrap_or(serde_json::Value::Null);

                        let results: Vec<WebSearchResult> = data["results"]
                            .as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .map(|item| WebSearchResult {
                                title: item["title"].as_str().unwrap_or("").to_string(),
                                url: item["url"].as_str().unwrap_or("").to_string(),
                                content: item["content"].as_str().unwrap_or("").to_string(),
                            })
                            .collect();

                        let total = results.len();
                        let provider = data["provider"]
                            .as_str()
                            .unwrap_or("unknown")
                            .to_string();

                        return Ok(WebSearchResponse {
                            results,
                            total,
                            query,
                            provider,
                        });
                    }
                    "Failed" | "Cancelled" => {
                        let err = status_resp
                            .error
                            .unwrap_or_else(|| format!("Task {}", status_str.to_lowercase()));
                        error!("[K2K] Web search task {}: {}", status_str, err);
                        return Err(err);
                    }
                    _ => {
                        // Still queued or running — keep polling
                        if attempt % 5 == 0 {
                            info!(
                                "[K2K] Web search task {} still {} (attempt {}/{})",
                                task_id,
                                status_str,
                                attempt + 1,
                                max_polls
                            );
                        }
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[K2K] Poll attempt {}/{} for web search task {} failed: {}",
                    attempt + 1,
                    max_polls,
                    task_id,
                    e
                );
            }
        }
    }

    Err(format!(
        "Web search task {} timed out after {} seconds",
        task_id, max_polls
    ))
}

// ============================================================================
// Knowledge Tier Promotion
// ============================================================================

/// Promote a knowledge article to a higher tier (team / org / public).
///
/// Calls `POST /research/kb/contribute/multi-tier` on the knowledge-nexus
/// backend via the K2K router URL (falls back to local agent URL).
/// Returns a contribution_id the caller can use to track approval status.
#[tauri::command]
pub async fn promote_knowledge(
    article_id: String,
    target_tier: u8,
    justification: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    info!(
        "[K2K] promote_knowledge: article_id='{}' target_tier={}",
        article_id, target_tier
    );

    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config
        .k2k
        .router_url
        .clone()
        .unwrap_or_else(|| config.k2k.local_agent_url.clone());
    let client_id = config.k2k.client_id.clone();
    drop(config);

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/research/kb/contribute/multi-tier", base_url))
        .json(&serde_json::json!({
            "article_id": article_id,
            "target_tier": target_tier,
            "justification": justification,
            "submitted_by": client_id,
        }))
        .send()
        .await
        .map_err(|e| {
            error!("[K2K] promote_knowledge HTTP error: {}", e);
            e.to_string()
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("promote_knowledge failed (HTTP {}): {}", status, body));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let contribution_id = json["contribution_id"]
        .as_str()
        .or_else(|| json["id"].as_str())
        .ok_or_else(|| "No contribution_id in promote response".to_string())?
        .to_string();

    info!(
        "[K2K] promote_knowledge: contribution_id={}",
        contribution_id
    );
    Ok(contribution_id)
}

// ============================================================================
// Approval Request Commands
// ============================================================================

/// List pending approval requests for knowledge contributions.
///
/// Calls `GET /api/v1/approvals/pending` via the approval-orchestrator service
/// (routed through the K2K router or local agent URL).
#[tauri::command]
pub async fn list_pending_approvals(
    state: State<'_, AppState>,
) -> Result<Vec<ApprovalRequest>, String> {
    info!("[K2K] list_pending_approvals");

    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config
        .k2k
        .router_url
        .clone()
        .unwrap_or_else(|| config.k2k.local_agent_url.clone());
    drop(config);

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/api/v1/approvals/pending", base_url))
        .send()
        .await
        .map_err(|e| {
            error!("[K2K] list_pending_approvals HTTP error: {}", e);
            e.to_string()
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "list_pending_approvals failed (HTTP {}): {}",
            status, body
        ));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let items = json
        .as_array()
        .or_else(|| json["approvals"].as_array())
        .or_else(|| json["items"].as_array())
        .cloned()
        .unwrap_or_default();

    let approvals: Vec<ApprovalRequest> = items
        .into_iter()
        .map(|v| ApprovalRequest {
            approval_id: v["approval_id"]
                .as_str()
                .or_else(|| v["id"].as_str())
                .unwrap_or("")
                .to_string(),
            contribution_id: v["contribution_id"].as_str().unwrap_or("").to_string(),
            article_id: v["article_id"].as_str().unwrap_or("").to_string(),
            title: v["title"].as_str().unwrap_or("").to_string(),
            target_tier: v["target_tier"].as_u64().unwrap_or(0) as u8,
            justification: v["justification"].as_str().unwrap_or("").to_string(),
            submitted_by: v["submitted_by"].as_str().unwrap_or("").to_string(),
            submitted_at: v["submitted_at"].as_str().unwrap_or("").to_string(),
            status: v["status"].as_str().unwrap_or("pending").to_string(),
        })
        .collect();

    info!("[K2K] list_pending_approvals: {} items", approvals.len());
    Ok(approvals)
}

/// Approve or reject a knowledge contribution.
///
/// Calls `POST /api/v1/approvals/{approval_id}/respond` on the
/// approval-orchestrator service.
#[tauri::command]
pub async fn approve_contribution(
    approval_id: String,
    approve: bool,
    comment: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!(
        "[K2K] approve_contribution: approval_id='{}' approve={}",
        approval_id, approve
    );

    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config
        .k2k
        .router_url
        .clone()
        .unwrap_or_else(|| config.k2k.local_agent_url.clone());
    let client_id = config.k2k.client_id.clone();
    drop(config);

    let http = reqwest::Client::new();
    let mut body = serde_json::json!({
        "decision": if approve { "approved" } else { "rejected" },
        "reviewer": client_id,
    });
    if let Some(c) = comment {
        body["comment"] = serde_json::Value::String(c);
    }

    let resp = http
        .post(format!(
            "{}/api/v1/approvals/{}/respond",
            base_url, approval_id
        ))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            error!("[K2K] approve_contribution HTTP error: {}", e);
            e.to_string()
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "approve_contribution failed (HTTP {}): {}",
            status, body_text
        ));
    }

    info!(
        "[K2K] approve_contribution: {} decision recorded",
        if approve { "approved" } else { "rejected" }
    );
    Ok(())
}

// ============================================================================
// On-demand Research Commands
// ============================================================================

/// Trigger a research task on the research-service.
///
/// Calls `POST /research/tasks` and returns a task_id for polling.
#[tauri::command]
pub async fn trigger_research(
    topic: String,
    research_type: String,
    target_store_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    info!(
        "[K2K] trigger_research: topic='{}' type='{}'",
        topic, research_type
    );

    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config
        .k2k
        .router_url
        .clone()
        .unwrap_or_else(|| config.k2k.local_agent_url.clone());
    let client_id = config.k2k.client_id.clone();
    drop(config);

    let http = reqwest::Client::new();
    let mut payload = serde_json::json!({
        "topic": topic,
        "research_type": research_type,
        "requested_by": client_id,
    });
    if let Some(store_id) = target_store_id {
        payload["target_store_id"] = serde_json::Value::String(store_id);
    }

    let resp = http
        .post(format!("{}/research/tasks", base_url))
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            error!("[K2K] trigger_research HTTP error: {}", e);
            e.to_string()
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("trigger_research failed (HTTP {}): {}", status, body));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let task_id = json["task_id"]
        .as_str()
        .or_else(|| json["id"].as_str())
        .ok_or_else(|| "No task_id in trigger_research response".to_string())?
        .to_string();

    info!("[K2K] trigger_research: task_id={}", task_id);
    Ok(task_id)
}

/// Poll the status of a research task.
///
/// Calls `GET /research/tasks/{task_id}` and returns the full status JSON.
#[tauri::command]
pub async fn poll_research_task(
    task_id: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config
        .k2k
        .router_url
        .clone()
        .unwrap_or_else(|| config.k2k.local_agent_url.clone());
    drop(config);

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/research/tasks/{}", base_url, task_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "poll_research_task failed (HTTP {}): {}",
            status, body
        ));
    }

    resp.json::<serde_json::Value>().await.map_err(|e| e.to_string())
}

/// Resume a previously checkpointed research task from a specific step.
///
/// Calls `POST /research/tasks/{task_id}/resume` and returns the
/// `resume_task_id` assigned by the research-service for the new run.
///
/// # Arguments
/// * `task_id`   - ID of the failed / paused task to resume.
/// * `from_step` - Optional step index to resume from (omit to use the
///                 checkpoint saved by the research-service).
#[tauri::command]
pub async fn resume_research_task(
    task_id: String,
    from_step: Option<u32>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    info!(
        "[K2K] resume_research_task: task_id='{}' from_step={:?}",
        task_id, from_step
    );

    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config
        .k2k
        .router_url
        .clone()
        .unwrap_or_else(|| config.k2k.local_agent_url.clone());
    drop(config);

    let mut payload = serde_json::json!({});
    if let Some(step) = from_step {
        payload["from_step"] = serde_json::json!(step);
    }

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/research/tasks/{}/resume", base_url, task_id))
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            error!("[K2K] resume_research_task HTTP error: {}", e);
            e.to_string()
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "resume_research_task failed (HTTP {}): {}",
            status, body
        ));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let resume_task_id = json["task_id"]
        .as_str()
        .or_else(|| json["resume_task_id"].as_str())
        .or_else(|| json["id"].as_str())
        .ok_or_else(|| "No task_id in resume_research_task response".to_string())?
        .to_string();

    info!(
        "[K2K] resume_research_task: new resume_task_id={}",
        resume_task_id
    );
    Ok(resume_task_id)
}

/// Summary of a research task returned by `list_research_tasks`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResearchTaskSummary {
    pub task_id: String,
    pub topic: String,
    pub research_type: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub error: Option<String>,
    pub progress: Option<u8>,
    pub checkpoint_step: Option<u32>,
}

/// List research tasks, optionally filtered by status.
///
/// Calls `GET /research/tasks` and returns a list of task summaries so the
/// user can identify failed tasks that need to be resumed via
/// `resume_research_task`.
///
/// # Arguments
/// * `status_filter` - Optional status string (e.g. `"failed"`, `"paused"`,
///                     `"running"`).  When `None`, all tasks are returned.
#[tauri::command]
pub async fn list_research_tasks(
    status_filter: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ResearchTaskSummary>, String> {
    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config
        .k2k
        .router_url
        .clone()
        .unwrap_or_else(|| config.k2k.local_agent_url.clone());
    drop(config);

    let http = reqwest::Client::new();
    let mut req = http.get(format!("{}/research/tasks", base_url));
    if let Some(ref status) = status_filter {
        req = req.query(&[("status", status.as_str())]);
    }

    let resp = req.send().await.map_err(|e| {
        error!("[K2K] list_research_tasks HTTP error: {}", e);
        e.to_string()
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "list_research_tasks failed (HTTP {}): {}",
            status, body
        ));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    // Accept either a bare array or a paginated envelope.
    let items = if let Some(arr) = json.as_array() {
        arr.clone()
    } else {
        json["tasks"]
            .as_array()
            .or_else(|| json["items"].as_array())
            .cloned()
            .unwrap_or_default()
    };

    let tasks: Vec<ResearchTaskSummary> = items
        .into_iter()
        .map(|v| ResearchTaskSummary {
            task_id: v["task_id"]
                .as_str()
                .or_else(|| v["id"].as_str())
                .unwrap_or("")
                .to_string(),
            topic: v["topic"].as_str().unwrap_or("").to_string(),
            research_type: v["research_type"].as_str().unwrap_or("").to_string(),
            status: v["status"].as_str().unwrap_or("unknown").to_string(),
            created_at: v["created_at"].as_str().unwrap_or("").to_string(),
            updated_at: v["updated_at"].as_str().unwrap_or("").to_string(),
            error: v["error"].as_str().map(|s| s.to_string()),
            progress: v["progress"].as_u64().map(|n| n as u8),
            checkpoint_step: v["checkpoint_step"].as_u64().map(|n| n as u32),
        })
        .collect();

    info!("[K2K] list_research_tasks: {} tasks (filter={:?})", tasks.len(), status_filter);
    Ok(tasks)
}

// ============================================================================
// KB Browsing Commands
// ============================================================================

/// List knowledge items from the KB, with optional filtering.
///
/// Calls `GET /api/v1/articles` on the local agent, with query params for
/// store, search text, tags, and pagination.
#[tauri::command]
pub async fn list_knowledge(
    store_id: Option<String>,
    search: Option<String>,
    tags: Option<Vec<String>>,
    page: Option<u32>,
    state: State<'_, AppState>,
) -> Result<KnowledgeListResponse, String> {
    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config.k2k.local_agent_url.clone();
    drop(config);

    let http = reqwest::Client::new();
    let mut req = http.get(format!("{}/api/v1/articles", base_url));

    if let Some(ref sid) = store_id {
        req = req.query(&[("store_id", sid.as_str())]);
    }
    if let Some(ref q) = search {
        req = req.query(&[("search", q.as_str())]);
    }
    if let Some(ref tag_list) = tags {
        for tag in tag_list {
            req = req.query(&[("tag", tag.as_str())]);
        }
    }
    let page_num = page.unwrap_or(1);
    req = req.query(&[("page", page_num.to_string().as_str())]);

    let resp = req.send().await.map_err(|e| {
        error!("[K2K] list_knowledge HTTP error: {}", e);
        e.to_string()
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("list_knowledge failed (HTTP {}): {}", status, body));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    // Accept either a bare array or a paginated envelope
    let (items_json, total, page_size) = if let Some(arr) = json.as_array() {
        let len = arr.len();
        (arr.clone(), len, len)
    } else {
        let items = json["items"]
            .as_array()
            .or_else(|| json["articles"].as_array())
            .cloned()
            .unwrap_or_default();
        let total = json["total"].as_u64().unwrap_or(items.len() as u64) as usize;
        let page_size = json["page_size"].as_u64().unwrap_or(20) as usize;
        (items, total, page_size)
    };

    let items: Vec<KnowledgeItem> = items_json
        .into_iter()
        .map(|v| KnowledgeItem {
            id: v["id"].as_str().unwrap_or("").to_string(),
            title: v["title"].as_str().unwrap_or("").to_string(),
            summary: v["summary"].as_str().unwrap_or("").to_string(),
            content: v["content"].as_str().unwrap_or("").to_string(),
            tags: v["tags"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect(),
            tier: v["tier"].as_u64().unwrap_or(0) as u8,
            store_id: v["store_id"].as_str().unwrap_or("").to_string(),
            created_at: v["created_at"].as_str().unwrap_or("").to_string(),
            updated_at: v["updated_at"].as_str().unwrap_or("").to_string(),
        })
        .collect();

    Ok(KnowledgeListResponse {
        total,
        page: page_num,
        page_size: page_size as u32,
        items,
    })
}

/// Get a single knowledge item by article ID.
///
/// Calls `GET /api/v1/articles/{article_id}` on the local agent.
#[tauri::command]
pub async fn get_knowledge_item(
    article_id: String,
    state: State<'_, AppState>,
) -> Result<KnowledgeItem, String> {
    let config = state.config.read().await;
    if !config.k2k.enabled {
        return Err("K2K integration is disabled".to_string());
    }
    let base_url = config.k2k.local_agent_url.clone();
    drop(config);

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/api/v1/articles/{}", base_url, article_id))
        .send()
        .await
        .map_err(|e| {
            error!("[K2K] get_knowledge_item HTTP error: {}", e);
            e.to_string()
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "get_knowledge_item failed (HTTP {}): {}",
            status, body
        ));
    }

    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(KnowledgeItem {
        id: v["id"].as_str().unwrap_or("").to_string(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        summary: v["summary"].as_str().unwrap_or("").to_string(),
        content: v["content"].as_str().unwrap_or("").to_string(),
        tags: v["tags"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.to_string()))
            .collect(),
        tier: v["tier"].as_u64().unwrap_or(0) as u8,
        store_id: v["store_id"].as_str().unwrap_or("").to_string(),
        created_at: v["created_at"].as_str().unwrap_or("").to_string(),
        updated_at: v["updated_at"].as_str().unwrap_or("").to_string(),
    })
}
