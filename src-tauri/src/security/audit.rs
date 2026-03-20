//! Security Audit System
//!
//! Runs a comprehensive security audit across configuration, filesystem,
//! runtime state, and channel settings. Each check returns an optional
//! finding; the aggregate report summarizes posture with pass/fail counts
//! and severity-ranked findings.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info, warn};
use url::Url;

use crate::config::NexiBotConfig;
use crate::defense::DefenseConfig;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Severity level for an audit finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SecurityAuditSeverity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for SecurityAuditSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "Info"),
            Self::Low => write!(f, "Low"),
            Self::Medium => write!(f, "Medium"),
            Self::High => write!(f, "High"),
            Self::Critical => write!(f, "Critical"),
        }
    }
}

/// A single audit finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAuditFinding {
    /// Unique identifier for this check (e.g. "cfg-api-key").
    pub id: String,
    /// Severity of the finding.
    pub severity: SecurityAuditSeverity,
    /// Short human-readable title.
    pub title: String,
    /// Detailed description of the issue.
    pub description: String,
    /// Optional hint on how to remediate.
    pub fix_hint: Option<String>,
    /// Whether `auto_fix` can resolve this finding automatically.
    pub auto_fixable: bool,
}

/// Aggregate audit report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAuditReport {
    /// Findings discovered during the audit.
    pub findings: Vec<SecurityAuditFinding>,
    /// Number of checks that passed (no finding).
    pub passed_count: usize,
    /// Total number of checks executed.
    pub total_checks: usize,
    /// Timestamp of the audit run.
    pub timestamp: DateTime<Utc>,
}

/// Result of attempting to auto-fix a finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixResult {
    /// The finding ID that was targeted.
    pub finding_id: String,
    /// Whether the fix succeeded.
    pub success: bool,
    /// Human-readable message describing what happened.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Individual checks — each returns `Option<SecurityAuditFinding>`
// ---------------------------------------------------------------------------

/// Check: at least one API key is configured so the bot can function.
fn check_api_key_presence(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    let has_claude = config
        .claude
        .api_key
        .as_ref()
        .is_some_and(|k| !k.is_empty());
    let has_openai = config
        .openai
        .api_key
        .as_ref()
        .is_some_and(|k| !k.is_empty());
    let has_cerebras = config
        .cerebras
        .api_key
        .as_ref()
        .is_some_and(|k| !k.is_empty());
    let has_google = config
        .google
        .as_ref()
        .and_then(|g| g.api_key.as_ref())
        .is_some_and(|k| !k.is_empty());
    let has_deepseek = config
        .deepseek
        .as_ref()
        .and_then(|d| d.api_key.as_ref())
        .is_some_and(|k| !k.is_empty());
    let has_github_copilot = config
        .github_copilot
        .as_ref()
        .and_then(|c| c.token.as_ref())
        .is_some_and(|t| !t.is_empty());
    let has_minimax = config
        .minimax
        .as_ref()
        .and_then(|m| m.api_key.as_ref())
        .is_some_and(|k| !k.is_empty());
    let has_ollama = config.ollama.enabled;

    if has_claude
        || has_openai
        || has_cerebras
        || has_google
        || has_deepseek
        || has_github_copilot
        || has_minimax
        || has_ollama
    {
        return None;
    }
    Some(SecurityAuditFinding {
        id: "cfg-api-key".into(),
        severity: SecurityAuditSeverity::Critical,
        title: "No API key configured".into(),
        description: "No LLM provider credential is configured and Ollama is disabled. The bot cannot function without at least one provider credential (Claude, OpenAI, Cerebras, Google, DeepSeek, GitHub Copilot, or MiniMax) or Ollama enabled.".into(),
        fix_hint: Some("Add at least one provider credential (for example claude.api_key, openai.api_key, or google.api_key), or enable ollama.enabled for local inference.".into()),
        auto_fixable: false,
    })
}

/// Check: defense pipeline is enabled.
fn check_defense_enabled(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.defense.enabled {
        return None;
    }
    Some(SecurityAuditFinding {
        id: "cfg-defense-disabled".into(),
        severity: SecurityAuditSeverity::High,
        title: "Defense pipeline disabled".into(),
        description: "The multi-model defense pipeline is disabled. Prompt injection and content safety checks will not run.".into(),
        fix_hint: Some("Set defense.enabled = true in config.".into()),
        auto_fixable: true,
    })
}

/// Check: DM policy is set to Pairing (safe default) on at least one enabled channel.
fn check_dm_policy(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    let mut channels_without_policy = Vec::new();
    let mut channels_open = Vec::new();

    if config.telegram.enabled
        && config.telegram.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.telegram.allowed_chat_ids.is_empty()
    {
        channels_without_policy.push("Telegram");
    }
    if config.whatsapp.enabled
        && config.whatsapp.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.whatsapp.allowed_phone_numbers.is_empty()
    {
        channels_without_policy.push("WhatsApp");
    }
    if config.discord.enabled
        && config.discord.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.discord.allowed_guild_ids.is_empty()
        && config.discord.allowed_channel_ids.is_empty()
        && config.discord.admin_user_ids.is_empty()
    {
        channels_without_policy.push("Discord");
    }
    if config.discord.enabled && config.discord.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("Discord");
    }
    if config.slack.enabled
        && config.slack.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.slack.allowed_channel_ids.is_empty()
        && config.slack.admin_user_ids.is_empty()
    {
        channels_without_policy.push("Slack");
    }
    if config.slack.enabled && config.slack.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("Slack");
    }
    if config.signal.enabled
        && config.signal.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.signal.allowed_numbers.is_empty()
    {
        channels_without_policy.push("Signal");
    }
    if config.signal.enabled && config.signal.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("Signal");
    }
    if config.bluebubbles.enabled
        && config.bluebubbles.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.bluebubbles.allowed_handles.is_empty()
    {
        channels_without_policy.push("BlueBubbles");
    }
    if config.bluebubbles.enabled && config.bluebubbles.dm_policy == crate::pairing::DmPolicy::Open
    {
        channels_open.push("BlueBubbles");
    }
    if config.google_chat.enabled
        && config.google_chat.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.google_chat.allowed_spaces.is_empty()
    {
        channels_without_policy.push("Google Chat");
    }
    if config.google_chat.enabled && config.google_chat.dm_policy == crate::pairing::DmPolicy::Open
    {
        channels_open.push("Google Chat");
    }
    if config.mattermost.enabled
        && config.mattermost.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.mattermost.allowed_channel_ids.is_empty()
    {
        channels_without_policy.push("Mattermost");
    }
    if config.mattermost.enabled && config.mattermost.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("Mattermost");
    }
    if config.messenger.enabled
        && config.messenger.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.messenger.allowed_sender_ids.is_empty()
    {
        channels_without_policy.push("Messenger");
    }
    if config.messenger.enabled && config.messenger.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("Messenger");
    }
    if config.instagram.enabled
        && config.instagram.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.instagram.allowed_sender_ids.is_empty()
    {
        channels_without_policy.push("Instagram");
    }
    if config.instagram.enabled && config.instagram.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("Instagram");
    }
    if config.line.enabled
        && config.line.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.line.allowed_user_ids.is_empty()
    {
        channels_without_policy.push("LINE");
    }
    if config.line.enabled && config.line.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("LINE");
    }
    if config.twilio.enabled
        && config.twilio.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.twilio.allowed_numbers.is_empty()
    {
        channels_without_policy.push("Twilio");
    }
    if config.twilio.enabled && config.twilio.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("Twilio");
    }
    if config.mastodon.enabled
        && config.mastodon.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.mastodon.allowed_account_ids.is_empty()
    {
        channels_without_policy.push("Mastodon");
    }
    if config.mastodon.enabled && config.mastodon.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("Mastodon");
    }
    if config.rocketchat.enabled
        && config.rocketchat.dm_policy == crate::pairing::DmPolicy::Allowlist
        && config.rocketchat.allowed_room_ids.is_empty()
    {
        channels_without_policy.push("Rocket.Chat");
    }
    if config.rocketchat.enabled && config.rocketchat.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("Rocket.Chat");
    }
    if config.webchat.enabled
        && config.webchat.dm_policy == crate::pairing::DmPolicy::Allowlist
        && !config.webchat.require_api_key
    {
        channels_without_policy.push("WebChat");
    }
    if config.webchat.enabled && config.webchat.dm_policy == crate::pairing::DmPolicy::Open {
        channels_open.push("WebChat");
    }

    if channels_without_policy.is_empty() && channels_open.is_empty() {
        return None;
    }

    let mut details = Vec::new();
    if !channels_without_policy.is_empty() {
        details.push(format!(
            "Allowlist mode with empty allowlist (allows all senders): {}",
            channels_without_policy.join(", ")
        ));
    }
    if !channels_open.is_empty() {
        details.push(format!(
            "Open DM policy (explicitly allows all senders): {}",
            channels_open.join(", ")
        ));
    }

    Some(SecurityAuditFinding {
        id: "cfg-dm-policy".into(),
        severity: SecurityAuditSeverity::High,
        title: "DM policy allows all senders".into(),
        description: format!(
            "{}. Consider switching to Pairing mode or populating explicit allowlists/admin lists.",
            details.join(" | ")
        ),
        fix_hint: Some(
            "Set dm_policy to \"Pairing\" or populate the allowlist/admin list for each channel."
                .into(),
        ),
        auto_fixable: false,
    })
}

/// Check: ingress scope should not be left fully open on channel integrations.
fn check_channel_ingress_scope(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    let mut open_ingress = Vec::new();

    if config.teams.enabled && config.teams.allowed_team_ids.is_empty() {
        open_ingress.push("Microsoft Teams (all teams/tenants)");
    }

    if config.matrix.enabled && config.matrix.allowed_room_ids.is_empty() {
        open_ingress.push("Matrix (all joined rooms)");
    }

    if config.discord.enabled
        && config.discord.allowed_guild_ids.is_empty()
        && config.discord.allowed_channel_ids.is_empty()
    {
        open_ingress.push("Discord (all guilds/channels)");
    }

    if config.slack.enabled && config.slack.allowed_channel_ids.is_empty() {
        open_ingress.push("Slack (all channels/DMs)");
    }

    if config.google_chat.enabled && config.google_chat.allowed_spaces.is_empty() {
        open_ingress.push("Google Chat (all spaces)");
    }

    if config.mattermost.enabled && config.mattermost.allowed_channel_ids.is_empty() {
        open_ingress.push("Mattermost (all channels)");
    }

    if config.rocketchat.enabled && config.rocketchat.allowed_room_ids.is_empty() {
        open_ingress.push("Rocket.Chat (all rooms)");
    }

    if config.email.enabled && config.email.allowed_senders.is_empty() {
        open_ingress.push("Email (all senders)");
    }

    if open_ingress.is_empty() {
        return None;
    }

    Some(SecurityAuditFinding {
        id: "ch-ingress-open".into(),
        severity: SecurityAuditSeverity::Medium,
        title: "Channel ingress is broadly open".into(),
        description: format!(
            "These enabled channels accept inbound messages from any reachable room/space unless additional DM policy restrictions apply: {}.",
            open_ingress.join(", ")
        ),
        fix_hint: Some(
            "Populate channel allowlists (e.g., discord.allowed_guild_ids / discord.allowed_channel_ids, slack.allowed_channel_ids) to scope where NexiBot can be addressed.".into(),
        ),
        auto_fixable: false,
    })
}

/// Check: SSRF protection — fetch tool should have blocked domains.
fn check_ssrf_policy(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.fetch.enabled && config.fetch.blocked_domains.is_empty() {
        return Some(SecurityAuditFinding {
            id: "cfg-ssrf-blocked-domains".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "Fetch tool has no blocked domains".into(),
            description: "The fetch tool is enabled but has no blocked domains. Internal network addresses (localhost, metadata endpoints) could be reachable.".into(),
            fix_hint: Some("Add at least localhost, 127.0.0.1, 169.254.169.254 to fetch.blocked_domains.".into()),
            auto_fixable: true,
        });
    }
    None
}

/// Check: execute tool safety — should use DCG or be disabled.
fn check_execute_tool_safety(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.execute.enabled && !config.execute.use_dcg {
        return Some(SecurityAuditFinding {
            id: "cfg-execute-no-dcg".into(),
            severity: SecurityAuditSeverity::High,
            title: "Execute tool enabled without DCG".into(),
            description: "The code execution tool is enabled but Destructive Command Guard (DCG) is disabled. This allows arbitrary command execution without safety checks.".into(),
            fix_hint: Some("Set execute.use_dcg = true or disable the execute tool entirely.".into()),
            auto_fixable: true,
        });
    }
    None
}

/// Check: guardrails security level is not Disabled.
fn check_guardrails_level(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.guardrails.security_level == crate::guardrails::SecurityLevel::Disabled {
        return Some(SecurityAuditFinding {
            id: "cfg-guardrails-disabled".into(),
            severity: SecurityAuditSeverity::Critical,
            title: "Guardrails security disabled".into(),
            description: "The guardrails security level is set to Disabled. All safety checks (destructive commands, sensitive data, prompt injection) are bypassed.".into(),
            fix_hint: Some("Set guardrails.security_level to Standard or higher.".into()),
            auto_fixable: false,
        });
    }
    None
}

/// Check: defense fail_open is not true in production-like setups.
fn check_defense_fail_open(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.defense.enabled && config.defense.fail_open {
        return Some(SecurityAuditFinding {
            id: "cfg-defense-fail-open".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "Defense pipeline set to fail-open".into(),
            description: "The defense pipeline will allow requests through even when no models are loaded. In production this should be fail-closed.".into(),
            fix_hint: Some("Set defense.fail_open = false for production deployments.".into()),
            auto_fixable: true,
        });
    }
    None
}

/// Check: config file permissions are restrictive (cross-platform).
fn check_config_file_permissions() -> Option<SecurityAuditFinding> {
    let config_path = match NexiBotConfig::config_path() {
        Ok(p) => p,
        Err(_) => return None,
    };

    if !config_path.exists() {
        return None;
    }

    if let Some(warning) = crate::platform::file_security::check_file_permissions(&config_path) {
        return Some(SecurityAuditFinding {
            id: "fs-config-perms".into(),
            severity: SecurityAuditSeverity::High,
            title: "Config file permissions too permissive".into(),
            description: format!(
                "{}. Other users may read API keys.",
                warning
            ),
            fix_hint: Some(crate::platform::file_security::fix_hint_for_file(&config_path)),
            auto_fixable: true,
        });
    }
    None
}

