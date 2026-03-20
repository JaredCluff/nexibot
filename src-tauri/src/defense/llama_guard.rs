//! Llama Guard 3 content safety classifier
//!
//! Supports two modes:
//! - **API**: Uses Ollama-compatible endpoint (localhost:11434) to run Llama Guard 3
//! - **Local**: Placeholder for future local ONNX inference (complex due to generative nature)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use tracing::warn;

use crate::guardrails::SecurityLevel;

/// Llama Guard content safety classifier
pub struct LlamaGuardClassifier {
    mode: LlamaGuardMode,
    api_url: String,
    http_client: reqwest::Client,
    error_count: AtomicU32,
    security_level: SecurityLevel,
}

#[derive(Debug, Clone)]
enum LlamaGuardMode {
    Api,
    Local, // Placeholder for future ONNX implementation
}

#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    response: String,
}

impl LlamaGuardClassifier {
    pub fn new(
        mode: &str,
        api_url: &str,
        security_level: SecurityLevel,
        allow_remote: bool,
    ) -> Result<Self> {
        let mode = match mode {
            "api" => LlamaGuardMode::Api,
            "local" => {
                warn!("[LLAMA_GUARD] Local mode is not yet implemented, falling back to API");
                LlamaGuardMode::Local
            }
            _ => anyhow::bail!("Unknown Llama Guard mode: {}. Use 'api' or 'local'.", mode),
        };

        // Validate endpoint URL - reject non-localhost unless explicitly allowed
        if let Ok(parsed) = url::Url::parse(api_url) {
            let host = parsed.host_str().unwrap_or("");
            let is_local =
                host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]";
            if !is_local {
                if !allow_remote {
                    anyhow::bail!(
                        "Llama Guard endpoint '{}' is not localhost. Set allow_remote_llama_guard=true to allow remote endpoints.",
                        api_url
                    );
                }
                warn!(
                    "[LLAMA_GUARD] Using remote endpoint: {} — ensure this connection is trusted",
                    api_url
                );
            }
        }

