//! Configuration management for NexiBot

pub mod autonomy;
pub use autonomy::{
    AutonomousModeConfig, AutonomyLevel, ChannelToolPolicy,
};

pub mod channels;
pub use channels::{
    DiscordConfig, MatrixConfig, NatsConfig, SignalConfig, SlackConfig, TeamsConfig,
    TelegramConfig, WhatsAppConfig,
};

pub mod voice;
pub use voice::{
    AudioConfig, LoggingConfig, SttConfig, TtsConfig, VadConfig,
    WakewordConfig,
};

pub mod tools;
pub use tools::{ExecuteConfig, FetchConfig, FilesystemConfig, SearchConfig};

pub mod providers;
pub use providers::{
    default_max_tokens_for_model, CerebrasConfig, DeepSeekConfig, GitHubCopilotConfig,
    GoogleConfig, LMStudioConfig, MiniMaxConfig, OllamaConfig, OpenAIConfig, QwenConfig,
};

pub mod agents;
pub use agents::{
    AgentCapabilityConfig, AgentConfig, ChannelBinding, DefaultsConfig,
    MCPServerConfig,
};

pub mod shell;
#[allow(unused_imports)]
pub use shell::{
    DiscoveryConfig, ExtraDiscoveryPattern, GatedShellConfig, PluginConfig,
};

pub mod routing;
pub use routing::{RoutingConfig, YoloModeConfig};

pub mod core;
pub use core::{ClaudeConfig, K2KConfig, ManagedPolicyConfig};

pub mod webhooks;
pub use webhooks::{BrowserConfig, TlsConfig, WebhookAction, WebhookConfig, WebhookEndpoint};

pub mod mcp;
pub use mcp::{
    ComputerUseConfig, MCPConfig, ScheduledTask, ScheduledTasksConfig, ToolSearchConfig,
};

use anyhow::{Context, Result};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupConfig {
    /// Launch NexiBot at login
    pub nexibot_at_login: bool,
    /// Launch the K2K System Agent at login
    pub k2k_agent_at_login: bool,
    /// Path to the `kn-agent` binary
    pub k2k_agent_binary: String,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            nexibot_at_login: false,
            k2k_agent_at_login: false,
            k2k_agent_binary: crate::platform::default_k2k_agent_binary(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexiBotConfig {
    /// Config file version for migration support
    #[serde(default = "default_config_version")]
    pub config_version: u32,

    /// Logging configuration
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Claude API configuration
    #[serde(default)]
    pub claude: ClaudeConfig,

    /// K2K configuration
    #[serde(default)]
    pub k2k: K2KConfig,

    /// Audio configuration
    #[serde(default)]
    pub audio: AudioConfig,

    /// Wake word configuration
    #[serde(default)]
    pub wakeword: WakewordConfig,

    /// Voice Activity Detection configuration
    #[serde(default)]
    pub vad: VadConfig,

    /// Speech-to-Text configuration
    #[serde(default)]
    pub stt: SttConfig,

    /// Text-to-Speech configuration
    #[serde(default)]
    pub tts: TtsConfig,

    /// MCP (Model Context Protocol) configuration
    #[serde(default)]
    pub mcp: MCPConfig,

    /// Computer Use configuration
    #[serde(default)]
    pub computer_use: ComputerUseConfig,

    /// Defense pipeline configuration
    #[serde(default)]
    pub defense: crate::defense::DefenseConfig,

    /// Guardrails configuration
    #[serde(default)]
    pub guardrails: crate::guardrails::GuardrailsConfig,

    /// Scheduled tasks configuration
    #[serde(default)]
    pub scheduled_tasks: ScheduledTasksConfig,

    /// OpenAI API configuration
    #[serde(default)]
    pub openai: OpenAIConfig,

    /// Cerebras API configuration
    #[serde(default)]
    pub cerebras: CerebrasConfig,

    /// Ollama (local LLM) configuration
    #[serde(default)]
    pub ollama: OllamaConfig,

    /// LM Studio (local LLM) configuration
    #[serde(default)]
    pub lmstudio: LMStudioConfig,

    /// Webhook configuration
    #[serde(default)]
    pub webhooks: WebhookConfig,

    /// Browser CDP configuration
    #[serde(default)]
    pub browser: BrowserConfig,

    /// Telegram Bot configuration
    #[serde(default)]
    pub telegram: TelegramConfig,

    /// WhatsApp Cloud API configuration
    #[serde(default)]
    pub whatsapp: WhatsAppConfig,

    /// Discord Bot configuration
    #[serde(default)]
    pub discord: DiscordConfig,

    /// Slack Bot configuration
    #[serde(default)]
    pub slack: SlackConfig,

    /// Signal messaging configuration
    #[serde(default)]
    pub signal: SignalConfig,

    /// Microsoft Teams Bot configuration
    #[serde(default)]
    pub teams: TeamsConfig,

    /// Matrix messaging configuration
    #[serde(default)]
    pub matrix: MatrixConfig,

    /// NATS messaging bus configuration
    #[serde(default)]
    pub nats: NatsConfig,

    /// Web search tool configuration
    #[serde(default)]
    pub search: SearchConfig,

    /// HTTP fetch tool configuration
    #[serde(default)]
    pub fetch: FetchConfig,

    /// Filesystem tool configuration
    #[serde(default)]
    pub filesystem: FilesystemConfig,

    /// Code execution tool configuration
    #[serde(default)]
    pub execute: ExecuteConfig,

    /// Autonomous mode configuration
    #[serde(default)]
    pub autonomous_mode: AutonomousModeConfig,

    /// Startup / launch-at-login configuration
    #[serde(default)]
    pub startup: StartupConfig,

    /// Multi-agent configuration
    #[serde(default)]
    pub agents: Vec<AgentConfig>,

    /// Global defaults for model resolution
    #[serde(default)]
    pub defaults: Option<DefaultsConfig>,

    /// Google Gemini API configuration
    #[serde(default)]
    pub google: Option<GoogleConfig>,

    /// DeepSeek API configuration
    #[serde(default)]
    pub deepseek: Option<DeepSeekConfig>,

    /// GitHub Copilot configuration
    #[serde(default)]
    pub github_copilot: Option<GitHubCopilotConfig>,

    /// MiniMax API configuration
    #[serde(default)]
    pub minimax: Option<MiniMaxConfig>,

    /// Qwen (DashScope) API configuration
    #[serde(default)]
    pub qwen: Option<QwenConfig>,

    /// Email channel configuration (generic IMAP/SMTP)
    #[serde(default)]
    pub email: crate::email::EmailConfig,

    /// Gmail channel configuration (Google Gmail API)
    #[serde(default)]
    pub gmail: crate::gmail::GmailConfig,

    /// BlueBubbles (iMessage) configuration
    #[serde(default)]
    pub bluebubbles: crate::bluebubbles::BlueBubblesConfig,

    /// Google Chat / Workspace configuration
    #[serde(default)]
    pub google_chat: crate::google_chat::GoogleChatConfig,

    /// Mattermost bot configuration
    #[serde(default)]
    pub mattermost: crate::mattermost::MattermostConfig,

    /// Facebook Messenger configuration
    #[serde(default)]
    pub messenger: crate::messenger::MessengerConfig,

    /// Instagram Direct Messages configuration
    #[serde(default)]
    pub instagram: crate::instagram::InstagramConfig,

    /// LINE Messaging API configuration
    #[serde(default)]
    pub line: crate::line::LineConfig,

    /// Twilio SMS/MMS configuration
    #[serde(default)]
    pub twilio: crate::twilio::TwilioConfig,

    /// Mastodon configuration
    #[serde(default)]
    pub mastodon: crate::mastodon::MastodonConfig,

    /// Rocket.Chat configuration
    #[serde(default)]
    pub rocketchat: crate::rocketchat::RocketChatConfig,

    /// Self-hosted WebChat browser widget configuration
    #[serde(default)]
    pub webchat: crate::webchat::WebChatConfig,

    /// WebSocket gateway configuration
    #[serde(default)]
    pub gateway: crate::gateway::GatewayConfig,

    /// Docker sandbox configuration
    #[serde(default)]
    pub sandbox: crate::sandbox::SandboxConfig,

    /// Network policy configuration
    #[serde(default)]
    pub network_policy: crate::security::network_policy::NetworkPolicy,

    /// URL of the backend agent-engine service.
    /// Used when delegating long-running or remote-capability workflows.
    /// Defaults to "http://agent-engine:8019" (Docker Compose DNS) when None.
    #[serde(default)]
    pub agent_engine_url: Option<String>,

    /// Intelligent model routing configuration
    #[serde(default)]
    pub routing: RoutingConfig,

    /// Smart Key Vault configuration
    #[serde(default)]
    pub key_vault: KeyVaultConfig,

    /// NexiGate gated shell configuration
    #[serde(default)]
    pub gated_shell: GatedShellConfig,

    /// Yolo mode — time-limited elevated access authorized by the human.
    #[serde(default)]
    pub yolo_mode: YoloModeConfig,

    /// Session transcript encryption configuration.
    #[serde(default)]
    pub session_encryption: crate::security::session_encryption::SessionEncryptionConfig,

    /// Knowledge Nexus Central Management configuration.
    #[serde(default)]
    pub managed_policy: ManagedPolicyConfig,

    /// External skill directories to scan for skill definitions.
    #[serde(default)]
    pub external_skill_dirs: Vec<String>,

    /// Skill formats to auto-discover (e.g., "openclaw", "codex", "claude").
    #[serde(default = "default_auto_discover_formats")]
    pub auto_discover_formats: Vec<String>,

    /// LSP server configuration
    #[serde(default)]
    pub lsp: LspConfig,
}

// ---------------------------------------------------------------------------
// LSP configuration
// ---------------------------------------------------------------------------

/// Configuration for a single LSP server.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LspServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub extensions: Vec<String>,
}

/// LSP server configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LspConfig {
    #[serde(default)]
    pub servers: std::collections::HashMap<String, LspServerConfig>,
}

// ---------------------------------------------------------------------------
// Smart Key Vault
// ---------------------------------------------------------------------------

/// Configuration for the Smart Key Vault.
///
/// When enabled, the vault intercepts real API keys at the model boundary,
/// stores them encrypted locally, and hands the model format-mimicking proxy
/// keys instead. Proxy keys are silently restored before tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyVaultConfig {
    /// Enable the Smart Key Vault. Default: true.
    pub enabled: bool,
    /// Intercept API keys in chat input. Default: true.
    pub intercept_chat_input: bool,
    /// Intercept API keys when config is saved via the Settings UI. Default: true.
    pub intercept_config: bool,
    /// Intercept API keys that appear in tool results. Default: true.
    pub intercept_tool_results: bool,
    /// Restore proxy keys in tool inputs before tool execution. Default: true.
    pub restore_tool_inputs: bool,
    /// Optional remote sync endpoint (Phase 2 / deferred).
    #[serde(default)]
    pub remote_sync_url: Option<String>,
}

impl Default for KeyVaultConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            intercept_chat_input: true,
            intercept_config: true,
            intercept_tool_results: true,
            restore_tool_inputs: true,
            remote_sync_url: None,
        }
    }
}

#[allow(dead_code)]
fn default_true() -> bool {
    true
}

#[allow(dead_code)]
fn default_auto_compact_threshold() -> f64 {
    0.85
}

#[allow(dead_code)]
fn default_conversation_timeout() -> u64 {
    60
}

fn default_config_version() -> u32 {
    2
}

fn default_auto_discover_formats() -> Vec<String> {
    vec![
        "openclaw".to_string(),
        "codex".to_string(),
        "claude".to_string(),
    ]
}

/// Current config version
/// v2: Defense pipeline disabled by default, fail_open=true by default.
///     Config backup/restore on reinstall.
const CURRENT_CONFIG_VERSION: u32 = 2;

impl Default for NexiBotConfig {
    fn default() -> Self {
        Self {
            config_version: CURRENT_CONFIG_VERSION,
            logging: LoggingConfig::default(),
            claude: ClaudeConfig::default(),
            k2k: K2KConfig::default(),
            audio: AudioConfig::default(),
            wakeword: WakewordConfig::default(),
            vad: VadConfig::default(),
            stt: SttConfig::default(),
            tts: TtsConfig::default(),
            mcp: MCPConfig::default(),
            computer_use: ComputerUseConfig::default(),
            defense: crate::defense::DefenseConfig::default(),
            guardrails: crate::guardrails::GuardrailsConfig::default(),
            scheduled_tasks: ScheduledTasksConfig::default(),
            openai: OpenAIConfig::default(),
            cerebras: CerebrasConfig::default(),
            ollama: OllamaConfig::default(),
            lmstudio: LMStudioConfig::default(),
            webhooks: WebhookConfig::default(),
            browser: BrowserConfig::default(),
            telegram: TelegramConfig::default(),
            whatsapp: WhatsAppConfig::default(),
            discord: DiscordConfig::default(),
            slack: SlackConfig::default(),
            signal: SignalConfig::default(),
            teams: TeamsConfig::default(),
            matrix: MatrixConfig::default(),
            search: SearchConfig::default(),
            fetch: FetchConfig::default(),
            filesystem: FilesystemConfig::default(),
            execute: ExecuteConfig::default(),
            autonomous_mode: AutonomousModeConfig::default(),
            startup: StartupConfig::default(),
            agents: Vec::new(),
            defaults: None,
            google: None,
            deepseek: None,
            github_copilot: None,
            minimax: None,
            qwen: None,
            email: crate::email::EmailConfig::default(),
            gmail: crate::gmail::GmailConfig::default(),
            bluebubbles: crate::bluebubbles::BlueBubblesConfig::default(),
            google_chat: crate::google_chat::GoogleChatConfig::default(),
            mattermost: crate::mattermost::MattermostConfig::default(),
            messenger: crate::messenger::MessengerConfig::default(),
            instagram: crate::instagram::InstagramConfig::default(),
            line: crate::line::LineConfig::default(),
            twilio: crate::twilio::TwilioConfig::default(),
            mastodon: crate::mastodon::MastodonConfig::default(),
            rocketchat: crate::rocketchat::RocketChatConfig::default(),
            webchat: crate::webchat::WebChatConfig::default(),
            gateway: crate::gateway::GatewayConfig::default(),
            sandbox: crate::sandbox::SandboxConfig::default(),
            network_policy: crate::security::network_policy::NetworkPolicy::default(),
            agent_engine_url: None,
            routing: RoutingConfig::default(),
            key_vault: KeyVaultConfig::default(),
            gated_shell: GatedShellConfig::default(),
            yolo_mode: YoloModeConfig::default(),
            session_encryption: crate::security::session_encryption::SessionEncryptionConfig::default(),
            managed_policy: ManagedPolicyConfig::default(),
            external_skill_dirs: Vec::new(),
            auto_discover_formats: default_auto_discover_formats(),
            nats: NatsConfig::default(),
        }
    }
}

