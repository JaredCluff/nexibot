//! WhatsApp Cloud API integration for NexiBot.
//!
//! Receives messages via webhook (shared with the existing Axum server),
//! routes them through the Claude pipeline, and sends responses back
//! via the WhatsApp Cloud API.

use axum::{
    body::Bytes,
    extract::{Query, State as AxumState},
    http::HeaderMap,
    http::StatusCode,
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::commands::AppState;
use crate::router::{self, IncomingMessage, RouteOptions, RouterError};
use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::ToolLoopConfig;

/// Per-phone session state for WhatsApp conversations.
pub(crate) struct WhatsAppChatSession {
    /// Dedicated Claude client with its own conversation history
    claude_client: ClaudeClient,
    /// Last activity timestamp
    last_activity: Instant,
}

/// Shared state for WhatsApp message handling.
pub struct WhatsAppState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, WhatsAppChatSession>>,
    /// Per-sender rate limiter (10 messages per 60 seconds, 30-second lockout)
    rate_limiter: Arc<RateLimiter>,
    /// Recently-processed message IDs for deduplication
    msg_dedup: Mutex<LruCache<String, ()>>,
    /// Per-sender mutex serializing concurrent LLM calls for the same contact.
    chat_llm_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl WhatsAppState {
    pub fn new(app_state: AppState) -> Self {
        Self {
            app_state,
            chat_sessions: RwLock::new(HashMap::new()),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig {
                max_attempts: 10,
                window_seconds: 60,
                lockout_seconds: 30,
            })),
            msg_dedup: Mutex::new(LruCache::new(NonZeroUsize::new(10_000).unwrap())),
            chat_llm_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the per-sender mutex for serializing LLM calls.
    async fn chat_lock(&self, from: &str) -> Arc<Mutex<()>> {
        let mut map = self.chat_llm_locks.lock().await;
        map.entry(from.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Get or create a Claude client for the given phone number.
    async fn get_or_create_client(&self, phone: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(phone) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            phone.to_string(),
            WhatsAppChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

/// Observer for WhatsApp tool execution flow, including in-channel approvals.
pub(crate) struct WhatsAppObserver {
    app_state: AppState,
    sender_phone: String,
    has_send_config: bool,
    pending_approvals: Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
}

impl WhatsAppObserver {
    pub(crate) fn new(
        app_state: AppState,
        sender_phone: String,
        has_send_config: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            sender_phone,
            has_send_config,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for WhatsAppObserver {
    fn supports_approval(&self) -> bool {
        self.has_send_config && !self.sender_phone.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = self.sender_phone.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                send_whatsapp_message(
                    &self.app_state,
                    &self.sender_phone,
                    "Another approval is already pending for this sender. Denying this request.",
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
        if !send_whatsapp_message_checked(&self.app_state, &self.sender_phone, &prompt).await {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                send_whatsapp_message(
                    &self.app_state,
                    &self.sender_phone,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

// --- Webhook verification (GET /whatsapp/webhook) ---

#[derive(Deserialize)]
pub struct VerifyQuery {
    #[serde(rename = "hub.mode")]
    pub hub_mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    pub hub_verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    pub hub_challenge: Option<String>,
}

/// Handle Meta webhook verification (GET request).
pub async fn whatsapp_verify_handler(
    AxumState(state): AxumState<Arc<WhatsAppState>>,
    Query(query): Query<VerifyQuery>,
) -> Result<String, (StatusCode, String)> {
    let config = state.app_state.config.read().await;
    let enabled = config.whatsapp.enabled;
    let verify_token = state
        .app_state
        .key_interceptor
        .restore_config_string(&config.whatsapp.verify_token);
    drop(config);

    if !enabled {
        return Err((StatusCode::NOT_FOUND, "Not found".to_string()));
    }

    if verify_token.is_empty() {
        warn!("[WHATSAPP] verify_token not configured — rejecting verification request");
        return Err((StatusCode::FORBIDDEN, "Verification failed".to_string()));
    }

    if query.hub_mode.as_deref() == Some("subscribe")
        && query
            .hub_verify_token
            .as_deref()
            .map(|t| crate::security::constant_time::secure_compare(t, &verify_token))
            .unwrap_or(false)
    {
        info!("[WHATSAPP] Webhook verified");
        Ok(query.hub_challenge.unwrap_or_default())
    } else {
        warn!("[WHATSAPP] Webhook verification failed");
        Err((StatusCode::FORBIDDEN, "Verification failed".to_string()))
    }
}

// --- Incoming message webhook (POST /whatsapp/webhook) ---

/// Handle incoming WhatsApp webhook messages (POST request).
/// Returns 200 immediately; processes async.
pub async fn whatsapp_webhook_handler(
    AxumState(state): AxumState<Arc<WhatsAppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let (enabled, app_secret) = {
        let config = state.app_state.config.read().await;
        (
            config.whatsapp.enabled,
            state
                .app_state
                .key_interceptor
                .restore_config_string(&config.whatsapp.app_secret),
        )
    };

    if !enabled {
        return StatusCode::NOT_FOUND;
    }

    if app_secret.is_empty() {
        warn!("[WHATSAPP] app_secret not configured — rejecting webhook");
        return StatusCode::UNAUTHORIZED;
    }

    if !verify_meta_signature(&body, &app_secret, &headers) {
        warn!("[WHATSAPP] Invalid X-Hub-Signature-256 — rejecting request");
        return StatusCode::UNAUTHORIZED;
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!("[WHATSAPP] Failed to parse webhook body: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = process_whatsapp_payload(state, payload).await {
            error!("[WHATSAPP] Error processing payload: {}", e);
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

/// Parse and process the WhatsApp Cloud API webhook payload.
async fn process_whatsapp_payload(
    state: Arc<WhatsAppState>,
    payload: serde_json::Value,
) -> Result<(), String> {
    // WhatsApp Cloud API structure:
    // { "entry": [{ "changes": [{ "value": { "messages": [...], "metadata": {...} } }] }] }
    let entries = payload
        .get("entry")
        .and_then(|v| v.as_array())
        .ok_or("Missing entry array")?;

    let empty_vec = vec![];
    for entry in entries {
        let changes = entry
            .get("changes")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty_vec);

        for change in changes {
            let value = match change.get("value") {
                Some(v) => v,
                None => continue,
            };

            let messages = value
                .get("messages")
                .and_then(|v| v.as_array())
                .unwrap_or(&empty_vec);

            for message in messages {
                let msg_type = message.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let from = message.get("from").and_then(|v| v.as_str()).unwrap_or("");

                if from.is_empty() {
                    continue;
                }

                // Authorization check based on DM policy
                {
                    let config = state.app_state.config.read().await;
                    let enabled = config.whatsapp.enabled;
                    let dm_policy = config.whatsapp.dm_policy;
                    let allowed = config.whatsapp.allowed_phone_numbers.clone();
                    let admins = config.whatsapp.admin_phone_numbers.clone();
                    drop(config);

                    if !enabled {
                        continue;
                    }

                    // Admin bypass: admin phone numbers skip DM policy
                    let is_admin = !admins.is_empty() && admins.contains(&from.to_string());

                    if !is_admin {
                        match dm_policy {
                            crate::pairing::DmPolicy::Allowlist => {
                                if !allowed.is_empty() && !allowed.contains(&from.to_string()) {
                                    info!(
                                        "[WHATSAPP] Ignoring message from unauthorized number: {}",
                                        from
                                    );
                                    continue;
                                }
                            }
                            crate::pairing::DmPolicy::Open => {}
                            crate::pairing::DmPolicy::Pairing => {
                                let pairing_mgr = state.app_state.pairing_manager.read().await;
                                if !pairing_mgr.is_whatsapp_allowed(from, &allowed) {
                                    drop(pairing_mgr);
                                    // Generate pairing code
                                    let mut pairing_mgr =
                                        state.app_state.pairing_manager.write().await;
                                    match pairing_mgr.create_pairing_request("whatsapp", from, None)
                                    {
                                        Ok(code) => {
                                            drop(pairing_mgr);
                                            send_whatsapp_message(
                                                &state.app_state,
                                                from,
                                                &format!(
                                                    "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                                    code
                                                ),
                                            ).await;
                                        }
                                        Err(e) => {
                                            drop(pairing_mgr);
                                            send_whatsapp_message(
                                                &state.app_state,
                                                from,
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

                // Message deduplication using message id + sender as dedup key
                {
                    let msg_id = message.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    if !msg_id.is_empty() {
                        let dedup_key = format!("{}:{}", from, msg_id);
                        let mut dedup = state.msg_dedup.lock().await;
                        if dedup.put(dedup_key, ()).is_some() {
                            debug!("[WHATSAPP] Duplicate message from {}, skipping", from);
                            continue;
                        }
                    }
                }

                // Per-sender rate limiting
                {
                    let rate_key = format!("whatsapp:{}", from);
                    if let Err(e) = state.rate_limiter.check(&rate_key) {
                        warn!("[WHATSAPP] Rate limit hit for user {}: {}", from, e);
                        send_whatsapp_message(
                            &state.app_state,
                            from,
                            "Too many messages. Please wait a moment.",
                        )
                        .await;
                        continue;
                    }
                }

                if msg_type == "text" {
                    let text = message
                        .get("text")
                        .and_then(|v| v.get("body"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if !text.is_empty() {
                        let text_lc = text.trim().to_lowercase();
                        if matches!(
                            text_lc.as_str(),
                            "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
                        ) {
                            let approval_tx = state
                                .app_state
                                .whatsapp_pending_approvals
                                .lock()
                                .await
                                .remove(from);
                            if let Some(approval_tx) = approval_tx {
                                let approved =
                                    matches!(text_lc.as_str(), "approve" | "/approve" | "!approve");
                                let _ = approval_tx.send(approved);
                                let reply = if approved {
                                    "Approved. Continuing..."
                                } else {
                                    "Denied."
                                };
                                send_whatsapp_message(&state.app_state, from, reply).await;
                                continue;
                            }
                        }
                        let chat_lock = state.chat_lock(from).await;
                        let _chat_guard = chat_lock.lock().await;
                        handle_whatsapp_text_message(&state, from, text).await;
                    }
                }
                // Future: handle voice, image, etc.
            }
        }
    }

    Ok(())
}

/// Process a text message through the Claude pipeline and send the response.
async fn handle_whatsapp_text_message(state: &WhatsAppState, from: &str, text: &str) {
    let app_state = &state.app_state;
    info!("[WHATSAPP] Message from {}: {}", from, text);

    let claude_client = state.get_or_create_client(from).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::WhatsApp {
            phone_number: from.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_send_config = {
        let config = app_state.config.read().await;
        !config.whatsapp.phone_number_id.trim().is_empty()
            && !config.whatsapp.access_token.trim().is_empty()
    };

    let observer = WhatsAppObserver::new(
        app_state.clone(),
        from.to_string(),
        has_send_config,
        app_state.whatsapp_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::whatsapp(from.to_string()),
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
                send_whatsapp_message(app_state, from, "(No response)").await;
            } else {
                for chunk in router::split_message(&response, 4096) {
                    send_whatsapp_message(app_state, from, &chunk).await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            send_whatsapp_message(app_state, from, &msg).await;
        }
        Err(e) => {
            send_whatsapp_message(app_state, from, &format!("Error: {}", e)).await;
        }
    }
}

/// Send a text message via WhatsApp Cloud API.
async fn send_whatsapp_message(app_state: &AppState, to: &str, text: &str) {
    let _ = send_whatsapp_message_checked(app_state, to, text).await;
}

async fn send_whatsapp_message_checked(app_state: &AppState, to: &str, text: &str) -> bool {
    let (phone_number_id, access_token) = {
        let config = app_state.config.read().await;
        (
            config.whatsapp.phone_number_id.clone(),
            app_state
                .key_interceptor
                .restore_config_string(&config.whatsapp.access_token),
        )
    };

    if phone_number_id.is_empty() || access_token.is_empty() {
        error!("[WHATSAPP] Cannot send message: phone_number_id or access_token not configured");
        return false;
    }

    let url = format!(
        "https://graph.facebook.com/v21.0/{}/messages",
        phone_number_id
    );

    let body = serde_json::json!({
        "messaging_product": "whatsapp",
        "to": to,
        "type": "text",
        "text": { "body": text }
    });

    let client = reqwest::Client::new();
    match client
        .post(&url)
        .bearer_auth(&access_token)
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            info!("[WHATSAPP] Message sent to {}", to);
            true
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            error!("[WHATSAPP] Send failed ({}): {}", status, body);
            false
        }
        Err(e) => {
            error!("[WHATSAPP] HTTP error sending message: {}", e);
            false
        }
    }
}

/// Maximum number of concurrent WhatsApp chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale WhatsApp chat sessions (>24h inactive).
pub async fn session_cleanup_loop(state: Arc<WhatsAppState>) {
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
                "[WHATSAPP] Cleaned up {} stale sessions ({} remaining)",
                removed,
                sessions.len()
            );
        }

        // Evict oldest sessions if still over the hard cap
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
                "[WHATSAPP] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}
