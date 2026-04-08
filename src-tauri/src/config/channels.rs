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

/// Telegram Bot configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    /// When enabled, NexiBot synthesizes its response and sends an OGG Opus
    /// voice note back via Telegram.  Requires a working TTS backend.
    #[serde(default = "default_true")]
    pub voice_response: bool,
    /// DM authorization policy
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy
    #[serde(default)]
    pub tool_policy: ChannelToolPolicy,
}

/// WhatsApp Cloud API configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhatsAppConfig {
    /// Whether WhatsApp integration is enabled
    #[serde(default)]
    pub enabled: bool,
    /// WhatsApp Business Phone Number ID
    #[serde(default)]
    pub phone_number_id: String,
    /// WhatsApp Cloud API permanent access token
    #[serde(default)]
    pub access_token: String,
    /// Webhook verify token (for Meta webhook verification handshake)
    #[serde(default)]
    pub verify_token: String,
    /// Meta app secret used to verify webhook signatures.
    #[serde(default)]
    pub app_secret: String,
    /// Allowed phone numbers (empty = allow all)
    #[serde(default)]
    pub allowed_phone_numbers: Vec<String>,
    /// Admin phone numbers — bypass DM policy (always allowed)
    #[serde(default)]
    pub admin_phone_numbers: Vec<String>,
    /// DM authorization policy
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy
    #[serde(default)]
    pub tool_policy: ChannelToolPolicy,
}

/// Discord Bot configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    /// Whether the Discord bot is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Discord Bot token
    #[serde(default)]
    pub bot_token: String,
    /// Allowed guild (server) IDs (empty = allow all)
    #[serde(default)]
    pub allowed_guild_ids: Vec<u64>,
    /// Allowed channel IDs (empty = allow all)
    #[serde(default)]
    pub allowed_channel_ids: Vec<u64>,
    /// Admin user IDs — bypass DM policy (always allowed)
    #[serde(default)]
    pub admin_user_ids: Vec<u64>,
    /// DM authorization policy
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy
    #[serde(default)]
    pub tool_policy: ChannelToolPolicy,
}

/// Slack Bot configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {
    /// Whether the Slack bot is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Slack Bot OAuth token (xoxb-...)
    #[serde(default)]
    pub bot_token: String,
    /// Slack App-Level token for Socket Mode (xapp-...)
    #[serde(default)]
    pub app_token: String,
    /// Slack signing secret (for verifying webhook requests)
    #[serde(default)]
    pub signing_secret: String,
    /// Allowed channel IDs (empty = allow all)
    #[serde(default)]
    pub allowed_channel_ids: Vec<String>,
    /// Admin user IDs — bypass DM policy (always allowed)
    #[serde(default)]
    pub admin_user_ids: Vec<String>,
    /// DM authorization policy
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy
    #[serde(default)]
    pub tool_policy: ChannelToolPolicy,
}

/// Signal messaging configuration (via signal-cli-rest-api).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalConfig {
    /// Whether Signal integration is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Signal CLI REST API URL (e.g., "http://localhost:8080")
    #[serde(default = "default_signal_api_url")]
    pub api_url: String,
    /// Bot's registered phone number (e.g., "+1234567890")
    #[serde(default)]
    pub phone_number: String,
    /// Allowed sender phone numbers (empty = allow all)
    #[serde(default)]
    pub allowed_numbers: Vec<String>,
    /// Admin phone numbers -- bypass DM policy (always allowed)
    #[serde(default)]
    pub admin_numbers: Vec<String>,
    /// DM authorization policy
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy
    #[serde(default)]
    pub tool_policy: ChannelToolPolicy,
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_url: default_signal_api_url(),
            phone_number: String::new(),
            allowed_numbers: Vec::new(),
            admin_numbers: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: ChannelToolPolicy::default(),
        }
    }
}

/// Microsoft Teams Bot Framework configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TeamsConfig {
    /// Whether Teams integration is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Azure Bot App ID (from Bot Framework registration)
    #[serde(default)]
    pub app_id: String,
    /// Azure Bot App Password / Client Secret
    #[serde(default)]
    pub app_password: String,
    /// Azure AD Tenant ID (optional; defaults to botframework.com)
    #[serde(default)]
    pub tenant_id: Option<String>,
    /// Allowed team/tenant IDs (empty = allow all)
    #[serde(default)]
    pub allowed_team_ids: Vec<String>,
    /// Admin user IDs (AAD object IDs) — bypass tool policy
    #[serde(default)]
    pub admin_user_ids: Vec<String>,
    /// DM policy for Teams messages (Open / Allowlist / Pairing).
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy
    #[serde(default)]
    pub tool_policy: ChannelToolPolicy,
}

/// Matrix Client-Server API configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixConfig {
    /// Whether Matrix integration is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Homeserver URL (e.g., "https://matrix.org")
    #[serde(default)]
    pub homeserver_url: String,
    /// Access token for the bot account
    #[serde(default)]
    pub access_token: String,
    /// Bot user ID (e.g., "@nexibot:matrix.org")
    #[serde(default)]
    pub user_id: String,
    /// Allowed room IDs (empty = allow all joined rooms)
    #[serde(default)]
    pub allowed_room_ids: Vec<String>,
    /// Command prefix for room messages (default: "!nexi")
    #[serde(default = "default_matrix_command_prefix")]
    pub command_prefix: Option<String>,
    /// Admin user IDs (e.g., "@admin:matrix.org") — bypass tool policy
    #[serde(default)]
    pub admin_user_ids: Vec<String>,
    /// DM policy for Matrix messages.
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy
    #[serde(default)]
    pub tool_policy: ChannelToolPolicy,
    /// Send typing indicators while processing messages (default: true).
    #[serde(default = "default_true")]
    pub typing_indicators: bool,
    /// Send read receipts when messages are received (default: true).
    #[serde(default = "default_true")]
    pub read_receipts: bool,
    /// Allow sending m.reaction events (default: true).
    #[serde(default = "default_true")]
    pub reactions_enabled: bool,
}

impl Default for MatrixConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            homeserver_url: String::new(),
            access_token: String::new(),
            user_id: String::new(),
            allowed_room_ids: Vec::new(),
            command_prefix: default_matrix_command_prefix(),
            admin_user_ids: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: ChannelToolPolicy::default(),
            typing_indicators: true,
            read_receipts: true,
            reactions_enabled: true,
        }
    }
}

fn default_nats_url() -> String {
    "nats://localhost:14222".to_string()
}

fn default_nats_inbound_subject() -> String {
    "nexibot.in.>".to_string()
}

/// NATS messaging bus configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatsConfig {
    /// Whether NATS integration is enabled
    #[serde(default)]
    pub enabled: bool,
    /// NATS server URL
    #[serde(default = "default_nats_url")]
    pub url: String,
    /// Inbound subject pattern (messages addressed to this agent)
    #[serde(default = "default_nats_inbound_subject")]
    pub inbound_subject: String,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: default_nats_url(),
            inbound_subject: default_nats_inbound_subject(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_config_has_feature_flags() {
        let c = MatrixConfig::default();
        assert!(c.typing_indicators, "typing_indicators should default to true");
        assert!(c.read_receipts, "read_receipts should default to true");
        assert!(c.reactions_enabled, "reactions_enabled should default to true");
    }
}
