//! Tool schema conversion between Anthropic and OpenAI formats.
//!
//! Anthropic format: `{ name, description, input_schema }`
//! OpenAI format:    `{ type: "function", function: { name, description, parameters } }`
//!
//! Computer Use tools are filtered out for OpenAI since they are not supported.

use serde_json::{json, Value};

use crate::llm_provider::LlmProvider;

/// Convert an array of Anthropic-format tool definitions to the target provider's format.
/// For OpenAI, also filters out Computer Use tools (type starts with "computer_").
pub fn convert_tools(tools: &[Value], target_provider: LlmProvider) -> Vec<Value> {
    match target_provider {
        LlmProvider::Anthropic => tools.to_vec(),
        LlmProvider::Ollama | LlmProvider::LMStudio | LlmProvider::OpenAI => tools
            .iter()
            .filter(|tool| {
                // Filter out Computer Use tools
                let tool_type = tool.get("type").and_then(|v| v.as_str()).unwrap_or("");
                !tool_type.starts_with("computer_")
            })
            .map(anthropic_tool_to_openai)
            .collect(),
        // Google, DeepSeek, Qwen, GitHubCopilot, MiniMax, and any future providers
        // default to OpenAI-compatible tool format.
        _ => tools
            .iter()
            .filter(|tool| {
                let tool_type = tool.get("type").and_then(|v| v.as_str()).unwrap_or("");
                !tool_type.starts_with("computer_")
            })
            .map(anthropic_tool_to_openai)
            .collect(),
    }
}

/// Convert a single Anthropic tool definition to OpenAI function-calling format.
fn anthropic_tool_to_openai(tool: &Value) -> Value {
    let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let description = tool
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let parameters = tool
        .get("input_schema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));

    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters,
        }
    })
}

/// Convert an OpenAI tool_call response block back to Anthropic tool_use format.
///
/// OpenAI: `{ id, type: "function", function: { name, arguments } }`
/// Anthropic: `{ type: "tool_use", id, name, input }`
pub fn openai_tool_call_to_internal(tool_call: &Value) -> Option<Value> {
    let id = tool_call.get("id")?.as_str()?;
    let function = tool_call.get("function")?;
    let name = function.get("name")?.as_str()?;
    let arguments_str = function.get("arguments")?.as_str().unwrap_or("{}");
    let input: Value = serde_json::from_str(arguments_str).unwrap_or(json!({}));

    Some(json!({
        "type": "tool_use",
        "id": id,
        "name": name,
        "input": input,
    }))
}

