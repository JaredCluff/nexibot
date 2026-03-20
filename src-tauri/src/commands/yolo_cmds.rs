//! Yolo mode Tauri commands.
//!
//! `request_yolo_mode` — callable by the model (via UI invoke from tool result display)
//!                       or directly from the chat UI; submits a pending request.
//! `approve_yolo_mode` — UI-ONLY; the model has NO path to call this.
//! `revoke_yolo_mode`  — callable from UI or model (revoking is always safe).
//! `get_yolo_status`   — readable by anyone.
//!
//! The approval segregation is enforced architecturally: Tauri commands are only
//! reachable via `window.__TAURI__.invoke()` from the React UI.  The model's tool
//! execution path (tool_loop → execute_tool_call) only has access to `nexibot_*`
//! tool definitions — none of which map to `approve_yolo_mode`.

use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};
use tracing::{info, warn};

use super::AppState;
use crate::observability::AuditLogEntry;
use crate::telegram::send_yolo_approval_request;
use crate::yolo_mode::{YoloRequest, YoloStatus};

// ---------------------------------------------------------------------------
// Shared response type
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct YoloCmdResult {
    pub ok: bool,
    pub message: String,
    pub status: Option<YoloStatus>,
    pub request: Option<YoloRequest>,
}

impl YoloCmdResult {
    fn success(message: impl Into<String>, status: YoloStatus) -> Self {
        Self {
            ok: true,
            message: message.into(),
            status: Some(status),
            request: None,
        }
    }
    fn with_request(req: YoloRequest) -> Self {
        Self {
            ok: true,
            message: format!(
                "Yolo mode request submitted (id={}). Waiting for user approval.",
                req.id
            ),
            status: None,
            request: Some(req),
        }
    }
    fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            status: None,
            request: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Submit a yolo mode request.
///
/// Can be called from the React UI directly (e.g. a "Request Yolo Mode" button)
/// or invoked programmatically when the model needs elevated access.  A pending
/// approval notification is emitted to all UI windows.
#[tauri::command]
pub async fn request_yolo_mode(
    duration_secs: Option<u64>,
    reason: Option<String>,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<YoloCmdResult, String> {
    let mgr = &state.yolo_manager;
    match mgr.request(duration_secs, reason.clone()).await {
        Ok(req) => {
            // Emit a Tauri event so all open windows can show an approval prompt.
            if let Err(e) = app.emit("yolo:request-pending", &req) {
                warn!(
                    "[YOLO_CMD] Failed to emit yolo:request-pending for {}: {}",
                    req.id, e
                );
            }
            info!("[YOLO_CMD] Request submitted: id={}", req.id);

            // Also send a Telegram notification with Approve/Deny buttons so the
            // user can approve from their phone without touching the desktop UI.
            let state_clone = state.inner().clone();
            let req_id = req.id.clone();
            let req_duration = req.duration_secs;
            let req_reason = req.reason.clone();
            tauri::async_runtime::spawn(async move {
                send_yolo_approval_request(
                    &state_clone,
                    &req_id,
                    req_duration,
                    req_reason.as_deref(),
                )
                .await;
            });

            Ok(YoloCmdResult::with_request(req))
        }
        Err(e) => Ok(YoloCmdResult::error(e)),
    }
}

/// Approve a pending yolo mode request.
///
/// **UI-ONLY** — this command is registered in the invoke_handler but intentionally
/// NOT included in any `nexibot_*` tool definition.  The model cannot call this;
/// only the human interacting with the Tauri window can invoke it.
#[tauri::command]
pub async fn approve_yolo_mode(
    request_id: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<YoloCmdResult, String> {
    let mgr = &state.yolo_manager;
    match mgr.approve(&request_id).await {
        Ok(status) => {
            if let Err(e) = app.emit("yolo:approved", &status) {
                warn!(
                    "[YOLO_CMD] Failed to emit yolo:approved for {}: {}",
                    request_id, e
                );
            }
            info!("[YOLO_CMD] Approved by user (id={})", request_id);
            state.audit_log.log(
                AuditLogEntry::new("yolo_mode_approved", "ui_user", "yolo_mode", "approve")
                    .with_metadata(serde_json::json!({
                        "request_id": request_id,
                        "expires_at_ms": status.expires_at_ms,
                        "remaining_secs": status.remaining_secs,
                    })),
            );
            Ok(YoloCmdResult::success("Yolo mode activated.", status))
        }
        Err(e) => Ok(YoloCmdResult::error(e)),
    }
}

/// Revoke yolo mode immediately.
///
/// Safe to call from either the UI or a model tool (revoking is never privileged).
#[tauri::command]
pub async fn revoke_yolo_mode(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<YoloCmdResult, String> {
    let mgr = &state.yolo_manager;
    let status = mgr.revoke().await;
    if let Err(e) = app.emit("yolo:revoked", &status) {
        warn!("[YOLO_CMD] Failed to emit yolo:revoked: {}", e);
    }
    info!("[YOLO_CMD] Revoked");
    Ok(YoloCmdResult::success("Yolo mode revoked.", status))
}

/// Get the current yolo mode status.
#[tauri::command]
pub async fn get_yolo_status(state: State<'_, AppState>) -> Result<YoloStatus, String> {
    Ok(state.yolo_manager.status().await)
}