/// Check: sessions directory permissions are restrictive (cross-platform).
fn check_sessions_dir_permissions() -> Option<SecurityAuditFinding> {
    let sessions_dir = config_dir_path().map(|d| d.join("sessions"));
    let sessions_dir = match sessions_dir {
        Some(d) if d.exists() => d,
        _ => return None,
    };

    if let Some(warning) = crate::platform::file_security::check_dir_permissions(&sessions_dir) {
        return Some(SecurityAuditFinding {
            id: "fs-sessions-perms".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "Sessions directory permissions too permissive".into(),
            description: format!(
                "{}. Session data may be exposed.",
                warning
            ),
            fix_hint: Some(crate::platform::file_security::fix_hint_for_dir(&sessions_dir)),
            auto_fixable: true,
        });
    }
    None
}

/// Check: credential storage — keyring should be available.
fn check_credential_storage() -> Option<SecurityAuditFinding> {
    if !crate::security::credentials::is_keyring_available() {
        return Some(SecurityAuditFinding {
            id: "fs-keyring-unavailable".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "OS keyring not available".into(),
            description: "The OS keyring is not available. Secrets (API keys, tokens) will be stored in plaintext in the config file.".into(),
            fix_hint: Some("Ensure a keyring daemon (e.g. gnome-keyring, macOS Keychain) is running.".into()),
            auto_fixable: false,
        });
    }
    None
}

/// Check: defense models loaded (runtime status).
fn check_defense_models_loaded(defense: &DefenseConfig) -> Option<SecurityAuditFinding> {
    // We can only check config-level hints here; actual model loaded state
    // requires the DefensePipeline instance which we don't hold in this context.
    // Instead we check if defense is enabled but no model backends are configured.
    if defense.enabled && !defense.deberta_enabled && !defense.llama_guard_enabled {
        return Some(SecurityAuditFinding {
            id: "rt-defense-no-models".into(),
            severity: SecurityAuditSeverity::High,
            title: "No defense models configured".into(),
            description: "The defense pipeline is enabled but neither DeBERTa nor Llama Guard backends are enabled. No content will be scanned.".into(),
            fix_hint: Some("Enable at least one defense backend: defense.deberta_enabled or defense.llama_guard_enabled.".into()),
            auto_fixable: false,
        });
    }
    None
}

/// Check: session encryption (placeholder — check if sessions dir uses encrypted storage).
fn check_session_encryption() -> Option<SecurityAuditFinding> {
    // Session files are stored as plaintext JSON today. Flag as informational.
    let sessions_dir = config_dir_path().map(|d| d.join("sessions"));
    if let Some(dir) = sessions_dir {
        if dir.exists() {
            return Some(SecurityAuditFinding {
                id: "rt-session-encryption".into(),
                severity: SecurityAuditSeverity::Info,
                title: "Session data stored unencrypted".into(),
                description: "Chat sessions are stored as plaintext JSON on disk. Consider encrypting at rest if the machine is shared.".into(),
                fix_hint: Some("Use full-disk encryption or an encrypted home directory.".into()),
                auto_fixable: false,
            });
        }
    }
    None
}

/// Check: workspace confinement is active (filesystem tool has path restrictions).
fn check_workspace_confinement(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.filesystem.enabled && config.filesystem.blocked_paths.is_empty() {
        return Some(SecurityAuditFinding {
            id: "rt-workspace-confinement".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "Filesystem tool has no blocked paths".into(),
            description: "The filesystem tool is enabled with no blocked paths. Sensitive system directories could be accessed.".into(),
            fix_hint: Some("Populate filesystem.blocked_paths with system directories like /etc, /System, /usr.".into()),
            auto_fixable: true,
        });
    }
    None
}

/// Check: webhook auth is configured when webhook server is enabled.
fn check_webhook_auth(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.webhooks.enabled
        && config
            .webhooks
            .auth_token
            .as_ref()
            .is_none_or(|t| t.is_empty())
    {
        return Some(SecurityAuditFinding {
            id: "ch-webhook-auth".into(),
            severity: SecurityAuditSeverity::High,
            title: "Webhook server has no auth token".into(),
            description: "The webhook server is enabled but no bearer auth_token is configured. Anyone who can reach the port can trigger webhooks.".into(),
            fix_hint: Some("Set webhooks.auth_token to a strong random value.".into()),
            auto_fixable: false,
        });
    }
    None
}

/// Check: at least one channel has admin IDs populated.
fn check_admin_lists(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    let has_telegram_admin = config.telegram.enabled && !config.telegram.admin_chat_ids.is_empty();
    let has_whatsapp_admin =
        config.whatsapp.enabled && !config.whatsapp.admin_phone_numbers.is_empty();
    let has_discord_admin = config.discord.enabled && !config.discord.admin_user_ids.is_empty();
    let has_slack_admin = config.slack.enabled && !config.slack.admin_user_ids.is_empty();
    let has_signal_admin = config.signal.enabled && !config.signal.admin_numbers.is_empty();
    let has_teams_admin = config.teams.enabled && !config.teams.admin_user_ids.is_empty();
    let has_matrix_admin = config.matrix.enabled && !config.matrix.admin_user_ids.is_empty();
    let has_bluebubbles_admin =
        config.bluebubbles.enabled && !config.bluebubbles.admin_handles.is_empty();
    let has_google_chat_admin =
        config.google_chat.enabled && !config.google_chat.admin_user_ids.is_empty();
    let has_mattermost_admin =
        config.mattermost.enabled && !config.mattermost.admin_user_ids.is_empty();
    let has_messenger_admin =
        config.messenger.enabled && !config.messenger.admin_sender_ids.is_empty();
    let has_instagram_admin =
        config.instagram.enabled && !config.instagram.admin_sender_ids.is_empty();
    let has_line_admin = config.line.enabled && !config.line.admin_user_ids.is_empty();
    let has_twilio_admin = config.twilio.enabled && !config.twilio.admin_numbers.is_empty();
    let has_mastodon_admin =
        config.mastodon.enabled && !config.mastodon.admin_account_ids.is_empty();
    let has_rocketchat_admin =
        config.rocketchat.enabled && !config.rocketchat.admin_user_ids.is_empty();

    let any_channel_enabled = config.telegram.enabled
        || config.whatsapp.enabled
        || config.discord.enabled
        || config.slack.enabled
        || config.signal.enabled
        || config.teams.enabled
        || config.matrix.enabled
        || config.bluebubbles.enabled
        || config.google_chat.enabled
        || config.mattermost.enabled
        || config.messenger.enabled
        || config.instagram.enabled
        || config.line.enabled
        || config.twilio.enabled
        || config.mastodon.enabled
        || config.rocketchat.enabled;

    if any_channel_enabled
        && !has_telegram_admin
        && !has_whatsapp_admin
        && !has_discord_admin
        && !has_slack_admin
        && !has_signal_admin
        && !has_teams_admin
        && !has_matrix_admin
        && !has_bluebubbles_admin
        && !has_google_chat_admin
        && !has_mattermost_admin
        && !has_messenger_admin
        && !has_instagram_admin
        && !has_line_admin
        && !has_twilio_admin
        && !has_mastodon_admin
        && !has_rocketchat_admin
    {
        return Some(SecurityAuditFinding {
            id: "ch-admin-list".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "No admin users configured on any channel".into(),
            description: "Channels are enabled but no admin user IDs are set. Pairing approvals and elevated operations will not be possible via channel messages.".into(),
            fix_hint: Some("Add your user/chat ID to the admin list on at least one enabled channel.".into()),
            auto_fixable: false,
        });
    }
    None
}

