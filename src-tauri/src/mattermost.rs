//! Mattermost bot integration for NexiBot.
//!
//! Connects to a Mattermost instance via WebSocket (`/api/v4/websocket`) for
//! real-time event delivery and uses the REST API (`/api/v4/posts`) to send
//! outbound messages.
//!
//! The bot authenticates using a Bot access token. Incoming `posted` events
//! include a JSON-in-JSON `post` field that is double-decoded.
//!
//! Reconnects with exponential backoff (3 attempts: 5s → 10s → 20s).

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;

use futures_util::{SinkExt, StreamExt};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};
use tracing::{debug, error, info, warn};
use url::Url;

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

/// Configuration for the Mattermost bot integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MattermostConfig {
    /// Whether the Mattermost integration is enabled.
    pub enabled: bool,
    /// Mattermost server URL, e.g. "https://mattermost.example.com"
    #[serde(default)]
    pub server_url: String,
    /// Bot access token obtained from the Mattermost System Console.
    #[serde(default)]
    pub bot_token: String,
    /// Optional team name to scope operations to a single team.
    #[serde(default)]
    pub team_name: Option<String>,
    /// Allowlist of channel IDs the bot will respond in.
    /// Empty = respond in all channels.
    #[serde(default)]
    pub allowed_channel_ids: Vec<String>,
    /// User IDs with admin privileges (bypass DM policy).
    #[serde(default)]
    pub admin_user_ids: Vec<String>,
    /// DM policy controlling who may interact with the bot.
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy.
    #[serde(default)]
    pub tool_policy: crate::config::ChannelToolPolicy,
}

impl Default for MattermostConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: String::new(),
            bot_token: String::new(),
            team_name: None,
            allowed_channel_ids: Vec::new(),
            admin_user_ids: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-channel session for Mattermost conversations.
pub(crate) struct MattermostChatSession {
    claude_client: ClaudeClient,
    last_activity: Instant,
}

/// Shared state for the Mattermost bot.
pub struct MattermostState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, MattermostChatSession>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for Mattermost tool execution flow, including in-channel approvals.
pub(crate) struct MattermostObserver {
    app_state: AppState,
    channel_id: String,
    requester_user_id: String,
    has_send_config: bool,
    pending_approvals:
        Arc<tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>>,
}

