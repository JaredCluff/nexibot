//! Computer Use API handler
//!
//! Handles Anthropic's built-in Computer Use tool type.
//! Computer Use tools are NOT MCP tools — they use a special tool type
//! that requires the `computer-use-2025-01-24` beta header.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use crate::native_control;

/// Display configuration for Computer Use
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub width: u32,
    pub height: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
        }
    }
}

/// Manager for Computer Use tool execution
#[allow(dead_code)]
pub struct ComputerUseManager {
    pub enabled: bool,
    pub display: DisplayConfig,
    pub require_confirmation: bool,
}

impl ComputerUseManager {
    pub fn new(enabled: bool, display: DisplayConfig, require_confirmation: bool) -> Self {
        Self {
            enabled,
            display,
            require_confirmation,
        }
    }

    /// Get the tool definition for the Claude API
    /// This is NOT an MCP tool — it uses the special computer_20251124 type
    #[allow(dead_code)]
    pub fn get_tool_definition(&self) -> serde_json::Value {
        json!({
            "type": "computer_20251124",
            "name": "computer",
            "display_width_px": self.display.width,
            "display_height_px": self.display.height,
        })
    }

    /// Check if a tool name is the Computer Use tool
    pub fn is_computer_use_tool(name: &str) -> bool {
        name == "computer"
    }

    /// Execute a Computer Use action.
    /// Returns NeedsConfirmation error if require_confirmation is true.
    pub async fn execute(&self, input: &serde_json::Value) -> Result<serde_json::Value> {
        self.execute_with_approval(input, false).await
    }

    /// Execute a Computer Use action with explicit approval state.
    ///
    /// `approval_granted=true` is used by the chat pipeline after a human has
    /// approved a pending confirmation request.
    pub async fn execute_with_approval(
        &self,
        input: &serde_json::Value,
        approval_granted: bool,
    ) -> Result<serde_json::Value> {
        let action = input
            .get("action")
            .and_then(|a| a.as_str())
            .context("Missing 'action' field in computer use input")?;

        // Check confirmation requirement before every action (except screenshots which are read-only)
        if self.require_confirmation
            && !approval_granted
            && action != "screenshot"
            && action != "cursor_position"
        {
            anyhow::bail!(
                "NEEDS_CONFIRMATION: Computer Use action '{}' requires user confirmation",
                action
            );
        }

        info!("[COMPUTER_USE] Executing action: {}", action);

        match action {
            "screenshot" => self.handle_screenshot().await,
            "mouse_move" => {
                let x = i32::try_from(
                    input["coordinate"][0].as_i64().context("Missing x coordinate")?,
                )
                .context("X coordinate out of i32 range")?;
                let y = i32::try_from(
                    input["coordinate"][1].as_i64().context("Missing y coordinate")?,
                )
                .context("Y coordinate out of i32 range")?;
                native_control::mouse_move(x, y)?;
                Ok(json!({"status": "ok", "action": "mouse_move", "x": x, "y": y}))
            }
            "left_click" => {
                let (x, y) = self.get_optional_coords(input);
                if let (Some(x), Some(y)) = (x, y) {
                    native_control::mouse_move(x, y)?;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                native_control::left_click()?;
                Ok(json!({"status": "ok", "action": "left_click"}))
            }
            "right_click" => {
                let (x, y) = self.get_optional_coords(input);
                if let (Some(x), Some(y)) = (x, y) {
                    native_control::mouse_move(x, y)?;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                native_control::right_click()?;
                Ok(json!({"status": "ok", "action": "right_click"}))
            }
            "double_click" => {
                let (x, y) = self.get_optional_coords(input);
                if let (Some(x), Some(y)) = (x, y) {
                    native_control::mouse_move(x, y)?;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                native_control::double_click()?;
                Ok(json!({"status": "ok", "action": "double_click"}))
            }
            "type" => {
                let text = input
                    .get("text")
                    .and_then(|t| t.as_str())
                    .context("Missing 'text' field for type action")?;
                native_control::type_text(text)?;
                Ok(json!({"status": "ok", "action": "type", "length": text.len()}))
            }
            "key" => {
                let key = input
                    .get("key")
                    .and_then(|k| k.as_str())
                    .context("Missing 'key' field for key action")?;
                native_control::key_press(key)?;
                Ok(json!({"status": "ok", "action": "key", "key": key}))
            }
            "scroll" => {
                let (x, y) = self.get_optional_coords(input);
                if let (Some(x), Some(y)) = (x, y) {
                    native_control::mouse_move(x, y)?;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                let delta_x = i32::try_from(
                    input.get("delta_x").and_then(|d| d.as_i64()).unwrap_or(0),
                )
                .context("delta_x out of i32 range")?;
                let delta_y = i32::try_from(
                    input.get("delta_y").and_then(|d| d.as_i64()).unwrap_or(0),
                )
                .context("delta_y out of i32 range")?;
                native_control::scroll(delta_x, delta_y)?;
                Ok(
                    json!({"status": "ok", "action": "scroll", "delta_x": delta_x, "delta_y": delta_y}),
                )
            }
            "cursor_position" => {
                let (x, y) = native_control::cursor_position()?;
                Ok(json!({"status": "ok", "action": "cursor_position", "x": x, "y": y}))
            }
            _ => {
                warn!("[COMPUTER_USE] Unknown action: {}", action);
                Ok(json!({"error": format!("Unknown action: {}", action)}))
            }
        }
    }

    async fn handle_screenshot(&self) -> Result<serde_json::Value> {
        let base64_data = native_control::screenshot_base64()?;
        Ok(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": base64_data,
            }
        }))
    }

    fn get_optional_coords(&self, input: &serde_json::Value) -> (Option<i32>, Option<i32>) {
        if let Some(coord) = input.get("coordinate") {
            let x = coord.get(0).and_then(|v| v.as_i64()).map(|v| v as i32);
            let y = coord.get(1).and_then(|v| v.as_i64()).map(|v| v as i32);
            (x, y)
        } else {
            (None, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manager(require_confirmation: bool) -> ComputerUseManager {
        ComputerUseManager::new(true, DisplayConfig::default(), require_confirmation)
    }

    #[tokio::test]
    async fn execute_requires_confirmation_without_approval() {
        let mgr = test_manager(true);
        let input = serde_json::json!({ "action": "unknown_action" });
        let err = mgr.execute(&input).await.unwrap_err().to_string();
        assert!(err.starts_with("NEEDS_CONFIRMATION:"));
    }

    #[tokio::test]
    async fn execute_with_approval_bypasses_confirmation_gate() {
        let mgr = test_manager(true);
        let input = serde_json::json!({ "action": "unknown_action" });
        let out = mgr.execute_with_approval(&input, true).await.unwrap();
        assert_eq!(out["error"], "Unknown action: unknown_action");
    }
}