/// Convert Anthropic-format conversation messages to OpenAI-format messages.
///
/// Handles three key differences:
/// 1. Anthropic `user` messages with `tool_result` content → OpenAI `tool` role messages
/// 2. Anthropic `assistant` messages with `tool_use` blocks → OpenAI `assistant` with `tool_calls`
/// 3. Plain text messages are passed through unchanged
pub fn convert_messages_to_openai(
    system_prompt: &str,
    messages: &[crate::claude::Message],
) -> Vec<Value> {
    let mut openai_messages = vec![json!({
        "role": "system",
        "content": system_prompt,
    })];

    for msg in messages {
        // Try parsing content as JSON array (Anthropic content blocks)
        if let Ok(blocks) = serde_json::from_str::<Vec<Value>>(&msg.content) {
            if let Some(first) = blocks.first() {
                let block_type = first.get("type").and_then(|t| t.as_str()).unwrap_or("");

                // User message with tool_result blocks → OpenAI tool messages
                if block_type == "tool_result" && msg.role == "user" {
                    for block in &blocks {
                        let tool_call_id = block
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let content = block
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        openai_messages.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_call_id,
                            "content": content,
                        }));
                    }
                    continue;
                }

                // Assistant message with tool_use blocks → OpenAI assistant with tool_calls
                if block_type == "tool_use"
                    || (block_type == "text"
                        && blocks.iter().any(|b| {
                            b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                        }))
                {
                    if msg.role == "assistant" {
                        let mut text_parts = String::new();
                        let mut tool_calls = Vec::new();

                        for block in &blocks {
                            match block.get("type").and_then(|t| t.as_str()) {
                                Some("text") => {
                                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                        text_parts.push_str(t);
                                    }
                                }
                                Some("tool_use") => {
                                    let id =
                                        block.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                    let name =
                                        block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    let empty_obj = json!({});
                                    let input = block.get("input").unwrap_or(&empty_obj);
                                    tool_calls.push(json!({
                                        "id": id,
                                        "type": "function",
                                        "function": {
                                            "name": name,
                                            "arguments": serde_json::to_string(input).unwrap_or_default(),
                                        }
                                    }));
                                }
                                _ => {}
                            }
                        }

                        let mut assistant_msg = json!({ "role": "assistant" });
                        if !text_parts.is_empty() {
                            assistant_msg["content"] = json!(text_parts);
                        }
                        if !tool_calls.is_empty() {
                            assistant_msg["tool_calls"] = json!(tool_calls);
                        }
                        openai_messages.push(assistant_msg);
                        continue;
                    }
                }
            }
        }

        // Default: pass through as-is
        openai_messages.push(json!({
            "role": msg.role,
            "content": msg.content,
        }));
    }

    openai_messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_tools_anthropic_passthrough() {
        let tools = vec![json!({
            "name": "search",
            "description": "Search the web",
            "input_schema": { "type": "object", "properties": {} }
        })];
        let result = convert_tools(&tools, LlmProvider::Anthropic);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "search");
    }

    #[test]
    fn test_convert_tools_openai_format() {
        let tools = vec![json!({
            "name": "search",
            "description": "Search the web",
            "input_schema": { "type": "object", "properties": { "query": { "type": "string" } } }
        })];
        let result = convert_tools(&tools, LlmProvider::OpenAI);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "function");
        assert_eq!(result[0]["function"]["name"], "search");
        assert_eq!(result[0]["function"]["description"], "Search the web");
        assert!(result[0]["function"]["parameters"]["properties"]["query"].is_object());
    }

    #[test]
    fn test_convert_tools_filters_computer_use() {
        let tools = vec![
            json!({ "name": "search", "description": "Search", "input_schema": {} }),
            json!({ "type": "computer_20241022", "name": "computer", "description": "Computer Use" }),
        ];
        let result = convert_tools(&tools, LlmProvider::OpenAI);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["function"]["name"], "search");
    }

    #[test]
    fn test_openai_tool_call_to_internal() {
        let tool_call = json!({
            "id": "call_abc123",
            "type": "function",
            "function": {
                "name": "search",
                "arguments": "{\"query\": \"hello\"}"
            }
        });
        let result = openai_tool_call_to_internal(&tool_call).unwrap();
        assert_eq!(result["type"], "tool_use");
        assert_eq!(result["id"], "call_abc123");
        assert_eq!(result["name"], "search");
        assert_eq!(result["input"]["query"], "hello");
    }

    #[test]
    fn test_convert_tools_ollama_same_as_openai() {
        let tools = vec![json!({
            "name": "lookup",
            "description": "Look something up",
            "input_schema": { "type": "object", "properties": { "term": { "type": "string" } } }
        })];
        let openai_result = convert_tools(&tools, LlmProvider::OpenAI);
        let ollama_result = convert_tools(&tools, LlmProvider::Ollama);
        assert_eq!(openai_result, ollama_result);
    }

    #[test]
    fn test_convert_empty_tools() {
        let tools: Vec<Value> = vec![];
        assert!(convert_tools(&tools, LlmProvider::Anthropic).is_empty());
        assert!(convert_tools(&tools, LlmProvider::OpenAI).is_empty());
        assert!(convert_tools(&tools, LlmProvider::Ollama).is_empty());
    }

    #[test]
    fn test_tool_missing_name() {
        let tools = vec![json!({
            "description": "A tool with no name",
            "input_schema": { "type": "object", "properties": {} }
        })];
        let result = convert_tools(&tools, LlmProvider::OpenAI);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["function"]["name"], "");
    }

    #[test]
    fn test_tool_missing_description() {
        let tools = vec![json!({
            "name": "silent_tool",
            "input_schema": { "type": "object", "properties": {} }
        })];
        let result = convert_tools(&tools, LlmProvider::OpenAI);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["function"]["description"], "");
    }

    #[test]
    fn test_tool_null_input_schema() {
        let tools = vec![json!({
            "name": "no_params",
            "description": "Tool with no input_schema"
        })];
        let result = convert_tools(&tools, LlmProvider::OpenAI);
        assert_eq!(result.len(), 1);
        let params = &result[0]["function"]["parameters"];
        assert_eq!(params["type"], "object");
        assert!(params["properties"].is_object());
    }

    #[test]
    fn test_complex_nested_parameters() {
        let tools = vec![json!({
            "name": "complex",
            "description": "Complex tool",
            "input_schema": {
                "type": "object",
                "properties": {
                    "config": {
                        "type": "object",
                        "properties": {
                            "nested_array": {
                                "type": "array",
                                "items": { "type": "string" }
                            },
                            "nested_obj": {
                                "type": "object",
                                "properties": {
                                    "deep": { "type": "boolean" }
                                }
                            }
                        }
                    }
                }
            }
        })];
        let result = convert_tools(&tools, LlmProvider::OpenAI);
        let params = &result[0]["function"]["parameters"];
        assert_eq!(params["properties"]["config"]["type"], "object");
        assert_eq!(
            params["properties"]["config"]["properties"]["nested_array"]["type"],
            "array"
        );
        assert_eq!(
            params["properties"]["config"]["properties"]["nested_obj"]["properties"]["deep"]
                ["type"],
            "boolean"
        );
    }

    #[test]
    fn test_multiple_computer_use_filtered() {
        let tools = vec![
            json!({ "name": "search", "description": "Search", "input_schema": {} }),
            json!({ "type": "computer_20241022", "name": "computer", "description": "CU1" }),
            json!({ "type": "computer_20250101", "name": "computer2", "description": "CU2" }),
        ];
        let result = convert_tools(&tools, LlmProvider::OpenAI);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["function"]["name"], "search");
    }

    #[test]
    fn test_openai_tool_call_invalid_json_arguments() {
        let tool_call = json!({
            "id": "call_bad",
            "type": "function",
            "function": {
                "name": "broken",
                "arguments": "this is not json {{{{"
            }
        });
        // The function parses invalid JSON and falls back to json!({})
        let result = openai_tool_call_to_internal(&tool_call).unwrap();
        assert_eq!(result["type"], "tool_use");
        assert_eq!(result["name"], "broken");
        // input should be empty object (fallback)
        assert_eq!(result["input"], json!({}));
    }
}
