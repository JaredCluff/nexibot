//! Session management Tauri commands for inter-agent messaging.

use tauri::State;

use super::AppState;
use crate::sessions::{InterSessionMessage, NamedSession};

/// Create a new named session.
#[tauri::command]
pub async fn create_named_session(
    state: State<'_, AppState>,
    name: String,
) -> Result<NamedSession, String> {
    let mut mgr = state.session_manager.write().await;
    mgr.create_session(&name)
}

/// List all named sessions.
#[tauri::command]
pub async fn list_named_sessions(state: State<'_, AppState>) -> Result<Vec<NamedSession>, String> {
    let mgr = state.session_manager.read().await;
    Ok(mgr.list_sessions())
}

/// Switch active session.
#[tauri::command]
pub async fn switch_named_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    let mut mgr = state.session_manager.write().await;
    mgr.switch_session(&session_id)
}

/// Send a message between sessions.
#[tauri::command]
pub async fn send_inter_session_message(
    state: State<'_, AppState>,
    from_session: String,
    to_session: String,
    content: String,
) -> Result<(), String> {
    let mut mgr = state.session_manager.write().await;
    mgr.send_to_session(&from_session, &to_session, &content)
}

/// Get inbox messages for a session.
#[tauri::command]
pub async fn get_session_inbox(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<InterSessionMessage>, String> {
    let mgr = state.session_manager.read().await;
    Ok(mgr.get_inbox(&session_id))
}

/// Delete a named session.
#[tauri::command]
pub async fn delete_named_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    let mut mgr = state.session_manager.write().await;
    mgr.delete_session(&session_id)
}
