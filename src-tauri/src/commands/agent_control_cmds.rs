//! Agent control Tauri commands (killswitch)

use tauri::State;
use tracing::{info, warn};

use super::AppState;
use crate::agent_control::AgentStatusInfo;
use crate::observability::AuditLogEntry;

/// Emergency stop the agent (instant, no confirmation)
#[tauri::command]
pub async fn agent_emergency_stop(state: State<'_, AppState>) -> Result<AgentStatusInfo, String> {
    warn!("[KILLSWITCH] Emergency stop command received");

    state.agent_control.emergency_stop();

    // Log to audit trail
    let audit_entry = AuditLogEntry::new("agent_emergency_stop", "user", "agent", "emergency_stop");
    state.audit_log.log(audit_entry);

    Ok(state.agent_control.get_status())
}

/// Pause the agent (queue messages, don't process)
#[tauri::command]
pub async fn agent_pause(state: State<'_, AppState>) -> Result<AgentStatusInfo, String> {
    info!("[KILLSWITCH] Pause command received");

    state.agent_control.pause();

    // Log to audit trail
    let audit_entry = AuditLogEntry::new("agent_pause", "user", "agent", "pause");
    state.audit_log.log(audit_entry);

    Ok(state.agent_control.get_status())
}

/// Resume normal operation
#[tauri::command]
pub async fn agent_resume(state: State<'_, AppState>) -> Result<AgentStatusInfo, String> {
    info!("[KILLSWITCH] Resume command received");

    state.agent_control.resume();

    // Log to audit trail
    let audit_entry = AuditLogEntry::new("agent_resume", "user", "agent", "resume");
    state.audit_log.log(audit_entry);

    Ok(state.agent_control.get_status())
}

/// Get current agent status
#[tauri::command]
pub async fn get_agent_status(state: State<'_, AppState>) -> Result<AgentStatusInfo, String> {
    Ok(state.agent_control.get_status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_status_info_serialization() {
        let status = AgentStatusInfo {
            state: "running".to_string(),
            is_stopped: false,
            is_paused: false,
            is_running: true,
            stopped_at: None,
            paused_at: None,
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("running"));
        assert!(json.contains("is_running"));
    }
}
