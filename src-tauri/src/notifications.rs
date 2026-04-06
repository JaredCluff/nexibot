//! Notification dispatcher — routes completion messages to the appropriate channel.
//!
//! Used by background tasks, DAG runs, and the heartbeat to notify users when
//! long-running work finishes.  Each notification target maps to a specific
//! channel or the GUI event bus.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
use url::Url;

use crate::config::NexiBotConfig;

const MAX_NOTIFICATION_CHARS: usize = 1500;

/// Where to deliver a completion notification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotificationTarget {
    /// Send to a specific Telegram chat_id.
    Telegram { chat_id: i64 },
    /// Send to all configured Telegram allowlist/admin chat IDs.
    TelegramConfigured,
    /// Send to a specific Discord channel_id.
    Discord { channel_id: u64 },
    /// Send to a specific Slack channel_id.
    Slack { channel_id: String },
    /// Send to a specific WhatsApp phone number.
    WhatsApp { phone_number: String },
    /// Send to a specific Signal phone number.
    Signal { phone_number: String },
    /// Send to a specific Matrix room.
    Matrix { room_id: String },
    /// Send to a specific Mattermost channel.
    Mattermost { channel_id: String },
    /// Send using the configured Google Chat incoming webhook.
    GoogleChat,
    /// Send to a specific BlueBubbles iMessage chat GUID.
    BlueBubbles { chat_guid: String },
    /// Send to a specific Messenger recipient (PSID).
    Messenger { recipient_id: String },
    /// Send to a specific Instagram recipient ID.
    Instagram { recipient_id: String },
    /// Send to a specific LINE user ID.
    Line { user_id: String },
    /// Send to a specific Twilio phone number.
    Twilio { phone_number: String },
    /// Emit a Tauri event to the GUI (`notification:received`).
    Gui,
    /// Broadcast to all enabled/configured notification channels that have
    /// concrete delivery targets available from config.
    AllConfigured,
}

/// Routes notification messages to one or more channels.
///
/// Cheap to clone (all fields are `Arc`).
#[derive(Clone)]
pub struct NotificationDispatcher {
    config: Arc<RwLock<NexiBotConfig>>,
    app_handle: Option<tauri::AppHandle>,
}

fn is_i64_target_allowed(target: i64, allowlist: &[i64], admin_list: &[i64]) -> bool {
    // Explicit notification targets fail closed when no trusted target lists
    // are configured for the channel.
    if allowlist.is_empty() && admin_list.is_empty() {
        return false;
    }
    allowlist.contains(&target) || admin_list.contains(&target)
}

fn is_u64_target_allowed(target: u64, allowlist: &[u64]) -> bool {
    !allowlist.is_empty() && allowlist.contains(&target)
}

fn is_string_target_allowed(target: &str, allowlist: &[String], admin_list: &[String]) -> bool {
    if allowlist.is_empty() && admin_list.is_empty() {
        return false;
    }
    allowlist.iter().any(|v| v == target) || admin_list.iter().any(|v| v == target)
}

fn is_string_target_allowed_allowlist_only(target: &str, allowlist: &[String]) -> bool {
    !allowlist.is_empty() && allowlist.iter().any(|v| v == target)
}

fn is_bluebubbles_chat_guid_allowed(
    chat_guid: &str,
    allowed_handles: &[String],
    admin_handles: &[String],
) -> bool {
    if allowed_handles.is_empty() && admin_handles.is_empty() {
        return false;
    }

    // BlueBubbles chat GUIDs include delimiter-separated components like:
    // "iMessage;-;+15551234567". Match against normalized tokens so explicit
    // notify targets stay bound to trusted handles configured for ingress.
    let guid_tokens: Vec<String> = chat_guid
        .split(|c| [';', ',', ':'].contains(&c))
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect();

    allowed_handles
        .iter()
        .chain(admin_handles.iter())
        .map(|handle| handle.trim().to_ascii_lowercase())
        .filter(|handle| !handle.is_empty())
        .any(|handle| {
            guid_tokens.iter().any(|token| {
                token == &handle
                    || (token.starts_with('+') && token.trim_start_matches('+') == handle)
                    || (handle.starts_with('+') && handle.trim_start_matches('+') == token)
            })
        })
}

fn merge_unique_i64(primary: &[i64], secondary: &[i64]) -> Vec<i64> {
    let mut merged = primary.to_vec();
    for value in secondary {
        if !merged.contains(value) {
            merged.push(*value);
        }
    }
    merged
}

fn merge_unique_strings(primary: &[String], secondary: &[String]) -> Vec<String> {
    let mut merged = primary.to_vec();
    for value in secondary {
        if !merged.iter().any(|existing| existing == value) {
            merged.push(value.clone());
        }
    }
    merged
}

fn sanitize_notification_message(message: &str) -> String {
    let cleaned = message.replace('\0', "");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return "(empty notification)".to_string();
    }

    let mut output = String::new();
    for (idx, ch) in trimmed.chars().enumerate() {
        if idx >= MAX_NOTIFICATION_CHARS {
            output.push_str("... [truncated]");
            return output;
        }
        output.push(ch);
    }
    output
}

fn append_url_path_segment(base_path: &str, segment: &str) -> String {
    let base = base_path.trim_end_matches('/');
    let segment = segment.trim_start_matches('/');
    if base.is_empty() || base == "/" {
        format!("/{}", segment)
    } else {
        format!("{}/{}", base, segment)
    }
}

fn build_bluebubbles_send_url(server_url: &str, password: &str) -> Result<String, String> {
    let mut url =
        Url::parse(server_url).map_err(|e| format!("invalid BlueBubbles server_url: {}", e))?;
    let path = append_url_path_segment(url.path(), "api/v1/message/text");
    url.set_path(&path);
    url.set_query(None);
    url.query_pairs_mut().append_pair("password", password);
    Ok(url.to_string())
}

impl NotificationDispatcher {
    pub fn new(config: Arc<RwLock<NexiBotConfig>>, app_handle: Option<tauri::AppHandle>) -> Self {
        Self { config, app_handle }
    }

