//! Built-in nexibot_settings tool — allows Claude to view and modify NexiBot settings.

use serde_json::{json, Value};
use tracing::{info, warn};

use crate::config::NexiBotConfig;

/// Get the tool definition to pass to Claude alongside MCP and memory tools.
pub fn nexibot_settings_tool_definition() -> Value {
    json!({
        "name": "nexibot_settings",
        "description": "View or modify NexiBot settings. Use 'get' to view current settings, 'set' to change a setting.",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["get", "set"],
                    "description": "Action: 'get' to view settings, 'set' to change a setting"
                },
                "key": {
                    "type": "string",
                    "description": "Setting key (e.g. 'claude.model', 'stt.backend', 'tts.backend', 'wakeword.wake_word')"
                },
                "value": {
                    "type": "string",
                    "description": "New value for the setting (required for 'set' action)"
                }
            },
            "required": ["action"]
        }
    })
}

/// Execute the nexibot_settings tool.
///
/// Returns `(result_text, changed)` where `changed=true` only when a setting
/// update was successfully persisted.
pub async fn execute_settings_tool(input: &Value, config: &mut NexiBotConfig) -> (String, bool) {
    let action = input.get("action").and_then(|a| a.as_str()).unwrap_or("");

    match action {
        "get" => {
            let key = input.get("key").and_then(|k| k.as_str());
            match key {
                Some(k) => {
                    info!("[SETTINGS_TOOL] Getting setting: {}", k);
                    match get_setting(config, k) {
                        Some(val) => (format!("{} = {}", k, val), false),
                        None => (format!("Unknown setting: {}", k), false),
                    }
                }
                None => {
                    info!("[SETTINGS_TOOL] Getting all settings");
                    (get_all_settings(config), false)
                }
            }
        }
        "set" => {
            let key = match input.get("key").and_then(|k| k.as_str()) {
                Some(k) => k,
                None => return ("Error: 'key' is required for 'set' action".to_string(), false),
            };
            let value = match input.get("value").and_then(|v| v.as_str()) {
                Some(v) => v,
                None => return ("Error: 'value' is required for 'set' action".to_string(), false),
            };

            info!("[SETTINGS_TOOL] Setting {} = {}", key, value);
            let previous = config.clone();
            match set_setting(config, key, value) {
                Ok(msg) => {
                    if let Err(e) = config.save() {
                        *config = previous;
                        warn!("[SETTINGS_TOOL] Failed to save config: {}", e);
                        return (format!("Failed to save setting: {}", e), false);
                    }
                    (msg, true)
                }
                Err(e) => (e, false),
            }
        }
        _ => (
            format!("Error: unknown action '{}'. Use 'get' or 'set'.", action),
            false,
        ),
    }
}

fn get_setting(config: &NexiBotConfig, key: &str) -> Option<String> {
    match key {
        "claude.model" => Some(config.claude.model.clone()),
        "claude.system_prompt" => Some(config.claude.system_prompt.clone()),
        "wakeword.wake_word" => Some(config.wakeword.wake_word.clone()),
        "wakeword.enabled" => Some(config.wakeword.enabled.to_string()),
        "stt.backend" => Some(config.stt.backend.clone()),
        "tts.backend" => Some(config.tts.backend.clone()),
        "tts.macos_voice" => Some(config.tts.macos_voice.clone()),
        _ => None,
    }
}

fn get_all_settings(config: &NexiBotConfig) -> String {
    let mut output = String::from("Current NexiBot settings:\n\n");
    output.push_str(&format!("claude.model = {}\n", config.claude.model));
    output.push_str(&format!(
        "wakeword.wake_word = {}\n",
        config.wakeword.wake_word
    ));
    output.push_str(&format!("wakeword.enabled = {}\n", config.wakeword.enabled));
    output.push_str(&format!("stt.backend = {}\n", config.stt.backend));
    output.push_str(&format!("tts.backend = {}\n", config.tts.backend));
    output.push_str(&format!("tts.macos_voice = {}\n", config.tts.macos_voice));
    output
}

fn set_setting(config: &mut NexiBotConfig, key: &str, value: &str) -> Result<String, String> {
    // Block security-critical settings from LLM modification.
    // Use case-insensitive comparison to prevent bypass via "Guardrails.x" etc.
    let key_lower = key.to_lowercase();
    if key_lower.starts_with("guardrails.") || key_lower.starts_with("defense.") || key_lower.starts_with("security.") {
        return Err("Security-critical settings can only be changed through the desktop UI.".to_string());
    }
    match key {
        "claude.model" => {
            let model = value.to_string();
            let valid_prefixes = [
                "claude-", "gpt-", "o1-", "o3-", "o4-", "gemini-", "llama",
                "mistral", "qwen", "deepseek", "cerebras/", "openai/", "anthropic/",
            ];
            if model.is_empty()
                || (!valid_prefixes.iter().any(|p| model.starts_with(p)) && !model.contains('/'))
            {
                return Err(format!(
                    "Invalid model name '{}'. Must start with a known provider prefix or use provider/model format.",
                    model
                ));
            }
            config.claude.model = model.clone();
            Ok(format!("Model set to: {}", model))
        }
        "wakeword.wake_word" => {
            config.wakeword.wake_word = value.to_string();
            Ok(format!("Wake word set to: {}", value))
        }
        "stt.backend" => {
            config.stt.backend = value.to_string();
            Ok(format!("STT backend set to: {}", value))
        }
        "tts.backend" => {
            config.tts.backend = value.to_string();
            Ok(format!("TTS backend set to: {}", value))
        }
        "tts.macos_voice" => {
            config.tts.macos_voice = value.to_string();
            Ok(format!("macOS voice set to: {}", value))
        }
        _ => Err(format!("Unknown or read-only setting: {}", key)),
    }
}
