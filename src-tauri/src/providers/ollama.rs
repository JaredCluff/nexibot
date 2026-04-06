//! Ollama provider implementation.
//!
//! Extracted from claude.rs Ollama-specific methods.
//! Communicates directly with Ollama (no bridge) using the OpenAI-compatible API.
#![allow(dead_code)]

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use tracing::info;

use crate::claude::Message;
use crate::llm_provider::{LlmProvider, ProviderCapabilities};
use crate::session_overrides::SessionOverrides;
use crate::tool_converter;

use super::{LlmClient, LlmMessageResult, LlmToolUse};

/// Ollama client for local model inference.
pub struct OllamaClient {
    model_id: String,
    ollama_url: String,
    http_client: reqwest::Client,
}

impl OllamaClient {
    pub fn new(model_id: &str, ollama_url: &str) -> Self {
        // Strip ollama/ prefix if present
        let clean_model = model_id.strip_prefix("ollama/").unwrap_or(model_id);

        Self {
            model_id: clean_model.to_string(),
            ollama_url: ollama_url.to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    fn build_openai_messages(system_prompt: &str, messages: &[Message]) -> Vec<serde_json::Value> {
        crate::tool_converter::convert_messages_to_openai(system_prompt, messages)
    }
}

#[async_trait]
impl LlmClient for OllamaClient {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Ollama
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
        let openai_messages = Self::build_openai_messages(system_prompt, messages);
        let converted_tools = tool_converter::convert_tools(tools, LlmProvider::Ollama);

        let mut request_body = json!({
            "model": self.model_id,
            "messages": openai_messages,
            "stream": false,
        });

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        info!(
            "[OLLAMA] Sending request to {} (model: {})",
            self.ollama_url, self.model_id
        );

        let response = self
            .http_client
            .post(format!("{}/v1/chat/completions", self.ollama_url))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to Ollama")?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            anyhow::bail!("Ollama error (HTTP {}): {}", status, error_text);
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
        // Ollama streaming: use OpenAI-compatible streaming endpoint
        let openai_messages = Self::build_openai_messages(system_prompt, messages);
        let converted_tools = tool_converter::convert_tools(tools, LlmProvider::Ollama);

        let mut request_body = json!({
            "model": self.model_id,
            "messages": openai_messages,
            "stream": true,
        });

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        let mut response = self
            .http_client
            .post(format!("{}/v1/chat/completions", self.ollama_url))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send streaming request to Ollama")?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            anyhow::bail!("Ollama error (HTTP {}): {}", status, error_text);
        }

        let mut full_text = String::new();
        let mut tool_uses: Vec<LlmToolUse> = Vec::new();
        // Accumulate streaming tool_calls: index -> (id, name, arguments_json)
        let mut tool_call_accum: std::collections::HashMap<usize, (String, String, String)> =
            std::collections::HashMap::new();

        let mut line_buffer = String::new();
        const MAX_LINE_BUFFER: usize = 1024 * 1024; // 1 MB

        while let Some(chunk) = response.chunk().await? {
            let chunk_text = String::from_utf8_lossy(&chunk);
            line_buffer.push_str(&chunk_text);
            if line_buffer.len() > MAX_LINE_BUFFER {
                anyhow::bail!("Ollama streaming line buffer exceeded 1 MB — possible malformed response");
            }

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
                    // Text content
                    if let Some(delta_text) = parsed["choices"][0]["delta"]["content"].as_str() {
                        let delta_owned = delta_text.to_string();
                        full_text.push_str(&delta_owned);
                        on_chunk(&delta_owned);
                    }
                    // Tool calls (streamed incrementally)
                    if let Some(tool_calls) = parsed["choices"][0]["delta"]["tool_calls"].as_array()
                    {
                        for tc in tool_calls {
                            let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                            let entry = tool_call_accum
                                .entry(idx)
                                .or_insert_with(|| (String::new(), String::new(), String::new()));
                            if let Some(id) = tc["id"].as_str() {
                                entry.0 = id.to_string();
                            }
                            if let Some(name) = tc["function"]["name"].as_str() {
                                entry.1 = name.to_string();
                            }
                            if let Some(args) = tc["function"]["arguments"].as_str() {
                                entry.2.push_str(args);
                            }
                        }
                    }
                }
            }
        }

        // Convert accumulated tool calls to LlmToolUse
        for (_idx, (id, name, args_json)) in tool_call_accum {
            if !name.is_empty() {
                let input: serde_json::Value =
                    serde_json::from_str(&args_json).unwrap_or(json!({}));
                tool_uses.push(LlmToolUse {
                    id: if id.is_empty() {
                        format!("call_{}", uuid::Uuid::new_v4())
                    } else {
                        id
                    },
                    name,
                    input,
                });
            }
        }

        let stop_reason = if !tool_uses.is_empty() {
            "tool_use".to_string()
        } else {
            "end_turn".to_string()
        };

        Ok(LlmMessageResult {
            text: full_text.clone(),
            tool_uses,
            stop_reason,
            raw_content: vec![json!({ "type": "text", "text": full_text })],
            usage: None,
            model_used: self.model_id.clone(),
        })
    }
}
