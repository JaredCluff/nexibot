//! Facebook Messenger integration via Meta Graph API.
//!
//! Receives messages via the Meta webhook system (POST /api/messenger/webhook)
//! and replies using the Send API (POST /v19.0/me/messages).
//!
//! Security:
//!   - GET /api/messenger/webhook  — webhook verification (hub.challenge)
//!   - POST /api/messenger/webhook — HMAC-SHA256 signature verified via
//!     `X-Hub-Signature-256` before any processing occurs
//!
//! The raw request body is read first for signature verification; the
//! JSON payload is then deserialized from those bytes.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::Bytes,
    extract::{Query, State as AxumState},
    http::{HeaderMap, StatusCode},
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use lru::LruCache;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::commands::AppState;
use crate::router::{self, IncomingMessage, RouteOptions, RouterError};
use crate::session_overrides::SessionOverrides;
use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
use crate::tool_loop::ToolLoopConfig;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the Facebook Messenger integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessengerConfig {
    /// Whether the Messenger integration is enabled.
    pub enabled: bool,
    /// Page Access Token from Meta Developer Console.
    #[serde(default)]
    pub page_access_token: String,
    /// Verify token — set this in the Meta App Dashboard webhook configuration.
    #[serde(default)]
    pub verify_token: String,
    /// App Secret — used to verify `X-Hub-Signature-256` on incoming webhooks.
    #[serde(default)]
    pub app_secret: String,
    /// Allowlist of Facebook sender PSIDs. Empty = allow all when dm_policy is Allowlist.
    #[serde(default)]
    pub allowed_sender_ids: Vec<String>,
    /// Sender PSIDs with admin privileges (bypass DM policy).
    #[serde(default)]
    pub admin_sender_ids: Vec<String>,
    /// DM policy controlling who may interact.
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy.
    #[serde(default)]
    pub tool_policy: crate::config::ChannelToolPolicy,
}

impl Default for MessengerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            page_access_token: String::new(),
            verify_token: String::new(),
            app_secret: String::new(),
            allowed_sender_ids: Vec::new(),
            admin_sender_ids: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-sender session for Messenger conversations.
pub(crate) struct MessengerChatSession {
    claude_client: ClaudeClient,
    last_activity: Instant,
}

/// Shared state for the Messenger webhook handler.
pub struct MessengerState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, MessengerChatSession>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for Messenger tool execution flow, including in-channel approvals.
pub(crate) struct MessengerObserver {
    app_state: AppState,
    requester_sender_id: String,
    has_send_config: bool,
    pending_approvals: Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
}

