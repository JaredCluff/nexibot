//! LLM provider configurations: OpenAI, Cerebras, Google, DeepSeek, GitHub Copilot, MiniMax, Qwen, Ollama.

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