/// Check if a string is a valid memory limit string (e.g., "256m", "1g", "512M", "2G").
fn is_valid_memory_string(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let s_lower = s.to_lowercase();
    // Must end with a valid suffix
    let (num_part, suffix) = if s_lower.ends_with("gb") {
        (&s_lower[..s_lower.len() - 2], "gb")
    } else if s_lower.ends_with("mb") {
        (&s_lower[..s_lower.len() - 2], "mb")
    } else if s_lower.ends_with("kb") {
        (&s_lower[..s_lower.len() - 2], "kb")
    } else if s_lower.ends_with('g') {
        (&s_lower[..s_lower.len() - 1], "g")
    } else if s_lower.ends_with('m') {
        (&s_lower[..s_lower.len() - 1], "m")
    } else if s_lower.ends_with('k') {
        (&s_lower[..s_lower.len() - 1], "k")
    } else if s_lower.ends_with('b') {
        (&s_lower[..s_lower.len() - 1], "b")
    } else {
        return false;
    };

    let _ = suffix; // Used for matching above
                    // The numeric part must parse as a positive number
    if num_part.is_empty() {
        return false;
    }
    num_part.parse::<f64>().map(|n| n > 0.0).unwrap_or(false)
}

/// Check if a string contains path traversal characters.
fn contains_path_traversal_chars(s: &str) -> bool {
    s.contains("../") || s.contains("..\\") || s.contains('\0') || s.contains("..")
}

impl NexiBotConfig {
    /// Get the secondary backup directory path that survives app reinstalls.
    /// Uses ~/.config/nexibot/ which is separate from the
    /// ~/Library/Application Support/ directory that macOS may clean on uninstall.
    fn backup_dir() -> Result<PathBuf> {
        #[cfg(windows)]
        {
            let data = dirs::data_dir()
                .ok_or_else(|| anyhow::anyhow!("Failed to get data directory"))?;
            Ok(data.join("nexibot"))
        }
        #[cfg(not(windows))]
        {
            let home =
                dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?;
            Ok(home.join(".config/nexibot"))
        }
    }

    /// Path to the durable backup config (survives reinstalls).
    fn durable_backup_path() -> Result<PathBuf> {
        Ok(Self::backup_dir()?.join("config.yaml.backup"))
    }

    fn restore_proxy_in_string(
        vault: &crate::security::key_vault::KeyVault,
        field_name: &str,
        value: &mut String,
    ) {
        if value.is_empty() || !crate::security::key_vault::KeyVault::is_proxy_key(value) {
            return;
        }
        match vault.resolve(value) {
            Ok(Some(real)) => {
                *value = real;
            }
            Ok(None) => {
                warn!(
                    "[CONFIG] Vault proxy for '{}' not found; leaving value unchanged",
                    field_name
                );
            }
            Err(e) => {
                warn!(
                    "[CONFIG] Failed resolving vault proxy for '{}': {}",
                    field_name, e
                );
            }
        }
    }

    fn restore_proxy_in_option(
        vault: &crate::security::key_vault::KeyVault,
        field_name: &str,
        value: &mut Option<String>,
    ) {
        let Some(current) = value.clone() else {
            return;
        };
        if current.is_empty() || !crate::security::key_vault::KeyVault::is_proxy_key(&current) {
            return;
        }
        match vault.resolve(&current) {
            Ok(Some(real)) => {
                *value = Some(real);
            }
            Ok(None) => {
                warn!(
                    "[CONFIG] Vault proxy for '{}' not found; leaving value unchanged",
                    field_name
                );
            }
            Err(e) => {
                warn!(
                    "[CONFIG] Failed resolving vault proxy for '{}': {}",
                    field_name, e
                );
            }
        }
    }

