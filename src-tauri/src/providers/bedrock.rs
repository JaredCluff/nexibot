//! AWS Bedrock provider with SigV4 request signing.
#![allow(dead_code)]

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::claude::Message;
use crate::llm_provider::{LlmProvider, ProviderCapabilities};
use crate::session_overrides::SessionOverrides;

use super::{LlmClient, LlmMessageResult, LlmToolUse, TokenUsage};

type HmacSha256 = Hmac<Sha256>;

/// AWS Bedrock client with SigV4 request signing.
pub struct BedrockClient {
    model_id: String,
    region: String,
    access_key_id: String,
    secret_access_key: String,
    max_tokens: usize,
    http_client: reqwest::Client,
}

impl BedrockClient {
    /// Create a new Bedrock client.
    pub fn new(
        model_id: &str,
        region: &str,
        access_key_id: &str,
        secret_access_key: &str,
        max_tokens: usize,
    ) -> Self {
        Self {
            model_id: model_id.to_string(),
            region: region.to_string(),
            access_key_id: access_key_id.to_string(),
            secret_access_key: secret_access_key.to_string(),
            max_tokens,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// Apply transport settings (proxy, timeouts) to this client.
    pub fn with_transport(mut self, transport: crate::config::providers::TransportConfig) -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(transport.read_timeout_secs))
            .connect_timeout(std::time::Duration::from_secs(transport.connect_timeout_secs));
        if let Some(proxy_url) = &transport.proxy_url {
            match reqwest::Proxy::all(proxy_url) {
                Ok(proxy) => {
                    builder = builder.proxy(proxy);
                }
                Err(e) => tracing::warn!("[BEDROCK] Proxy error: {}", e),
            }
        }
        self.http_client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
        self
    }

    /// Build the Bedrock runtime invoke endpoint URL for the given region and model ID.
    pub fn endpoint_url(region: &str, model_id: &str) -> String {
        let encoded = urlencoding::encode(model_id);
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/invoke",
            region, encoded
        )
    }

    /// Sign a Bedrock request using AWS Signature Version 4.
    fn sign_request(
        &self,
        method: &str,
        url: &str,
        payload_hash: &str,
        datetime: &str,
        date: &str,
    ) -> Result<String> {
        let parsed = url::Url::parse(url)?;
        let host = parsed.host_str().unwrap_or("");
        let path = parsed.path();
        let service = "bedrock";

        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
            host, payload_hash, datetime
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";

        let canonical_request = format!(
            "{}\n{}\n\n{}\n{}\n{}",
            method, path, canonical_headers, signed_headers, payload_hash
        );

        let credential_scope = format!("{}/{}/{}/aws4_request", date, self.region, service);
        let cr_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            datetime, credential_scope, cr_hash
        );

        let signing_key =
            derive_signing_key(&self.secret_access_key, date, &self.region, service)?;
        let mut mac = HmacSha256::new_from_slice(&signing_key)
            .map_err(|e| anyhow::anyhow!("HMAC key error: {}", e))?;
        mac.update(string_to_sign.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        Ok(format!(
            "AWS4-HMAC-SHA256 Credential={}/{},SignedHeaders={},Signature={}",
            self.access_key_id, credential_scope, signed_headers, signature
        ))
    }
}

fn sigv4_date() -> String {
    Utc::now().format("%Y%m%d").to_string()
}