impl MattermostObserver {
    pub(crate) fn new(
        app_state: AppState,
        channel_id: String,
        requester_user_id: String,
        has_send_config: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            channel_id,
            requester_user_id,
            has_send_config,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for MattermostObserver {
    fn supports_approval(&self) -> bool {
        self.has_send_config
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
                send_mattermost_message(
                    &self.app_state,
                    &self.channel_id,
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
        if !send_mattermost_message_checked(&self.app_state, &self.channel_id, &prompt).await {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                send_mattermost_message(
                    &self.app_state,
                    &self.channel_id,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

impl MattermostState {
    fn new(app_state: AppState) -> Self {
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

    async fn get_or_create_client(&self, channel_id: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(channel_id) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            channel_id.to_string(),
            MattermostChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

// ---------------------------------------------------------------------------
// WebSocket event structures
// ---------------------------------------------------------------------------

/// Top-level Mattermost WebSocket event.
#[derive(Debug, Deserialize)]
struct MattermostWsEvent {
    event: Option<String>,
    data: Option<MattermostWsData>,
}

/// Data payload for a Mattermost `posted` event.
#[derive(Debug, Deserialize)]
struct MattermostWsData {
    /// The post object, serialized as a JSON string (JSON-in-JSON).
    post: Option<String>,
    /// Sender username (informational).
    sender_name: Option<String>,
}

/// A Mattermost post (inner JSON, decoded from `data.post`).
#[derive(Debug, Deserialize)]
struct MattermostPost {
    id: Option<String>,
    channel_id: Option<String>,
    user_id: Option<String>,
    message: Option<String>,
    /// Indicates a system message if non-empty (e.g. "system_join_channel").
    #[serde(default)]
    r#type: String,
}

// ---------------------------------------------------------------------------
// Bot entry point
// ---------------------------------------------------------------------------

/// Start the Mattermost bot WebSocket listener.
///
/// Spawns a background task that connects to the WebSocket endpoint and
/// processes events. Reconnects with exponential backoff on failure.
pub async fn start_mattermost_bot(app_state: AppState) -> Result<(), String> {
    let enabled = {
        let config = app_state.config.read().await;
        config.mattermost.enabled
    };
    if !enabled {
        info!("[MATTERMOST] Integration disabled in config");
        return Ok(());
    }

    info!("[MATTERMOST] Starting bot...");

    let state = Arc::new(MattermostState::new(app_state));

    // Spawn session cleanup task
    let cleanup_state = state.clone();
    tokio::spawn(session_cleanup_loop(cleanup_state));

    // Spawn the WebSocket loop with indefinite reconnect backoff.
    tokio::spawn(async move {
        let mut backoff_secs: u64 = 5;
        loop {
            // Check if still enabled before attempting connection
            {
                let config = state.app_state.config.read().await;
                if !config.mattermost.enabled {
                    info!("[MATTERMOST] Integration disabled at runtime, stopping bot");
                    break;
                }
            }

            match run_mattermost_ws_loop(&state).await {
                Ok(()) => {
                    info!("[MATTERMOST] WebSocket loop exited cleanly, reconnecting...");
                    backoff_secs = 5;
                }
                Err(e) => {
                    warn!(
                        "[MATTERMOST] WebSocket error: {}. Reconnecting in {}s...",
                        e, backoff_secs
                    );
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(64);
        }
    });

    Ok(())
}

/// Run the Mattermost WebSocket event loop.
async fn run_mattermost_ws_loop(state: &Arc<MattermostState>) -> Result<(), String> {
    let (server_url, bot_token) = {
        let config = state.app_state.config.read().await;
        (
            config.mattermost.server_url.clone(),
            state
                .app_state
                .key_interceptor
                .restore_config_string(&config.mattermost.bot_token),
        )
    };

    if server_url.is_empty() || bot_token.is_empty() {
        return Err("Mattermost server_url or bot_token not configured".to_string());
    }

    let ws_url = build_mattermost_ws_url(&server_url)?;

    info!("[MATTERMOST] Connecting to WebSocket: {}", ws_url);

    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .map_err(|e| format!("WebSocket connect failed: {}", e))?;

    info!("[MATTERMOST] WebSocket connected");

    let (mut write, mut read) = ws_stream.split();

    // Send authentication challenge immediately after connect
    let auth_msg = serde_json::json!({
        "seq": 1,
        "action": "authentication_challenge",
        "data": { "token": bot_token }
    });
    write
        .send(WsMessage::Text(auth_msg.to_string()))
        .await
        .map_err(|e| format!("Failed to send auth challenge: {}", e))?;

    info!("[MATTERMOST] Authentication challenge sent");

    while let Some(msg_result) = read.next().await {
        match msg_result {
            Ok(WsMessage::Text(text)) => {
                if let Err(e) = handle_mattermost_ws_message(state, &text).await {
                    warn!("[MATTERMOST] Error handling message: {}", e);
                }
            }
            Ok(WsMessage::Close(_)) => {
                return Err("WebSocket closed by server".to_string());
            }
            Ok(WsMessage::Ping(data)) => {
                // Respond to pings to keep the connection alive
                if let Err(e) = write.send(WsMessage::Pong(data)).await {
                    warn!("[MATTERMOST] Failed to send pong: {}", e);
                }
            }
            Ok(WsMessage::Pong(_)) | Ok(WsMessage::Binary(_)) => {}
            Ok(WsMessage::Frame(_)) => {}
            Err(e) => {
                return Err(format!("WebSocket receive error: {}", e));
            }
        }
    }

    Err("WebSocket stream ended unexpectedly".to_string())
}

fn append_path_segment(base_path: &str, segment: &str) -> String {
    let base = base_path.trim_end_matches('/');
    let segment = segment.trim_start_matches('/');
    if base.is_empty() || base == "/" {
        format!("/{}", segment)
    } else {
        format!("{}/{}", base, segment)
    }
}

fn build_mattermost_ws_url(server_url: &str) -> Result<String, String> {
    let mut url =
        Url::parse(server_url).map_err(|e| format!("invalid Mattermost server_url: {}", e))?;
    match url.scheme() {
        "http" => {
            if url.set_scheme("ws").is_err() {
                return Err(format!("failed to convert URL scheme to ws for '{}'", server_url));
            }
        }
        "https" => {
            if url.set_scheme("wss").is_err() {
                return Err(format!("failed to convert URL scheme to wss for '{}'", server_url));
            }
        }
        "ws" | "wss" => {}
        scheme => {
            return Err(format!(
                "unsupported Mattermost server_url scheme '{}'",
                scheme
            ));
        }
    }
    let path = append_path_segment(url.path(), "api/v4/websocket");
    url.set_path(&path);
    url.set_query(None);
    Ok(url.to_string())
}

/// Dispatch a single Mattermost WebSocket text frame.
async fn handle_mattermost_ws_message(
    state: &Arc<MattermostState>,
    text: &str,
) -> Result<(), String> {
    let event: MattermostWsEvent = match serde_json::from_str(text) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    if event.event.as_deref() != Some("posted") {
        return Ok(());
    }

    let data = match &event.data {
        Some(d) => d,
        None => return Ok(()),
    };

    // The `post` field is a JSON string embedded inside the outer JSON
    let post_json = match &data.post {
        Some(p) => p,
        None => return Ok(()),
    };

    let post: MattermostPost = match serde_json::from_str(post_json) {
        Ok(p) => p,
        Err(e) => {
            warn!("[MATTERMOST] Failed to parse inner post JSON: {}", e);
            return Ok(());
        }
    };

    // Skip system messages
    if !post.r#type.is_empty() {
        return Ok(());
    }

    let channel_id = post.channel_id.as_deref().unwrap_or("").to_string();
    let user_id = post.user_id.as_deref().unwrap_or("").to_string();
    let message = post.message.as_deref().unwrap_or("").trim().to_string();

    if channel_id.is_empty() || message.is_empty() {
        return Ok(());
    }

    // Authorization checks
    let (allowed_channel_ids, admin_user_ids, dm_policy, enabled) = {
        let config = state.app_state.config.read().await;
        (
            config.mattermost.allowed_channel_ids.clone(),
            config.mattermost.admin_user_ids.clone(),
            config.mattermost.dm_policy,
            config.mattermost.enabled,
        )
    };

    if !enabled {
        return Ok(());
    }

    // --- Message deduplication using Mattermost post ID ---
    let post_id = post.id.as_deref().unwrap_or("").to_string();
    let dedup_key = if post_id.is_empty() {
        format!("{}:{}:{}", channel_id, user_id, message)
    } else {
        post_id.clone()
    };
    {
        let mut dedup = state.msg_dedup.lock().await;
        if dedup.put(dedup_key.clone(), ()).is_some() {
            debug!("[MATTERMOST] Dropping duplicate post: {}", dedup_key);
            return Ok(());
        }
    }

    // --- Per-sender rate limiting ---
    let rate_key = format!("mattermost:{}", user_id);
    if state.rate_limiter.check(&rate_key).is_err() {
        warn!("[MATTERMOST] Rate limit exceeded for user {} — dropping message", user_id);
        return Ok(());
    }

    // Channel allowlist
    if !allowed_channel_ids.is_empty() && !allowed_channel_ids.contains(&channel_id) {
        return Ok(());
    }

    // Admin bypass
    let is_admin = !admin_user_ids.is_empty() && admin_user_ids.contains(&user_id);

    if !is_admin {
        match dm_policy {
            crate::pairing::DmPolicy::Allowlist => {
                // Channel allowlist above serves as primary access control
            }
            crate::pairing::DmPolicy::Open => {}
            crate::pairing::DmPolicy::Pairing => {
                let allowed_users: Vec<String> = vec![];
                let pairing_mgr = state.app_state.pairing_manager.read().await;
                let is_allowed =
                    pairing_mgr.is_channel_allowed("mattermost", &user_id, &allowed_users);
                drop(pairing_mgr);

                if !is_allowed {
                    let display_name = data.sender_name.clone();
                    let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                    match pairing_mgr.create_pairing_request("mattermost", &user_id, display_name) {
                        Ok(code) => {
                            drop(pairing_mgr);
                            send_mattermost_message(
                                &state.app_state,
                                &channel_id,
                                &format!(
                                    "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                    code
                                ),
                            )
                            .await;
                        }
                        Err(e) => {
                            drop(pairing_mgr);
                            send_mattermost_message(
                                &state.app_state,
                                &channel_id,
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

    let text_lc = message.trim().to_lowercase();
    if matches!(
        text_lc.as_str(),
        "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
    ) {
        let key = (channel_id.clone(), user_id.clone());
        let (approval_tx, owner_mismatch) = {
            let mut map = state.app_state.mattermost_pending_approvals.lock().await;
            if let Some(tx) = map.remove(&key) {
                (Some(tx), false)
            } else {
                let mismatch = map
                    .keys()
                    .any(|(pending_channel_id, _)| pending_channel_id == &channel_id);
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
            send_mattermost_message(&state.app_state, &channel_id, reply).await;
            return Ok(());
        }
        if owner_mismatch {
            send_mattermost_message(
                &state.app_state,
                &channel_id,
                "This approval request belongs to another user in this channel.",
            )
            .await;
            return Ok(());
        }
    }

    info!(
        "[MATTERMOST] Message from {} in {}: {}",
        user_id,
        channel_id,
        &message[..message.len().min(80)]
    );

    let state_clone = state.clone();
    tokio::spawn(async move {
        handle_mattermost_text_message(&state_clone, &channel_id, &user_id, &message).await;
    });

    Ok(())
}

/// Route a Mattermost text message through the Claude pipeline.
async fn handle_mattermost_text_message(
    state: &MattermostState,
    channel_id: &str,
    user_id: &str,
    text: &str,
) {
    let app_state = &state.app_state;

    let claude_client = state.get_or_create_client(channel_id).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::Mattermost {
            channel_id: channel_id.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_send_config = {
        let config = app_state.config.read().await;
        !config.mattermost.server_url.trim().is_empty()
            && !config.mattermost.bot_token.trim().is_empty()
    };

    let observer = MattermostObserver::new(
        app_state.clone(),
        channel_id.to_string(),
        user_id.to_string(),
        has_send_config,
        app_state.mattermost_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::mattermost(channel_id.to_string(), user_id.to_string()),
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
                send_mattermost_message(app_state, channel_id, "(No response)").await;
            } else {
                for chunk in router::split_message(&response, 4096) {
                    send_mattermost_message(app_state, channel_id, &chunk).await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            send_mattermost_message(app_state, channel_id, &msg).await;
        }
        Err(e) => {
            send_mattermost_message(app_state, channel_id, &format!("Error: {}", e)).await;
        }
    }
}

/// Post a message to a Mattermost channel via the REST API.
pub async fn send_mattermost_message(app_state: &AppState, channel_id: &str, text: &str) {
    let _ = send_mattermost_message_checked(app_state, channel_id, text).await;
}

async fn send_mattermost_message_checked(
    app_state: &AppState,
    channel_id: &str,
    text: &str,
) -> bool {
    let (server_url, bot_token) = {
        let config = app_state.config.read().await;
        (
            config.mattermost.server_url.clone(),
            app_state
                .key_interceptor
                .restore_config_string(&config.mattermost.bot_token),
        )
    };

    if server_url.is_empty() || bot_token.is_empty() {
        error!("[MATTERMOST] Cannot send: server_url or bot_token not configured");
        return false;
    }

    let url = format!("{}/api/v4/posts", server_url);
    let body = serde_json::json!({
        "channel_id": channel_id,
        "message": text,
    });

    let client = reqwest::Client::new();
    match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", bot_token))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            info!("[MATTERMOST] Message sent to channel {}", channel_id);
            true
        }
        Ok(resp) => {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            error!("[MATTERMOST] Send failed ({}): {}", status, body_text);
            false
        }
        Err(e) => {
            error!("[MATTERMOST] HTTP error sending message: {}", e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum number of concurrent Mattermost channel sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale Mattermost sessions (>24h inactive).
pub async fn session_cleanup_loop(state: Arc<MattermostState>) {
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
                "[MATTERMOST] Cleaned up {} stale sessions ({} remaining)",
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
                "[MATTERMOST] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_mattermost_ws_url_sets_ws_scheme() {
        let url = build_mattermost_ws_url("https://mattermost.example.com")
            .expect("Mattermost ws url should build");
        assert_eq!(url, "wss://mattermost.example.com/api/v4/websocket");
    }

    #[test]
    fn test_build_mattermost_ws_url_preserves_base_path() {
        let url = build_mattermost_ws_url("http://example.com/mattermost")
            .expect("Mattermost ws url should build");
        assert_eq!(url, "ws://example.com/mattermost/api/v4/websocket");
    }
}
