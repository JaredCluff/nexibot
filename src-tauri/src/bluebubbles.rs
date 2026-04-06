//! iMessage integration via BlueBubbles server.
//!
//! BlueBubbles is a local Mac server that exposes iMessage via REST and WebSocket APIs.
//! This module connects via WebSocket for real-time message events and uses the REST API
//! to send outbound messages.
//!
//! WebSocket endpoint: ws://{server_url}/socket.io/websocket?password={password}
//! Send endpoint:      POST {server_url}/api/v1/message/text?password={password}

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;

use futures_util::StreamExt;
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
use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::ToolLoopConfig;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the BlueBubbles iMessage channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueBubblesConfig {
    /// Whether the BlueBubbles integration is enabled.
    pub enabled: bool,
    /// BlueBubbles server URL, e.g. "http://localhost:1234"
    #[serde(default)]
    pub server_url: String,
    /// BlueBubbles server password.
    #[serde(default)]
    pub password: String,
    /// Allowlist of iMessage handles (e.g. ["+15551234567", "user@example.com"]).
    /// Empty list allows all handles when dm_policy is Allowlist.
    #[serde(default)]
    pub allowed_handles: Vec<String>,
    /// Admin handles that bypass DM policy restrictions.
    #[serde(default)]
    pub admin_handles: Vec<String>,
    /// DM policy controlling who may interact.
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,
    /// Per-channel tool access policy.
    #[serde(default)]
    pub tool_policy: crate::config::ChannelToolPolicy,
}

impl Default for BlueBubblesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: String::new(),
            password: String::new(),
            allowed_handles: Vec::new(),
            admin_handles: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-chat session state for BlueBubbles conversations.
pub(crate) struct BlueBubblesChatSession {
    /// Dedicated Claude client with its own conversation history.
    claude_client: ClaudeClient,
    /// Last activity timestamp for session eviction.
    last_activity: Instant,
}

/// Shared state for the BlueBubbles listener.
pub struct BlueBubblesState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, BlueBubblesChatSession>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for BlueBubbles tool execution flow, including in-channel approvals.
pub(crate) struct BlueBubblesObserver {
    app_state: AppState,
    chat_guid: String,
    requester_handle: String,
    has_send_config: bool,
    pending_approvals:
        Arc<tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>>,
}

