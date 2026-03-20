//! Observability and monitoring commands

use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::info;

use super::AppState;
use crate::observability::{AuditLogEntry, CostMetrics};

/// Get current cost metrics
#[tauri::command]
pub async fn get_cost_metrics(state: State<'_, AppState>) -> Result<CostMetrics, String> {
    Ok(state.cost_tracker.get_metrics())
}

/// Check if daily budget is exceeded
#[tauri::command]
pub async fn check_daily_budget(
    budget_usd: f64,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    Ok(state.cost_tracker.check_daily_budget(budget_usd))
}

/// Record API usage (input/output tokens)
#[tauri::command]
pub async fn record_api_usage(
    input_tokens: u64,
    output_tokens: u64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .cost_tracker
        .record_tokens(input_tokens, output_tokens);
    Ok(())
}

/// Get recent audit log entries
#[derive(Debug, Serialize, Deserialize)]
pub struct AuditLogResponse {
    pub timestamp: String,
    pub event_type: String,
    pub actor: String,
    pub resource: String,
    pub action: String,
    pub status: String,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn get_audit_logs(
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<AuditLogResponse>, String> {
    let limit = limit.unwrap_or(100).min(1000); // Max 1000
    let entries = state.audit_log.get_recent(limit);

    Ok(entries
        .into_iter()
        .map(|e| AuditLogResponse {
            timestamp: e.timestamp,
            event_type: e.event_type,
            actor: e.actor,
            resource: e.resource,
            action: e.action,
            status: e.status,
            error: e.error,
        })
        .collect())
}

/// Log an audit entry
#[tauri::command]
pub async fn log_audit_event(
    event_type: String,
    actor: String,
    resource: String,
    action: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let entry = AuditLogEntry::new(event_type, actor, resource, action);
    state.audit_log.log(entry);
    info!("[AUDIT] Event logged");
    Ok(())
}

/// Filter audit logs by event type
#[tauri::command]
pub async fn filter_audit_logs(
    event_type: String,
    state: State<'_, AppState>,
) -> Result<Vec<AuditLogResponse>, String> {
    let entries = state.audit_log.filter_by_type(&event_type);

    Ok(entries
        .into_iter()
        .map(|e| AuditLogResponse {
            timestamp: e.timestamp,
            event_type: e.event_type,
            actor: e.actor,
            resource: e.resource,
            action: e.action,
            status: e.status,
            error: e.error,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_log_response() {
        let response = AuditLogResponse {
            timestamp: "2026-02-26T10:30:00Z".to_string(),
            event_type: "api_call".to_string(),
            actor: "user123".to_string(),
            resource: "/api/messages".to_string(),
            action: "POST".to_string(),
            status: "success".to_string(),
            error: None,
        };

        assert_eq!(response.event_type, "api_call");
        assert_eq!(response.status, "success");
    }
}
