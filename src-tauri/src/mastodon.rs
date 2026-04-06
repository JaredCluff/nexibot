//! Mastodon integration for NexiBot.
//!
//! Connects to a Mastodon instance via the Streaming API (WebSocket) to
//! receive mention notifications in real time and replies via the REST API
//! using direct-message visibility.

use futures_util::StreamExt;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::Message};
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

fn default_true() -> bool {
    true
}

/// Configuration for the Mastodon channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MastodonConfig {
    /// Whether the Mastodon channel is enabled.
    pub enabled: bool,

    /// Mastodon instance base URL (e.g. "https://mastodon.social").
    #[serde(default)]
    pub instance_url: String,

    /// OAuth2 access token with `read:notifications` + `write:statuses` scopes.
    #[serde(default)]
    pub access_token: String,

    /// Whether to respond to @mention notifications. Default: true.
    #[serde(default = "default_true")]
    pub respond_to_mentions: bool,

    /// Allow-list of account IDs. Empty = apply dm_policy.
    #[serde(default)]
    pub allowed_account_ids: Vec<String>,

    /// Admin account IDs that bypass DM policy.
    #[serde(default)]
    pub admin_account_ids: Vec<String>,

    /// DM access policy.
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,

    /// Per-channel tool access policy.
    #[serde(default)]
    pub tool_policy: crate::config::ChannelToolPolicy,
}

impl Default for MastodonConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            instance_url: String::new(),
            access_token: String::new(),
            respond_to_mentions: true,
            allowed_account_ids: Vec::new(),
            admin_account_ids: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-account conversation session for Mastodon.
pub(crate) struct MastodonChatSession {
    /// Dedicated Claude client with its own conversation history.
    claude_client: ClaudeClient,
    /// Last activity timestamp for session expiry.
    last_activity: Instant,
}

/// Shared state for the Mastodon bot.
pub struct MastodonState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, MastodonChatSession>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for Mastodon tool execution flow, including in-channel approvals.
pub(crate) struct MastodonObserver {
    app_state: AppState,
    requester_account_id: String,
    requester_acct: String,
    has_send_config: bool,
    pending_approvals: Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
}

