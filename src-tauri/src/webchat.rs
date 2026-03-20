//! Self-hosted WebChat browser widget for NexiBot.
//!
//! Hosts a WebSocket server (port 18792 by default) for browser-based chat.
//! Serves a JavaScript embed widget at GET /webchat/widget.js.
//!
//! Protocol (JSON over WebSocket):
//!   Client -> Server: { "type": "message", "content": "...", "session_id": "uuid" }
//!   Server -> Client: { "type": "message", "content": "...", "session_id": "uuid" }
//!   Server -> Client: { "type": "status",  "content": "thinking", "session_id": "uuid" }
//!   Server -> Client: { "type": "error",   "content": "...",      "session_id": "uuid" }

use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request, Response as WsResponse,
};
use tokio_tungstenite::{accept_hdr_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

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

fn default_webchat_port() -> u16 {
    18792
}
fn default_max_connections() -> usize {
    100
}
fn default_session_timeout() -> u64 {
    30
}

/// Configuration for the self-hosted WebChat widget server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebChatConfig {
    /// Whether the WebChat server is enabled.
    pub enabled: bool,

    /// Port for the WebSocket server. Default: 18792.
    #[serde(default = "default_webchat_port")]
    pub port: u16,

    /// Allowed origins for CORS / Origin header checks.
    /// Empty = allow all origins.
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// Whether to require an API key in the client handshake.
    #[serde(default)]
    pub require_api_key: bool,

    /// The API key clients must present when require_api_key is true.
    #[serde(default)]
    pub api_key: Option<String>,

    /// Maximum simultaneous WebSocket connections. Default: 100.
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,

    /// Session inactivity timeout in minutes. Default: 30.
    #[serde(default = "default_session_timeout")]
    pub session_timeout_minutes: u64,

    /// Allowlist of session IDs permitted to connect. Empty = allow all sessions.
    #[serde(default)]
    pub allowed_session_ids: Vec<String>,

    /// Admin session IDs — bypass DM policy (always allowed, elevated access).
    #[serde(default)]
    pub admin_session_ids: Vec<String>,

    /// DM access policy (applied per session_id).
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,

    /// Per-channel tool access policy.
    #[serde(default)]
    pub tool_policy: crate::config::ChannelToolPolicy,
}

impl Default for WebChatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_webchat_port(),
            allowed_origins: Vec::new(),
            require_api_key: false,
            api_key: None,
            max_connections: default_max_connections(),
            session_timeout_minutes: default_session_timeout(),
            allowed_session_ids: Vec::new(),
            admin_session_ids: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket message types
// ---------------------------------------------------------------------------

/// A JSON message sent from the browser client to NexiBot.
#[derive(Debug, Deserialize)]
struct ClientMessage {
    #[serde(rename = "type")]
    msg_type: String,
    content: Option<String>,
    session_id: Option<String>,
    /// Optional API key for authenticated sessions.
    api_key: Option<String>,
}

/// A JSON message sent from NexiBot to the browser client.
#[derive(Debug, Serialize)]
struct ServerMessage<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    content: &'a str,
    session_id: &'a str,
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-browser-tab conversation session.
pub struct WebChatSession {
    /// Dedicated Claude client with its own conversation history.
    claude_client: ClaudeClient,
    /// Last activity timestamp for session expiry.
    last_activity: Instant,
}

/// Shared state for the WebChat server.
pub struct WebChatState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, WebChatSession>>,
    pub connection_count: Arc<AtomicUsize>,
    pub rate_limiter: Arc<RateLimiter>,
    pub msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for WebChat background tool execution approvals.
pub(crate) struct WebChatObserver {
    app_state: AppState,
    requester_session_id: String,
    pending_approvals: Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
}