/// Check: Telegram admin_chat_ids should avoid group/supergroup IDs.
///
/// Telegram group and supergroup IDs are negative. Treating a group as "admin"
/// effectively grants elevated behavior to all chat participants.
fn check_telegram_group_admin_ids(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if !config.telegram.enabled {
        return None;
    }

    let risky_ids: Vec<i64> = config
        .telegram
        .admin_chat_ids
        .iter()
        .copied()
        .filter(|id| *id < 0)
        .collect();

    if risky_ids.is_empty() {
        return None;
    }

    Some(SecurityAuditFinding {
        id: "ch-telegram-group-admin".into(),
        severity: SecurityAuditSeverity::Medium,
        title: "Telegram admin list includes group chat IDs".into(),
        description: format!(
            "telegram.admin_chat_ids contains group/supergroup IDs (negative values): {}. In group chats, admin-level bypass and approval actions can be triggered by any participant.",
            risky_ids
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        fix_hint: Some(
            "Use private Telegram DM chat IDs for admin privileges, and keep shared groups in allowed_chat_ids only.".into(),
        ),
        auto_fixable: false,
    })
}

/// Check: Slack admin_user_ids should contain user IDs (start with "U"), not channel/workspace IDs.
///
/// Slack user IDs start with "U". Channel IDs start with "C"; workspace-level IDs start with "W".
/// A channel ID in the admin list grants admin to everyone who posts in that channel.
fn check_slack_admin_ids(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if !config.slack.enabled {
        return None;
    }

    let bad: Vec<&String> = config
        .slack
        .admin_user_ids
        .iter()
        .filter(|id| {
            let upper = id.to_uppercase();
            upper.starts_with('C') || upper.starts_with('W')
        })
        .collect();

    if bad.is_empty() {
        return None;
    }

    Some(SecurityAuditFinding {
        id: "ch-slack-channel-admin".into(),
        severity: SecurityAuditSeverity::Medium,
        title: "Slack admin list includes channel or workspace IDs".into(),
        description: format!(
            "slack.admin_user_ids contains IDs that appear to be channel (C…) or workspace (W…) IDs rather than user IDs (U…): {}. Any user posting in those channels would receive admin privileges.",
            bad.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ),
        fix_hint: Some(
            "Use Slack user IDs (starting with U) in admin_user_ids. Find your user ID via the Slack profile → More → Copy member ID.".into(),
        ),
        auto_fixable: false,
    })
}

/// Check: Matrix admin_user_ids should contain user IDs (start with "@"), not room IDs.
///
/// Matrix user IDs start with "@" (e.g., "@admin:matrix.org"). Room IDs start with "!".
/// A room ID in the admin list grants elevated access to all room members.
fn check_matrix_admin_ids(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if !config.matrix.enabled {
        return None;
    }

    let bad: Vec<&String> = config
        .matrix
        .admin_user_ids
        .iter()
        .filter(|id| id.starts_with('!'))
        .collect();

    if bad.is_empty() {
        return None;
    }

    Some(SecurityAuditFinding {
        id: "ch-matrix-room-admin".into(),
        severity: SecurityAuditSeverity::Medium,
        title: "Matrix admin list includes room IDs".into(),
        description: format!(
            "matrix.admin_user_ids contains room IDs (starting with '!'): {}. All members of those rooms would receive admin-level access.",
            bad.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ),
        fix_hint: Some(
            "Use Matrix user IDs (starting with '@') in admin_user_ids, e.g. @you:matrix.org.".into(),
        ),
        auto_fixable: false,
    })
}

/// Check: Signal admin_numbers should be E.164-formatted phone numbers (start with "+").
///
/// Entries without a leading "+" will silently never match any incoming sender,
/// leaving admin operations unreachable.
fn check_signal_admin_numbers(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if !config.signal.enabled {
        return None;
    }

    let bad: Vec<&String> = config
        .signal
        .admin_numbers
        .iter()
        .filter(|n| !n.starts_with('+'))
        .collect();

    if bad.is_empty() {
        return None;
    }

    Some(SecurityAuditFinding {
        id: "ch-signal-admin-format".into(),
        severity: SecurityAuditSeverity::Low,
        title: "Signal admin numbers missing E.164 '+' prefix".into(),
        description: format!(
            "signal.admin_numbers contains entries without a leading '+': {}. These will never match an incoming Signal sender and admin access will be silently unavailable.",
            bad.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ),
        fix_hint: Some(
            "Format admin numbers in E.164 international format, e.g. +15551234567.".into(),
        ),
        auto_fixable: false,
    })
}

/// Check: Email allowed_senders should contain valid email addresses (must contain "@").
///
/// Entries without "@" will silently never match any sender, leaving the allowlist
/// ineffective or blocking all mail if DM policy is strict.
fn check_email_admin_addresses(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if !config.email.enabled {
        return None;
    }

    let bad: Vec<&String> = config
        .email
        .allowed_senders
        .iter()
        .filter(|addr| !addr.contains('@'))
        .collect();

    if bad.is_empty() {
        return None;
    }

    Some(SecurityAuditFinding {
        id: "ch-email-admin-format".into(),
        severity: SecurityAuditSeverity::Low,
        title: "Email allowed_senders contains malformed addresses".into(),
        description: format!(
            "email.allowed_senders contains entries without '@': {}. These entries will silently never match any sender address.",
            bad.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ),
        fix_hint: Some(
            "Ensure all entries in email.allowed_senders are valid email addresses containing '@'.".into(),
        ),
        auto_fixable: false,
    })
}

/// Whether autonomous mode currently has any capability set to AskUser.
fn autonomous_mode_uses_ask_user(config: &NexiBotConfig) -> bool {
    use crate::config::AutonomyLevel;

    if !config.autonomous_mode.enabled {
        return false;
    }

    let a = &config.autonomous_mode;
    a.filesystem.read == AutonomyLevel::AskUser
        || a.filesystem.write == AutonomyLevel::AskUser
        || a.filesystem.delete == AutonomyLevel::AskUser
        || a.execute.run_command == AutonomyLevel::AskUser
        || a.execute.run_python == AutonomyLevel::AskUser
        || a.execute.run_node == AutonomyLevel::AskUser
        || a.fetch.get_requests == AutonomyLevel::AskUser
        || a.fetch.post_requests == AutonomyLevel::AskUser
        || a.browser.navigate == AutonomyLevel::AskUser
        || a.browser.interact == AutonomyLevel::AskUser
        || a.computer_use.level == AutonomyLevel::AskUser
        || a.settings_modification.level == AutonomyLevel::AskUser
        || a.memory_modification.level == AutonomyLevel::AskUser
        || a.soul_modification.level == AutonomyLevel::AskUser
        || a.mcp
            .values()
            .any(|capability| capability.level == AutonomyLevel::AskUser)
}

/// Check: when any confirmation policy is enabled, channel sources should have
/// a usable approval acceptance path.
///
/// Today, in-channel approval handling exists for Telegram, Discord, Slack,
/// Signal, WhatsApp, Microsoft Teams, Matrix, Mattermost, Google Chat,
/// Twilio SMS, Facebook Messenger, Instagram, LINE, Mastodon, Rocket.Chat,
/// BlueBubbles, WebChat, and the desktop GUI.
/// Most other channel integrations still fail closed on confirmation-required
/// tool calls.
fn check_confirmation_acceptance_path(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    let guardrails_confirmation = config.guardrails.confirm_external_actions;
    let autonomy_ask_user = autonomous_mode_uses_ask_user(config);
    let browser_confirmation = config.browser.enabled && config.browser.require_confirmation;
    let computer_use_confirmation =
        config.computer_use.enabled && config.computer_use.require_confirmation;
    if !guardrails_confirmation
        && !autonomy_ask_user
        && !browser_confirmation
        && !computer_use_confirmation
    {
        return None;
    }

    let mut missing_paths: Vec<&str> = Vec::new();

    if config.email.enabled {
        missing_paths.push("Email");
    }

    if missing_paths.is_empty() {
        return None;
    }

    let mut basis_parts: Vec<&str> = Vec::new();
    if guardrails_confirmation {
        basis_parts.push("guardrails.confirm_external_actions is enabled");
    }
    if autonomy_ask_user {
        basis_parts.push("autonomous mode AskUser capabilities are enabled");
    }
    if browser_confirmation {
        basis_parts.push("browser.require_confirmation is enabled");
    }
    if computer_use_confirmation {
        basis_parts.push("computer_use.require_confirmation is enabled");
    }

    let policy_basis = match basis_parts.as_slice() {
        [] => unreachable!("checked above"),
        [only] => (*only).to_string(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let mut prefix = basis_parts;
            let last = prefix.pop().unwrap_or_default();
            format!("{}, and {}", prefix.join(", "), last)
        }
    };

    Some(SecurityAuditFinding {
        id: "ch-confirmation-acceptance-path".into(),
        severity: SecurityAuditSeverity::Low,
        title: "Some channels cannot accept confirmation requests".into(),
        description: format!(
            "{}, but these enabled channels do not have an in-channel approval acceptance flow yet: {}. Confirmation-required tool calls from those channels will be blocked unless approved via the desktop GUI, Telegram, Discord, Slack, Signal, WhatsApp, Microsoft Teams, Matrix, Mattermost, Google Chat, Twilio SMS, Facebook Messenger, Instagram, LINE, Mastodon, Rocket.Chat, BlueBubbles, or WebChat.",
            policy_basis,
            missing_paths.join(", ")
        ),
        fix_hint: Some(
            "Use desktop chat, Telegram, Discord, Slack, Signal, WhatsApp, Microsoft Teams, Matrix, Mattermost, Google Chat, Twilio SMS, Facebook Messenger, Instagram, LINE, Mastodon, Rocket.Chat, BlueBubbles, or WebChat for confirmation-required operations, or disable confirmation-required policies (guardrails.confirm_external_actions, autonomous AskUser levels, browser.require_confirmation, and/or computer_use.require_confirmation) when channel-only execution is required.".into(),
        ),
        auto_fixable: false,
    })
}

/// Check: some background-task channels still depend on desktop GUI routing.
///
/// Background tasks preserve source-channel tool policy.
/// Telegram, Discord, Slack, Signal, WhatsApp, Microsoft Teams, Matrix,
/// Mattermost, and Google Chat background tasks can collect in-channel
/// approvals, and Twilio SMS background tasks can collect in-channel approvals
/// too. Facebook Messenger, Instagram, LINE, Mastodon, Rocket.Chat, and
/// BlueBubbles background tasks can also collect in-channel approvals.
/// WebChat background tasks can collect in-channel approvals while the
/// originating browser session remains connected.
/// Most other channels currently route background approvals through desktop GUI
/// events. In channel-only/headless deployments, those actions can fail closed.
fn check_background_confirmation_path(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    let guardrails_confirmation = config.guardrails.confirm_external_actions;
    let autonomy_ask_user = autonomous_mode_uses_ask_user(config);
    let browser_confirmation = config.browser.enabled && config.browser.require_confirmation;
    let computer_use_confirmation =
        config.computer_use.enabled && config.computer_use.require_confirmation;
    if !guardrails_confirmation
        && !autonomy_ask_user
        && !browser_confirmation
        && !computer_use_confirmation
    {
        return None;
    }

    let mut gui_dependent_channels: Vec<&str> = Vec::new();

    if config.email.enabled {
        gui_dependent_channels.push("Email");
    }

    if gui_dependent_channels.is_empty() {
        return None;
    }

    Some(SecurityAuditFinding {
        id: "bg-confirmation-gui-dependency".into(),
        severity: SecurityAuditSeverity::Low,
        title: "Some background task channels depend on desktop GUI confirmation".into(),
        description: format!(
            "Confirmation-required operations inside nexibot_background_task still rely on desktop GUI approval routing for these enabled channels: {}. In headless/channel-only deployments, those background steps can be blocked. Telegram, Discord, Slack, Signal, WhatsApp, Microsoft Teams, Matrix, Mattermost, Google Chat, Twilio SMS, Facebook Messenger, Instagram, LINE, Mastodon, Rocket.Chat, BlueBubbles, and WebChat background tasks support in-channel approval replies.",
            gui_dependent_channels.join(", ")
        ),
        fix_hint: Some(
            "Use desktop UI for background confirmations on these channels, keep sensitive confirmation-required work in foreground channel flows, or relax confirmation-required policies when headless background execution is required.".into(),
        ),
        auto_fixable: false,
    })
}

/// Check: pairing is enabled on channels using Pairing DM policy.
fn check_pairing_enabled(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    let uses_pairing = (config.telegram.enabled
        && config.telegram.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.whatsapp.enabled
            && config.whatsapp.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.discord.enabled
            && config.discord.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.slack.enabled && config.slack.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.signal.enabled && config.signal.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.bluebubbles.enabled
            && config.bluebubbles.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.google_chat.enabled
            && config.google_chat.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.mattermost.enabled
            && config.mattermost.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.messenger.enabled
            && config.messenger.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.instagram.enabled
            && config.instagram.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.line.enabled && config.line.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.twilio.enabled && config.twilio.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.mastodon.enabled
            && config.mastodon.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.rocketchat.enabled
            && config.rocketchat.dm_policy == crate::pairing::DmPolicy::Pairing)
        || (config.webchat.enabled
            && config.webchat.dm_policy == crate::pairing::DmPolicy::Pairing);

    if !uses_pairing {
        let any_channel = config.telegram.enabled
            || config.whatsapp.enabled
            || config.discord.enabled
            || config.slack.enabled
            || config.signal.enabled
            || config.bluebubbles.enabled
            || config.google_chat.enabled
            || config.mattermost.enabled
            || config.messenger.enabled
            || config.instagram.enabled
            || config.line.enabled
            || config.twilio.enabled
            || config.mastodon.enabled
            || config.rocketchat.enabled
            || config.webchat.enabled;

        if any_channel {
            return Some(SecurityAuditFinding {
                id: "ch-pairing-not-used".into(),
                severity: SecurityAuditSeverity::Low,
                title: "Pairing DM policy not used on any channel".into(),
                description: "No enabled channel uses the Pairing DM policy. Pairing provides a secure handshake for unknown senders.".into(),
                fix_hint: Some("Set dm_policy to \"Pairing\" on your messaging channels.".into()),
                auto_fixable: false,
            });
        }
    }
    None
}

/// Check: prompt injection detection is blocking (not just warning).
fn check_prompt_injection_blocking(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.guardrails.detect_prompt_injection && !config.guardrails.block_prompt_injection {
        return Some(SecurityAuditFinding {
            id: "cfg-prompt-injection-warn-only".into(),
            severity: SecurityAuditSeverity::Low,
            title: "Prompt injection detection is warn-only".into(),
            description: "Prompt injection detection is enabled but not set to blocking mode. Detected injections will only produce warnings, not prevent execution.".into(),
            fix_hint: Some("Set guardrails.block_prompt_injection = true.".into()),
            auto_fixable: true,
        });
    }
    None
}

/// Check: skill runtime execution is disabled by default.
fn check_skill_exec_disabled(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.execute.skill_runtime_exec_enabled {
        return Some(SecurityAuditFinding {
            id: "cfg-skill-exec-enabled".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "Skill runtime execution is enabled".into(),
            description: "Skills can trigger command execution at runtime. This increases the attack surface if a malicious skill is installed.".into(),
            fix_hint: Some("Set execute.skill_runtime_exec_enabled = false unless you specifically need skills to execute commands.".into()),
            auto_fixable: false,
        });
    }
    None
}

/// Check: channel bot tokens are present for enabled channels.
fn check_channel_tokens(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    let mut missing = Vec::new();

    if config.telegram.enabled && config.telegram.bot_token.is_empty() {
        missing.push("Telegram (bot_token)");
    }
    if config.whatsapp.enabled
        && (config.whatsapp.phone_number_id.is_empty()
            || config.whatsapp.access_token.is_empty()
            || config.whatsapp.verify_token.is_empty()
            || config.whatsapp.app_secret.is_empty())
    {
        missing.push("WhatsApp (phone_number_id/access_token/verify_token/app_secret)");
    }
    if config.discord.enabled && config.discord.bot_token.is_empty() {
        missing.push("Discord (bot_token)");
    }
    if config.slack.enabled && config.slack.bot_token.is_empty() {
        missing.push("Slack (bot_token)");
    }
    if config.slack.enabled && config.slack.signing_secret.is_empty() {
        missing.push("Slack (signing_secret)");
    }
    if config.signal.enabled
        && (config.signal.api_url.is_empty() || config.signal.phone_number.is_empty())
    {
        missing.push("Signal (api_url/phone_number)");
    }
    if config.teams.enabled
        && (config.teams.app_id.is_empty() || config.teams.app_password.is_empty())
    {
        missing.push("Teams (app_id/app_password)");
    }
    if config.matrix.enabled
        && (config.matrix.homeserver_url.is_empty()
            || config.matrix.access_token.is_empty()
            || config.matrix.user_id.is_empty())
    {
        missing.push("Matrix (homeserver_url/access_token/user_id)");
    }
    if config.bluebubbles.enabled
        && (config.bluebubbles.server_url.is_empty() || config.bluebubbles.password.is_empty())
    {
        missing.push("BlueBubbles (server_url/password)");
    }
    if config.google_chat.enabled
        && (config.google_chat.incoming_webhook_url.is_empty()
            || config.google_chat.verification_token.is_empty())
    {
        missing.push("Google Chat (incoming_webhook_url/verification_token)");
    }
    if config.mattermost.enabled
        && (config.mattermost.server_url.is_empty() || config.mattermost.bot_token.is_empty())
    {
        missing.push("Mattermost (server_url/bot_token)");
    }
    if config.messenger.enabled
        && (config.messenger.page_access_token.is_empty()
            || config.messenger.verify_token.is_empty()
            || config.messenger.app_secret.is_empty())
    {
        missing.push("Messenger (page_access_token/verify_token/app_secret)");
    }
    if config.instagram.enabled
        && (config.instagram.access_token.is_empty()
            || config.instagram.instagram_account_id.is_empty()
            || config.instagram.verify_token.is_empty()
            || config.instagram.app_secret.is_empty())
    {
        missing.push("Instagram (access_token/instagram_account_id/verify_token/app_secret)");
    }
    if config.line.enabled
        && (config.line.channel_access_token.is_empty() || config.line.channel_secret.is_empty())
    {
        missing.push("LINE (channel_access_token/channel_secret)");
    }
    if config.twilio.enabled
        && (config.twilio.account_sid.is_empty()
            || config.twilio.auth_token.is_empty()
            || config.twilio.from_number.is_empty()
            || config.twilio.webhook_url.is_empty())
    {
        missing.push("Twilio (account_sid/auth_token/from_number/webhook_url)");
    }
    if config.mastodon.enabled
        && (config.mastodon.instance_url.is_empty() || config.mastodon.access_token.is_empty())
    {
        missing.push("Mastodon (instance_url/access_token)");
    }
    if config.rocketchat.enabled
        && (config.rocketchat.server_url.is_empty()
            || config.rocketchat.username.is_empty()
            || config.rocketchat.password.is_empty())
    {
        missing.push("Rocket.Chat (server_url/username/password)");
    }
    if config.webchat.enabled && config.webchat.require_api_key {
        let api_key_missing = config
            .webchat
            .api_key
            .as_ref()
            .is_none_or(|k| k.trim().is_empty());
        if api_key_missing {
            missing.push("WebChat (api_key required)");
        }
    }
    if config.email.enabled
        && (config.email.imap_host.is_empty()
            || config.email.imap_username.is_empty()
            || config.email.imap_password.is_empty()
            || config.email.smtp_host.is_empty()
            || config.email.smtp_username.is_empty()
            || config.email.smtp_password.is_empty()
            || config.email.from_address.is_empty())
    {
        missing.push("Email (imap/smtp credentials and from_address)");
    }
    if config.webchat.enabled
        && config.webchat.dm_policy == crate::pairing::DmPolicy::Allowlist
        && !config.webchat.require_api_key
    {
        missing.push("WebChat (require_api_key when dm_policy=Allowlist)");
    }

    if !missing.is_empty() {
        return Some(SecurityAuditFinding {
            id: "ch-missing-tokens".into(),
            severity: SecurityAuditSeverity::High,
            title: "Missing channel credentials".into(),
            description: format!(
                "The following enabled channels are missing required credentials/secrets: {}. They will fail to authenticate, receive, or send messages.",
                missing.join(", ")
            ),
            fix_hint: Some(
                "Fill in required channel credential fields before enabling each integration."
                    .into(),
            ),
            auto_fixable: false,
        });
    }
    None
}

fn is_remote_http_url(value: &str) -> bool {
    let parsed = match Url::parse(value.trim()) {
        Ok(url) => url,
        Err(_) => return false,
    };
    if parsed.scheme() != "http" {
        return false;
    }

    let host = parsed
        .host_str()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }

    // Allow local development endpoints.
    !(host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "0.0.0.0")
}

