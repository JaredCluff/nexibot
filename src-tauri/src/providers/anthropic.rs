//! Anthropic provider implementation.
//!
//! Extracted from claude.rs Anthropic-specific paths.
//! Routes through the Anthropic Bridge Service for OAuth support.
#![allow(dead_code)]

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use tracing::warn;

use crate::claude::Message;
use crate::llm_provider::{LlmProvider, ProviderCapabilities};
use crate::session_overrides::SessionOverrides;
use crate::tool_converter;

use super::{LlmClient, LlmMessageResult, LlmToolUse};

/// Process-wide lock that serialises the OAuth load → refresh → save sequence.
/// Without this, concurrent requests can each refresh the token independently and
/// the last writer wins, silently discarding refreshed tokens from other callers.
static AUTH_PROFILE_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> =
    std::sync::OnceLock::new();

/// Anthropic API client that routes through the Anthropic Bridge Service.
pub struct AnthropicClient {
    model_id: String,
    http_client: reqwest::Client,
    bridge_url: String,
    max_tokens: usize,
}

impl AnthropicClient {
    pub fn new(model_id: &str, bridge_url: &str, max_tokens: usize) -> Self {
        Self {
            model_id: model_id.to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .tcp_keepalive(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            bridge_url: bridge_url.to_string(),
            max_tokens,
        }
    }

    /// Get auth token (delegates to the same flow as ClaudeClient).
    async fn get_auth_token(&self) -> Result<String> {
        use crate::oauth::AuthProfileManager;

        // 1. Try OAuth profile — serialised with a process-wide lock to prevent
        //    a race where multiple concurrent callers each refresh and save the
        //    token, causing last-write-wins token loss.
        {
            let mutex = AUTH_PROFILE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
            let _guard = mutex.lock().await;
            if let Ok(mut manager) = AuthProfileManager::load() {
                if let Some(profile) = manager.get_default_profile("anthropic") {
                    match profile.get_valid_token().await {
                        Ok(token) => {
                            if let Err(e) = manager.save() {
                                warn!(
                                    "[ANTHROPIC] Failed to persist refreshed OAuth profile after token refresh: {}",
                                    e
                                );
                            }
                            return Ok(token);
                        }
                        Err(e) => {
                            warn!("[ANTHROPIC] OAuth token failed: {}", e);
                        }
                    }
                }
            }
        } // _guard drops here, releasing the lock

        // 2. Try Claude Code keychain (reuse ClaudeClient static method via direct import)
        // This is a fallback — for now just check env/config
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            return Ok(key);
        }

        anyhow::bail!("No Anthropic authentication available")
    }

    fn has_computer_use_tools(tools: &[serde_json::Value]) -> bool {
        tools.iter().any(|tool| {
            tool["type"]
                .as_str()
                .is_some_and(|t| t.starts_with("computer_"))
        })
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Anthropic
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_thinking: true,
            supports_computer_use: true,
            supports_tools: true,
        }
    }

    async fn send_message_with_tools(
        &self,
        messages: &[Message],
        tools: &[serde_json::Value],
        system_prompt: &str,
        overrides: &SessionOverrides,
    ) -> Result<LlmMessageResult> {
        let api_key = self.get_auth_token().await?;
        let converted_tools = tool_converter::convert_tools(tools, LlmProvider::Anthropic);

        let mut request_body = json!({
            "apiKey": api_key,
            "model": self.model_id,
            "max_tokens": self.max_tokens,
            "system": system_prompt,
            "messages": messages,
        });

        if let Some(budget) = overrides.thinking_budget {
            request_body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget,
            });
        }

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        if Self::has_computer_use_tools(tools) {
            request_body["betas"] = json!(["computer-use-2025-01-24"]);
        }

        let response = self
            .http_client
            .post(format!("{}/api/messages", self.bridge_url))
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to Bridge")?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            anyhow::bail!("Bridge error (HTTP {}): {}", status, error_text);
        }

        let claude_response: serde_json::Value = response.json().await?;

        let content = claude_response["content"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let text = extract_text(&content);
        let tool_uses = extract_tool_uses(&content);
        let stop_reason = claude_response["stop_reason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_string();

        Ok(LlmMessageResult {
            text,
            tool_uses,
            stop_reason,
            raw_content: content,
            usage: None,
            model_used: self.model_id.clone(),
        })
    }

    async fn send_message_streaming_with_tools(
        &self,
        messages: &[Message],
        tools: &[serde_json::Value],
        system_prompt: &str,
        overrides: &SessionOverrides,
        on_chunk: Box<dyn for<'a> Fn(&'a str) + Send + Sync + 'static>,
    ) -> Result<LlmMessageResult> {
        let api_key = self.get_auth_token().await?;
        let converted_tools = tool_converter::convert_tools(tools, LlmProvider::Anthropic);

        let mut request_body = json!({
            "apiKey": api_key,
            "model": self.model_id,
            "max_tokens": self.max_tokens,
            "system": system_prompt,
            "messages": messages,
        });

        if let Some(budget) = overrides.thinking_budget {
            request_body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget,
            });
        }

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        if Self::has_computer_use_tools(tools) {
            request_body["betas"] = json!(["computer-use-2025-01-24"]);
        }

        let response = self
            .http_client
            .post(format!("{}/api/messages/stream", self.bridge_url))
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send streaming request to Bridge")?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            anyhow::bail!("Bridge error (HTTP {}): {}", status, error_text);
        }

        parse_sse_stream(response, &self.model_id, on_chunk).await
    }
}

