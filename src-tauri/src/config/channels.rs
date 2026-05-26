//! Channel integration configurations: Telegram, WhatsApp, Discord, Slack, Signal, Teams, Matrix, NATS.

use super::ChannelToolPolicy;
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}
fn default_signal_api_url() -> String {
    "http://localhost:8080".to_string()
}
fn default_matrix_command_prefix() -> Option<String> {
    Some("!nexi".to_string())
}

/// Scope for ack/done reactions.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReactionScope {
    #[default]
    All,
    Direct,
    GroupMentions,
    Off,
}

/// Error policy for Telegram responses.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPolicy {
    #[default]
    Reply,
    Silent,
}

/// DM thread reply mode.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DmThreadReplyMode {
    #[default]
    Off,
    Inbound,
    Always,
}

/// Bot sender filtering policy.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SenderTypePolicy {
    #[default]
    HumansOnly,
    HumansAndAllowlistedBots,
    Open,
}

/// Group access policy.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    #[default]
    Open,
    Restricted,
}

/// A custom slash command registered with BotFather via setMyCommands.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomCommand {
    pub command: String,
    pub description: String,
}

/// Configuration for a single Telegram bot instance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramBotConfig {
    pub bot_token: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
    #[serde(default)]
    pub admin_chat_ids: Vec<i64>,
}

// -- P1: Group-level config types --

/// Per-topic override within a Telegram group.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramTopicConfig {
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub require_mention: Option<bool>,
    #[serde(default)]
    pub allow_from: Option<Vec<i64>>,
    #[serde(default)]
    pub group_policy: Option<GroupPolicy>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
}

/// Per-group configuration with topic overrides.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramGroupConfig {
    #[serde(default)]
    pub require_mention: bool,
    #[serde(default)]
    pub allow_from: Vec<i64>,
    #[serde(default)]
    pub group_policy: GroupPolicy,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub topics: std::collections::HashMap<String, TelegramTopicConfig>,
}

/// Reply threading mode for Telegram responses.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplyToMode {
    #[default]
    Off,
    First,
    All,
}

/// Message formatting configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramFormattingConfig {
    #[serde(default = "default_parse_mode_html")]
    pub parse_mode: String,
    #[serde(default = "default_chunk_mode")]
    pub chunk_mode: String,
    #[serde(default = "default_chunk_limit")]
    pub chunk_limit: usize,
}

fn default_parse_mode_html() -> String {
    "html".to_string()
}
fn default_chunk_mode() -> String {
    "newline".to_string()
}
fn default_chunk_limit() -> usize {
    4000
}

impl Default for TelegramFormattingConfig {
    fn default() -> Self {
        Self {
            parse_mode: default_parse_mode_html(),
            chunk_mode: default_chunk_mode(),
            chunk_limit: default_chunk_limit(),
        }
    }
}

/// Streaming progress draft configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramStreamingConfig {
    #[serde(default = "default_streaming_mode")]
    pub mode: String,
    #[serde(default = "default_max_progress_lines")]
    pub max_progress_lines: usize,
    #[serde(default = "default_true")]
    pub show_tool_names: bool,
}

fn default_streaming_mode() -> String {
    "off".to_string()
}
fn default_max_progress_lines() -> usize {
    4
}

impl Default for TelegramStreamingConfig {
    fn default() -> Self {
        Self {
            mode: default_streaming_mode(),
            max_progress_lines: default_max_progress_lines(),
            show_tool_names: true,
        }
    }
}

/// Webhook configuration for Telegram bot.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramWebhookConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub path: String,
    #[serde(default = "default_webhook_host")]
    pub host: String,
    #[serde(default = "default_webhook_port")]
    pub port: u16,
}

fn default_webhook_host() -> String {
    "127.0.0.1".to_string()
}
fn default_webhook_port() -> u16 {
    8443
}

