//! Agent and MCP server configurations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

fn default_capability_category() -> String {
    "skill".to_string()
}

/// Configuration for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerConfig {
    /// Unique name for this server
    pub name: String,

    /// Whether this server is enabled
    pub enabled: bool,

    /// Command to launch the server
    pub command: String,

    /// Arguments for the command
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables for the server process
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Channel binding for an agent — which channel+peer an agent handles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelBinding {
    /// Channel type: "telegram", "whatsapp", "webchat", "webhook"
    pub channel: String,
    /// Specific peer ID (chat_id or phone). None = all on this channel.
    pub peer_id: Option<String>,
}

/// LLM provider for an agent.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum LlmProvider {
    #[default]
    Claude,
    OpenAI,
    Ollama,
    LMStudio,
}

/// Configuration for a single agent instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Unique agent ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Avatar path or emoji
    #[serde(default)]
    pub avatar: Option<String>,
    /// Model override (falls back to global claude.model). Kept for backward compat.
    #[serde(default)]
    pub model: Option<String>,
    /// Explicit primary model (takes precedence over `model`).
    #[serde(default)]
    pub primary_model: Option<String>,
    /// Fallback model on primary failure.
    #[serde(default)]
    pub backup_model: Option<String>,
    /// Provider override
    #[serde(default)]
    pub provider: Option<LlmProvider>,
    /// Path to a per-agent SOUL.md file
    #[serde(default)]
    pub soul_path: Option<PathBuf>,
    /// Additional system prompt text
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Whether this is the default agent
    #[serde(default)]
    pub is_default: bool,
    /// Channel bindings for message routing
    #[serde(default)]
    pub channel_bindings: Vec<ChannelBinding>,
    /// Agent capabilities for task delegation
    #[serde(default)]
    pub capabilities: Vec<AgentCapabilityConfig>,
    /// Per-agent workspace isolation configuration
    #[serde(default)]
    pub workspace: crate::agent_workspace::WorkspaceConfig,
}

/// Configuration for a single agent capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCapabilityConfig {
    /// Capability name (e.g., "web_research")
    #[serde(default)]
    pub name: String,
    /// Category: "knowledge", "tool", "skill", "compute"
    #[serde(default = "default_capability_category")]
    pub category: String,
    /// Description of what this capability does
    #[serde(default)]
    pub description: String,
}

/// Global defaults for model resolution.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DefaultsConfig {
    /// Default provider name (e.g., "anthropic", "openai").
    #[serde(default)]
    pub provider: String,
    /// Default model ID.
    #[serde(default)]
    pub model: String,
    /// Optional global backup model.
    #[serde(default)]
    pub backup_model: Option<String>,
}
