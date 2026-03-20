//! Canvas operations that agents can invoke via tool calls.
//!
//! Defines the operation enum (inbound from agent tool use), the event
//! enum (outbound to the frontend via Tauri), and the tool definition
//! that is surfaced to the LLM.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{CanvasLayout, CanvasManager, CanvasPanel, PanelContentType, PanelPosition, PanelSize};

// ---------------------------------------------------------------------------
// Operations (agent -> backend)
// ---------------------------------------------------------------------------

/// An operation the agent wishes to perform on the canvas.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CanvasOperation {
    CreatePanel {
        title: String,
        content_type: PanelContentType,
        content: String,
        position: Option<PanelPosition>,
        size: Option<PanelSize>,
    },
    UpdateContent {
        panel_id: String,
        content: String,
    },
    UpdateTitle {
        panel_id: String,
        title: String,
    },
    DeletePanel {
        panel_id: String,
    },
    ClearAll,
    SetLayout {
        layout: CanvasLayout,
    },
}

// ---------------------------------------------------------------------------
// Events (backend -> frontend)
// ---------------------------------------------------------------------------

/// Events emitted to the Tauri frontend after canvas mutations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CanvasEvent {
    PanelCreated(CanvasPanel),
    PanelUpdated {
        id: String,
        content: String,
        updated_at: DateTime<Utc>,
    },
    PanelRemoved {
        id: String,
    },
    LayoutChanged(CanvasLayout),
    AllCleared,
}

// ---------------------------------------------------------------------------
// Tool definition (surfaced to LLM)
// ---------------------------------------------------------------------------

/// Returns the JSON tool definition for `nexibot_canvas` suitable for
/// inclusion in the LLM tool list.
pub fn nexibot_canvas_tool_definition() -> Value {
    json!({
        "name": "nexibot_canvas",
        "description": "Create and manage visual panels in the Canvas workspace. Use for structured output like tables, code, markdown documents, or JSON visualizations.",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "update", "delete", "clear", "set_layout"]
                },
                "title": {
                    "type": "string",
                    "description": "Title of the panel (required for create)"
                },
                "content": {
                    "type": "string",
                    "description": "Content of the panel (required for create and update)"
                },
                "content_type": {
                    "type": "string",
                    "enum": ["markdown", "code", "table", "json", "html"],
                    "description": "Type of content in the panel (default: markdown)"
                },
                "panel_id": {
                    "type": "string",
                    "description": "ID of an existing panel (required for update and delete)"
                },
                "language": {
                    "type": "string",
                    "description": "Programming language for code panels"
                },
                "layout": {
                    "type": "string",
                    "enum": ["freeform", "grid", "stack"],
                    "description": "Canvas layout strategy (required for set_layout)"
                }
            },
            "required": ["action"]
        }
    })
}

// ---------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------

