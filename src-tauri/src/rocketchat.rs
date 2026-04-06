//! Rocket.Chat integration via DDP WebSocket protocol.
//!
//! Connects to a Rocket.Chat server using the DDP (Distributed Data Protocol)
//! WebSocket interface for real-time message subscription, and sends replies
//! via the Rocket.Chat REST API.

use futures_util::{SinkExt, StreamExt};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use url::Url;

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

/// Configuration for the Rocket.Chat channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RocketChatConfig {
    /// Whether the Rocket.Chat channel is enabled.
    pub enabled: bool,

    /// Rocket.Chat server URL (e.g. "https://chat.example.com").
    #[serde(default)]
    pub server_url: String,

    /// Bot login username.
    #[serde(default)]
    pub username: String,

    /// Bot login password (hashed before transmitting via DDP).
    #[serde(default)]
    pub password: String,

    /// List of room IDs to subscribe to. Empty = subscribe to all DMs.
    #[serde(default)]
    pub allowed_room_ids: Vec<String>,

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

impl Default for RocketChatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: String::new(),
            username: String::new(),
            password: String::new(),
            allowed_room_ids: Vec::new(),
            admin_user_ids: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Auth state (REST API credentials received after DDP login)
// ---------------------------------------------------------------------------

/// Cached REST API credentials from a successful DDP login.
#[allow(dead_code)]
pub(crate) struct RocketChatAuth {
    auth_token: String,
    user_id: String,
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-room conversation session for Rocket.Chat.
#[allow(dead_code)]
pub(crate) struct RocketChatChatSession {
    /// Dedicated Claude client with its own conversation history.
    claude_client: ClaudeClient,
    /// Last activity timestamp for session expiry.
    last_activity: Instant,
}

/// Shared state for the Rocket.Chat bot.
pub struct RocketChatState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, RocketChatChatSession>>,
    pub auth: RwLock<Option<RocketChatAuth>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for Rocket.Chat tool execution flow, including in-channel approvals.
pub(crate) struct RocketChatObserver {
    app_state: AppState,
    room_id: String,
    requester_user_id: String,
    has_send_config: bool,
    pending_approvals:
        Arc<tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>>,
}

impl RocketChatObserver {
    pub(crate) fn new(
        app_state: AppState,
        room_id: String,
        requester_user_id: String,
        has_send_config: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            room_id,
            requester_user_id,
            has_send_config,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for RocketChatObserver {
    fn supports_approval(&self) -> bool {
        self.has_send_config
            && !self.room_id.trim().is_empty()
            && !self.requester_user_id.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = (self.room_id.clone(), self.requester_user_id.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                send_rocketchat_message_with_app(
                    &self.app_state,
                    &self.room_id,
                    "Another approval is already pending for this requester in this room. Denying this request.",
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
        if !send_rocketchat_message_with_app_checked(&self.app_state, &self.room_id, &prompt).await
        {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                send_rocketchat_message_with_app(
                    &self.app_state,
                    &self.room_id,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

impl RocketChatState {
    fn new(app_state: AppState) -> Self {
        Self {
            app_state,
            chat_sessions: RwLock::new(HashMap::new()),
            auth: RwLock::new(None),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig {
                max_attempts: 30,
                window_seconds: 60,
                lockout_seconds: 30,
            })),
            msg_dedup: Mutex::new(LruCache::new(NonZeroUsize::new(10_000).unwrap())),
        }
    }

    /// Get or create a Claude client for the given room ID.
    async fn get_or_create_client(&self, room_id: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(room_id) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            room_id.to_string(),
            RocketChatChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

// ---------------------------------------------------------------------------
// DDP protocol helpers
// ---------------------------------------------------------------------------

/// Compute SHA-256 hex digest of a password for DDP login.
fn sha256_hex(input: &str) -> String {
    let hash = Sha256::digest(input.as_bytes());
    hex::encode(hash)
}

/// Build the DDP connect message.
fn ddp_connect() -> String {
    serde_json::json!({
        "msg": "connect",
        "version": "1",
        "support": ["1"],
    })
    .to_string()
}

/// Build a DDP method call message.
fn ddp_method(id: &str, method: &str, params: serde_json::Value) -> String {
    serde_json::json!({
        "msg": "method",
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string()
}

/// Build a DDP subscription message.
fn ddp_subscribe(id: &str, name: &str, params: serde_json::Value) -> String {
    serde_json::json!({
        "msg": "sub",
        "id": id,
        "name": name,
        "params": params,
    })
    .to_string()
}

/// Build the DDP pong response.
fn ddp_pong() -> String {
    serde_json::json!({"msg": "pong"}).to_string()
}

// ---------------------------------------------------------------------------
// Main bot entry point
// ---------------------------------------------------------------------------

/// Start the Rocket.Chat bot with reconnect-on-error behavior.
pub async fn start_rocketchat_bot(app_state: AppState) -> Result<(), String> {
    let (enabled, server_url, username, password) = {
        let config = app_state.config.read().await;
        (
            config.rocketchat.enabled,
            config.rocketchat.server_url.clone(),
            config.rocketchat.username.clone(),
            app_state
                .key_interceptor
                .restore_config_string(&config.rocketchat.password),
        )
    };

    if !enabled {
        info!("[ROCKETCHAT] Rocket.Chat integration disabled in config");
        return Ok(());
    }

    if server_url.is_empty() || username.is_empty() {
        return Err("[ROCKETCHAT] server_url or username not configured".to_string());
    }

    let state = Arc::new(RocketChatState::new(app_state));

    // Spawn session cleanup task.
    let cleanup_state = state.clone();
    tokio::spawn(session_cleanup_loop(cleanup_state));

    // Main reconnect loop.
    let mut backoff_secs: u64 = 5;
    loop {
        match run_ddp_loop(&state, &server_url, &username, &password).await {
            Ok(()) => {
                info!("[ROCKETCHAT] DDP loop exited cleanly, reconnecting...");
            }
            Err(e) => {
                warn!(
                    "[ROCKETCHAT] DDP error: {}. Reconnecting in {}s...",
                    e, backoff_secs
                );
            }
        }

        // Check if still enabled.
        {
            let config = state.app_state.config.read().await;
            if !config.rocketchat.enabled {
                info!("[ROCKETCHAT] Disabled at runtime, stopping");
                return Ok(());
            }
        }

        // Clear auth on disconnect.
        {
            let mut auth = state.auth.write().await;
            *auth = None;
        }
        {
            let mut auth = state.app_state.rocketchat_rest_auth.write().await;
            *auth = None;
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(120);
    }
}

/// Connect via DDP WebSocket, login, subscribe to rooms, and handle messages.
async fn run_ddp_loop(
    state: &Arc<RocketChatState>,
    server_url: &str,
    username: &str,
    password: &str,
) -> Result<(), String> {
    let ws_url = build_rocketchat_ws_url(server_url)?;

    info!("[ROCKETCHAT] Connecting to DDP WebSocket at {}...", ws_url);

    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .map_err(|e| format!("WebSocket connect error: {}", e))?;

    info!("[ROCKETCHAT] WebSocket connected");

    let (mut write, mut read) = ws_stream.split();

    // Step 1: Send DDP connect.
    write
        .send(Message::Text(ddp_connect()))
        .await
        .map_err(|e| format!("DDP connect send error: {}", e))?;

    // Step 2: Wait for "connected" response then login.
    let mut logged_in = false;
    let password_digest = sha256_hex(password);
    let bot_user_id: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));

    while let Some(msg_result) = read.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => return Err(format!("WebSocket read error: {}", e)),
        };

        let text = match msg {
            Message::Text(t) => t,
            Message::Ping(_) => {
                let _ = write.send(Message::Pong(vec![])).await;
                continue;
            }
            Message::Close(_) => {
                info!("[ROCKETCHAT] Server closed connection");
                return Ok(());
            }
            _ => continue,
        };

        let json: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = json.get("msg").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            "connected" => {
                info!("[ROCKETCHAT] DDP connected, logging in as {}...", username);
                let login_params = serde_json::json!([{
                    "user": { "username": username },
                    "password": {
                        "digest": password_digest,
                        "algorithm": "sha-256",
                    }
                }]);
                write
                    .send(Message::Text(ddp_method("login-1", "login", login_params)))
                    .await
                    .map_err(|e| format!("DDP login send error: {}", e))?;
            }

            "result" if json.get("id").and_then(|v| v.as_str()) == Some("login-1") => {
                if let Some(error) = json.get("error") {
                    return Err(format!("DDP login error: {}", error));
                }

                let result = json.get("result").cloned().unwrap_or_default();
                let token = result
                    .get("token")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let uid = result
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if token.is_empty() || uid.is_empty() {
                    return Err("DDP login succeeded but token/userId empty".to_string());
                }

                info!("[ROCKETCHAT] Logged in as user_id={}", uid);

                // Cache auth for REST API use.
                {
                    let mut auth = state.auth.write().await;
                    *auth = Some(RocketChatAuth {
                        auth_token: token.clone(),
                        user_id: uid.clone(),
                    });
                }
                {
                    let mut auth = state.app_state.rocketchat_rest_auth.write().await;
                    *auth = Some((token, uid.clone()));
                }
                {
                    let mut uid_lock = bot_user_id.write().await;
                    *uid_lock = Some(uid);
                }

                logged_in = true;

                // Subscribe to room messages.
                let allowed_room_ids = {
                    let config = state.app_state.config.read().await;
                    config.rocketchat.allowed_room_ids.clone()
                };

                if allowed_room_ids.is_empty() {
                    // Subscribe to all DM (direct) rooms via user notifications stream.
                    // The topic format for stream-notify-user is "{uid}/message".
                    let bot_uid = bot_user_id.read().await;
                    if let Some(ref uid) = *bot_uid {
                        let topic = format!("{}/message", uid);
                        let sub_params = serde_json::json!([topic, false]);
                        write
                            .send(Message::Text(ddp_subscribe(
                                "sub-notif",
                                "stream-notify-user",
                                sub_params,
                            )))
                            .await
                            .map_err(|e| format!("subscribe error: {}", e))?;
                    }
                } else {
                    // Subscribe to each configured room.
                    for (i, room_id) in allowed_room_ids.iter().enumerate() {
                        let sub_id = format!("sub-room-{}", i);
                        let sub_params = serde_json::json!([room_id, false]);
                        write
                            .send(Message::Text(ddp_subscribe(
                                &sub_id,
                                "stream-room-messages",
                                sub_params,
                            )))
                            .await
                            .map_err(|e| format!("subscribe error: {}", e))?;
                        info!("[ROCKETCHAT] Subscribed to room {}", room_id);
                    }
                }
            }

            "ping" => {
                write
                    .send(Message::Text(ddp_pong()))
                    .await
                    .map_err(|e| format!("pong send error: {}", e))?;
            }

            "changed" if logged_in => {
                if let Err(e) = handle_ddp_changed_message(state, &json, &bot_user_id).await {
                    warn!("[ROCKETCHAT] Error handling message: {}", e);
                }
            }

            "error" => {
                return Err(format!("DDP server error: {}", json));
            }

            _ => {}
        }
    }

    Ok(())
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

fn build_rocketchat_ws_url(server_url: &str) -> Result<String, String> {
    let mut url =
        Url::parse(server_url).map_err(|e| format!("invalid Rocket.Chat server_url: {}", e))?;
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
                "unsupported Rocket.Chat server_url scheme '{}'",
                scheme
            ));
        }
    }
    let path = append_path_segment(url.path(), "websocket");
    url.set_path(&path);
    url.set_query(None);
    Ok(url.to_string())
}

/// Process a DDP "changed" message that may contain a new chat message.
async fn handle_ddp_changed_message(
    state: &Arc<RocketChatState>,
    json: &serde_json::Value,
    bot_user_id: &Arc<RwLock<Option<String>>>,
) -> Result<(), String> {
    let collection = json
        .get("collection")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if collection != "stream-room-messages" && collection != "stream-notify-user" {
        return Ok(());
    }

    let args = json
        .get("fields")
        .and_then(|f| f.get("args"))
        .and_then(|a| a.as_array())
        .ok_or("Missing fields.args")?;

    // stream-notify-user delivers a single message object as args[0],
    // while stream-room-messages delivers an array as args[0].
    let messages: Vec<serde_json::Value> = if collection == "stream-notify-user" {
        match args.first() {
            Some(msg) => vec![msg.clone()],
            None => return Ok(()),
        }
    } else {
        args.first()
            .and_then(|a| a.as_array())
            .map(|v| v.to_vec())
            .unwrap_or_default()
    };

    let bot_uid = bot_user_id.read().await;

    for msg in &messages {
        let sender_id = msg
            .get("u")
            .and_then(|u| u.get("_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Don't respond to our own messages.
        if let Some(ref uid) = *bot_uid {
            if sender_id == uid.as_str() {
                continue;
            }
        }

        let room_id = msg.get("rid").and_then(|v| v.as_str()).unwrap_or("");
        let text = msg.get("msg").and_then(|v| v.as_str()).unwrap_or("");

        if room_id.is_empty() || text.is_empty() {
            continue;
        }

        // ---- Authorization ----
        {
            let config = state.app_state.config.read().await;
            let enabled = config.rocketchat.enabled;
            let dm_policy = config.rocketchat.dm_policy;
            let allowed_rooms = config.rocketchat.allowed_room_ids.clone();
            let admins = config.rocketchat.admin_user_ids.clone();
            drop(config);

            if !enabled {
                continue;
            }

            // Room filter: if allowed_room_ids is configured, only process listed rooms.
            if !allowed_rooms.is_empty() && !allowed_rooms.contains(&room_id.to_string()) {
                continue;
            }

            let is_admin = !admins.is_empty() && admins.contains(&sender_id.to_string());

            if !is_admin {
                match dm_policy {
                    crate::pairing::DmPolicy::Allowlist => {
                        // For Rocket.Chat, allowlist is enforced via allowed_room_ids.
                        // Per-user filtering is available via admin_user_ids.
                    }
                    crate::pairing::DmPolicy::Open => {}
                    crate::pairing::DmPolicy::Pairing => {
                        let pairing_mgr = state.app_state.pairing_manager.read().await;
                        let empty_allowed: Vec<String> = Vec::new();
                        if !pairing_mgr.is_channel_allowed("rocketchat", sender_id, &empty_allowed)
                        {
                            drop(pairing_mgr);
                            let state_clone = state.clone();
                            let sender_id_owned = sender_id.to_string();
                            let room_id_owned = room_id.to_string();
                            tokio::spawn(async move {
                                let mut pairing_mgr =
                                    state_clone.app_state.pairing_manager.write().await;
                                match pairing_mgr.create_pairing_request(
                                    "rocketchat",
                                    &sender_id_owned,
                                    None,
                                ) {
                                    Ok(code) => {
                                        drop(pairing_mgr);
                                        send_rocketchat_message(
                                            &state_clone,
                                            &room_id_owned,
                                            &format!(
                                                "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                                code
                                            ),
                                        )
                                        .await;
                                    }
                                    Err(e) => {
                                        drop(pairing_mgr);
                                        send_rocketchat_message(
                                            &state_clone,
                                            &room_id_owned,
                                            &format!("Authorization pending. {}", e),
                                        )
                                        .await;
                                    }
                                }
                            });
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
            let key = (room_id.to_string(), sender_id.to_string());
            let (approval_tx, owner_mismatch) = {
                let mut map = state.app_state.rocketchat_pending_approvals.lock().await;
                if let Some(tx) = map.remove(&key) {
                    (Some(tx), false)
                } else {
                    let mismatch = map
                        .keys()
                        .any(|(pending_room_id, _)| pending_room_id == room_id);
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
                send_rocketchat_message(state, room_id, reply).await;
                continue;
            }
            if owner_mismatch {
                send_rocketchat_message(
                    state,
                    room_id,
                    "This approval request belongs to another user in this room.",
                )
                .await;
                continue;
            }
        }

        // --- Message deduplication using Rocket.Chat message _id ---
        let msg_id = msg.get("_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if !msg_id.is_empty() {
            let mut dedup = state.msg_dedup.lock().await;
            if dedup.put(msg_id.clone(), ()).is_some() {
                debug!("[ROCKETCHAT] Dropping duplicate message: {}", msg_id);
                continue;
            }
        }

        // --- Rate limiting per sender ---
        let rate_key = format!("rocketchat:{}", sender_id);
        if state.rate_limiter.check(&rate_key).is_err() {
            warn!("[ROCKETCHAT] Rate limit exceeded for sender: {}", sender_id);
            continue;
        }

        let state_clone = state.clone();
        let room_id_owned = room_id.to_string();
        let sender_id_owned = sender_id.to_string();
        let text_owned = text.to_string();
        tokio::spawn(async move {
            handle_rocketchat_message(&state_clone, &room_id_owned, &sender_id_owned, &text_owned)
                .await;
        });
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Message handling
// ---------------------------------------------------------------------------

/// Route a Rocket.Chat message through Claude and send the reply.
async fn handle_rocketchat_message(
    state: &RocketChatState,
    room_id: &str,
    user_id: &str,
    text: &str,
) {
    let app_state = &state.app_state;
    info!("[ROCKETCHAT] Message in room {}: {}", room_id, text);

    let claude_client = state.get_or_create_client(room_id).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::RocketChat {
            room_id: room_id.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_send_config = {
        let server_url = {
            let config = app_state.config.read().await;
            config.rocketchat.server_url.clone()
        };
        !server_url.trim().is_empty() && app_state.rocketchat_rest_auth.read().await.is_some()
    };

    let observer = RocketChatObserver::new(
        app_state.clone(),
        room_id.to_string(),
        user_id.to_string(),
        has_send_config,
        app_state.rocketchat_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::rocketchat(room_id.to_string(), user_id.to_string()),
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
                send_rocketchat_message(state, room_id, "(No response)").await;
            } else {
                for chunk in router::split_message(&response, 4096) {
                    send_rocketchat_message(state, room_id, &chunk).await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            send_rocketchat_message(state, room_id, &msg).await;
        }
        Err(e) => {
            send_rocketchat_message(state, room_id, &format!("Error: {}", e)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Rocket.Chat REST API
// ---------------------------------------------------------------------------

/// Post a message to a Rocket.Chat room via the REST API.
async fn send_rocketchat_message(state: &RocketChatState, room_id: &str, text: &str) {
    let _ = send_rocketchat_message_checked(state, room_id, text).await;
}

async fn send_rocketchat_message_checked(
    state: &RocketChatState,
    room_id: &str,
    text: &str,
) -> bool {
    let (server_url, auth_token, user_id) = {
        let config = state.app_state.config.read().await;
        let server_url = config.rocketchat.server_url.clone();
        drop(config);
        let auth = state.auth.read().await;
        match auth.as_ref() {
            Some(a) => (server_url, a.auth_token.clone(), a.user_id.clone()),
            None => {
                warn!("[ROCKETCHAT] Cannot send message: not authenticated yet");
                return false;
            }
        }
    };

    post_rocketchat_message_checked(&server_url, &auth_token, &user_id, room_id, text).await
}

/// Post a message to Rocket.Chat using auth cached in AppState.
async fn send_rocketchat_message_with_app(app_state: &AppState, room_id: &str, text: &str) {
    let _ = send_rocketchat_message_with_app_checked(app_state, room_id, text).await;
}

async fn send_rocketchat_message_with_app_checked(
    app_state: &AppState,
    room_id: &str,
    text: &str,
) -> bool {
    let (server_url, auth_token, user_id) = {
        let config = app_state.config.read().await;
        let server_url = config.rocketchat.server_url.clone();
        drop(config);
        let auth = app_state.rocketchat_rest_auth.read().await;
        match auth.as_ref() {
            Some((token, uid)) => (server_url, token.clone(), uid.clone()),
            None => {
                warn!("[ROCKETCHAT] Cannot send message: no cached REST auth");
                return false;
            }
        }
    };

    post_rocketchat_message_checked(&server_url, &auth_token, &user_id, room_id, text).await
}

/// Shared Rocket.Chat REST message sender.
async fn post_rocketchat_message_checked(
    server_url: &str,
    auth_token: &str,
    user_id: &str,
    room_id: &str,
    text: &str,
) -> bool {
    if server_url.is_empty() || auth_token.is_empty() || user_id.is_empty() {
        error!("[ROCKETCHAT] Cannot send message: missing server_url/auth_token/user_id");
        return false;
    }

    let url = format!(
        "{}/api/v1/chat.postMessage",
        server_url.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "roomId": room_id,
        "text": text,
    });

    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    match client
        .post(&url)
        .header("X-Auth-Token", auth_token)
        .header("X-User-Id", user_id)
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            info!("[ROCKETCHAT] Message posted to room {}", room_id);
            true
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            error!("[ROCKETCHAT] Post failed ({}): {}", status, body);
            false
        }
        Err(e) => {
            error!("[ROCKETCHAT] HTTP error posting message: {}", e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum concurrent Rocket.Chat chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically evict stale Rocket.Chat sessions (>24 h inactive).
pub async fn session_cleanup_loop(state: Arc<RocketChatState>) {
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
                "[ROCKETCHAT] Cleaned up {} stale sessions ({} remaining)",
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
                "[ROCKETCHAT] Evicted {} oldest sessions to enforce cap (now {})",
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
    fn test_build_rocketchat_ws_url_sets_ws_scheme() {
        let url = build_rocketchat_ws_url("https://chat.example.com").expect("Rocket.Chat ws url");
        assert_eq!(url, "wss://chat.example.com/websocket");
    }

    #[test]
    fn test_build_rocketchat_ws_url_preserves_base_path() {
        let url = build_rocketchat_ws_url("http://example.com/chat").expect("Rocket.Chat ws url");
        assert_eq!(url, "ws://example.com/chat/websocket");
    }
}