/// Telegram Bot configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Whether the Telegram bot is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Telegram Bot API token (from @BotFather)
    #[serde(default)]
    pub bot_token: String,
    /// Allowed chat IDs (empty = allow all)
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
    /// Admin chat IDs — bypass DM policy (always allowed)
    #[serde(default)]
    pub admin_chat_ids: Vec<i64>,
    /// Whether to process voice messages (requires STT backend)
    #[serde(default)]
    pub voice_enabled: bool,
    /// Reply to voice messages with a voice message (TTS) instead of text only.
    #[serde(default = "default_true")]
    pub voice_response: bool,
    /// DM authorization policy
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy
    #[serde(default)]
    pub tool_policy: ChannelToolPolicy,
    /// Additional bot instances (each with their own token and optional agent binding)
    #[serde(default)]
    pub bots: Vec<TelegramBotConfig>,
    /// Enable forum topic/thread routing (separate session per thread)
    #[serde(default)]
    pub thread_routing_enabled: bool,
    /// Legacy flag: react to messages with emojis. Prefer ack_emoji/done_emoji + reaction_scope.
    #[serde(default = "default_true")]
    pub reactions_enabled: bool,

    // -- v0.11.0 P2 features --

    /// Ack reaction emoji while processing. None = disabled.
    #[serde(default)]
    pub ack_emoji: Option<String>,
    /// Done reaction emoji when completed. None = disabled.
    #[serde(default)]
    pub done_emoji: Option<String>,
    /// Scope for ack/done reactions.
    #[serde(default)]
    pub reaction_scope: ReactionScope,

    /// Error policy: reply (send text) or silent (log only).
    #[serde(default)]
    pub error_policy: ErrorPolicy,
    /// Suppress repeated error messages in the same chat for N milliseconds.
    #[serde(default)]
    pub error_cooldown_ms: u64,

    /// DM thread reply mode: off, inbound (only when msg is a reply), or always.
    #[serde(default)]
    pub dm_thread_replies: DmThreadReplyMode,

    /// Detect polling stall when no update received for N ms (0 = disabled).
    #[serde(default)]
    pub polling_stall_threshold_ms: u64,

    /// Bot sender filtering (default humans-only).
    #[serde(default)]
    pub sender_type_policy: SenderTypePolicy,

    /// Custom commands registered with BotFather at startup.
    #[serde(default)]
    pub custom_commands: Vec<CustomCommand>,

    /// Prefix prepended to every outbound message text.
    #[serde(default)]
    pub response_prefix: String,

    /// Whether Telegram shows link previews in bot messages.
    #[serde(default = "default_true")]
    pub link_preview: bool,

    // -- P1 features --

    /// Per-group configuration overrides.
    #[serde(default)]
    pub groups: std::collections::HashMap<i64, TelegramGroupConfig>,

    /// Reply threading mode: off, first, or all.
    #[serde(default)]
    pub reply_to_mode: ReplyToMode,

    /// Message formatting (HTML vs plain text, chunking).
    #[serde(default)]
    pub formatting: TelegramFormattingConfig,

    /// Streaming progress draft configuration.
    #[serde(default)]
    pub streaming: TelegramStreamingConfig,

    /// Webhook mode configuration.
    #[serde(default)]
    pub webhook: TelegramWebhookConfig,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token: String::new(),
            allowed_chat_ids: vec![],
            admin_chat_ids: vec![],
            voice_enabled: false,
            voice_response: true,
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: ChannelToolPolicy::default(),
            bots: vec![],
            thread_routing_enabled: false,
            reactions_enabled: true,
            ack_emoji: None,
            done_emoji: None,
            reaction_scope: ReactionScope::All,
            error_policy: ErrorPolicy::Reply,
            error_cooldown_ms: 0,
            dm_thread_replies: DmThreadReplyMode::Off,
            polling_stall_threshold_ms: 0,
            sender_type_policy: SenderTypePolicy::HumansOnly,
            custom_commands: vec![],
            response_prefix: String::new(),
            link_preview: true,
            groups: std::collections::HashMap::new(),
            reply_to_mode: ReplyToMode::Off,
            formatting: TelegramFormattingConfig::default(),
            streaming: TelegramStreamingConfig::default(),
            webhook: TelegramWebhookConfig::default(),
        }
    }
}