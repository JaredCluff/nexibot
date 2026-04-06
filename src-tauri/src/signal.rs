//! Signal messaging integration via Signal CLI REST API.
//!
//! Uses the signal-cli-rest-api bridge for receiving and sending messages.
//! The user must run signal-cli-rest-api separately (Docker or native).
//! See: https://github.com/bbernhard/signal-cli-rest-api

use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::commands::AppState;
use crate::pairing::DmPolicy;
use crate::router::{self, IncomingMessage, RouteOptions, RouterError};
use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::ToolLoopConfig;

// ---------------------------------------------------------------------------
// Signal adapter (ChannelAdapter implementation)
// ---------------------------------------------------------------------------

/// Signal adapter that sends messages via the Signal CLI REST API.
#[allow(dead_code)]
pub struct SignalAdapter {
    source: ChannelSource,
    api_url: String,
    sender_number: String,
    bot_number: String,
}

impl SignalAdapter {
    #[allow(dead_code)]
    pub fn new(api_url: String, bot_number: String, sender_number: String) -> Self {
        Self {
            source: ChannelSource::Signal {
                phone_number: sender_number.clone(),
            },
            api_url,
            sender_number,
            bot_number,
        }
    }
}

#[async_trait::async_trait]
impl crate::channel::ChannelAdapter for SignalAdapter {
    async fn send_response(&self, text: &str) -> Result<(), String> {
        let response = router::extract_text_from_response(text);
        if response.is_empty() {
            send_signal_message(
                &self.api_url,
                &self.bot_number,
                &self.sender_number,
                "(No response)",
            )
            .await
            .map_err(|e| e.to_string())?;
            return Ok(());
        }

        // Signal has no hard character limit, but split at 4096 for readability.
        for chunk in router::split_message(&response, 4096) {
            if let Err(e) =
                send_signal_message(&self.api_url, &self.bot_number, &self.sender_number, &chunk)
                    .await
            {
                warn!("[SIGNAL] Failed to send chunk: {}", e);
            }
        }
        Ok(())
    }