impl MessengerObserver {
    pub(crate) fn new(
        app_state: AppState,
        requester_sender_id: String,
        has_send_config: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            requester_sender_id,
            has_send_config,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for MessengerObserver {
    fn supports_approval(&self) -> bool {
        self.has_send_config && !self.requester_sender_id.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = self.requester_sender_id.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                send_messenger_message(
                    &self.app_state,
                    &self.requester_sender_id,
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
        if !send_messenger_message_checked(&self.app_state, &self.requester_sender_id, &prompt)
            .await
        {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                send_messenger_message(
                    &self.app_state,
                    &self.requester_sender_id,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

impl MessengerState {
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

    async fn get_or_create_client(&self, sender_id: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(sender_id) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            sender_id.to_string(),
            MessengerChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

// ---------------------------------------------------------------------------
// Webhook verification query parameters
// ---------------------------------------------------------------------------

/// Query parameters for the Meta webhook verification GET request.
#[derive(Debug, Deserialize)]
pub struct MessengerVerifyQuery {
    #[serde(rename = "hub.mode")]
    pub hub_mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    pub hub_verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    pub hub_challenge: Option<String>,
}

// ---------------------------------------------------------------------------
// Incoming webhook deserialization
// ---------------------------------------------------------------------------

/// Top-level Messenger webhook payload.
#[derive(Debug, Deserialize)]
struct MessengerWebhookPayload {
    object: Option<String>,
    entry: Option<Vec<MessengerEntry>>,
}

#[derive(Debug, Deserialize)]
struct MessengerEntry {
    messaging: Option<Vec<MessengerEvent>>,
}

#[derive(Debug, Deserialize)]
struct MessengerEvent {
    sender: Option<MessengerId>,
    #[allow(dead_code)]
    recipient: Option<MessengerId>,
    message: Option<MessengerMessage>,
}

#[derive(Debug, Deserialize)]
struct MessengerId {
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessengerMessage {
    mid: Option<String>,
    text: Option<String>,
    /// True if this is an echo of the page's own outbound message.
    #[serde(default)]
    is_echo: bool,
}

// ---------------------------------------------------------------------------
// Webhook handlers
// ---------------------------------------------------------------------------

/// Handle Meta webhook verification (GET /api/messenger/webhook).
pub async fn messenger_verify_handler(
    AxumState(state): AxumState<Arc<MessengerState>>,
    Query(query): Query<MessengerVerifyQuery>,
) -> Result<String, (StatusCode, String)> {
    let (verify_token, enabled) = {
        let config = state.app_state.config.read().await;
        (
            state
                .app_state
                .key_interceptor
                .restore_config_string(&config.messenger.verify_token),
            config.messenger.enabled,
        )
    };

    if !enabled {
        return Err((StatusCode::NOT_FOUND, "Not found".to_string()));
    }

    if verify_token.is_empty() {
        warn!("[MESSENGER] verify_token not configured — rejecting verification request");
        return Err((StatusCode::FORBIDDEN, "Verification failed".to_string()));
    }

    if query.hub_mode.as_deref() == Some("subscribe")
        && query
            .hub_verify_token
            .as_deref()
            .map(|t| crate::security::constant_time::secure_compare(t, &verify_token))
            .unwrap_or(false)
    {
        info!("[MESSENGER] Webhook verified");
        Ok(query.hub_challenge.unwrap_or_default())
    } else {
        warn!("[MESSENGER] Webhook verification failed");
        Err((StatusCode::FORBIDDEN, "Verification failed".to_string()))
    }
}

/// Handle incoming Messenger webhook events (POST /api/messenger/webhook).
///
/// Reads raw bytes first to verify the HMAC-SHA256 signature before
/// deserializing the JSON payload.
pub async fn messenger_webhook_handler(
    AxumState(state): AxumState<Arc<MessengerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let (app_secret, enabled) = {
        let config = state.app_state.config.read().await;
        (
            state
                .app_state
                .key_interceptor
                .restore_config_string(&config.messenger.app_secret),
            config.messenger.enabled,
        )
    };

    if !enabled {
        return StatusCode::NOT_FOUND;
    }

    if app_secret.is_empty() {
        warn!("[MESSENGER] app_secret not configured — rejecting webhook");
        return StatusCode::UNAUTHORIZED;
    }

    // Signature verification MUST happen before parsing the body
    if !verify_meta_signature(&body, &app_secret, &headers) {
        warn!("[MESSENGER] Invalid X-Hub-Signature-256 — rejecting request");
        return StatusCode::UNAUTHORIZED;
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!("[MESSENGER] Failed to parse webhook body: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = process_messenger_payload(state, payload).await {
            error!("[MESSENGER] Error processing payload: {}", e);
        }
    });

    StatusCode::OK
}

/// Verify the Meta `X-Hub-Signature-256: sha256=<hex>` header.
fn verify_meta_signature(body: &[u8], app_secret: &str, headers: &HeaderMap) -> bool {
    let signature_header = headers
        .get("X-Hub-Signature-256")
        .or_else(|| headers.get("x-hub-signature-256"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let hex_signature = signature_header.strip_prefix("sha256=").unwrap_or("");
    if hex_signature.is_empty() {
        return false;
    }

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = match HmacSha256::new_from_slice(app_secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let computed = hex::encode(mac.finalize().into_bytes());

    crate::security::constant_time::secure_compare(&computed, hex_signature)
}

/// Parse and dispatch a Messenger webhook payload.
async fn process_messenger_payload(
    state: Arc<MessengerState>,
    payload: serde_json::Value,
) -> Result<(), String> {
    let webhook: MessengerWebhookPayload = serde_json::from_value(payload)
        .map_err(|e| format!("Failed to parse Messenger payload: {}", e))?;

    // Only handle page events
    if webhook.object.as_deref() != Some("page") {
        return Ok(());
    }

    for entry in webhook.entry.iter().flatten() {
        for event in entry.messaging.iter().flatten() {
            let sender_id = event
                .sender
                .as_ref()
                .and_then(|s| s.id.as_deref())
                .unwrap_or("");

            if sender_id.is_empty() {
                continue;
            }

            let message = match &event.message {
                Some(m) => m,
                None => continue,
            };

            // Skip echoes (the page's own outbound messages)
            if message.is_echo {
                continue;
            }

            let text = match message.text.as_deref() {
                Some(t) if !t.trim().is_empty() => t.trim(),
                _ => continue,
            };

            // Authorization checks
            let (allowed_sender_ids, admin_sender_ids, dm_policy, enabled) = {
                let config = state.app_state.config.read().await;
                (
                    config.messenger.allowed_sender_ids.clone(),
                    config.messenger.admin_sender_ids.clone(),
                    config.messenger.dm_policy,
                    config.messenger.enabled,
                )
            };

            if !enabled {
                continue;
            }

            // --- Message deduplication using Meta message ID ---
            let msg_id = message
                .mid
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{}:{}", sender_id, text));
            {
                let mut dedup = state.msg_dedup.lock().await;
                if dedup.put(msg_id.clone(), ()).is_some() {
                    debug!("[MESSENGER] Dropping duplicate message: {}", msg_id);
                    continue;
                }
            }

            // --- Per-sender rate limiting ---
            let rate_key = format!("messenger:{}", sender_id);
            if state.rate_limiter.check(&rate_key).is_err() {
                warn!("[MESSENGER] Rate limit exceeded for sender {} — dropping message", sender_id);
                continue;
            }

            let is_admin =
                !admin_sender_ids.is_empty() && admin_sender_ids.contains(&sender_id.to_string());

            if !is_admin {
                match dm_policy {
                    crate::pairing::DmPolicy::Allowlist => {
                        if !allowed_sender_ids.is_empty()
                            && !allowed_sender_ids.contains(&sender_id.to_string())
                        {
                            info!(
                                "[MESSENGER] Ignoring message from unauthorized sender: {}",
                                sender_id
                            );
                            continue;
                        }
                    }
                    crate::pairing::DmPolicy::Open => {}
                    crate::pairing::DmPolicy::Pairing => {
                        let pairing_mgr = state.app_state.pairing_manager.read().await;
                        let is_allowed = pairing_mgr.is_channel_allowed(
                            "messenger",
                            sender_id,
                            &allowed_sender_ids,
                        );
                        drop(pairing_mgr);

                        if !is_allowed {
                            let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                            match pairing_mgr.create_pairing_request("messenger", sender_id, None) {
                                Ok(code) => {
                                    drop(pairing_mgr);
                                    send_messenger_message(
                                        &state.app_state,
                                        sender_id,
                                        &format!(
                                            "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                            code
                                        ),
                                    )
                                    .await;
                                }
                                Err(e) => {
                                    drop(pairing_mgr);
                                    send_messenger_message(
                                        &state.app_state,
                                        sender_id,
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

            let text_lc = text.trim().to_lowercase();
            if matches!(
                text_lc.as_str(),
                "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
            ) {
                let approval_tx = state
                    .app_state
                    .messenger_pending_approvals
                    .lock()
                    .await
                    .remove(sender_id);
                if let Some(approval_tx) = approval_tx {
                    let approved = matches!(text_lc.as_str(), "approve" | "/approve" | "!approve");
                    let _ = approval_tx.send(approved);
                    let reply = if approved {
                        "Approved. Continuing..."
                    } else {
                        "Denied."
                    };
                    send_messenger_message(&state.app_state, sender_id, reply).await;
                    continue;
                }
            }

            info!(
                "[MESSENGER] Message from {}: {}",
                sender_id,
                &text[..text.len().min(80)]
            );

            let state_clone = state.clone();
            let sender_owned = sender_id.to_string();
            let text_owned = text.to_string();
            tokio::spawn(async move {
                handle_messenger_text_message(&state_clone, &sender_owned, &text_owned).await;
            });
        }
    }

    Ok(())
}

/// Route a Messenger text message through the Claude pipeline.
async fn handle_messenger_text_message(state: &MessengerState, sender_id: &str, text: &str) {
    let app_state = &state.app_state;

    let claude_client = state.get_or_create_client(sender_id).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::Messenger {
            sender_id: sender_id.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_send_config = {
        let config = app_state.config.read().await;
        !config.messenger.page_access_token.trim().is_empty()
    };

    let observer = MessengerObserver::new(
        app_state.clone(),
        sender_id.to_string(),
        has_send_config,
        app_state.messenger_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::messenger(sender_id.to_string()),
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
                send_messenger_message(app_state, sender_id, "(No response)").await;
            } else {
                for chunk in router::split_message(&response, 2000) {
                    send_messenger_message(app_state, sender_id, &chunk).await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            send_messenger_message(app_state, sender_id, &msg).await;
        }
        Err(e) => {
            send_messenger_message(app_state, sender_id, &format!("Error: {}", e)).await;
        }
    }
}

/// Send a text message to a Messenger recipient via the Meta Graph API.
pub async fn send_messenger_message(app_state: &AppState, recipient_id: &str, text: &str) {
    let _ = send_messenger_message_checked(app_state, recipient_id, text).await;
}

async fn send_messenger_message_checked(
    app_state: &AppState,
    recipient_id: &str,
    text: &str,
) -> bool {
    let page_access_token = {
        let config = app_state.config.read().await;
        app_state
            .key_interceptor
            .restore_config_string(&config.messenger.page_access_token)
    };

    if page_access_token.is_empty() {
        error!("[MESSENGER] Cannot send: page_access_token not configured");
        return false;
    }

    let url = "https://graph.facebook.com/v19.0/me/messages";

    let body = serde_json::json!({
        "recipient": { "id": recipient_id },
        "message": { "text": text },
    });

    let client = reqwest::Client::new();
    match client
        .post(url)
        .bearer_auth(page_access_token)
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            info!("[MESSENGER] Message sent to {}", recipient_id);
            true
        }
        Ok(resp) => {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            error!("[MESSENGER] Send failed ({}): {}", status, body_text);
            false
        }
        Err(e) => {
            error!("[MESSENGER] HTTP error sending message: {}", e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum number of concurrent Messenger chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale Messenger sessions (>24h inactive).
pub async fn session_cleanup_loop(state: Arc<MessengerState>) {
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
                "[MESSENGER] Cleaned up {} stale sessions ({} remaining)",
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
                "[MESSENGER] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}

/// Start background tasks for the Messenger integration.
///
/// The webhook handlers are mounted externally (api_server.rs / webhooks.rs).
pub async fn start_messenger(app_state: AppState) -> Result<(), String> {
    let enabled = {
        let config = app_state.config.read().await;
        config.messenger.enabled
    };
    if !enabled {
        return Ok(());
    }

    info!("[MESSENGER] Integration enabled — webhook handlers should be mounted at /api/messenger/webhook");
    Ok(())
}
