//! Google Gemini provider implementation.
//!
//! Supports Google's Gemini API for model inference.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use tracing::info;

use crate::claude::Message;
use crate::llm_provider::{LlmProvider, ProviderCapabilities};
use crate::session_overrides::SessionOverrides;

use super::{LlmClient, LlmMessageResult, LlmToolUse};

/// Google Gemini API client.
pub struct GoogleGeminiClient {
    model_id: String,
    api_key: String,
    http_client: reqwest::Client,
    max_tokens: usize,
    /// Whether to route requests through the bridge for logging and credential isolation.
    use_bridge: bool,
    /// Bridge URL (default: http://127.0.0.1:18790).
    bridge_url: String,
}

impl GoogleGeminiClient {
    pub fn new(model_id: &str, api_key: &str, max_tokens: usize) -> Self {
        Self {
            model_id: model_id.to_string(),
            api_key: api_key.to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            max_tokens,
            use_bridge: false,
            bridge_url: String::new(),
        }
    }

    /// Create a client that routes through the NexiBot bridge.
    pub fn via_bridge(model_id: &str, api_key: &str, bridge_url: &str, max_tokens: usize) -> Self {
        Self {
            model_id: model_id.to_string(),
            api_key: api_key.to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            max_tokens,
            use_bridge: true,
            bridge_url: bridge_url.to_string(),
        }
    }

    fn build_gemini_contents(
        system_prompt: &str,
        messages: &[Message],
    ) -> (serde_json::Value, Vec<serde_json::Value>) {
        let system_instruction = json!({
            "parts": [{ "text": system_prompt }]
        });

        let contents: Vec<serde_json::Value> = messages
            .iter()
            .map(|msg| {
                let role = match msg.role.as_str() {
                    "user" => "user",
                    "assistant" => "model",
                    _ => "user",
                };
                json!({
                    "role": role,
                    "parts": [{ "text": msg.content }]
                })
            })
            .collect();

        (system_instruction, contents)
    }

    /// Send a request through the NexiBot bridge instead of directly to the Gemini API.
    async fn send_via_bridge(
        &self,
        messages: &[Message],
        tools: &[serde_json::Value],
        system_prompt: &str,
        _streaming: bool,
    ) -> Result<LlmMessageResult> {
        let mut request_body = json!({
            "apiKey": self.api_key,
            "model": self.model_id,
            "max_tokens": self.max_tokens,
            "system": system_prompt,
            "messages": messages,
        });

        // Forward tools in Anthropic format. The bridge Google plugin converts
        // them to Gemini function declarations internally.
        if !tools.is_empty() {
            request_body["tools"] = json!(tools);
        }

        let endpoint = format!("{}/api/google/messages", self.bridge_url);
        info!("[GOOGLE] Sending request via bridge (model: {})", self.model_id);

        let mut req = self
            .http_client
            .post(&endpoint)
            .header("Content-Type", "application/json");
        if let Some(secret) = crate::bridge::get_bridge_secret() {
            req = req.header("x-bridge-secret", secret);
        }
        let response = req
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to bridge")?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            anyhow::bail!("Bridge error (HTTP {}): {}", status, error_text);
        }

        let resp: serde_json::Value = response.json().await?;
        let content = resp["content"].as_array().cloned().unwrap_or_default();

        let text = content
            .iter()
            .filter_map(|b| {
                if b["type"].as_str() == Some("text") {
                    b["text"].as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");

        let tool_uses = content
            .iter()
            .filter_map(|b| {
                if b["type"].as_str() == Some("tool_use") {
                    Some(LlmToolUse {
                        id: b["id"].as_str().unwrap_or("").to_string(),
                        name: b["name"].as_str().unwrap_or("").to_string(),
                        input: b["input"].clone(),
                    })
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        let stop_reason = resp["stop_reason"]
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
}

#[async_trait]
impl LlmClient for GoogleGeminiClient {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Google
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_thinking: false,
            supports_computer_use: false,
            supports_tools: true,
        }
    }

    async fn send_message_with_tools(
        &self,
        messages: &[Message],
        tools: &[serde_json::Value],
        system_prompt: &str,
        _overrides: &SessionOverrides,
    ) -> Result<LlmMessageResult> {
        // Route through bridge if configured
        if self.use_bridge {
            return self
                .send_via_bridge(messages, tools, system_prompt, false)
                .await;
        }

        let (system_instruction, contents) = Self::build_gemini_contents(system_prompt, messages);

        let mut request_body = json!({
            "system_instruction": system_instruction,
            "contents": contents,
            "generationConfig": {
                "maxOutputTokens": self.max_tokens,
            }
        });

        // Convert tools to Gemini format
        if !tools.is_empty() {
            let gemini_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool["name"],
                        "description": tool["description"],
                        "parameters": tool["input_schema"],
                    })
                })
                .collect();

            request_body["tools"] = json!([{
                "function_declarations": gemini_tools,
            }]);
        }

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            self.model_id
        );

        info!("[GOOGLE] Sending request (model: {})", self.model_id);