impl MastodonObserver {
    pub(crate) fn new(
        app_state: AppState,
        requester_account_id: String,
        requester_acct: String,
        has_send_config: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            requester_account_id,
            requester_acct,
            has_send_config,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for MastodonObserver {
    fn supports_approval(&self) -> bool {
        self.has_send_config
            && !self.requester_account_id.trim().is_empty()
            && !self.requester_acct.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = self.requester_account_id.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                send_mastodon_reply(
                    &self.app_state,
                    &self.requester_acct,
                    None,
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
        if !send_mastodon_reply_checked(&self.app_state, &self.requester_acct, None, &prompt).await
        {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                send_mastodon_reply(
                    &self.app_state,
                    &self.requester_acct,
                    None,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

impl MastodonState {
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

    /// Get or create a Claude client for the given Mastodon account ID.
    async fn get_or_create_client(&self, account_id: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(account_id) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            account_id.to_string(),
            MastodonChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

// ---------------------------------------------------------------------------
// Mastodon streaming payload structures
// ---------------------------------------------------------------------------

/// A raw streaming event from the Mastodon /api/v1/streaming endpoint.
#[derive(Debug, Deserialize)]
struct StreamingEvent {
    event: String,
    /// Payload is a JSON string (double-encoded) in Mastodon's streaming protocol.
    payload: Option<serde_json::Value>,
}

/// A Mastodon notification object (parsed from the payload JSON string).
#[derive(Debug, Deserialize)]
struct MastodonNotification {
    #[serde(rename = "type")]
    notification_type: String,
    account: Option<MastodonAccount>,
    status: Option<MastodonStatus>,
}

#[derive(Debug, Deserialize)]
struct MastodonAccount {
    id: String,
    acct: String, // "username" or "username@instance.tld"
}

#[derive(Debug, Deserialize)]
struct MastodonStatus {
    id: String,
    content: String, // HTML content
}

// ---------------------------------------------------------------------------
// HTML tag stripping
// ---------------------------------------------------------------------------

/// Strip HTML tags from Mastodon status content (content is always HTML).
///
/// This is intentionally simple — Mastodon content uses basic inline tags
/// (<p>, <br>, <span>, <a>, etc.). We don't need a full HTML parser.
fn strip_html(html: &str) -> String {
    // Replace block-level paragraph breaks with newlines before stripping tags.
    let with_newlines = html
        .replace("</p>", "\n")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");

    // Remove all remaining HTML tags.
    let mut result = String::with_capacity(with_newlines.len());
    let mut in_tag = false;
    for ch in with_newlines.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    // Decode common HTML entities.
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .trim()
        .to_string()
}

/// Strip @mention prefixes from the beginning of status text.
fn strip_mentions(text: &str, bot_acct: &str) -> String {
    let mut result = text.to_string();
    // Remove "@botname " patterns at the start.
    let prefix = format!("@{}", bot_acct);
    loop {
        let trimmed = result.trim_start();
        if trimmed.starts_with('@') {
            // Find end of this @mention (space or end of string)
            let mention_end = trimmed
                .find(|c: char| c.is_whitespace())
                .unwrap_or(trimmed.len());
            result = trimmed[mention_end..].trim_start().to_string();
        } else {
            break;
        }
    }
    // Also remove the bot's own @acct if still present.
    result.replace(&prefix, "").trim().to_string()
}

// ---------------------------------------------------------------------------
// Main bot entry point
// ---------------------------------------------------------------------------

/// Start the Mastodon bot, connecting to the streaming API and processing
/// mention notifications indefinitely with reconnect-on-error.
pub async fn start_mastodon_bot(app_state: AppState) -> Result<(), String> {
    let (enabled, instance_url, access_token) = {
        let config = app_state.config.read().await;
        (
            config.mastodon.enabled,
            config.mastodon.instance_url.clone(),
            app_state
                .key_interceptor
                .restore_config_string(&config.mastodon.access_token),
        )
    };

    if !enabled {
        info!("[MASTODON] Mastodon integration disabled in config");
        return Ok(());
    }

    if instance_url.is_empty() || access_token.is_empty() {
        return Err("[MASTODON] instance_url or access_token not configured".to_string());
    }

    {
        use crate::security::ssrf::{self, SsrfPolicy};
        if let Err(e) = ssrf::validate_outbound_request(&instance_url, &SsrfPolicy::default(), &[]) {
            return Err(format!("[MASTODON] instance_url blocked by SSRF policy: {}", e));
        }
    }

    let state = Arc::new(MastodonState::new(app_state));

    // Spawn session cleanup task.
    let cleanup_state = state.clone();
    tokio::spawn(session_cleanup_loop(cleanup_state));

    // Look up the bot's own account handle so we can strip self-mentions.
    let bot_acct = fetch_own_acct(&instance_url, &access_token)
        .await
        .unwrap_or_default();
    info!("[MASTODON] Bot account: @{} on {}", bot_acct, instance_url);

    // Main reconnect loop.
    let mut backoff_secs: u64 = 5;
    loop {
        match run_streaming_loop(&state, &instance_url, &access_token, &bot_acct).await {
            Ok(()) => {
                info!("[MASTODON] Streaming loop exited cleanly, reconnecting...");
            }
            Err(e) => {
                warn!(
                    "[MASTODON] Streaming error: {}. Reconnecting in {}s...",
                    e, backoff_secs
                );
            }
        }

        // Check if still enabled before reconnecting.
        {
            let config = state.app_state.config.read().await;
            if !config.mastodon.enabled {
                info!("[MASTODON] Disabled at runtime, stopping");
                return Ok(());
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(120);
    }
}

/// Fetch the bot's own @acct handle from /api/v1/accounts/verify_credentials.
async fn fetch_own_acct(instance_url: &str, access_token: &str) -> Option<String> {
    let url = format!("{}/api/v1/accounts/verify_credentials", instance_url);
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .bearer_auth(access_token)
        .send()
        .await
        .ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;
    json.get("acct")?.as_str().map(|s| s.to_string())
}

/// Connect to the Mastodon streaming WebSocket and process events.
async fn run_streaming_loop(
    state: &Arc<MastodonState>,
    instance_url: &str,
    access_token: &str,
    bot_acct: &str,
) -> Result<(), String> {
    let ws_url = build_mastodon_stream_ws_url(instance_url)?;
    let ws_base = ws_url
        .split('?')
        .next()
        .unwrap_or(ws_url.as_str())
        .to_string();
    let mut request = ws_url
        .into_client_request()
        .map_err(|e| format!("Invalid Mastodon streaming request: {}", e))?;
    let auth = format!("Bearer {}", access_token);
    let auth_header = auth
        .parse()
        .map_err(|e| format!("Invalid Mastodon access token header: {}", e))?;
    request.headers_mut().insert("Authorization", auth_header);

    info!("[MASTODON] Connecting to streaming WebSocket: {}", ws_base);

    let (ws_stream, _) = connect_async(request)
        .await
        .map_err(|e| format!("WebSocket connect error: {}", e))?;

    info!("[MASTODON] WebSocket connected");

    let (mut _write, mut read) = ws_stream.split();

    while let Some(msg_result) = read.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                if let Err(e) = handle_streaming_message(state, &text, bot_acct).await {
                    warn!("[MASTODON] Error handling streaming message: {}", e);
                }
            }
            Ok(Message::Ping(_)) => {
                // tungstenite handles pong automatically.
            }
            Ok(Message::Close(_)) => {
                info!("[MASTODON] Server closed WebSocket connection");
                return Ok(());
            }
            Ok(_) => {}
            Err(e) => {
                return Err(format!("WebSocket error: {}", e));
            }
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

fn build_mastodon_stream_ws_url(instance_url: &str) -> Result<String, String> {
    let mut url =
        Url::parse(instance_url).map_err(|e| format!("invalid Mastodon instance_url: {}", e))?;
    match url.scheme() {
        "http" => {
            if url.set_scheme("ws").is_err() {
                return Err(format!("failed to convert URL scheme to ws for '{}'", instance_url));
            }
        }
        "https" => {
            if url.set_scheme("wss").is_err() {
                return Err(format!("failed to convert URL scheme to wss for '{}'", instance_url));
            }
        }
        "ws" | "wss" => {}
        scheme => {
            return Err(format!(
                "unsupported Mastodon instance_url scheme '{}'",
                scheme
            ))
        }
    }
    let path = append_path_segment(url.path(), "api/v1/streaming");
    url.set_path(&path);
    url.set_query(None);
    url.query_pairs_mut().append_pair("stream", "user");
    Ok(url.to_string())
}

/// Parse and dispatch a single streaming text frame.
async fn handle_streaming_message(
    state: &Arc<MastodonState>,
    text: &str,
    bot_acct: &str,
) -> Result<(), String> {
    // Mastodon streaming frames are newline-delimited key:value lines.
    // Format:
    //   event: notification\n
    //   data: {"type":"mention",...}\n
    //   \n
    // Or as a single JSON object on some instances.

    // Try JSON first (newer instances / alternative format).
    if let Ok(event) = serde_json::from_str::<StreamingEvent>(text) {
        return dispatch_streaming_event(state, &event, bot_acct).await;
    }

    // Fall back to line-based parsing.
    let mut event_type = String::new();
    let mut data_str = String::new();

    for line in text.lines() {
        if let Some(val) = line.strip_prefix("event: ") {
            event_type = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("data: ") {
            data_str = val.trim().to_string();
        }
    }

    if event_type.is_empty() || data_str.is_empty() {
        return Ok(());
    }

    let payload: serde_json::Value =
        serde_json::from_str(&data_str).map_err(|e| format!("payload parse error: {}", e))?;

    let event = StreamingEvent {
        event: event_type,
        payload: Some(payload),
    };

    dispatch_streaming_event(state, &event, bot_acct).await
}

/// Dispatch a parsed streaming event.
async fn dispatch_streaming_event(
    state: &Arc<MastodonState>,
    event: &StreamingEvent,
    bot_acct: &str,
) -> Result<(), String> {
    if event.event != "notification" {
        return Ok(());
    }

    let payload_value = match &event.payload {
        Some(v) => v,
        None => return Ok(()),
    };

    // Payload may be a JSON string (double-encoded) or a JSON object.
    let notification: MastodonNotification = if let Some(s) = payload_value.as_str() {
        serde_json::from_str(s).map_err(|e| format!("notification parse error: {}", e))?
    } else {
        serde_json::from_value(payload_value.clone())
            .map_err(|e| format!("notification parse error: {}", e))?
    };

    if notification.notification_type != "mention" {
        return Ok(());
    }

    let account = match &notification.account {
        Some(a) => a,
        None => return Ok(()),
    };
    let status = match &notification.status {
        Some(s) => s,
        None => return Ok(()),
    };

    let account_id = account.id.clone();
    let acct = account.acct.clone();
    let status_id = status.id.clone();
    let raw_content = strip_html(&status.content);
    let text = strip_mentions(&raw_content, bot_acct).trim().to_string();

    if text.is_empty() {
        return Ok(());
    }

    // ---- Authorization ----
    {
        let config = state.app_state.config.read().await;
        let enabled = config.mastodon.enabled;
        let respond_to_mentions = config.mastodon.respond_to_mentions;
        let dm_policy = config.mastodon.dm_policy;
        let allowed = config.mastodon.allowed_account_ids.clone();
        let admins = config.mastodon.admin_account_ids.clone();
        drop(config);

        if !enabled || !respond_to_mentions {
            return Ok(());
        }

        // --- Message deduplication ---
        {
            let mut dedup = state.msg_dedup.lock().await;
            if dedup.put(status_id.clone(), ()).is_some() {
                debug!("[MASTODON] Dropping duplicate status: {}", status_id);
                return Ok(());
            }
        }

        // --- Per-account rate limiting ---
        let rate_key = format!("mastodon:{}", account_id);
        if state.rate_limiter.check(&rate_key).is_err() {
            warn!("[MASTODON] Rate limit exceeded for {} — dropping message", acct);
            return Ok(());
        }

        let is_admin = !admins.is_empty() && admins.contains(&account_id);

        if !is_admin {
            match dm_policy {
                crate::pairing::DmPolicy::Allowlist => {
                    if !allowed.is_empty() && !allowed.contains(&account_id) {
                        info!(
                            "[MASTODON] Ignoring mention from unauthorized account: {}",
                            acct
                        );
                        return Ok(());
                    }
                }
                crate::pairing::DmPolicy::Open => {}
                crate::pairing::DmPolicy::Pairing => {
                    let pairing_mgr = state.app_state.pairing_manager.read().await;
                    if !pairing_mgr.is_channel_allowed("mastodon", &account_id, &allowed) {
                        drop(pairing_mgr);
                        let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                        match pairing_mgr.create_pairing_request(
                            "mastodon",
                            &account_id,
                            Some(acct.clone()),
                        ) {
                            Ok(code) => {
                                drop(pairing_mgr);
                                send_mastodon_reply(
                                    &state.app_state,
                                    &acct,
                                    Some(&status_id),
                                    &format!(
                                        "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                        code
                                    ),
                                )
                                .await;
                            }
                            Err(e) => {
                                drop(pairing_mgr);
                                send_mastodon_reply(
                                    &state.app_state,
                                    &acct,
                                    Some(&status_id),
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

    {
        let mut handles = state.app_state.mastodon_account_handles.write().await;
        handles.insert(account_id.clone(), acct.clone());
    }

    let text_lc = text.trim().to_lowercase();
    if matches!(
        text_lc.as_str(),
        "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
    ) {
        let approval_tx = state
            .app_state
            .mastodon_pending_approvals
            .lock()
            .await
            .remove(&account_id);
        if let Some(approval_tx) = approval_tx {
            let approved = matches!(text_lc.as_str(), "approve" | "/approve" | "!approve");
            let _ = approval_tx.send(approved);
            let reply = if approved {
                "Approved. Continuing..."
            } else {
                "Denied."
            };
            send_mastodon_reply(&state.app_state, &acct, Some(&status_id), reply).await;
            return Ok(());
        }
    }

    let state_clone = state.clone();
    let acct_clone = acct.clone();
    let status_id_clone = status_id.clone();
    tokio::spawn(async move {
        handle_mastodon_mention(
            &state_clone,
            &account_id,
            &acct_clone,
            &status_id_clone,
            &text,
        )
        .await;
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Message handling
// ---------------------------------------------------------------------------

/// Route a Mastodon mention through Claude and send a direct reply.
async fn handle_mastodon_mention(
    state: &MastodonState,
    account_id: &str,
    acct: &str,
    status_id: &str,
    text: &str,
) {
    let app_state = &state.app_state;
    info!("[MASTODON] Mention from @{}: {}", acct, text);

    let claude_client = state.get_or_create_client(account_id).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::Mastodon {
            account_id: account_id.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_send_config = {
        let config = app_state.config.read().await;
        !config.mastodon.instance_url.trim().is_empty()
            && !config.mastodon.access_token.trim().is_empty()
    };

    let observer = MastodonObserver::new(
        app_state.clone(),
        account_id.to_string(),
        acct.to_string(),
        has_send_config,
        app_state.mastodon_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::mastodon(account_id.to_string()),
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
                send_mastodon_reply(app_state, acct, Some(status_id), "(No response)").await;
            } else {
                // Mastodon status limit is 500 chars by default (configurable per instance).
                for chunk in router::split_message(&response, 480) {
                    send_mastodon_reply(app_state, acct, Some(status_id), &chunk).await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            send_mastodon_reply(app_state, acct, Some(status_id), &msg).await;
        }
        Err(e) => {
            send_mastodon_reply(app_state, acct, Some(status_id), &format!("Error: {}", e)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Mastodon REST API
// ---------------------------------------------------------------------------

/// Post a direct-message reply to a Mastodon account.
async fn send_mastodon_reply(
    app_state: &AppState,
    acct: &str,
    in_reply_to_id: Option<&str>,
    text: &str,
) {
    if !send_mastodon_reply_checked(app_state, acct, in_reply_to_id, text).await {
        warn!("[MASTODON] Failed to send reply to {}", acct);
    }
}

async fn send_mastodon_reply_checked(
    app_state: &AppState,
    acct: &str,
    in_reply_to_id: Option<&str>,
    text: &str,
) -> bool {
    let (instance_url, access_token) = {
        let config = app_state.config.read().await;
        (
            config.mastodon.instance_url.clone(),
            app_state
                .key_interceptor
                .restore_config_string(&config.mastodon.access_token),
        )
    };

    if instance_url.is_empty() || access_token.is_empty() {
        error!("[MASTODON] Cannot send reply: instance_url or access_token not configured");
        return false;
    }

    let status_text = format!("@{} {}", acct, text);

    let mut body = serde_json::json!({
        "status": status_text,
        "visibility": "direct",
    });

    if let Some(reply_id) = in_reply_to_id {
        body["in_reply_to_id"] = serde_json::Value::String(reply_id.to_string());
    }

    let url = format!("{}/api/v1/statuses", instance_url);
    let client = reqwest::Client::new();

    match client
        .post(&url)
        .bearer_auth(&access_token)
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            info!("[MASTODON] Reply posted to @{}", acct);
            true
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            error!("[MASTODON] Post failed ({}): {}", status, body);
            false
        }
        Err(e) => {
            error!("[MASTODON] HTTP error posting reply: {}", e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum concurrent Mastodon chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically evict stale Mastodon chat sessions (>24 h inactive).
pub async fn session_cleanup_loop(state: Arc<MastodonState>) {
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
                "[MASTODON] Cleaned up {} stale sessions ({} remaining)",
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
                "[MASTODON] Evicted {} oldest sessions to enforce cap (now {})",
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
    fn test_build_mastodon_stream_ws_url_sets_scheme_and_stream_query() {
        let url = build_mastodon_stream_ws_url("https://mastodon.social")
            .expect("Mastodon ws url should build");
        assert_eq!(url, "wss://mastodon.social/api/v1/streaming?stream=user");
    }

    #[test]
    fn test_build_mastodon_stream_ws_url_preserves_base_path() {
        let url = build_mastodon_stream_ws_url("http://example.com/masto")
            .expect("Mastodon ws url should build");
        assert_eq!(url, "ws://example.com/masto/api/v1/streaming?stream=user");
    }
}
