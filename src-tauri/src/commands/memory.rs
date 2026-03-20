//! Memory management commands

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::info;

use crate::claude::{ClaudeClient, Message};
use crate::memory::{ConversationSession, MemoryEntry, MemoryManager, MemoryType};

use super::AppState;

/// Add a memory entry
#[tauri::command]
pub async fn add_memory(
    state: State<'_, AppState>,
    content: String,
    memory_type: MemoryType,
    tags: Vec<String>,
) -> Result<String, String> {
    info!("Adding memory: {:?}", memory_type);
    let mut memory_manager = state.memory_manager.write().await;
    memory_manager
        .add_memory(content, memory_type, tags)
        .map_err(|e| e.to_string())
}

/// Get a memory by ID
#[tauri::command]
pub async fn get_memory(
    state: State<'_, AppState>,
    memory_id: String,
) -> Result<MemoryEntry, String> {
    let mut memory_manager = state.memory_manager.write().await;
    memory_manager
        .get_memory(&memory_id)
        .cloned()
        .ok_or_else(|| format!("Memory not found: {}", memory_id))
}

/// Search memories by content using the enhanced hybrid search pipeline.
///
/// Uses query expansion, RRF score fusion, reranking, and MMR diversity
/// for improved retrieval quality.
#[tauri::command]
pub async fn search_memories(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<MemoryEntry>, String> {
    let memory_manager = state.memory_manager.read().await;
    let results = memory_manager.semantic_search(&query, 20);
    Ok(results
        .into_iter()
        .map(|(entry, _score)| entry.clone())
        .collect())
}

/// Get memories by type
#[tauri::command]
pub async fn get_memories_by_type(
    state: State<'_, AppState>,
    memory_type: MemoryType,
) -> Result<Vec<MemoryEntry>, String> {
    let memory_manager = state.memory_manager.read().await;
    let results = memory_manager.get_memories_by_type(memory_type);
    Ok(results.into_iter().cloned().collect())
}

/// Delete a memory
#[tauri::command]
pub async fn delete_memory(state: State<'_, AppState>, memory_id: String) -> Result<(), String> {
    info!("Deleting memory: {}", memory_id);
    let mut memory_manager = state.memory_manager.write().await;
    memory_manager
        .delete_memory(&memory_id)
        .map_err(|e| e.to_string())
}

/// Start a new conversation session
#[tauri::command]
pub async fn start_conversation_session(state: State<'_, AppState>) -> Result<String, String> {
    info!("Starting new conversation session");
    let mut memory_manager = state.memory_manager.write().await;
    memory_manager.start_session().map_err(|e| e.to_string())
}

/// Add a message to the current session
#[tauri::command]
pub async fn add_session_message(
    state: State<'_, AppState>,
    role: String,
    content: String,
) -> Result<(), String> {
    let mut memory_manager = state.memory_manager.write().await;
    memory_manager
        .add_message(role, content)
        .map_err(|e| e.to_string())
}

/// Set the title for the current session
#[tauri::command]
pub async fn set_session_title(state: State<'_, AppState>, title: String) -> Result<(), String> {
    let mut memory_manager = state.memory_manager.write().await;
    memory_manager
        .set_session_title(title)
        .map_err(|e| e.to_string())
}

/// Get the current conversation session
#[tauri::command]
pub async fn get_current_session(
    state: State<'_, AppState>,
) -> Result<ConversationSession, String> {
    let memory_manager = state.memory_manager.read().await;
    memory_manager
        .get_current_session()
        .cloned()
        .ok_or_else(|| "No active session".to_string())
}

/// List all conversation sessions
#[tauri::command]
pub async fn list_conversation_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<ConversationSession>, String> {
    let memory_manager = state.memory_manager.read().await;
    let sessions = memory_manager.list_sessions();
    Ok(sessions.into_iter().cloned().collect())
}

/// End the current conversation session
#[tauri::command]
pub async fn end_conversation_session(state: State<'_, AppState>) -> Result<(), String> {
    info!("Ending current conversation session");
    let mut memory_manager = state.memory_manager.write().await;
    memory_manager.end_session().map_err(|e| e.to_string())
}

/// Load a saved conversation session: populates Claude history, sets as current session,
/// and resets session overrides.
#[tauri::command]
pub async fn load_conversation_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<ConversationSession, String> {
    info!("[MEMORY] Loading conversation session: {}", session_id);

    // Get the session data
    let session = {
        let memory_manager = state.memory_manager.read().await;
        memory_manager
            .get_session(&session_id)
            .cloned()
            .ok_or_else(|| format!("Session not found: {}", session_id))?
    };

    // Load messages into Claude history
    {
        let claude_client = state.claude_client.read().await;
        claude_client.load_session_messages(&session.messages).await;
    }

    // Set as current memory session
    {
        let mut memory_manager = state.memory_manager.write().await;
        memory_manager.set_current_session_id(session_id);
    }

    // Reset session overrides
    {
        let mut overrides = state.session_overrides.write().await;
        overrides.reset();
    }

    Ok(session)
}

/// Start a new conversation: ends current session, clears Claude history,
/// resets overrides, starts fresh session. Returns new session ID.
#[tauri::command]
pub async fn new_conversation(state: State<'_, AppState>) -> Result<String, String> {
    info!("[MEMORY] Starting new conversation");

    // End current session
    {
        let mut memory_manager = state.memory_manager.write().await;
        let _ = memory_manager.end_session();
    }

    // Clear Claude history
    {
        let claude_client = state.claude_client.read().await;
        claude_client.clear_history().await;
    }

    // Reset session overrides
    {
        let mut overrides = state.session_overrides.write().await;
        overrides.reset();
    }

    // Start fresh session
    let session_id = {
        let mut memory_manager = state.memory_manager.write().await;
        memory_manager.start_session().map_err(|e| e.to_string())?
    };

    Ok(session_id)
}

// ─── Session History Persistence (full Message format) ───────────────────────

/// Lightweight summary of a conversation session, used for /resume listings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub message_count: usize,
}

