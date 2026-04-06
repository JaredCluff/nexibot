//! Slack integration for NexiBot.
//!
//! Receives messages via the Slack Events API (webhook) and routes them
//! through the same Claude pipeline as the GUI chat. Also supports
//! Slack Socket Mode for firewall-friendly deployments.

use axum::{
    body::Bytes,
    extract::State as AxumState,
    http::{HeaderMap, StatusCode},
    Json,
};
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::commands::AppState;
use crate::pairing::DmPolicy;
use crate::router::{self, IncomingMessage, RouteOptions, RouterError};
use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::ToolLoopConfig;

/// Per-channel session state for Slack conversations.
pub(crate) struct SlackChatSession {
    /// Dedicated Claude client with its own conversation history
    claude_client: ClaudeClient,
    /// Last activity timestamp
    last_activity: Instant,
}

/// Shared state for Slack message handling.
pub struct SlackState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, SlackChatSession>>,
    /// Per-user rate limiter (10 messages per 60 seconds, 60-second lockout)
    pub rate_limiter: Arc<RateLimiter>,
    /// Recently-processed event timestamps for deduplication (event_ts+user key)
    pub msg_dedup: Mutex<LruCache<String, ()>>,
    /// Per-channel mutex serializing concurrent LLM calls for the same channel.
    chat_llm_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl SlackState {
    pub fn new(app_state: AppState) -> Self {
        Self {
            app_state,
            chat_sessions: RwLock::new(HashMap::new()),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig {
                max_attempts: 10,
                window_seconds: 60,
                lockout_seconds: 60,
            })),
            msg_dedup: Mutex::new(LruCache::new(NonZeroUsize::new(10_000).unwrap())),
            chat_llm_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the per-channel mutex for serializing LLM calls.
    async fn chat_lock(&self, channel_id: &str) -> Arc<Mutex<()>> {
        let mut map = self.chat_llm_locks.lock().await;
        map.entry(channel_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Get or create a Claude client for the given Slack channel.
    async fn get_or_create_client(&self, channel_id: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(channel_id) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            channel_id.to_string(),
            SlackChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

/// Observer for Slack tool execution flow, including in-channel approvals.
pub(crate) struct SlackObserver {
    app_state: AppState,
    channel_id: String,
    requester_user_id: String,
    has_bot_token: bool,
    pending_approvals:
        Arc<tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>>,
}

impl SlackObserver {
    pub(crate) fn new(
        app_state: AppState,
        channel_id: String,
        requester_user_id: String,
        has_bot_token: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            channel_id,
            requester_user_id,
            has_bot_token,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for SlackObserver {
    fn supports_approval(&self) -> bool {
        self.has_bot_token
            && !self.channel_id.trim().is_empty()
            && !self.requester_user_id.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = (self.channel_id.clone(), self.requester_user_id.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                send_slack_message(
                    &self.app_state,
                    &self.channel_id,
                    "⚠️ Another approval is already pending for this user. Denying this request.",
                )
                .await;
                return false;
            }
            map.insert(key.clone(), tx);
        }

        let prompt = format!(
            "🔐 Tool approval required\n\nTool: {}\nReason: {}\n\nReply `approve` to allow or `deny` to block (5 min timeout).",
            tool_name, reason
        );
        if !send_slack_message_checked(&self.app_state, &self.channel_id, &prompt).await {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                send_slack_message(
                    &self.app_state,
                    &self.channel_id,
                    "⏰ Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

/// Handle Slack Events API webhook.
///
/// Handles:
/// - URL verification challenge
/// - Event callbacks (message events)
fn verify_slack_signature(headers: &HeaderMap, body: &[u8], signing_secret: &str) -> bool {
    if signing_secret.is_empty() {
        warn!(
            "[SLACK] Signing secret is empty — rejecting request (configure slack.signing_secret)"
        );
        return false;
    }

    let timestamp = match headers
        .get("x-slack-request-timestamp")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
    {
        Some(ts) => ts,
        None => {
            warn!("[SLACK] Missing or invalid x-slack-request-timestamp");
            return false;
        }
    };

    // Reject stale requests to reduce replay attacks.
    let now = chrono::Utc::now().timestamp();
    if (now - timestamp).abs() > 300 {
        warn!("[SLACK] Stale request timestamp rejected");
        return false;
    }

    let provided_signature = match headers
        .get("x-slack-signature")
        .and_then(|v| v.to_str().ok())
    {
        Some(sig) => sig,
        None => {
            warn!("[SLACK] Missing x-slack-signature");
            return false;
        }
    };

    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let mut mac = match HmacSha256::new_from_slice(signing_secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            warn!("[SLACK] Invalid signing secret for HMAC");
            return false;
        }
    };
    mac.update(format!("v0:{}:", timestamp).as_bytes());
    mac.update(body);
    let expected = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

    crate::security::constant_time::secure_compare(&expected, provided_signature)
}

pub async fn slack_events_handler(
    AxumState(state): AxumState<Arc<SlackState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (enabled, signing_secret) = {
        let config = state.app_state.config.read().await;
        (
            config.slack.enabled,
            state
                .app_state
                .key_interceptor
                .restore_config_string(&config.slack.signing_secret),
        )
    };

    if !enabled {
        return Err((
            StatusCode::NOT_FOUND,
            "Slack integration is disabled".to_string(),
        ));
    }

    if signing_secret.is_empty() {
        warn!("[SLACK] Rejecting request: signing_secret not configured");
        return Err((
            StatusCode::UNAUTHORIZED,
            "Slack signing secret not configured".to_string(),
        ));
    }

    if !verify_slack_signature(&headers, &body, &signing_secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Invalid Slack signature".to_string(),
        ));
    }

    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid JSON body: {}", e)))?;

    // Handle URL verification challenge
    if payload.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
        let challenge = payload
            .get("challenge")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        info!("[SLACK] URL verification challenge received");
        return Ok(Json(serde_json::json!({ "challenge": challenge })));
    }

    // Handle event callbacks
    if payload.get("type").and_then(|v| v.as_str()) == Some("event_callback") {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = process_slack_event(state, payload).await {
                error!("[SLACK] Error processing event: {}", e);
            }
        });
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Process a Slack event callback.
async fn process_slack_event(
    state: Arc<SlackState>,
    payload: serde_json::Value,
) -> Result<(), String> {
    let event = payload.get("event").ok_or("Missing event field")?;

    let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

    // Only handle message events (not bot messages or edits)
    if event_type != "message" {
        return Ok(());
    }

    // Skip bot messages
    if event.get("bot_id").is_some() || event.get("subtype").is_some() {
        return Ok(());
    }

    let text = event.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let channel = event.get("channel").and_then(|v| v.as_str()).unwrap_or("");
    let user = event.get("user").and_then(|v| v.as_str()).unwrap_or("");
    let event_ts = event.get("event_ts").and_then(|v| v.as_str()).unwrap_or("");

    if text.is_empty() || channel.is_empty() || user.is_empty() {
        return Ok(());
    }

    // Fix 2: Message deduplication using event_ts + user as the dedup key
    {
        let dedup_key = format!("{}:{}", event_ts, user);
        let mut dedup = state.msg_dedup.lock().await;
        if dedup.put(dedup_key, ()).is_some() {
            // Already processed this event
            return Ok(());
        }
    }

    // Fix 1: Per-user rate limiting
    let rate_key = format!("slack:{}", user);
    if let Err(e) = state.rate_limiter.check(&rate_key) {
        warn!("[SLACK] Rate limit hit for user {}: {}", user, e);
        send_slack_message(
            &state.app_state,
            channel,
            "Please slow down — too many messages. Try again in a moment.",
        )
        .await;
        return Ok(());
    }

    // Authorization check
    {
        let config = state.app_state.config.read().await;
        let slack_config = config.slack.clone();
        drop(config);

        if !slack_config.enabled {
            return Ok(());
        }

        let is_admin = !slack_config.admin_user_ids.is_empty()
            && slack_config.admin_user_ids.contains(&user.to_string());

        if !is_admin {
            // Channel allowlist
            if !slack_config.allowed_channel_ids.is_empty()
                && !slack_config
                    .allowed_channel_ids
                    .contains(&channel.to_string())
            {
                return Ok(()); // Silently ignore
            }

            // DM policy for direct messages (channels starting with 'D')
            if channel.starts_with('D') {
                match slack_config.dm_policy {
                    DmPolicy::Allowlist => {
                        if !slack_config.admin_user_ids.is_empty()
                            && !slack_config.admin_user_ids.contains(&user.to_string())
                        {
                            send_slack_message(
                                &state.app_state,
                                channel,
                                "You are not authorized to use this bot.",
                            )
                            .await;
                            return Ok(());
                        }
                    }
                    DmPolicy::Open => {}
                    DmPolicy::Pairing => {
                        let pairing_mgr = state.app_state.pairing_manager.read().await;
                        let admin_ids: Vec<String> = slack_config.admin_user_ids.clone();
                        if !pairing_mgr.is_channel_allowed("slack", user, &admin_ids) {
                            drop(pairing_mgr);
                            let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                            match pairing_mgr.create_pairing_request("slack", user, None) {
                                Ok(code) => {
                                    drop(pairing_mgr);
                                    send_slack_message(
                                        &state.app_state,
                                        channel,
                                        &format!(
                                            "You are not yet authorized. Your pairing code is:\n\n`{}`\n\nAsk the admin to approve this code in NexiBot Settings.",
                                            code
                                        ),
                                    ).await;
                                }
                                Err(e) => {
                                    drop(pairing_mgr);
                                    send_slack_message(
                                        &state.app_state,
                                        channel,
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
        }
    }

    // Handle approval responses before normal message routing.
    {
        let text_lc = text.trim().to_lowercase();
        if matches!(text_lc.as_str(), "approve" | "deny" | "!approve" | "!deny") {
            let key = (channel.to_string(), user.to_string());
            let (sender, owner_mismatch) = {
                let mut map = state.app_state.slack_pending_approvals.lock().await;
                if let Some(sender) = map.remove(&key) {
                    (Some(sender), false)
                } else {
                    let mismatch = map
                        .keys()
                        .any(|(pending_channel, _)| pending_channel == channel);
                    (None, mismatch)
                }
            };

            if let Some(sender) = sender {
                let approved = matches!(text_lc.as_str(), "approve" | "!approve");
                let _ = sender.send(approved);
                let reply = if approved {
                    "✅ Approved. Continuing…"
                } else {
                    "❌ Denied."
                };
                send_slack_message(&state.app_state, channel, reply).await;
                return Ok(());
            }

            if owner_mismatch {
                send_slack_message(
                    &state.app_state,
                    channel,
                    "❌ This approval request belongs to another user in this channel.",
                )
                .await;
                return Ok(());
            }
        }
    }

    let chat_lock = state.chat_lock(channel).await;
    let _chat_guard = chat_lock.lock().await;
    handle_slack_text_message(&state, channel, user, text).await;
    Ok(())
}

/// Process a text message through the Claude pipeline and send the response.
async fn handle_slack_text_message(state: &SlackState, channel: &str, user: &str, text: &str) {
    let app_state = &state.app_state;
    info!("[SLACK] Message from {} in {}: {}", user, channel, text);

    let claude_client = state.get_or_create_client(channel).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::Slack {
            channel_id: channel.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_bot_token = {
        let config = app_state.config.read().await;
        !app_state
            .key_interceptor
            .restore_config_string(&config.slack.bot_token)
            .trim()
            .is_empty()
    };

    let observer = SlackObserver::new(
        app_state.clone(),
        channel.to_string(),
        user.to_string(),
        has_bot_token,
        app_state.slack_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::slack(channel.to_string(), user.to_string()),
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
                send_slack_message(app_state, channel, "(No response)").await;
            } else {
                // Slack has a 4000 char limit per message
                for chunk in router::split_message(&response, 4000) {
                    send_slack_message(app_state, channel, &chunk).await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            send_slack_message(app_state, channel, &msg).await;
        }
        Err(e) => {
            send_slack_message(app_state, channel, &format!("Error: {}", e)).await;
        }
    }
}

/// Send a message via Slack Web API.
async fn send_slack_message(app_state: &AppState, channel: &str, text: &str) {
    let _ = send_slack_message_checked(app_state, channel, text).await;
}

async fn send_slack_message_checked(app_state: &AppState, channel: &str, text: &str) -> bool {
    let bot_token = {
        let config = app_state.config.read().await;
        app_state
            .key_interceptor
            .restore_config_string(&config.slack.bot_token)
    };

    if bot_token.is_empty() {
        error!("[SLACK] Cannot send message: bot_token not configured");
        return false;
    }

    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let body = serde_json::json!({
        "channel": channel,
        "text": text,
    });

    match client
        .post("https://slack.com/api/chat.postMessage")
        .bearer_auth(&bot_token)
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                    let err = json
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    error!("[SLACK] API error sending message: {}", err);
                    return false;
                }
                true
            } else {
                error!("[SLACK] Failed to decode API response");
                false
            }
        }
        Err(e) => {
            error!("[SLACK] HTTP error sending message: {}", e);
            false
        }
    }
}

/// Maximum number of concurrent Slack chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale Slack chat sessions (>24h inactive).
pub async fn session_cleanup_loop(state: Arc<SlackState>) {
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
                "[SLACK] Cleaned up {} stale sessions ({} remaining)",
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
                "[SLACK] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}
