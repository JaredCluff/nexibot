//! Matrix messaging integration via Client-Server API.
//!
//! NOTE: Full E2EE support requires the matrix-sdk crate.
//! This implementation uses the plain HTTP Client-Server API
//! for basic unencrypted room messaging.
//!
//! Uses the Matrix /sync endpoint for long-polling and sends replies via
//! PUT /_matrix/client/v3/rooms/{roomId}/send/m.room.message/{txnId}

use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::commands::AppState;
use crate::router::{self, IncomingMessage, RouteOptions, RouterError};
use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::ToolLoopConfig;

static MATRIX_APPROVAL_TXN_COUNTER: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Matrix adapter (ChannelAdapter implementation)
// ---------------------------------------------------------------------------

/// Matrix adapter that sends messages via the Matrix Client-Server API.
#[allow(dead_code)]
pub struct MatrixAdapter {
    source: ChannelSource,
    homeserver_url: String,
    access_token: String,
    room_id: String,
    txn_counter: Arc<AtomicU64>,
}

impl MatrixAdapter {
    #[allow(dead_code)]
    pub fn new(homeserver_url: String, access_token: String, room_id: String) -> Self {
        Self {
            source: ChannelSource::Matrix {
                room_id: room_id.clone(),
            },
            homeserver_url,
            access_token,
            room_id,
            txn_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    #[allow(dead_code)]
    fn next_txn_id(&self) -> String {
        let counter = self.txn_counter.fetch_add(1, Ordering::SeqCst);
        format!("nexi_{}_{}", std::process::id(), counter)
    }
}

#[async_trait::async_trait]
impl crate::channel::ChannelAdapter for MatrixAdapter {
    async fn send_response(&self, text: &str) -> Result<(), String> {
        let response = router::extract_text_from_response(text);
        if response.is_empty() {
            send_matrix_message(
                &self.homeserver_url,
                &self.access_token,
                &self.room_id,
                "(No response)",
                &self.next_txn_id(),
            )
            .await
            .map_err(|e| e.to_string())?;
            return Ok(());
        }

        // Matrix has no strict character limit, but split at 4096 for readability.
        for chunk in router::split_message(&response, 4096) {
            if let Err(e) = send_matrix_message(
                &self.homeserver_url,
                &self.access_token,
                &self.room_id,
                &chunk,
                &self.next_txn_id(),
            )
            .await
            {
                warn!("[MATRIX] Failed to send chunk: {}", e);
            }
        }
        Ok(())
    }

    async fn send_error(&self, error: &str) -> Result<(), String> {
        send_matrix_message(
            &self.homeserver_url,
            &self.access_token,
            &self.room_id,
            &format!("Error: {}", error),
            &self.next_txn_id(),
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn source(&self) -> &ChannelSource {
        &self.source
    }
}

// ---------------------------------------------------------------------------
// Per-room session state
// ---------------------------------------------------------------------------

/// Per-room session state for Matrix conversations.
struct MatrixChatSession {
    /// Dedicated Claude client with its own conversation history
    claude_client: ClaudeClient,
    /// Last activity timestamp
    last_activity: Instant,
}

/// Shared state for the Matrix sync loop.
pub struct MatrixBotState {
    /// Reference to the global app state
    app_state: AppState,
    /// Per-room sessions (room_id -> session)
    chat_sessions: RwLock<HashMap<String, MatrixChatSession>>,
    /// Transaction ID counter for idempotent sends
    txn_counter: AtomicU64,
    /// Per-sender rate limiter (10 messages per 60 seconds, 60-second lockout)
    rate_limiter: Arc<RateLimiter>,
    /// Recently-processed event IDs for deduplication
    msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for Matrix tool execution flow, including in-channel approvals.
pub(crate) struct MatrixObserver {
    homeserver_url: String,
    access_token: String,
    room_id: String,
    requester_user_id: String,
    pending_approvals:
        Arc<tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>>,
}

impl MatrixObserver {
    pub(crate) fn new(
        homeserver_url: String,
        access_token: String,
        room_id: String,
        requester_user_id: String,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            homeserver_url,
            access_token,
            room_id,
            requester_user_id,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for MatrixObserver {
    fn supports_approval(&self) -> bool {
        !self.homeserver_url.trim().is_empty()
            && !self.access_token.trim().is_empty()
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
                let _ = send_matrix_message(
                    &self.homeserver_url,
                    &self.access_token,
                    &self.room_id,
                    "Another approval is already pending for this requester. Denying this request.",
                    &next_matrix_approval_txn_id(),
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
        if send_matrix_message(
            &self.homeserver_url,
            &self.access_token,
            &self.room_id,
            &prompt,
            &next_matrix_approval_txn_id(),
        )
        .await
        .is_err()
        {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                let _ = send_matrix_message(
                    &self.homeserver_url,
                    &self.access_token,
                    &self.room_id,
                    "Approval timed out. Tool blocked.",
                    &next_matrix_approval_txn_id(),
                )
                .await;
                false
            }
        }
    }
}

impl MatrixBotState {
    fn new(app_state: AppState) -> Self {
        Self {
            app_state,
            chat_sessions: RwLock::new(HashMap::new()),
            txn_counter: AtomicU64::new(0),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig {
                max_attempts: 10,
                window_seconds: 60,
                lockout_seconds: 60,
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
            MatrixChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }

    /// Clear conversation history for a room.
    async fn clear_session(&self, room_id: &str) {
        let mut sessions = self.chat_sessions.write().await;
        sessions.remove(room_id);
    }

    /// Generate a unique transaction ID for idempotent sends.
    fn next_txn_id(&self) -> String {
        let counter = self.txn_counter.fetch_add(1, Ordering::SeqCst);
        format!("nexi_{}_{}", std::process::id(), counter)
    }
}

// ---------------------------------------------------------------------------
// Matrix /sync response structures (simplified)
// ---------------------------------------------------------------------------

/// Simplified Matrix /sync response.
#[derive(Debug, Deserialize)]
struct SyncResponse {
    /// Opaque token for next sync call.
    next_batch: String,
    /// Room updates.
    #[serde(default)]
    rooms: Option<SyncRooms>,
}

/// Room updates within a sync response.
#[derive(Debug, Deserialize)]
struct SyncRooms {
    /// Joined rooms with new events.
    #[serde(default)]
    join: HashMap<String, JoinedRoom>,
}

/// A joined room in the sync response.
#[derive(Debug, Deserialize)]
struct JoinedRoom {
    /// Timeline events (messages, state changes).
    #[serde(default)]
    timeline: Option<Timeline>,
}

/// Timeline within a joined room.
#[derive(Debug, Deserialize)]
struct Timeline {
    /// List of events.
    #[serde(default)]
    events: Vec<TimelineEvent>,
}

/// A single timeline event.
#[derive(Debug, Deserialize)]
struct TimelineEvent {
    /// Event type (e.g., "m.room.message").
    #[serde(rename = "type")]
    event_type: String,
    /// Sender user ID (e.g., "@user:matrix.org").
    #[serde(default)]
    sender: String,
    /// Event content.
    #[serde(default)]
    content: Value,
    /// Event ID.
    #[serde(default)]
    event_id: String,
}

// ---------------------------------------------------------------------------
// Matrix sync loop
// ---------------------------------------------------------------------------

/// Start the Matrix sync loop.
///
/// Uses the `/sync` endpoint with long-polling to receive new messages,
/// filters for `m.room.message` events, and routes them through the Claude
/// pipeline. Replies are sent via PUT on the send endpoint.
pub async fn start_matrix_sync(app_state: AppState) -> Result<()> {
    let config = app_state.config.read().await;
    if !config.matrix.enabled {
        info!("[MATRIX] Matrix integration disabled in config");
        return Ok(());
    }

    let homeserver_url = config.matrix.homeserver_url.clone();
    let access_token = app_state
        .key_interceptor
        .restore_config_string(&config.matrix.access_token);
    let bot_user_id = config.matrix.user_id.clone();
    drop(config);

    {
        use crate::security::ssrf::{self, SsrfPolicy};
        if let Err(e) = ssrf::validate_outbound_request(&homeserver_url, &SsrfPolicy::default(), &[]) {
            return Err(anyhow::anyhow!("Matrix homeserver_url blocked by SSRF policy: {}", e));
        }
    }

    if access_token.is_empty() {
        warn!("[MATRIX] Matrix enabled but no access_token configured");
        return Err(anyhow::anyhow!("Matrix access_token not configured"));
    }

    if bot_user_id.is_empty() {
        warn!("[MATRIX] Matrix enabled but no user_id configured");
        return Err(anyhow::anyhow!("Matrix user_id not configured"));
    }

    let state = Arc::new(MatrixBotState::new(app_state));
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());

    info!(
        "[MATRIX] Starting Matrix sync loop (homeserver: {}, user: {})",
        homeserver_url, bot_user_id
    );

    // Spawn session cleanup task
    let cleanup_state = state.clone();
    tokio::spawn(session_cleanup_loop(cleanup_state));

    // Initial sync to get the since token (filter out old messages)
    let mut since_token = initial_sync(&client, &homeserver_url, &access_token).await?;

    info!("[MATRIX] Initial sync complete, starting incremental sync");

    // Main sync loop — exponential backoff (1s..64s) on HTTP/network errors.
    let mut sync_backoff = std::time::Duration::from_secs(1);
    loop {
        // Allow runtime disabling without restart.
        {
            let config = state.app_state.config.read().await;
            if !config.matrix.enabled {
                info!("[MATRIX] Matrix integration disabled at runtime, stopping sync loop");
                break Ok(());
            }
        }

        match incremental_sync(&client, &homeserver_url, &access_token, &since_token).await {
            Ok(sync_response) => {
                // Successful sync — reset backoff.
                sync_backoff = std::time::Duration::from_secs(1);
                since_token = sync_response.next_batch.clone();

                let (allowed_room_ids, command_prefix) = {
                    let config = state.app_state.config.read().await;
                    (
                        config.matrix.allowed_room_ids.clone(),
                        config
                            .matrix
                            .command_prefix
                            .clone()
                            .unwrap_or_else(|| "!nexi".to_string()),
                    )
                };

                if let Some(rooms) = &sync_response.rooms {
                    for (room_id, joined_room) in &rooms.join {
                        // Check room allowlist
                        if !allowed_room_ids.is_empty() && !allowed_room_ids.contains(room_id) {
                            continue;
                        }

                        if let Some(timeline) = &joined_room.timeline {
                            for event in &timeline.events {
                                // Skip our own messages
                                if event.sender == bot_user_id {
                                    continue;
                                }

                                // Only handle m.room.message events
                                if event.event_type != "m.room.message" {
                                    continue;
                                }

                                let msgtype = event
                                    .content
                                    .get("msgtype")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");

                                if msgtype != "m.text" {
                                    continue;
                                }

                                let body = event
                                    .content
                                    .get("body")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                if body.is_empty() {
                                    continue;
                                }

                                // Check command prefix
                                let text = if !command_prefix.is_empty() {
                                    if body.starts_with(&command_prefix) {
                                        body[command_prefix.len()..].trim().to_string()
                                    } else {
                                        continue; // Does not start with prefix, ignore
                                    }
                                } else {
                                    body
                                };

                                if text.is_empty() {
                                    continue;
                                }

                                handle_matrix_message(
                                    &text,
                                    room_id,
                                    &event.sender,
                                    &event.event_id,
                                    &state,
                                    &homeserver_url,
                                    &access_token,
                                )
                                .await;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("[MATRIX] Sync error: {}. Retrying in {:?}", e, sync_backoff);
                tokio::time::sleep(sync_backoff).await;
                sync_backoff = (sync_backoff * 2).min(std::time::Duration::from_secs(64));
            }
        }
    }
}

/// Perform an initial sync to get the `since` token without processing old messages.
async fn initial_sync(
    client: &reqwest::Client,
    homeserver_url: &str,
    access_token: &str,
) -> Result<String> {
    let url = format!(
        "{}/_matrix/client/v3/sync?timeout=0&filter={{\"room\":{{\"timeline\":{{\"limit\":0}}}}}}",
        homeserver_url.trim_end_matches('/')
    );

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Matrix initial sync failed ({}): {}",
            status,
            body
        ));
    }

    let sync: SyncResponse = response.json().await?;
    Ok(sync.next_batch)
}

/// Perform an incremental sync with long-polling.
async fn incremental_sync(
    client: &reqwest::Client,
    homeserver_url: &str,
    access_token: &str,
    since: &str,
) -> Result<SyncResponse> {
    // Build URL with query() so reqwest percent-encodes `since`; raw string
    // interpolation would allow a malicious homeserver to inject extra
    // query params via a crafted next_batch token.
    let base = format!(
        "{}/_matrix/client/v3/sync",
        homeserver_url.trim_end_matches('/')
    );

    let response = client
        .get(&base)
        .query(&[("since", since), ("timeout", "30000")])
        .header("Authorization", format!("Bearer {}", access_token))
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Matrix sync failed ({}): {}", status, body));
    }

    let sync: SyncResponse = response.json().await?;
    Ok(sync)
}

/// Handle a single Matrix message event.
async fn handle_matrix_message(
    text: &str,
    room_id: &str,
    sender: &str,
    event_id: &str,
    state: &MatrixBotState,
    homeserver_url: &str,
    access_token: &str,
) {
    // Fix 2: Message deduplication using event_id
    if !event_id.is_empty() {
        let mut dedup = state.msg_dedup.lock().await;
        if dedup.put(event_id.to_string(), ()).is_some() {
            // Already processed this event
            return;
        }
    }

    // Fix 1: Per-sender rate limiting
    let rate_key = format!("matrix:{}", sender);
    if let Err(e) = state.rate_limiter.check(&rate_key) {
        warn!("[MATRIX] Rate limit hit for sender {}: {}", sender, e);
        let _ = send_matrix_message(
            homeserver_url,
            access_token,
            room_id,
            "Please slow down — too many messages. Try again in a moment.",
            &state.next_txn_id(),
        )
        .await;
        return;
    }

    let text_lc = text.trim().to_lowercase();

    // --- DM policy enforcement ---
    let (dm_policy, admin_user_ids) = {
        let config = state.app_state.config.read().await;
        (
            config.matrix.dm_policy,
            config.matrix.admin_user_ids.clone(),
        )
    };
    if !admin_user_ids.contains(&sender.to_string()) {
        match dm_policy {
            crate::pairing::DmPolicy::Open => {}
            crate::pairing::DmPolicy::Allowlist => {
                let allowed = {
                    let mgr = state.app_state.pairing_manager.read().await;
                    mgr.is_channel_allowed("matrix", sender, &admin_user_ids)
                };
                if !allowed {
                    let _ = send_matrix_message(
                        homeserver_url,
                        access_token,
                        room_id,
                        "You are not authorized to use this bot.",
                        &state.next_txn_id(),
                    )
                    .await;
                    return;
                }
            }
            crate::pairing::DmPolicy::Pairing => {
                let allowed = {
                    let mgr = state.app_state.pairing_manager.read().await;
                    mgr.is_channel_allowed("matrix", sender, &admin_user_ids)
                };
                if !allowed {
                    let result = state
                        .app_state
                        .pairing_manager
                        .write()
                        .await
                        .create_pairing_request("matrix", sender, Some(sender.to_string()));
                    match result {
                        Ok(code) => {
                            let _ = send_matrix_message(
                                homeserver_url,
                                access_token,
                                room_id,
                                &format!(
                                    "Pairing request created. Share this code with an admin: {}",
                                    code
                                ),
                                &state.next_txn_id(),
                            )
                            .await;
                        }
                        Err(e) => {
                            warn!("[MATRIX] Pairing request failed for {}: {}", sender, e);
                        }
                    }
                    return;
                }
            }
        }
    }

    if matches!(
        text_lc.as_str(),
        "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
    ) {
        let key = (room_id.to_string(), sender.to_string());
        let (approval_tx, owner_mismatch) = {
            let mut map = state.app_state.matrix_pending_approvals.lock().await;
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
            let reply = if approval_tx.send(approved).is_ok() {
                if approved {
                    "Approved. Continuing..."
                } else {
                    "Denied."
                }
            } else {
                warn!(
                    "[MATRIX] Approval response could not be delivered for room {} sender {}; request likely expired",
                    room_id, sender
                );
                "This approval request is no longer active."
            };
            if let Err(e) = send_matrix_message(
                homeserver_url,
                access_token,
                room_id,
                reply,
                &state.next_txn_id(),
            )
            .await
            {
                warn!(
                    "[MATRIX] Failed to send approval reply in {}: {}",
                    room_id, e
                );
            }
            return;
        }
        if owner_mismatch {
            if let Err(e) = send_matrix_message(
                homeserver_url,
                access_token,
                room_id,
                "This approval request belongs to another user in this room.",
                &state.next_txn_id(),
            )
            .await
            {
                warn!(
                    "[MATRIX] Failed to send approval owner-mismatch reply in {}: {}",
                    room_id, e
                );
            }
            return;
        }

        if let Err(e) = send_matrix_message(
            homeserver_url,
            access_token,
            room_id,
            "No active approval request.",
            &state.next_txn_id(),
        )
        .await
        {
            warn!(
                "[MATRIX] Failed to send no-active-approval reply in {}: {}",
                room_id, e
            );
        }
        return;
    }

    // Handle bot commands
    if text.starts_with('/') || text.starts_with('!') {
        handle_command(text, room_id, state, homeserver_url, access_token).await;
        return;
    }

    info!(
        "[MATRIX] Message from {} in {}: {}",
        sender,
        room_id,
        &text[..text.len().min(80)]
    );

    // Route through the unified pipeline
    let claude_client = state.get_or_create_client(room_id).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::Matrix {
            room_id: room_id.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };

    let observer = MatrixObserver::new(
        homeserver_url.to_string(),
        access_token.to_string(),
        room_id.to_string(),
        sender.to_string(),
        state.app_state.matrix_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::matrix(room_id.to_string(), sender.to_string()),
        observer: &observer,
        streaming: false,
        window: None,
        on_stream_chunk: None,
        auto_compact: true,
        save_to_memory: true,
        sync_supermemory: true,
        check_sensitive_data: true,
    };

    match router::route_message(&message, options, &state.app_state).await {
        Ok(routed) => {
            let response = router::extract_text_from_response(&routed.text);
            if response.is_empty() {
                let _ = send_matrix_message(
                    homeserver_url,
                    access_token,
                    room_id,
                    "(No response)",
                    &state.next_txn_id(),
                )
                .await;
            } else {
                for chunk in router::split_message(&response, 4096) {
                    let _ = send_matrix_message(
                        homeserver_url,
                        access_token,
                        room_id,
                        &chunk,
                        &state.next_txn_id(),
                    )
                    .await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            let _ = send_matrix_message(
                homeserver_url,
                access_token,
                room_id,
                &msg,
                &state.next_txn_id(),
            )
            .await;
        }
        Err(e) => {
            let _ = send_matrix_message(
                homeserver_url,
                access_token,
                room_id,
                &format!("Error: {}", e),
                &state.next_txn_id(),
            )
            .await;
        }
    }
}

/// Handle Matrix bot commands.
async fn handle_command(
    text: &str,
    room_id: &str,
    state: &MatrixBotState,
    homeserver_url: &str,
    access_token: &str,
) {
    let cmd = text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('/')
        .trim_start_matches('!');

    match cmd {
        "help" | "start" => {
            let _ = send_matrix_message(
                homeserver_url,
                access_token,
                room_id,
                "NexiBot Commands:\n\n\
                 - /new - Start a new conversation\n\
                 - /status - Check bot status\n\
                 - /help - Show this help message\n\n\
                 Send a message (with the configured prefix) to chat with AI.",
                &state.next_txn_id(),
            )
            .await;
        }
        "new" => {
            state.clear_session(room_id).await;
            let _ = send_matrix_message(
                homeserver_url,
                access_token,
                room_id,
                "Conversation cleared. Starting fresh!",
                &state.next_txn_id(),
            )
            .await;
        }
        "status" => {
            let (model, has_key) = {
                let config = state.app_state.config.read().await;
                (
                    config.claude.model.clone(),
                    config
                        .claude
                        .api_key
                        .as_ref()
                        .is_some_and(|k| !k.is_empty()),
                )
            };
            let auth_status = if has_key {
                "configured"
            } else {
                "NOT configured"
            };
            let _ = send_matrix_message(
                homeserver_url,
                access_token,
                room_id,
                &format!(
                    "NexiBot is online.\nModel: {}\nAuth: {}",
                    model, auth_status
                ),
                &state.next_txn_id(),
            )
            .await;
        }
        _ => {
            let _ = send_matrix_message(
                homeserver_url,
                access_token,
                room_id,
                "Unknown command. Use /help for available commands.",
                &state.next_txn_id(),
            )
            .await;
        }
    }
}

fn next_matrix_approval_txn_id() -> String {
    let counter = MATRIX_APPROVAL_TXN_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("nexi_approve_{}_{}", std::process::id(), counter)
}

// ---------------------------------------------------------------------------
// Matrix Client-Server API communication
// ---------------------------------------------------------------------------

/// Percent-encode a Matrix room ID for use in URLs.
///
/// Room IDs contain `!` and `:` which must be encoded in URL path segments.
fn encode_room_id(room_id: &str) -> String {
    let mut encoded = String::with_capacity(room_id.len() * 3);
    for byte in room_id.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

/// Send a text message to a Matrix room.
pub async fn send_matrix_message(
    homeserver: &str,
    token: &str,
    room_id: &str,
    text: &str,
    txn_id: &str,
) -> Result<()> {
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());

    // URL-encode the room_id (it contains special chars like ! and :)
    let encoded_room_id = encode_room_id(room_id);
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
        homeserver.trim_end_matches('/'),
        encoded_room_id,
        txn_id
    );

    let body = json!({
        "msgtype": "m.text",
        "body": text,
    });

    let response = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        warn!("[MATRIX] Failed to send message ({}): {}", status, body);
        return Err(anyhow::anyhow!("Matrix send failed: {}", status));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum number of concurrent Matrix chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale Matrix chat sessions (>24h inactive).
async fn session_cleanup_loop(state: Arc<MatrixBotState>) {
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
                "[MATRIX] Cleaned up {} stale sessions ({} remaining)",
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
                "[MATRIX] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Typing indicator, read receipts, and reactions
// ---------------------------------------------------------------------------

/// Build the typing indicator URL for a user in a room.
/// Room ID and user ID are URL-encoded to handle the `:` character.
pub fn typing_url(homeserver_url: &str, room_id: &str, user_id: &str) -> String {
    format!(
        "{}/_matrix/client/v3/rooms/{}/typing/{}",
        homeserver_url,
        urlencoding::encode(room_id),
        urlencoding::encode(user_id),
    )
}

/// Send or clear a typing indicator in a Matrix room.
///
/// `typing = true` starts the indicator (auto-expires after `timeout_ms`).
/// `typing = false` stops it immediately.
pub async fn send_typing_indicator(
    homeserver_url: &str,
    access_token: &str,
    room_id: &str,
    user_id: &str,
    typing: bool,
    timeout_ms: u32,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let url = typing_url(homeserver_url, room_id, user_id);
    let body = if typing {
        serde_json::json!({ "typing": true, "timeout": timeout_ms })
    } else {
        serde_json::json!({ "typing": false })
    };

    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Typing indicator error {}: {}", status, text);
    }

    Ok(())
}

/// Build the read receipt URL for an event.
pub fn read_receipt_url(homeserver_url: &str, room_id: &str, event_id: &str) -> String {
    format!(
        "{}/_matrix/client/v3/rooms/{}/receipt/m.read/{}",
        homeserver_url,
        urlencoding::encode(room_id),
        urlencoding::encode(event_id),
    )
}

/// Send an m.read receipt for an event in a room.
/// This marks the event as read by the bot user.
pub async fn send_read_receipt(
    homeserver_url: &str,
    access_token: &str,
    room_id: &str,
    event_id: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let url = read_receipt_url(homeserver_url, room_id, event_id);

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&serde_json::json!({})) // empty body required by spec
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Read receipt error {}: {}", status, text);
    }

    Ok(())
}

/// Build the content body for an m.reaction event.
pub fn reaction_event_body(relates_to_event_id: &str, emoji: &str) -> serde_json::Value {
    serde_json::json!({
        "m.relates_to": {
            "rel_type": "m.annotation",
            "event_id": relates_to_event_id,
            "key": emoji
        }
    })
}

/// Send an m.reaction event in a Matrix room.
///
/// `relates_to_event_id` is the ID of the event being reacted to.
/// `emoji` is any valid Matrix reaction key (typically a single emoji character).
pub async fn send_reaction(
    homeserver_url: &str,
    access_token: &str,
    room_id: &str,
    relates_to_event_id: &str,
    emoji: &str,
    txn_id: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/send/m.reaction/{}",
        homeserver_url,
        urlencoding::encode(room_id),
        urlencoding::encode(txn_id),
    );

    let body = reaction_event_body(relates_to_event_id, emoji);

    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Reaction send error {}: {}", status, text);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_url_format() {
        let url = typing_url("https://matrix.org", "!roomid:matrix.org", "@user:matrix.org");
        assert_eq!(
            url,
            "https://matrix.org/_matrix/client/v3/rooms/%21roomid%3Amatrix.org/typing/%40user%3Amatrix.org"
        );
    }

    #[test]
    fn read_receipt_url_format() {
        let url = read_receipt_url("https://matrix.org", "!room:matrix.org", "$eventid:matrix.org");
        assert!(url.contains("receipt"), "url: {}", url);
        assert!(url.contains("m.read"), "url: {}", url);
    }

    #[test]
    fn reaction_event_body_format() {
        let body = reaction_event_body("$event123:matrix.org", "✅");
        assert_eq!(body["m.relates_to"]["rel_type"], "m.annotation");
        assert_eq!(body["m.relates_to"]["event_id"], "$event123:matrix.org");
        assert_eq!(body["m.relates_to"]["key"], "✅");
    }
}
