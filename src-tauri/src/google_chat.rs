//! Google Chat / Workspace integration for NexiBot.
//!
//! Uses a simplified webhook approach:
//!   - Incoming messages arrive via HTTP POST to `/api/google-chat/events`
//!   - A `verification_token` header guards the endpoint (or HMAC-SHA256 via `hmac_secret`)
//!   - Outbound messages are sent to the configured `incoming_webhook_url`
//!
//! NOTE: Full service-account OAuth2 integration (for subscribing to space events
//! and posting as a proper bot) is deferred to Phase 2. This implementation covers
//! the common "add the bot as a webhook and enable Events" pattern available in
//! Google Workspace environments.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::Bytes,
    extract::State as AxumState,
    http::{HeaderMap, StatusCode},
};
use hmac::{Hmac, Mac};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::commands::AppState;
use crate::router::{self, IncomingMessage, RouteOptions, RouterError};
use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::ToolLoopConfig;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the Google Chat integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatConfig {
    /// Whether the Google Chat integration is enabled.
    pub enabled: bool,
    /// Webhook URL for sending messages to a Google Chat Space.
    /// Obtain from Space Settings → Manage webhooks.
    #[serde(default)]
    pub incoming_webhook_url: String,
    /// HMAC-SHA256 secret for verifying the `X-Goog-Signature` header.
    /// Preferred over `verification_token` — set this for proper request signing.
    #[serde(default)]
    pub hmac_secret: String,
    /// Legacy static token for `X-Goog-Verification-Token` header checks.
    /// Used only when `hmac_secret` is not configured.
    #[serde(default)]
    pub verification_token: String,
    /// Allowlist of Google Chat space resource names (e.g. "spaces/AAAA...").
    /// Empty = infer a single allowed space from `incoming_webhook_url`.
    /// If no space can be inferred, inbound events are rejected.
    #[serde(default)]
    pub allowed_spaces: Vec<String>,
    /// User resource names that bypass DM policy (e.g. "users/123456789").
    #[serde(default)]
    pub admin_user_ids: Vec<String>,
    /// DM policy controlling who may interact.
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy.
    #[serde(default)]
    pub tool_policy: crate::config::ChannelToolPolicy,
}

impl Default for GoogleChatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            incoming_webhook_url: String::new(),
            hmac_secret: String::new(),
            verification_token: String::new(),
            allowed_spaces: Vec::new(),
            admin_user_ids: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-conversation session for Google Chat.
/// Keyed by `{space_id}/{sender_id}` to give each user their own history.
pub(crate) struct GoogleChatSession {
    claude_client: ClaudeClient,
    last_activity: Instant,
}

/// Shared state for the Google Chat webhook handler.
pub struct GoogleChatState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, GoogleChatSession>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for Google Chat tool execution flow, including in-channel approvals.
pub(crate) struct GoogleChatObserver {
    app_state: AppState,
    space_id: String,
    requester_user_id: String,
    has_send_config: bool,
    pending_approvals:
        Arc<tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>>,
}