        Ok(Self {
            mode,
            api_url: api_url.to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?,
            error_count: AtomicU32::new(0),
            security_level,
        })
    }

    /// Check if the classifier is healthy (error count below threshold)
    pub fn is_healthy(&self) -> bool {
        self.error_count.load(Ordering::Relaxed) < 5
    }

    /// Classify text for content safety
    /// Returns (is_safe, category, confidence)
    pub async fn classify(&self, text: &str) -> (bool, String, f32) {
        match &self.mode {
            LlamaGuardMode::Api => self.classify_api(text).await,
            LlamaGuardMode::Local => {
                tracing::warn!("[LLAMA_GUARD] Local ONNX inference is not implemented — returning unsafe (fail-closed). Configure llama_guard_mode: api with a running Ollama instance for active content safety, or disable Llama Guard.");
                (false, "local_mode_not_implemented".to_string(), 1.0)
            }
        }
    }

    async fn classify_api(&self, text: &str) -> (bool, String, f32) {
        match self.classify_api_inner(text).await {
            Ok(result) => {
                // Reset error count on success
                self.error_count.store(0, Ordering::Relaxed);
                result
            }
            Err(e) => {
                let count = self.error_count.fetch_add(1, Ordering::Relaxed) + 1;
                warn!(
                    "[LLAMA_GUARD] API classification failed (error #{}/5): {}",
                    count, e
                );
                // Fail-closed in Standard or higher security
                match self.security_level {
                    SecurityLevel::Relaxed | SecurityLevel::Disabled => {
                        warn!(
                            "[LLAMA_GUARD] Failing open due to {:?} security level",
                            self.security_level
                        );
                        (true, "error".to_string(), 0.0)
                    }
                    _ => {
                        warn!(
                            "[LLAMA_GUARD] Failing closed due to {:?} security level",
                            self.security_level
                        );
                        (false, "error_fail_closed".to_string(), 1.0)
                    }
                }
            }
        }
    }

    async fn classify_api_inner(&self, text: &str) -> Result<(bool, String, f32)> {
        // Build the Llama Guard prompt format
        let prompt = format!(
            r#"<|begin_of_text|><|start_header_id|>user<|end_header_id|>

Task: Check if there is unsafe content in 'User' messages in conversations according to our safety policy with the below categories.

<BEGIN UNSAFE CONTENT CATEGORIES>
S1: Violent Crimes.
S2: Non-Violent Crimes.
S3: Sex-Related Crimes.
S4: Child Sexual Exploitation.
S5: Defamation.
S6: Specialized Advice.
S7: Privacy.
S8: Intellectual Property.
S9: Indiscriminate Weapons.
S10: Hate.
S11: Suicide & Self-Harm.
S12: Sexual Content.
S13: Elections.
S14: Code Interpreter Abuse.
<END UNSAFE CONTENT CATEGORIES>

<BEGIN CONVERSATION>

User: {}

<END CONVERSATION>

Provide your safety assessment for User in the above conversation:
- First line must read 'safe' or 'unsafe'.
- If unsafe, a second line must include a comma-separated list of violated categories.<|eot_id|><|start_header_id|>assistant<|end_header_id|>

"#,
            text
        );

        let request = OllamaRequest {
            model: "llama-guard3".to_string(),
            prompt,
            stream: false,
        };

        let response = self
            .http_client
            .post(format!("{}/api/generate", self.api_url))
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Llama Guard API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Llama Guard API error: {} - {}", status, error_text);
        }

        let ollama_response: OllamaResponse = response
            .json()
            .await
            .context("Failed to parse Llama Guard response")?;

        // Parse response: first line is "safe" or "unsafe"
        let response_text = ollama_response.response.trim().to_lowercase();
        let lines: Vec<&str> = response_text.lines().collect();

        if lines.is_empty() {
            anyhow::bail!("Empty response from Llama Guard");
        }

        let is_safe = lines[0].trim() == "safe";
        let category = if !is_safe && lines.len() > 1 {
            lines[1].trim().to_string()
        } else if !is_safe {
            "unknown".to_string()
        } else {
            "none".to_string()
        };

        // Llama Guard doesn't provide explicit confidence, use 1.0 for definitive answers
        let confidence = if is_safe { 1.0 } else { 0.9 };

        Ok((is_safe, category, confidence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_mode_returns_unsafe() {
        // Local mode is not implemented — must fail closed (return unsafe)
        let classifier = LlamaGuardClassifier::new(
            "local",
            "http://localhost:11434",
            SecurityLevel::Standard,
            false,
        )
        .unwrap();

        let (is_safe, category, confidence) = classifier.classify("Hello world").await;
        assert!(!is_safe, "Local mode must return unsafe (fail-closed)");
        assert_eq!(category, "local_mode_not_implemented");
        assert_eq!(confidence, 1.0);
    }

    #[test]
    fn test_rejects_unknown_mode() {
        let result = LlamaGuardClassifier::new(
            "invalid_mode",
            "http://localhost:11434",
            SecurityLevel::Standard,
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_remote_endpoint_without_allow() {
        let result = LlamaGuardClassifier::new(
            "api",
            "http://evil-server.com:11434",
            SecurityLevel::Standard,
            false, // allow_remote = false
        );
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().unwrap());
        assert!(err_msg.contains("not localhost"), "Error: {}", err_msg);
    }

    #[test]
    fn test_allows_localhost_endpoint() {
        let result = LlamaGuardClassifier::new(
            "api",
            "http://localhost:11434",
            SecurityLevel::Standard,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_allows_127001_endpoint() {
        let result = LlamaGuardClassifier::new(
            "api",
            "http://127.0.0.1:11434",
            SecurityLevel::Standard,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_allows_remote_with_flag() {
        let result = LlamaGuardClassifier::new(
            "api",
            "http://remote-host.com:11434",
            SecurityLevel::Standard,
            true, // allow_remote = true
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_healthy_initially() {
        let classifier = LlamaGuardClassifier::new(
            "api",
            "http://localhost:11434",
            SecurityLevel::Standard,
            false,
        )
        .unwrap();
        assert!(classifier.is_healthy());
    }
}