        let response = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to Gemini")?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            anyhow::bail!("Gemini error (HTTP {}): {}", status, error_text);
        }

        let resp: serde_json::Value = response.json().await?;

        let candidate = resp
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .ok_or_else(|| anyhow::anyhow!("Empty or missing 'candidates' in Gemini response"))?;
        let parts = candidate["content"]["parts"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut text = String::new();
        let mut tool_uses = Vec::new();

        for part in &parts {
            if let Some(t) = part["text"].as_str() {
                text.push_str(t);
            }
            if let Some(fc) = part.get("functionCall") {
                tool_uses.push(LlmToolUse {
                    id: format!("gemini-{}", tool_uses.len()),
                    name: fc["name"].as_str().unwrap_or("").to_string(),
                    input: fc["args"].clone(),
                });
            }
        }

        let stop_reason = if !tool_uses.is_empty() {
            "tool_use".to_string()
        } else {
            candidate["finishReason"]
                .as_str()
                .unwrap_or("STOP")
                .to_string()
        };

        Ok(LlmMessageResult {
            text: text.clone(),
            tool_uses,
            stop_reason,
            raw_content: vec![json!({ "type": "text", "text": text })],
            usage: None,
            model_used: self.model_id.clone(),
        })
    }

    async fn send_message_streaming_with_tools(
        &self,
        messages: &[Message],
        tools: &[serde_json::Value],
        system_prompt: &str,
        _overrides: &SessionOverrides,
        on_chunk: Box<dyn for<'a> Fn(&'a str) + Send + Sync + 'static>,
    ) -> Result<LlmMessageResult> {
        // Route through bridge if configured
        // TODO: Implement bridge streaming for Google provider.
        // For now, bridge routing falls back to non-streaming.
        if self.use_bridge {
            return self
                .send_via_bridge(messages, tools, system_prompt, true)
                .await;
        }

        let (system_instruction, contents) = Self::build_gemini_contents(system_prompt, messages);

        let mut request_body = json!({
            "system_instruction": system_instruction,
            "contents": contents,
            "generationConfig": {
                "maxOutputTokens": self.max_tokens,
            }
        });

        if !tools.is_empty() {
            let gemini_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool["name"],
                        "description": tool["description"],
                        "parameters": tool["input_schema"],
                    })
                })
                .collect();

            request_body["tools"] = json!([{
                "function_declarations": gemini_tools,
            }]);
        }

        // Gemini streaming uses streamGenerateContent with alt=sse
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.model_id
        );

        info!("[GOOGLE] Sending streaming request (model: {})", self.model_id);

        let mut response = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .json(&request_body)
            .send()
            .await
            .context("Failed to send streaming request to Gemini")?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            anyhow::bail!("Gemini streaming error (HTTP {}): {}", status, error_text);
        }

        let mut full_text = String::new();
        let mut tool_uses: Vec<LlmToolUse> = Vec::new();
        let mut stop_reason = "STOP".to_string();
        let mut line_buffer = String::new();
        const MAX_LINE_BUFFER: usize = 1024 * 1024; // 1 MB

        while let Some(chunk_result) = response.chunk().await.transpose() {
            let chunk_bytes = chunk_result.context("Failed to read Gemini streaming chunk")?;
            let chunk_text = String::from_utf8_lossy(&chunk_bytes);
            line_buffer.push_str(&chunk_text);
            if line_buffer.len() > MAX_LINE_BUFFER {
                anyhow::bail!("Gemini streaming line buffer exceeded 1 MB — possible malformed response");
            }

            // Process complete lines — Gemini SSE format: "data: {json}\n\n"
            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].trim_end_matches('\r').to_string();
                line_buffer = line_buffer[newline_pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                let data = if let Some(stripped) = line.strip_prefix("data: ") {
                    stripped
                } else {
                    continue;
                };

                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(candidates) = parsed.get("candidates").and_then(|c| c.as_array()) {
                        for candidate in candidates {
                            if let Some(parts) = candidate
                                .get("content")
                                .and_then(|c| c.get("parts"))
                                .and_then(|p| p.as_array())
                            {
                                for part in parts {
                                    if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                                        full_text.push_str(t);
                                        on_chunk(t);
                                    }
                                    if let Some(fc) = part.get("functionCall") {
                                        tool_uses.push(LlmToolUse {
                                            id: format!("gemini-{}", tool_uses.len()),
                                            name: fc["name"].as_str().unwrap_or("").to_string(),
                                            input: fc["args"].clone(),
                                        });
                                    }
                                }
                            }

                            if let Some(fr) = candidate
                                .get("finishReason")
                                .and_then(|f| f.as_str())
                            {
                                stop_reason = fr.to_string();
                            }
                        }
                    }
                }
            }
        }

        let final_stop = if !tool_uses.is_empty() {
            "tool_use".to_string()
        } else if stop_reason == "STOP" {
            "end_turn".to_string()
        } else {
            stop_reason
        };

        info!(
            "[GOOGLE] Streaming complete: {} chars, {} tool_uses (model: {})",
            full_text.len(),
            tool_uses.len(),
            self.model_id
        );

        Ok(LlmMessageResult {
            text: full_text.clone(),
            tool_uses,
            stop_reason: final_stop,
            raw_content: vec![json!({ "type": "text", "text": full_text })],
            usage: None,
            model_used: self.model_id.clone(),
        })
    }
}