    /// Resolve key-vault proxy tokens to real secrets for runtime use.
    ///
    /// Persisted configs may intentionally contain proxy values for secrets.
    /// Integrations/providers require the real values in memory.
    pub(crate) fn resolve_key_vault_proxies(&mut self) {
        use sha2::{Digest, Sha256};

        let home = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown-home".to_string());
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown-user".to_string());
        let derivation_input = format!(
            "nexibot-vault-v1:{}:{}:ai.nexibot.desktop",
            home, username
        );
        let passphrase = format!("{:x}", Sha256::digest(derivation_input.as_bytes()));

        let vault_db_path = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("nexibot/vault/key_vault.sqlite");
        if !vault_db_path.exists() {
            return;
        }

        let vault =
            match crate::security::key_vault::KeyVault::new(vault_db_path, &passphrase, true) {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        "[CONFIG] Failed to initialize key vault for proxy resolution: {}",
                        e
                    );
                    return;
                }
            };

        Self::restore_proxy_in_option(&vault, "claude.api_key", &mut self.claude.api_key);
        Self::restore_proxy_in_option(&vault, "openai.api_key", &mut self.openai.api_key);
        Self::restore_proxy_in_option(&vault, "cerebras.api_key", &mut self.cerebras.api_key);
        if let Some(ref mut google) = self.google {
            Self::restore_proxy_in_option(&vault, "google.api_key", &mut google.api_key);
        }
        if let Some(ref mut deepseek) = self.deepseek {
            Self::restore_proxy_in_option(&vault, "deepseek.api_key", &mut deepseek.api_key);
        }
        if let Some(ref mut copilot) = self.github_copilot {
            Self::restore_proxy_in_option(&vault, "github_copilot.token", &mut copilot.token);
        }
        if let Some(ref mut minimax) = self.minimax {
            Self::restore_proxy_in_option(&vault, "minimax.api_key", &mut minimax.api_key);
        }
        Self::restore_proxy_in_option(&vault, "k2k.private_key_pem", &mut self.k2k.private_key_pem);
        Self::restore_proxy_in_option(
            &vault,
            "search.brave_api_key",
            &mut self.search.brave_api_key,
        );
        Self::restore_proxy_in_option(
            &vault,
            "search.tavily_api_key",
            &mut self.search.tavily_api_key,
        );
        Self::restore_proxy_in_option(
            &vault,
            "stt.deepgram_api_key",
            &mut self.stt.deepgram_api_key,
        );
        Self::restore_proxy_in_option(&vault, "stt.openai_api_key", &mut self.stt.openai_api_key);
        Self::restore_proxy_in_option(
            &vault,
            "tts.elevenlabs_api_key",
            &mut self.tts.elevenlabs_api_key,
        );
        Self::restore_proxy_in_option(
            &vault,
            "tts.cartesia_api_key",
            &mut self.tts.cartesia_api_key,
        );
        Self::restore_proxy_in_option(&vault, "webhooks.auth_token", &mut self.webhooks.auth_token);
        Self::restore_proxy_in_string(&vault, "telegram.bot_token", &mut self.telegram.bot_token);
        Self::restore_proxy_in_string(&vault, "discord.bot_token", &mut self.discord.bot_token);
        Self::restore_proxy_in_string(
            &vault,
            "whatsapp.access_token",
            &mut self.whatsapp.access_token,
        );
        Self::restore_proxy_in_string(
            &vault,
            "whatsapp.verify_token",
            &mut self.whatsapp.verify_token,
        );
        Self::restore_proxy_in_string(&vault, "whatsapp.app_secret", &mut self.whatsapp.app_secret);
        Self::restore_proxy_in_string(&vault, "slack.bot_token", &mut self.slack.bot_token);
        Self::restore_proxy_in_string(&vault, "slack.app_token", &mut self.slack.app_token);
        Self::restore_proxy_in_string(
            &vault,
            "slack.signing_secret",
            &mut self.slack.signing_secret,
        );
        Self::restore_proxy_in_string(&vault, "teams.app_password", &mut self.teams.app_password);
        Self::restore_proxy_in_string(&vault, "matrix.access_token", &mut self.matrix.access_token);
        Self::restore_proxy_in_string(&vault, "email.imap_password", &mut self.email.imap_password);
        Self::restore_proxy_in_string(&vault, "email.smtp_password", &mut self.email.smtp_password);
        Self::restore_proxy_in_string(
            &vault,
            "bluebubbles.password",
            &mut self.bluebubbles.password,
        );
        Self::restore_proxy_in_string(
            &vault,
            "mattermost.bot_token",
            &mut self.mattermost.bot_token,
        );
        Self::restore_proxy_in_string(
            &vault,
            "google_chat.verification_token",
            &mut self.google_chat.verification_token,
        );
        Self::restore_proxy_in_string(
            &vault,
            "google_chat.incoming_webhook_url",
            &mut self.google_chat.incoming_webhook_url,
        );
        Self::restore_proxy_in_string(
            &vault,
            "messenger.page_access_token",
            &mut self.messenger.page_access_token,
        );
        Self::restore_proxy_in_string(
            &vault,
            "messenger.verify_token",
            &mut self.messenger.verify_token,
        );
        Self::restore_proxy_in_string(
            &vault,
            "messenger.app_secret",
            &mut self.messenger.app_secret,
        );
        Self::restore_proxy_in_string(
            &vault,
            "instagram.access_token",
            &mut self.instagram.access_token,
        );
        Self::restore_proxy_in_string(
            &vault,
            "instagram.verify_token",
            &mut self.instagram.verify_token,
        );
        Self::restore_proxy_in_string(
            &vault,
            "instagram.app_secret",
            &mut self.instagram.app_secret,
        );
        Self::restore_proxy_in_string(
            &vault,
            "line.channel_access_token",
            &mut self.line.channel_access_token,
        );
        Self::restore_proxy_in_string(&vault, "line.channel_secret", &mut self.line.channel_secret);
        Self::restore_proxy_in_string(&vault, "twilio.auth_token", &mut self.twilio.auth_token);
        Self::restore_proxy_in_string(
            &vault,
            "mastodon.access_token",
            &mut self.mastodon.access_token,
        );
        Self::restore_proxy_in_string(&vault, "rocketchat.password", &mut self.rocketchat.password);
        Self::restore_proxy_in_option(&vault, "webchat.api_key", &mut self.webchat.api_key);
    }

    /// Enforce restrictive file permissions on the config file.
    ///
    /// The config may contain plaintext secrets when the OS keyring is disabled.
    /// This ensures the file is readable only by the owning user at every load.
    /// On Unix: sets mode 0600. On Windows: restricts ACLs to the current user.
    fn enforce_config_permissions_inner(path: &std::path::Path) {
        if !path.exists() {
            return;
        }
        if let Err(e) = crate::platform::file_security::restrict_file_permissions(path) {
            tracing::warn!(
                "[CONFIG] Failed to enforce permissions on config file {:?}: {}",
                path,
                e
            );
        }
    }

    /// Save a backup copy of the config to the durable backup location.
    /// This is called after every successful load to keep the backup fresh.
    fn save_durable_backup(config_path: &std::path::Path) {
        match Self::durable_backup_path() {
            Ok(backup_path) => {
                if let Some(parent) = backup_path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        tracing::warn!(
                            "[CONFIG] Failed to create backup directory {:?}: {}",
                            parent,
                            e
                        );
                    }
                }
                match std::fs::copy(config_path, &backup_path) {
                    Ok(_) => {
                        tracing::debug!("[CONFIG] Durable backup saved to {:?}", backup_path);
                    }
                    Err(e) => {
                        tracing::warn!("[CONFIG] Failed to save durable backup: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[CONFIG] Failed to determine backup path: {}", e);
            }
        }
    }

    /// Try to restore config from backup locations.
    /// Checks (in order):
    ///   1. config.yaml.pre-build (in same dir as config)
    ///   2. config.yaml.bak (in same dir as config)
    ///   3. config.yaml.backup (durable backup in ~/.config/)
    /// Returns the restored config if any backup was found and valid.
    fn try_restore_from_backup(config_path: &std::path::Path) -> Option<Self> {
        let config_dir = config_path.parent()?;

        let backup_candidates: Vec<PathBuf> = vec![
            config_dir.join("config.yaml.pre-build"),
            config_dir.join("config.yaml.bak"),
            Self::durable_backup_path().ok().unwrap_or_default(),
        ];

        for backup in &backup_candidates {
            if !backup.exists() || backup.as_os_str().is_empty() {
                continue;
            }

            // Reject symlinks to prevent information disclosure via symlink following
            if let Ok(meta) = std::fs::symlink_metadata(backup) {
                if meta.file_type().is_symlink() {
                    tracing::warn!("[CONFIG] Backup {:?} is a symlink, skipping", backup);
                    continue;
                }
            }

            tracing::info!(
                "[CONFIG] Found backup at {:?}, attempting restore...",
                backup
            );
            match std::fs::read_to_string(backup) {
                Ok(content) => {
                    match serde_yml::from_str::<Self>(&content) {
                        Ok(config) => {
                            tracing::info!(
                                "[CONFIG] Successfully restored config from {:?}",
                                backup
                            );
                            // Copy the backup to the primary config location
                            if let Err(e) = std::fs::copy(backup, config_path) {
                                tracing::warn!(
                                    "[CONFIG] Failed to copy backup to config path: {}",
                                    e
                                );
                            }
                            return Some(config);
                        }
                        Err(e) => {
                            tracing::warn!("[CONFIG] Backup {:?} is invalid YAML: {}", backup, e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("[CONFIG] Failed to read backup {:?}: {}", backup, e);
                }
            }
        }

        None
    }

    /// Load configuration from file
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        // Enforce restrictive permissions on the config file at startup.
        // The config may contain plaintext secrets (keyring is disabled), so
        // it must be readable only by the owning user (0600 on Unix).
        Self::enforce_config_permissions_inner(&config_path);

        if !config_path.exists() {
            // Before creating defaults, try to restore from backup
            if let Some(mut config) = Self::try_restore_from_backup(&config_path) {
                tracing::info!("[CONFIG] Restored config from backup after reinstall/deletion");

                // Apply migrations if needed
                if config.config_version < CURRENT_CONFIG_VERSION {
                    tracing::info!(
                        "[CONFIG] Migrating restored config from version {} to {}",
                        config.config_version,
                        CURRENT_CONFIG_VERSION
                    );
                    Self::apply_migrations(&mut config);
                    config.config_version = CURRENT_CONFIG_VERSION;
                    config.save()?;
                }

                crate::security::credentials::resolve_keyring_secrets(
                    &mut config.claude.api_key,
                    &mut config.telegram.bot_token,
                    &mut config.whatsapp.access_token,
                    &mut config.webhooks.auth_token,
                );
                config.apply_env_overrides();
                config.resolve_key_vault_proxies();
                config.validate_and_clamp();

                // Update the durable backup with the (possibly migrated) config
                Self::save_durable_backup(&config_path);

                return Ok(config);
            }

            // No backup found — create default config
            let config = Self::default();
            config.save()?;
            Self::save_durable_backup(&config_path);
            return Ok(config);
        }

        let content = std::fs::read_to_string(&config_path)?;
        let mut config: Self = serde_yml::from_str(&content)?;

        // Apply migrations if config version is old
        if config.config_version < CURRENT_CONFIG_VERSION {
            tracing::info!(
                "[CONFIG] Migrating config from version {} to {}",
                config.config_version,
                CURRENT_CONFIG_VERSION
            );
            Self::apply_migrations(&mut config);
            config.config_version = CURRENT_CONFIG_VERSION;
            config.save()?;
        }

        // Resolve keyring-stored secrets before env overrides
        crate::security::credentials::resolve_keyring_secrets(
            &mut config.claude.api_key,
            &mut config.telegram.bot_token,
            &mut config.whatsapp.access_token,
            &mut config.webhooks.auth_token,
        );

        // Apply environment variable overrides (for Docker deployment)
        config.apply_env_overrides();
        config.resolve_key_vault_proxies();

        // Validate, clamp invalid values to safe defaults, and log warnings
        config.validate_and_clamp();
        let warnings = config.validate();
        for warning in &warnings {
            tracing::warn!("[CONFIG] Validation warning: {}", warning);
        }

        // Keep the durable backup fresh after every successful load
        Self::save_durable_backup(&config_path);

        Ok(config)
    }

    /// Apply environment variable overrides to config values.
    /// This allows Docker containers to configure NexiBot without a config file.
    fn apply_env_overrides(&mut self) {
        if let Ok(key) = std::env::var("CLAUDE_API_KEY") {
            if !key.is_empty() {
                self.claude.api_key = Some(key);
                info!("[CONFIG] Applied CLAUDE_API_KEY from environment");
            }
        }
        if let Ok(model) = std::env::var("CLAUDE_MODEL") {
            if !model.is_empty() {
                self.claude.model = model;
                info!("[CONFIG] Applied CLAUDE_MODEL from environment");
            }
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            if !key.is_empty() {
                self.openai.api_key = Some(key);
                info!("[CONFIG] Applied OPENAI_API_KEY from environment");
            }
        }
        if let Ok(url) = std::env::var("OLLAMA_URL") {
            if !url.is_empty() {
                self.ollama.url = url;
                self.ollama.enabled = true;
                info!("[CONFIG] Applied OLLAMA_URL from environment");
            }
        }
        if let Ok(token) = std::env::var("TELEGRAM_BOT_TOKEN") {
            if !token.is_empty() {
                self.telegram.bot_token = token;
                self.telegram.enabled = true;
                info!("[CONFIG] Applied TELEGRAM_BOT_TOKEN from environment");
            }
        }
        if let Ok(id) = std::env::var("WHATSAPP_PHONE_NUMBER_ID") {
            if !id.is_empty() {
                self.whatsapp.phone_number_id = id;
                info!("[CONFIG] Applied WHATSAPP_PHONE_NUMBER_ID from environment");
            }
        }
        if let Ok(token) = std::env::var("WHATSAPP_ACCESS_TOKEN") {
            if !token.is_empty() {
                self.whatsapp.access_token = token;
                info!("[CONFIG] Applied WHATSAPP_ACCESS_TOKEN from environment");
            }
        }
        if let Ok(token) = std::env::var("WHATSAPP_VERIFY_TOKEN") {
            if !token.is_empty() {
                self.whatsapp.verify_token = token;
                self.whatsapp.enabled = true;
                info!("[CONFIG] Applied WHATSAPP_VERIFY_TOKEN from environment");
            }
        }
        if let Ok(secret) = std::env::var("WHATSAPP_APP_SECRET") {
            if !secret.is_empty() {
                self.whatsapp.app_secret = secret;
                self.whatsapp.enabled = true;
                info!("[CONFIG] Applied WHATSAPP_APP_SECRET from environment");
            }
        }
    }

    /// Load config with profile overlay support.
    /// Looks for `config.{profile}.yaml` and merges over the base config.
    /// Profile is determined by `NEXIBOT_PROFILE` env var (default: none).
    #[allow(dead_code)]
    pub fn load_with_profile() -> Result<Self> {
        let mut config = Self::load()?;

        // Check for profile overlay
        if let Ok(profile) = std::env::var("NEXIBOT_PROFILE") {
            if !profile.is_empty() {
                // Path traversal prevention: reject profiles with dangerous characters
                if profile.contains("..")
                    || profile.contains('/')
                    || profile.contains('\\')
                    || profile.contains('\0')
                {
                    anyhow::bail!(
                        "Invalid profile name '{}': must not contain path separators or traversal sequences",
                        profile
                    );
                }
                // Additional validation: only allow alphanumeric, hyphens, underscores, dots
                if !profile
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
                {
                    anyhow::bail!(
                        "Invalid profile name '{}': only alphanumeric, hyphen, underscore, and dot characters are allowed",
                        profile
                    );
                }

                let config_dir = Self::config_path()?
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("No config parent dir"))?
                    .to_path_buf();
                let profile_path = config_dir.join(format!("config.{}.yaml", profile));

                // Verify the resolved path is within config_dir (catches symlink escapes)
                let canonical_dir = config_dir
                    .canonicalize()
                    .unwrap_or_else(|_| config_dir.clone());
                if profile_path.exists() {
                    let canonical_profile = profile_path
                        .canonicalize()
                        .context("Failed to canonicalize profile path")?;
                    if !canonical_profile.starts_with(&canonical_dir) {
                        anyhow::bail!(
                            "Profile path {:?} escapes config directory {:?}",
                            canonical_profile,
                            canonical_dir
                        );
                    }
                }

                if profile_path.exists() {
                    info!("[CONFIG] Loading profile overlay: {}", profile);
                    let overlay_content = std::fs::read_to_string(&profile_path)?;
                    let overlay: serde_yml::Value = serde_yml::from_str(&overlay_content)?;
                    let base: serde_yml::Value = serde_yml::to_value(&config)?;
                    let merged = deep_merge_yaml(base, overlay);
                    config = serde_yml::from_value(merged)?;
                } else {
                    warn!(
                        "[CONFIG] Profile '{}' not found at {:?}",
                        profile, profile_path
                    );
                }
            }
        }

        // Apply NEXIBOT_* env var overrides (on top of profile)
        config.apply_nexibot_env_overrides();
        config.resolve_key_vault_proxies();

        Ok(config)
    }

    /// Apply `NEXIBOT_*` environment variable overrides to config fields.
    ///
    /// Converts env var names to config paths using a known mapping:
    /// - `NEXIBOT_CLAUDE_MODEL` -> `claude.model`
    /// - `NEXIBOT_CLAUDE_MAX_TOKENS` -> `claude.max_tokens`
    /// - `NEXIBOT_DEFENSE_ENABLED` -> `defense.enabled`
    /// - `NEXIBOT_SCHEDULED_TASKS_ENABLED` -> `scheduled_tasks.enabled`
    /// - `NEXIBOT_MCP_ENABLED` -> `mcp.enabled`
    /// - `NEXIBOT_COMPUTER_USE_ENABLED` -> `computer_use.enabled`
    /// - `NEXIBOT_BROWSER_ENABLED` -> `browser.enabled`
    /// - `NEXIBOT_BROWSER_HEADLESS` -> `browser.headless`
    /// - `NEXIBOT_EXECUTE_ENABLED` -> `execute.enabled`
    /// - `NEXIBOT_FETCH_ENABLED` -> `fetch.enabled`
    /// - `NEXIBOT_FILESYSTEM_ENABLED` -> `filesystem.enabled`
    /// - `NEXIBOT_WEBHOOKS_ENABLED` -> `webhooks.enabled`
    /// - `NEXIBOT_WEBHOOKS_PORT` -> `webhooks.port`
    /// - `NEXIBOT_OLLAMA_ENABLED` -> `ollama.enabled`
    /// - `NEXIBOT_OLLAMA_URL` -> `ollama.url`
    /// - `NEXIBOT_OLLAMA_MODEL` -> `ollama.model`
    #[allow(dead_code)]
    pub fn apply_nexibot_env_overrides(&mut self) {
        // String overrides
        if let Ok(val) = std::env::var("NEXIBOT_CLAUDE_MODEL") {
            if !val.is_empty() {
                info!("[CONFIG] NEXIBOT_CLAUDE_MODEL override -> {}", val);
                self.claude.model = val;
            }
        }
        if let Ok(val) = std::env::var("NEXIBOT_OLLAMA_URL") {
            if !val.is_empty() {
                info!("[CONFIG] NEXIBOT_OLLAMA_URL override -> {}", val);
                self.ollama.url = val;
            }
        }
        if let Ok(val) = std::env::var("NEXIBOT_OLLAMA_MODEL") {
            if !val.is_empty() {
                info!("[CONFIG] NEXIBOT_OLLAMA_MODEL override -> {}", val);
                self.ollama.model = val;
            }
        }

        // usize overrides
        if let Ok(val) = std::env::var("NEXIBOT_CLAUDE_MAX_TOKENS") {
            if let Ok(n) = val.parse::<usize>() {
                info!("[CONFIG] NEXIBOT_CLAUDE_MAX_TOKENS override -> {}", n);
                self.claude.max_tokens = n;
            }
        }

        // u16 overrides
        if let Ok(val) = std::env::var("NEXIBOT_WEBHOOKS_PORT") {
            if let Ok(n) = val.parse::<u16>() {
                info!("[CONFIG] NEXIBOT_WEBHOOKS_PORT override -> {}", n);
                self.webhooks.port = n;
            }
        }

        // Boolean overrides (accepts "true"/"false"/"1"/"0")
        let bool_overrides: Vec<(&str, Box<dyn FnMut(&mut Self, bool)>)> = vec![
            (
                "NEXIBOT_DEFENSE_ENABLED",
                Box::new(|c: &mut Self, v| c.defense.enabled = v),
            ),
            (
                "NEXIBOT_SCHEDULED_TASKS_ENABLED",
                Box::new(|c: &mut Self, v| c.scheduled_tasks.enabled = v),
            ),
            (
                "NEXIBOT_MCP_ENABLED",
                Box::new(|c: &mut Self, v| c.mcp.enabled = v),
            ),
            (
                "NEXIBOT_COMPUTER_USE_ENABLED",
                Box::new(|c: &mut Self, v| c.computer_use.enabled = v),
            ),
            (
                "NEXIBOT_BROWSER_ENABLED",
                Box::new(|c: &mut Self, v| c.browser.enabled = v),
            ),
            (
                "NEXIBOT_BROWSER_HEADLESS",
                Box::new(|c: &mut Self, v| c.browser.headless = v),
            ),
            (
                "NEXIBOT_EXECUTE_ENABLED",
                Box::new(|c: &mut Self, v| c.execute.enabled = v),
            ),
            (
                "NEXIBOT_FETCH_ENABLED",
                Box::new(|c: &mut Self, v| c.fetch.enabled = v),
            ),
            (
                "NEXIBOT_FILESYSTEM_ENABLED",
                Box::new(|c: &mut Self, v| c.filesystem.enabled = v),
            ),
            (
                "NEXIBOT_WEBHOOKS_ENABLED",
                Box::new(|c: &mut Self, v| c.webhooks.enabled = v),
            ),
            (
                "NEXIBOT_OLLAMA_ENABLED",
                Box::new(|c: &mut Self, v| c.ollama.enabled = v),
            ),
        ];

        for (env_key, mut setter) in bool_overrides {
            if let Ok(val) = std::env::var(env_key) {
                match val.to_lowercase().as_str() {
                    "true" | "1" => {
                        info!("[CONFIG] {} override -> true", env_key);
                        setter(self, true);
                    }
                    "false" | "0" => {
                        info!("[CONFIG] {} override -> false", env_key);
                        setter(self, false);
                    }
                    _ => {
                        warn!("[CONFIG] {} has invalid boolean value: '{}'", env_key, val);
                    }
                }
            }
        }
    }

    /// Apply version migrations to a config loaded from an older version.
    fn apply_migrations(config: &mut Self) {
        // Migration v1 -> v2: Defense pipeline defaults changed.
        // Disable defense for existing users who had it accidentally enabled so they
        // aren't blocked on model-download failures after updating.
        // Preserve the user's existing fail_open value — do NOT overwrite it.
        if config.config_version < 2 {
            tracing::info!("[CONFIG] Migration v1→v2: Changing defense defaults to safe values");
            // Disable defense by default — users who want it can re-enable it.
            // This prevents the DeBERTa model download + fail-closed blocking on fresh installs.
            config.defense.enabled = false;
            tracing::info!("[CONFIG] Migration v1→v2: Set defense.enabled=false (opt-in only)");
            // NOTE: fail_open is intentionally left as-is (preserve user's setting).
        }
    }

    /// Warn if credential fields are being cleared (potential data loss).
    /// This is a non-blocking warning — saves still proceed, but the log
    /// provides a breadcrumb for diagnosing credential loss incidents.
    fn warn_if_credentials_cleared(config: &NexiBotConfig) {
        let credential_checks: &[(&str, &str)] = &[
            ("claude.api_key", config.claude.api_key.as_deref().unwrap_or("")),
            ("telegram.bot_token", &config.telegram.bot_token),
            ("whatsapp.access_token", &config.whatsapp.access_token),
            ("discord.bot_token", &config.discord.bot_token),
            ("slack.bot_token", &config.slack.bot_token),
            ("slack.app_token", &config.slack.app_token),
            ("teams.app_password", &config.teams.app_password),
            ("matrix.access_token", &config.matrix.access_token),
            ("email.imap_password", &config.email.imap_password),
            ("email.smtp_password", &config.email.smtp_password),
            ("mattermost.bot_token", &config.mattermost.bot_token),
            ("mastodon.access_token", &config.mastodon.access_token),
        ];

        for (field, value) in credential_checks {
            if value.is_empty() {
                tracing::warn!(
                    "[CONFIG] Credential field '{}' is empty — this may indicate credential loss. \
                     If this is unexpected, restore from config.yaml.bak",
                    field
                );
            }
        }
    }

    /// Preserve in-memory credential values during hot-reload.
    ///
    /// When the config file is modified externally (e.g. by `sed`, a script, or
    /// a manual edit), credential fields may end up empty in the file while the
    /// in-memory config still holds the real values. This function copies
    /// non-empty in-memory credentials into the newly loaded config wherever the
    /// file had them as empty, preventing credential loss.
    ///
    /// **IMPORTANT**: Never directly edit credential fields in config.yaml with
    /// sed or similar tools. Use the Settings UI or the `update_config` command
    /// which preserves credentials through the restore_if_masked logic.
    pub(crate) fn preserve_credentials_on_reload(
        current: &NexiBotConfig,
        incoming: &mut NexiBotConfig,
    ) {
        // Helper: restore Option<String> credential if incoming is empty/None
        fn restore_opt(current: &Option<String>, incoming: &mut Option<String>, field: &str) {
            match incoming {
                Some(ref val) if val.is_empty() => {
                    if current.as_ref().map(|v| !v.is_empty()).unwrap_or(false) {
                        tracing::warn!(
                            "[HOT_RELOAD] CREDENTIAL PRESERVED: '{}' was empty in file but \
                             had a value in memory — keeping in-memory value",
                            field
                        );
                        *incoming = current.clone();
                    }
                }
                None => {
                    if current.is_some() {
                        tracing::warn!(
                            "[HOT_RELOAD] CREDENTIAL PRESERVED: '{}' was missing in file but \
                             had a value in memory — keeping in-memory value",
                            field
                        );
                        *incoming = current.clone();
                    }
                }
                _ => {} // incoming has a non-empty value — keep it
            }
        }

        // Helper: restore String credential if incoming is empty
        fn restore_str(current: &str, incoming: &mut String, field: &str) {
            if incoming.is_empty() && !current.is_empty() {
                tracing::warn!(
                    "[HOT_RELOAD] CREDENTIAL PRESERVED: '{}' was empty in file but \
                     had a value in memory — keeping in-memory value",
                    field
                );
                *incoming = current.to_string();
            }
        }

        // Option<String> credential fields
        restore_opt(&current.claude.api_key, &mut incoming.claude.api_key, "claude.api_key");
        restore_opt(&current.openai.api_key, &mut incoming.openai.api_key, "openai.api_key");
        restore_opt(&current.cerebras.api_key, &mut incoming.cerebras.api_key, "cerebras.api_key");
        if let (Some(cur_g), Some(new_g)) = (current.google.as_ref(), incoming.google.as_mut()) {
            restore_opt(&cur_g.api_key, &mut new_g.api_key, "google.api_key");
        }
        if let (Some(cur_d), Some(new_d)) = (current.deepseek.as_ref(), incoming.deepseek.as_mut()) {
            restore_opt(&cur_d.api_key, &mut new_d.api_key, "deepseek.api_key");
        }
        if let (Some(cur_c), Some(new_c)) = (current.github_copilot.as_ref(), incoming.github_copilot.as_mut()) {
            restore_opt(&cur_c.token, &mut new_c.token, "github_copilot.token");
        }
        if let (Some(cur_m), Some(new_m)) = (current.minimax.as_ref(), incoming.minimax.as_mut()) {
            restore_opt(&cur_m.api_key, &mut new_m.api_key, "minimax.api_key");
        }
        if let (Some(cur_q), Some(new_q)) = (current.qwen.as_ref(), incoming.qwen.as_mut()) {
            restore_opt(&cur_q.api_key, &mut new_q.api_key, "qwen.api_key");
        }
        restore_opt(&current.k2k.private_key_pem, &mut incoming.k2k.private_key_pem, "k2k.private_key_pem");
        restore_opt(&current.search.brave_api_key, &mut incoming.search.brave_api_key, "search.brave_api_key");
        restore_opt(&current.search.tavily_api_key, &mut incoming.search.tavily_api_key, "search.tavily_api_key");
        restore_opt(&current.stt.deepgram_api_key, &mut incoming.stt.deepgram_api_key, "stt.deepgram_api_key");
        restore_opt(&current.stt.openai_api_key, &mut incoming.stt.openai_api_key, "stt.openai_api_key");
        restore_opt(&current.tts.elevenlabs_api_key, &mut incoming.tts.elevenlabs_api_key, "tts.elevenlabs_api_key");
        restore_opt(&current.tts.cartesia_api_key, &mut incoming.tts.cartesia_api_key, "tts.cartesia_api_key");
        restore_opt(&current.webhooks.auth_token, &mut incoming.webhooks.auth_token, "webhooks.auth_token");
        restore_opt(&current.webchat.api_key, &mut incoming.webchat.api_key, "webchat.api_key");

        // Plain String credential fields
        restore_str(&current.telegram.bot_token, &mut incoming.telegram.bot_token, "telegram.bot_token");
        restore_str(&current.discord.bot_token, &mut incoming.discord.bot_token, "discord.bot_token");
        restore_str(&current.whatsapp.access_token, &mut incoming.whatsapp.access_token, "whatsapp.access_token");
        restore_str(&current.whatsapp.verify_token, &mut incoming.whatsapp.verify_token, "whatsapp.verify_token");
        restore_str(&current.whatsapp.app_secret, &mut incoming.whatsapp.app_secret, "whatsapp.app_secret");
        restore_str(&current.slack.bot_token, &mut incoming.slack.bot_token, "slack.bot_token");
        restore_str(&current.slack.app_token, &mut incoming.slack.app_token, "slack.app_token");
        restore_str(&current.slack.signing_secret, &mut incoming.slack.signing_secret, "slack.signing_secret");
        restore_str(&current.teams.app_password, &mut incoming.teams.app_password, "teams.app_password");
        restore_str(&current.matrix.access_token, &mut incoming.matrix.access_token, "matrix.access_token");
        restore_str(&current.email.imap_password, &mut incoming.email.imap_password, "email.imap_password");
        restore_str(&current.email.smtp_password, &mut incoming.email.smtp_password, "email.smtp_password");
        restore_str(&current.gmail.client_secret, &mut incoming.gmail.client_secret, "gmail.client_secret");
        restore_str(&current.gmail.refresh_token, &mut incoming.gmail.refresh_token, "gmail.refresh_token");
        restore_str(&current.bluebubbles.password, &mut incoming.bluebubbles.password, "bluebubbles.password");
        restore_str(&current.mattermost.bot_token, &mut incoming.mattermost.bot_token, "mattermost.bot_token");
        restore_str(&current.google_chat.verification_token, &mut incoming.google_chat.verification_token, "google_chat.verification_token");
        restore_str(&current.google_chat.incoming_webhook_url, &mut incoming.google_chat.incoming_webhook_url, "google_chat.incoming_webhook_url");
        restore_str(&current.messenger.page_access_token, &mut incoming.messenger.page_access_token, "messenger.page_access_token");
        restore_str(&current.messenger.verify_token, &mut incoming.messenger.verify_token, "messenger.verify_token");
        restore_str(&current.messenger.app_secret, &mut incoming.messenger.app_secret, "messenger.app_secret");
        restore_str(&current.instagram.access_token, &mut incoming.instagram.access_token, "instagram.access_token");
        restore_str(&current.instagram.verify_token, &mut incoming.instagram.verify_token, "instagram.verify_token");
        restore_str(&current.instagram.app_secret, &mut incoming.instagram.app_secret, "instagram.app_secret");
        restore_str(&current.line.channel_access_token, &mut incoming.line.channel_access_token, "line.channel_access_token");
        restore_str(&current.line.channel_secret, &mut incoming.line.channel_secret, "line.channel_secret");
        restore_str(&current.twilio.auth_token, &mut incoming.twilio.auth_token, "twilio.auth_token");
        restore_str(&current.mastodon.access_token, &mut incoming.mastodon.access_token, "mastodon.access_token");
        restore_str(&current.rocketchat.password, &mut incoming.rocketchat.password, "rocketchat.password");
    }

    /// Save configuration to file (atomic write)
    pub fn save(&self) -> Result<()> {
        // Reject saves with critical validation errors
        self.validate_for_save()
            .context("Refusing to save config with critical validation errors")?;

        // Warn if credential fields are being cleared (potential data loss)
        Self::warn_if_credentials_cleared(self);

        let config_path = Self::config_path()?;

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Backup existing config before overwriting — this is the last-resort
        // recovery point if credentials are lost. The backup MUST succeed before
        // we proceed with the write.
        if config_path.exists() {
            let bak_path = config_path.with_extension("yaml.bak");
            std::fs::copy(&config_path, &bak_path).with_context(|| {
                format!(
                    "Failed to create config backup at {:?} — refusing to overwrite config",
                    bak_path
                )
            })?;
            tracing::debug!("[CONFIG] Backup created at {:?}", bak_path);
        }

        // Move secrets to OS keyring, replacing values with placeholders
        let mut save_copy = self.clone();
        let stored = crate::security::credentials::store_secrets_to_keyring(
            &mut save_copy.claude.api_key,
            &mut save_copy.telegram.bot_token,
            &mut save_copy.whatsapp.access_token,
            &mut save_copy.webhooks.auth_token,
        );
        if stored > 0 {
            tracing::info!("[CONFIG] Stored {} secrets to OS keyring", stored);
        }

        let content = serde_yml::to_string(&save_copy)?;

        // Atomic save: write to temp file first, then rename
        let tmp_path = config_path.with_extension("yaml.tmp");
        std::fs::write(&tmp_path, &content)?;

        // Verify the temp file is valid YAML before committing
        let verify_content = std::fs::read_to_string(&tmp_path)?;
        let _: serde_yml::Value = serde_yml::from_str(&verify_content).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            anyhow::anyhow!("Config validation failed after write: {}", e)
        })?;

        // Rename (atomic on most filesystems)
        std::fs::rename(&tmp_path, &config_path)?;

        // Set restrictive permissions on config file (contains API keys)
        crate::platform::file_security::restrict_file_permissions(&config_path)?;

        // Also update the durable backup (survives app reinstalls)
        Self::save_durable_backup(&config_path);

        Ok(())
    }

    /// Validate configuration values, returning warnings for invalid settings
    pub fn validate(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        // ── Numeric range validations ──────────────────────────────
        if self.claude.max_tokens < 1 || self.claude.max_tokens > 1_000_000 {
            warnings.push(format!(
                "claude.max_tokens {} is out of range [1, 1000000]",
                self.claude.max_tokens
            ));
        }
        if self.gateway.port < 1024 {
            warnings.push(format!(
                "gateway.port {} is below minimum 1024",
                self.gateway.port
            ));
        }
        if self.gateway.max_connections < 1 || self.gateway.max_connections > 10_000 {
            warnings.push(format!(
                "gateway.max_connections {} is out of range [1, 10000]",
                self.gateway.max_connections
            ));
        }
        // Validate gateway.bind_address is a valid IP
        if self
            .gateway
            .bind_address
            .parse::<std::net::IpAddr>()
            .is_err()
        {
            warnings.push(format!(
                "gateway.bind_address '{}' is not a valid IP address",
                self.gateway.bind_address
            ));
        }

        // ── Threshold validations (0.0-1.0) ───────────────────────
        if self.wakeword.threshold < 0.0 || self.wakeword.threshold > 1.0 {
            warnings.push(format!(
                "Wakeword threshold {} is out of range [0.0, 1.0]",
                self.wakeword.threshold
            ));
        }
        if self.vad.threshold < 0.0 || self.vad.threshold > 1.0 {
            warnings.push(format!(
                "VAD threshold {} is out of range [0.0, 1.0]",
                self.vad.threshold
            ));
        }
        if self.defense.deberta_threshold < 0.0 || self.defense.deberta_threshold > 1.0 {
            warnings.push(format!(
                "defense.deberta_threshold {} is out of range [0.0, 1.0]",
                self.defense.deberta_threshold
            ));
        }

        // ── Sandbox validations ────────────────────────────────────
        if self.sandbox.timeout_seconds < 1 || self.sandbox.timeout_seconds > 3600 {
            warnings.push(format!(
                "sandbox.timeout_seconds {} is out of range [1, 3600]",
                self.sandbox.timeout_seconds
            ));
        }
        if self.sandbox.cpu_limit < 0.1 || self.sandbox.cpu_limit > 16.0 {
            warnings.push(format!(
                "sandbox.cpu_limit {} is out of range [0.1, 16.0]",
                self.sandbox.cpu_limit
            ));
        }
        if !is_valid_memory_string(&self.sandbox.memory_limit) {
            warnings.push(format!(
                "sandbox.memory_limit '{}' is not a valid memory string (expected e.g. '256m', '1g')",
                self.sandbox.memory_limit
            ));
        }

        // ── URL validations (http/https only) ─────────────────────
        for (name, url_str) in self.collect_url_fields() {
            if url_str.is_empty() {
                continue;
            }
            match url::Url::parse(&url_str) {
                Ok(parsed) => {
                    if parsed.scheme() != "http" && parsed.scheme() != "https" {
                        warnings.push(format!(
                            "{} '{}' uses disallowed scheme '{}' (only http/https allowed)",
                            name,
                            url_str,
                            parsed.scheme()
                        ));
                    }
                }
                Err(_) => {
                    warnings.push(format!("{} '{}' is not a valid URL", name, url_str));
                }
            }
        }

        // ── Profile name validation (path traversal) ──────────────
        for agent in &self.agents {
            if contains_path_traversal_chars(&agent.id) {
                warnings.push(format!(
                    "Agent ID '{}' contains path traversal characters",
                    agent.id
                ));
            }
            if contains_path_traversal_chars(&agent.name) {
                warnings.push(format!(
                    "Agent name '{}' contains path traversal characters",
                    agent.name
                ));
            }
        }

        // ── Channel credential warnings ─────────────────────────
        if self.slack.enabled {
            if self.slack.signing_secret.is_empty() {
                warnings.push(
                    "Slack enabled but signing_secret is empty — webhook verification will fail"
                        .to_string(),
                );
            }
            if self.slack.bot_token.is_empty() {
                warnings.push(
                    "Slack enabled but bot_token is empty — cannot send messages".to_string(),
                );
            }
            if self.slack.app_token.is_empty() {
                warnings.push(
                    "Slack enabled but app_token is empty — Socket Mode unavailable".to_string(),
                );
            }
        }
        if self.telegram.enabled && self.telegram.bot_token.is_empty() {
            warnings.push("Telegram enabled but bot_token is empty".to_string());
        }
        if self.discord.enabled && self.discord.bot_token.is_empty() {
            warnings.push("Discord enabled but token is empty".to_string());
        }

        // ── TTS speed range ─────────────────────────────────────────
        if let Some(speed) = self.tts.cartesia_speed {
            if !(0.1..=3.0).contains(&speed) {
                warnings.push(format!(
                    "tts.cartesia_speed {} is out of range [0.1, 3.0]",
                    speed
                ));
            }
        }

        // ── Backend-specific API key warnings ───────────────────────
        if self.tts.backend == "cartesia"
            && self
                .tts
                .cartesia_api_key
                .as_deref()
                .unwrap_or("")
                .is_empty()
        {
            warnings
                .push("TTS backend 'cartesia' selected but cartesia_api_key is empty".to_string());
        }
        if self.stt.backend == "deepgram"
            && self
                .stt
                .deepgram_api_key
                .as_deref()
                .unwrap_or("")
                .is_empty()
        {
            warnings
                .push("STT backend 'deepgram' selected but deepgram_api_key is empty".to_string());
        }

        // ── SSRF hostname checks for config URLs ────────────────────
        for (name, url_str) in self.collect_url_fields() {
            if url_str.is_empty() {
                continue;
            }
            if let Ok(parsed) = url::Url::parse(&url_str) {
                if let Some(host) = parsed.host_str() {
                    let host_lower = host.to_lowercase();
                    let is_blocked = host_lower == "169.254.169.254"
                        || host_lower == "metadata.google.internal"
                        || host_lower.ends_with(".internal")
                        || host_lower.ends_with(".localhost")
                        || host_lower == "metadata.aws"
                        || host_lower == "[fd00:ec2::254]";
                    if is_blocked {
                        warnings.push(format!(
                            "{} '{}' points to a blocked SSRF hostname '{}'",
                            name, url_str, host
                        ));
                    }
                }
            }
        }

        // ── File path traversal validations ───────────────────────
        if let Some(ref path) = self.defense.deberta_model_path {
            let p = std::path::PathBuf::from(path);
            if let Err(e) = crate::security::path_validation::validate_config_path(&p) {
                warnings.push(format!("DeBERTa model path '{}': {}", path, e));
            }
        }

        warnings
    }

    /// Validate and clamp config values to safe defaults on load.
    /// Logs warnings for any values that were clamped.
    pub fn validate_and_clamp(&mut self) {
        // ── claude.max_tokens: 1-1,000,000 ────────────────────────
        if self.claude.max_tokens < 1 {
            warn!(
                "[CONFIG] claude.max_tokens {} < 1, clamping to 1",
                self.claude.max_tokens
            );
            self.claude.max_tokens = 1;
        } else if self.claude.max_tokens > 1_000_000 {
            warn!(
                "[CONFIG] claude.max_tokens {} > 1000000, clamping to 1000000",
                self.claude.max_tokens
            );
            self.claude.max_tokens = 1_000_000;
        }

        // ── gateway.port: 1024-65535 ──────────────────────────────
        if self.gateway.port < 1024 {
            warn!(
                "[CONFIG] gateway.port {} < 1024, clamping to 1024",
                self.gateway.port
            );
            self.gateway.port = 1024;
        }
        // u16 max is 65535, so no upper clamp needed

        // ── gateway.max_connections: 1-10,000 ─────────────────────
        if self.gateway.max_connections < 1 {
            warn!(
                "[CONFIG] gateway.max_connections {} < 1, clamping to 1",
                self.gateway.max_connections
            );
            self.gateway.max_connections = 1;
        } else if self.gateway.max_connections > 10_000 {
            warn!(
                "[CONFIG] gateway.max_connections {} > 10000, clamping to 10000",
                self.gateway.max_connections
            );
            self.gateway.max_connections = 10_000;
        }

        // ── gateway.bind_address: must be valid IP ────────────────
        if self
            .gateway
            .bind_address
            .parse::<std::net::IpAddr>()
            .is_err()
        {
            warn!(
                "[CONFIG] gateway.bind_address '{}' is not a valid IP, resetting to 127.0.0.1",
                self.gateway.bind_address
            );
            self.gateway.bind_address = "127.0.0.1".to_string();
        }

        // ── defense.deberta_threshold: 0.0-1.0 ───────────────────
        if self.defense.deberta_threshold < 0.0 {
            warn!(
                "[CONFIG] defense.deberta_threshold {} < 0.0, clamping to 0.0",
                self.defense.deberta_threshold
            );
            self.defense.deberta_threshold = 0.0;
        } else if self.defense.deberta_threshold > 1.0 {
            warn!(
                "[CONFIG] defense.deberta_threshold {} > 1.0, clamping to 1.0",
                self.defense.deberta_threshold
            );
            self.defense.deberta_threshold = 1.0;
        }

        // ── sandbox.timeout_seconds: 1-3600 ──────────────────────
        if self.sandbox.timeout_seconds < 1 {
            warn!(
                "[CONFIG] sandbox.timeout_seconds {} < 1, clamping to 1",
                self.sandbox.timeout_seconds
            );
            self.sandbox.timeout_seconds = 1;
        } else if self.sandbox.timeout_seconds > 3600 {
            warn!(
                "[CONFIG] sandbox.timeout_seconds {} > 3600, clamping to 3600",
                self.sandbox.timeout_seconds
            );
            self.sandbox.timeout_seconds = 3600;
        }

        // ── sandbox.cpu_limit: 0.1-16.0 ──────────────────────────
        if self.sandbox.cpu_limit < 0.1 {
            warn!(
                "[CONFIG] sandbox.cpu_limit {} < 0.1, clamping to 0.1",
                self.sandbox.cpu_limit
            );
            self.sandbox.cpu_limit = 0.1;
        } else if self.sandbox.cpu_limit > 16.0 {
            warn!(
                "[CONFIG] sandbox.cpu_limit {} > 16.0, clamping to 16.0",
                self.sandbox.cpu_limit
            );
            self.sandbox.cpu_limit = 16.0;
        }

        // ── sandbox.memory_limit: must be valid ───────────────────
        if !is_valid_memory_string(&self.sandbox.memory_limit) {
            warn!(
                "[CONFIG] sandbox.memory_limit '{}' is invalid, resetting to '512m'",
                self.sandbox.memory_limit
            );
            self.sandbox.memory_limit = "512m".to_string();
        }

        // ── tts.cartesia_speed: 0.1-3.0 ────────────────────────────
        if let Some(speed) = self.tts.cartesia_speed {
            if speed < 0.1 {
                warn!(
                    "[CONFIG] tts.cartesia_speed {} < 0.1, clamping to 0.1",
                    speed
                );
                self.tts.cartesia_speed = Some(0.1);
            } else if speed > 3.0 {
                warn!(
                    "[CONFIG] tts.cartesia_speed {} > 3.0, clamping to 3.0",
                    speed
                );
                self.tts.cartesia_speed = Some(3.0);
            }
        }
    }

    /// Validate configuration for save. Returns errors for critical validation
    /// failures that should prevent saving.
    pub fn validate_for_save(&self) -> Result<()> {
        let mut errors = Vec::new();

        // Critical: numeric ranges
        if self.claude.max_tokens < 1 || self.claude.max_tokens > 1_000_000 {
            errors.push(format!(
                "claude.max_tokens {} is out of range [1, 1000000]",
                self.claude.max_tokens
            ));
        }
        if self.gateway.port < 1024 {
            errors.push(format!(
                "gateway.port {} is below minimum 1024",
                self.gateway.port
            ));
        }
        if self.gateway.max_connections < 1 || self.gateway.max_connections > 10_000 {
            errors.push(format!(
                "gateway.max_connections {} is out of range [1, 10000]",
                self.gateway.max_connections
            ));
        }

        // Critical: bind address must be a valid IP
        if self
            .gateway
            .bind_address
            .parse::<std::net::IpAddr>()
            .is_err()
        {
            errors.push(format!(
                "gateway.bind_address '{}' is not a valid IP address",
                self.gateway.bind_address
            ));
        }

        // Critical: thresholds
        if self.defense.deberta_threshold < 0.0 || self.defense.deberta_threshold > 1.0 {
            errors.push(format!(
                "defense.deberta_threshold {} is out of range [0.0, 1.0]",
                self.defense.deberta_threshold
            ));
        }

        // Critical: sandbox values
        if self.sandbox.timeout_seconds < 1 || self.sandbox.timeout_seconds > 3600 {
            errors.push(format!(
                "sandbox.timeout_seconds {} is out of range [1, 3600]",
                self.sandbox.timeout_seconds
            ));
        }
        if self.sandbox.cpu_limit < 0.1 || self.sandbox.cpu_limit > 16.0 {
            errors.push(format!(
                "sandbox.cpu_limit {} is out of range [0.1, 16.0]",
                self.sandbox.cpu_limit
            ));
        }
        if !is_valid_memory_string(&self.sandbox.memory_limit) {
            errors.push(format!(
                "sandbox.memory_limit '{}' is not a valid memory string",
                self.sandbox.memory_limit
            ));
        }

        // Critical: URL fields must be valid http/https
        for (name, url_str) in self.collect_url_fields() {
            if url_str.is_empty() {
                continue;
            }
            match url::Url::parse(&url_str) {
                Ok(parsed) => {
                    if parsed.scheme() != "http" && parsed.scheme() != "https" {
                        errors.push(format!(
                            "{} uses disallowed scheme '{}'",
                            name,
                            parsed.scheme()
                        ));
                    }
                }
                Err(e) => {
                    errors.push(format!("{} '{}' is not a valid URL: {}", name, url_str, e));
                }
            }
        }

        // Critical: profile/agent names must not have path traversal
        for agent in &self.agents {
            if contains_path_traversal_chars(&agent.id) {
                errors.push(format!(
                    "Agent ID '{}' contains path traversal characters",
                    agent.id
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(
                "Config validation failed with {} error(s):\n  - {}",
                errors.len(),
                errors.join("\n  - ")
            )
        }
    }

    /// Collect all URL fields for validation.
    fn collect_url_fields(&self) -> Vec<(&'static str, String)> {
        let mut urls = Vec::new();
        urls.push(("k2k.local_agent_url", self.k2k.local_agent_url.clone()));
        if let Some(ref url) = self.k2k.router_url {
            urls.push(("k2k.router_url", url.clone()));
        }
        urls.push(("ollama.url", self.ollama.url.clone()));
        if !self.defense.llama_guard_api_url.is_empty() {
            urls.push((
                "defense.llama_guard_api_url",
                self.defense.llama_guard_api_url.clone(),
            ));
        }
        if !self.signal.api_url.is_empty() {
            urls.push(("signal.api_url", self.signal.api_url.clone()));
        }
        if !self.matrix.homeserver_url.is_empty() {
            urls.push(("matrix.homeserver_url", self.matrix.homeserver_url.clone()));
        }
        if let Some(ref ds) = self.deepseek {
            if !ds.api_url.is_empty() {
                urls.push(("deepseek.api_url", ds.api_url.clone()));
            }
        }
        if let Some(ref cp) = self.github_copilot {
            if !cp.api_url.is_empty() {
                urls.push(("github_copilot.api_url", cp.api_url.clone()));
            }
        }
        if let Some(ref mm) = self.minimax {
            if !mm.api_url.is_empty() {
                urls.push(("minimax.api_url", mm.api_url.clone()));
            }
        }
        urls
    }

    /// Returns the effective global default model based on which provider is
    /// actually configured.  `claude.model` is the canonical "global default"
    /// field, but if it points to a provider the user never set up (e.g. Claude
    /// when only OpenAI auth exists), fall back to the provider that *is*
    /// configured.
    pub fn effective_default_model(&self) -> &str {
        let model = &self.claude.model;
        let provider = crate::llm_provider::provider_for_model(model);

        if provider == crate::llm_provider::LlmProvider::Anthropic {
            // Check if Claude actually has auth
            let has_claude_key = self
                .claude
                .api_key
                .as_ref()
                .is_some_and(|k| !k.trim().is_empty());

            if !has_claude_key {
                // No API key — check OAuth profiles (cheap file read, cached by OS)
                let has_claude_oauth = crate::oauth::AuthProfileManager::load()
                    .ok()
                    .map(|m| !m.list_profiles("anthropic").is_empty())
                    .unwrap_or(false);

                if !has_claude_oauth {
                    // Claude has no auth at all.  Try OpenAI.
                    let has_openai_key = self
                        .openai
                        .api_key
                        .as_ref()
                        .is_some_and(|k| !k.trim().is_empty());
                    let has_openai_oauth = crate::oauth::AuthProfileManager::load()
                        .ok()
                        .map(|m| !m.list_profiles("openai").is_empty())
                        .unwrap_or(false);

                    if has_openai_key || has_openai_oauth {
                        return &self.openai.model;
                    }

                    // Try Cerebras
                    let has_cerebras_key = self
                        .cerebras
                        .api_key
                        .as_ref()
                        .is_some_and(|k| !k.trim().is_empty());
                    if has_cerebras_key {
                        return &self.cerebras.model;
                    }
                }
            }
        }

        model
    }

    /// Get configuration file path
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = directories::ProjectDirs::from("ai", "nexibot", "desktop")
            .ok_or_else(|| anyhow::anyhow!("Failed to get project directories"))?
            .config_dir()
            .to_path_buf();

        Ok(config_dir.join("config.yaml"))
    }
}

/// Deep-merge two YAML values. Overlay wins on conflicts.
/// - Mappings: recursively merge keys (overlay wins on leaf conflicts).
/// - Sequences: overlay replaces entirely.
/// - Scalars/Null: overlay replaces.
#[allow(dead_code)]
pub fn deep_merge_yaml(base: serde_yml::Value, overlay: serde_yml::Value) -> serde_yml::Value {
    use serde_yml::Value;

    /// Keys that must be rejected during merging to prevent prototype pollution.
    const POISONED_KEYS: &[&str] = &["__proto__", "prototype", "constructor"];

    fn is_poisoned_key(key: &Value) -> bool {
        if let Value::String(s) = key {
            POISONED_KEYS.contains(&s.as_str())
        } else {
            false
        }
    }

    match (base, overlay) {
        (Value::Mapping(mut base_map), Value::Mapping(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                // Block prototype-polluting keys from untrusted config overlays
                if is_poisoned_key(&key) {
                    tracing::warn!(
                        "[CONFIG] Blocked prototype-polluting key during config merge: {:?}",
                        key
                    );
                    continue;
                }
                let merged = if let Some(base_val) = base_map.remove(&key) {
                    deep_merge_yaml(base_val, overlay_val)
                } else {
                    overlay_val
                };
                base_map.insert(key, merged);
            }
            Value::Mapping(base_map)
        }
        // For sequences, scalars, and mixed types: overlay wins
        (_base, overlay) => overlay,
    }
}

/// Start a config file watcher that reloads config on changes.
/// Returns a broadcast sender that signals config changes.
pub fn start_config_watcher(config: Arc<RwLock<NexiBotConfig>>) -> Result<broadcast::Sender<()>> {
    let (tx, _) = broadcast::channel::<()>(16);
    let tx_clone = tx.clone();

    let config_path = NexiBotConfig::config_path()?;
    let watch_dir = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Config path has no parent directory"))?
        .to_path_buf();

    let config_filename = config_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Config path has no filename"))?
        .to_os_string();

    info!("[HOT_RELOAD] Watching config at: {:?}", config_path);

    std::thread::spawn(move || {
        let (notify_tx, notify_rx) = std::sync::mpsc::channel();

        let mut debouncer = match new_debouncer(Duration::from_millis(200), notify_tx) {
            Ok(d) => d,
            Err(e) => {
                warn!("[HOT_RELOAD] Failed to create file watcher: {}", e);
                return;
            }
        };

        if let Err(e) = debouncer
            .watcher()
            .watch(&watch_dir, RecursiveMode::NonRecursive)
        {
            warn!("[HOT_RELOAD] Failed to watch config directory: {}", e);
            return;
        }

        info!("[HOT_RELOAD] Config file watcher started");

        loop {
            match notify_rx.recv() {
                Ok(Ok(events)) => {
                    // Check if our config file was changed
                    let config_changed = events.iter().any(|e| {
                        e.path
                            .file_name()
                            .map(|f| f == config_filename)
                            .unwrap_or(false)
                    });

                    if config_changed {
                        info!("[HOT_RELOAD] Config file changed, reloading...");

                        match NexiBotConfig::load() {
                            Ok(mut new_config) => {
                                let config_clone = config.clone();
                                let tx_inner = tx_clone.clone();
                                // tauri::async_runtime::spawn works from any thread,
                                // unlike tokio::spawn which requires a runtime context.
                                tauri::async_runtime::spawn(async move {
                                    let current = config_clone.read().await;
                                    // Preserve in-memory credentials that would be
                                    // cleared by the file reload. This guards against
                                    // external tools (sed, etc.) that strip credential
                                    // values from config.yaml.
                                    NexiBotConfig::preserve_credentials_on_reload(
                                        &current,
                                        &mut new_config,
                                    );
                                    drop(current);

                                    let mut cfg = config_clone.write().await;
                                    *cfg = new_config;
                                    drop(cfg);
                                    let _ = tx_inner.send(());
                                    info!("[HOT_RELOAD] Config reloaded successfully");
                                });
                            }
                            Err(e) => {
                                warn!("[HOT_RELOAD] Failed to reload config: {}", e);
                            }
                        }
                    }
                }
                Ok(Err(errors)) => {
                    warn!("[HOT_RELOAD] Watch errors: {:?}", errors);
                }
                Err(e) => {
                    warn!("[HOT_RELOAD] Watch channel closed: {}", e);
                    break;
                }
            }
        }
    });

    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutex to serialize tests that modify process-wide environment variables.
    /// Without this, parallel tests clobber each other's env vars.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_config_round_trip() {
        let config = NexiBotConfig::default();
        let yaml = serde_yml::to_string(&config).expect("serialize");
        let parsed: NexiBotConfig = serde_yml::from_str(&yaml).expect("deserialize");
        assert_eq!(parsed.claude.model, config.claude.model);
        assert_eq!(parsed.config_version, config.config_version);
        assert_eq!(parsed.k2k.local_agent_url, config.k2k.local_agent_url);
    }

    #[test]
    fn test_config_validation_good() {
        let config = NexiBotConfig::default();
        let warnings = config.validate();
        assert!(
            warnings.is_empty(),
            "Default config should have no warnings: {:?}",
            warnings
        );
    }

    #[test]
    fn test_config_validation_bad_threshold() {
        let mut config = NexiBotConfig::default();
        config.wakeword.threshold = 1.5; // out of range
        let warnings = config.validate();
        assert!(!warnings.is_empty());
        assert!(warnings.iter().any(|w| w.contains("Wakeword threshold")));
    }

    #[test]
    fn test_config_validation_bad_url() {
        let mut config = NexiBotConfig::default();
        config.k2k.local_agent_url = "not a url".to_string();
        let warnings = config.validate();
        assert!(!warnings.is_empty());
        assert!(warnings.iter().any(|w| w.contains("k2k.local_agent_url")));
    }

    #[test]
    fn test_config_validation_path_traversal() {
        let mut config = NexiBotConfig::default();
        config.defense.deberta_model_path = Some("../../../etc/passwd".to_string());
        let warnings = config.validate();
        assert!(!warnings.is_empty());
        assert!(warnings.iter().any(|w| w.contains("DeBERTa model path")));
    }

    #[test]
    fn test_default_config_version() {
        let config = NexiBotConfig::default();
        assert_eq!(config.config_version, CURRENT_CONFIG_VERSION);
    }

    #[test]
    fn test_computer_use_config_default() {
        let config = ComputerUseConfig::default();
        assert!(!config.enabled);
        assert!(config.require_confirmation);
    }

    #[test]
    fn test_browser_config_default_values() {
        let config = BrowserConfig::default();
        assert!(!config.enabled);
        assert!(config.headless);
        assert!(config.require_confirmation);
        assert!(config.allowed_domains.is_empty());
        assert!(config.use_guardrails);
    }

    #[test]
    fn test_browser_config_round_trip() {
        let config = BrowserConfig {
            enabled: true,
            headless: false,
            default_timeout_ms: 5000,
            chrome_path: Some("/usr/bin/chromium".into()),
            viewport_width: 1920,
            viewport_height: 1080,
            require_confirmation: false,
            allowed_domains: vec!["example.com".into(), "test.org".into()],
            use_guardrails: false,
        };
        let yaml = serde_yml::to_string(&config).expect("serialize");
        let parsed: BrowserConfig = serde_yml::from_str(&yaml).expect("deserialize");
        assert_eq!(parsed.enabled, config.enabled);
        assert_eq!(parsed.require_confirmation, config.require_confirmation);
        assert_eq!(parsed.allowed_domains, config.allowed_domains);
        assert_eq!(parsed.use_guardrails, config.use_guardrails);
    }

    #[test]
    fn test_browser_config_serde_defaults_from_partial_yaml() {
        // Simulate an old config that doesn't have the new Phase 5b fields
        let yaml = r#"
enabled: true
headless: true
default_timeout_ms: 30000
chrome_path: null
viewport_width: 1280
viewport_height: 720
"#;
        let config: BrowserConfig = serde_yml::from_str(yaml).expect("deserialize");
        // Serde defaults should fill in the new fields
        assert!(config.require_confirmation);
        assert!(config.allowed_domains.is_empty());
        assert!(config.use_guardrails);
    }

    // ── Environment variable override tests ──────────────────────────

    /// Helper: remove a list of env vars (cleanup).
    fn remove_env_vars(vars: &[&str]) {
        for var in vars {
            std::env::remove_var(var);
        }
    }

    #[test]
    fn test_env_override_claude_api_key() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["CLAUDE_API_KEY"];
        remove_env_vars(&vars);
        std::env::set_var("CLAUDE_API_KEY", "sk-test-key-12345");

        let mut config = NexiBotConfig::default();
        assert!(config.claude.api_key.is_none());
        config.apply_env_overrides();
        assert_eq!(config.claude.api_key.as_deref(), Some("sk-test-key-12345"));

        remove_env_vars(&vars);
    }

    #[test]
    fn test_env_override_claude_model() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["CLAUDE_MODEL"];
        remove_env_vars(&vars);
        std::env::set_var("CLAUDE_MODEL", "claude-opus-4-20250514");

        let mut config = NexiBotConfig::default();
        config.apply_env_overrides();
        assert_eq!(config.claude.model, "claude-opus-4-20250514");

        remove_env_vars(&vars);
    }

    #[test]
    fn test_env_override_ollama_url() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["OLLAMA_URL"];
        remove_env_vars(&vars);
        std::env::set_var("OLLAMA_URL", "http://myhost:11434");

        let mut config = NexiBotConfig::default();
        assert!(!config.ollama.enabled);
        config.apply_env_overrides();
        assert_eq!(config.ollama.url, "http://myhost:11434");
        assert!(
            config.ollama.enabled,
            "Setting OLLAMA_URL should also enable Ollama"
        );

        remove_env_vars(&vars);
    }

    #[test]
    fn test_env_override_telegram_bot_token() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["TELEGRAM_BOT_TOKEN"];
        remove_env_vars(&vars);
        std::env::set_var("TELEGRAM_BOT_TOKEN", "123456:ABC-DEF");

        let mut config = NexiBotConfig::default();
        assert!(!config.telegram.enabled);
        config.apply_env_overrides();
        assert_eq!(config.telegram.bot_token, "123456:ABC-DEF");
        assert!(
            config.telegram.enabled,
            "Setting TELEGRAM_BOT_TOKEN should also enable Telegram"
        );

        remove_env_vars(&vars);
    }

    #[test]
    fn test_env_override_openai_api_key() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["OPENAI_API_KEY"];
        remove_env_vars(&vars);
        std::env::set_var("OPENAI_API_KEY", "sk-openai-test-key");

        let mut config = NexiBotConfig::default();
        assert!(config.openai.api_key.is_none());
        config.apply_env_overrides();
        assert_eq!(config.openai.api_key.as_deref(), Some("sk-openai-test-key"));

        remove_env_vars(&vars);
    }

    #[test]
    fn test_env_override_empty_ignored() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["CLAUDE_API_KEY", "CLAUDE_MODEL"];
        remove_env_vars(&vars);
        std::env::set_var("CLAUDE_API_KEY", "");
        std::env::set_var("CLAUDE_MODEL", "");

        let mut config = NexiBotConfig::default();
        let original_model = config.claude.model.clone();
        config.apply_env_overrides();

        // Empty env vars should not change the config
        assert!(
            config.claude.api_key.is_none(),
            "Empty CLAUDE_API_KEY should be ignored"
        );
        assert_eq!(
            config.claude.model, original_model,
            "Empty CLAUDE_MODEL should be ignored"
        );

        remove_env_vars(&vars);
    }

    #[test]
    fn test_env_override_unset_ignored() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = [
            "CLAUDE_API_KEY",
            "CLAUDE_MODEL",
            "OPENAI_API_KEY",
            "OLLAMA_URL",
            "TELEGRAM_BOT_TOKEN",
            "WHATSAPP_PHONE_NUMBER_ID",
            "WHATSAPP_ACCESS_TOKEN",
            "WHATSAPP_VERIFY_TOKEN",
        ];
        remove_env_vars(&vars);

        let mut config = NexiBotConfig::default();
        let original_model = config.claude.model.clone();
        config.apply_env_overrides();

        assert!(config.claude.api_key.is_none());
        assert_eq!(config.claude.model, original_model);
        assert!(config.openai.api_key.is_none());
        assert!(!config.ollama.enabled);
        assert!(!config.telegram.enabled);
        assert!(!config.whatsapp.enabled);
    }

    #[test]
    fn test_env_override_whatsapp_vars() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = [
            "WHATSAPP_PHONE_NUMBER_ID",
            "WHATSAPP_ACCESS_TOKEN",
            "WHATSAPP_VERIFY_TOKEN",
            "WHATSAPP_APP_SECRET",
        ];
        remove_env_vars(&vars);
        std::env::set_var("WHATSAPP_PHONE_NUMBER_ID", "12345678");
        std::env::set_var("WHATSAPP_ACCESS_TOKEN", "EAAx...");
        std::env::set_var("WHATSAPP_VERIFY_TOKEN", "my-verify-token");
        std::env::set_var("WHATSAPP_APP_SECRET", "my-app-secret");

        let mut config = NexiBotConfig::default();
        assert!(!config.whatsapp.enabled);
        config.apply_env_overrides();

        assert_eq!(config.whatsapp.phone_number_id, "12345678");
        assert_eq!(config.whatsapp.access_token, "EAAx...");
        assert_eq!(config.whatsapp.verify_token, "my-verify-token");
        assert_eq!(config.whatsapp.app_secret, "my-app-secret");
        assert!(
            config.whatsapp.enabled,
            "Setting WHATSAPP_VERIFY_TOKEN/WHATSAPP_APP_SECRET should enable WhatsApp"
        );

        remove_env_vars(&vars);
    }

    #[test]
    fn test_multiple_env_overrides() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = [
            "CLAUDE_API_KEY",
            "CLAUDE_MODEL",
            "OPENAI_API_KEY",
            "OLLAMA_URL",
            "TELEGRAM_BOT_TOKEN",
        ];
        remove_env_vars(&vars);
        std::env::set_var("CLAUDE_API_KEY", "sk-claude-key");
        std::env::set_var("CLAUDE_MODEL", "claude-haiku-3-5-20241022");
        std::env::set_var("OPENAI_API_KEY", "sk-openai-key");
        std::env::set_var("OLLAMA_URL", "http://gpu-server:11434");
        std::env::set_var("TELEGRAM_BOT_TOKEN", "bot-token-123");

        let mut config = NexiBotConfig::default();
        config.apply_env_overrides();

        assert_eq!(config.claude.api_key.as_deref(), Some("sk-claude-key"));
        assert_eq!(config.claude.model, "claude-haiku-3-5-20241022");
        assert_eq!(config.openai.api_key.as_deref(), Some("sk-openai-key"));
        assert_eq!(config.ollama.url, "http://gpu-server:11434");
        assert!(config.ollama.enabled);
        assert_eq!(config.telegram.bot_token, "bot-token-123");
        assert!(config.telegram.enabled);

        remove_env_vars(&vars);
    }

    // ── deep_merge_yaml tests ────────────────────────────────────────

    #[test]
    fn test_deep_merge_yaml_scalars() {
        let base: serde_yml::Value = serde_yml::from_str("key: base_value").unwrap();
        let overlay: serde_yml::Value = serde_yml::from_str("key: overlay_value").unwrap();
        let merged = deep_merge_yaml(base, overlay);
        let m = merged.as_mapping().unwrap();
        let key = serde_yml::Value::String("key".into());
        assert_eq!(m.get(&key).unwrap().as_str().unwrap(), "overlay_value");
    }

    #[test]
    fn test_deep_merge_yaml_nested_maps() {
        let base: serde_yml::Value = serde_yml::from_str(
            r#"
claude:
  model: claude-sonnet-4-5-20250929
  max_tokens: 4096
  api_key: base-key
"#,
        )
        .unwrap();

        let overlay: serde_yml::Value = serde_yml::from_str(
            r#"
claude:
  model: claude-opus-4-20250514
"#,
        )
        .unwrap();

        let merged = deep_merge_yaml(base, overlay);
        let m = merged.as_mapping().unwrap();
        let claude_key = serde_yml::Value::String("claude".into());
        let claude_map = m.get(&claude_key).unwrap().as_mapping().unwrap();

        let model_key = serde_yml::Value::String("model".into());
        let max_tokens_key = serde_yml::Value::String("max_tokens".into());
        let api_key_key = serde_yml::Value::String("api_key".into());

        // Overlay wins for model
        assert_eq!(
            claude_map.get(&model_key).unwrap().as_str().unwrap(),
            "claude-opus-4-20250514"
        );
        // Base preserved for max_tokens
        assert_eq!(
            claude_map.get(&max_tokens_key).unwrap().as_u64().unwrap(),
            4096
        );
        // Base preserved for api_key
        assert_eq!(
            claude_map.get(&api_key_key).unwrap().as_str().unwrap(),
            "base-key"
        );
    }

    #[test]
    fn test_deep_merge_yaml_sequences_replaced() {
        let base: serde_yml::Value = serde_yml::from_str(
            r#"
items:
  - one
  - two
"#,
        )
        .unwrap();

        let overlay: serde_yml::Value = serde_yml::from_str(
            r#"
items:
  - three
"#,
        )
        .unwrap();

        let merged = deep_merge_yaml(base, overlay);
        let m = merged.as_mapping().unwrap();
        let items_key = serde_yml::Value::String("items".into());
        let items = m.get(&items_key).unwrap().as_sequence().unwrap();
        // Overlay completely replaces the sequence
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].as_str().unwrap(), "three");
    }

    #[test]
    fn test_deep_merge_yaml_overlay_adds_new_keys() {
        let base: serde_yml::Value = serde_yml::from_str("a: 1").unwrap();
        let overlay: serde_yml::Value = serde_yml::from_str("b: 2").unwrap();
        let merged = deep_merge_yaml(base, overlay);
        let m = merged.as_mapping().unwrap();
        let a_key = serde_yml::Value::String("a".into());
        let b_key = serde_yml::Value::String("b".into());
        assert_eq!(m.get(&a_key).unwrap().as_u64().unwrap(), 1);
        assert_eq!(m.get(&b_key).unwrap().as_u64().unwrap(), 2);
    }

    #[test]
    fn test_deep_merge_yaml_overlay_null_replaces() {
        let base: serde_yml::Value = serde_yml::from_str("key: value").unwrap();
        let overlay: serde_yml::Value = serde_yml::from_str("key: null").unwrap();
        let merged = deep_merge_yaml(base, overlay);
        let m = merged.as_mapping().unwrap();
        let key = serde_yml::Value::String("key".into());
        assert!(m.get(&key).unwrap().is_null());
    }

    // ── NEXIBOT_* env var override tests ─────────────────────────────

    #[test]
    fn test_nexibot_env_override_claude_model() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_CLAUDE_MODEL"];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_CLAUDE_MODEL", "claude-opus-4-20250514");

        let mut config = NexiBotConfig::default();
        config.apply_nexibot_env_overrides();
        assert_eq!(config.claude.model, "claude-opus-4-20250514");

        remove_env_vars(&vars);
    }

    #[test]
    fn test_nexibot_env_override_defense_enabled() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_DEFENSE_ENABLED"];
        remove_env_vars(&vars);

        // Enable defense via env
        std::env::set_var("NEXIBOT_DEFENSE_ENABLED", "true");
        let mut config = NexiBotConfig::default();
        config.apply_nexibot_env_overrides();
        assert!(config.defense.enabled);

        // Disable via "0"
        std::env::set_var("NEXIBOT_DEFENSE_ENABLED", "0");
        config.apply_nexibot_env_overrides();
        assert!(!config.defense.enabled);

        remove_env_vars(&vars);
    }

    #[test]
    fn test_nexibot_env_override_claude_max_tokens() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_CLAUDE_MAX_TOKENS"];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_CLAUDE_MAX_TOKENS", "8192");

        let mut config = NexiBotConfig::default();
        config.apply_nexibot_env_overrides();
        assert_eq!(config.claude.max_tokens, 8192);

        remove_env_vars(&vars);
    }

    #[test]
    fn test_nexibot_env_override_webhooks_port() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_WEBHOOKS_PORT"];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_WEBHOOKS_PORT", "9999");

        let mut config = NexiBotConfig::default();
        config.apply_nexibot_env_overrides();
        assert_eq!(config.webhooks.port, 9999);

        remove_env_vars(&vars);
    }

    #[test]
    fn test_nexibot_env_override_ollama_fields() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = [
            "NEXIBOT_OLLAMA_ENABLED",
            "NEXIBOT_OLLAMA_URL",
            "NEXIBOT_OLLAMA_MODEL",
        ];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_OLLAMA_ENABLED", "1");
        std::env::set_var("NEXIBOT_OLLAMA_URL", "http://remote:11434");
        std::env::set_var("NEXIBOT_OLLAMA_MODEL", "mistral");

        let mut config = NexiBotConfig::default();
        config.apply_nexibot_env_overrides();
        assert!(config.ollama.enabled);
        assert_eq!(config.ollama.url, "http://remote:11434");
        assert_eq!(config.ollama.model, "mistral");

        remove_env_vars(&vars);
    }

    #[test]
    fn test_nexibot_env_override_invalid_bool_ignored() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_DEFENSE_ENABLED"];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_DEFENSE_ENABLED", "maybe");

        let mut config = NexiBotConfig::default();
        let original = config.defense.enabled;
        config.apply_nexibot_env_overrides();
        // Invalid bool string should not change the value
        assert_eq!(config.defense.enabled, original);

        remove_env_vars(&vars);
    }

    #[test]
    fn test_nexibot_env_override_empty_string_ignored() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_CLAUDE_MODEL"];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_CLAUDE_MODEL", "");

        let mut config = NexiBotConfig::default();
        let original = config.claude.model.clone();
        config.apply_nexibot_env_overrides();
        assert_eq!(config.claude.model, original);

        remove_env_vars(&vars);
    }

    #[test]
    fn test_nexibot_env_override_multiple_booleans() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = [
            "NEXIBOT_BROWSER_ENABLED",
            "NEXIBOT_BROWSER_HEADLESS",
            "NEXIBOT_EXECUTE_ENABLED",
        ];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_BROWSER_ENABLED", "true");
        std::env::set_var("NEXIBOT_BROWSER_HEADLESS", "false");
        std::env::set_var("NEXIBOT_EXECUTE_ENABLED", "1");

        let mut config = NexiBotConfig::default();
        config.apply_nexibot_env_overrides();
        assert!(config.browser.enabled);
        assert!(!config.browser.headless);
        assert!(config.execute.enabled);

        remove_env_vars(&vars);
    }

    // ── ChannelToolPolicy tests ────────────────────────────────────

    #[test]
    fn test_channel_tool_policy_default_denied_tools() {
        let policy = ChannelToolPolicy::default();
        // Only filesystem is denied by default; execute is allowed for headless channels
        assert_eq!(policy.denied_tools, vec!["nexibot_filesystem"]);
        assert!(policy.allowed_tools.is_empty());
        assert!(policy.admin_bypass);
    }

    #[test]
    fn test_channel_tool_policy_is_tool_denied() {
        let policy = ChannelToolPolicy::default();
        // execute is NOT denied by default (headless channels need it)
        assert!(!policy.is_tool_denied("nexibot_execute"));
        assert!(policy.is_tool_denied("nexibot_filesystem"));
        assert!(!policy.is_tool_denied("nexibot_memory"));
        assert!(!policy.is_tool_denied("nexibot_search"));
    }

    #[test]
    fn test_channel_tool_policy_allowed_overrides_denied() {
        let policy = ChannelToolPolicy {
            denied_tools: vec!["nexibot_execute".into(), "nexibot_filesystem".into()],
            allowed_tools: vec!["nexibot_execute".into()],
            admin_bypass: true,
        };
        // nexibot_execute in both lists -> allowed_tools wins
        assert!(!policy.is_tool_denied("nexibot_execute"));
        // nexibot_filesystem only in denied -> still denied
        assert!(policy.is_tool_denied("nexibot_filesystem"));
    }

    #[test]
    fn test_channel_tool_policy_allow_all() {
        let policy = ChannelToolPolicy::allow_all();
        assert!(!policy.is_tool_denied("nexibot_execute"));
        assert!(!policy.is_tool_denied("nexibot_filesystem"));
        assert!(!policy.is_tool_denied("anything"));
    }

    #[test]
    fn test_channel_tool_policy_serde_defaults() {
        // Empty YAML should produce correct defaults
        let yaml = "{}";
        let policy: ChannelToolPolicy = serde_yml::from_str(yaml).expect("deserialize");
        // Only filesystem denied by default
        assert_eq!(policy.denied_tools, vec!["nexibot_filesystem"]);
        assert!(policy.allowed_tools.is_empty());
        assert!(policy.admin_bypass);
    }

    #[test]
    fn test_channel_tool_policy_custom_serde() {
        let yaml = r#"
denied_tools: []
allowed_tools:
  - nexibot_execute
admin_bypass: false
"#;
        let policy: ChannelToolPolicy = serde_yml::from_str(yaml).expect("deserialize");
        assert!(policy.denied_tools.is_empty());
        assert_eq!(policy.allowed_tools, vec!["nexibot_execute"]);
        assert!(!policy.admin_bypass);
    }

    // ── Profile path traversal prevention tests ──────────────────────

    #[test]
    fn test_profile_name_rejects_path_traversal() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_PROFILE"];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_PROFILE", "../../../etc/passwd");

        let result = NexiBotConfig::load_with_profile();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("must not contain path separators"),
            "Error: {}",
            err_msg
        );

        remove_env_vars(&vars);
    }

    #[test]
    fn test_profile_name_rejects_forward_slash() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_PROFILE"];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_PROFILE", "foo/bar");

        let result = NexiBotConfig::load_with_profile();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("must not contain path separators"),
            "Error: {}",
            err_msg
        );

        remove_env_vars(&vars);
    }

    #[test]
    fn test_profile_name_rejects_backslash() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_PROFILE"];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_PROFILE", "foo\\bar");

        let result = NexiBotConfig::load_with_profile();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("must not contain path separators"),
            "Error: {}",
            err_msg
        );

        remove_env_vars(&vars);
    }

    #[test]
    fn test_profile_name_rejects_special_chars() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_PROFILE"];
        remove_env_vars(&vars);
        std::env::set_var("NEXIBOT_PROFILE", "profile;rm -rf");

        let result = NexiBotConfig::load_with_profile();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("only alphanumeric"), "Error: {}", err_msg);

        remove_env_vars(&vars);
    }

    #[test]
    fn test_profile_name_allows_valid_names() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let vars = ["NEXIBOT_PROFILE"];
        remove_env_vars(&vars);
        // A valid name that won't have a matching file, but should not error on validation
        std::env::set_var("NEXIBOT_PROFILE", "production-v2.1");

        // This should succeed (will just warn that the profile file doesn't exist)
        let result = NexiBotConfig::load_with_profile();
        // If it fails, it should NOT be due to profile name validation
        if let Err(e) = &result {
            let msg = e.to_string();
            assert!(
                !msg.contains("path separators"),
                "Unexpected error: {}",
                msg
            );
            assert!(
                !msg.contains("only alphanumeric"),
                "Unexpected error: {}",
                msg
            );
        }

        remove_env_vars(&vars);
    }

    // ── Config schema validation tests (P3C) ──────────────────────

    #[test]
    fn test_validate_max_tokens_out_of_range() {
        let mut config = NexiBotConfig::default();
        config.claude.max_tokens = 0;
        let warnings = config.validate();
        assert!(warnings.iter().any(|w| w.contains("claude.max_tokens")));

        config.claude.max_tokens = 1_000_001;
        let warnings = config.validate();
        assert!(warnings.iter().any(|w| w.contains("claude.max_tokens")));
    }

    #[test]
    fn test_validate_gateway_port_below_minimum() {
        let mut config = NexiBotConfig::default();
        config.gateway.port = 80;
        let warnings = config.validate();
        assert!(warnings.iter().any(|w| w.contains("gateway.port")));
    }

    #[test]
    fn test_validate_gateway_max_connections_out_of_range() {
        let mut config = NexiBotConfig::default();
        config.gateway.max_connections = 0;
        let warnings = config.validate();
        assert!(warnings
            .iter()
            .any(|w| w.contains("gateway.max_connections")));

        config.gateway.max_connections = 10_001;
        let warnings = config.validate();
        assert!(warnings
            .iter()
            .any(|w| w.contains("gateway.max_connections")));
    }

    #[test]
    fn test_validate_gateway_bind_address_invalid() {
        let mut config = NexiBotConfig::default();
        config.gateway.bind_address = "not-an-ip".to_string();
        let warnings = config.validate();
        assert!(warnings.iter().any(|w| w.contains("gateway.bind_address")));
    }

    #[test]
    fn test_validate_gateway_bind_address_valid() {
        let mut config = NexiBotConfig::default();
        config.gateway.bind_address = "0.0.0.0".to_string();
        let warnings = config.validate();
        assert!(!warnings.iter().any(|w| w.contains("gateway.bind_address")));
    }

    #[test]
    fn test_validate_deberta_threshold_out_of_range() {
        let mut config = NexiBotConfig::default();
        config.defense.deberta_threshold = -0.1;
        let warnings = config.validate();
        assert!(warnings
            .iter()
            .any(|w| w.contains("defense.deberta_threshold")));

        config.defense.deberta_threshold = 1.1;
        let warnings = config.validate();
        assert!(warnings
            .iter()
            .any(|w| w.contains("defense.deberta_threshold")));
    }

    #[test]
    fn test_validate_sandbox_timeout_out_of_range() {
        let mut config = NexiBotConfig::default();
        config.sandbox.timeout_seconds = 0;
        let warnings = config.validate();
        assert!(warnings
            .iter()
            .any(|w| w.contains("sandbox.timeout_seconds")));

        config.sandbox.timeout_seconds = 3601;
        let warnings = config.validate();
        assert!(warnings
            .iter()
            .any(|w| w.contains("sandbox.timeout_seconds")));
    }

    #[test]
    fn test_validate_sandbox_cpu_limit_out_of_range() {
        let mut config = NexiBotConfig::default();
        config.sandbox.cpu_limit = 0.0;
        let warnings = config.validate();
        assert!(warnings.iter().any(|w| w.contains("sandbox.cpu_limit")));

        config.sandbox.cpu_limit = 17.0;
        let warnings = config.validate();
        assert!(warnings.iter().any(|w| w.contains("sandbox.cpu_limit")));
    }

    #[test]
    fn test_validate_sandbox_memory_limit_invalid() {
        let mut config = NexiBotConfig::default();
        config.sandbox.memory_limit = "not-memory".to_string();
        let warnings = config.validate();
        assert!(warnings.iter().any(|w| w.contains("sandbox.memory_limit")));
    }

    #[test]
    fn test_validate_sandbox_memory_limit_valid() {
        assert!(is_valid_memory_string("256m"));
        assert!(is_valid_memory_string("1g"));
        assert!(is_valid_memory_string("512M"));
        assert!(is_valid_memory_string("2G"));
        assert!(is_valid_memory_string("1024mb"));
        assert!(is_valid_memory_string("1.5g"));
        assert!(!is_valid_memory_string(""));
        assert!(!is_valid_memory_string("abc"));
        assert!(!is_valid_memory_string("0m"));
        assert!(!is_valid_memory_string("-1g"));
    }

    #[test]
    fn test_validate_url_fields_invalid_scheme() {
        let mut config = NexiBotConfig::default();
        config.k2k.local_agent_url = "ftp://evil.com/hack".to_string();
        let warnings = config.validate();
        assert!(warnings.iter().any(|w| w.contains("disallowed scheme")));
    }

    #[test]
    fn test_validate_agent_path_traversal() {
        let mut config = NexiBotConfig::default();
        config.agents.push(AgentConfig {
            id: "../../../etc/passwd".to_string(),
            name: "Evil Agent".to_string(),
            avatar: None,
            model: None,
            primary_model: None,
            backup_model: None,
            provider: None,
            soul_path: None,
            system_prompt: None,
            is_default: false,
            channel_bindings: Vec::new(),
            capabilities: Vec::new(),
            workspace: Default::default(),
        });
        let warnings = config.validate();
        assert!(warnings.iter().any(|w| w.contains("path traversal")));
    }

    #[test]
    fn test_validate_and_clamp_max_tokens() {
        let mut config = NexiBotConfig::default();
        config.claude.max_tokens = 0;
        config.validate_and_clamp();
        assert_eq!(config.claude.max_tokens, 1);

        config.claude.max_tokens = 2_000_000;
        config.validate_and_clamp();
        assert_eq!(config.claude.max_tokens, 1_000_000);
    }

    #[test]
    fn test_validate_and_clamp_gateway_port() {
        let mut config = NexiBotConfig::default();
        config.gateway.port = 80;
        config.validate_and_clamp();
        assert_eq!(config.gateway.port, 1024);
    }

    #[test]
    fn test_validate_and_clamp_gateway_max_connections() {
        let mut config = NexiBotConfig::default();
        config.gateway.max_connections = 0;
        config.validate_and_clamp();
        assert_eq!(config.gateway.max_connections, 1);

        config.gateway.max_connections = 20_000;
        config.validate_and_clamp();
        assert_eq!(config.gateway.max_connections, 10_000);
    }

    #[test]
    fn test_validate_and_clamp_bind_address() {
        let mut config = NexiBotConfig::default();
        config.gateway.bind_address = "not-an-ip".to_string();
        config.validate_and_clamp();
        assert_eq!(config.gateway.bind_address, "127.0.0.1");
    }

    #[test]
    fn test_validate_and_clamp_deberta_threshold() {
        let mut config = NexiBotConfig::default();
        config.defense.deberta_threshold = -0.5;
        config.validate_and_clamp();
        assert_eq!(config.defense.deberta_threshold, 0.0);

        config.defense.deberta_threshold = 2.0;
        config.validate_and_clamp();
        assert_eq!(config.defense.deberta_threshold, 1.0);
    }

    #[test]
    fn test_validate_and_clamp_sandbox_timeout() {
        let mut config = NexiBotConfig::default();
        config.sandbox.timeout_seconds = 0;
        config.validate_and_clamp();
        assert_eq!(config.sandbox.timeout_seconds, 1);

        config.sandbox.timeout_seconds = 5000;
        config.validate_and_clamp();
        assert_eq!(config.sandbox.timeout_seconds, 3600);
    }

    #[test]
    fn test_validate_and_clamp_sandbox_cpu_limit() {
        let mut config = NexiBotConfig::default();
        config.sandbox.cpu_limit = 0.0;
        config.validate_and_clamp();
        assert!((config.sandbox.cpu_limit - 0.1).abs() < f64::EPSILON);

        config.sandbox.cpu_limit = 32.0;
        config.validate_and_clamp();
        assert!((config.sandbox.cpu_limit - 16.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_validate_and_clamp_sandbox_memory_limit() {
        let mut config = NexiBotConfig::default();
        config.sandbox.memory_limit = "garbage".to_string();
        config.validate_and_clamp();
        assert_eq!(config.sandbox.memory_limit, "512m");
    }

    #[test]
    fn test_validate_for_save_rejects_invalid() {
        let mut config = NexiBotConfig::default();
        config.claude.max_tokens = 0;
        let result = config.validate_for_save();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("claude.max_tokens"));
    }

    #[test]
    fn test_validate_for_save_accepts_valid_default() {
        let config = NexiBotConfig::default();
        let result = config.validate_for_save();
        assert!(
            result.is_ok(),
            "Default config should pass save validation: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_validate_for_save_rejects_bad_bind_address() {
        let mut config = NexiBotConfig::default();
        config.gateway.bind_address = "evil-host".to_string();
        let result = config.validate_for_save();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("gateway.bind_address"));
    }

    #[test]
    fn test_validate_for_save_rejects_bad_url_scheme() {
        let mut config = NexiBotConfig::default();
        config.k2k.local_agent_url = "file:///etc/passwd".to_string();
        let result = config.validate_for_save();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("disallowed scheme"));
    }

    #[test]
    fn test_contains_path_traversal_chars() {
        assert!(contains_path_traversal_chars("../../../etc/passwd"));
        assert!(contains_path_traversal_chars("foo..bar"));
        assert!(contains_path_traversal_chars("hello\0world"));
        assert!(contains_path_traversal_chars("test..\\hack"));
        assert!(!contains_path_traversal_chars("normal-name"));
        assert!(!contains_path_traversal_chars("my_agent_v2"));
    }

    #[test]
    fn test_default_config_passes_all_validation() {
        let config = NexiBotConfig::default();
        let warnings = config.validate();
        assert!(
            warnings.is_empty(),
            "Default config should produce no validation warnings: {:?}",
            warnings
        );
        let result = config.validate_for_save();
        assert!(
            result.is_ok(),
            "Default config should pass save validation: {:?}",
            result.err()
        );
    }
}
