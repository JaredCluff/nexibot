//! Renders canvas state changes as Tauri events for the frontend.
//!
//! Each function emits a named event that the frontend listens on to
//! synchronize its canvas rendering. When running under `#[cfg(test)]`
//! the Tauri-specific emit calls are skipped (there is no real app handle
//! in unit tests).
#![allow(dead_code)]

use serde::Serialize;
use tracing::{debug, warn};

/// Payload for the `canvas:panel-updated` event.
#[derive(Debug, Clone, Serialize)]
pub struct PanelUpdatedPayload {
    pub panel_id: String,
    pub content: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Stateless renderer (emitter)
// ---------------------------------------------------------------------------

/// Stateless renderer that emits Tauri events for canvas mutations.
///
/// All methods are static; the struct exists solely as a namespace.
pub struct CanvasRenderer;

impl CanvasRenderer {
    /// Emit a `canvas:panel-updated` event to the frontend.
    #[cfg(not(test))]
    pub fn emit_panel_updated(app_handle: &tauri::AppHandle, panel_id: &str, content: &str) {
        use tauri::Emitter;
        let payload = PanelUpdatedPayload {
            panel_id: panel_id.to_string(),
            content: content.to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(e) = app_handle.emit("canvas:panel-updated", &payload) {
            warn!("[CANVAS_RENDER] Failed to emit panel-updated: {}", e);
        } else {
            debug!("[CANVAS_RENDER] Emitted panel-updated for '{}'", panel_id);
        }
    }

    // --- Test stubs (no-op when AppHandle is unavailable) ---

    #[cfg(test)]
    pub fn emit_panel_updated(_app_handle: &(), panel_id: &str, _content: &str) {
        debug!(
            "[CANVAS_RENDER][TEST] Would emit panel-updated for '{}'",
            panel_id
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_panel_updated_stub() {
        CanvasRenderer::emit_panel_updated(&(), "test-1", "new content");
    }

    #[test]
    fn test_payload_serialization_panel_updated() {
        let payload = PanelUpdatedPayload {
            panel_id: "p1".to_string(),
            content: "updated".to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("p1"));
        assert!(json.contains("updated"));
    }
}
