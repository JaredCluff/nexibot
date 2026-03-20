//! Multi-provider LLM abstraction layer.
//!
//! Provides the `LlmClient` trait for provider-agnostic LLM interaction,
//! `ModelRegistry` for managing provider instances and model resolution,
//! and concrete implementations for Anthropic, OpenAI-compatible, Ollama, and Google.

pub mod anthropic;
pub mod auth_profiles;
pub mod cerebras;
pub mod conversation;
pub mod google;
pub mod model_router;
pub mod ollama;
pub mod openai_compat;
pub mod system_prompt;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::config::{AgentConfig, NexiBotConfig};
use crate::llm_provider::{LlmProvider, ProviderCapabilities};
use crate::session_overrides::SessionOverrides;

/// Whether an LLM provider error should trigger failover to the next model.
///
/// Prefers reqwest's typed error methods (is_connect, is_timeout, HTTP status)
/// over fragile string matching. Falls back to string matching only for
/// non-reqwest errors where typed introspection is unavailable.
pub fn is_failover_eligible(err: &anyhow::Error) -> bool {
    // Check for specific reqwest error types first — these are stable, typed,
    // and immune to message-format changes across reqwest versions.
    if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
        return reqwest_err.is_connect()
            || reqwest_err.is_timeout()
            || reqwest_err
                .status()
                .map(|s| s.is_server_error())
                .unwrap_or(false)
            // 429 Too Many Requests: rate-limited — failover to another provider.
            || reqwest_err
                .status()
                .map(|s| s.as_u16() == 429)
                .unwrap_or(false)
            // 402 Payment Required: quota/subscription exhausted — failover.
            || reqwest_err
                .status()
                .map(|s| s.as_u16() == 402)
                .unwrap_or(false);
    }

    // Fall back to string matching only for non-reqwest errors (e.g. custom
    // provider wrappers, timeout wrappers, DNS errors reported as strings).
    let msg = err.to_string().to_lowercase();
    msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("timed out")
        || msg.contains("dns")
        || msg.contains("503")
        || msg.contains("502")
        || msg.contains("429")  // rate limited — failover
        || msg.contains("402")  // quota exhausted — failover
}

/// Normalized response from any LLM provider.
#[derive(Debug, Clone)]
pub struct LlmMessageResult {
    /// Text content from the response.
    pub text: String,
    /// Tool use requests (if stop_reason == "tool_use").
    pub tool_uses: Vec<LlmToolUse>,
    /// The stop reason (e.g., "end_turn", "tool_use", "max_tokens").
    pub stop_reason: String,
    /// Raw content blocks for conversation history.
    pub raw_content: Vec<serde_json::Value>,
    /// Token usage info (if available).
    #[allow(dead_code)]
    pub usage: Option<TokenUsage>,
    /// The model that actually executed the request.
    #[allow(dead_code)]
    pub model_used: String,
}

/// A tool use request from the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolUse {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Token usage information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
}

/// Provider-agnostic LLM client trait.
///
/// Each provider (Anthropic, OpenAI, Ollama, Google) implements this trait.
/// The `ModelRegistry` selects the appropriate client based on model ID.
#[allow(dead_code)]
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Which provider this client talks to.
    fn provider(&self) -> LlmProvider;

    /// The model ID this client is configured for.
    fn model_id(&self) -> &str;

    /// Provider capability flags.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Send a message with tools (non-streaming).
    async fn send_message_with_tools(
        &self,
        messages: &[crate::claude::Message],
        tools: &[serde_json::Value],
        system_prompt: &str,
        overrides: &SessionOverrides,
    ) -> Result<LlmMessageResult>;

    /// Send a message with tools (streaming text via callback).
    async fn send_message_streaming_with_tools(
        &self,
        messages: &[crate::claude::Message],
        tools: &[serde_json::Value],
        system_prompt: &str,
        overrides: &SessionOverrides,
        on_chunk: Box<dyn for<'a> Fn(&'a str) + Send + Sync + 'static>,
    ) -> Result<LlmMessageResult>;
}