fn derive_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Result<Vec<u8>> {
    let k_secret = format!("AWS4{}", secret);
    let k_date = hmac_sha256(k_secret.as_bytes(), date.as_bytes())?;
    let k_region = hmac_sha256(&k_date, region.as_bytes())?;
    let k_service = hmac_sha256(&k_region, service.as_bytes())?;
    hmac_sha256(&k_service, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|e| anyhow::anyhow!("HMAC key error: {}", e))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

#[async_trait]
impl LlmClient for BedrockClient {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Bedrock
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_thinking: false,
            supports_computer_use: false,
            supports_tools: true,
            supports_vision: true,
        }
    }

    async fn send_message_with_tools(
        &self,
        messages: &[Message],
        tools: &[serde_json::Value],
        system_prompt: &str,
        _overrides: &SessionOverrides,
    ) -> Result<LlmMessageResult> {
        let url = Self::endpoint_url(&self.region, &self.model_id);

        let body = serde_json::json!({
            "anthropic_version": "bedrock-2023-05-31",
            "max_tokens": self.max_tokens,
            "system": system_prompt,
            "messages": messages,
            "tools": tools,
        });

        let payload = serde_json::to_vec(&body)?;
        let payload_hash = hex::encode(Sha256::digest(&payload));

        let now = Utc::now();
        let datetime = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();

        let auth_header =
            self.sign_request("POST", &url, &payload_hash, &datetime, &date)?;

        let response = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-amz-date", &datetime)
            .header("x-amz-content-sha256", &payload_hash)
            .header("Authorization", auth_header)
            .body(payload)
            .send()
            .await
            .context("Bedrock request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Bedrock error {}: {}", status, text);
        }

        let resp: serde_json::Value = response.json().await.context("Bedrock JSON parse")?;

        let text = resp["content"]
            .as_array()
            .and_then(|arr| arr.iter().find(|b| b["type"] == "text"))
            .and_then(|b| b["text"].as_str())
            .unwrap_or("")
            .to_string();

        let tool_uses: Vec<LlmToolUse> = resp["content"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter(|b| b["type"] == "tool_use")
                    .map(|b| LlmToolUse {
                        id: b["id"].as_str().unwrap_or("").to_string(),
                        name: b["name"].as_str().unwrap_or("").to_string(),
                        input: b["input"].clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let stop_reason = resp["stop_reason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_string();

        let raw_content = resp["content"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let usage = Some(TokenUsage {
            input_tokens: resp["usage"]["input_tokens"].as_u64().map(|v| v as usize),
            output_tokens: resp["usage"]["output_tokens"].as_u64().map(|v| v as usize),
        });

        Ok(LlmMessageResult {
            text,
            tool_uses,
            stop_reason,
            raw_content,
            usage,
            model_used: self.model_id.clone(),
        })
    }

    async fn send_message_streaming_with_tools(
        &self,
        messages: &[Message],
        tools: &[serde_json::Value],
        system_prompt: &str,
        overrides: &SessionOverrides,
        _on_chunk: Box<dyn for<'a> Fn(&'a str) + Send + Sync + 'static>,
    ) -> Result<LlmMessageResult> {
        // Bedrock streaming not yet implemented — fall back to non-streaming.
        self.send_message_with_tools(messages, tools, system_prompt, overrides)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bedrock_endpoint_format() {
        let url = BedrockClient::endpoint_url("us-east-1", "anthropic.claude-sonnet-4-6-v1:0");
        assert_eq!(
            url,
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.claude-sonnet-4-6-v1%3A0/invoke"
        );
    }

    #[test]
    fn sigv4_date_format() {
        let date = sigv4_date();
        assert_eq!(date.len(), 8, "YYYYMMDD should be 8 chars, got: {}", date);
    }

    #[test]
    fn derive_signing_key_deterministic() {
        let k1 = derive_signing_key("secret", "20260407", "us-east-1", "bedrock").unwrap();
        let k2 = derive_signing_key("secret", "20260407", "us-east-1", "bedrock").unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn bedrock_client_new_smoke() {
        let c = BedrockClient::new(
            "anthropic.claude-sonnet-4-6-v1:0",
            "us-east-1",
            "AKID",
            "secret",
            4096,
        );
        assert_eq!(c.model_id(), "anthropic.claude-sonnet-4-6-v1:0");
        assert_eq!(c.provider(), crate::llm_provider::LlmProvider::Bedrock);
    }
}