    async fn send_error(&self, error: &str) -> Result<(), String> {
        send_signal_message(
            &self.api_url,
            &self.bot_number,
            &self.sender_number,
            &format!("Error: {}", error),
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
// Per-sender session state
// ---------------------------------------------------------------------------

/// Per-sender session state for Signal conversations.
struct SignalChatSession {
    /// Dedicated Claude client with its own conversation history
    claude_client: ClaudeClient,
    /// Last activity timestamp
    last_activity: Instant,
}

/// Shared state for the Signal listener.
pub struct SignalBotState {
    /// Reference to the global app state
    app_state: AppState,
    /// Per-sender sessions (phone_number -> session)
    chat_sessions: RwLock<HashMap<String, SignalChatSession>>,
    /// Per-sender rate limiter (10 messages per 60 seconds, 60-second lockout)
    rate_limiter: Arc<RateLimiter>,
    /// Recently-processed message dedup cache (sender:timestamp key)
    msg_dedup: Mutex<LruCache<String, ()>>,
}

impl SignalBotState {
    fn new(app_state: AppState) -> Self {
        Self {
            app_state,
            chat_sessions: RwLock::new(HashMap::new()),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig {
                max_attempts: 10,
                window_seconds: 60,
                lockout_seconds: 60,
            })),
            msg_dedup: Mutex::new(LruCache::new(NonZeroUsize::new(10_000).unwrap())),
        }
    }

    /// Get or create a Claude client for the given Signal phone number.
    async fn get_or_create_client(&self, phone: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(phone) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            phone.to_string(),
            SignalChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }

    /// Clear conversation history for a sender.
    async fn clear_session(&self, phone: &str) {
        let mut sessions = self.chat_sessions.write().await;
        sessions.remove(phone);
    }
}

/// Observer for Signal tool execution flow, including in-channel approvals.
pub(crate) struct SignalObserver {
    api_url: String,
    bot_number: String,
    sender_number: String,
    pending_approvals: Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
}

impl SignalObserver {
    pub(crate) fn new(
        api_url: String,
        bot_number: String,
        sender_number: String,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            api_url,
            bot_number,
            sender_number,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for SignalObserver {
    fn supports_approval(&self) -> bool {
        !self.api_url.trim().is_empty()
            && !self.bot_number.trim().is_empty()
            && !self.sender_number.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = self.sender_number.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                let _ = send_signal_message(
                    &self.api_url,
                    &self.bot_number,
                    &self.sender_number,
                    "Another approval is already pending for this sender. Denying this request.",
                )
                .await;
                return false;
            }
            map.insert(key.clone(), tx);
        }

        let prompt = format!(
            "Tool approval required\n\nTool: {}\nReason: {}\n\nReply /approve to allow or /deny to block (5 min timeout).",
            tool_name, reason
        );
        if send_signal_message(
            &self.api_url,
            &self.bot_number,
            &self.sender_number,
            &prompt,
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
                let _ = send_signal_message(
                    &self.api_url,
                    &self.bot_number,
                    &self.sender_number,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Signal CLI REST API message structures
// ---------------------------------------------------------------------------

/// A message envelope received from the Signal CLI REST API.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignalEnvelope {
    /// Sender phone number (e.g., "+1234567890")
    source: Option<String>,
    /// Source name / profile name
    source_name: Option<String>,
    /// Data message payload
    data_message: Option<SignalDataMessage>,
}

/// The data message portion of a Signal envelope.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignalDataMessage {
    /// UTC timestamp in milliseconds
    timestamp: Option<u64>,
    /// Text body of the message
    message: Option<String>,
    /// Group info (if this is a group message)
    group_info: Option<SignalGroupInfo>,
}

/// Group info within a Signal data message.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignalGroupInfo {
    #[allow(dead_code)]
    group_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Signal listener (polling loop)
// ---------------------------------------------------------------------------

/// Start the Signal message listener.
///
/// Polls the Signal CLI REST API (`GET /v1/receive/{number}`) every 2 seconds
/// for new messages, routes them through the Claude pipeline, and replies via
/// `POST /v2/send`.
pub async fn start_signal_listener(app_state: AppState) -> Result<()> {
    let config = app_state.config.read().await;
    if !config.signal.enabled {
        info!("[SIGNAL] Signal integration disabled in config");
        return Ok(());
    }

    let api_url = config.signal.api_url.clone();
    let phone_number = config.signal.phone_number.clone();
    drop(config);

    {
        use crate::security::ssrf::{self, SsrfPolicy};
        if let Err(e) = ssrf::validate_outbound_request(&api_url, &SsrfPolicy::default(), &[]) {
            return Err(anyhow::anyhow!("Signal api_url blocked by SSRF policy: {}", e));
        }
    }

    if phone_number.is_empty() {
        warn!("[SIGNAL] Signal enabled but no phone_number configured");
        return Err(anyhow::anyhow!("Signal phone_number not configured"));
    }

    let state = Arc::new(SignalBotState::new(app_state));
    let client = reqwest::Client::new();

    info!(
        "[SIGNAL] Starting Signal listener (polling {} for {})",
        api_url, phone_number
    );

    // Spawn session cleanup task
    let cleanup_state = state.clone();
    tokio::spawn(session_cleanup_loop(cleanup_state));

    // Main polling loop — exponential backoff on consecutive errors (2s..64s).
    let mut consecutive_errors: u32 = 0;
    loop {
        // Allow runtime disabling without restart.
        {
            let config = state.app_state.config.read().await;
            if !config.signal.enabled {
                info!("[SIGNAL] Signal integration disabled at runtime, stopping listener");
                break Ok(());
            }
        }

        match poll_signal_messages(&client, &api_url, &phone_number).await {
            Ok(envelopes) => {
                consecutive_errors = 0;
                for envelope in envelopes {
                    handle_signal_envelope(&envelope, &state, &api_url, &phone_number).await;
                }
            }
            Err(e) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                let backoff_secs = (2u64 << consecutive_errors.min(5)).min(64);
                warn!(
                    "[SIGNAL] Poll error (consecutive: {}): {}. Retrying in {}s",
                    consecutive_errors, e, backoff_secs
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                continue;
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

/// Poll the Signal CLI REST API for new messages.
async fn poll_signal_messages(
    client: &reqwest::Client,
    api_url: &str,
    phone_number: &str,
) -> Result<Vec<SignalEnvelope>> {
    let url = format!("{}/v1/receive/{}", api_url, phone_number);
    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Signal API returned {}: {}", status, body));
    }

    let envelopes: Vec<SignalEnvelope> = response.json().await.unwrap_or_default();
    Ok(envelopes)
}

/// Handle a single Signal message envelope.
async fn handle_signal_envelope(
    envelope: &SignalEnvelope,
    state: &SignalBotState,
    api_url: &str,
    bot_number: &str,
) {
    let (allowed_numbers, admin_numbers, dm_policy, enabled) = {
        let config = state.app_state.config.read().await;
        (
            config.signal.allowed_numbers.clone(),
            config.signal.admin_numbers.clone(),
            config.signal.dm_policy,
            config.signal.enabled,
        )
    };

    if !enabled {
        return;
    }

    let sender = match &envelope.source {
        Some(s) => s.clone(),
        None => return, // No source, skip
    };

    let data_msg = match &envelope.data_message {
        Some(dm) => dm,
        None => return, // Not a data message (e.g., receipt, typing indicator)
    };

    let text = match &data_msg.message {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => return, // No text content
    };

    // Skip group messages (only handle DMs)
    if data_msg.group_info.is_some() {
        return;
    }

    // Fix 2: Message deduplication using sender + timestamp as dedup key
    {
        let ts_str = data_msg
            .timestamp
            .map(|t| t.to_string())
            .unwrap_or_default();
        let dedup_key = format!("{}:{}", sender, ts_str);
        let mut dedup = state.msg_dedup.lock().await;
        if dedup.put(dedup_key, ()).is_some() {
            // Already processed this message
            return;
        }
    }

    // Fix 1: Per-sender rate limiting
    let rate_key = format!("signal:{}", sender);
    if let Err(e) = state.rate_limiter.check(&rate_key) {
        warn!("[SIGNAL] Rate limit hit for sender {}: {}", sender, e);
        if let Err(send_err) = send_signal_message(
            api_url,
            bot_number,
            &sender,
            "Please slow down — too many messages. Try again in a moment.",
        )
        .await
        {
            warn!(
                "[SIGNAL] Failed to send rate-limit reply to {}: {}",
                sender, send_err
            );
        }
        return;
    }

    // Admin bypass
    let is_admin = !admin_numbers.is_empty() && admin_numbers.contains(&sender);

    if !is_admin {
        // Authorization check based on DM policy
        match dm_policy {
            DmPolicy::Allowlist => {
                if !allowed_numbers.is_empty() && !allowed_numbers.contains(&sender) {
                    if let Err(e) = send_signal_message(
                        api_url,
                        bot_number,
                        &sender,
                        "You are not authorized to use this bot.",
                    )
                    .await
                    {
                        warn!(
                            "[SIGNAL] Failed to send auth rejection to {}: {}",
                            sender, e
                        );
                    }
                    return;
                }
            }
            DmPolicy::Open => {}
            DmPolicy::Pairing => {
                let pairing_mgr = state.app_state.pairing_manager.read().await;
                let is_allowed =
                    pairing_mgr.is_channel_allowed("signal", &sender, &allowed_numbers);
                drop(pairing_mgr);

                if !is_allowed {
                    let display_name = envelope.source_name.clone();
                    let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                    match pairing_mgr.create_pairing_request("signal", &sender, display_name) {
                        Ok(code) => {
                            if let Err(e) = send_signal_message(
                                api_url,
                                bot_number,
                                &sender,
                                &format!(
                                    "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                    code
                                ),
                            )
                            .await
                            {
                                warn!("[SIGNAL] Failed to send pairing code to {}: {}", sender, e);
                            }
                        }
                        Err(e) => {
                            if let Err(send_err) = send_signal_message(
                                api_url,
                                bot_number,
                                &sender,
                                &format!("Authorization pending. {}", e),
                            )
                            .await
                            {
                                warn!("[SIGNAL] Failed to send auth-pending to {}: {}", sender, send_err);
                            }
                        }
                    }
                    return;
                }
            }
        }
    }

    // Handle approval responses before normal command routing.
    {
        let text_lc = text.trim().to_lowercase();
        if matches!(
            text_lc.as_str(),
            "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
        ) {
            let sender_tx = state
                .app_state
                .signal_pending_approvals
                .lock()
                .await
                .remove(&sender);
            if let Some(sender_tx) = sender_tx {
                let approved = matches!(text_lc.as_str(), "approve" | "/approve" | "!approve");
                let _ = sender_tx.send(approved);
                let reply = if approved {
                    "Approved. Continuing..."
                } else {
                    "Denied."
                };
                if let Err(e) = send_signal_message(api_url, bot_number, &sender, reply).await {
                    warn!(
                        "[SIGNAL] Failed to send approval reply acknowledgment to {}: {}",
                        sender, e
                    );
                }
                return;
            }
        }
    }

    // Handle bot commands
    if text.starts_with('/') {
        handle_command(api_url, bot_number, &sender, &text, state).await;
        return;
    }

    info!(
        "[SIGNAL] Message from {}: {}",
        sender,
        &text[..text.len().min(80)]
    );

    // Route through the unified pipeline
    let claude_client = state.get_or_create_client(&sender).await;

    let message = IncomingMessage {
        text: text.clone(),
        channel: ChannelSource::Signal {
            phone_number: sender.clone(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };

    let observer = SignalObserver::new(
        api_url.to_string(),
        bot_number.to_string(),
        sender.clone(),
        state.app_state.signal_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::signal(sender.clone()),
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
                if let Err(e) =
                    send_signal_message(api_url, bot_number, &sender, "(No response)").await
                {
                    warn!(
                        "[SIGNAL] Failed to send empty response to {}: {}",
                        sender, e
                    );
                }
            } else {
                for chunk in router::split_message(&response, 4096) {
                    if let Err(e) = send_signal_message(api_url, bot_number, &sender, &chunk).await
                    {
                        warn!(
                            "[SIGNAL] Failed to send response chunk to {}: {}",
                            sender, e
                        );
                    }
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            if let Err(e) = send_signal_message(api_url, bot_number, &sender, &msg).await {
                warn!(
                    "[SIGNAL] Failed to send blocked message to {}: {}",
                    sender, e
                );
            }
        }
        Err(e) => {
            if let Err(send_err) =
                send_signal_message(api_url, bot_number, &sender, &format!("Error: {}", e)).await
            {
                warn!("[SIGNAL] Failed to send error to {}: {}", sender, send_err);
            }
        }
    }
}

/// Handle Signal bot commands.
async fn handle_command(
    api_url: &str,
    bot_number: &str,
    sender: &str,
    text: &str,
    state: &SignalBotState,
) {
    let cmd = text.split_whitespace().next().unwrap_or("");
    match cmd {
        "/start" | "/help" => {
            if let Err(e) = send_signal_message(
                api_url,
                bot_number,
                sender,
                "Welcome to NexiBot! Send me a message and I'll respond using AI.\n\n\
                 Commands:\n\
                 /new - Start a new conversation\n\
                 /status - Check bot status\n\
                 /help - Show this help message",
            )
            .await
            {
                warn!("[SIGNAL] Failed to send help to {}: {}", sender, e);
            }
        }
        "/new" => {
            state.clear_session(sender).await;
            if let Err(e) = send_signal_message(
                api_url,
                bot_number,
                sender,
                "Conversation cleared. Starting fresh!",
            )
            .await
            {
                warn!(
                    "[SIGNAL] Failed to send new-session ack to {}: {}",
                    sender, e
                );
            }
        }
        "/status" => {
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
            if let Err(e) = send_signal_message(
                api_url,
                bot_number,
                sender,
                &format!(
                    "NexiBot is online.\nModel: {}\nAuth: {}",
                    model, auth_status
                ),
            )
            .await
            {
                warn!("[SIGNAL] Failed to send status to {}: {}", sender, e);
            }
        }
        _ => {
            if let Err(e) = send_signal_message(
                api_url,
                bot_number,
                sender,
                "Unknown command. Use /help for available commands.",
            )
            .await
            {
                warn!(
                    "[SIGNAL] Failed to send unknown-cmd reply to {}: {}",
                    sender, e
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sending messages via Signal CLI REST API
// ---------------------------------------------------------------------------

/// Send a text message via the Signal CLI REST API.
pub async fn send_signal_message(api_url: &str, from: &str, to: &str, text: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/v2/send", api_url);

    let body = json!({
        "message": text,
        "number": from,
        "recipients": [to],
    });

    let response = client
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        warn!("[SIGNAL] Failed to send message ({}): {}", status, body);
        return Err(anyhow::anyhow!("Signal send failed: {}", status));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum number of concurrent Signal chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale Signal chat sessions (>24h inactive).
async fn session_cleanup_loop(state: Arc<SignalBotState>) {
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
                "[SIGNAL] Cleaned up {} stale sessions ({} remaining)",
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
                "[SIGNAL] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}