/// Parse the `content_type` string (+ optional `language`) into a
/// [`PanelContentType`].
fn parse_content_type(input: &Value) -> PanelContentType {
    let ct = input
        .get("content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("markdown");

    match ct {
        "code" => {
            let language = input
                .get("language")
                .and_then(|v| v.as_str())
                .unwrap_or("plaintext")
                .to_string();
            PanelContentType::Code { language }
        }
        "table" => PanelContentType::Table,
        "json" => PanelContentType::Json,
        "html" => PanelContentType::Html,
        _ => PanelContentType::Markdown,
    }
}

/// Parse a `layout` string into a [`CanvasLayout`].
fn parse_layout(value: &str) -> CanvasLayout {
    match value {
        "grid" => CanvasLayout::Grid { columns: 2 },
        "stack" => CanvasLayout::Stack,
        _ => CanvasLayout::Freeform,
    }
}

/// Execute a canvas tool call.
///
/// Dispatches the action described by `input` (a JSON object) to the
/// provided [`CanvasManager`] and returns a human-readable result string.
pub fn execute_canvas_tool(input: &Value, manager: &mut CanvasManager) -> String {
    let action = match input.get("action").and_then(|v| v.as_str()) {
        Some(a) => a,
        None => return "Error: missing required field 'action'.".to_string(),
    };

    match action {
        "create" => {
            let title = input
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Untitled Panel")
                .to_string();
            let content = input
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let content_type = parse_content_type(input);
            let position = PanelPosition::default();
            let size = PanelSize::default();

            let now = Utc::now();
            let panel = CanvasPanel {
                id: Uuid::new_v4().to_string(),
                title: title.clone(),
                content_type,
                content,
                position,
                size,
                created_at: now,
                updated_at: now,
                agent_id: "default".to_string(),
                visible: true,
            };

            let id = manager.create_panel(panel);
            info!("[CANVAS] Tool: created panel '{}' ({})", id, title);
            format!("Created panel '{}' with id '{}'.", title, id)
        }
        "update" => {
            let panel_id = match input.get("panel_id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => return "Error: 'update' action requires 'panel_id'.".to_string(),
            };
            let content = match input.get("content").and_then(|v| v.as_str()) {
                Some(c) => c,
                None => return "Error: 'update' action requires 'content'.".to_string(),
            };

            match manager.update_panel(panel_id, content) {
                Ok(()) => {
                    debug!("[CANVAS] Tool: updated panel '{}'", panel_id);
                    format!("Updated panel '{}'.", panel_id)
                }
                Err(e) => format!("Error updating panel: {}", e),
            }
        }
        "delete" => {
            let panel_id = match input.get("panel_id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => return "Error: 'delete' action requires 'panel_id'.".to_string(),
            };

            match manager.remove_panel(panel_id) {
                Ok(()) => {
                    info!("[CANVAS] Tool: deleted panel '{}'", panel_id);
                    format!("Deleted panel '{}'.", panel_id)
                }
                Err(e) => format!("Error deleting panel: {}", e),
            }
        }
        "clear" => {
            manager.clear_all();
            info!("[CANVAS] Tool: cleared all panels");
            "Cleared all panels.".to_string()
        }
        "set_layout" => {
            let layout_str = input
                .get("layout")
                .and_then(|v| v.as_str())
                .unwrap_or("freeform");
            let layout = parse_layout(layout_str);
            manager.set_layout(layout);
            info!("[CANVAS] Tool: layout set to '{}'", layout_str);
            format!("Canvas layout set to '{}'.", layout_str)
        }
        unknown => {
            warn!("[CANVAS] Tool: unknown action '{}'", unknown);
            format!("Error: unknown action '{}'. Valid actions: create, update, delete, clear, set_layout.", unknown)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definition_structure() {
        let def = nexibot_canvas_tool_definition();
        assert_eq!(def["name"], "nexibot_canvas");
        assert!(def["input_schema"]["properties"]["action"].is_object());
        assert_eq!(def["input_schema"]["required"][0], "action");
    }

    #[test]
    fn test_execute_create() {
        let mut mgr = CanvasManager::new();
        let input = json!({
            "action": "create",
            "title": "Test Panel",
            "content": "# Hello",
            "content_type": "markdown"
        });
        let result = execute_canvas_tool(&input, &mut mgr);
        assert!(result.starts_with("Created panel"));
        assert_eq!(mgr.list_panels().len(), 1);
    }

    #[test]
    fn test_execute_create_code_panel() {
        let mut mgr = CanvasManager::new();
        let input = json!({
            "action": "create",
            "title": "Code Example",
            "content": "fn main() {}",
            "content_type": "code",
            "language": "rust"
        });
        let result = execute_canvas_tool(&input, &mut mgr);
        assert!(result.contains("Created panel"));
        let panels = mgr.list_panels();
        assert_eq!(panels.len(), 1);
        assert_eq!(
            panels[0].content_type,
            PanelContentType::Code {
                language: "rust".to_string()
            }
        );
    }

    #[test]
    fn test_execute_update() {
        let mut mgr = CanvasManager::new();
        // Create first
        let create_input = json!({ "action": "create", "title": "T", "content": "old" });
        execute_canvas_tool(&create_input, &mut mgr);
        let panel_id = mgr.list_panels()[0].id.clone();

        // Update
        let update_input = json!({
            "action": "update",
            "panel_id": panel_id,
            "content": "new content"
        });
        let result = execute_canvas_tool(&update_input, &mut mgr);
        assert!(result.starts_with("Updated panel"));
        assert_eq!(mgr.get_panel(&panel_id).unwrap().content, "new content");
    }

    #[test]
    fn test_execute_update_missing_panel() {
        let mut mgr = CanvasManager::new();
        let input = json!({
            "action": "update",
            "panel_id": "nonexistent",
            "content": "x"
        });
        let result = execute_canvas_tool(&input, &mut mgr);
        assert!(result.contains("Error"));
    }

    #[test]
    fn test_execute_delete() {
        let mut mgr = CanvasManager::new();
        let create_input = json!({ "action": "create", "title": "T", "content": "" });
        execute_canvas_tool(&create_input, &mut mgr);
        let panel_id = mgr.list_panels()[0].id.clone();

        let delete_input = json!({ "action": "delete", "panel_id": panel_id });
        let result = execute_canvas_tool(&delete_input, &mut mgr);
        assert!(result.starts_with("Deleted panel"));
        assert_eq!(mgr.list_panels().len(), 0);
    }

    #[test]
    fn test_execute_clear() {
        let mut mgr = CanvasManager::new();
        execute_canvas_tool(
            &json!({ "action": "create", "title": "A", "content": "" }),
            &mut mgr,
        );
        execute_canvas_tool(
            &json!({ "action": "create", "title": "B", "content": "" }),
            &mut mgr,
        );
        assert_eq!(mgr.list_panels().len(), 2);

        let result = execute_canvas_tool(&json!({ "action": "clear" }), &mut mgr);
        assert_eq!(result, "Cleared all panels.");
        assert_eq!(mgr.list_panels().len(), 0);
    }

    #[test]
    fn test_execute_set_layout() {
        let mut mgr = CanvasManager::new();
        let input = json!({ "action": "set_layout", "layout": "grid" });
        let result = execute_canvas_tool(&input, &mut mgr);
        assert!(result.contains("grid"));
        assert_eq!(mgr.state.layout, CanvasLayout::Grid { columns: 2 });
    }

    #[test]
    fn test_execute_set_layout_stack() {
        let mut mgr = CanvasManager::new();
        let input = json!({ "action": "set_layout", "layout": "stack" });
        execute_canvas_tool(&input, &mut mgr);
        assert_eq!(mgr.state.layout, CanvasLayout::Stack);
    }

    #[test]
    fn test_execute_unknown_action() {
        let mut mgr = CanvasManager::new();
        let input = json!({ "action": "explode" });
        let result = execute_canvas_tool(&input, &mut mgr);
        assert!(result.contains("unknown action"));
    }

    #[test]
    fn test_execute_missing_action() {
        let mut mgr = CanvasManager::new();
        let input = json!({ "title": "no action" });
        let result = execute_canvas_tool(&input, &mut mgr);
        assert!(result.contains("missing required field"));
    }

    #[test]
    fn test_execute_update_missing_panel_id() {
        let mut mgr = CanvasManager::new();
        let input = json!({ "action": "update", "content": "x" });
        let result = execute_canvas_tool(&input, &mut mgr);
        assert!(result.contains("requires 'panel_id'"));
    }

    #[test]
    fn test_execute_update_missing_content() {
        let mut mgr = CanvasManager::new();
        let input = json!({ "action": "update", "panel_id": "p1" });
        let result = execute_canvas_tool(&input, &mut mgr);
        assert!(result.contains("requires 'content'"));
    }

    #[test]
    fn test_execute_delete_missing_panel_id() {
        let mut mgr = CanvasManager::new();
        let input = json!({ "action": "delete" });
        let result = execute_canvas_tool(&input, &mut mgr);
        assert!(result.contains("requires 'panel_id'"));
    }

    #[test]
    fn test_canvas_operation_serialization() {
        let op = CanvasOperation::CreatePanel {
            title: "Test".to_string(),
            content_type: PanelContentType::Markdown,
            content: "Hello".to_string(),
            position: None,
            size: None,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("create_panel"));
    }

    #[test]
    fn test_canvas_event_serialization() {
        let event = CanvasEvent::PanelRemoved {
            id: "p1".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("panel_removed"));
        assert!(json.contains("p1"));
    }
}