impl GoogleChatObserver {
    pub(crate) fn new(
        app_state: AppState,
        space_id: String,
        requester_user_id: String,
        has_send_config: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            space_id,
            requester_user_id,
            has_send_config,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for GoogleChatObserver {
    fn supports_approval(&self) -> bool {
        self.has_send_config
            && !self.space_id.trim().is_empty()
            && !self.requester_user_id.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = (self.space_id.clone(), self.requester_user_id.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                send_google_chat_message(
                    &self.app_state,
                    &self.space_id,
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
        if !send_google_chat_message_checked(&self.app_state, &self.space_id, &prompt).await {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                send_google_chat_message(
                    &self.app_state,
                    &self.space_id,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

impl GoogleChatState {
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

    /// Get or create a Claude client for the given session key.
    async fn get_or_create_client(&self, session_key: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(session_key) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            session_key.to_string(),
            GoogleChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

// ---------------------------------------------------------------------------
// Incoming event deserialization
// ---------------------------------------------------------------------------

/// Top-level Google Chat event envelope (simplified).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleChatEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    space: Option<GoogleChatSpace>,
    message: Option<GoogleChatMessage>,
    user: Option<GoogleChatUser>,
}

#[derive(Debug, Deserialize)]
struct GoogleChatSpace {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleChatMessage {
    /// Resource name of the message (e.g. "spaces/AAA/messages/BBB").
    /// Used for deduplication.
    name: Option<String>,
    text: Option<String>,
    sender: Option<GoogleChatUser>,
}

#[derive(Debug, Deserialize)]
struct GoogleChatUser {
    name: Option<String>,
}

// ---------------------------------------------------------------------------
// Webhook handlers
// ---------------------------------------------------------------------------

/// Decode a base64 string leniently (standard or URL-safe alphabet, with or without padding).
fn base64_decode_loose(s: &str) -> Result<Vec<u8>, ()> {
    use base64::{engine::general_purpose, Engine as _};
    general_purpose::STANDARD
        .decode(s)
        .or_else(|_| general_purpose::URL_SAFE.decode(s))
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(s))
        .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(s))
        .map_err(|_| ())
}

/// Handle incoming Google Chat events (POST /api/google-chat/events).
///
/// When `hmac_secret` is configured, verifies the `X-Goog-Signature` header
/// (HMAC-SHA256 over the raw request body) and rejects events with a timestamp
/// older than 5 minutes to prevent replay attacks.
///
/// Falls back to static `verification_token` comparison when `hmac_secret`
/// is not set.
///
/// Returns 200 OK immediately; message processing runs asynchronously.
pub async fn google_chat_webhook_handler(
    AxumState(state): AxumState<Arc<GoogleChatState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let (hmac_secret, verification_token) = {
        let config = state.app_state.config.read().await;
        if !config.google_chat.enabled {
            return StatusCode::NOT_FOUND;
        }
        (
            state
                .app_state
                .key_interceptor
                .restore_config_string(&config.google_chat.hmac_secret),
            state
                .app_state
                .key_interceptor
                .restore_config_string(&config.google_chat.verification_token),
        )
    };

    if !hmac_secret.is_empty() {
        // --- HMAC-SHA256 signature verification ---
        let provided_sig = headers
            .get("X-Goog-Signature")
            .or_else(|| headers.get("x-goog-signature"))
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if provided_sig.is_empty() {
            warn!("[GOOGLE_CHAT] Missing X-Goog-Signature header -- rejecting webhook");
            return StatusCode::UNAUTHORIZED;
        }

        // Decode provided signature (hex or base64).
        let provided_bytes = if let Ok(b) = hex::decode(provided_sig) {
            b
        } else if let Ok(b) = base64_decode_loose(provided_sig) {
            b
        } else {
            warn!("[GOOGLE_CHAT] X-Goog-Signature is not valid hex or base64 -- rejecting");
            return StatusCode::UNAUTHORIZED;
        };

        let mut mac = match Hmac::<Sha256>::new_from_slice(hmac_secret.as_bytes()) {
            Ok(m) => m,
            Err(e) => {
                error!("[GOOGLE_CHAT] Failed to create HMAC instance: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        };
        mac.update(&body);
        if mac.verify_slice(&provided_bytes).is_err() {
            warn!("[GOOGLE_CHAT] HMAC-SHA256 signature mismatch -- rejecting request");
            return StatusCode::UNAUTHORIZED;
        }

        // --- Timestamp / replay protection ---
        // Google Chat includes an `eventTime` field in the payload.
        // Reject events older than 5 minutes.
        if let Ok(payload_val) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Some(ts_str) = payload_val.get("eventTime").and_then(|v| v.as_str()) {
                if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                    let age_secs = chrono::Utc::now()
                        .signed_duration_since(ts.with_timezone(&chrono::Utc))
                        .num_seconds()
                        .abs();
                    if age_secs > 300 {
                        warn!(
                            "[GOOGLE_CHAT] Rejecting stale webhook event (age {}s > 300s)",
                            age_secs
                        );
                        return StatusCode::UNAUTHORIZED;
                    }
                }
            }
        }
    } else if !verification_token.is_empty() {
        // --- Legacy static token fallback ---
        let provided = headers
            .get("X-Goog-Verification-Token")
            .or_else(|| headers.get("x-goog-verification-token"))
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !crate::security::constant_time::secure_compare(provided, &verification_token) {
            warn!("[GOOGLE_CHAT] Verification token mismatch -- rejecting request");
            return StatusCode::UNAUTHORIZED;
        }
    } else {
        warn!("[GOOGLE_CHAT] Neither hmac_secret nor verification_token configured -- rejecting webhook");
        return StatusCode::UNAUTHORIZED;
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!("[GOOGLE_CHAT] Failed to parse JSON body: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = process_google_chat_event(state, payload).await {
            error!("[GOOGLE_CHAT] Error processing event: {}", e);
        }
    });

    StatusCode::OK
}

/// Parse and process a Google Chat event payload.
async fn process_google_chat_event(
    state: Arc<GoogleChatState>,
    payload: serde_json::Value,
) -> Result<(), String> {
    let event: GoogleChatEvent =
        serde_json::from_value(payload).map_err(|e| format!("Failed to parse event: {}", e))?;

    // Only process MESSAGE events
    if event.event_type.as_deref() != Some("MESSAGE") {
        return Ok(());
    }

    let space_id = event
        .space
        .as_ref()
        .and_then(|s| s.name.as_deref())
        .unwrap_or("")
        .to_string();

    let message = match &event.message {
        Some(m) => m,
        None => return Ok(()),
    };

    let text = message.text.as_deref().unwrap_or("").trim();
    if text.is_empty() {
        return Ok(());
    }

    // Sender can come from message.sender or event.user
    let sender_id = message
        .sender
        .as_ref()
        .and_then(|s| s.name.as_deref())
        .or_else(|| event.user.as_ref().and_then(|u| u.name.as_deref()))
        .unwrap_or("")
        .to_string();

    // Authorization checks
    let (allowed_spaces, admin_user_ids, dm_policy, enabled, incoming_webhook_url) = {
        let config = state.app_state.config.read().await;
        (
            config.google_chat.allowed_spaces.clone(),
            config.google_chat.admin_user_ids.clone(),
            config.google_chat.dm_policy,
            config.google_chat.enabled,
            state
                .app_state
                .key_interceptor
                .restore_config_string(&config.google_chat.incoming_webhook_url),
        )
    };

    if !enabled {
        return Ok(());
    }

    // --- Message deduplication ---
    let msg_id = message
        .name
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}:{}:{}", space_id, sender_id, text));
    {
        let mut dedup = state.msg_dedup.lock().await;
        if dedup.put(msg_id.clone(), ()).is_some() {
            debug!("[GOOGLE_CHAT] Dropping duplicate message: {}", msg_id);
            return Ok(());
        }
    }

    // --- Per-sender rate limiting ---
    let rate_key = format!("google_chat:{}", sender_id);
    if state.rate_limiter.check(&rate_key).is_err() {
        warn!(
            "[GOOGLE_CHAT] Rate limit exceeded for sender {} -- dropping message",
            sender_id
        );
        return Ok(());
    }

    let effective_allowed_spaces = if allowed_spaces.is_empty() {
        match configured_google_chat_space_id(&incoming_webhook_url) {
            Some(space) => vec![space],
            None => {
                warn!(
                    "[GOOGLE_CHAT] allowed_spaces is empty and incoming_webhook_url does not contain a parseable space id; rejecting inbound event from {}",
                    space_id
                );
                return Ok(());
            }
        }
    } else {
        allowed_spaces
    };

    // Space allowlist check
    if space_id.is_empty() || !effective_allowed_spaces.contains(&space_id) {
        info!(
            "[GOOGLE_CHAT] Ignoring message from unauthorized space: {}",
            space_id
        );
        return Ok(());
    }

    // Admin bypass
    let is_admin = !admin_user_ids.is_empty() && admin_user_ids.contains(&sender_id);

    if !is_admin {
        match dm_policy {
            crate::pairing::DmPolicy::Allowlist => {
                // Google Chat doesn't have a per-user allowlist in this simplified mode.
                // Space allowlist above covers the primary access control.
            }
            crate::pairing::DmPolicy::Open => {}
            crate::pairing::DmPolicy::Pairing => {
                let allowed_ids: Vec<String> = vec![]; // pairing checks against runtime list
                let pairing_mgr = state.app_state.pairing_manager.read().await;
                let is_allowed =
                    pairing_mgr.is_channel_allowed("google_chat", &sender_id, &allowed_ids);
                drop(pairing_mgr);

                if !is_allowed {
                    let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                    match pairing_mgr.create_pairing_request("google_chat", &sender_id, None) {
                        Ok(code) => {
                            drop(pairing_mgr);
                            send_google_chat_message(
                                &state.app_state,
                                &space_id,
                                &format!(
                                    "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                    code
                                ),
                            )
                            .await;
                        }
                        Err(e) => {
                            drop(pairing_mgr);
                            send_google_chat_message(
                                &state.app_state,
                                &space_id,
                                &format!("Authorization pending. {}", e),
                            )
                            .await;
                        }
                    }
                    return Ok(());
                }
            }
        }
    }

    let text_lc = text.trim().to_lowercase();
    if matches!(
        text_lc.as_str(),
        "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
    ) {
        let key = (space_id.clone(), sender_id.clone());
        let (approval_tx, owner_mismatch) = {
            let mut map = state.app_state.google_chat_pending_approvals.lock().await;
            if let Some(tx) = map.remove(&key) {
                (Some(tx), false)
            } else {
                let mismatch = map
                    .keys()
                    .any(|(pending_space_id, _)| pending_space_id == &space_id);
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
            send_google_chat_message(&state.app_state, &space_id, reply).await;
            return Ok(());
        }
        if owner_mismatch {
            send_google_chat_message(
                &state.app_state,
                &space_id,
                "This approval request belongs to another user in this space.",
            )
            .await;
            return Ok(());
        }
    }

    info!(
        "[GOOGLE_CHAT] Message from {} in {}: {}",
        sender_id,
        space_id,
        &text[..text.len().min(80)]
    );

    handle_google_chat_text_message(&state, &space_id, &sender_id, text).await;
    Ok(())
}

/// Route a Google Chat text message through the Claude pipeline.
async fn handle_google_chat_text_message(
    state: &GoogleChatState,
    space_id: &str,
    sender_id: &str,
    text: &str,
) {
    let app_state = &state.app_state;

    // Session key is space+sender so each user has separate conversation history
    let session_key = format!("{}/{}", space_id, sender_id);
    let claude_client = state.get_or_create_client(&session_key).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::GoogleChat {
            space_id: space_id.to_string(),
            sender_id: sender_id.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_send_config = {
        let config = app_state.config.read().await;
        !app_state
            .key_interceptor
            .restore_config_string(&config.google_chat.incoming_webhook_url)
            .trim()
            .is_empty()
    };

    let observer = GoogleChatObserver::new(
        app_state.clone(),
        space_id.to_string(),
        sender_id.to_string(),
        has_send_config,
        app_state.google_chat_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::google_chat(space_id.to_string(), sender_id.to_string()),
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
                send_google_chat_message(app_state, space_id, "(No response)").await;
            } else {
                for chunk in router::split_message(&response, 4096) {
                    send_google_chat_message(app_state, space_id, &chunk).await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            send_google_chat_message(app_state, space_id, &msg).await;
        }
        Err(e) => {
            send_google_chat_message(app_state, space_id, &format!("Error: {}", e)).await;
        }
    }
}

/// Send a message to a Google Chat Space via the incoming webhook URL.
///
pub async fn send_google_chat_message(app_state: &AppState, space_id: &str, text: &str) {
    let _ = send_google_chat_message_checked(app_state, space_id, text).await;
}

async fn send_google_chat_message_checked(
    app_state: &AppState,
    space_id: &str,
    text: &str,
) -> bool {
    let webhook_url = {
        let config = app_state.config.read().await;
        app_state
            .key_interceptor
            .restore_config_string(&config.google_chat.incoming_webhook_url)
    };

    if webhook_url.is_empty() {
        error!("[GOOGLE_CHAT] Cannot send: incoming_webhook_url not configured");
        return false;
    }

    if let Some(configured_space) = configured_google_chat_space_id(&webhook_url) {
        if !space_id.trim().is_empty() && space_id != configured_space {
            warn!(
                "[GOOGLE_CHAT] Not sending message for {} via webhook bound to {}",
                space_id, configured_space
            );
            return false;
        }
    }

    let body = serde_json::json!({ "text": text });
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());

    match client.post(&webhook_url).json(&body).send().await {
        Ok(resp) if resp.status().is_success() => {
            info!("[GOOGLE_CHAT] Message sent via webhook");
            true
        }
        Ok(resp) => {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            error!("[GOOGLE_CHAT] Send failed ({}): {}", status, body_text);
            false
        }
        Err(e) => {
            error!("[GOOGLE_CHAT] HTTP error sending message: {}", e);
            false
        }
    }
}

fn configured_google_chat_space_id(webhook_url: &str) -> Option<String> {
    let parsed = url::Url::parse(webhook_url).ok()?;
    let segments: Vec<_> = parsed.path_segments()?.collect();
    for window in segments.windows(2) {
        if window[0] == "spaces" && !window[1].trim().is_empty() {
            return Some(format!("spaces/{}", window[1]));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum number of concurrent Google Chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale Google Chat sessions (>24h inactive).
pub async fn session_cleanup_loop(state: Arc<GoogleChatState>) {
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
                "[GOOGLE_CHAT] Cleaned up {} stale sessions ({} remaining)",
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
                "[GOOGLE_CHAT] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}

/// Start background tasks for the Google Chat integration.
///
/// Only spawns the session cleanup task; the webhook handler is mounted
/// on the HTTP server in webhooks.rs / api_server.rs.
pub async fn start_google_chat(app_state: AppState) -> Result<(), String> {
    let enabled = {
        let config = app_state.config.read().await;
        config.google_chat.enabled
    };
    if !enabled {
        return Ok(());
    }

    info!("[GOOGLE_CHAT] Integration enabled -- webhook handler should be mounted at /api/google-chat/events");
    Ok(())
}
