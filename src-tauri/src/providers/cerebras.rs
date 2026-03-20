//! Cerebras provider implementation.
//!
//! Cerebras provides fast inference via an OpenAI-compatible API.
//! API base: https://api.cerebras.ai/v1
//! Supports Llama and GPT-OSS models with optional reasoning output.
#![allow(dead_code)]

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;

use crate::claude::Message;
use crate::llm_provider::{LlmProvider, ProviderCapabilities};
use crate::session_overrides::SessionOverrides;
use crate::tool_converter;

use super::{LlmClient, LlmMessageResult, LlmToolUse};

/// Cerebras API client.
///
/// Uses OpenAI-compatible chat completions API at https://api.cerebras.ai/v1
pub struct CerebrasClient {
    model_id: String,
    api_key: String,
    base_url: String,
    http_client: reqwest::Client,
    max_tokens: usize,
}

impl CerebrasClient {
    /// Create a new Cerebras client.
    pub fn new(model_id: &str, api_key: &str, max_tokens: usize) -> Self {
        Self {
            model_id: model_id.to_string(),
            api_key: api_key.to_string(),
            base_url: "https://api.cerebras.ai/v1".to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            max_tokens,
        }
    }

    /// The model ID to send in API requests (strip cerebras/ prefix).
    fn api_model_id(&self) -> &str {
        self.model_id
            .strip_prefix("cerebras/")
            .unwrap_or(&self.model_id)
    }

    fn build_openai_messages(system_prompt: &str, messages: &[Message]) -> Vec<serde_json::Value> {
        crate::tool_converter::convert_messages_to_openai(system_prompt, messages)
    }

    /// Build the raw_content array, prepending reasoning as a thinking block if present.
    fn build_raw_content(text: &str, reasoning: &str) -> Vec<serde_json::Value> {
        let mut raw_content = Vec::new();
        if !reasoning.is_empty() {
            raw_content.push(json!({ "type": "thinking", "thinking": reasoning }));
        }
        raw_content.push(json!({ "type": "text", "text": text }));
        raw_content
    }
}

#[async_trait]
impl LlmClient for CerebrasClient {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Cerebras
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
        let converted_tools = tool_converter::convert_tools(tools, LlmProvider::OpenAI);

        let mut request_body = json!({
            "model": self.api_model_id(),
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
            .context("Failed to send request to Cerebras")?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            anyhow::bail!("Cerebras API error (HTTP {}): {}", status, error_text);
        }

        let resp: serde_json::Value = response.json().await?;
        let choice = &resp["choices"][0];
        let content = choice["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        // gpt-oss-120b returns a `reasoning` field with chain-of-thought
        let reasoning = choice["message"]["reasoning"]
            .as_str()
            .unwrap_or("")
            .to_string();

        // If content is empty but reasoning is present, use reasoning as the response text
        let text = if content.is_empty() && !reasoning.is_empty() {
            reasoning.clone()
        } else {
            content
        };

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

        let raw_content = Self::build_raw_content(&text, &reasoning);
        Ok(LlmMessageResult {
            text,
            tool_uses,
            stop_reason,
            raw_content,
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
        let openai_messages = Self::build_openai_messages(system_prompt, messages);
        let converted_tools = tool_converter::convert_tools(tools, LlmProvider::OpenAI);

        let mut request_body = json!({
            "model": self.api_model_id(),
            "messages": openai_messages,
            "max_tokens": self.max_tokens,
            "stream": true,
        });

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        let mut response = self
            .http_client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send streaming request to Cerebras")?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            anyhow::bail!("Cerebras API error (HTTP {}): {}", status, error_text);
        }

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls_map: std::collections::HashMap<
            u64,
            (String, String, String),
        > = std::collections::HashMap::new(); // index -> (id, name, arguments)

        // Parse SSE stream in real-time (chunked, not buffered)
        let mut line_buffer = String::new();

        while let Some(chunk_bytes) = response.chunk().await? {
            let chunk_text = String::from_utf8_lossy(&chunk_bytes);
            line_buffer.push_str(&chunk_text);

            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].trim().to_string();
                line_buffer = line_buffer[newline_pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                let data = if let Some(d) = line.strip_prefix("data: ") {
                    d.trim()
                } else {
                    continue;
                };

                if data == "[DONE]" {
                    break;
                }

                let chunk: serde_json::Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

            let delta = &chunk["choices"][0]["delta"];

            // Content text
            if let Some(content) = delta["content"].as_str() {
                if !content.is_empty() {
                    on_chunk(content);
                    full_text.push_str(content);
                }
            }

            // Reasoning (gpt-oss-120b) — stream to UI if no content is being produced
            if let Some(reasoning) = delta["reasoning"].as_str() {
                if !reasoning.is_empty() {
                    full_reasoning.push_str(reasoning);
                    // Stream reasoning as visible text when content is empty
                    if full_text.is_empty() {
                        on_chunk(reasoning);
                    }
                }
            }

            // Tool calls (accumulated across chunks)
            if let Some(tcs) = delta["tool_calls"].as_array() {
                for tc in tcs {
                    let idx = tc["index"].as_u64().unwrap_or(0);
                    let entry = tool_calls_map.entry(idx).or_insert_with(|| {
                        (
                            tc["id"].as_str().unwrap_or("").to_string(),
                            tc["function"]["name"].as_str().unwrap_or("").to_string(),
                            String::new(),
                        )
                    });
                    if let Some(id) = tc["id"].as_str() {
                        if !id.is_empty() {
                            entry.0 = id.to_string();
                        }
                    }
                    if let Some(name) = tc["function"]["name"].as_str() {
                        if !name.is_empty() {
                            entry.1 = name.to_string();
                        }
                    }
                    if let Some(args) = tc["function"]["arguments"].as_str() {
                        entry.2.push_str(args);
                    }
                }
            }
            } // end while let newline
        } // end while let chunk

        // Convert accumulated tool calls
        let mut tool_uses = Vec::new();
        let mut indices: Vec<u64> = tool_calls_map.keys().copied().collect();
        indices.sort();
        for idx in indices {
            if let Some((id, name, arguments)) = tool_calls_map.remove(&idx) {
                let input: serde_json::Value =
                    serde_json::from_str(&arguments).unwrap_or(json!({}));
                tool_uses.push(LlmToolUse {
                    id,
                    name,
                    input,
                });
            }
        }

        // If content is empty but reasoning is present, use reasoning as the response text
        let final_text = if full_text.is_empty() && !full_reasoning.is_empty() {
            full_reasoning.clone()
        } else {
            full_text
        };

        let stop_reason = if !tool_uses.is_empty() {
            "tool_use".to_string()
        } else {
            "end_turn".to_string()
        };

        let raw_content = Self::build_raw_content(&final_text, &full_reasoning);
        Ok(LlmMessageResult {
            text: final_text,
            tool_uses,
            stop_reason,
            raw_content,
            usage: None,
            model_used: self.model_id.clone(),
        })
    }
}