/// Directory where raw Claude-format session histories are stored.
/// Separate from the simplified SessionMessage format used by MemoryManager.
fn session_history_path(session_id: &str) -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nexibot")
        .join("sessions")
        .join(format!("{}_history.json", session_id))
}

/// Persist the current Claude conversation history for a session.
/// Called after each completed assistant turn so the session can be fully restored.
pub async fn save_session_history(
    claude_client: &ClaudeClient,
    session_id: &str,
) -> Result<(), String> {
    let history = claude_client.get_history().await;
    if history.is_empty() {
        return Ok(());
    }
    let path = session_history_path(session_id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string(&history).map_err(|e| e.to_string())?;
    tokio::fs::write(&path, json)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Restore a previously-saved Claude conversation history for a session.
/// Returns the number of messages loaded, or an error if the file is missing/corrupt.
#[allow(dead_code)]
pub async fn load_session_history(
    claude_client: &ClaudeClient,
    session_id: &str,
) -> Result<usize, String> {
    let path = session_history_path(session_id);
    let json = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("Cannot read session history: {}", e))?;
    let history: Vec<Message> =
        serde_json::from_str(&json).map_err(|e| format!("Cannot parse session history: {}", e))?;
    let count = history.len();
    claude_client.set_history(history).await;
    Ok(count)
}

/// List sessions eligible for /resume, optionally filtered by age.
/// Sessions are returned sorted by last_activity descending (most recent first).
pub fn list_sessions_for_resume(
    memory_manager: &MemoryManager,
    max_age_days: Option<u64>,
) -> Vec<SessionSummary> {
    let cutoff = max_age_days.map(|d| chrono::Utc::now() - chrono::Duration::days(d as i64));
    memory_manager
        .list_sessions()
        .into_iter()
        .filter(|s| cutoff.map_or(true, |c| s.last_activity > c))
        .map(|s| SessionSummary {
            id: s.id.clone(),
            title: s.title.clone().unwrap_or_else(|| "Unnamed".to_string()),
            started_at: s.started_at,
            last_activity: s.last_activity,
            message_count: s.messages.len(),
        })
        .collect()
}
