//! OpenAI-compatible provider implementation.
//!
//! Shared base for OpenAI, DeepSeek, Qwen, GitHub Copilot, MiniMax.
//! All these providers use the OpenAI API format with different base URLs.
#![allow(dead_code)]

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;

use crate::claude::Message;
use crate::llm_provider::{LlmProvider, ProviderCapabilities};
use crate::session_overrides::SessionOverrides;
use crate::tool_converter;

use super::{LlmClient, LlmMessageResult, LlmToolUse};

/// OpenAI-compatible API client.
///
/// Works with OpenAI, DeepSeek, Qwen, GitHub Copilot, MiniMax,
/// and any other provider that implements the OpenAI chat completions API.
pub struct OpenAICompatibleClient {
    model_id: String,
    provider: LlmProvider,
    api_key: String,
    base_url: String,
    http_client: reqwest::Client,
    max_tokens: usize,
    /// Whether to route through the Anthropic Bridge (OpenAI) or direct.
    use_bridge: bool,
    bridge_url: String,
}

impl OpenAICompatibleClient {
    /// Create a client that routes through the Anthropic Bridge (for OpenAI).
    pub fn via_bridge(model_id: &str, api_key: &str, bridge_url: &str, max_tokens: usize) -> Self {
        Self {
            model_id: model_id.to_string(),
            provider: LlmProvider::OpenAI,
            api_key: api_key.to_string(),
            base_url: String::new(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            max_tokens,
            use_bridge: true,
            bridge_url: bridge_url.to_string(),
        }
    }

    /// Create a direct client (for DeepSeek, Qwen, GitHub Copilot, MiniMax).
    pub fn direct(
        model_id: &str,
        provider: LlmProvider,
        api_key: &str,
        base_url: &str,
        max_tokens: usize,
    ) -> Self {
        Self {
            model_id: model_id.to_string(),
            provider,
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            max_tokens,
            use_bridge: false,
            bridge_url: String::new(),
        }
    }

    fn build_openai_messages(system_prompt: &str, messages: &[Message]) -> Vec<serde_json::Value> {
        crate::tool_converter::convert_messages_to_openai(system_prompt, messages)
    }
}

#[async_trait]
impl LlmClient for OpenAICompatibleClient {
    fn provider(&self) -> LlmProvider {
        self.provider
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
        if self.use_bridge {
            // Route through Anthropic Bridge for OpenAI
            let converted_tools = tool_converter::convert_tools(tools, LlmProvider::OpenAI);

            let mut request_body = json!({
                "apiKey": self.api_key,
                "model": self.model_id,
                "max_tokens": self.max_tokens,
                "system": system_prompt,
                "messages": messages,
            });

            if !converted_tools.is_empty() {
                request_body["tools"] = serde_json::Value::Array(converted_tools);
            }

            let response = self
                .http_client
                .post(format!("{}/api/openai/messages", self.bridge_url))
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
        } else {
            // Direct OpenAI-compatible API call
            let openai_messages = Self::build_openai_messages(system_prompt, messages);
            let converted_tools = tool_converter::convert_tools(tools, LlmProvider::OpenAI);

            let mut request_body = json!({
                "model": self.model_id,
                "messages": openai_messages,
                "max_tokens": self.max_tokens,
                "stream": false,
            });

            if !converted_tools.is_empty() {
                request_body["tools"] = serde_json::Value::Array(converted_tools);
            }

            let response = self
                .http_client
                .post(format!("{}/chat/completions", self.base_url))
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .context("Failed to send request")?;

            if !response.status().is_success() {
                let status = response.status().as_u16();
                let error_text = response.text().await?;
                anyhow::bail!("API error (HTTP {}): {}", status, error_text);
            }

            let resp: serde_json::Value = response.json().await?;
            let choice = &resp["choices"][0];
            let text = choice["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();

            let mut tool_uses = Vec::new();
            if let Some(tool_calls) = choice["message"]["tool_calls"].as_array() {
                for tc in tool_calls {
                    if let Some(tu) = tool_converter::openai_tool_call_to_internal(tc) {
                        tool_uses.push(LlmToolUse {
                            id: tu["id"].as_str().unwrap_or("").to_string(),
                            name: tu["name"].as_str().unwrap_or("").to_string(),
                            input: tu["input"].clone(),
                        });
                    }
                }
            }

            let stop_reason = if !tool_uses.is_empty() {
                "tool_use".to_string()
            } else {
                "end_turn".to_string()
            };

            let raw_content = vec![json!({ "type": "text", "text": &text })];
            Ok(LlmMessageResult {
                text,
                tool_uses,
                stop_reason,
                raw_content,
                usage: None,
                model_used: self.model_id.clone(),
            })
        }
    }

    async fn send_message_streaming_with_tools(
        &self,
        messages: &[Message],
        tools: &[serde_json::Value],
        system_prompt: &str,
        overrides: &SessionOverrides,
        _on_chunk: Box<dyn for<'a> Fn(&'a str) + Send + Sync + 'static>,
    ) -> Result<LlmMessageResult> {
        // For now, fall back to non-streaming
        self.send_message_with_tools(messages, tools, system_prompt, overrides)
            .await
    }
}
