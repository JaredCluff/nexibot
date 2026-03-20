//! Built-in nexibot_soul tool — allows Claude to read and modify its own soul (identity).

use serde_json::{json, Value};
use tracing::{info, warn};

use crate::soul::Soul;

/// Get the tool definition to pass to Claude alongside other tools.
pub fn nexibot_soul_tool_definition() -> Value {
    json!({
        "name": "nexibot_soul",
        "description": "Read or update your SOUL.md file that defines your persistent identity, values, and personality. Use 'read' to view your current soul. Use 'update' to replace the soul content with new content you provide.",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "update"],
                    "description": "Action: 'read' to view current soul content, 'update' to replace the soul with new content"
                },
                "content": {
                    "type": "string",
                    "description": "The complete new soul content (required for 'update' action). Should be valid markdown."
                }
            },
            "required": ["action"]
        }
    })
}

/// Execute the nexibot_soul tool. Returns the tool result string.
pub fn execute_soul_tool(input: &Value) -> String {
    let action = input.get("action").and_then(|a| a.as_str()).unwrap_or("");

    match action {
        "read" => {
            info!("[SOUL_TOOL] Reading soul content");
            match Soul::load() {
                Ok(soul) => {
                    format!(
                        "Current SOUL.md content (version: {}, last modified: {}):\n\n{}",
                        soul.version, soul.last_modified, soul.content
                    )
                }
                Err(e) => {
                    warn!("[SOUL_TOOL] Failed to load soul: {}", e);
                    format!("Error reading soul: {}", e)
                }
            }
        }
        "update" => {
            let content = match input.get("content").and_then(|c| c.as_str()) {
                Some(c) if !c.trim().is_empty() => c.to_string(),
                _ => return "Error: 'content' field is required for 'update' action".to_string(),
            };

            info!(
                "[SOUL_TOOL] Updating soul content ({} chars)",
                content.len()
            );
            match Soul::load() {
                Ok(mut soul) => match soul.update(content) {
                    Ok(()) => {
                        info!(
                            "[SOUL_TOOL] Soul updated successfully (version: {})",
                            soul.version
                        );
                        format!(
                            "Soul updated successfully. New version: {}, last modified: {}",
                            soul.version, soul.last_modified
                        )
                    }
                    Err(e) => {
                        warn!("[SOUL_TOOL] Failed to update soul: {}", e);
                        format!("Error updating soul: {}", e)
                    }
                },
                Err(e) => {
                    warn!("[SOUL_TOOL] Failed to load soul for update: {}", e);
                    format!("Error loading soul: {}", e)
                }
            }
        }
        _ => format!(
            "Error: unknown action '{}'. Use 'read' or 'update'.",
            action
        ),
    }
}