/// Parse an SSE stream from the Anthropic Bridge into an LlmMessageResult.
async fn parse_sse_stream(
    mut response: reqwest::Response,
    model_id: &str,
    on_chunk: Box<dyn for<'a> Fn(&'a str) + Send + Sync + 'static>,
) -> Result<LlmMessageResult> {
    let mut full_text = String::new();
    let mut raw_content: Vec<serde_json::Value> = Vec::new();
    let mut tool_uses: Vec<LlmToolUse> = Vec::new();
    let mut stop_reason = "end_turn".to_string();
    let mut current_tool_use: Option<(String, String, String)> = None;

    let mut line_buffer = String::new();

    while let Some(chunk) = response.chunk().await? {
        let chunk_text = String::from_utf8_lossy(&chunk);
        line_buffer.push_str(&chunk_text);

        // Process complete lines from the buffer
        while let Some(newline_pos) = line_buffer.find('\n') {
            let line = line_buffer[..newline_pos].trim_end_matches('\r').to_string();
            line_buffer = line_buffer[newline_pos + 1..].to_string();

            if !line.starts_with("data: ") {
                continue;
            }
            let data = &line[6..];
            if data == "[DONE]" {
                break;
            }

            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                let event_type = parsed["type"].as_str().unwrap_or("");
                match event_type {
                    "error" => {
                        let error_msg = parsed["error"]["message"]
                            .as_str()
                            .unwrap_or("Unknown error");
                        anyhow::bail!("Claude API error: {}", error_msg);
                    }
                    "content_block_start" => {
                        let block = &parsed["content_block"];
                        if block["type"].as_str() == Some("tool_use") {
                            let id = block["id"].as_str().unwrap_or("").to_string();
                            let name = block["name"].as_str().unwrap_or("").to_string();
                            current_tool_use = Some((id, name, String::new()));
                        }
                    }
                    "content_block_delta" => {
                        if let Some(delta_text) = parsed["delta"]["text"].as_str() {
                            full_text.push_str(delta_text);
                            on_chunk(delta_text);
                        }
                        if let Some(partial_json) = parsed["delta"]["partial_json"].as_str() {
                            if let Some(ref mut tu) = current_tool_use {
                                tu.2.push_str(partial_json);
                            }
                        }
                    }
                    "content_block_stop" => {
                        if let Some((id, name, input_json)) = current_tool_use.take() {
                            let input: serde_json::Value = if input_json.is_empty() {
                                json!({})
                            } else {
                                match serde_json::from_str(&input_json) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        warn!(
                                            "[ANTHROPIC] Failed to deserialize complete tool input JSON for tool '{}' (id={}): {}. Raw: {:?}",
                                            name, id, e, &input_json[..input_json.len().min(200)]
                                        );
                                        json!({})
                                    }
                                }
                            };
                            raw_content.push(json!({
                                "type": "tool_use", "id": id, "name": name, "input": input,
                            }));
                            tool_uses.push(LlmToolUse { id, name, input });
                        }
                    }
                    "message_delta" => {
                        if let Some(sr) = parsed["delta"]["stop_reason"].as_str() {
                            stop_reason = sr.to_string();
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if !full_text.is_empty() {
        let mut final_content = vec![json!({ "type": "text", "text": full_text })];
        final_content.extend(raw_content.clone());
        raw_content = final_content;
    }

    Ok(LlmMessageResult {
        text: full_text,
        tool_uses,
        stop_reason,
        raw_content,
        usage: None,
        model_used: model_id.to_string(),
    })
}

/// Extract text content from Anthropic response content blocks.
fn extract_text(content: &[serde_json::Value]) -> String {
    content
        .iter()
        .filter_map(|block| {
            if block["type"].as_str() == Some("text") {
                block["text"].as_str().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect::<Vec<String>>()
        .join("")
}

/// Extract tool use blocks from Anthropic response content blocks.
fn extract_tool_uses(content: &[serde_json::Value]) -> Vec<LlmToolUse> {
    content
        .iter()
        .filter_map(|block| {
            if block["type"].as_str() == Some("tool_use") {
                Some(LlmToolUse {
                    id: block["id"].as_str().unwrap_or("").to_string(),
                    name: block["name"].as_str().unwrap_or("").to_string(),
                    input: block["input"].clone(),
                })
            } else {
                None
            }
        })
        .collect()
}
