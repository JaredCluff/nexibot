//! LLM provider configurations: OpenAI, Cerebras, Google, DeepSeek, GitHub Copilot, MiniMax, Qwen, Ollama, xAI, Moonshot.

use serde::{Deserialize, Serialize};

fn default_openai_model() -> String {
    "gpt-4o".to_string()
}
fn default_openai_max_tokens() -> usize {
    4096
}
fn default_cerebras_model() -> String {
    "cerebras/gpt-oss-120b".to_string()
}
fn default_cerebras_max_tokens() -> usize {
    4096
}
fn default_google_model() -> String {
    "gemini-2.0-flash".to_string()
}
fn default_deepseek_url() -> String {
    "https://api.deepseek.com/v1".to_string()
}
fn default_deepseek_model() -> String {
    "deepseek-chat".to_string()
}
fn default_copilot_url() -> String {
    "https://api.githubcopilot.com".to_string()
}
fn default_minimax_url() -> String {
    "https://api.minimax.chat/v1".to_string()
}
fn default_minimax_model() -> String {
    "minimax-2.5".to_string()
}
fn default_qwen_url() -> String {
    "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()
}
fn default_qwen_model() -> String {
    "qwen-plus".to_string()
}
fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}
fn default_ollama_model() -> String {
    "llama3.2".to_string()
}
fn default_lmstudio_url() -> String {
    "http://localhost:1234".to_string()
}
fn default_lmstudio_model() -> String {
    String::new()
}

/// OpenAI API configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIConfig {
    /// OpenAI API key
    #[serde(default)]
    pub api_key: Option<String>,
    /// Model to use (default "gpt-4o")
    #[serde(default = "default_openai_model")]
    pub model: String,
    /// Max tokens for responses
    #[serde(default = "default_openai_max_tokens")]
    pub max_tokens: usize,
    /// OpenAI organization ID (optional)
    #[serde(default)]
    pub organization_id: Option<String>,
    /// Route requests through the bridge for logging and credential isolation.
    #[serde(default)]
    pub use_bridge: bool,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            model: default_openai_model(),
            max_tokens: default_openai_max_tokens(),
            organization_id: None,
            use_bridge: false,
        }
    }
}

/// Cerebras API configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CerebrasConfig {
    /// Cerebras API key
    #[serde(default)]
    pub api_key: Option<String>,
    /// Model to use (default "cerebras/gpt-oss-120b")
    #[serde(default = "default_cerebras_model")]
    pub model: String,
    /// Max tokens for responses
    #[serde(default = "default_cerebras_max_tokens")]
    pub max_tokens: usize,
}

impl Default for CerebrasConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            model: default_cerebras_model(),
            max_tokens: default_cerebras_max_tokens(),
        }
    }
}

/// Google Gemini API configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GoogleConfig {
    /// Google AI API key.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Default Gemini model.
    #[serde(default = "default_google_model")]
    pub default_model: String,
    /// Route requests through the bridge for logging and credential isolation.
    #[serde(default)]
    pub use_bridge: bool,
}

/// DeepSeek API configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeepSeekConfig {
    /// DeepSeek API key.
    #[serde(default)]
    pub api_key: Option<String>,
    /// DeepSeek API URL.
    #[serde(default = "default_deepseek_url")]
    pub api_url: String,
    /// Default DeepSeek model.
    #[serde(default = "default_deepseek_model")]
    pub default_model: String,
    /// Route requests through the bridge for logging and credential isolation.
    #[serde(default)]
    pub use_bridge: bool,
}

/// GitHub Copilot configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitHubCopilotConfig {
    /// GitHub Copilot token.
    #[serde(default)]
    pub token: Option<String>,
    /// Copilot API URL.
    #[serde(default = "default_copilot_url")]
    pub api_url: String,
}

/// MiniMax API configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MiniMaxConfig {
    /// MiniMax API key.
    #[serde(default)]
    pub api_key: Option<String>,
    /// MiniMax API URL.
    #[serde(default = "default_minimax_url")]
    pub api_url: String,
    /// Default MiniMax model.
    #[serde(default = "default_minimax_model")]
    pub default_model: String,
}