/// Authentication credentials for a provider.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ProviderAuth {
    /// API key authentication.
    ApiKey(String),
    /// OAuth token (e.g., Anthropic OAuth).
    OAuthToken(String),
    /// No authentication needed (e.g., local Ollama).
    None,
}

/// Information about an available model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ModelInfo {
    pub model_id: String,
    pub provider: String,
    pub display_name: String,
}

/// Status of a configured provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ProviderStatus {
    pub provider: String,
    pub configured: bool,
    pub default_model: String,
    pub available_models: Vec<String>,
}

/// View of model configuration for the settings UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ModelConfigView {
    pub global_default: String,
    pub global_backup: Option<String>,
    pub agents: Vec<AgentModelView>,
}

/// Per-agent model view for the settings UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AgentModelView {
    pub agent_id: String,
    pub agent_name: String,
    pub primary_model: Option<String>,
    pub backup_model: Option<String>,
    pub effective_model: String,
}

/// Registry of LLM provider clients with model resolution.
///
/// Implements the layered fallback chain:
/// 1. Agent-specific primary_model
/// 2. Agent-specific backup_model
/// 3. User's global default_model
/// 4. Provider system default
pub struct ModelRegistry {
    /// Cached clients keyed by model ID.
    clients: HashMap<String, Arc<dyn LlmClient>>,
    /// Provider authentication info.
    provider_configs: HashMap<LlmProvider, ProviderAuth>,
    /// Global default model ID.
    global_default_model: String,
    /// Global backup model ID.
    global_backup_model: Option<String>,
    /// Shared config reference.
    config: Arc<RwLock<NexiBotConfig>>,
}

impl ModelRegistry {
    /// Create a new ModelRegistry from the application config.
    pub fn new(config: Arc<RwLock<NexiBotConfig>>) -> Self {
        Self {
            clients: HashMap::new(),
            provider_configs: HashMap::new(),
            global_default_model: String::new(),
            global_backup_model: None,
            config,
        }
    }

    /// Initialize from the current config, detecting available providers.
    pub async fn initialize(&mut self) {
        let cfg = self.config.read().await;

        // Set global defaults from config
        self.global_default_model = cfg
            .defaults
            .as_ref()
            .map(|d| d.model.clone())
            .unwrap_or_else(|| cfg.claude.model.clone());

        self.global_backup_model = cfg
            .defaults
            .as_ref()
            .and_then(|d| d.backup_model.clone())
            .or_else(|| cfg.claude.fallback_model.clone());

        // Register provider auth from config
        if let Some(ref key) = cfg.claude.api_key {
            if !key.is_empty() {
                self.provider_configs
                    .insert(LlmProvider::Anthropic, ProviderAuth::ApiKey(key.clone()));
            }
        }
        if let Some(ref key) = cfg.openai.api_key {
            if !key.is_empty() {
                self.provider_configs
                    .insert(LlmProvider::OpenAI, ProviderAuth::ApiKey(key.clone()));
            }
        }
        if let Some(ref key) = cfg.cerebras.api_key {
            if !key.is_empty() {
                self.provider_configs
                    .insert(LlmProvider::Cerebras, ProviderAuth::ApiKey(key.clone()));
            }
        }
        // Ollama and LM Studio are always registered — they auto-detect at connection time
        self.provider_configs
            .insert(LlmProvider::Ollama, ProviderAuth::None);
        self.provider_configs
            .insert(LlmProvider::LMStudio, ProviderAuth::None);
        if let Some(ref google) = cfg.google {
            if let Some(ref key) = google.api_key {
                if !key.is_empty() {
                    self.provider_configs
                        .insert(LlmProvider::Google, ProviderAuth::ApiKey(key.clone()));
                }
            }
        }
        if let Some(ref deepseek) = cfg.deepseek {
            if let Some(ref key) = deepseek.api_key {
                if !key.is_empty() {
                    self.provider_configs
                        .insert(LlmProvider::DeepSeek, ProviderAuth::ApiKey(key.clone()));
                }
            }
        }
        if let Some(ref gh) = cfg.github_copilot {
            if let Some(ref token) = gh.token {
                if !token.is_empty() {
                    self.provider_configs.insert(
                        LlmProvider::GitHubCopilot,
                        ProviderAuth::ApiKey(token.clone()),
                    );
                }
            }
        }
        if let Some(ref mm) = cfg.minimax {
            if let Some(ref key) = mm.api_key {
                if !key.is_empty() {
                    self.provider_configs
                        .insert(LlmProvider::MiniMax, ProviderAuth::ApiKey(key.clone()));
                }
            }
        }

        info!(
            "[MODEL_REGISTRY] Initialized with {} providers, default model: {}",
            self.provider_configs.len(),
            self.global_default_model
        );
    }

