//! Built-in nexibot_memory tool — allows Claude to remember and recall information.

use serde_json::{json, Value};
use tracing::{info, warn};

use crate::memory::{MemoryManager, MemoryType};

/// Get the tool definition to pass to Claude alongside MCP and settings tools.
pub fn nexibot_memory_tool_definition() -> Value {
    json!({
        "name": "nexibot_memory",
        "description": "Save or recall memories. Use 'remember' to store important facts, preferences, or context about the user. Use 'recall' to search for previously stored memories. Use 'list' to see all stored memories.",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["remember", "recall", "list"],
                    "description": "Action: 'remember' stores a memory, 'recall' searches memories, 'list' shows all memories"
                },
                "content": {
                    "type": "string",
                    "description": "The content to remember (required for 'remember' action)"
                },
                "memory_type": {
                    "type": "string",
                    "enum": ["preference", "fact", "context"],
                    "description": "Type of memory: 'preference' for user preferences, 'fact' for important facts, 'context' for general context"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tags to categorize the memory"
                },
                "query": {
                    "type": "string",
                    "description": "Search query (required for 'recall' action)"
                }
            },
            "required": ["action"]
        }
    })
}

/// Execute the nexibot_memory tool. Returns the tool result string.
pub fn execute_memory_tool(input: &Value, memory_manager: &mut MemoryManager) -> String {
    let action = input.get("action").and_then(|a| a.as_str()).unwrap_or("");

    match action {
        "remember" => {
            let content = match input.get("content").and_then(|c| c.as_str()) {
                Some(c) if !c.trim().is_empty() => c.to_string(),
                _ => return "Error: 'content' field is required for 'remember' action".to_string(),
            };

            let memory_type = match input.get("memory_type").and_then(|t| t.as_str()) {
                Some("preference") => MemoryType::Preference,
                Some("fact") => MemoryType::Fact,
                Some("context") | None => MemoryType::Context,
                Some(other) => {
                    warn!("[MEMORY_TOOL] Unknown memory_type: {}", other);
                    MemoryType::Context
                }
            };

            let tags: Vec<String> = input
                .get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            match memory_manager.add_memory(content.clone(), memory_type, tags) {
                Ok(id) => {
                    info!("[MEMORY_TOOL] Stored memory: {} (id: {})", content, id);
                    format!("Remembered: \"{}\" (id: {})", content, id)
                }
                Err(e) => {
                    warn!("[MEMORY_TOOL] Failed to store memory: {}", e);
                    format!("Error storing memory: {}", e)
                }
            }
        }
        "recall" => {
            let query = match input.get("query").and_then(|q| q.as_str()) {
                Some(q) if !q.trim().is_empty() => q,
                _ => return "Error: 'query' field is required for 'recall' action".to_string(),
            };

            let results = memory_manager.search_memories(query);

            if results.is_empty() {
                info!("[MEMORY_TOOL] No memories found for query: {}", query);
                format!("No memories found matching \"{}\"", query)
            } else {
                info!(
                    "[MEMORY_TOOL] Found {} memories for query: {}",
                    results.len(),
                    query
                );
                let mut output = format!(
                    "Found {} memories matching \"{}\":\n\n",
                    results.len(),
                    query
                );
                for (i, memory) in results.iter().enumerate().take(10) {
                    output.push_str(&format!(
                        "{}. [{}] {} (tags: {})\n",
                        i + 1,
                        format!("{:?}", memory.memory_type).to_lowercase(),
                        memory.content,
                        if memory.tags.is_empty() {
                            "none".to_string()
                        } else {
                            memory.tags.join(", ")
                        }
                    ));
                }
                output
            }
        }
        "list" => {
            let preferences = memory_manager.get_memories_by_type(MemoryType::Preference);
            let facts = memory_manager.get_memories_by_type(MemoryType::Fact);
            let contexts = memory_manager.get_memories_by_type(MemoryType::Context);

            let total = preferences.len() + facts.len() + contexts.len();
            if total == 0 {
                info!("[MEMORY_TOOL] No memories stored");
                return "No memories stored yet.".to_string();
            }

            info!("[MEMORY_TOOL] Listing {} memories", total);
            let mut output = format!("Total memories: {}\n\n", total);

            if !preferences.is_empty() {
                output.push_str(&format!("Preferences ({}):\n", preferences.len()));
                for pref in preferences.iter().take(20) {
                    output.push_str(&format!("  - {}\n", pref.content));
                }
                output.push('\n');
            }

            if !facts.is_empty() {
                output.push_str(&format!("Facts ({}):\n", facts.len()));
                for fact in facts.iter().take(20) {
                    output.push_str(&format!("  - {}\n", fact.content));
                }
                output.push('\n');
            }

            if !contexts.is_empty() {
                output.push_str(&format!("Context ({}):\n", contexts.len()));
                for ctx in contexts.iter().take(20) {
                    output.push_str(&format!("  - {}\n", ctx.content));
                }
            }

            output
        }
        _ => {
            format!(
                "Error: unknown action '{}'. Use 'remember', 'recall', or 'list'.",
                action
            )
        }
    }
}