/// Qwen (DashScope) API configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QwenConfig {
    /// DashScope API key.
    #[serde(default)]
    pub api_key: Option<String>,
    /// DashScope OpenAI-compatible API URL.
    #[serde(default = "default_qwen_url")]
    pub api_url: String,
    /// Default Qwen model.
    #[serde(default = "default_qwen_model")]
    pub default_model: String,
}

/// Ollama local inference configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    /// Whether Ollama integration is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Ollama server URL (default: "http://localhost:11434")
    #[serde(default = "default_ollama_url")]
    pub url: String,
    /// Default model to use (default: "llama3.2")
    #[serde(default = "default_ollama_model")]
    pub model: String,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: default_ollama_url(),
            model: default_ollama_model(),
        }
    }
}

/// LM Studio local inference configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LMStudioConfig {
    /// LM Studio server URL (default: "http://localhost:1234")
    #[serde(default = "default_lmstudio_url")]
    pub url: String,
    /// Default model to use (empty = auto-detect from LM Studio)
    #[serde(default = "default_lmstudio_model")]
    pub model: String,
}

impl Default for LMStudioConfig {
    fn default() -> Self {
        Self {
            url: default_lmstudio_url(),
            model: default_lmstudio_model(),
        }
    }
}

fn default_connect_timeout() -> u64 { 30 }
fn default_read_timeout() -> u64 { 600 }
fn default_bedrock_region() -> String { "us-east-1".to_string() }

/// Transport configuration for proxy and timeout settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_read_timeout")]
    pub read_timeout_secs: u64,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            proxy_url: None,
            connect_timeout_secs: default_connect_timeout(),
            read_timeout_secs: default_read_timeout(),
        }
    }
}

/// AWS Bedrock API configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BedrockConfig {
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(default = "default_bedrock_region")]
    pub region: String,
    #[serde(default)]
    pub model_id: String,
    #[serde(default = "default_openai_max_tokens")]
    pub max_tokens: usize,
}

impl Default for BedrockConfig {
    fn default() -> Self {
        Self {
            access_key_id: String::new(),
            secret_access_key: String::new(),
            region: default_bedrock_region(),
            model_id: String::new(),
            max_tokens: 4096,
        }
    }
}

/// Mantle API configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MantleConfig {
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    #[serde(default = "default_openai_max_tokens")]
    pub max_tokens: usize,
}

fn default_xai_url() -> String {
    "https://api.x.ai/v1".to_string()
}
fn default_xai_model() -> String {
    "grok-3".to_string()
}
fn default_moonshot_url() -> String {
    "https://api.moonshot.cn/v1".to_string()
}
fn default_moonshot_model() -> String {
    "moonshot-v1-8k".to_string()
}

/// xAI (Grok) API configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct XaiConfig {
    /// xAI API key (from console.x.ai).
    #[serde(default)]
    pub api_key: Option<String>,
    /// xAI API URL (default: "https://api.x.ai/v1").
    #[serde(default = "default_xai_url")]
    pub api_url: String,
    /// Default xAI model (default: "grok-3").
    #[serde(default = "default_xai_model")]
    pub default_model: String,
}

/// Moonshot (Kimi) API configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MoonshotConfig {
    /// Moonshot API key (from platform.moonshot.cn).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Moonshot API URL (default: "https://api.moonshot.cn/v1").
    #[serde(default = "default_moonshot_url")]
    pub api_url: String,
    /// Default Moonshot model (default: "moonshot-v1-8k").
    #[serde(default = "default_moonshot_model")]
    pub default_model: String,
}

/// Get the default max_tokens for a given model.
/// Returns a sensible default based on model capabilities.
pub fn default_max_tokens_for_model(model: &str) -> usize {
    if model.starts_with("claude-opus") {
        8192
    } else if model.starts_with("claude-sonnet") {
        8192
    } else if model.starts_with("claude-haiku") {
        4096
    } else if model.starts_with("gpt-4o") || model.starts_with("o1") || model.starts_with("o3") {
        4096
    } else {
        4096
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_config_defaults() {
        let t = TransportConfig::default();
        assert!(t.proxy_url.is_none());
        assert_eq!(t.connect_timeout_secs, 30);
        assert_eq!(t.read_timeout_secs, 600);
    }

    #[test]
    fn bedrock_config_default_region() {
        let b = BedrockConfig::default();
        assert_eq!(b.region, "us-east-1");
    }
}
