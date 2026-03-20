//! Core LLM configurations: Claude API and K2K integration.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}
fn default_auto_compact_threshold() -> f64 {
    0.85
}
fn default_max_history_messages() -> usize {
    200
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    /// Claude API key
    pub api_key: Option<String>,

    /// Model to use (e.g., "claude-sonnet-4-5-20250929")
    pub model: String,

    /// Fallback model when primary is unavailable or rate-limited
    #[serde(default)]
    pub fallback_model: Option<String>,

    /// Max tokens for responses
    pub max_tokens: usize,

    /// System prompt
    pub system_prompt: String,

    /// Auto-compact threshold (0.0-1.0). Compaction triggers when estimated
    /// token usage exceeds this fraction of the context window. Default: 0.85
    #[serde(default = "default_auto_compact_threshold")]
    pub auto_compact_threshold: f64,

    /// Whether auto-compaction is enabled. Default: true
    #[serde(default = "default_true")]
    pub auto_compact_enabled: bool,

    /// Maximum number of messages to keep in the in-memory conversation history.
    /// Configurable at runtime (hotloaded). Default: 200.
    #[serde(default = "default_max_history_messages")]
    pub max_history_messages: usize,

    /// Maximum age (in days) of messages to load when restoring a session.
    /// Messages older than this are dropped on session load. None = no age filter.
    #[serde(default)]
    pub max_history_age_days: Option<u64>,

    /// Maximum age (in days) of sessions to show in /resume listings.
    /// Sessions older than this are hidden. None = show all sessions.
    #[serde(default)]
    pub max_session_age_days: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K2KConfig {
    /// Enable K2K integration
    #[serde(default)]
    pub enabled: bool,

    /// Local System Agent URL
    #[serde(default)]
    pub local_agent_url: String,

    /// K2K Router URL (optional)
    #[serde(default)]
    pub router_url: Option<String>,

    /// RSA private key for K2K authentication
    #[serde(default)]
    pub private_key_pem: Option<String>,

    /// Client ID for K2K
    #[serde(default)]
    pub client_id: String,

    /// Enable supermemory (auto-use System Agent for persistent memory when detected)
    #[serde(default = "default_true")]
    pub supermemory_enabled: bool,

    /// Auto-extract knowledge from conversations synced to supermemory
    #[serde(default = "default_true")]
    pub supermemory_auto_extract: bool,

    /// Base URL for Knowledge Nexus REST API (connectors, auth, etc.)
    /// Must be configured by the user; defaults to None.
    #[serde(default)]
    pub kn_base_url: Option<String>,

    /// JWT auth token for the Knowledge Nexus API (set after sign-in).
    #[serde(default)]
    pub kn_auth_token: Option<String>,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            model: "claude-sonnet-4-5-20250929".to_string(),
            fallback_model: None,
            max_tokens: 4096,
            system_prompt: "You are NexiBot, a desktop AI assistant. You have access to local \
                knowledge via the K2K protocol, a built-in task scheduler for recurring \
                automations, and various tools depending on what the user has enabled. Your \
                dynamic capabilities are listed above in the system prompt."
                .to_string(),
            auto_compact_threshold: 0.85,
            auto_compact_enabled: true,
            max_history_messages: 200,
            max_history_age_days: None,
            max_session_age_days: None,
        }
    }
}

impl Default for K2KConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            local_agent_url: "http://localhost:8765".to_string(),
            router_url: None,
            private_key_pem: None,
            client_id: format!("nexibot-{}", uuid::Uuid::new_v4()),
            supermemory_enabled: true,
            supermemory_auto_extract: true,
            kn_base_url: None,
            kn_auth_token: None,
        }
    }
}

fn default_heartbeat_interval() -> u64 {
    300 // 5 minutes
}

/// Configuration for Knowledge Nexus Central Management.
///
/// When enabled, NexiBot registers with the KN server, receives a managed
/// policy (security floors, allowed models, feature flags), and sends
/// periodic heartbeats so the server knows the instance is alive.
///
/// The server sets *floors* — minimum security levels.  Local config can be
/// stricter than the floor.  The server cannot relax a locally-enforced
/// restriction below its floor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedPolicyConfig {
    /// Enable central management. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Knowledge Nexus server base URL (must be configured by the user).
    #[serde(default)]
    pub kn_server_url: String,

    /// Service token issued by the KN server.  Must be set for management to work.
    #[serde(default)]
    pub service_token: Option<String>,

    /// Assigned instance ID (populated on first successful registration).
    /// Persist this — do not regenerate on every startup.
    #[serde(default)]
    pub instance_id: Option<String>,

    /// How often to send heartbeats and check for policy updates (seconds).
    /// Default: 300 (5 minutes).
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_secs: u64,

    /// Last successfully applied policy, persisted to disk so that floors/ceilings
    /// can be re-applied immediately on next startup before the first heartbeat.
    /// This eliminates the unrestricted window between app launch and first contact.
    #[serde(default)]
    pub cached_policy: Option<serde_json::Value>,
}

impl Default for ManagedPolicyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            kn_server_url: String::new(),
            service_token: None,
            instance_id: None,
            heartbeat_interval_secs: default_heartbeat_interval(),
            cached_policy: None,
        }
    }
}