    /// Reload provider credentials and model defaults from the current config.
    ///
    /// Called when the config file changes at runtime (hotloading). Clears
    /// cached clients and provider auth so all subsequent calls re-authenticate
    /// with the updated credentials.
    pub async fn reload(&mut self) {
        self.clients.clear();
        self.provider_configs.clear();
        self.global_default_model.clear();
        self.global_backup_model = None;
        self.initialize().await;
        info!("[MODEL_REGISTRY] Reloaded after config change");
    }

    /// Resolve the effective model for an agent with full fallback chain.
    ///
    /// Resolution order:
    /// 1. agent.primary_model (or agent.model for backward compat)
    /// 2. agent.backup_model
    /// 3. global_default_model
    /// 4. "claude-sonnet-4-5-20250929" (hardcoded fallback)
    #[allow(dead_code)]
    pub fn resolve_model_for_agent(&self, agent: &AgentConfig) -> String {
        // 1. Agent primary model
        if let Some(ref primary) = agent.primary_model {
            if !primary.is_empty() {
                return primary.clone();
            }
        }
        // Backward compat: `model` field serves as primary if `primary_model` isn't set
        if let Some(ref model) = agent.model {
            if !model.is_empty() {
                return model.clone();
            }
        }

        // 2. Agent backup model (only if primary wasn't set)
        if let Some(ref backup) = agent.backup_model {
            if !backup.is_empty() {
                return backup.clone();
            }
        }

        // 3. Global default
        if !self.global_default_model.is_empty() {
            return self.global_default_model.clone();
        }

        // 4. Hardcoded fallback
        "claude-sonnet-4-5-20250929".to_string()
    }

    /// Register a new provider's credentials at runtime.
    #[allow(dead_code)]
    pub fn register_provider(&mut self, provider: LlmProvider, auth: ProviderAuth) {
        info!("[MODEL_REGISTRY] Registered provider: {:?}", provider);
        self.provider_configs.insert(provider, auth);
    }

    /// Update the global default model.
    #[allow(dead_code)]
    pub fn set_global_default(&mut self, model_id: &str) {
        info!("[MODEL_REGISTRY] Global default changed to: {}", model_id);
        self.global_default_model = model_id.to_string();
    }

    /// Get the global default model.
    #[allow(dead_code)]
    pub fn global_default(&self) -> &str {
        &self.global_default_model
    }

    /// Get the global backup model.
    #[allow(dead_code)]
    pub fn global_backup(&self) -> Option<&str> {
        self.global_backup_model.as_deref()
    }

    /// List all available models across configured providers.
    // NOTE: Model lists are fetched dynamically from provider APIs at runtime.
    // See `get_available_models` / `fetch_cerebras_models_internal` etc. in
    // `commands/session_cmds.rs`.  No hardcoded model lists belong here.

    /// Build a model config view for the settings UI.
    #[allow(dead_code)]
    pub fn model_config_view(&self, agents: &[AgentConfig]) -> ModelConfigView {
        ModelConfigView {
            global_default: self.global_default_model.clone(),
            global_backup: self.global_backup_model.clone(),
            agents: agents
                .iter()
                .map(|agent| AgentModelView {
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    primary_model: agent.primary_model.clone().or_else(|| agent.model.clone()),
                    backup_model: agent.backup_model.clone(),
                    effective_model: self.resolve_model_for_agent(agent),
                })
                .collect(),
        }
    }
}