/// Check: enabled channels should not use remote cleartext HTTP endpoints.
fn check_channel_transport_security(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    let mut insecure = Vec::new();

    if config.matrix.enabled && is_remote_http_url(&config.matrix.homeserver_url) {
        insecure.push("Matrix homeserver_url");
    }
    if config.mattermost.enabled && is_remote_http_url(&config.mattermost.server_url) {
        insecure.push("Mattermost server_url");
    }
    if config.rocketchat.enabled && is_remote_http_url(&config.rocketchat.server_url) {
        insecure.push("Rocket.Chat server_url");
    }
    if config.mastodon.enabled && is_remote_http_url(&config.mastodon.instance_url) {
        insecure.push("Mastodon instance_url");
    }
    if config.google_chat.enabled && is_remote_http_url(&config.google_chat.incoming_webhook_url) {
        insecure.push("Google Chat incoming_webhook_url");
    }
    if config.twilio.enabled && is_remote_http_url(&config.twilio.webhook_url) {
        insecure.push("Twilio webhook_url");
    }
    if config.signal.enabled && is_remote_http_url(&config.signal.api_url) {
        insecure.push("Signal api_url");
    }
    if config.bluebubbles.enabled && is_remote_http_url(&config.bluebubbles.server_url) {
        insecure.push("BlueBubbles server_url");
    }

    if insecure.is_empty() {
        return None;
    }

    Some(SecurityAuditFinding {
        id: "ch-insecure-transport".into(),
        severity: SecurityAuditSeverity::Medium,
        title: "Some channel endpoints use cleartext HTTP".into(),
        description: format!(
            "These enabled channel endpoints are configured with remote http:// URLs: {}. Tokens and message content may be exposed in transit.",
            insecure.join(", ")
        ),
        fix_hint: Some(
            "Use https:// (or wss://) endpoints for non-local channel integrations. Keep localhost-only development endpoints on isolated machines.".into(),
        ),
        auto_fixable: false,
    })
}

/// Check: completion alert broadcast has concrete channel targets.
///
/// `notify_target: all_configured` only delivers to channels that can derive concrete
/// recipient targets from config (allowlist/admin lists). Some enabled channels can
/// otherwise appear configured but still skip notification delivery.
fn check_notification_delivery_scope(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    let mut gaps = Vec::new();

    if config.telegram.enabled
        && !config.telegram.bot_token.is_empty()
        && config.telegram.allowed_chat_ids.is_empty()
        && config.telegram.admin_chat_ids.is_empty()
    {
        gaps.push("Telegram (no allowed/admin chat IDs)");
    }
    if config.discord.enabled
        && !config.discord.bot_token.is_empty()
        && config.discord.allowed_channel_ids.is_empty()
    {
        gaps.push("Discord (no allowed_channel_ids)");
    }
    if config.slack.enabled
        && !config.slack.bot_token.is_empty()
        && config.slack.allowed_channel_ids.is_empty()
    {
        gaps.push("Slack (no allowed_channel_ids)");
    }
    if config.whatsapp.enabled
        && !config.whatsapp.phone_number_id.is_empty()
        && !config.whatsapp.access_token.is_empty()
        && config.whatsapp.allowed_phone_numbers.is_empty()
        && config.whatsapp.admin_phone_numbers.is_empty()
    {
        gaps.push("WhatsApp (no allowed/admin phone numbers)");
    }
    if config.signal.enabled
        && !config.signal.api_url.is_empty()
        && !config.signal.phone_number.is_empty()
        && config.signal.allowed_numbers.is_empty()
        && config.signal.admin_numbers.is_empty()
    {
        gaps.push("Signal (no allowed/admin numbers)");
    }
    if config.matrix.enabled
        && !config.matrix.homeserver_url.is_empty()
        && !config.matrix.access_token.is_empty()
        && config.matrix.allowed_room_ids.is_empty()
    {
        gaps.push("Matrix (no allowed_room_ids)");
    }
    if config.mattermost.enabled
        && !config.mattermost.server_url.is_empty()
        && !config.mattermost.bot_token.is_empty()
        && config.mattermost.allowed_channel_ids.is_empty()
    {
        gaps.push("Mattermost (no allowed_channel_ids)");
    }
    if config.messenger.enabled
        && !config.messenger.page_access_token.is_empty()
        && config.messenger.allowed_sender_ids.is_empty()
        && config.messenger.admin_sender_ids.is_empty()
    {
        gaps.push("Messenger (no allowed/admin sender IDs)");
    }
    if config.instagram.enabled
        && !config.instagram.access_token.is_empty()
        && !config.instagram.instagram_account_id.is_empty()
        && config.instagram.allowed_sender_ids.is_empty()
        && config.instagram.admin_sender_ids.is_empty()
    {
        gaps.push("Instagram (no allowed/admin sender IDs)");
    }
    if config.line.enabled
        && !config.line.channel_access_token.is_empty()
        && config.line.allowed_user_ids.is_empty()
        && config.line.admin_user_ids.is_empty()
    {
        gaps.push("LINE (no allowed/admin user IDs)");
    }
    if config.twilio.enabled
        && !config.twilio.account_sid.is_empty()
        && !config.twilio.auth_token.is_empty()
        && !config.twilio.from_number.is_empty()
        && config.twilio.allowed_numbers.is_empty()
        && config.twilio.admin_numbers.is_empty()
    {
        gaps.push("Twilio (no allowed/admin numbers)");
    }
    if config.bluebubbles.enabled
        && !config.bluebubbles.server_url.is_empty()
        && !config.bluebubbles.password.is_empty()
    {
        gaps.push("BlueBubbles (all_configured unsupported; explicit chat_guid required)");
    }
    if config.teams.enabled
        && !config.teams.app_id.is_empty()
        && !config.teams.app_password.is_empty()
    {
        gaps.push("Microsoft Teams (notify_target unsupported)");
    }
    if config.mastodon.enabled
        && !config.mastodon.instance_url.is_empty()
        && !config.mastodon.access_token.is_empty()
    {
        gaps.push("Mastodon (notify_target unsupported)");
    }
    if config.rocketchat.enabled
        && !config.rocketchat.server_url.is_empty()
        && !config.rocketchat.username.is_empty()
        && !config.rocketchat.password.is_empty()
    {
        gaps.push("Rocket.Chat (notify_target unsupported)");
    }
    if config.webchat.enabled {
        gaps.push("WebChat (notify_target unsupported)");
    }
    if config.email.enabled {
        gaps.push("Email (notify_target unsupported)");
    }

    if gaps.is_empty() {
        return None;
    }

    Some(SecurityAuditFinding {
        id: "ch-notification-delivery-scope".into(),
        severity: SecurityAuditSeverity::Low,
        title: "Some enabled channels are not reachable via all_configured notifications".into(),
        description: format!(
            "Background/completion alerts using notify_target type=all_configured will skip these channel configs: {}.",
            gaps.join(", ")
        ),
        fix_hint: Some(
            "Populate channel allowlist/admin recipient fields, use channel-specific notify_target values where available, use explicit type=bluebubbles with chat_guid for BlueBubbles, and for currently unsupported channels (Microsoft Teams, Mastodon, Rocket.Chat, WebChat, Email) route alerts to type=gui or a supported messaging target.".into(),
        ),
        auto_fixable: false,
    })
}

/// Check: Email channel is currently a stub integration.
///
/// The current Email module can be enabled in config/UI, but inbound polling and
/// outbound send paths remain stubbed and do not route full channel interactions
/// through the same tool/approval pipeline as real-time channels.
fn check_email_channel_stub(_config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    // Email channel is now fully implemented with IMAP/SMTP, pipeline routing,
    // thread tracking, rate limiting, deduplication, and tool policy enforcement.
    None
}

/// Check: gateway bind_address is not "0.0.0.0" unless explicitly intended.
fn check_gateway_bind_address(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.gateway.enabled && config.gateway.bind_address == "0.0.0.0" {
        return Some(SecurityAuditFinding {
            id: "cfg-gateway-bind-all".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "Gateway bound to all interfaces".into(),
            description: "The WebSocket gateway bind_address is \"0.0.0.0\", exposing it on all network interfaces. This allows remote connections from any host on the network.".into(),
            fix_hint: Some("Set gateway.bind_address to \"127.0.0.1\" unless remote access is intentionally required. If remote access is needed, ensure TLS and strong auth are enabled.".into()),
            auto_fixable: true,
        });
    }
    None
}

/// Check: DeBERTa threshold is >= 0.7 to avoid excessive false negatives.
fn check_deberta_threshold(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if config.defense.enabled
        && config.defense.deberta_enabled
        && config.defense.deberta_threshold < 0.7
    {
        return Some(SecurityAuditFinding {
            id: "cfg-deberta-threshold-low".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "DeBERTa detection threshold too low".into(),
            description: format!(
                "The DeBERTa prompt injection detection threshold is {:.2}, which is below the recommended minimum of 0.70. A low threshold increases false positives but values below 0.7 may indicate misconfiguration.",
                config.defense.deberta_threshold
            ),
            fix_hint: Some("Set defense.deberta_threshold to at least 0.7 (recommended: 0.85).".into()),
            auto_fixable: true,
        });
    }
    None
}

/// Check: defense pipeline is enabled (critical — without it no content scanning occurs).
fn check_defense_pipeline_critical(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if !config.defense.enabled {
        return Some(SecurityAuditFinding {
            id: "cfg-defense-pipeline-off".into(),
            severity: SecurityAuditSeverity::Critical,
            title: "Defense pipeline is disabled".into(),
            description: "The defense pipeline is completely disabled. No prompt injection detection or content safety classification will run. This leaves the application vulnerable to adversarial inputs.".into(),
            fix_hint: Some("Set defense.enabled = true and configure at least one backend (deberta_enabled or llama_guard_enabled).".into()),
            auto_fixable: true,
        });
    }
    None
}

/// Check: sandbox blocked_paths includes sensitive system paths.
fn check_sandbox_blocked_paths(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    if !config.sandbox.enabled {
        return None;
    }

    let required_paths = ["/etc", "/proc", "/sys", "/var/run/docker.sock"];

    let missing: Vec<&str> = required_paths
        .iter()
        .filter(|p| !config.sandbox.blocked_paths.iter().any(|bp| bp == **p))
        .copied()
        .collect();

    if !missing.is_empty() {
        return Some(SecurityAuditFinding {
            id: "cfg-sandbox-blocked-paths".into(),
            severity: SecurityAuditSeverity::High,
            title: "Sandbox missing critical blocked paths".into(),
            description: format!(
                "The sandbox is enabled but its blocked_paths list is missing sensitive system paths: {}. Containers may access host system resources.",
                missing.join(", ")
            ),
            fix_hint: Some(format!(
                "Add the following to sandbox.blocked_paths: {}",
                missing.join(", ")
            )),
            auto_fixable: false,
        });
    }
    None
}