impl BlueBubblesObserver {
    pub(crate) fn new(
        app_state: AppState,
        chat_guid: String,
        requester_handle: String,
        has_send_config: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            chat_guid,
            requester_handle,
            has_send_config,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for BlueBubblesObserver {
    fn supports_approval(&self) -> bool {
        self.has_send_config
            && !self.chat_guid.trim().is_empty()
            && !self.requester_handle.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = (self.chat_guid.clone(), self.requester_handle.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                send_bluebubbles_message(
                    &self.app_state,
                    &self.chat_guid,
                    "Another approval is already pending for this requester in this chat. Denying this request.",
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
        if !send_bluebubbles_message_checked(&self.app_state, &self.chat_guid, &prompt).await {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                send_bluebubbles_message(
                    &self.app_state,
                    &self.chat_guid,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

impl BlueBubblesState {
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

    /// Get or create a Claude client for the given chat GUID.
    async fn get_or_create_client(&self, chat_guid: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(chat_guid) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            chat_guid.to_string(),
            BlueBubblesChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

// ---------------------------------------------------------------------------
// WebSocket message parsing
// ---------------------------------------------------------------------------

/// Incoming WebSocket event from BlueBubbles.
#[derive(Debug, Deserialize)]
struct BlueBubblesEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    data: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Listener entry point
// ---------------------------------------------------------------------------

/// Start the BlueBubbles WebSocket listener.
///
/// Connects to the BlueBubbles server WebSocket endpoint and listens for
/// `new-message` events. Reconnects with exponential backoff on failure.
pub async fn start_bluebubbles_listener(app_state: AppState) -> Result<(), String> {
    let enabled = {
        let config = app_state.config.read().await;
        config.bluebubbles.enabled
    };
    if !enabled {
        info!("[BLUEBUBBLES] Integration disabled in config");
        return Ok(());
    }

    info!("[BLUEBUBBLES] Starting listener...");

    let state = Arc::new(BlueBubblesState::new(app_state));

    // Spawn session cleanup task
    let cleanup_state = state.clone();
    tokio::spawn(session_cleanup_loop(cleanup_state));

    // Run the WebSocket loop with reconnect backoff
    tokio::spawn(async move {
        let mut backoff_secs: u64 = 5;
        loop {
            // Check if still enabled before reconnecting
            {
                let config = state.app_state.config.read().await;
                if !config.bluebubbles.enabled {
                    info!("[BLUEBUBBLES] Integration disabled at runtime, stopping listener");
                    break;
                }
            }

            match run_bluebubbles_ws_loop(&state).await {
                Ok(()) => {
                    info!("[BLUEBUBBLES] WebSocket loop exited cleanly");
                    break;
                }
                Err(e) => {
                    warn!(
                        "[BLUEBUBBLES] WebSocket error: {}. Reconnecting in {}s...",
                        e, backoff_secs
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(60);
                }
            }
        }
    });

    Ok(())
}

/// Run the BlueBubbles WebSocket event loop until disconnect.
async fn run_bluebubbles_ws_loop(state: &Arc<BlueBubblesState>) -> Result<(), String> {
    let (server_url, password) = {
        let config = state.app_state.config.read().await;
        (
            config.bluebubbles.server_url.clone(),
            state
                .app_state
                .key_interceptor
                .restore_config_string(&config.bluebubbles.password),
        )
    };

    if server_url.is_empty() {
        return Err("BlueBubbles server_url not configured".to_string());
    }

    let ws_url = build_bluebubbles_ws_url(&server_url, &password)?;
    let ws_base = ws_url.split('?').next().unwrap_or(ws_url.as_str());
    info!("[BLUEBUBBLES] Connecting to WebSocket: {}", ws_base);

    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .map_err(|e| format!("WebSocket connect failed: {}", e))?;

    info!("[BLUEBUBBLES] WebSocket connected");

    let (_write, mut read) = ws_stream.split();

    while let Some(msg_result) = read.next().await {
        match msg_result {
            Ok(WsMessage::Text(text)) => {
                if let Err(e) = handle_bluebubbles_ws_message(state, &text).await {
                    warn!("[BLUEBUBBLES] Error handling message: {}", e);
                }
            }
            Ok(WsMessage::Close(_)) => {
                info!("[BLUEBUBBLES] WebSocket closed by server");
                return Err("WebSocket closed".to_string());
            }
            Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)) | Ok(WsMessage::Binary(_)) => {}
            Ok(WsMessage::Frame(_)) => {}
            Err(e) => {
                return Err(format!("WebSocket receive error: {}", e));
            }
        }
    }

    Err("WebSocket stream ended unexpectedly".to_string())
}

/// Parse and dispatch a single BlueBubbles WebSocket text message.
async fn handle_bluebubbles_ws_message(
    state: &Arc<BlueBubblesState>,
    text: &str,
) -> Result<(), String> {
    let event: BlueBubblesEvent = match serde_json::from_str(text) {
        Ok(e) => e,
        Err(_) => return Ok(()), // Ignore non-JSON frames (e.g., Socket.IO ping "2")
    };

    let event_type = match event.event_type.as_deref() {
        Some(t) => t,
        None => return Ok(()),
    };

    if event_type != "new-message" {
        return Ok(());
    }

    let data = match &event.data {
        Some(d) => d,
        None => return Ok(()),
    };

    // Extract fields from the message data object
    let chat_guid = data
        .get("chats")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|chat| chat.get("guid"))
        .or_else(|| data.get("chatGuid"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if chat_guid.is_empty() {
        return Ok(());
    }

    // Extract sender handle
    let handle = data
        .get("handle")
        .and_then(|h| h.get("address"))
        .or_else(|| data.get("sender"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Skip messages sent by the bot itself (isFromMe)
    if data
        .get("isFromMe")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Ok(());
    }

    let text_body = data
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    if text_body.is_empty() {
        return Ok(());
    }

    // Authorization check
    let (allowed_handles, admin_handles, dm_policy, enabled) = {
        let config = state.app_state.config.read().await;
        (
            config.bluebubbles.allowed_handles.clone(),
            config.bluebubbles.admin_handles.clone(),
            config.bluebubbles.dm_policy,
            config.bluebubbles.enabled,
        )
    };

    if !enabled {
        return Ok(());
    }

    // --- Message deduplication using BlueBubbles message GUID ---
    let msg_guid = data
        .get("guid")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}:{}:{}", chat_guid, handle, text_body));
    {
        let mut dedup = state.msg_dedup.lock().await;
        if dedup.put(msg_guid.clone(), ()).is_some() {
            debug!("[BLUEBUBBLES] Dropping duplicate message: {}", msg_guid);
            return Ok(());
        }
    }

    // --- Per-sender rate limiting ---
    let rate_key = format!("bluebubbles:{}", handle);
    if state.rate_limiter.check(&rate_key).is_err() {
        warn!(
            "[BLUEBUBBLES] Rate limit exceeded for handle {} — dropping message",
            handle
        );
        return Ok(());
    }

    // Admin bypass
    let is_admin = !admin_handles.is_empty() && admin_handles.contains(&handle.to_string());

    if !is_admin {
        match dm_policy {
            crate::pairing::DmPolicy::Allowlist => {
                if !allowed_handles.is_empty() && !allowed_handles.contains(&handle.to_string()) {
                    info!(
                        "[BLUEBUBBLES] Ignoring message from unauthorized handle: {}",
                        handle
                    );
                    return Ok(());
                }
            }
            crate::pairing::DmPolicy::Open => {}
            crate::pairing::DmPolicy::Pairing => {
                let pairing_mgr = state.app_state.pairing_manager.read().await;
                let is_allowed =
                    pairing_mgr.is_channel_allowed("bluebubbles", handle, &allowed_handles);
                drop(pairing_mgr);

                if !is_allowed {
                    let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                    match pairing_mgr.create_pairing_request("bluebubbles", handle, None) {
                        Ok(code) => {
                            drop(pairing_mgr);
                            send_bluebubbles_message(
                                &state.app_state,
                                chat_guid,
                                &format!(
                                    "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                    code
                                ),
                            )
                            .await;
                        }
                        Err(e) => {
                            drop(pairing_mgr);
                            send_bluebubbles_message(
                                &state.app_state,
                                chat_guid,
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

    let text_lc = text_body.trim().to_lowercase();
    if matches!(
        text_lc.as_str(),
        "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
    ) {
        let key = (chat_guid.to_string(), handle.to_string());
        let (approval_tx, owner_mismatch) = {
            let mut map = state.app_state.bluebubbles_pending_approvals.lock().await;
            if let Some(tx) = map.remove(&key) {
                (Some(tx), false)
            } else {
                let mismatch = map
                    .keys()
                    .any(|(pending_chat_guid, _)| pending_chat_guid == chat_guid);
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
            send_bluebubbles_message(&state.app_state, chat_guid, reply).await;
            return Ok(());
        }
        if owner_mismatch {
            send_bluebubbles_message(
                &state.app_state,
                chat_guid,
                "This approval request belongs to another user in this chat.",
            )
            .await;
            return Ok(());
        }
    }

    info!(
        "[BLUEBUBBLES] Message from {} in {}: {}",
        handle,
        chat_guid,
        &text_body[..text_body.len().min(80)]
    );

    let state_clone = state.clone();
    let chat_guid_owned = chat_guid.to_string();
    let handle_owned = handle.to_string();
    let text_owned = text_body.to_string();

    tokio::spawn(async move {
        handle_bluebubbles_text_message(&state_clone, &chat_guid_owned, &handle_owned, &text_owned)
            .await;
    });

    Ok(())
}

/// Process a text message through the Claude pipeline and send the response.
async fn handle_bluebubbles_text_message(
    state: &BlueBubblesState,
    chat_guid: &str,
    handle: &str,
    text: &str,
) {
    let app_state = &state.app_state;

    let claude_client = state.get_or_create_client(chat_guid).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::BlueBubbles {
            chat_guid: chat_guid.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_send_config = {
        let config = app_state.config.read().await;
        !config.bluebubbles.server_url.trim().is_empty()
            && !config.bluebubbles.password.trim().is_empty()
    };

    let observer = BlueBubblesObserver::new(
        app_state.clone(),
        chat_guid.to_string(),
        handle.to_string(),
        has_send_config,
        app_state.bluebubbles_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::bluebubbles(chat_guid.to_string(), handle.to_string()),
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
                send_bluebubbles_message(app_state, chat_guid, "(No response)").await;
            } else {
                for chunk in split_imessage(&response, 1024) {
                    send_bluebubbles_message(app_state, chat_guid, &chunk).await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            send_bluebubbles_message(app_state, chat_guid, &msg).await;
        }
        Err(e) => {
            send_bluebubbles_message(app_state, chat_guid, &format!("Error: {}", e)).await;
        }
    }
}

/// Send a text message via the BlueBubbles REST API.
pub async fn send_bluebubbles_message(app_state: &AppState, chat_guid: &str, text: &str) {
    let _ = send_bluebubbles_message_checked(app_state, chat_guid, text).await;
}

async fn send_bluebubbles_message_checked(
    app_state: &AppState,
    chat_guid: &str,
    text: &str,
) -> bool {
    let (server_url, password) = {
        let config = app_state.config.read().await;
        (
            config.bluebubbles.server_url.clone(),
            app_state
                .key_interceptor
                .restore_config_string(&config.bluebubbles.password),
        )
    };

    if server_url.is_empty() || password.is_empty() {
        error!("[BLUEBUBBLES] Cannot send: server_url or password not configured");
        return false;
    }

    let url = match build_bluebubbles_send_url(&server_url, &password) {
        Ok(url) => url,
        Err(e) => {
            error!("[BLUEBUBBLES] Cannot send: {}", e);
            return false;
        }
    };

    // Generate a simple temp GUID for deduplication on the BlueBubbles side
    let temp_guid = format!(
        "nexibot-{}-{}",
        chat_guid,
        chrono::Utc::now().timestamp_millis()
    );

    let body = serde_json::json!({
        "chatGuid": chat_guid,
        "message": text,
        "tempGuid": temp_guid,
    });

    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    match client.post(&url).json(&body).send().await {
        Ok(resp) if resp.status().is_success() => {
            info!("[BLUEBUBBLES] Message sent to chat {}", chat_guid);
            true
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            error!("[BLUEBUBBLES] Send failed ({}): {}", status, body);
            false
        }
        Err(e) => {
            error!("[BLUEBUBBLES] HTTP error sending message: {}", e);
            false
        }
    }
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

fn build_bluebubbles_ws_url(server_url: &str, password: &str) -> Result<String, String> {
    let mut url =
        Url::parse(server_url).map_err(|e| format!("invalid BlueBubbles server_url: {}", e))?;
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
                "unsupported BlueBubbles server_url scheme '{}'",
                scheme
            ));
        }
    }
    let path = append_path_segment(url.path(), "socket.io/websocket");
    url.set_path(&path);
    url.set_query(None);
    url.query_pairs_mut().append_pair("password", password);
    Ok(url.to_string())
}

fn build_bluebubbles_send_url(server_url: &str, password: &str) -> Result<String, String> {
    let mut url =
        Url::parse(server_url).map_err(|e| format!("invalid BlueBubbles server_url: {}", e))?;
    let path = append_path_segment(url.path(), "api/v1/message/text");
    url.set_path(&path);
    url.set_query(None);
    url.query_pairs_mut().append_pair("password", password);
    Ok(url.to_string())
}

/// Split a message into chunks of at most `max_len` characters.
/// Prefers splitting on newlines or spaces to avoid breaking mid-word.
fn split_imessage(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while remaining.len() > max_len {
        // Try to find a good split point (newline, then space, then hard cut)
        let split_at = remaining[..max_len]
            .rfind('\n')
            .or_else(|| remaining[..max_len].rfind(' '))
            .map(|i| i + 1)
            .unwrap_or(max_len);

        chunks.push(remaining[..split_at].to_string());
        remaining = &remaining[split_at..];
    }

    if !remaining.is_empty() {
        chunks.push(remaining.to_string());
    }

    chunks
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum number of concurrent BlueBubbles chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale BlueBubbles chat sessions (>24h inactive).
pub async fn session_cleanup_loop(state: Arc<BlueBubblesState>) {
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
                "[BLUEBUBBLES] Cleaned up {} stale sessions ({} remaining)",
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
                "[BLUEBUBBLES] Evicted {} oldest sessions to enforce cap (now {})",
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
    fn test_build_bluebubbles_ws_url_encodes_password_and_sets_ws_scheme() {
        let url = build_bluebubbles_ws_url("https://example.com/bridge", "p@ss word&token")
            .expect("ws url should build");
        assert!(url.starts_with("wss://example.com/bridge/socket.io/websocket?"));
        assert!(url.contains("password=p%40ss+word%26token"));
    }

    #[test]
    fn test_build_bluebubbles_send_url_encodes_password_and_path() {
        let url = build_bluebubbles_send_url("http://localhost:1234", "abc+123&x")
            .expect("send url should build");
        assert_eq!(
            url,
            "http://localhost:1234/api/v1/message/text?password=abc%2B123%26x"
        );
    }
}