impl WebChatObserver {
    pub(crate) fn new(
        app_state: AppState,
        requester_session_id: String,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            requester_session_id,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for WebChatObserver {
    fn supports_approval(&self) -> bool {
        !self.requester_session_id.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let has_active_session = self
            .app_state
            .webchat_session_senders
            .read()
            .await
            .contains_key(&self.requester_session_id);
        if !has_active_session {
            warn!(
                "[WEBCHAT] Cannot request approval: no active websocket for session {}",
                self.requester_session_id
            );
            return false;
        }

        let key = self.requester_session_id.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                let _ = send_webchat_server_message(
                    &self.app_state,
                    &self.requester_session_id,
                    "message",
                    "Another approval is already pending for this session. Denying this request.",
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
        if !send_webchat_server_message(
            &self.app_state,
            &self.requester_session_id,
            "message",
            &prompt,
        )
        .await
        {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                let _ = send_webchat_server_message(
                    &self.app_state,
                    &self.requester_session_id,
                    "message",
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

impl WebChatState {
    pub fn new(app_state: AppState) -> Self {
        Self {
            app_state,
            chat_sessions: RwLock::new(HashMap::new()),
            connection_count: Arc::new(AtomicUsize::new(0)),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig {
                max_attempts: 30,
                window_seconds: 60,
                lockout_seconds: 30,
            })),
            msg_dedup: Mutex::new(LruCache::new(NonZeroUsize::new(10_000).unwrap())),
        }
    }

    /// Get or create a Claude client for the given session ID.
    async fn get_or_create_client(&self, session_id: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            session_id.to_string(),
            WebChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

async fn send_webchat_server_message(
    app_state: &AppState,
    session_id: &str,
    msg_type: &str,
    content: &str,
) -> bool {
    let payload = serde_json::json!({
        "type": msg_type,
        "content": content,
        "session_id": session_id,
    })
    .to_string();

    let sender = app_state
        .webchat_session_senders
        .read()
        .await
        .get(session_id)
        .cloned();
    if let Some(sender) = sender {
        if sender.send(payload).is_err() {
            warn!(
                "[WEBCHAT] Failed to enqueue outbound message for session {}",
                session_id
            );
            false
        } else {
            true
        }
    } else {
        warn!(
            "[WEBCHAT] Cannot send message: no active websocket for session {}",
            session_id
        );
        false
    }
}

fn origin_allowed(origin: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }

    allowed.iter().any(|entry| {
        let candidate = entry.trim();
        if candidate == "*" {
            return true;
        }

        // Exact origin match (scheme://host[:port]) first.
        if origin.eq_ignore_ascii_case(candidate) {
            return true;
        }

        // Host-only allowlist entry (e.g. "example.com").
        if let Ok(parsed) = url::Url::parse(origin) {
            if let Some(host) = parsed.host_str() {
                if host.eq_ignore_ascii_case(candidate) {
                    return true;
                }
            }
        }

        false
    })
}

fn is_valid_webchat_session_id(candidate: &str) -> bool {
    let id = candidate.trim();
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
}

async fn is_webchat_enabled(app_state: &AppState) -> bool {
    let config = app_state.config.read().await;
    config.webchat.enabled
}

// ---------------------------------------------------------------------------
// WebSocket server
// ---------------------------------------------------------------------------

/// Start the WebChat WebSocket server on the configured port.
pub async fn start_webchat_server(app_state: AppState) -> Result<(), String> {
    let (enabled, port, max_connections) = {
        let config = app_state.config.read().await;
        (
            config.webchat.enabled,
            config.webchat.port,
            config.webchat.max_connections,
        )
    };

    if !enabled {
        info!("[WEBCHAT] WebChat server disabled in config");
        return Ok(());
    }

    let bind_addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| format!("[WEBCHAT] Failed to bind to {}: {}", bind_addr, e))?;

    info!("[WEBCHAT] WebSocket server listening on ws://{}", bind_addr);

    let state = Arc::new(WebChatState::new(app_state));

    // Spawn session cleanup task.
    let cleanup_state = state.clone();
    tokio::spawn(session_cleanup_loop(cleanup_state));

    loop {
        if !is_webchat_enabled(&state.app_state).await {
            info!("[WEBCHAT] WebChat disabled in config — stopping WebSocket listener");
            break;
        }

        let accept_result = tokio::select! {
            result = listener.accept() => Some(result),
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => None,
        };

        let Some(accept_result) = accept_result else {
            continue;
        };

        match accept_result {
            Ok((stream, peer_addr)) => {
                // Enforce connection cap.
                let current = state.connection_count.load(Ordering::Relaxed);
                if current >= max_connections {
                    warn!(
                        "[WEBCHAT] Connection limit ({}) reached, rejecting {}",
                        max_connections, peer_addr
                    );
                    drop(stream);
                    continue;
                }

                let state_clone = state.clone();
                tokio::spawn(async move {
                    state_clone.connection_count.fetch_add(1, Ordering::Relaxed);
                    if let Err(e) = handle_connection(state_clone.clone(), stream).await {
                        warn!("[WEBCHAT] Connection from {} error: {}", peer_addr, e);
                    }
                    state_clone.connection_count.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) => {
                error!("[WEBCHAT] Accept error: {}", e);
            }
        }
    }

    Ok(())
}

/// Handle a single WebSocket connection from a browser client.
async fn handle_connection(
    state: Arc<WebChatState>,
    stream: tokio::net::TcpStream,
) -> Result<(), String> {
    let allowed_origins = {
        let config = state.app_state.config.read().await;
        config.webchat.allowed_origins.clone()
    };

    let ws_stream = accept_hdr_async(stream, move |req: &Request, response: WsResponse| {
        let origin = req
            .headers()
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if origin_allowed(origin, &allowed_origins) {
            return Ok(response);
        }

        let mut denied = ErrorResponse::new(Some("Origin not allowed".to_string()));
        *denied.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
        Err(denied)
    })
    .await
    .map_err(|e| format!("WebSocket handshake error: {}", e))?;

    let (mut write, mut read) = ws_stream.split();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (completion_tx, mut completion_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    // Each connection gets a server-assigned session UUID by default.
    // The client can override it by sending session_id in their first message.
    let mut session_id = Uuid::new_v4().to_string();
    let mut registered_session_id: Option<String> = None;
    let mut authenticated = false;
    let mut request_in_flight = false;

    loop {
        if !is_webchat_enabled(&state.app_state).await {
            let err_msg = ServerMessage {
                msg_type: "error",
                content: "WebChat is disabled by configuration.",
                session_id: &session_id,
            };
            let _ = write
                .send(Message::Text(
                    serde_json::to_string(&err_msg).unwrap_or_default().into(),
                ))
                .await;
            let _ = write.close().await;
            break;
        }

        let msg_result = tokio::select! {
            completion = completion_rx.recv() => {
                if completion.is_some() {
                    request_in_flight = false;
                }
                continue;
            }
            outbound_payload = outbound_rx.recv() => {
                match outbound_payload {
                    Some(payload) => {
                        if let Err(e) = write.send(Message::Text(payload.into())).await {
                            warn!("[WEBCHAT] Failed to send outbound event: {}", e);
                            break;
                        }
                        continue;
                    }
                    None => break,
                }
            }
            next = read.next() => {
                match next {
                    Some(msg_result) => msg_result,
                    None => break,
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                continue;
            }
        };

        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => return Err(format!("WebSocket read error: {}", e)),
        };

        let text = match msg {
            Message::Text(t) => t,
            Message::Ping(data) => {
                let _ = write.send(Message::Pong(data)).await;
                continue;
            }
            Message::Close(_) => {
                info!("[WEBCHAT] Client disconnected (session {})", session_id);
                break;
            }
            _ => continue,
        };

        let client_msg: ClientMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                warn!("[WEBCHAT] Invalid JSON from client: {}", e);
                let err_msg = ServerMessage {
                    msg_type: "error",
                    content: "Invalid message format",
                    session_id: &session_id,
                };
                let _ = write
                    .send(Message::Text(
                        serde_json::to_string(&err_msg).unwrap_or_default(),
                    ))
                    .await;
                continue;
            }
        };

        // ---- API key check ----
        let (require_api_key, configured_key) = {
            let config = state.app_state.config.read().await;
            (
                config.webchat.require_api_key,
                config
                    .webchat
                    .api_key
                    .as_ref()
                    .map(|k| state.app_state.key_interceptor.restore_config_string(k)),
            )
        };

        if require_api_key && !authenticated {
            let provided = client_msg.api_key.as_deref().unwrap_or("");
            let expected = configured_key.as_deref().unwrap_or("");
            if expected.is_empty()
                || !crate::security::constant_time::secure_compare(provided, expected)
            {
                warn!("[WEBCHAT] Invalid API key from client");
                let err_msg = ServerMessage {
                    msg_type: "error",
                    content: "Unauthorized: invalid API key",
                    session_id: &session_id,
                };
                let _ = write
                    .send(Message::Text(
                        serde_json::to_string(&err_msg).unwrap_or_default(),
                    ))
                    .await;
                break;
            }
            authenticated = true;
        } else {
            authenticated = true;
        }

        // ---- Use client-provided session_id only for initial registration ----
        if registered_session_id.is_none() {
            let mut invalid_session_id = false;
            let mut session_id_in_use = false;
            let requested_session_id = client_msg
                .session_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);

            let valid_requested = match requested_session_id {
                Some(candidate) if is_valid_webchat_session_id(&candidate) => Some(candidate),
                Some(_) => {
                    invalid_session_id = true;
                    None
                }
                None => None,
            };

            {
                let mut senders = state.app_state.webchat_session_senders.write().await;
                if let Some(candidate) = valid_requested {
                    if senders.contains_key(&candidate) {
                        session_id_in_use = true;
                    } else {
                        session_id = candidate;
                    }
                }
                senders.insert(session_id.clone(), outbound_tx.clone());
                registered_session_id = Some(session_id.clone());
            }

            if invalid_session_id {
                warn!(
                    "[WEBCHAT] Rejected invalid session_id, using server-assigned id {}",
                    session_id
                );
                let err_msg = ServerMessage {
                    msg_type: "error",
                    content: "Invalid session_id format; using server-assigned session.",
                    session_id: &session_id,
                };
                let _ = write
                    .send(Message::Text(
                        serde_json::to_string(&err_msg).unwrap_or_default().into(),
                    ))
                    .await;
            }
            if session_id_in_use {
                warn!(
                    "[WEBCHAT] Rejected session_id takeover attempt; using server-assigned id {}",
                    session_id
                );
                let err_msg = ServerMessage {
                    msg_type: "error",
                    content:
                        "Requested session_id is already active; using server-assigned session.",
                    session_id: &session_id,
                };
                let _ = write
                    .send(Message::Text(
                        serde_json::to_string(&err_msg).unwrap_or_default().into(),
                    ))
                    .await;
            }
        }

        if client_msg.msg_type != "message" {
            continue;
        }

        let content = match client_msg.content {
            Some(ref c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => continue,
        };

        // ---- DM policy ----
        {
            let config = state.app_state.config.read().await;
            let dm_policy = config.webchat.dm_policy;
            drop(config);

            match dm_policy {
                crate::pairing::DmPolicy::Open => {}
                crate::pairing::DmPolicy::Allowlist => {
                    if !require_api_key {
                        warn!(
                            "[WEBCHAT] Rejecting message for session {}: dm_policy=Allowlist requires webchat.require_api_key=true",
                            session_id
                        );
                        let err_msg = ServerMessage {
                            msg_type: "error",
                            content:
                                "WebChat dm_policy=Allowlist requires webchat.require_api_key=true.",
                            session_id: &session_id,
                        };
                        let _ = write
                            .send(Message::Text(
                                serde_json::to_string(&err_msg).unwrap_or_default().into(),
                            ))
                            .await;
                        continue;
                    }
                }
                crate::pairing::DmPolicy::Pairing => {
                    let pairing_mgr = state.app_state.pairing_manager.read().await;
                    let empty_allowed: Vec<String> = Vec::new();
                    if !pairing_mgr.is_channel_allowed("webchat", &session_id, &empty_allowed) {
                        drop(pairing_mgr);
                        let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                        match pairing_mgr.create_pairing_request("webchat", &session_id, None) {
                            Ok(code) => {
                                drop(pairing_mgr);
                                let pairing_msg = format!(
                                    "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                    code
                                );
                                let srv_msg = ServerMessage {
                                    msg_type: "message",
                                    content: &pairing_msg,
                                    session_id: &session_id,
                                };
                                let _ = write
                                    .send(Message::Text(
                                        serde_json::to_string(&srv_msg).unwrap_or_default(),
                                    ))
                                    .await;
                            }
                            Err(e) => {
                                drop(pairing_mgr);
                                let pending_msg = format!("Authorization pending. {}", e);
                                let srv_msg = ServerMessage {
                                    msg_type: "message",
                                    content: &pending_msg,
                                    session_id: &session_id,
                                };
                                let _ = write
                                    .send(Message::Text(
                                        serde_json::to_string(&srv_msg).unwrap_or_default(),
                                    ))
                                    .await;
                            }
                        }
                        continue;
                    }
                }
            }
        }

        let content_lc = content.trim().to_lowercase();
        if matches!(
            content_lc.as_str(),
            "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
        ) {
            let approval_tx = state
                .app_state
                .webchat_pending_approvals
                .lock()
                .await
                .remove(&session_id);
            if let Some(approval_tx) = approval_tx {
                let approved = matches!(content_lc.as_str(), "approve" | "/approve" | "!approve");
                let reply = if approval_tx.send(approved).is_ok() {
                    if approved {
                        "Approved. Continuing..."
                    } else {
                        "Denied."
                    }
                } else {
                    warn!(
                        "[WEBCHAT] Approval response could not be delivered for session {}; request likely expired",
                        session_id
                    );
                    "This approval request is no longer active."
                };
                let srv_msg = ServerMessage {
                    msg_type: "message",
                    content: reply,
                    session_id: &session_id,
                };
                let _ = write
                    .send(Message::Text(
                        serde_json::to_string(&srv_msg).unwrap_or_default().into(),
                    ))
                    .await;
                continue;
            }

            let srv_msg = ServerMessage {
                msg_type: "message",
                content: "No active approval request.",
                session_id: &session_id,
            };
            let _ = write
                .send(Message::Text(
                    serde_json::to_string(&srv_msg).unwrap_or_default().into(),
                ))
                .await;
            continue;
        }

        // --- Message deduplication (session_id + content hash) ---
        {
            let dedup_key = format!("{}:{}", session_id, content);
            let mut dedup = state.msg_dedup.lock().await;
            if dedup.put(dedup_key.clone(), ()).is_some() {
                debug!("[WEBCHAT] Dropping duplicate message from session: {}", session_id);
                continue;
            }
        }

        // --- Rate limiting per session ---
        {
            let rate_key = format!("webchat:{}", session_id);
            if state.rate_limiter.check(&rate_key).is_err() {
                warn!("[WEBCHAT] Rate limit exceeded for session: {}", session_id);
                let rate_msg = ServerMessage {
                    msg_type: "error",
                    content: "Rate limit exceeded. Please slow down.",
                    session_id: &session_id,
                };
                let _ = write
                    .send(Message::Text(
                        serde_json::to_string(&rate_msg).unwrap_or_default().into(),
                    ))
                    .await;
                continue;
            }
        }

        if request_in_flight {
            let busy_msg = ServerMessage {
                msg_type: "status",
                content: "busy",
                session_id: &session_id,
            };
            let _ = write
                .send(Message::Text(
                    serde_json::to_string(&busy_msg).unwrap_or_default().into(),
                ))
                .await;
            continue;
        }

        request_in_flight = true;
        let state_clone = state.clone();
        let session_id_owned = session_id.clone();
        let content_owned = content.clone();
        let completion_tx_clone = completion_tx.clone();
        tokio::spawn(async move {
            let _ = send_webchat_server_message(
                &state_clone.app_state,
                &session_id_owned,
                "status",
                "thinking",
            )
            .await;

            info!(
                "[WEBCHAT] Message from session {}: {}",
                session_id_owned, content_owned
            );

            let claude_client = state_clone.get_or_create_client(&session_id_owned).await;

            let message = IncomingMessage {
                text: content_owned.clone(),
                channel: ChannelSource::WebChat {
                    session_id: session_id_owned.clone(),
                },
                agent_id: None,
                metadata: HashMap::new(),
            };

            let observer = WebChatObserver::new(
                state_clone.app_state.clone(),
                session_id_owned.clone(),
                state_clone.app_state.webchat_pending_approvals.clone(),
            );
            let options = RouteOptions {
                claude_client: &claude_client,
                overrides: SessionOverrides::default(),
                loop_config: ToolLoopConfig::webchat(session_id_owned.clone()),
                observer: &observer,
                streaming: false,
                window: None,
                on_stream_chunk: None,
                auto_compact: true,
                save_to_memory: true,
                sync_supermemory: true,
                check_sensitive_data: true,
            };

            let app_state = &state_clone.app_state;
            let reply_text = match router::route_message(&message, options, app_state).await {
                Ok(routed) => {
                    let response = router::extract_text_from_response(&routed.text);
                    if response.is_empty() {
                        "(No response)".to_string()
                    } else {
                        response
                    }
                }
                Err(RouterError::Blocked(msg)) => msg,
                Err(e) => format!("Error: {}", e),
            };

            for chunk in router::split_message(&reply_text, 4096) {
                let _ =
                    send_webchat_server_message(app_state, &session_id_owned, "message", &chunk)
                        .await;
            }

            let _ = completion_tx_clone.send(());
        });
    }

    if let Some(sid) = registered_session_id {
        state
            .app_state
            .webchat_session_senders
            .write()
            .await
            .remove(&sid);
        if let Some(approval_tx) = state
            .app_state
            .webchat_pending_approvals
            .lock()
            .await
            .remove(&sid)
        {
            if approval_tx.send(false).is_err() {
                warn!(
                    "[WEBCHAT] Pending approval for session {} was already closed during disconnect",
                    sid
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Widget JS handler (served via Axum route /webchat/widget.js)
// ---------------------------------------------------------------------------

/// Minimal vanilla-JS embed widget for NexiBot WebChat.
static WIDGET_JS: &str = r#"
// NexiBot WebChat Widget v1
(function () {
  'use strict';
  var NEXIBOT_WS_PORT = 18792;
  var STORAGE_KEY = 'nexibot_session_id';

  function getOrCreateSessionId() {
    var id = sessionStorage.getItem(STORAGE_KEY);
    if (!id) {
      id = 'ws-' + Math.random().toString(36).slice(2) + Date.now().toString(36);
      sessionStorage.setItem(STORAGE_KEY, id);
    }
    return id;
  }

  function createWidget(port) {
    var sessionId = getOrCreateSessionId();
    var ws = null;
    var open = false;

    // --- Styles ---
    var style = document.createElement('style');
    style.textContent = [
      '#nexibot-btn{position:fixed;bottom:24px;right:24px;width:56px;height:56px;',
      'border-radius:50%;background:#4f46e5;color:#fff;font-size:24px;border:none;',
      'cursor:pointer;box-shadow:0 4px 12px rgba(0,0,0,.3);z-index:99999;}',
      '#nexibot-panel{position:fixed;bottom:92px;right:24px;width:340px;height:480px;',
      'display:none;flex-direction:column;border-radius:12px;overflow:hidden;',
      'box-shadow:0 8px 32px rgba(0,0,0,.25);z-index:99998;font-family:sans-serif;}',
      '#nexibot-header{background:#4f46e5;color:#fff;padding:12px 16px;font-weight:600;}',
      '#nexibot-msgs{flex:1;overflow-y:auto;padding:12px;background:#f9fafb;}',
      '#nexibot-msgs .nb-msg{margin-bottom:8px;padding:8px 12px;border-radius:8px;',
      'max-width:85%;word-break:break-word;font-size:14px;line-height:1.4;}',
      '#nexibot-msgs .nb-user{background:#4f46e5;color:#fff;margin-left:auto;}',
      '#nexibot-msgs .nb-bot{background:#e5e7eb;color:#111;}',
      '#nexibot-msgs .nb-status{color:#9ca3af;font-size:12px;font-style:italic;}',
      '#nexibot-input-row{display:flex;padding:8px;background:#fff;border-top:1px solid #e5e7eb;}',
      '#nexibot-input{flex:1;border:1px solid #d1d5db;border-radius:8px;padding:8px 12px;',
      'font-size:14px;outline:none;}',
      '#nexibot-send{margin-left:8px;background:#4f46e5;color:#fff;border:none;',
      'border-radius:8px;padding:8px 14px;cursor:pointer;font-size:14px;}'
    ].join('');
    document.head.appendChild(style);

    // --- DOM ---
    var btn = document.createElement('button');
    btn.id = 'nexibot-btn';
    btn.textContent = '💬';
    btn.title = 'Chat with NexiBot';

    var panel = document.createElement('div');
    panel.id = 'nexibot-panel';
    panel.style.display = 'none';

    var header = document.createElement('div');
    header.id = 'nexibot-header';
    header.textContent = 'NexiBot';

    var msgs = document.createElement('div');
    msgs.id = 'nexibot-msgs';

    var inputRow = document.createElement('div');
    inputRow.id = 'nexibot-input-row';

    var input = document.createElement('input');
    input.id = 'nexibot-input';
    input.type = 'text';
    input.placeholder = 'Type a message...';

    var sendBtn = document.createElement('button');
    sendBtn.id = 'nexibot-send';
    sendBtn.textContent = 'Send';

    inputRow.appendChild(input);
    inputRow.appendChild(sendBtn);
    panel.appendChild(header);
    panel.appendChild(msgs);
    panel.appendChild(inputRow);
    document.body.appendChild(btn);
    document.body.appendChild(panel);

    // --- WebSocket ---
    function connect() {
      ws = new WebSocket('ws://localhost:' + port);
      ws.onmessage = function (e) {
        try {
          var data = JSON.parse(e.data);
          if (data.type === 'status') {
            addMsg(data.content, 'nb-status');
          } else if (data.type === 'message') {
            removeStatus();
            addMsg(data.content, 'nb-bot');
          } else if (data.type === 'error') {
            removeStatus();
            addMsg('Error: ' + data.content, 'nb-bot');
          }
        } catch (_) {}
      };
      ws.onclose = function () {
        setTimeout(connect, 3000);
      };
    }

    function addMsg(text, cls) {
      var el = document.createElement('div');
      el.className = 'nb-msg ' + cls;
      el.textContent = text;
      msgs.appendChild(el);
      msgs.scrollTop = msgs.scrollHeight;
    }

    function removeStatus() {
      var statuses = msgs.querySelectorAll('.nb-status');
      statuses.forEach(function (el) { el.remove(); });
    }

    function sendMessage() {
      var text = input.value.trim();
      if (!text || !ws || ws.readyState !== 1) return;
      addMsg(text, 'nb-user');
      ws.send(JSON.stringify({ type: 'message', content: text, session_id: sessionId }));
      input.value = '';
    }

    sendBtn.addEventListener('click', sendMessage);
    input.addEventListener('keydown', function (e) {
      if (e.key === 'Enter') sendMessage();
    });

    btn.addEventListener('click', function () {
      open = !open;
      panel.style.display = open ? 'flex' : 'none';
      if (open && (!ws || ws.readyState > 1)) connect();
    });

    connect();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', function () { createWidget(NEXIBOT_WS_PORT); });
  } else {
    createWidget(NEXIBOT_WS_PORT);
  }
})();
"#;

fn widget_js_for_port(port: u16) -> String {
    WIDGET_JS.replace(
        "var NEXIBOT_WS_PORT = 18792;",
        &format!("var NEXIBOT_WS_PORT = {};", port),
    )
}

/// Build the widget JS response with a runtime-selected WebSocket port.
pub fn widget_js_response(port: u16) -> Response {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        widget_js_for_port(port),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Periodically evict idle WebChat sessions based on session_timeout_minutes.
pub async fn session_cleanup_loop(state: Arc<WebChatState>) {
    let cleanup_interval = tokio::time::Duration::from_secs(300); // 5 min

    loop {
        tokio::time::sleep(cleanup_interval).await;

        let timeout_minutes = {
            let config = state.app_state.config.read().await;
            config.webchat.session_timeout_minutes
        };
        let max_age = std::time::Duration::from_secs(timeout_minutes * 60);

        let mut sessions = state.chat_sessions.write().await;
        let before = sessions.len();
        sessions.retain(|_, session| session.last_activity.elapsed() < max_age);
        let removed = before - sessions.len();
        if removed > 0 {
            info!(
                "[WEBCHAT] Cleaned up {} idle sessions ({} remaining)",
                removed,
                sessions.len()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_valid_webchat_session_id;

    #[test]
    fn test_is_valid_webchat_session_id_accepts_safe_values() {
        assert!(is_valid_webchat_session_id("ws-abc123"));
        assert!(is_valid_webchat_session_id("session_01.user:alpha"));
        assert!(is_valid_webchat_session_id("A1_b2-C3.d4:e5"));
    }

    #[test]
    fn test_is_valid_webchat_session_id_rejects_unsafe_values() {
        assert!(!is_valid_webchat_session_id(""));
        assert!(!is_valid_webchat_session_id("   "));
        assert!(!is_valid_webchat_session_id("bad/session"));
        assert!(!is_valid_webchat_session_id("bad session"));
        assert!(!is_valid_webchat_session_id(&"a".repeat(129)));
    }
}