/// Check: no config secrets are stored in plaintext (should use keyring placeholders).
fn check_plaintext_secrets(config: &NexiBotConfig) -> Option<SecurityAuditFinding> {
    use crate::security::credentials::KEYRING_PLACEHOLDER;

    fn mark_plaintext(
        plaintext_fields: &mut Vec<&'static str>,
        field: &'static str,
        value: &str,
        enabled: bool,
    ) {
        if enabled && !value.is_empty() && value != KEYRING_PLACEHOLDER {
            plaintext_fields.push(field);
        }
    }

    let mut plaintext_fields = Vec::new();

    // Check each secret field: if present, non-empty, and not the keyring placeholder,
    // it's stored in plaintext.
    if let Some(ref key) = config.claude.api_key {
        if !key.is_empty() && key != KEYRING_PLACEHOLDER {
            plaintext_fields.push("claude.api_key");
        }
    }
    if let Some(ref key) = config.openai.api_key {
        if !key.is_empty() && key != KEYRING_PLACEHOLDER {
            plaintext_fields.push("openai.api_key");
        }
    }
    if let Some(ref key) = config.cerebras.api_key {
        if !key.is_empty() && key != KEYRING_PLACEHOLDER {
            plaintext_fields.push("cerebras.api_key");
        }
    }
    if let Some(ref google) = config.google {
        if let Some(ref key) = google.api_key {
            if !key.is_empty() && key != KEYRING_PLACEHOLDER {
                plaintext_fields.push("google.api_key");
            }
        }
    }
    if let Some(ref deepseek) = config.deepseek {
        if let Some(ref key) = deepseek.api_key {
            if !key.is_empty() && key != KEYRING_PLACEHOLDER {
                plaintext_fields.push("deepseek.api_key");
            }
        }
    }
    if let Some(ref github_copilot) = config.github_copilot {
        if let Some(ref token) = github_copilot.token {
            if !token.is_empty() && token != KEYRING_PLACEHOLDER {
                plaintext_fields.push("github_copilot.token");
            }
        }
    }
    if let Some(ref minimax) = config.minimax {
        if let Some(ref key) = minimax.api_key {
            if !key.is_empty() && key != KEYRING_PLACEHOLDER {
                plaintext_fields.push("minimax.api_key");
            }
        }
    }
    if let Some(ref private_key) = config.k2k.private_key_pem {
        if !private_key.is_empty() && private_key != KEYRING_PLACEHOLDER {
            plaintext_fields.push("k2k.private_key_pem");
        }
    }
    if let Some(ref key) = config.search.brave_api_key {
        if !key.is_empty() && key != KEYRING_PLACEHOLDER {
            plaintext_fields.push("search.brave_api_key");
        }
    }
    if let Some(ref key) = config.search.tavily_api_key {
        if !key.is_empty() && key != KEYRING_PLACEHOLDER {
            plaintext_fields.push("search.tavily_api_key");
        }
    }
    if let Some(ref key) = config.stt.deepgram_api_key {
        if !key.is_empty() && key != KEYRING_PLACEHOLDER {
            plaintext_fields.push("stt.deepgram_api_key");
        }
    }
    if let Some(ref key) = config.stt.openai_api_key {
        if !key.is_empty() && key != KEYRING_PLACEHOLDER {
            plaintext_fields.push("stt.openai_api_key");
        }
    }
    if let Some(ref key) = config.tts.elevenlabs_api_key {
        if !key.is_empty() && key != KEYRING_PLACEHOLDER {
            plaintext_fields.push("tts.elevenlabs_api_key");
        }
    }
    if let Some(ref key) = config.tts.cartesia_api_key {
        if !key.is_empty() && key != KEYRING_PLACEHOLDER {
            plaintext_fields.push("tts.cartesia_api_key");
        }
    }
    mark_plaintext(
        &mut plaintext_fields,
        "telegram.bot_token",
        &config.telegram.bot_token,
        config.telegram.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "whatsapp.access_token",
        &config.whatsapp.access_token,
        config.whatsapp.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "whatsapp.verify_token",
        &config.whatsapp.verify_token,
        config.whatsapp.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "whatsapp.app_secret",
        &config.whatsapp.app_secret,
        config.whatsapp.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "discord.bot_token",
        &config.discord.bot_token,
        config.discord.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "slack.bot_token",
        &config.slack.bot_token,
        config.slack.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "slack.app_token",
        &config.slack.app_token,
        config.slack.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "slack.signing_secret",
        &config.slack.signing_secret,
        config.slack.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "teams.app_password",
        &config.teams.app_password,
        config.teams.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "matrix.access_token",
        &config.matrix.access_token,
        config.matrix.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "bluebubbles.password",
        &config.bluebubbles.password,
        config.bluebubbles.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "google_chat.verification_token",
        &config.google_chat.verification_token,
        config.google_chat.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "google_chat.incoming_webhook_url",
        &config.google_chat.incoming_webhook_url,
        config.google_chat.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "mattermost.bot_token",
        &config.mattermost.bot_token,
        config.mattermost.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "messenger.page_access_token",
        &config.messenger.page_access_token,
        config.messenger.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "messenger.verify_token",
        &config.messenger.verify_token,
        config.messenger.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "messenger.app_secret",
        &config.messenger.app_secret,
        config.messenger.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "instagram.access_token",
        &config.instagram.access_token,
        config.instagram.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "instagram.verify_token",
        &config.instagram.verify_token,
        config.instagram.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "instagram.app_secret",
        &config.instagram.app_secret,
        config.instagram.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "line.channel_access_token",
        &config.line.channel_access_token,
        config.line.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "line.channel_secret",
        &config.line.channel_secret,
        config.line.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "twilio.auth_token",
        &config.twilio.auth_token,
        config.twilio.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "mastodon.access_token",
        &config.mastodon.access_token,
        config.mastodon.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "rocketchat.password",
        &config.rocketchat.password,
        config.rocketchat.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "email.imap_password",
        &config.email.imap_password,
        config.email.enabled,
    );
    mark_plaintext(
        &mut plaintext_fields,
        "email.smtp_password",
        &config.email.smtp_password,
        config.email.enabled,
    );
    if config.webchat.enabled {
        if let Some(ref api_key) = config.webchat.api_key {
            if !api_key.is_empty() && api_key != KEYRING_PLACEHOLDER {
                plaintext_fields.push("webchat.api_key");
            }
        }
    }
    if let Some(ref token) = config.webhooks.auth_token {
        if !token.is_empty() && token != KEYRING_PLACEHOLDER && config.webhooks.enabled {
            plaintext_fields.push("webhooks.auth_token");
        }
    }

    if plaintext_fields.is_empty() {
        return None;
    }

    let keyring_available = crate::security::credentials::is_keyring_available();
    let (description, fix_hint) = if keyring_available {
        (
            format!(
                "The following secrets appear to be stored in plaintext in the config file rather than the OS keyring: {}. If the config file is compromised, these secrets are exposed.",
                plaintext_fields.join(", ")
            ),
            "Use the credentials manager to migrate secrets to the OS keyring. Plaintext values in config should be replaced with the \"__keyring__\" placeholder.".to_string(),
        )
    } else {
        (
            format!(
                "The following secrets are stored in plaintext in the config file: {}. Keyring-backed secret storage is currently unavailable in this build, so config file compromise exposes these values directly.",
                plaintext_fields.join(", ")
            ),
            "Restrict config file permissions (0600), use disk encryption, and rotate these tokens regularly. Re-enable keyring-backed secret storage before using \"__keyring__\" placeholders.".to_string(),
        )
    };

    Some(SecurityAuditFinding {
        id: "cfg-plaintext-secrets".into(),
        severity: SecurityAuditSeverity::High,
        title: "Secrets stored in plaintext config".into(),
        description,
        fix_hint: Some(fix_hint),
        auto_fixable: false,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the config directory path.
fn config_dir_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("ai", "nexibot", "desktop")
        .map(|p| p.config_dir().to_path_buf())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run a full security audit against the provided configuration.
///
/// Each check is executed and findings are collected. The report includes
/// both the findings and pass/fail statistics.
pub fn run_full_audit(config: &NexiBotConfig) -> SecurityAuditReport {
    info!("[AUDIT] Starting full security audit");

    // Collect all checks into a vec so we can count them.
    let checks: Vec<Option<SecurityAuditFinding>> = vec![
        // Config checks
        check_api_key_presence(config),
        check_defense_enabled(config),
        check_dm_policy(config),
        check_ssrf_policy(config),
        check_execute_tool_safety(config),
        check_guardrails_level(config),
        check_defense_fail_open(config),
        check_prompt_injection_blocking(config),
        // Filesystem checks
        check_config_file_permissions(),
        check_sessions_dir_permissions(),
        check_credential_storage(),
        // Runtime checks
        check_defense_models_loaded(&config.defense),
        check_session_encryption(),
        check_workspace_confinement(config),
        // Channel checks
        check_webhook_auth(config),
        check_channel_ingress_scope(config),
        check_admin_lists(config),
        check_telegram_group_admin_ids(config),
        check_slack_admin_ids(config),
        check_matrix_admin_ids(config),
        check_signal_admin_numbers(config),
        check_email_admin_addresses(config),
        check_confirmation_acceptance_path(config),
        check_background_confirmation_path(config),
        check_pairing_enabled(config),
        check_channel_tokens(config),
        check_channel_transport_security(config),
        check_notification_delivery_scope(config),
        check_email_channel_stub(config),
        // Execution checks
        check_skill_exec_disabled(config),
        // Enhanced security checks
        check_gateway_bind_address(config),
        check_deberta_threshold(config),
        check_defense_pipeline_critical(config),
        check_sandbox_blocked_paths(config),
        check_plaintext_secrets(config),
    ];

    let mut total_checks = checks.len();
    let mut findings: Vec<SecurityAuditFinding> = checks.into_iter().flatten().collect();

    // Check 20: Cloud-sync folder detection
    total_checks += 1;
    {
        let home = dirs::home_dir().unwrap_or_default();
        let config_dir = config_dir_path().unwrap_or_else(|| home.join(".config/nexibot"));
        let mut sync_markers = vec![
            (home.join("Dropbox"), "Dropbox"),
            (home.join("OneDrive"), "OneDrive"),
            (home.join("Google Drive"), "Google Drive"),
        ];
        // iCloud Drive is macOS-only
        #[cfg(target_os = "macos")]
        sync_markers.push((home.join("Library/Mobile Documents"), "iCloud Drive"));
        for (sync_path, service) in &sync_markers {
            if sync_path.exists() {
                if let Ok(canon_config) = config_dir.canonicalize() {
                    if let Ok(canon_sync) = sync_path.canonicalize() {
                        if canon_config.starts_with(&canon_sync) {
                            findings.push(SecurityAuditFinding {
                                id: format!("cfg-cloud-sync-{}", service.to_lowercase().replace(' ', "-")),
                                title: format!("Config directory is inside {} sync folder", service),
                                severity: SecurityAuditSeverity::High,
                                description: format!(
                                    "NexiBot config at {:?} is inside {} at {:?}. Secrets may be synced to cloud.",
                                    config_dir, service, sync_path
                                ),
                                fix_hint: Some("Move NexiBot config outside of cloud-synced directories.".into()),
                                auto_fixable: false,
                            });
                        }
                    }
                }
            }
        }
    }

    // Check 21: Model hygiene
    total_checks += 1;
    {
        let model: &str = &config.claude.model;
        let legacy_models = ["gpt-3.5", "claude-2", "claude-instant", "text-davinci"];
        for legacy in &legacy_models {
            if model.contains(legacy) {
                findings.push(SecurityAuditFinding {
                    id: "cfg-legacy-model".to_string(),
                    title: "Legacy model configured".to_string(),
                    severity: SecurityAuditSeverity::Medium,
                    description: format!(
                        "Model '{}' is a legacy model with weaker instruction following. Consider upgrading.",
                        model
                    ),
                    fix_hint: Some("Set claude.model to a current model (claude-sonnet-4-5-20250929 or newer).".into()),
                    auto_fixable: false,
                });
                break;
            }
        }
    }

    let passed_count = total_checks - findings.len();

    info!(
        "[AUDIT] Audit complete: {}/{} checks passed, {} findings",
        passed_count,
        total_checks,
        findings.len()
    );

    for finding in &findings {
        match finding.severity {
            SecurityAuditSeverity::Critical | SecurityAuditSeverity::High => {
                warn!(
                    "[AUDIT] {} [{}]: {}",
                    finding.severity, finding.id, finding.title
                );
            }
            _ => {
                debug!(
                    "[AUDIT] {} [{}]: {}",
                    finding.severity, finding.id, finding.title
                );
            }
        }
    }

    SecurityAuditReport {
        findings,
        passed_count,
        total_checks,
        timestamp: Utc::now(),
    }
}

/// Attempt to auto-fix a finding by its ID.
///
/// Only findings marked `auto_fixable = true` can be fixed. Returns a
/// `FixResult` describing whether the fix succeeded.
pub fn auto_fix(finding: &SecurityAuditFinding) -> FixResult {
    if !finding.auto_fixable {
        return FixResult {
            finding_id: finding.id.clone(),
            success: false,
            message: format!("Finding '{}' is not auto-fixable.", finding.id),
        };
    }

    info!("[AUDIT] Attempting auto-fix for finding: {}", finding.id);

    match finding.id.as_str() {
        "fs-config-perms" => fix_config_file_permissions(),
        "fs-sessions-perms" => fix_sessions_dir_permissions(),
        "cfg-defense-disabled" => fix_config_value(&finding.id, "defense.enabled", "true"),
        "cfg-defense-fail-open" => fix_config_value(&finding.id, "defense.fail_open", "false"),
        "cfg-execute-no-dcg" => fix_config_value(&finding.id, "execute.use_dcg", "true"),
        "cfg-ssrf-blocked-domains" => fix_config_value(
            &finding.id,
            "fetch.blocked_domains",
            "restored to defaults (localhost, 127.0.0.1, 0.0.0.0, 169.254.169.254, [::1])",
        ),
        "rt-workspace-confinement" => fix_config_value(
            &finding.id,
            "filesystem.blocked_paths",
            "restored to defaults (/etc, /System, /usr, /var, /bin, /sbin)",
        ),
        "cfg-prompt-injection-warn-only" => {
            fix_config_value(&finding.id, "guardrails.block_prompt_injection", "true")
        }
        "cfg-gateway-bind-all" => {
            fix_config_value(&finding.id, "gateway.bind_address", "127.0.0.1")
        }
        "cfg-deberta-threshold-low" => {
            fix_config_value(&finding.id, "defense.deberta_threshold", "0.85")
        }
        "cfg-defense-pipeline-off" => fix_config_value(&finding.id, "defense.enabled", "true"),
        _ => FixResult {
            finding_id: finding.id.clone(),
            success: false,
            message: format!("No auto-fix handler for finding '{}'.", finding.id),
        },
    }
}

/// Fix config file permissions (cross-platform).
fn fix_config_file_permissions() -> FixResult {
    let config_path = match NexiBotConfig::config_path() {
        Ok(p) => p,
        Err(e) => {
            return FixResult {
                finding_id: "fs-config-perms".into(),
                success: false,
                message: format!("Could not determine config path: {}", e),
            };
        }
    };

    match crate::platform::file_security::restrict_file_permissions(&config_path) {
        Ok(()) => {
            info!(
                "[AUDIT] Fixed config file permissions: {}",
                config_path.display()
            );
            FixResult {
                finding_id: "fs-config-perms".into(),
                success: true,
                message: format!("Restricted permissions on {}.", config_path.display()),
            }
        }
        Err(e) => FixResult {
            finding_id: "fs-config-perms".into(),
            success: false,
            message: format!("Failed to restrict {}: {}", config_path.display(), e),
        },
    }
}

/// Fix sessions directory permissions (cross-platform).
fn fix_sessions_dir_permissions() -> FixResult {
    let sessions_dir = match config_dir_path() {
        Some(d) => d.join("sessions"),
        None => {
            return FixResult {
                finding_id: "fs-sessions-perms".into(),
                success: false,
                message: "Could not determine config directory.".into(),
            };
        }
    };

    if !sessions_dir.exists() {
        return FixResult {
            finding_id: "fs-sessions-perms".into(),
            success: false,
            message: "Sessions directory does not exist.".into(),
        };
    }

    match crate::platform::file_security::restrict_dir_permissions(&sessions_dir) {
        Ok(()) => {
            info!(
                "[AUDIT] Fixed sessions dir permissions: {}",
                sessions_dir.display()
            );
            FixResult {
                finding_id: "fs-sessions-perms".into(),
                success: true,
                message: format!("Restricted permissions on {}.", sessions_dir.display()),
            }
        }
        Err(e) => FixResult {
            finding_id: "fs-sessions-perms".into(),
            success: false,
            message: format!("Failed to restrict {}: {}", sessions_dir.display(), e),
        },
    }
}

/// Auto-fix a config value by loading, mutating, and re-saving the YAML config.
fn fix_config_value(finding_id: &str, key: &str, value: &str) -> FixResult {
    info!("[AUDIT] Auto-fixing: set {} = {}", key, value);

    let config_path = match NexiBotConfig::config_path() {
        Ok(p) => p,
        Err(e) => {
            return FixResult {
                finding_id: finding_id.into(),
                success: false,
                message: format!("Could not determine config path: {}", e),
            };
        }
    };

    if !config_path.exists() {
        return FixResult {
            finding_id: finding_id.into(),
            success: false,
            message: format!("Config file not found at {:?}", config_path),
        };
    }

    match std::fs::read_to_string(&config_path) {
        Ok(content) => {
            match serde_yml::from_str::<serde_yml::Value>(&content) {
                Ok(mut yaml_value) => {
                    // Navigate the key path (e.g., "defense.enabled" -> defense -> enabled)
                    let parts: Vec<&str> = key.split('.').collect();
                    let mut current = &mut yaml_value;

                    for (i, part) in parts.iter().enumerate() {
                        if i == parts.len() - 1 {
                            // Set the value
                            if let serde_yml::Value::Mapping(map) = current {
                                let parsed_value = match value {
                                    "true" => serde_yml::Value::Bool(true),
                                    "false" => serde_yml::Value::Bool(false),
                                    _ => serde_yml::Value::String(value.to_string()),
                                };
                                map.insert(
                                    serde_yml::Value::String(part.to_string()),
                                    parsed_value,
                                );
                            }
                        } else {
                            if let serde_yml::Value::Mapping(map) = current {
                                current = map
                                    .entry(serde_yml::Value::String(part.to_string()))
                                    .or_insert(
                                        serde_yml::Value::Mapping(serde_yml::Mapping::new()),
                                    );
                            }
                        }
                    }

                    match serde_yml::to_string(&yaml_value) {
                        Ok(new_content) => match std::fs::write(&config_path, &new_content) {
                            Ok(()) => FixResult {
                                finding_id: finding_id.into(),
                                success: true,
                                message: format!("Set {} = {} in config.yaml", key, value),
                            },
                            Err(e) => FixResult {
                                finding_id: finding_id.into(),
                                success: false,
                                message: format!("Failed to write config: {}", e),
                            },
                        },
                        Err(e) => FixResult {
                            finding_id: finding_id.into(),
                            success: false,
                            message: format!("Failed to serialize YAML: {}", e),
                        },
                    }
                }
                Err(e) => FixResult {
                    finding_id: finding_id.into(),
                    success: false,
                    message: format!("Failed to parse config YAML: {}", e),
                },
            }
        }
        Err(e) => FixResult {
            finding_id: finding_id.into(),
            success: false,
            message: format!("Failed to read config: {}", e),
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal default config for testing.
    fn test_config() -> NexiBotConfig {
        NexiBotConfig::default()
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(SecurityAuditSeverity::Critical.to_string(), "Critical");
        assert_eq!(SecurityAuditSeverity::Info.to_string(), "Info");
    }

    #[test]
    fn test_severity_ordering() {
        assert!(SecurityAuditSeverity::Critical > SecurityAuditSeverity::High);
        assert!(SecurityAuditSeverity::High > SecurityAuditSeverity::Medium);
        assert!(SecurityAuditSeverity::Medium > SecurityAuditSeverity::Low);
        assert!(SecurityAuditSeverity::Low > SecurityAuditSeverity::Info);
    }

    #[test]
    fn test_check_api_key_present() {
        let mut config = test_config();
        // Default config has no key set, should flag.
        let finding = check_api_key_presence(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "cfg-api-key");
        assert_eq!(
            finding.as_ref().unwrap().severity,
            SecurityAuditSeverity::Critical
        );

        // Set a claude key, should pass.
        config.claude.api_key = Some("sk-ant-test123".into());
        assert!(check_api_key_presence(&config).is_none());
    }

    #[test]
    fn test_check_api_key_present_for_alternative_providers() {
        let mut config = test_config();

        config.claude.api_key = None;
        config.openai.api_key = None;
        config.google = None;
        config.cerebras.api_key = Some("csk-test-key".into());
        assert!(check_api_key_presence(&config).is_none());

        config.cerebras.api_key = None;
        config.deepseek = Some(crate::config::DeepSeekConfig {
            api_key: Some("deepseek-test-key".into()),
            api_url: "https://api.deepseek.com/v1".into(),
            default_model: "deepseek-chat".into(),
        });
        assert!(check_api_key_presence(&config).is_none());

        config.deepseek = None;
        config.ollama.enabled = true;
        assert!(check_api_key_presence(&config).is_none());
    }

    #[test]
    fn test_check_defense_enabled() {
        let mut config = test_config();
        config.defense.enabled = true;
        assert!(check_defense_enabled(&config).is_none());

        config.defense.enabled = false;
        let finding = check_defense_enabled(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "cfg-defense-disabled");
    }

    #[test]
    fn test_check_ssrf_policy() {
        let mut config = test_config();
        // Default has blocked domains, should pass.
        assert!(check_ssrf_policy(&config).is_none());

        // Clear blocked domains when fetch is enabled.
        config.fetch.blocked_domains = Vec::new();
        let finding = check_ssrf_policy(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "cfg-ssrf-blocked-domains");
    }

    #[test]
    fn test_check_execute_tool_safety() {
        let mut config = test_config();
        // Default execute is disabled, should pass.
        assert!(check_execute_tool_safety(&config).is_none());

        config.execute.enabled = true;
        config.execute.use_dcg = false;
        let finding = check_execute_tool_safety(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "cfg-execute-no-dcg");
    }

    #[test]
    fn test_check_guardrails_level() {
        let mut config = test_config();
        assert!(check_guardrails_level(&config).is_none());

        config.guardrails.security_level = crate::guardrails::SecurityLevel::Disabled;
        let finding = check_guardrails_level(&config);
        assert!(finding.is_some());
        assert_eq!(
            finding.as_ref().unwrap().severity,
            SecurityAuditSeverity::Critical
        );
    }

    #[test]
    fn test_check_webhook_auth_disabled_server() {
        let config = test_config();
        // Webhooks not enabled by default, should pass.
        assert!(check_webhook_auth(&config).is_none());
    }

    #[test]
    fn test_check_webhook_auth_enabled_no_token() {
        let mut config = test_config();
        config.webhooks.enabled = true;
        config.webhooks.auth_token = None;
        let finding = check_webhook_auth(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "ch-webhook-auth");
    }

    #[test]
    fn test_check_channel_tokens_detects_mattermost_credentials() {
        let mut config = test_config();
        config.mattermost.enabled = true;
        let finding = check_channel_tokens(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("Mattermost (server_url/bot_token)"));
    }

    #[test]
    fn test_check_channel_tokens_detects_bluebubbles_password() {
        let mut config = test_config();
        config.bluebubbles.enabled = true;
        config.bluebubbles.server_url = "http://localhost:1234".into();
        config.bluebubbles.password = String::new();

        let finding = check_channel_tokens(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("BlueBubbles (server_url/password)"));

        config.bluebubbles.password = "bb-password".into();
        assert!(check_channel_tokens(&config).is_none());
    }

    #[test]
    fn test_check_channel_ingress_scope_detects_teams_empty_allowlist() {
        let mut config = test_config();
        config.teams.enabled = true;
        config.teams.allowed_team_ids = Vec::new();
        let finding = check_channel_ingress_scope(&config);
        assert!(finding.is_some());
        assert!(finding.as_ref().unwrap().description.contains("Teams"));

        config.teams.allowed_team_ids = vec!["tenant-1".to_string()];
        assert!(check_channel_ingress_scope(&config).is_none());
    }

    #[test]
    fn test_check_dm_policy_discord_allowlist_with_admin_is_not_flagged() {
        let mut config = test_config();
        config.discord.enabled = true;
        config.discord.dm_policy = crate::pairing::DmPolicy::Allowlist;
        config.discord.allowed_guild_ids = Vec::new();
        config.discord.allowed_channel_ids = Vec::new();
        config.discord.admin_user_ids = vec![123_456_789];

        assert!(check_dm_policy(&config).is_none());
    }

    #[test]
    fn test_check_dm_policy_slack_allowlist_with_admin_is_not_flagged() {
        let mut config = test_config();
        config.slack.enabled = true;
        config.slack.dm_policy = crate::pairing::DmPolicy::Allowlist;
        config.slack.allowed_channel_ids = Vec::new();
        config.slack.admin_user_ids = vec!["U123456".to_string()];

        assert!(check_dm_policy(&config).is_none());
    }

    #[test]
    fn test_check_channel_ingress_scope_flags_open_discord_and_slack() {
        let mut config = test_config();
        config.discord.enabled = true;
        config.slack.enabled = true;

        let finding = check_channel_ingress_scope(&config);
        assert!(finding.is_some());
        let finding = finding.unwrap();
        assert_eq!(finding.id, "ch-ingress-open");
        assert!(finding.description.contains("Discord"));
        assert!(finding.description.contains("Slack"));
    }

    #[test]
    fn test_check_channel_ingress_scope_flags_open_matrix() {
        let mut config = test_config();
        config.matrix.enabled = true;
        config.matrix.allowed_room_ids = Vec::new();

        let finding = check_channel_ingress_scope(&config);
        assert!(finding.is_some());
        let finding = finding.unwrap();
        assert_eq!(finding.id, "ch-ingress-open");
        assert!(finding.description.contains("Matrix"));
    }

    #[test]
    fn test_check_channel_ingress_scope_flags_google_chat_mattermost_and_rocketchat() {
        let mut config = test_config();
        config.google_chat.enabled = true;
        config.google_chat.allowed_spaces = Vec::new();
        config.mattermost.enabled = true;
        config.mattermost.allowed_channel_ids = Vec::new();
        config.rocketchat.enabled = true;
        config.rocketchat.allowed_room_ids = Vec::new();

        let finding = check_channel_ingress_scope(&config);
        assert!(finding.is_some());
        let finding = finding.unwrap();
        assert_eq!(finding.id, "ch-ingress-open");
        assert!(finding.description.contains("Google Chat"));
        assert!(finding.description.contains("Mattermost"));
        assert!(finding.description.contains("Rocket.Chat"));

        config.google_chat.allowed_spaces = vec!["spaces/AAA".to_string()];
        config.mattermost.allowed_channel_ids = vec!["chan-1".to_string()];
        config.rocketchat.allowed_room_ids = vec!["room-1".to_string()];
        assert!(check_channel_ingress_scope(&config).is_none());
    }

    #[test]
    fn test_check_channel_ingress_scope_flags_email_without_allowed_senders() {
        let mut config = test_config();
        config.email.enabled = true;
        config.email.allowed_senders = Vec::new();

        let finding = check_channel_ingress_scope(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("Email (all senders)"));

        config.email.allowed_senders = vec!["trusted@example.com".to_string()];
        assert!(check_channel_ingress_scope(&config).is_none());
    }

    #[test]
    fn test_check_channel_ingress_scope_respects_allowlists() {
        let mut config = test_config();
        config.discord.enabled = true;
        config.discord.allowed_guild_ids = vec![123_456];
        config.slack.enabled = true;
        config.slack.allowed_channel_ids = vec!["C123".to_string()];

        assert!(check_channel_ingress_scope(&config).is_none());
    }

    #[test]
    fn test_check_confirmation_acceptance_path_supported_channels_are_not_flagged() {
        let mut config = test_config();
        config.webchat.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.webchat.enabled = false;
        config.bluebubbles.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.bluebubbles.enabled = false;
        config.signal.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.signal.enabled = false;
        config.whatsapp.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.whatsapp.enabled = false;
        config.teams.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.teams.enabled = false;
        config.matrix.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.matrix.enabled = false;
        config.mattermost.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.mattermost.enabled = false;
        config.google_chat.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.google_chat.enabled = false;
        config.twilio.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.twilio.enabled = false;
        config.messenger.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.messenger.enabled = false;
        config.instagram.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.instagram.enabled = false;
        config.line.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.line.enabled = false;
        config.mastodon.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());

        config.mastodon.enabled = false;
        config.rocketchat.enabled = true;
        assert!(check_confirmation_acceptance_path(&config).is_none());
    }

    #[test]
    fn test_check_confirmation_acceptance_path_flags_email_channel() {
        let mut config = test_config();
        config.email.enabled = true;
        config.guardrails.confirm_external_actions = true;
        config.browser.enabled = false;
        config.computer_use.enabled = false;
        config.autonomous_mode.enabled = false;

        let finding = check_confirmation_acceptance_path(&config)
            .expect("email should be flagged without in-channel approval acceptance");
        assert_eq!(finding.id, "ch-confirmation-acceptance-path");
        assert!(finding.description.contains("Email"));
    }

    #[test]
    fn test_check_confirmation_acceptance_path_respects_global_toggle() {
        let mut config = test_config();
        config.teams.enabled = true;
        config.guardrails.confirm_external_actions = false;
        config.browser.enabled = false;
        config.computer_use.enabled = false;
        assert!(check_confirmation_acceptance_path(&config).is_none());
    }

    #[test]
    fn test_check_background_confirmation_path_webchat_is_not_flagged() {
        let mut config = test_config();
        config.webchat.enabled = true;
        config.guardrails.confirm_external_actions = true;
        config.browser.enabled = false;
        config.computer_use.enabled = false;
        config.autonomous_mode.enabled = false;

        assert!(check_background_confirmation_path(&config).is_none());
    }

    #[test]
    fn test_check_background_confirmation_path_telegram_is_not_flagged() {
        let mut config = test_config();
        config.telegram.enabled = true;
        config.guardrails.confirm_external_actions = true;
        config.browser.enabled = false;
        config.computer_use.enabled = false;
        config.autonomous_mode.enabled = false;

        assert!(check_background_confirmation_path(&config).is_none());
    }

    #[test]
    fn test_check_background_confirmation_path_discord_slack_signal_whatsapp_teams_matrix_bluebubbles_mattermost_google_chat_twilio_messenger_instagram_line_mastodon_rocketchat_webchat_are_not_flagged(
    ) {
        let mut config = test_config();
        config.discord.enabled = true;
        config.slack.enabled = true;
        config.signal.enabled = true;
        config.whatsapp.enabled = true;
        config.teams.enabled = true;
        config.matrix.enabled = true;
        config.bluebubbles.enabled = true;
        config.mattermost.enabled = true;
        config.google_chat.enabled = true;
        config.twilio.enabled = true;
        config.messenger.enabled = true;
        config.instagram.enabled = true;
        config.line.enabled = true;
        config.mastodon.enabled = true;
        config.rocketchat.enabled = true;
        config.webchat.enabled = true;
        config.guardrails.confirm_external_actions = true;
        config.browser.enabled = false;
        config.computer_use.enabled = false;
        config.autonomous_mode.enabled = false;

        assert!(check_background_confirmation_path(&config).is_none());
    }

    #[test]
    fn test_check_background_confirmation_path_flags_email_channel() {
        let mut config = test_config();
        config.email.enabled = true;
        config.guardrails.confirm_external_actions = true;
        config.browser.enabled = false;
        config.computer_use.enabled = false;
        config.autonomous_mode.enabled = false;

        let finding = check_background_confirmation_path(&config)
            .expect("email should be flagged for GUI-dependent background confirmations");
        assert_eq!(finding.id, "bg-confirmation-gui-dependency");
        assert!(finding.description.contains("Email"));
    }

    #[test]
    fn test_check_background_confirmation_path_respects_disabled_confirmation() {
        let mut config = test_config();
        config.telegram.enabled = true;
        config.guardrails.confirm_external_actions = false;
        config.browser.enabled = false;
        config.computer_use.enabled = false;
        config.autonomous_mode.enabled = false;

        assert!(check_background_confirmation_path(&config).is_none());
    }

    #[test]
    fn test_check_telegram_group_admin_ids_flags_negative_ids() {
        let mut config = test_config();
        config.telegram.enabled = true;
        config.telegram.admin_chat_ids = vec![-100_987_654_321, 123_456_789];

        let finding = check_telegram_group_admin_ids(&config);
        assert!(finding.is_some());
        let finding = finding.unwrap();
        assert_eq!(finding.id, "ch-telegram-group-admin");
        assert!(finding.description.contains("-100987654321"));
    }

    #[test]
    fn test_check_telegram_group_admin_ids_allows_private_chat_ids() {
        let mut config = test_config();
        config.telegram.enabled = true;
        config.telegram.admin_chat_ids = vec![123_456_789];

        assert!(check_telegram_group_admin_ids(&config).is_none());
    }

    #[test]
    fn test_check_confirmation_acceptance_path_autonomy_askuser_webchat_is_not_flagged() {
        use crate::config::AutonomyLevel;

        let mut config = test_config();
        config.webchat.enabled = true;
        config.guardrails.confirm_external_actions = false;
        config.autonomous_mode.enabled = true;
        config.autonomous_mode.execute.run_command = AutonomyLevel::AskUser;

        assert!(check_confirmation_acceptance_path(&config).is_none());
    }

    #[test]
    fn test_check_confirmation_acceptance_path_browser_confirmation_webchat_is_not_flagged() {
        let mut config = test_config();
        config.webchat.enabled = true;
        config.guardrails.confirm_external_actions = false;
        config.autonomous_mode.enabled = false;
        config.browser.enabled = true;
        config.browser.require_confirmation = true;
        config.computer_use.enabled = false;

        assert!(check_confirmation_acceptance_path(&config).is_none());
    }

    #[test]
    fn test_check_confirmation_acceptance_path_computer_use_webchat_is_not_flagged() {
        let mut config = test_config();
        config.webchat.enabled = true;
        config.guardrails.confirm_external_actions = false;
        config.autonomous_mode.enabled = false;
        config.browser.enabled = false;
        config.computer_use.enabled = true;
        config.computer_use.require_confirmation = true;

        assert!(check_confirmation_acceptance_path(&config).is_none());
    }

    #[test]
    fn test_check_channel_tokens_detects_whatsapp_verification_secrets() {
        let mut config = test_config();
        config.whatsapp.enabled = true;
        config.whatsapp.phone_number_id = "12345678".into();
        config.whatsapp.access_token = "EAAx".into();
        config.whatsapp.verify_token = String::new();
        config.whatsapp.app_secret = String::new();
        let finding = check_channel_tokens(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("WhatsApp (phone_number_id/access_token/verify_token/app_secret)"));

        config.whatsapp.verify_token = "verify-token".into();
        config.whatsapp.app_secret = "app-secret".into();
        assert!(check_channel_tokens(&config).is_none());
    }

    #[test]
    fn test_check_channel_tokens_detects_messenger_verification_secrets() {
        let mut config = test_config();
        config.messenger.enabled = true;
        config.messenger.page_access_token = "EAAB...".into();
        config.messenger.verify_token = String::new();
        config.messenger.app_secret = String::new();
        let finding = check_channel_tokens(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("Messenger (page_access_token/verify_token/app_secret)"));

        config.messenger.verify_token = "verify-token".into();
        config.messenger.app_secret = "app-secret".into();
        assert!(check_channel_tokens(&config).is_none());
    }

    #[test]
    fn test_check_channel_tokens_detects_instagram_verification_secrets() {
        let mut config = test_config();
        config.instagram.enabled = true;
        config.instagram.access_token = "IGAA...".into();
        config.instagram.instagram_account_id = "17841400000000000".into();
        config.instagram.verify_token = String::new();
        config.instagram.app_secret = String::new();
        let finding = check_channel_tokens(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("Instagram (access_token/instagram_account_id/verify_token/app_secret)"));

        config.instagram.verify_token = "verify-token".into();
        config.instagram.app_secret = "app-secret".into();
        assert!(check_channel_tokens(&config).is_none());
    }

    #[test]
    fn test_check_channel_tokens_webchat_api_key_required() {
        let mut config = test_config();
        config.webchat.enabled = true;
        config.webchat.require_api_key = true;
        config.webchat.api_key = None;
        let finding = check_channel_tokens(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("WebChat (api_key required)"));

        config.webchat.api_key = Some("test-key".to_string());
        assert!(check_channel_tokens(&config).is_none());
    }

    #[test]
    fn test_check_channel_tokens_webchat_allowlist_requires_api_key() {
        let mut config = test_config();
        config.webchat.enabled = true;
        config.webchat.dm_policy = crate::pairing::DmPolicy::Allowlist;
        config.webchat.require_api_key = false;
        let finding = check_channel_tokens(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("WebChat (require_api_key when dm_policy=Allowlist)"));

        config.webchat.require_api_key = true;
        config.webchat.api_key = Some("test-key".to_string());
        assert!(check_channel_tokens(&config).is_none());
    }

    #[test]
    fn test_check_channel_tokens_detects_google_chat_verification_token() {
        let mut config = test_config();
        config.google_chat.enabled = true;
        config.google_chat.incoming_webhook_url = "https://chat.googleapis.com/webhook".into();
        config.google_chat.verification_token = String::new();
        let finding = check_channel_tokens(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("Google Chat (incoming_webhook_url/verification_token)"));

        config.google_chat.verification_token = "token-123".into();
        assert!(check_channel_tokens(&config).is_none());
    }

    #[test]
    fn test_check_channel_tokens_detects_twilio_webhook_url() {
        let mut config = test_config();
        config.twilio.enabled = true;
        config.twilio.account_sid = "AC123".into();
        config.twilio.auth_token = "auth".into();
        config.twilio.from_number = "+15551234567".into();
        config.twilio.webhook_url = String::new();
        let finding = check_channel_tokens(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("Twilio (account_sid/auth_token/from_number/webhook_url)"));

        config.twilio.webhook_url = "https://example.com/api/twilio/webhook".into();
        assert!(check_channel_tokens(&config).is_none());
    }

    #[test]
    fn test_check_channel_tokens_detects_email_credentials() {
        let mut config = test_config();
        config.email.enabled = true;

        let finding = check_channel_tokens(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("Email (imap/smtp credentials and from_address)"));

        config.email.imap_host = "imap.example.com".into();
        config.email.imap_username = "inbox@example.com".into();
        config.email.imap_password = "imap-secret".into();
        config.email.smtp_host = "smtp.example.com".into();
        config.email.smtp_username = "inbox@example.com".into();
        config.email.smtp_password = "smtp-secret".into();
        config.email.from_address = "bot@example.com".into();
        assert!(check_channel_tokens(&config).is_none());
    }

    #[test]
    fn test_check_channel_transport_security_flags_remote_http_endpoints() {
        let mut config = test_config();
        config.matrix.enabled = true;
        config.matrix.homeserver_url = "http://matrix.example.com".into();
        config.mastodon.enabled = true;
        config.mastodon.instance_url = "http://mastodon.example.com".into();

        let finding =
            check_channel_transport_security(&config).expect("remote http endpoints should flag");
        assert_eq!(finding.id, "ch-insecure-transport");
        assert!(finding.description.contains("Matrix homeserver_url"));
        assert!(finding.description.contains("Mastodon instance_url"));
    }

    #[test]
    fn test_check_channel_transport_security_allows_local_http_endpoints() {
        let mut config = test_config();
        config.signal.enabled = true;
        config.signal.api_url = "http://localhost:8080".into();
        config.bluebubbles.enabled = true;
        config.bluebubbles.server_url = "http://127.0.0.1:1234".into();

        assert!(check_channel_transport_security(&config).is_none());
    }

    #[test]
    fn test_check_notification_delivery_scope_flags_missing_recipients() {
        let mut config = test_config();
        config.telegram.enabled = true;
        config.telegram.bot_token = "123456:ABCDEF".into();
        config.telegram.allowed_chat_ids.clear();
        config.telegram.admin_chat_ids.clear();

        let finding = check_notification_delivery_scope(&config)
            .expect("missing broadcast recipients should be flagged");
        assert_eq!(finding.id, "ch-notification-delivery-scope");
        assert!(finding
            .description
            .contains("Telegram (no allowed/admin chat IDs)"));
    }

    #[test]
    fn test_check_notification_delivery_scope_flags_bluebubbles_all_configured_gap() {
        let mut config = test_config();
        config.bluebubbles.enabled = true;
        config.bluebubbles.server_url = "http://localhost:1234".into();
        config.bluebubbles.password = "secret".into();
        config.bluebubbles.allowed_handles = vec!["+15551234567".into()];

        let finding = check_notification_delivery_scope(&config)
            .expect("BlueBubbles all_configured gap should be flagged");
        assert!(finding
            .description
            .contains("BlueBubbles (all_configured unsupported; explicit chat_guid required)"));
    }

    #[test]
    fn test_check_notification_delivery_scope_flags_unsupported_notify_channels() {
        let mut config = test_config();
        config.teams.enabled = true;
        config.teams.app_id = "teams-app".into();
        config.teams.app_password = "teams-secret".into();
        config.mastodon.enabled = true;
        config.mastodon.instance_url = "https://mastodon.example.com".into();
        config.mastodon.access_token = "mastodon-token".into();
        config.rocketchat.enabled = true;
        config.rocketchat.server_url = "https://chat.example.com".into();
        config.rocketchat.username = "bot".into();
        config.rocketchat.password = "secret".into();
        config.webchat.enabled = true;
        config.email.enabled = true;

        let finding = check_notification_delivery_scope(&config)
            .expect("unsupported notify channels should be flagged");
        assert!(finding
            .description
            .contains("Microsoft Teams (notify_target unsupported)"));
        assert!(finding
            .description
            .contains("Mastodon (notify_target unsupported)"));
        assert!(finding
            .description
            .contains("Rocket.Chat (notify_target unsupported)"));
        assert!(finding
            .description
            .contains("WebChat (notify_target unsupported)"));
        assert!(finding
            .description
            .contains("Email (notify_target unsupported)"));
    }

    #[test]
    fn test_check_notification_delivery_scope_not_flagged_when_targets_present() {
        let mut config = test_config();
        config.telegram.enabled = true;
        config.telegram.bot_token = "123456:ABCDEF".into();
        config.telegram.allowed_chat_ids = vec![123456789];

        assert!(check_notification_delivery_scope(&config).is_none());
    }

    #[test]
    fn test_check_email_channel_fully_implemented() {
        let mut config = test_config();
        config.email.enabled = true;
        // Email is now fully implemented — audit should return None
        assert!(check_email_channel_stub(&config).is_none());
    }

    #[test]
    fn test_check_email_channel_stub_not_flagged_when_disabled() {
        let config = test_config();
        assert!(check_email_channel_stub(&config).is_none());
    }

    #[test]
    fn test_check_defense_fail_open() {
        let mut config = test_config();
        config.defense.enabled = true;
        config.defense.fail_open = true;
        let finding = check_defense_fail_open(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "cfg-defense-fail-open");

        config.defense.fail_open = false;
        assert!(check_defense_fail_open(&config).is_none());
    }

    #[test]
    fn test_check_workspace_confinement() {
        let mut config = test_config();
        // Default has blocked paths, should pass.
        assert!(check_workspace_confinement(&config).is_none());

        config.filesystem.blocked_paths = Vec::new();
        let finding = check_workspace_confinement(&config);
        assert!(finding.is_some());
    }

    #[test]
    fn test_run_full_audit_returns_valid_report() {
        let config = test_config();
        let report = run_full_audit(&config);
        assert!(report.total_checks >= 15);
        assert_eq!(
            report.total_checks,
            report.passed_count + report.findings.len()
        );
        assert!(report.timestamp <= Utc::now());
    }

    #[test]
    fn test_auto_fix_not_auto_fixable() {
        let finding = SecurityAuditFinding {
            id: "cfg-api-key".into(),
            severity: SecurityAuditSeverity::Critical,
            title: "No API key".into(),
            description: "Test".into(),
            fix_hint: None,
            auto_fixable: false,
        };
        let result = auto_fix(&finding);
        assert!(!result.success);
        assert!(result.message.contains("not auto-fixable"));
    }

    #[test]
    fn test_auto_fix_defense_disabled() {
        let finding = SecurityAuditFinding {
            id: "cfg-defense-disabled".into(),
            severity: SecurityAuditSeverity::High,
            title: "Defense disabled".into(),
            description: "Test".into(),
            fix_hint: None,
            auto_fixable: true,
        };
        let result = auto_fix(&finding);
        // In test environments the config file may not exist, so the fix
        // will report failure with a descriptive message. In production the
        // fix would mutate config.yaml.
        assert_eq!(result.finding_id, "cfg-defense-disabled");
        // Either it succeeded (config existed) or it failed with a clear reason.
        if !result.success {
            assert!(
                result.message.contains("Config file not found")
                    || result.message.contains("Failed to"),
                "Unexpected failure message: {}",
                result.message,
            );
        }
    }

    #[test]
    fn test_auto_fix_unknown_id() {
        let finding = SecurityAuditFinding {
            id: "unknown-check".into(),
            severity: SecurityAuditSeverity::Low,
            title: "Unknown".into(),
            description: "Test".into(),
            fix_hint: None,
            auto_fixable: true,
        };
        let result = auto_fix(&finding);
        assert!(!result.success);
        assert!(result.message.contains("No auto-fix handler"));
    }

    #[test]
    fn test_check_prompt_injection_blocking() {
        let mut config = test_config();
        config.guardrails.detect_prompt_injection = true;
        config.guardrails.block_prompt_injection = false;
        let finding = check_prompt_injection_blocking(&config);
        assert!(finding.is_some());
        assert_eq!(
            finding.as_ref().unwrap().id,
            "cfg-prompt-injection-warn-only"
        );

        config.guardrails.block_prompt_injection = true;
        assert!(check_prompt_injection_blocking(&config).is_none());
    }

    #[test]
    fn test_finding_serialization() {
        let finding = SecurityAuditFinding {
            id: "test-id".into(),
            severity: SecurityAuditSeverity::High,
            title: "Test finding".into(),
            description: "A test".into(),
            fix_hint: Some("Fix it".into()),
            auto_fixable: true,
        };
        let json = serde_json::to_string(&finding).unwrap();
        let deserialized: SecurityAuditFinding = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-id");
        assert_eq!(deserialized.severity, SecurityAuditSeverity::High);
        assert!(deserialized.auto_fixable);
    }

    #[test]
    fn test_report_serialization() {
        let report = SecurityAuditReport {
            findings: vec![],
            passed_count: 10,
            total_checks: 10,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&report).unwrap();
        let deserialized: SecurityAuditReport = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.passed_count, 10);
        assert_eq!(deserialized.total_checks, 10);
    }

    #[test]
    fn test_fix_result_serialization() {
        let result = FixResult {
            finding_id: "test".into(),
            success: true,
            message: "Done".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: FixResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.success);
    }

    // -----------------------------------------------------------------------
    // Tests for enhanced security audit checks (P3A)
    // -----------------------------------------------------------------------

    #[test]
    fn test_check_gateway_bind_address_loopback() {
        let mut config = test_config();
        config.gateway.enabled = true;
        config.gateway.bind_address = "127.0.0.1".to_string();
        assert!(check_gateway_bind_address(&config).is_none());
    }

    #[test]
    fn test_check_gateway_bind_address_all_interfaces() {
        let mut config = test_config();
        config.gateway.enabled = true;
        config.gateway.bind_address = "0.0.0.0".to_string();
        let finding = check_gateway_bind_address(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "cfg-gateway-bind-all");
        assert_eq!(
            finding.as_ref().unwrap().severity,
            SecurityAuditSeverity::Medium
        );
        assert!(finding.as_ref().unwrap().auto_fixable);
    }

    #[test]
    fn test_check_gateway_bind_disabled_gateway() {
        let mut config = test_config();
        config.gateway.enabled = false;
        config.gateway.bind_address = "0.0.0.0".to_string();
        // Disabled gateway should not flag.
        assert!(check_gateway_bind_address(&config).is_none());
    }

    #[test]
    fn test_check_deberta_threshold_good() {
        let mut config = test_config();
        config.defense.enabled = true;
        config.defense.deberta_enabled = true;
        config.defense.deberta_threshold = 0.85;
        assert!(check_deberta_threshold(&config).is_none());
    }

    #[test]
    fn test_check_deberta_threshold_low() {
        let mut config = test_config();
        config.defense.enabled = true;
        config.defense.deberta_enabled = true;
        config.defense.deberta_threshold = 0.5;
        let finding = check_deberta_threshold(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "cfg-deberta-threshold-low");
        assert_eq!(
            finding.as_ref().unwrap().severity,
            SecurityAuditSeverity::Medium
        );
        assert!(finding.as_ref().unwrap().auto_fixable);
    }

    #[test]
    fn test_check_deberta_threshold_exactly_07() {
        let mut config = test_config();
        config.defense.enabled = true;
        config.defense.deberta_enabled = true;
        config.defense.deberta_threshold = 0.7;
        // 0.7 is the minimum acceptable; should pass.
        assert!(check_deberta_threshold(&config).is_none());
    }

    #[test]
    fn test_check_deberta_threshold_disabled_defense() {
        let mut config = test_config();
        config.defense.enabled = false;
        config.defense.deberta_threshold = 0.1;
        // Defense disabled; threshold check should not flag.
        assert!(check_deberta_threshold(&config).is_none());
    }

    #[test]
    fn test_check_defense_pipeline_critical_enabled() {
        let mut config = test_config();
        config.defense.enabled = true;
        assert!(check_defense_pipeline_critical(&config).is_none());
    }

    #[test]
    fn test_check_defense_pipeline_critical_disabled() {
        let mut config = test_config();
        config.defense.enabled = false;
        let finding = check_defense_pipeline_critical(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "cfg-defense-pipeline-off");
        assert_eq!(
            finding.as_ref().unwrap().severity,
            SecurityAuditSeverity::Critical
        );
        assert!(finding.as_ref().unwrap().auto_fixable);
    }

    #[test]
    fn test_check_sandbox_blocked_paths_complete() {
        let mut config = test_config();
        config.sandbox.enabled = true;
        // Default sandbox config has all required paths.
        assert!(check_sandbox_blocked_paths(&config).is_none());
    }

    #[test]
    fn test_check_sandbox_blocked_paths_missing() {
        let mut config = test_config();
        config.sandbox.enabled = true;
        config.sandbox.blocked_paths = vec!["/etc".to_string()]; // Missing /proc, /sys, docker.sock
        let finding = check_sandbox_blocked_paths(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "cfg-sandbox-blocked-paths");
        assert_eq!(
            finding.as_ref().unwrap().severity,
            SecurityAuditSeverity::High
        );
        assert!(finding.as_ref().unwrap().description.contains("/proc"));
    }

    #[test]
    fn test_check_sandbox_blocked_paths_disabled() {
        let mut config = test_config();
        config.sandbox.enabled = false;
        config.sandbox.blocked_paths = Vec::new();
        // Sandbox disabled; check should not flag.
        assert!(check_sandbox_blocked_paths(&config).is_none());
    }

    #[test]
    fn test_check_plaintext_secrets_none() {
        let config = test_config();
        // Default config has no API keys set, should pass.
        assert!(check_plaintext_secrets(&config).is_none());
    }

    #[test]
    fn test_check_plaintext_secrets_keyring_placeholder() {
        let mut config = test_config();
        config.claude.api_key = Some(crate::security::credentials::KEYRING_PLACEHOLDER.to_string());
        // Using keyring placeholder should pass.
        assert!(check_plaintext_secrets(&config).is_none());
    }

    #[test]
    fn test_check_plaintext_secrets_actual_key() {
        let mut config = test_config();
        config.claude.api_key = Some("sk-ant-real-secret-key-here".to_string());
        let finding = check_plaintext_secrets(&config);
        assert!(finding.is_some());
        assert_eq!(finding.as_ref().unwrap().id, "cfg-plaintext-secrets");
        assert_eq!(
            finding.as_ref().unwrap().severity,
            SecurityAuditSeverity::High
        );
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("claude.api_key"));
        assert!(finding
            .as_ref()
            .unwrap()
            .fix_hint
            .as_ref()
            .is_some_and(|hint| hint.contains("Restrict config file permissions")));
    }

    #[test]
    fn test_check_plaintext_secrets_telegram_token() {
        let mut config = test_config();
        config.telegram.enabled = true;
        config.telegram.bot_token = "123456:ABC-DEF".to_string();
        let finding = check_plaintext_secrets(&config);
        assert!(finding.is_some());
        assert!(finding
            .as_ref()
            .unwrap()
            .description
            .contains("telegram.bot_token"));
    }

    #[test]
    fn test_check_plaintext_secrets_detects_messenger_and_twilio() {
        let mut config = test_config();
        config.messenger.enabled = true;
        config.messenger.page_access_token = "EAAB-secret".to_string();
        config.twilio.enabled = true;
        config.twilio.auth_token = "twilio-auth-token".to_string();

        let finding = check_plaintext_secrets(&config).expect("plaintext secrets should be found");
        assert!(finding.description.contains("messenger.page_access_token"));
        assert!(finding.description.contains("twilio.auth_token"));
    }

    #[test]
    fn test_check_plaintext_secrets_detects_channel_verification_secrets() {
        let mut config = test_config();
        config.whatsapp.enabled = true;
        config.whatsapp.verify_token = "wa-verify-token".to_string();
        config.whatsapp.app_secret = "wa-app-secret".to_string();
        config.slack.enabled = true;
        config.slack.app_token = "xapp-secret-token".to_string();
        config.messenger.enabled = true;
        config.messenger.verify_token = "messenger-verify-token".to_string();
        config.instagram.enabled = true;
        config.instagram.verify_token = "instagram-verify-token".to_string();

        let finding =
            check_plaintext_secrets(&config).expect("verification secrets should be found");
        assert!(finding.description.contains("whatsapp.verify_token"));
        assert!(finding.description.contains("whatsapp.app_secret"));
        assert!(finding.description.contains("slack.app_token"));
        assert!(finding.description.contains("messenger.verify_token"));
        assert!(finding.description.contains("instagram.verify_token"));
    }

    #[test]
    fn test_check_plaintext_secrets_detects_provider_and_private_key_fields() {
        let mut config = test_config();
        config.cerebras.api_key = Some("csk-provider-secret-token".to_string());
        config.deepseek = Some(crate::config::DeepSeekConfig {
            api_key: Some("deepseek-secret-token".to_string()),
            api_url: "https://api.deepseek.com/v1".to_string(),
            default_model: "deepseek-chat".to_string(),
        });
        config.github_copilot = Some(crate::config::GitHubCopilotConfig {
            token: Some("ghu_copilot_secret_token_value".to_string()),
            api_url: "https://api.githubcopilot.com".to_string(),
        });
        config.minimax = Some(crate::config::MiniMaxConfig {
            api_key: Some("minimax-secret-token".to_string()),
            api_url: "https://api.minimax.chat/v1".to_string(),
            default_model: "minimax-2.5".to_string(),
        });
        config.k2k.private_key_pem =
            Some("-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----".to_string());
        config.search.brave_api_key = Some("brave-secret-key".to_string());
        config.search.tavily_api_key = Some("tavily-secret-key".to_string());
        config.stt.deepgram_api_key = Some("deepgram-secret-key".to_string());
        config.stt.openai_api_key = Some("openai-stt-secret-key".to_string());
        config.tts.elevenlabs_api_key = Some("elevenlabs-secret-key".to_string());
        config.tts.cartesia_api_key = Some("cartesia-secret-key".to_string());
        config.google_chat.enabled = true;
        config.google_chat.incoming_webhook_url =
            "https://chat.googleapis.com/v1/spaces/AAA/messages?key=secret".to_string();

        let finding = check_plaintext_secrets(&config)
            .expect("provider and private-key plaintext fields should be found");
        assert!(finding.description.contains("cerebras.api_key"));
        assert!(finding.description.contains("deepseek.api_key"));
        assert!(finding.description.contains("github_copilot.token"));
        assert!(finding.description.contains("minimax.api_key"));
        assert!(finding.description.contains("k2k.private_key_pem"));
        assert!(finding.description.contains("search.brave_api_key"));
        assert!(finding.description.contains("search.tavily_api_key"));
        assert!(finding.description.contains("stt.deepgram_api_key"));
        assert!(finding.description.contains("stt.openai_api_key"));
        assert!(finding.description.contains("tts.elevenlabs_api_key"));
        assert!(finding.description.contains("tts.cartesia_api_key"));
        assert!(finding
            .description
            .contains("google_chat.incoming_webhook_url"));
    }

    #[test]
    fn test_check_plaintext_secrets_detects_webchat_api_key() {
        let mut config = test_config();
        config.webchat.enabled = true;
        config.webchat.api_key = Some("webchat-secret".to_string());

        let finding = check_plaintext_secrets(&config).expect("webchat api key should be found");
        assert!(finding.description.contains("webchat.api_key"));
    }

    #[test]
    fn test_auto_fix_gateway_bind_address() {
        let finding = SecurityAuditFinding {
            id: "cfg-gateway-bind-all".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "Gateway bound to all interfaces".into(),
            description: "Test".into(),
            fix_hint: None,
            auto_fixable: true,
        };
        let result = auto_fix(&finding);
        assert_eq!(result.finding_id, "cfg-gateway-bind-all");
        // In test environments the config file may not exist.
        if !result.success {
            assert!(
                result.message.contains("Config file not found")
                    || result.message.contains("Failed to"),
            );
        }
    }

    #[test]
    fn test_auto_fix_deberta_threshold() {
        let finding = SecurityAuditFinding {
            id: "cfg-deberta-threshold-low".into(),
            severity: SecurityAuditSeverity::Medium,
            title: "DeBERTa threshold too low".into(),
            description: "Test".into(),
            fix_hint: None,
            auto_fixable: true,
        };
        let result = auto_fix(&finding);
        assert_eq!(result.finding_id, "cfg-deberta-threshold-low");
    }

    #[test]
    fn test_auto_fix_defense_pipeline_off() {
        let finding = SecurityAuditFinding {
            id: "cfg-defense-pipeline-off".into(),
            severity: SecurityAuditSeverity::Critical,
            title: "Defense pipeline disabled".into(),
            description: "Test".into(),
            fix_hint: None,
            auto_fixable: true,
        };
        let result = auto_fix(&finding);
        assert_eq!(result.finding_id, "cfg-defense-pipeline-off");
    }

    #[test]
    fn test_enhanced_checks_in_full_audit() {
        let mut config = test_config();
        // Set up conditions that trigger the new checks.
        config.gateway.enabled = true;
        config.gateway.bind_address = "0.0.0.0".to_string();
        config.defense.enabled = false;

        let report = run_full_audit(&config);

        // Verify the new check IDs appear in findings.
        let finding_ids: Vec<&str> = report.findings.iter().map(|f| f.id.as_str()).collect();
        assert!(
            finding_ids.contains(&"cfg-gateway-bind-all"),
            "Expected cfg-gateway-bind-all in findings, got: {:?}",
            finding_ids
        );
        assert!(
            finding_ids.contains(&"cfg-defense-pipeline-off"),
            "Expected cfg-defense-pipeline-off in findings, got: {:?}",
            finding_ids
        );
    }
}
