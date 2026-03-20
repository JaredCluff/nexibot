//! Tauri command wrappers for the security audit system.

use tauri::State;
use tracing::info;

use super::{timed_read, AppState};
use crate::security::audit;

/// Run a full security audit and return the report as JSON.
#[tauri::command]
pub async fn run_security_audit(state: State<'_, AppState>) -> Result<String, String> {
    info!("[AUDIT_CMD] Running full security audit");

    let config = timed_read(&state.config, "config").await?;
    let report = audit::run_full_audit(&config);

    serde_json::to_string(&report).map_err(|e| format!("Failed to serialize audit report: {}", e))
}

/// Attempt to auto-fix a specific audit finding by its ID.
///
/// The finding is looked up by re-running the audit and matching the ID.
/// If found and auto-fixable, the fix is attempted and a result returned as JSON.
#[tauri::command]
pub async fn auto_fix_finding(
    finding_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    info!("[AUDIT_CMD] Auto-fixing finding: {}", finding_id);

    let config = timed_read(&state.config, "config").await?;
    let report = audit::run_full_audit(&config);

    let finding = report
        .findings
        .iter()
        .find(|f| f.id == finding_id)
        .ok_or_else(|| {
            format!(
                "Finding '{}' not found in current audit results.",
                finding_id
            )
        })?;

    let result = audit::auto_fix(finding);

    serde_json::to_string(&result).map_err(|e| format!("Failed to serialize fix result: {}", e))
}
