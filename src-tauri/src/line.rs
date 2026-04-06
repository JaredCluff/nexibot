//! LINE Messaging API integration for NexiBot.
//!
//! Receives messages via webhook (POST /api/line/webhook), verifies
//! the X-Line-Signature header using HMAC-SHA256, and sends responses
//! back via the LINE push message API.

use axum::{
    body::Bytes,
    extract::State as AxumState,
    http::{HeaderMap, StatusCode},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use lru::LruCache;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use crate::security::rate_limit::{RateLimitConfig, RateLimiter};

use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::commands::AppState;
use crate::router::{self, IncomingMessage, RouteOptions, RouterError};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::ToolLoopConfig;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the LINE Messaging API channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineConfig {
    /// Whether the LINE channel is enabled.
    pub enabled: bool,

    /// Channel access token (long-lived) for sending push messages.
    #[serde(default)]
    pub channel_access_token: String,

    /// Channel secret for HMAC-SHA256 webhook signature verification.
    #[serde(default)]
    pub channel_secret: String,

    /// Allow-list of LINE user IDs. Empty = apply dm_policy.
    #[serde(default)]
    pub allowed_user_ids: Vec<String>,

    /// Admin user IDs that bypass DM policy.
    #[serde(default)]
    pub admin_user_ids: Vec<String>,

    /// DM access policy.
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,

    /// Per-channel tool access policy.
    #[serde(default)]
    pub tool_policy: crate::config::ChannelToolPolicy,
}

impl Default for LineConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            channel_access_token: String::new(),
            channel_secret: String::new(),
            allowed_user_ids: Vec::new(),
            admin_user_ids: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-conversation/user session for LINE.
pub(crate) struct LineChatSession {
    /// Dedicated Claude client with its own conversation history.
    claude_client: ClaudeClient,
    /// Last activity timestamp for session expiry.
    last_activity: Instant,
}

/// Shared state for the LINE webhook handler.
pub struct LineState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, LineChatSession>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for LINE tool execution flow, including in-channel approvals.
pub(crate) struct LineObserver {
    app_state: AppState,
    conversation_target_id: String,
    requester_user_id: String,
    has_send_config: bool,
    pending_approvals:
        Arc<tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>>,
}

impl LineObserver {
    pub(crate) fn new(
        app_state: AppState,
        conversation_target_id: String,
        requester_user_id: String,
        has_send_config: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            conversation_target_id,
            requester_user_id,
            has_send_config,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for LineObserver {
    fn supports_approval(&self) -> bool {
        self.has_send_config
            && !self.conversation_target_id.trim().is_empty()
            && !self.requester_user_id.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = (
            self.conversation_target_id.clone(),
            self.requester_user_id.clone(),
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                send_line_message(
                    &self.app_state,
                    &self.conversation_target_id,
                    "Another approval is already pending for this requester. Denying this request.",
                )
                .await;
                return false;
            }
            map.insert(key.clone(), tx);
        }

        let prompt = format!(
            "Tool approval required\n\nTool: {}\nReason: {}\n\nReply approve to allow or deny to block (5 min timeout).",
            tool_name, reason
        );
        if !send_line_message_checked(&self.app_state, &self.conversation_target_id, &prompt).await
        {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                send_line_message(
                    &self.app_state,
                    &self.conversation_target_id,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

impl LineState {
    pub fn new(app_state: AppState) -> Self {
        Self {
            app_state,
            chat_sessions: RwLock::new(HashMap::new()),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig {
                max_attempts: 30,
                window_seconds: 60,
                lockout_seconds: 30,
            })),
            msg_dedup: Mutex::new(LruCache::new(NonZeroUsize::new(10_000).unwrap())),
        }
    }

    /// Get or create a Claude client for the given LINE conversation key.
    async fn get_or_create_client(&self, conversation_key: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(conversation_key) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            conversation_key.to_string(),
            LineChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

// ---------------------------------------------------------------------------
// Webhook handler — POST /api/line/webhook
// ---------------------------------------------------------------------------

/// Handle incoming LINE webhook events.
///
/// Reads the raw body for signature verification before parsing JSON.
pub async fn line_webhook_handler(
    AxumState(state): AxumState<Arc<LineState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // ---- Signature verification ----
    let signature_valid = {
        let config = state.app_state.config.read().await;
        if !config.line.enabled {
            return StatusCode::NOT_FOUND;
        }
        let channel_secret = state
            .app_state
            .key_interceptor
            .restore_config_string(&config.line.channel_secret);
        drop(config);

        if channel_secret.is_empty() {
            warn!("[LINE] channel_secret not configured — rejecting webhook");
            false
        } else {
            match headers.get("x-line-signature") {
                Some(sig_header) => {
                    let sig_str = match sig_header.to_str() {
                        Ok(s) => s,
                        Err(_) => {
                            warn!("[LINE] Invalid X-Line-Signature header encoding");
                            return StatusCode::UNAUTHORIZED;
                        }
                    };
                    verify_line_signature(&channel_secret, &body, sig_str)
                }
                None => {
                    warn!("[LINE] Missing X-Line-Signature header");
                    false
                }
            }
        }
    };

    if !signature_valid {
        warn!("[LINE] Webhook signature verification failed");
        return StatusCode::UNAUTHORIZED;
    }

    // ---- Parse payload ----
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!("[LINE] Failed to parse webhook JSON: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = process_line_payload(state, payload).await {
            error!("[LINE] Error processing payload: {}", e);
        }
    });

    StatusCode::OK
}

/// Verify the X-Line-Signature header: base64(HMAC-SHA256(channel_secret, body)).
fn verify_line_signature(channel_secret: &str, body: &[u8], signature: &str) -> bool {
    type HmacSha256 = Hmac<Sha256>;

    let mut mac = match HmacSha256::new_from_slice(channel_secret.as_bytes()) {
        Ok(m) => m,
        Err(e) => {
            error!("[LINE] Failed to create HMAC: {}", e);
            return false;
        }
    };
    mac.update(body);
    let computed = STANDARD.encode(mac.finalize().into_bytes());

    crate::security::constant_time::secure_compare(&computed, signature)
}

// ---------------------------------------------------------------------------
// Payload processing
// ---------------------------------------------------------------------------

/// LINE event structure (partial — only what we need).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LineEvent {
    #[serde(rename = "type")]
    event_type: String,
    source: Option<LineEventSource>,
    message: Option<LineMessage>,
    /// Webhook event ID for deduplication.
    #[serde(default)]
    webhook_event_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LineEventSource {
    #[serde(rename = "type")]
    source_type: Option<String>,
    user_id: Option<String>,
    group_id: Option<String>,
    room_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LineMessage {
    /// Message ID for deduplication fallback.
    #[serde(default)]
    id: String,
    #[serde(rename = "type")]
    message_type: String,
    text: Option<String>,
}

/// Process the events array from the LINE webhook payload.
async fn process_line_payload(
    state: Arc<LineState>,
    payload: serde_json::Value,
) -> Result<(), String> {
    let events = payload
        .get("events")
        .and_then(|v| v.as_array())
        .ok_or("Missing events array")?;

    for event_value in events {
        let event: LineEvent = match serde_json::from_value(event_value.clone()) {
            Ok(e) => e,
            Err(e) => {
                warn!("[LINE] Failed to parse event: {}", e);
                continue;
            }
        };

        if event.event_type != "message" {
            continue;
        }

        let user_id = match event.source.as_ref().and_then(|s| s.user_id.as_deref()) {
            Some(uid) => uid.to_string(),
            None => {
                warn!("[LINE] Event missing user_id, skipping");
                continue;
            }
        };
        let conversation_target_id = match event.source.as_ref() {
            Some(source) => match source.source_type.as_deref() {
                Some("group") => source
                    .group_id
                    .as_ref()
                    .filter(|id| !id.trim().is_empty())
                    .cloned(),
                Some("room") => source
                    .room_id
                    .as_ref()
                    .filter(|id| !id.trim().is_empty())
                    .cloned(),
                _ => source
                    .user_id
                    .as_ref()
                    .filter(|id| !id.trim().is_empty())
                    .cloned(),
            }
            .or_else(|| {
                source
                    .group_id
                    .as_ref()
                    .filter(|id| !id.trim().is_empty())
                    .cloned()
            })
            .or_else(|| {
                source
                    .room_id
                    .as_ref()
                    .filter(|id| !id.trim().is_empty())
                    .cloned()
            }),
            None => None,
        };
        let conversation_target_id = match conversation_target_id {
            Some(id) => id,
            None => {
                warn!("[LINE] Event missing conversation target id, skipping");
                continue;
            }
        };

        let message = match &event.message {
            Some(m) => m,
            None => continue,
        };

        if message.message_type != "text" {
            continue;
        }

        let text = match &message.text {
            Some(t) if !t.is_empty() => t.clone(),
            _ => continue,
        };

        // ---- Authorization ----
        {
            let config = state.app_state.config.read().await;
            let enabled = config.line.enabled;
            let dm_policy = config.line.dm_policy;
            let allowed = config.line.allowed_user_ids.clone();
            let admins = config.line.admin_user_ids.clone();
            drop(config);

            if !enabled {
                continue;
            }

            // --- Message deduplication ---
            let msg_id = if !event.webhook_event_id.is_empty() {
                event.webhook_event_id.clone()
            } else if !message.id.is_empty() {
                message.id.clone()
            } else {
                format!("{}:{}", user_id, text)
            };
            {
                let mut dedup = state.msg_dedup.lock().await;
                if dedup.put(msg_id.clone(), ()).is_some() {
                    debug!("[LINE] Dropping duplicate message: {}", msg_id);
                    continue;
                }
            }

            // --- Per-sender rate limiting ---
            let rate_key = format!("line:{}", user_id);
            if state.rate_limiter.check(&rate_key).is_err() {
                warn!("[LINE] Rate limit exceeded for user {} — dropping message", user_id);
                continue;
            }

            let is_admin = !admins.is_empty() && admins.contains(&user_id);

            if !is_admin {
                match dm_policy {
                    crate::pairing::DmPolicy::Allowlist => {
                        if !allowed.is_empty() && !allowed.contains(&user_id) {
                            info!(
                                "[LINE] Ignoring message from unauthorized user: {}",
                                user_id
                            );
                            continue;
                        }
                    }
                    crate::pairing::DmPolicy::Open => {}
                    crate::pairing::DmPolicy::Pairing => {
                        let pairing_mgr = state.app_state.pairing_manager.read().await;
                        if !pairing_mgr.is_channel_allowed("line", &user_id, &allowed) {
                            drop(pairing_mgr);
                            let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                            match pairing_mgr.create_pairing_request("line", &user_id, None) {
                                Ok(code) => {
                                    drop(pairing_mgr);
                                    send_line_message(
                                        &state.app_state,
                                        &conversation_target_id,
                                        &format!(
                                            "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                            code
                                        ),
                                    )
                                    .await;
                                }
                                Err(e) => {
                                    drop(pairing_mgr);
                                    send_line_message(
                                        &state.app_state,
                                        &conversation_target_id,
                                        &format!("Authorization pending. {}", e),
                                    )
                                    .await;
                                }
                            }
                            continue;
                        }
                    }
                }
            }
        }

        let text_lc = text.trim().to_lowercase();
        if matches!(
            text_lc.as_str(),
            "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
        ) {
            let key = (conversation_target_id.clone(), user_id.clone());
            let (approval_tx, owner_mismatch) = {
                let mut map = state.app_state.line_pending_approvals.lock().await;
                if let Some(tx) = map.remove(&key) {
                    (Some(tx), false)
                } else {
                    let mismatch = map.keys().any(|(pending_conversation_id, _)| {
                        pending_conversation_id == &conversation_target_id
                    });
                    (None, mismatch)
                }
            };
            if let Some(approval_tx) = approval_tx {
                let approved = matches!(text_lc.as_str(), "approve" | "/approve" | "!approve");
                let _ = approval_tx.send(approved);
                let reply = if approved {
                    "Approved. Continuing..."
                } else {
                    "Denied."
                };
                send_line_message(&state.app_state, &conversation_target_id, reply).await;
                continue;
            }
            if owner_mismatch {
                send_line_message(
                    &state.app_state,
                    &conversation_target_id,
                    "This approval request belongs to another user in this conversation.",
                )
                .await;
                continue;
            }
        }

        handle_line_text_message(&state, &conversation_target_id, &user_id, &text).await;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Message handling
// ---------------------------------------------------------------------------

/// Route a LINE text message through the Claude pipeline and push the reply.
async fn handle_line_text_message(
    state: &LineState,
    conversation_target_id: &str,
    requester_user_id: &str,
    text: &str,
) {
    let app_state = &state.app_state;
    info!(
        "[LINE] Message from {} in {}: {}",
        requester_user_id, conversation_target_id, text
    );

    let session_key = format!("{}/{}", conversation_target_id, requester_user_id);
    let claude_client = state.get_or_create_client(&session_key).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::Line {
            user_id: requester_user_id.to_string(),
            conversation_id: conversation_target_id.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_send_config = {
        let config = app_state.config.read().await;
        !app_state
            .key_interceptor
            .restore_config_string(&config.line.channel_access_token)
            .trim()
            .is_empty()
    };

    let observer = LineObserver::new(
        app_state.clone(),
        conversation_target_id.to_string(),
        requester_user_id.to_string(),
        has_send_config,
        app_state.line_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::line(
            requester_user_id.to_string(),
            conversation_target_id.to_string(),
        ),
        observer: &observer,
        streaming: false,
        window: None,
        on_stream_chunk: None,
        auto_compact: true,
        save_to_memory: true,
        sync_supermemory: true,
        check_sensitive_data: true,
    };

    match router::route_message(&message, options, app_state).await {
        Ok(routed) => {
            let response = router::extract_text_from_response(&routed.text);
            if response.is_empty() {
                send_line_message(app_state, conversation_target_id, "(No response)").await;
            } else {
                for chunk in router::split_message(&response, 5000) {
                    send_line_message(app_state, conversation_target_id, &chunk).await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            send_line_message(app_state, conversation_target_id, &msg).await;
        }
        Err(e) => {
            send_line_message(app_state, conversation_target_id, &format!("Error: {}", e)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// LINE push message API
// ---------------------------------------------------------------------------

/// Send a text message to a LINE user/group/room via the push message API.
async fn send_line_message(app_state: &AppState, target_id: &str, text: &str) {
    let _ = send_line_message_checked(app_state, target_id, text).await;
}

async fn send_line_message_checked(app_state: &AppState, target_id: &str, text: &str) -> bool {
    let access_token = {
        let config = app_state.config.read().await;
        app_state
            .key_interceptor
            .restore_config_string(&config.line.channel_access_token)
    };

    if access_token.is_empty() {
        error!("[LINE] Cannot send message: channel_access_token not configured");
        return false;
    }

    let body = serde_json::json!({
        "to": target_id,
        "messages": [
            {
                "type": "text",
                "text": text,
            }
        ]
    });

    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    match client
        .post("https://api.line.me/v2/bot/message/push")
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            info!("[LINE] Message pushed to {}", target_id);
            true
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            error!("[LINE] Push failed ({}): {}", status, body);
            false
        }
        Err(e) => {
            error!("[LINE] HTTP error pushing message: {}", e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum concurrent LINE chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically evict stale LINE chat sessions (>24 h inactive).
pub async fn session_cleanup_loop(state: Arc<LineState>) {
    let cleanup_interval = tokio::time::Duration::from_secs(3600);
    let max_age = std::time::Duration::from_secs(86400);

    loop {
        tokio::time::sleep(cleanup_interval).await;
        let mut sessions = state.chat_sessions.write().await;
        let before = sessions.len();
        sessions.retain(|_, session| session.last_activity.elapsed() < max_age);
        let removed = before - sessions.len();
        if removed > 0 {
            info!(
                "[LINE] Cleaned up {} stale sessions ({} remaining)",
                removed,
                sessions.len()
            );
        }

        if sessions.len() > MAX_CHANNEL_SESSIONS {
            let mut entries: Vec<_> = sessions
                .iter()
                .map(|(k, s)| (k.clone(), s.last_activity))
                .collect();
            entries.sort_by_key(|&(_, t)| t);
            let evict_count = sessions.len() - MAX_CHANNEL_SESSIONS;
            for (key, _) in entries.into_iter().take(evict_count) {
                sessions.remove(&key);
            }
            info!(
                "[LINE] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Start function
// ---------------------------------------------------------------------------

/// Start the LINE integration (spawns cleanup task; webhook handler is
/// registered separately in webhooks.rs).
pub async fn start_line(app_state: AppState) -> Result<(), String> {
    let config = app_state.config.read().await;
    if !config.line.enabled {
        info!("[LINE] LINE integration disabled in config");
        return Ok(());
    }
    drop(config);

    info!("[LINE] LINE integration enabled — webhook handler ready at POST /api/line/webhook");

    // The LineState is created and registered by webhooks.rs.
    // This function exists for lifecycle symmetry with other channel start_xxx functions.
    Ok(())
}