    /// Dispatch a message to the given target.
    ///
    /// Returns `true` when at least one concrete delivery target was attempted.
    /// This is used by callers that track notification state to avoid marking
    /// notifications as sent when there was nowhere to deliver them.
    pub async fn dispatch(&self, target: &NotificationTarget, message: &str) -> bool {
        let sanitized_message = sanitize_notification_message(message);
        let message = sanitized_message.as_str();
        match target {
            NotificationTarget::Telegram { chat_id } => {
                let (enabled, bot_token, allowed_chat_ids, admin_chat_ids) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.telegram.enabled,
                        cfg.telegram.bot_token.clone(),
                        cfg.telegram.allowed_chat_ids.clone(),
                        cfg.telegram.admin_chat_ids.clone(),
                    )
                };
                if !enabled || bot_token.is_empty() {
                    warn!("[NOTIFY] Telegram target requested but channel is not configured");
                    false
                } else if is_i64_target_allowed(*chat_id, &allowed_chat_ids, &admin_chat_ids) {
                    self.send_telegram(*chat_id, message).await;
                    true
                } else {
                    warn!(
                        "[NOTIFY] Telegram target {} blocked by channel allowlist/admin policy",
                        chat_id
                    );
                    false
                }
            }
            NotificationTarget::TelegramConfigured => {
                let (enabled, bot_token, tg_targets) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.telegram.enabled,
                        cfg.telegram.bot_token.clone(),
                        merge_unique_i64(
                            &cfg.telegram.allowed_chat_ids,
                            &cfg.telegram.admin_chat_ids,
                        ),
                    )
                };
                if !enabled || bot_token.is_empty() {
                    warn!(
                        "[NOTIFY] TelegramConfigured target requested but Telegram is not configured"
                    );
                    false
                } else {
                    let attempted = !tg_targets.is_empty();
                    for chat_id in &tg_targets {
                        self.send_telegram(*chat_id, message).await;
                    }
                    attempted
                }
            }
            NotificationTarget::Discord { channel_id } => {
                let (enabled, bot_token, allowed_channel_ids) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.discord.enabled,
                        cfg.discord.bot_token.clone(),
                        cfg.discord.allowed_channel_ids.clone(),
                    )
                };
                if !enabled || bot_token.is_empty() {
                    warn!("[NOTIFY] Discord target requested but channel is not configured");
                    false
                } else if is_u64_target_allowed(*channel_id, &allowed_channel_ids) {
                    self.send_discord(*channel_id, message).await;
                    true
                } else {
                    warn!(
                        "[NOTIFY] Discord target {} blocked by channel allowlist policy",
                        channel_id
                    );
                    false
                }
            }
            NotificationTarget::Slack { channel_id } => {
                let (enabled, bot_token, allowed_channel_ids) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.slack.enabled,
                        cfg.slack.bot_token.clone(),
                        cfg.slack.allowed_channel_ids.clone(),
                    )
                };
                if !enabled || bot_token.is_empty() {
                    warn!("[NOTIFY] Slack target requested but channel is not configured");
                    false
                } else if is_string_target_allowed_allowlist_only(channel_id, &allowed_channel_ids)
                {
                    self.send_slack(channel_id, message).await;
                    true
                } else {
                    warn!(
                        "[NOTIFY] Slack target {} blocked by channel allowlist policy",
                        channel_id
                    );
                    false
                }
            }
            NotificationTarget::WhatsApp { phone_number } => {
                let (
                    enabled,
                    phone_number_id,
                    access_token,
                    allowed_phone_numbers,
                    admin_phone_numbers,
                ) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.whatsapp.enabled,
                        cfg.whatsapp.phone_number_id.clone(),
                        cfg.whatsapp.access_token.clone(),
                        cfg.whatsapp.allowed_phone_numbers.clone(),
                        cfg.whatsapp.admin_phone_numbers.clone(),
                    )
                };
                if enabled
                    && !phone_number_id.is_empty()
                    && !access_token.is_empty()
                    && is_string_target_allowed(
                        phone_number,
                        &allowed_phone_numbers,
                        &admin_phone_numbers,
                    )
                {
                    self.send_whatsapp(&phone_number_id, &access_token, phone_number, message)
                        .await;
                    true
                } else if enabled && !phone_number_id.is_empty() && !access_token.is_empty() {
                    warn!(
                        "[NOTIFY] WhatsApp target {} blocked by channel allowlist/admin policy",
                        phone_number
                    );
                    false
                } else {
                    warn!("[NOTIFY] WhatsApp target requested but channel is not configured");
                    false
                }
            }
            NotificationTarget::Signal { phone_number } => {
                let (enabled, api_url, bot_number, allowed_numbers, admin_numbers) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.signal.enabled,
                        cfg.signal.api_url.clone(),
                        cfg.signal.phone_number.clone(),
                        cfg.signal.allowed_numbers.clone(),
                        cfg.signal.admin_numbers.clone(),
                    )
                };
                if enabled
                    && !api_url.is_empty()
                    && !bot_number.is_empty()
                    && is_string_target_allowed(phone_number, &allowed_numbers, &admin_numbers)
                {
                    self.send_signal(&api_url, &bot_number, phone_number, message)
                        .await;
                    true
                } else if enabled && !api_url.is_empty() && !bot_number.is_empty() {
                    warn!(
                        "[NOTIFY] Signal target {} blocked by channel allowlist/admin policy",
                        phone_number
                    );
                    false
                } else {
                    warn!("[NOTIFY] Signal target requested but channel is not configured");
                    false
                }
            }
            NotificationTarget::Matrix { room_id } => {
                let (enabled, homeserver, token, allowed_room_ids) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.matrix.enabled,
                        cfg.matrix.homeserver_url.clone(),
                        cfg.matrix.access_token.clone(),
                        cfg.matrix.allowed_room_ids.clone(),
                    )
                };
                if enabled
                    && !homeserver.is_empty()
                    && !token.is_empty()
                    && is_string_target_allowed_allowlist_only(room_id, &allowed_room_ids)
                {
                    self.send_matrix(&homeserver, &token, room_id, message)
                        .await;
                    true
                } else if enabled && !homeserver.is_empty() && !token.is_empty() {
                    warn!(
                        "[NOTIFY] Matrix target {} blocked by channel allowlist policy",
                        room_id
                    );
                    false
                } else {
                    warn!("[NOTIFY] Matrix target requested but channel is not configured");
                    false
                }
            }
            NotificationTarget::Mattermost { channel_id } => {
                let (enabled, server_url, bot_token, allowed_channel_ids) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.mattermost.enabled,
                        cfg.mattermost.server_url.clone(),
                        cfg.mattermost.bot_token.clone(),
                        cfg.mattermost.allowed_channel_ids.clone(),
                    )
                };
                if enabled
                    && !server_url.is_empty()
                    && !bot_token.is_empty()
                    && is_string_target_allowed_allowlist_only(channel_id, &allowed_channel_ids)
                {
                    self.send_mattermost(&server_url, &bot_token, channel_id, message)
                        .await;
                    true
                } else if enabled && !server_url.is_empty() && !bot_token.is_empty() {
                    warn!(
                        "[NOTIFY] Mattermost target {} blocked by channel allowlist policy",
                        channel_id
                    );
                    false
                } else {
                    warn!("[NOTIFY] Mattermost target requested but channel is not configured");
                    false
                }
            }
            NotificationTarget::GoogleChat => {
                let (enabled, webhook_url) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.google_chat.enabled,
                        cfg.google_chat.incoming_webhook_url.clone(),
                    )
                };
                if enabled && !webhook_url.is_empty() {
                    self.send_google_chat(&webhook_url, message).await;
                    true
                } else {
                    warn!("[NOTIFY] Google Chat target requested but channel is not configured");
                    false
                }
            }
            NotificationTarget::BlueBubbles { chat_guid } => {
                let (enabled, server_url, password, allowed_handles, admin_handles) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.bluebubbles.enabled,
                        cfg.bluebubbles.server_url.clone(),
                        cfg.bluebubbles.password.clone(),
                        cfg.bluebubbles.allowed_handles.clone(),
                        cfg.bluebubbles.admin_handles.clone(),
                    )
                };
                if enabled
                    && !server_url.is_empty()
                    && !password.is_empty()
                    && is_bluebubbles_chat_guid_allowed(chat_guid, &allowed_handles, &admin_handles)
                {
                    self.send_bluebubbles(&server_url, &password, chat_guid, message)
                        .await;
                    true
                } else if enabled && !server_url.is_empty() && !password.is_empty() {
                    warn!(
                        "[NOTIFY] BlueBubbles target {} blocked by channel allowlist/admin policy",
                        chat_guid
                    );
                    false
                } else {
                    warn!("[NOTIFY] BlueBubbles target requested but channel is not configured");
                    false
                }
            }
            NotificationTarget::Messenger { recipient_id } => {
                let (enabled, page_access_token, allowed_sender_ids, admin_sender_ids) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.messenger.enabled,
                        cfg.messenger.page_access_token.clone(),
                        cfg.messenger.allowed_sender_ids.clone(),
                        cfg.messenger.admin_sender_ids.clone(),
                    )
                };
                if enabled
                    && !page_access_token.is_empty()
                    && is_string_target_allowed(
                        recipient_id,
                        &allowed_sender_ids,
                        &admin_sender_ids,
                    )
                {
                    self.send_messenger(&page_access_token, recipient_id, message)
                        .await;
                    true
                } else if enabled && !page_access_token.is_empty() {
                    warn!(
                        "[NOTIFY] Messenger target {} blocked by channel allowlist/admin policy",
                        recipient_id
                    );
                    false
                } else {
                    warn!("[NOTIFY] Messenger target requested but channel is not configured");
                    false
                }
            }
            NotificationTarget::Instagram { recipient_id } => {
                let (
                    enabled,
                    access_token,
                    instagram_account_id,
                    allowed_sender_ids,
                    admin_sender_ids,
                ) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.instagram.enabled,
                        cfg.instagram.access_token.clone(),
                        cfg.instagram.instagram_account_id.clone(),
                        cfg.instagram.allowed_sender_ids.clone(),
                        cfg.instagram.admin_sender_ids.clone(),
                    )
                };
                if enabled
                    && !access_token.is_empty()
                    && !instagram_account_id.is_empty()
                    && is_string_target_allowed(
                        recipient_id,
                        &allowed_sender_ids,
                        &admin_sender_ids,
                    )
                {
                    self.send_instagram(
                        &access_token,
                        &instagram_account_id,
                        recipient_id,
                        message,
                    )
                    .await;
                    true
                } else if enabled && !access_token.is_empty() && !instagram_account_id.is_empty() {
                    warn!(
                        "[NOTIFY] Instagram target {} blocked by channel allowlist/admin policy",
                        recipient_id
                    );
                    false
                } else {
                    warn!("[NOTIFY] Instagram target requested but channel is not configured");
                    false
                }
            }
            NotificationTarget::Line { user_id } => {
                let (enabled, channel_access_token, allowed_user_ids, admin_user_ids) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.line.enabled,
                        cfg.line.channel_access_token.clone(),
                        cfg.line.allowed_user_ids.clone(),
                        cfg.line.admin_user_ids.clone(),
                    )
                };
                if enabled
                    && !channel_access_token.is_empty()
                    && is_string_target_allowed(user_id, &allowed_user_ids, &admin_user_ids)
                {
                    self.send_line(&channel_access_token, user_id, message)
                        .await;
                    true
                } else if enabled && !channel_access_token.is_empty() {
                    warn!(
                        "[NOTIFY] LINE target {} blocked by channel allowlist/admin policy",
                        user_id
                    );
                    false
                } else {
                    warn!("[NOTIFY] LINE target requested but channel is not configured");
                    false
                }
            }
            NotificationTarget::Twilio { phone_number } => {
                let (enabled, account_sid, auth_token, from_number, allowed_numbers, admin_numbers) = {
                    let cfg = self.config.read().await;
                    (
                        cfg.twilio.enabled,
                        cfg.twilio.account_sid.clone(),
                        cfg.twilio.auth_token.clone(),
                        cfg.twilio.from_number.clone(),
                        cfg.twilio.allowed_numbers.clone(),
                        cfg.twilio.admin_numbers.clone(),
                    )
                };
                if enabled
                    && !account_sid.is_empty()
                    && !auth_token.is_empty()
                    && !from_number.is_empty()
                    && is_string_target_allowed(phone_number, &allowed_numbers, &admin_numbers)
                {
                    self.send_twilio(
                        &account_sid,
                        &auth_token,
                        &from_number,
                        phone_number,
                        message,
                    )
                    .await;
                    true
                } else if enabled
                    && !account_sid.is_empty()
                    && !auth_token.is_empty()
                    && !from_number.is_empty()
                {
                    warn!(
                        "[NOTIFY] Twilio target {} blocked by channel allowlist/admin policy",
                        phone_number
                    );
                    false
                } else {
                    warn!("[NOTIFY] Twilio target requested but channel is not configured");
                    false
                }
            }
            NotificationTarget::Gui => self.emit_gui(message),
            NotificationTarget::AllConfigured => self.broadcast(message).await,
        }
    }

    /// Send to every enabled/configured notification channel that has concrete delivery targets.
    pub async fn broadcast(&self, message: &str) -> bool {
        let sanitized_message = sanitize_notification_message(message);
        let message = sanitized_message.as_str();
        let mut attempted = false;
        let (
            tg_enabled,
            tg_token,
            tg_targets,
            dc_enabled,
            dc_token,
            dc_channels,
            sl_enabled,
            sl_token,
            sl_channels,
            wa_enabled,
            wa_phone_number_id,
            wa_access_token,
            wa_numbers,
            sig_enabled,
            sig_api_url,
            sig_bot_number,
            sig_numbers,
            mx_enabled,
            mx_homeserver,
            mx_access_token,
            mx_rooms,
            mm_enabled,
            mm_server_url,
            mm_bot_token,
            mm_channels,
            gc_enabled,
            gc_webhook_url,
            messenger_enabled,
            messenger_page_access_token,
            messenger_recipients,
            instagram_enabled,
            instagram_access_token,
            instagram_account_id,
            instagram_recipients,
            line_enabled,
            line_access_token,
            line_user_ids,
            twilio_enabled,
            twilio_account_sid,
            twilio_auth_token,
            twilio_from_number,
            twilio_numbers,
            bluebubbles_enabled,
            bluebubbles_server_url,
            bluebubbles_password,
            bluebubbles_allowed_handles,
            bluebubbles_admin_handles,
        ) = {
            let cfg = self.config.read().await;
            let tg_targets =
                merge_unique_i64(&cfg.telegram.allowed_chat_ids, &cfg.telegram.admin_chat_ids);
            let wa_numbers = merge_unique_strings(
                &cfg.whatsapp.allowed_phone_numbers,
                &cfg.whatsapp.admin_phone_numbers,
            );
            let sig_numbers =
                merge_unique_strings(&cfg.signal.allowed_numbers, &cfg.signal.admin_numbers);
            let messenger_recipients = merge_unique_strings(
                &cfg.messenger.allowed_sender_ids,
                &cfg.messenger.admin_sender_ids,
            );
            let instagram_recipients = merge_unique_strings(
                &cfg.instagram.allowed_sender_ids,
                &cfg.instagram.admin_sender_ids,
            );
            let line_user_ids =
                merge_unique_strings(&cfg.line.allowed_user_ids, &cfg.line.admin_user_ids);
            let twilio_numbers =
                merge_unique_strings(&cfg.twilio.allowed_numbers, &cfg.twilio.admin_numbers);

            (
                cfg.telegram.enabled,
                cfg.telegram.bot_token.clone(),
                tg_targets,
                cfg.discord.enabled,
                cfg.discord.bot_token.clone(),
                cfg.discord.allowed_channel_ids.clone(),
                cfg.slack.enabled,
                cfg.slack.bot_token.clone(),
                cfg.slack.allowed_channel_ids.clone(),
                cfg.whatsapp.enabled,
                cfg.whatsapp.phone_number_id.clone(),
                cfg.whatsapp.access_token.clone(),
                wa_numbers,
                cfg.signal.enabled,
                cfg.signal.api_url.clone(),
                cfg.signal.phone_number.clone(),
                sig_numbers,
                cfg.matrix.enabled,
                cfg.matrix.homeserver_url.clone(),
                cfg.matrix.access_token.clone(),
                cfg.matrix.allowed_room_ids.clone(),
                cfg.mattermost.enabled,
                cfg.mattermost.server_url.clone(),
                cfg.mattermost.bot_token.clone(),
                cfg.mattermost.allowed_channel_ids.clone(),
                cfg.google_chat.enabled,
                cfg.google_chat.incoming_webhook_url.clone(),
                cfg.messenger.enabled,
                cfg.messenger.page_access_token.clone(),
                messenger_recipients,
                cfg.instagram.enabled,
                cfg.instagram.access_token.clone(),
                cfg.instagram.instagram_account_id.clone(),
                instagram_recipients,
                cfg.line.enabled,
                cfg.line.channel_access_token.clone(),
                line_user_ids,
                cfg.twilio.enabled,
                cfg.twilio.account_sid.clone(),
                cfg.twilio.auth_token.clone(),
                cfg.twilio.from_number.clone(),
                twilio_numbers,
                cfg.bluebubbles.enabled,
                cfg.bluebubbles.server_url.clone(),
                cfg.bluebubbles.password.clone(),
                cfg.bluebubbles.allowed_handles.clone(),
                cfg.bluebubbles.admin_handles.clone(),
            )
        };

        if tg_enabled && !tg_token.is_empty() {
            if tg_targets.is_empty() {
                warn!(
                    "[NOTIFY] Skipping Telegram in all_configured broadcast: no allowed/admin chat IDs configured"
                );
            } else {
                for chat_id in &tg_targets {
                    attempted = true;
                    self.send_telegram(*chat_id, message).await;
                }
            }
        }

        if dc_enabled && !dc_token.is_empty() {
            if dc_channels.is_empty() {
                warn!(
                    "[NOTIFY] Skipping Discord in all_configured broadcast: no allowed_channel_ids configured"
                );
            } else {
                for channel_id in &dc_channels {
                    attempted = true;
                    self.send_discord(*channel_id, message).await;
                }
            }
        }

        if sl_enabled && !sl_token.is_empty() {
            if sl_channels.is_empty() {
                warn!(
                    "[NOTIFY] Skipping Slack in all_configured broadcast: no allowed_channel_ids configured"
                );
            } else {
                for channel_id in &sl_channels {
                    attempted = true;
                    self.send_slack(channel_id, message).await;
                }
            }
        }

        if wa_enabled && !wa_phone_number_id.is_empty() && !wa_access_token.is_empty() {
            if wa_numbers.is_empty() {
                warn!(
                    "[NOTIFY] Skipping WhatsApp in all_configured broadcast: no allowed/admin phone numbers configured"
                );
            } else {
                for number in &wa_numbers {
                    attempted = true;
                    self.send_whatsapp(&wa_phone_number_id, &wa_access_token, number, message)
                        .await;
                }
            }
        }

        if sig_enabled && !sig_api_url.is_empty() && !sig_bot_number.is_empty() {
            if sig_numbers.is_empty() {
                warn!(
                    "[NOTIFY] Skipping Signal in all_configured broadcast: no allowed/admin numbers configured"
                );
            } else {
                for number in &sig_numbers {
                    attempted = true;
                    self.send_signal(&sig_api_url, &sig_bot_number, number, message)
                        .await;
                }
            }
        }

        if mx_enabled && !mx_homeserver.is_empty() && !mx_access_token.is_empty() {
            if mx_rooms.is_empty() {
                warn!(
                    "[NOTIFY] Skipping Matrix in all_configured broadcast: no allowed_room_ids configured"
                );
            } else {
                for room_id in &mx_rooms {
                    attempted = true;
                    self.send_matrix(&mx_homeserver, &mx_access_token, room_id, message)
                        .await;
                }
            }
        }

        if mm_enabled && !mm_server_url.is_empty() && !mm_bot_token.is_empty() {
            if mm_channels.is_empty() {
                warn!(
                    "[NOTIFY] Skipping Mattermost in all_configured broadcast: no allowed_channel_ids configured"
                );
            } else {
                for channel_id in &mm_channels {
                    attempted = true;
                    self.send_mattermost(&mm_server_url, &mm_bot_token, channel_id, message)
                        .await;
                }
            }
        }

        if gc_enabled && !gc_webhook_url.is_empty() {
            attempted = true;
            self.send_google_chat(&gc_webhook_url, message).await;
        }

        if bluebubbles_enabled
            && !bluebubbles_server_url.is_empty()
            && !bluebubbles_password.is_empty()
        {
            if bluebubbles_allowed_handles.is_empty() && bluebubbles_admin_handles.is_empty() {
                warn!(
                    "[NOTIFY] Skipping BlueBubbles in all_configured broadcast: no allowed/admin handles configured"
                );
            } else {
                warn!(
                    "[NOTIFY] Skipping BlueBubbles in all_configured broadcast: explicit chat_guid is required (use notify_target type=bluebubbles)"
                );
            }
        }

        if messenger_enabled && !messenger_page_access_token.is_empty() {
            if messenger_recipients.is_empty() {
                warn!(
                    "[NOTIFY] Skipping Messenger in all_configured broadcast: no allowed/admin sender IDs configured"
                );
            } else {
                for recipient_id in &messenger_recipients {
                    attempted = true;
                    self.send_messenger(&messenger_page_access_token, recipient_id, message)
                        .await;
                }
            }
        }

        if instagram_enabled
            && !instagram_access_token.is_empty()
            && !instagram_account_id.is_empty()
        {
            if instagram_recipients.is_empty() {
                warn!(
                    "[NOTIFY] Skipping Instagram in all_configured broadcast: no allowed/admin sender IDs configured"
                );
            } else {
                for recipient_id in &instagram_recipients {
                    attempted = true;
                    self.send_instagram(
                        &instagram_access_token,
                        &instagram_account_id,
                        recipient_id,
                        message,
                    )
                    .await;
                }
            }
        }

        if line_enabled && !line_access_token.is_empty() {
            if line_user_ids.is_empty() {
                warn!(
                    "[NOTIFY] Skipping LINE in all_configured broadcast: no allowed/admin user IDs configured"
                );
            } else {
                for user_id in &line_user_ids {
                    attempted = true;
                    self.send_line(&line_access_token, user_id, message).await;
                }
            }
        }

        if twilio_enabled
            && !twilio_account_sid.is_empty()
            && !twilio_auth_token.is_empty()
            && !twilio_from_number.is_empty()
        {
            if twilio_numbers.is_empty() {
                warn!(
                    "[NOTIFY] Skipping Twilio in all_configured broadcast: no allowed/admin numbers configured"
                );
            } else {
                for number in &twilio_numbers {
                    attempted = true;
                    self.send_twilio(
                        &twilio_account_sid,
                        &twilio_auth_token,
                        &twilio_from_number,
                        number,
                        message,
                    )
                    .await;
                }
            }
        }

        if self.emit_gui(message) {
            attempted = true;
        }
        attempted
    }

    // -----------------------------------------------------------------------
    // Channel-specific senders
    // -----------------------------------------------------------------------

    async fn send_telegram(&self, chat_id: i64, message: &str) {
        let token = {
            let cfg = self.config.read().await;
            cfg.telegram.bot_token.clone()
        };
        if token.is_empty() {
            warn!("[NOTIFY] Telegram token not configured");
            return;
        }
        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": message,
        });
        match client.post(&url).json(&body).send().await {
            Ok(r) => {
                let status = r.status();
                let parsed = r.json::<serde_json::Value>().await;
                if !status.is_success() {
                    warn!("[NOTIFY] Telegram HTTP {} for chat_id {}", status, chat_id);
                    return;
                }

                match parsed {
                    Ok(json) => {
                        if json.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                            info!("[NOTIFY] Telegram notification sent to {}", chat_id);
                        } else {
                            let description = json
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            warn!(
                                "[NOTIFY] Telegram API rejected notification for chat_id {}: {}",
                                chat_id, description
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            "[NOTIFY] Telegram response parse error for chat_id {}: {}",
                            chat_id, e
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[NOTIFY] Telegram send error for chat_id {}: {}",
                    chat_id, e
                );
            }
        }
    }

    async fn send_discord(&self, channel_id: u64, message: &str) {
        let token = {
            let cfg = self.config.read().await;
            cfg.discord.bot_token.clone()
        };
        if token.is_empty() {
            warn!("[NOTIFY] Discord token not configured");
            return;
        }
        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        let url = format!(
            "https://discord.com/api/v10/channels/{}/messages",
            channel_id
        );
        let body = serde_json::json!({
            "content": message,
            "allowed_mentions": { "parse": [] }
        });
        match client
            .post(&url)
            .header("Authorization", format!("Bot {}", token))
            .json(&body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                info!(
                    "[NOTIFY] Discord notification sent to channel {}",
                    channel_id
                );
            }
            Ok(r) => {
                warn!(
                    "[NOTIFY] Discord HTTP {} for channel {}",
                    r.status(),
                    channel_id
                );
            }
            Err(e) => {
                warn!(
                    "[NOTIFY] Discord send error for channel {}: {}",
                    channel_id, e
                );
            }
        }
    }

    async fn send_slack(&self, channel_id: &str, message: &str) {
        let token = {
            let cfg = self.config.read().await;
            cfg.slack.bot_token.clone()
        };
        if token.is_empty() {
            warn!("[NOTIFY] Slack token not configured");
            return;
        }
        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        let body = serde_json::json!({
            "channel": channel_id,
            "text": message,
            "parse": "none",
            "link_names": false,
            "unfurl_links": false,
            "unfurl_media": false,
        });
        match client
            .post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await
        {
            Ok(r) => {
                let status = r.status();
                let parsed = r.json::<serde_json::Value>().await;
                if !status.is_success() {
                    warn!("[NOTIFY] Slack HTTP {} for channel {}", status, channel_id);
                    return;
                }

                match parsed {
                    Ok(json) => {
                        if json.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                            info!("[NOTIFY] Slack notification sent to {}", channel_id);
                        } else {
                            let err = json
                                .get("error")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            warn!(
                                "[NOTIFY] Slack API rejected notification to {}: {}",
                                channel_id, err
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            "[NOTIFY] Slack response parse error for channel {}: {}",
                            channel_id, e
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[NOTIFY] Slack send error for channel {}: {}",
                    channel_id, e
                );
            }
        }
    }

    async fn send_whatsapp(
        &self,
        phone_number_id: &str,
        access_token: &str,
        recipient: &str,
        message: &str,
    ) {
        let url = format!(
            "https://graph.facebook.com/v21.0/{}/messages",
            phone_number_id
        );
        let body = serde_json::json!({
            "messaging_product": "whatsapp",
            "to": recipient,
            "type": "text",
            "text": { "body": message }
        });

        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        match client
            .post(&url)
            .bearer_auth(access_token)
            .json(&body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                info!("[NOTIFY] WhatsApp notification sent to {}", recipient);
            }
            Ok(r) => {
                warn!(
                    "[NOTIFY] WhatsApp HTTP {} for recipient {}",
                    r.status(),
                    recipient
                );
            }
            Err(e) => {
                warn!(
                    "[NOTIFY] WhatsApp send error for recipient {}: {}",
                    recipient, e
                );
            }
        }
    }

    async fn send_signal(&self, api_url: &str, from: &str, to: &str, message: &str) {
        match crate::signal::send_signal_message(api_url, from, to, message).await {
            Ok(()) => info!("[NOTIFY] Signal notification sent to {}", to),
            Err(e) => warn!("[NOTIFY] Signal send error for {}: {}", to, e),
        }
    }

    async fn send_matrix(&self, homeserver: &str, token: &str, room_id: &str, message: &str) {
        let txn_id = format!("notify-{}", chrono::Utc::now().timestamp_millis());
        match crate::matrix::send_matrix_message(homeserver, token, room_id, message, &txn_id).await
        {
            Ok(()) => info!("[NOTIFY] Matrix notification sent to {}", room_id),
            Err(e) => warn!("[NOTIFY] Matrix send error for {}: {}", room_id, e),
        }
    }

    async fn send_mattermost(
        &self,
        server_url: &str,
        bot_token: &str,
        channel_id: &str,
        message: &str,
    ) {
        let url = format!("{}/api/v4/posts", server_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "channel_id": channel_id,
            "message": message,
        });

        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        match client
            .post(&url)
            .header("Authorization", format!("Bearer {}", bot_token))
            .json(&body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                info!(
                    "[NOTIFY] Mattermost notification sent to channel {}",
                    channel_id
                );
            }
            Ok(r) => {
                warn!(
                    "[NOTIFY] Mattermost HTTP {} for channel {}",
                    r.status(),
                    channel_id
                );
            }
            Err(e) => {
                warn!(
                    "[NOTIFY] Mattermost send error for channel {}: {}",
                    channel_id, e
                );
            }
        }
    }

    async fn send_google_chat(&self, webhook_url: &str, message: &str) {
        let body = serde_json::json!({ "text": message });
        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        match client.post(webhook_url).json(&body).send().await {
            Ok(r) if r.status().is_success() => {
                info!("[NOTIFY] Google Chat notification sent");
            }
            Ok(r) => {
                warn!("[NOTIFY] Google Chat HTTP {}", r.status());
            }
            Err(e) => {
                warn!("[NOTIFY] Google Chat send error: {}", e);
            }
        }
    }

    async fn send_bluebubbles(
        &self,
        server_url: &str,
        password: &str,
        chat_guid: &str,
        message: &str,
    ) {
        let url = match build_bluebubbles_send_url(server_url, password) {
            Ok(url) => url,
            Err(e) => {
                warn!(
                    "[NOTIFY] BlueBubbles target requested with invalid server_url: {}",
                    e
                );
                return;
            }
        };
        let temp_guid = format!(
            "nexibot-notify-{}-{}",
            chat_guid,
            chrono::Utc::now().timestamp_millis()
        );

        let body = serde_json::json!({
            "chatGuid": chat_guid,
            "message": message,
            "tempGuid": temp_guid,
        });

        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        match client.post(&url).json(&body).send().await {
            Ok(r) if r.status().is_success() => {
                info!("[NOTIFY] BlueBubbles notification sent to {}", chat_guid);
            }
            Ok(r) => {
                warn!(
                    "[NOTIFY] BlueBubbles HTTP {} for chat {}",
                    r.status(),
                    chat_guid
                );
            }
            Err(e) => {
                warn!(
                    "[NOTIFY] BlueBubbles send error for chat {}: {}",
                    chat_guid, e
                );
            }
        }
    }

    async fn send_messenger(&self, page_access_token: &str, recipient_id: &str, message: &str) {
        let url = "https://graph.facebook.com/v19.0/me/messages";
        let body = serde_json::json!({
            "recipient": { "id": recipient_id },
            "message": { "text": message },
        });

        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        match client
            .post(url)
            .bearer_auth(page_access_token)
            .json(&body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                info!("[NOTIFY] Messenger notification sent to {}", recipient_id);
            }
            Ok(r) => {
                warn!(
                    "[NOTIFY] Messenger HTTP {} for recipient {}",
                    r.status(),
                    recipient_id
                );
            }
            Err(e) => {
                warn!(
                    "[NOTIFY] Messenger send error for recipient {}: {}",
                    recipient_id, e
                );
            }
        }
    }

    async fn send_instagram(
        &self,
        access_token: &str,
        instagram_account_id: &str,
        recipient_id: &str,
        message: &str,
    ) {
        let url = format!(
            "https://graph.facebook.com/v19.0/{}/messages",
            instagram_account_id
        );
        let body = serde_json::json!({
            "recipient": { "id": recipient_id },
            "message": { "text": message },
        });

        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        match client
            .post(&url)
            .bearer_auth(access_token)
            .json(&body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                info!("[NOTIFY] Instagram notification sent to {}", recipient_id);
            }
            Ok(r) => {
                warn!(
                    "[NOTIFY] Instagram HTTP {} for recipient {}",
                    r.status(),
                    recipient_id
                );
            }
            Err(e) => {
                warn!(
                    "[NOTIFY] Instagram send error for recipient {}: {}",
                    recipient_id, e
                );
            }
        }
    }

    async fn send_line(&self, channel_access_token: &str, user_id: &str, message: &str) {
        let body = serde_json::json!({
            "to": user_id,
            "messages": [{
                "type": "text",
                "text": message,
            }],
        });
        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        match client
            .post("https://api.line.me/v2/bot/message/push")
            .header("Authorization", format!("Bearer {}", channel_access_token))
            .json(&body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                info!("[NOTIFY] LINE notification sent to {}", user_id);
            }
            Ok(r) => {
                warn!("[NOTIFY] LINE HTTP {} for user {}", r.status(), user_id);
            }
            Err(e) => {
                warn!("[NOTIFY] LINE send error for user {}: {}", user_id, e);
            }
        }
    }

    async fn send_twilio(
        &self,
        account_sid: &str,
        auth_token: &str,
        from_number: &str,
        to_number: &str,
        message: &str,
    ) {
        let url = format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}/Messages.json",
            account_sid
        );

        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
        match client
            .post(&url)
            .basic_auth(account_sid, Some(auth_token))
            .form(&[("To", to_number), ("From", from_number), ("Body", message)])
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                info!("[NOTIFY] Twilio notification sent to {}", to_number);
            }
            Ok(r) => {
                warn!(
                    "[NOTIFY] Twilio HTTP {} for number {}",
                    r.status(),
                    to_number
                );
            }
            Err(e) => {
                warn!("[NOTIFY] Twilio send error for number {}: {}", to_number, e);
            }
        }
    }

    fn emit_gui(&self, message: &str) -> bool {
        use tauri::Emitter;
        if let Some(handle) = &self.app_handle {
            if handle
                .emit(
                    "notification:received",
                    serde_json::json!({
                        "message": message,
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                )
                .is_ok()
            {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_i64_target_allowed_when_lists_empty_blocks() {
        assert!(!is_i64_target_allowed(42, &[], &[]));
    }

    #[test]
    fn test_sanitize_notification_message_trims_and_removes_nul() {
        let msg = " \0\0Hello world\0 ";
        assert_eq!(sanitize_notification_message(msg), "Hello world");
    }

    #[test]
    fn test_sanitize_notification_message_empty_fallback() {
        assert_eq!(
            sanitize_notification_message("   \0 "),
            "(empty notification)"
        );
    }

    #[test]
    fn test_sanitize_notification_message_truncates_long_messages() {
        let msg = "a".repeat(MAX_NOTIFICATION_CHARS + 25);
        let sanitized = sanitize_notification_message(&msg);
        assert!(sanitized.ends_with("... [truncated]"));
        assert_eq!(
            sanitized.chars().count(),
            MAX_NOTIFICATION_CHARS + "... [truncated]".chars().count()
        );
    }

    #[test]
    fn test_is_i64_target_allowed_matches_allow_or_admin_lists() {
        assert!(is_i64_target_allowed(100, &[100], &[]));
        assert!(is_i64_target_allowed(200, &[], &[200]));
        assert!(!is_i64_target_allowed(300, &[100], &[200]));
    }

    #[test]
    fn test_is_u64_target_allowed_respects_allowlist() {
        assert!(!is_u64_target_allowed(7, &[]));
        assert!(is_u64_target_allowed(7, &[7]));
        assert!(!is_u64_target_allowed(8, &[7]));
    }

    #[test]
    fn test_is_string_target_allowed_when_lists_empty_blocks() {
        assert!(!is_string_target_allowed("abc", &[], &[]));
    }

    #[test]
    fn test_is_string_target_allowed_matches_allow_or_admin_lists() {
        let allow = vec!["one".to_string()];
        let admin = vec!["two".to_string()];
        assert!(is_string_target_allowed("one", &allow, &admin));
        assert!(is_string_target_allowed("two", &allow, &admin));
        assert!(!is_string_target_allowed("three", &allow, &admin));
    }

    #[test]
    fn test_is_string_target_allowed_allowlist_only() {
        let allow = vec!["chan-a".to_string(), "chan-b".to_string()];
        assert!(is_string_target_allowed_allowlist_only("chan-a", &allow));
        assert!(!is_string_target_allowed_allowlist_only("chan-c", &allow));
        assert!(!is_string_target_allowed_allowlist_only("anything", &[]));
    }

    #[test]
    fn test_is_bluebubbles_chat_guid_allowed_when_lists_empty_blocks() {
        assert!(!is_bluebubbles_chat_guid_allowed(
            "iMessage;-;+15551234567",
            &[],
            &[]
        ));
    }

    #[test]
    fn test_is_bluebubbles_chat_guid_allowed_matches_allow_and_admin_handles() {
        let allow = vec!["+15551234567".to_string()];
        let admin = vec!["admin@example.com".to_string()];
        assert!(is_bluebubbles_chat_guid_allowed(
            "iMessage;-;+15551234567",
            &allow,
            &[]
        ));
        assert!(is_bluebubbles_chat_guid_allowed(
            "iMessage;-;admin@example.com",
            &[],
            &admin
        ));
        assert!(is_bluebubbles_chat_guid_allowed(
            "iMessage;-;+15551234567",
            &["15551234567".to_string()],
            &[]
        ));
        assert!(!is_bluebubbles_chat_guid_allowed(
            "iMessage;-;+15550000000",
            &allow,
            &admin
        ));
    }

    #[test]
    fn test_merge_unique_i64_deduplicates_and_preserves_order() {
        let merged = merge_unique_i64(&[1, 2, 3], &[3, 4, 2, 5]);
        assert_eq!(merged, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_merge_unique_strings_deduplicates_and_preserves_order() {
        let merged = merge_unique_strings(
            &["a".to_string(), "b".to_string()],
            &["b".to_string(), "c".to_string()],
        );
        assert_eq!(
            merged,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn test_build_bluebubbles_send_url_encodes_password() {
        let built = build_bluebubbles_send_url("http://localhost:1234", "p@ss word&token")
            .expect("BlueBubbles URL should build");
        assert_eq!(
            built,
            "http://localhost:1234/api/v1/message/text?password=p%40ss+word%26token"
        );
    }

    // ── NotificationTarget serde ──────────────────────────────────────

    #[test]
    fn test_all_configured_serde() {
        let t = NotificationTarget::AllConfigured;
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("all_configured"), "unexpected: {}", json);
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_gui_serde() {
        let t = NotificationTarget::Gui;
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("gui"), "unexpected: {}", json);
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_telegram_serde() {
        let t = NotificationTarget::Telegram {
            chat_id: -100456789,
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
        // Verify chat_id survives the round-trip (including negative group IDs)
        if let NotificationTarget::Telegram { chat_id } = back {
            assert_eq!(chat_id, -100456789);
        } else {
            panic!("wrong variant after roundtrip");
        }
    }

    #[test]
    fn test_telegram_configured_serde() {
        let t = NotificationTarget::TelegramConfigured;
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("telegram_configured"), "unexpected: {}", json);
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_discord_serde() {
        let t = NotificationTarget::Discord {
            channel_id: 123456789012345,
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_slack_serde() {
        let t = NotificationTarget::Slack {
            channel_id: "C0ABC1234".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_whatsapp_serde() {
        let t = NotificationTarget::WhatsApp {
            phone_number: "+15551234567".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_signal_serde() {
        let t = NotificationTarget::Signal {
            phone_number: "+15557654321".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_matrix_serde() {
        let t = NotificationTarget::Matrix {
            room_id: "!roomid:matrix.org".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_mattermost_serde() {
        let t = NotificationTarget::Mattermost {
            channel_id: "channel-id".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_google_chat_serde() {
        let t = NotificationTarget::GoogleChat;
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_bluebubbles_serde() {
        let t = NotificationTarget::BlueBubbles {
            chat_guid: "iMessage;-;+15551234567".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_messenger_serde() {
        let t = NotificationTarget::Messenger {
            recipient_id: "1234567890".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_instagram_serde() {
        let t = NotificationTarget::Instagram {
            recipient_id: "17841400000000000".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_line_serde() {
        let t = NotificationTarget::Line {
            user_id: "U1234567890abcdef".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_twilio_serde() {
        let t = NotificationTarget::Twilio {
            phone_number: "+15551234567".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: NotificationTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn test_serde_tag_format() {
        // Verify the discriminant is "type" (snake_case tag) as required by the
        // JSON schema exposed to Claude in the tool definition.
        let t = NotificationTarget::Telegram { chat_id: 1 };
        let json = serde_json::to_string(&t).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"].as_str(), Some("telegram"));
        assert_eq!(v["chat_id"].as_i64(), Some(1));
    }

    #[test]
    fn test_all_variants_are_distinct() {
        // Ensure no two variants accidentally serialize to the same "type" string.
        let variants = vec![
            NotificationTarget::AllConfigured,
            NotificationTarget::Gui,
            NotificationTarget::Telegram { chat_id: 1 },
            NotificationTarget::TelegramConfigured,
            NotificationTarget::Discord { channel_id: 1 },
            NotificationTarget::Slack {
                channel_id: "x".into(),
            },
            NotificationTarget::WhatsApp {
                phone_number: "+1".into(),
            },
            NotificationTarget::Signal {
                phone_number: "+2".into(),
            },
            NotificationTarget::Matrix {
                room_id: "!r:m.org".into(),
            },
            NotificationTarget::Mattermost {
                channel_id: "chan".into(),
            },
            NotificationTarget::GoogleChat,
            NotificationTarget::BlueBubbles {
                chat_guid: "iMessage;-;+15551234567".into(),
            },
            NotificationTarget::Messenger {
                recipient_id: "1234567890".into(),
            },
            NotificationTarget::Instagram {
                recipient_id: "17841400000000000".into(),
            },
            NotificationTarget::Line {
                user_id: "U1234567890abcdef".into(),
            },
            NotificationTarget::Twilio {
                phone_number: "+15551234567".into(),
            },
        ];
        let type_tags: Vec<String> = variants
            .iter()
            .map(|v| {
                let j = serde_json::to_value(v).unwrap();
                j["type"].as_str().unwrap().to_string()
            })
            .collect();
        let unique: std::collections::HashSet<_> = type_tags.iter().collect();
        assert_eq!(
            unique.len(),
            variants.len(),
            "duplicate type tags: {:?}",
            type_tags
        );
    }
}
