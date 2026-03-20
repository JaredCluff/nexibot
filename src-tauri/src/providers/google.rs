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
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model_id, self.api_key
        );

        info!("[GOOGLE] Sending request (model: {})", self.model_id);

        let response = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
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

        let candidate = &resp["candidates"][0];
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
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?key={}&alt=sse",
            self.model_id, self.api_key
        );

        info!("[GOOGLE] Sending streaming request (model: {})", self.model_id);

        let mut response = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
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

        while let Some(chunk_result) = response.chunk().await.transpose() {
            let chunk_bytes = chunk_result.context("Failed to read Gemini streaming chunk")?;
            let chunk_text = String::from_utf8_lossy(&chunk_bytes);
            line_buffer.push_str(&chunk_text);

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
