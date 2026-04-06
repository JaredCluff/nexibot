//! Telegram Bot integration for NexiBot.
//!
//! Runs a long-polling Telegram bot that routes messages through
//! the same Claude pipeline as the GUI chat.

use lru::LruCache;
use std::collections::HashMap;
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup};
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

use crate::channel::ChannelSource;
use crate::commands::memory::{list_sessions_for_resume, SessionSummary};
use crate::commands::AppState;
use crate::pairing::DmPolicy;
use crate::router::{self, IncomingMessage, RouteOptions, RouterError};
use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::{self, ToolLoopConfig};

/// Per-chat UI state for Telegram conversations.
/// The actual conversation history lives in the global AppState.claude_client.
struct TelegramChatSession {
    /// Last activity timestamp (used for cleanup/eviction)
    last_activity: Instant,
    /// Whether the chat is awaiting an API key
    awaiting_api_key: bool,
    /// Pending PKCE code_verifier for OAuth code exchange
    pending_oauth: Option<PendingOAuth>,
    /// Session list awaiting a numeric pick from the user (for /resume multi-step flow)
    awaiting_session_pick: Option<Vec<SessionSummary>>,
}

impl TelegramChatSession {
    fn new() -> Self {
        Self {
            last_activity: Instant::now(),
            awaiting_api_key: false,
            pending_oauth: None,
            awaiting_session_pick: None,
        }
    }
}

/// Stored PKCE state while waiting for user to paste OAuth code.
struct PendingOAuth {
    code_verifier: String,
    state: String,
    created_at: Instant,
}

/// Shared state for the Telegram bot.
pub struct TelegramBotState {
    /// Reference to the global app state
    app_state: AppState,
    /// Per-chat sessions (chat_id -> session)
    chat_sessions: RwLock<HashMap<i64, TelegramChatSession>>,
    /// Per-chat rate limiter (10 messages per 60 seconds, 60-second lockout)
    rate_limiter: Arc<RateLimiter>,
    /// Recently-processed message IDs for deduplication.
    /// Key is "chat_id:msg_id" to prevent cross-chat ID collisions.
    msg_dedup: Mutex<LruCache<String, ()>>,
    /// Per-chat LLM serialization locks.
    ///
    /// Each chat gets its own mutex so that concurrent messages from the same
    /// chat are queued rather than processed simultaneously. This prevents two
    /// messages from interleaving in the same conversation history while still
    /// allowing the approval callback path (which never acquires this lock) to
    /// resolve pending approvals while a handler waits for user input.
    chat_llm_locks: tokio::sync::Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
}

impl TelegramBotState {
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
            chat_llm_locks: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Obtain the per-chat LLM serialization lock for `chat_id`.
    async fn chat_lock(&self, chat_id: i64) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.chat_llm_locks.lock().await;
        map.entry(chat_id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Ensure a per-chat UI session entry exists for the given chat ID.
    /// Creates a new entry with default state if none exists.
    async fn ensure_session_exists(&self, chat_id: i64) {
        let mut sessions = self.chat_sessions.write().await;
        let entry = sessions
            .entry(chat_id)
            .or_insert_with(TelegramChatSession::new);
        entry.last_activity = Instant::now();
    }

    /// Check if chat is awaiting API key input.
    async fn is_awaiting_api_key(&self, chat_id: i64) -> bool {
        let sessions = self.chat_sessions.read().await;
        sessions.get(&chat_id).is_some_and(|s| s.awaiting_api_key)
    }

    /// Set awaiting_api_key flag for a chat.
    async fn set_awaiting_api_key(&self, chat_id: i64, awaiting: bool) {
        let mut sessions = self.chat_sessions.write().await;
        let session = sessions
            .entry(chat_id)
            .or_insert_with(TelegramChatSession::new);
        session.awaiting_api_key = awaiting;
    }

    /// Store pending OAuth PKCE state for a chat.
    async fn set_pending_oauth(&self, chat_id: i64, code_verifier: String, state: String) {
        let mut sessions = self.chat_sessions.write().await;
        let session = sessions
            .entry(chat_id)
            .or_insert_with(TelegramChatSession::new);
        session.pending_oauth = Some(PendingOAuth {
            code_verifier,
            state,
            created_at: Instant::now(),
        });
    }

    /// Take pending OAuth state (consumes it).
    async fn take_pending_oauth(&self, chat_id: i64) -> Option<PendingOAuth> {
        let mut sessions = self.chat_sessions.write().await;
        sessions
            .get_mut(&chat_id)
            .and_then(|s| s.pending_oauth.take())
    }

    /// Check if chat has pending OAuth.
    async fn has_pending_oauth(&self, chat_id: i64) -> bool {
        let sessions = self.chat_sessions.read().await;
        sessions
            .get(&chat_id)
            .is_some_and(|s| s.pending_oauth.is_some())
    }

    /// Clear conversation history for a chat.
    async fn clear_session(&self, chat_id: i64) {
        let mut sessions = self.chat_sessions.write().await;
        sessions.remove(&chat_id);
    }
}

/// Start the Telegram bot service.
///
/// Returns an `Arc<AtomicBool>` stop handle.  Set it to `true` to request
/// a graceful shutdown of the internal polling loop.  The function spawns a
/// background task that restarts the dispatcher with exponential backoff if
/// it exits unexpectedly.
///
/// Returns `Ok(None)` when the bot is disabled or has no token configured.
pub async fn start_telegram_bot(app_state: AppState) -> Result<Option<Arc<AtomicBool>>, String> {
    let config = app_state.config.read().await;
    if !config.telegram.enabled {
        info!("[TELEGRAM] Telegram bot disabled in config");
        return Ok(None);
    }

    if config.telegram.bot_token.is_empty() {
        warn!("[TELEGRAM] Telegram bot enabled but no bot_token configured");
        return Err("Telegram bot token not configured".to_string());
    }

    let bot_token = app_state
        .key_interceptor
        .restore_config_string(&config.telegram.bot_token);
    drop(config);

    let bot = Bot::new(bot_token);

    // Validate the bot token by calling getMe. This catches invalid/revoked
    // tokens immediately instead of silently failing in the dispatch loop.
    match bot.get_me().await {
        Ok(me) => {
            info!(
                "[TELEGRAM] Bot token validated — @{} (id: {})",
                me.username(),
                me.id
            );
        }
        Err(e) => {
            let msg = format!("Telegram bot token is invalid or revoked: {}", e);
            error!("[TELEGRAM] {}", msg);
            return Err(msg);
        }
    }

    let bot_state = Arc::new(TelegramBotState::new(app_state));

    // Spawn session cleanup task
    let cleanup_state = bot_state.clone();
    tokio::spawn(session_cleanup_loop(cleanup_state));

    // Stop flag: set to true by the caller to request graceful shutdown.
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_task = stop_flag.clone();

    // Spawn the long-polling loop with exponential backoff on unexpected exit.
    tokio::spawn(async move {
        let mut backoff = std::time::Duration::from_secs(1);
        loop {
            if stop_flag_task.load(Ordering::Relaxed) {
                info!("[TELEGRAM] Stop flag set — exiting polling loop");
                break;
            }

            // Rebuild handler and dispatcher for each attempt so the state
            // clones are fresh and the old dispatcher is fully dropped.
            let state_for_msg = bot_state.clone();
            let state_for_cb = bot_state.clone();
            let handler = dptree::entry()
                .branch(
                    Update::filter_message().endpoint(move |bot: Bot, msg: Message| {
                        let state = state_for_msg.clone();
                        async move {
                            tokio::spawn(handle_telegram_message(bot, msg, state));
                            respond(())
                        }
                    }),
                )
                .branch(
                    Update::filter_callback_query().endpoint(
                        move |bot: Bot, cb: CallbackQuery| {
                            let state = state_for_cb.clone();
                            async move {
                                tokio::spawn(handle_callback_query(bot, cb, state));
                                respond(())
                            }
                        },
                    ),
                );

            let mut dispatcher = Dispatcher::builder(bot.clone(), handler).build();
            info!("[TELEGRAM] Telegram bot started (long-polling)");
            dispatcher.dispatch().await;

            if stop_flag_task.load(Ordering::Relaxed) {
                info!("[TELEGRAM] Telegram bot dispatch loop ended (stop requested)");
                break;
            }

            error!(
                "[TELEGRAM] Telegram dispatch loop exited unexpectedly. Retrying in {:?}",
                backoff
            );
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(std::time::Duration::from_secs(64));
        }
    });

    Ok(Some(stop_flag))
}

/// Handle an incoming Telegram message.
async fn handle_telegram_message(bot: Bot, msg: Message, state: Arc<TelegramBotState>) {
    let (enabled, allowed_chat_ids, admin_chat_ids, dm_policy, voice_enabled, voice_response) = {
        let config = state.app_state.config.read().await;
        (
            config.telegram.enabled,
            config.telegram.allowed_chat_ids.clone(),
            config.telegram.admin_chat_ids.clone(),
            config.telegram.dm_policy,
            config.telegram.voice_enabled,
            config.telegram.voice_response,
        )
    };

    if !enabled {
        return;
    }

    let chat_id = msg.chat.id;
    let sender_user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok());

    // Fix 1: Per-chat rate limiting
    let rate_key = format!("telegram:{}", chat_id.0);
    if let Err(e) = state.rate_limiter.check(&rate_key) {
        warn!("[TELEGRAM] Rate limit hit for chat {}: {}", chat_id, e);
        if let Err(send_err) = bot
            .send_message(
                chat_id,
                "Please slow down — too many messages. Try again in a moment.",
            )
            .await
        {
            warn!(
                "[TELEGRAM] Failed to send rate-limit reply to {}: {}",
                chat_id, send_err
            );
        }
        return;
    }

    // Admin bypass: admin chat IDs always skip DM policy
    let is_admin = !admin_chat_ids.is_empty() && admin_chat_ids.contains(&chat_id.0);

    if !is_admin {
        // Authorization check based on DM policy
        match dm_policy {
            DmPolicy::Allowlist => {
                // Classic behavior: empty allowlist = allow all
                if !allowed_chat_ids.is_empty() && !allowed_chat_ids.contains(&chat_id.0) {
                    if let Err(e) = bot
                        .send_message(chat_id, "You are not authorized to use this bot.")
                        .await
                    {
                        warn!(
                            "[TELEGRAM] Failed to send unauthorized message to {}: {}",
                            chat_id, e
                        );
                    }
                    return;
                }
            }
            DmPolicy::Open => {}
            DmPolicy::Pairing => {
                // Check combined allowlist (config + runtime pairing)
                let pairing_mgr = state.app_state.pairing_manager.read().await;
                if !pairing_mgr.is_telegram_allowed(chat_id.0, &allowed_chat_ids) {
                    drop(pairing_mgr);
                    // Generate pairing code for unknown sender
                    let display_name = msg.chat.first_name().map(|n| n.to_string());
                    let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                    match pairing_mgr.create_pairing_request(
                        "telegram",
                        &chat_id.0.to_string(),
                        display_name,
                    ) {
                        Ok(code) => {
                            if let Err(e) = bot.send_message(
                                chat_id,
                                format!(
                                    "You are not yet authorized. Your pairing code is:\n\n`{}`\n\nAsk the admin to approve this code in NexiBot Settings.",
                                    code
                                ),
                            ).await {
                                warn!("[TELEGRAM] Failed to send pairing code to {}: {}", chat_id, e);
                            }
                            // Emit event to frontend
                            // Note: No AppHandle available here, but the frontend polls for pending requests
                        }
                        Err(e) => {
                            if let Err(send_err) = bot
                                .send_message(chat_id, format!("Authorization pending. {}", e))
                                .await
                            {
                                warn!(
                                    "[TELEGRAM] Failed to send authorization pending to {}: {}",
                                    chat_id, send_err
                                );
                            }
                        }
                    }
                    return;
                }
            }
        }
    }

    // Deduplication must happen after authorization to prevent unauthorized senders
    // from poisoning the dedup cache for authorized users (cross-user collision).
    // Key is chat_id:msg_id so the same message ID in different chats is not conflated.
    {
        let dedup_key = format!("{}:{}", chat_id.0, msg.id.0);
        let mut dedup = state.msg_dedup.lock().await;
        if dedup.put(dedup_key, ()).is_some() {
            // Already processed this message
            return;
        }
    }

    let mut was_voice_message = false;
    let mut text =
        if let Some(t) = msg.text() {
            t.to_string()
        } else if let Some(voice) = msg.voice() {
            // Check if voice messages are enabled (read once at top of handler)
            if !voice_enabled {
                if let Err(e) = bot
                .send_message(
                    chat_id,
                    "Voice messages are not enabled. Enable in Settings > Integrations > Telegram.",
                )
                .await
            {
                warn!("[TELEGRAM] Failed to send voice-disabled message to {}: {}", chat_id, e);
            }
                return;
            }
            was_voice_message = true;
            match handle_voice_message(&bot, &msg, voice, &state).await {
                Ok(transcript) => transcript,
                Err(e) => {
                    if let Err(send_err) = bot
                        .send_message(chat_id, format!("Failed to process voice: {}", e))
                        .await
                    {
                        warn!(
                            "[TELEGRAM] Failed to send voice error to {}: {}",
                            chat_id, send_err
                        );
                    }
                    return;
                }
            }
        } else if msg.photo().is_some() || msg.document().is_some() {
            // Media message — extract caption text (if any) and annotate with attachment info
            msg.caption().unwrap_or("").to_string()
        } else {
            return; // Ignore other message types (stickers, etc.)
        };

    // Append photo/document attachment info so the LLM is aware of received media
    if let Some(photos) = msg.photo() {
        // photo() returns a slice of PhotoSize in ascending resolution order; take the largest
        if let Some(largest) = photos.last() {
            text.push_str(&format!(
                "
[User sent an image - file_id: {}]",
                largest.file.id
            ));
        }
    }
    if let Some(document) = msg.document() {
        let filename = document
            .file_name
            .as_deref()
            .unwrap_or("(unnamed)");
        text.push_str(&format!(
            "
[User sent a file: {} - file_id: {}]",
            filename,
            document.file.id
        ));
    }

    if text.is_empty() {
        return; // Nothing to process (e.g. bare photo with no caption and annotation failed)
    }

    // Handle /approve and /deny for pending tool approvals (works with or without the slash).
    {
        let text_lc = text.trim().to_lowercase();
        if matches!(text_lc.as_str(), "/approve" | "approve" | "/deny" | "deny") {
            let Some(sender_user_id) = sender_user_id else {
                if let Err(e) = bot
                    .send_message(chat_id, "❌ Unable to verify sender for approval command.")
                    .await
                {
                    warn!(
                        "[TELEGRAM] Failed to send sender-verification error to {}: {}",
                        chat_id, e
                    );
                }
                return;
            };
            let key = (chat_id.0, sender_user_id);
            let (sender, owner_mismatch) = {
                let mut map = state.app_state.telegram_pending_approvals.lock().await;
                if let Some(sender) = map.remove(&key) {
                    (Some(sender), false)
                } else {
                    let mismatch = map
                        .keys()
                        .any(|(pending_chat_id, _)| *pending_chat_id == chat_id.0);
                    (None, mismatch)
                }
            };

            if let Some(sender) = sender {
                let approved = text_lc == "/approve" || text_lc == "approve";
                let reply = if sender.send(approved).is_ok() {
                    if approved {
                        "✅ Approved. Continuing…"
                    } else {
                        "❌ Denied."
                    }
                } else {
                    warn!(
                        "[TELEGRAM] Approval response could not be delivered for chat {} (sender {}); request likely expired",
                        chat_id.0, sender_user_id
                    );
                    "⚠️ This approval request is no longer active."
                };
                if let Err(e) = bot.send_message(chat_id, reply).await {
                    warn!(
                        "[TELEGRAM] Failed to send approval ack to {}: {}",
                        chat_id, e
                    );
                }
                return;
            }

            if owner_mismatch {
                if let Err(e) = bot
                    .send_message(
                        chat_id,
                        "❌ This approval request belongs to another user in this chat.",
                    )
                    .await
                {
                    warn!(
                        "[TELEGRAM] Failed to send approval-owner mismatch to {}: {}",
                        chat_id, e
                    );
                }
                return;
            }
            if let Err(e) = bot
                .send_message(chat_id, "⚠️ No active approval request.")
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send no-active-approval reply to {}: {}",
                    chat_id, e
                );
            }
            return;
        }
    }

    // Handle session pick response (follows a /resume command)
    {
        let pick_list: Option<Vec<SessionSummary>> = {
            let mut map = state.chat_sessions.write().await;
            map.get_mut(&chat_id.0)
                .and_then(|s| s.awaiting_session_pick.take())
        };
        if let Some(list) = pick_list {
            if let Ok(idx) = text.trim().parse::<usize>() {
                if idx >= 1 && idx <= list.len() {
                    let target = &list[idx - 1];
                    // Set the session ID for this chat — do NOT load history into the
                    // shared global ClaudeClient, as that would corrupt other channels'
                    // conversations. The router handles per-request context.
                    state
                        .app_state
                        .memory_manager
                        .write()
                        .await
                        .set_current_session_id(target.id.clone());
                    let msg_text = format!(
                        "✅ Resumed '{}'. Context will be used for this chat.",
                        target.title,
                    );
                    if let Err(e) = bot.send_message(chat_id, msg_text).await {
                        warn!("[TELEGRAM] Could not send resume ack to {}: {}", chat_id, e);
                    }
                    return;
                }
            }
            // Not a valid number — cancel
            if let Err(e) = bot
                .send_message(chat_id, "Cancelled. Continuing current session.")
                .await
            {
                warn!("[TELEGRAM] Could not send cancel ack to {}: {}", chat_id, e);
            }
            return;
        }
    }

    // Handle API key input
    if text.starts_with("sk-ant-") {
        handle_api_key_input(&bot, chat_id, &text, &msg, &state).await;
        return;
    }

    // Handle OAuth authorization code (pasted after auth prompt)
    if state.has_pending_oauth(chat_id.0).await {
        handle_oauth_code_input(&bot, chat_id, &text, &msg, &state).await;
        return;
    }

    // Handle bot commands
    if text.starts_with('/') {
        handle_command(&bot, chat_id, &text, &state, msg.date.timestamp()).await;
        return;
    }

    // Check if awaiting API key
    if state.is_awaiting_api_key(chat_id.0).await {
        if let Err(e) = bot
            .send_message(
                chat_id,
                "Please send your API key starting with `sk-ant-`, or use /cancel to cancel.",
            )
            .await
        {
            warn!(
                "[TELEGRAM] Failed to send awaiting-api-key prompt to {}: {}",
                chat_id, e
            );
        }
        return;
    }

    // Route through the unified pipeline
    let app_state = &state.app_state;

    // Send typing indicator before pipeline
    if let Err(e) = bot
        .send_chat_action(chat_id, teloxide::types::ChatAction::Typing)
        .await
    {
        warn!(
            "[TELEGRAM] Failed to send typing indicator to {}: {}",
            chat_id, e
        );
    }

    // Ensure per-chat UI session exists
    state.ensure_session_exists(chat_id.0).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::Telegram { chat_id: chat_id.0 },
        agent_id: None,
        metadata: HashMap::new(),
    };

    let observer = tool_loop::TelegramObserver::new(
        bot.clone(),
        chat_id,
        sender_user_id,
        state.app_state.telegram_pending_approvals.clone(),
    );

    // Serialize LLM calls per-chat so rapid successive messages don't
    // interleave in the same conversation history.  The approval callback path
    // (button taps → handle_callback_query) never acquires this lock, so
    // pending approvals can be resolved while this handler is mid-flight.
    let chat_lock = state.chat_lock(chat_id.0).await;
    let _chat_guard = chat_lock.lock().await;

    let result = {
        let client_guard = state.app_state.claude_client.read().await;
        let options = RouteOptions {
            claude_client: &*client_guard,
            overrides: SessionOverrides::default(),
            loop_config: ToolLoopConfig::telegram_with_sender(chat_id.0, sender_user_id),
            observer: &observer,
            streaming: false,
            window: None,
            on_stream_chunk: None,
            auto_compact: true,
            save_to_memory: true,
            sync_supermemory: true,
            check_sensitive_data: true,
        };
        router::route_message(&message, options, app_state).await
    };

    match result {
        Ok(routed) => {
            let response = router::extract_text_from_response(&routed.text);
            if response.is_empty() {
                if let Err(e) = bot.send_message(chat_id, "(No response)").await {
                    warn!(
                        "[TELEGRAM] Failed to send '(No response)' to {}: {}",
                        chat_id, e
                    );
                }
            } else {
                // If the incoming message was voice AND voice_response is enabled,
                // synthesize TTS audio and send as a Telegram voice note.
                let voice_response_enabled = was_voice_message && voice_response;

                if voice_response_enabled {
                    // Send text first, then voice
                    for chunk in router::split_message(&response, 4096) {
                        if let Err(e) = bot.send_message(chat_id, &chunk).await {
                            warn!(
                                "[TELEGRAM] Failed to send response chunk to {}: {}",
                                chat_id, e
                            );
                        }
                    }
                    // Send voice response
                    match synthesize_voice_response(&response, &state.app_state).await {
                        Ok(ogg_bytes) => {
                            use teloxide::types::InputFile;
                            let voice_file = InputFile::memory(ogg_bytes).file_name("response.ogg");
                            if let Err(e) = bot.send_voice(chat_id, voice_file).await {
                                warn!(
                                    "[TELEGRAM] Failed to send voice response to {}: {}",
                                    chat_id, e
                                );
                            }
                        }
                        Err(e) => {
                            warn!("[TELEGRAM] Voice response synthesis failed: {}", e);
                        }
                    }
                } else {
                    // Text-only response
                    for chunk in router::split_message(&response, 4096) {
                        if let Err(e) = bot.send_message(chat_id, &chunk).await {
                            warn!(
                                "[TELEGRAM] Failed to send response chunk to {}: {}",
                                chat_id, e
                            );
                        }
                    }
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            if let Err(e) = bot.send_message(chat_id, msg).await {
                warn!(
                    "[TELEGRAM] Failed to send blocked-message notice to {}: {}",
                    chat_id, e
                );
            }
        }
        Err(e) => {
            let err_str = e.to_string();
            if is_auth_error(&err_str) {
                send_auth_prompt(&bot, chat_id, &state).await;
            } else {
                if let Err(send_err) = bot
                    .send_message(chat_id, format!("Error: {}", err_str))
                    .await
                {
                    warn!(
                        "[TELEGRAM] Failed to send error message to {}: {}",
                        chat_id, send_err
                    );
                }
            }
        }
    }
}

/// Download, decode OGG Opus voice message, and transcribe via STT.
async fn handle_voice_message(
    bot: &Bot,
    msg: &Message,
    voice: &teloxide::types::Voice,
    state: &TelegramBotState,
) -> Result<String, String> {
    let chat_id = msg.chat.id;

    // 1. Send typing indicator
    if let Err(e) = bot
        .send_chat_action(chat_id, teloxide::types::ChatAction::Typing)
        .await
    {
        warn!(
            "[TELEGRAM] Failed to send typing indicator to {}: {}",
            chat_id, e
        );
    }

    // 2. Download the voice file from Telegram servers
    let file = bot
        .get_file(&voice.file.id)
        .await
        .map_err(|e| format!("Failed to get file info: {}", e))?;

    let mut ogg_bytes = Vec::new();
    {
        use futures_util::StreamExt;
        let mut stream = bot.download_file_stream(&file.path);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("Failed to download voice file: {}", e))?;
            ogg_bytes.extend_from_slice(&chunk);
        }
    }

    info!(
        "[TELEGRAM] Downloaded voice message: {} bytes, duration: {}s",
        ogg_bytes.len(),
        voice.duration
    );

    // 3. Decode OGG Opus to PCM f32 samples at 16kHz mono
    let samples =
        decode_ogg_opus(&ogg_bytes).map_err(|e| format!("Failed to decode OGG Opus: {}", e))?;

    info!(
        "[TELEGRAM] Decoded {} PCM samples from voice message",
        samples.len()
    );

    if samples.is_empty() {
        return Err(format!(
            "Voice message was empty or too short (duration: {}s, OGG size: {} bytes, 0 PCM samples after decode)",
            voice.duration, ogg_bytes.len()
        ));
    }

    // 4. Transcribe using the configured STT backend
    let voice_service = state.app_state.voice_service.read().await;
    let transcript = voice_service
        .ptt_transcribe(&samples)
        .await
        .map_err(|e| format!("STT failed: {}", e))?;

    if transcript.trim().is_empty() {
        let stt_backend = voice_service.get_stt_backend().await;
        return Err(format!(
            "Could not transcribe voice message (empty result from STT backend '{}')",
            stt_backend
        ));
    }

    info!("[TELEGRAM] Voice transcript: {}", transcript);

    // 5. Send transcript preview back to user
    if let Err(e) = bot
        .send_message(chat_id, format!("Heard: {}", transcript))
        .await
    {
        warn!(
            "[TELEGRAM] Failed to send transcript preview to {}: {}",
            chat_id, e
        );
    }

    Ok(transcript)
}

/// Decode OGG Opus bytes to f32 PCM samples at 16kHz mono.
fn decode_ogg_opus(ogg_data: &[u8]) -> Result<Vec<f32>, String> {
    use audiopus::coder::Decoder as OpusDecoder;
    use audiopus::{packet::Packet as OpusPacket, MutSignals, SampleRate};
    use ogg::reading::PacketReader;

    // Opus in OGG Telegram voice messages is typically 48kHz mono
    let opus_sample_rate = 48000u32;

    let mut decoder = OpusDecoder::new(SampleRate::Hz48000, audiopus::Channels::Mono)
        .map_err(|e| format!("Failed to create Opus decoder: {}", e))?;

    let cursor = Cursor::new(ogg_data);
    let mut packet_reader = PacketReader::new(cursor);
    let mut pcm_i16: Vec<i16> = Vec::new();
    // Max Opus frame: 120ms at 48kHz = 5760 samples per channel
    let mut decode_buf = vec![0i16; 5760];
    let mut packet_index = 0u64;

    loop {
        let packet = match packet_reader.read_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => {
                warn!("[TELEGRAM] OGG read error: {}", e);
                break;
            }
        };

        // Skip the first two OGG packets (OpusHead and OpusTags headers)
        if packet_index < 2 {
            // Parse channel count from OpusHead if it's the first packet
            if packet_index == 0 && packet.data.len() >= 10 && &packet.data[..8] == b"OpusHead" {
                let head_channels = packet.data[9] as u32;
                if head_channels > 1 {
                    warn!(
                        "[TELEGRAM] Multi-channel Opus ({} ch), will downmix to mono",
                        head_channels
                    );
                }
            }
            packet_index += 1;
            continue;
        }
        packet_index += 1;

        let opus_packet = match OpusPacket::try_from(&packet.data[..]) {
            Ok(p) => p,
            Err(e) => {
                warn!("[TELEGRAM] Invalid Opus packet {}: {}", packet_index, e);
                continue;
            }
        };
        let output = match MutSignals::try_from(&mut decode_buf[..]) {
            Ok(s) => s,
            Err(e) => {
                warn!("[TELEGRAM] MutSignals error: {}", e);
                continue;
            }
        };

        match decoder.decode(Some(opus_packet), output, false) {
            Ok(decoded_samples) => {
                pcm_i16.extend_from_slice(&decode_buf[..decoded_samples]);
            }
            Err(e) => {
                warn!(
                    "[TELEGRAM] Opus decode error on packet {}: {}",
                    packet_index, e
                );
            }
        }
    }

    if pcm_i16.is_empty() {
        return Err(format!(
            "No audio data decoded from OGG Opus stream ({} bytes input, {} packets processed)",
            ogg_data.len(), packet_index
        ));
    }

    // Convert i16 to f32
    let pcm_samples: Vec<f32> = pcm_i16.iter().map(|&s| s as f32 / 32768.0).collect();

    // Resample to 16kHz (Telegram Opus is 48kHz)
    let target_sample_rate = 16000u32;
    if opus_sample_rate != target_sample_rate {
        let ratio = target_sample_rate as f64 / opus_sample_rate as f64;
        let new_len = (pcm_samples.len() as f64 * ratio) as usize;
        let mut resampled = Vec::with_capacity(new_len);
        for i in 0..new_len {
            let src_idx = i as f64 / ratio;
            let idx = src_idx as usize;
            let frac = src_idx - idx as f64;
            let sample = if idx + 1 < pcm_samples.len() {
                pcm_samples[idx] * (1.0 - frac as f32) + pcm_samples[idx + 1] * frac as f32
            } else if idx < pcm_samples.len() {
                pcm_samples[idx]
            } else {
                0.0
            };
            resampled.push(sample);
        }
        Ok(resampled)
    } else {
        Ok(pcm_samples)
    }
}

/// Strip markdown, synthesize TTS, and encode as OGG Opus for Telegram voice notes.
async fn synthesize_voice_response(
    response_text: &str,
    app_state: &AppState,
) -> Result<Vec<u8>, String> {
    // 1. Strip markdown / formatting so TTS reads naturally
    let clean = crate::voice::strip_markdown_for_tts(response_text);
    if clean.trim().is_empty() {
        return Err("Empty response after stripping markdown".to_string());
    }

    info!(
        "[TELEGRAM] Synthesizing voice response ({} chars)",
        clean.len()
    );

    // 2. Synthesize via the configured TTS backend → WAV bytes
    let voice_service = app_state.voice_service.read().await;
    let tts_backend = voice_service.get_tts_backend().await;
    let wav_bytes = voice_service
        .synthesize_text(&clean)
        .await
        .map_err(|e| format!("TTS synthesis failed: {}", e))?;
    drop(voice_service);

    if wav_bytes.is_empty() {
        return Err(format!("TTS returned empty audio from backend '{}'", tts_backend));
    }

    // 3. Decode WAV to PCM i16 samples, then encode to OGG Opus
    let (pcm_samples, sample_rate) =
        wav_to_pcm_i16(&wav_bytes).map_err(|e| format!("WAV decode failed: {}", e))?;

    info!(
        "[TELEGRAM] WAV decoded: {} PCM samples at {}Hz, encoding to OGG Opus",
        pcm_samples.len(),
        sample_rate
    );

    // Add pauses between sentences so they don't sound jumbled
    // Pauses disabled — Cartesia handles pacing naturally, extra pauses cause stutter
    let pcm_with_pauses = pcm_samples;

    let ogg_bytes = encode_ogg_opus(&pcm_with_pauses, sample_rate)
        .map_err(|e| format!("OGG Opus encode failed: {}", e))?;

    info!(
        "[TELEGRAM] Voice response: {} bytes OGG Opus",
        ogg_bytes.len()
    );
    Ok(ogg_bytes)
}

/// Parse WAV bytes into i16 samples, returning (samples, sample_rate).
///
/// Reads the sample rate from the WAV header so callers can pass
/// the correct rate to the Opus encoder (e.g. 24kHz for Cartesia,
/// 16kHz for macOS say).
///
/// Some TTS backends (notably Cartesia) produce WAV files where the data
/// chunk length isn't a multiple of the sample size. The `hound` crate
/// rejects these, so we fall back to manual PCM extraction when hound fails.
fn wav_to_pcm_i16(wav_bytes: &[u8]) -> Result<(Vec<i16>, u32), String> {
    // Try hound first (strict WAV parser)
    let cursor = Cursor::new(wav_bytes);
    match hound::WavReader::new(cursor) {
        Ok(reader) => {
            let spec = reader.spec();
            if spec.bits_per_sample != 16 {
                return Err(format!(
                    "Expected 16-bit WAV, got {}-bit",
                    spec.bits_per_sample
                ));
            }
            let sample_rate = spec.sample_rate;
            let samples: Vec<i16> = reader
                .into_samples::<i16>()
                .filter_map(|s| s.ok())
                .collect();
            Ok((samples, sample_rate))
        }
        Err(e) => {
            // Hound rejected the WAV (e.g. data chunk not a multiple of sample size).
            // Fall back to manual header parsing and raw PCM extraction.
            info!(
                "[TELEGRAM] hound rejected WAV ({}), falling back to manual PCM extraction",
                e
            );
            wav_to_pcm_i16_manual(wav_bytes)
        }
    }
}

/// Manually extract PCM i16 samples from a WAV byte buffer.
/// Scans for the "fmt " and "data" chunks, reads the sample rate from fmt,
/// and interprets the data payload as little-endian i16 samples, truncating
/// any trailing partial sample.
fn wav_to_pcm_i16_manual(wav_bytes: &[u8]) -> Result<(Vec<i16>, u32), String> {
    if wav_bytes.len() < 12 {
        return Err(format!("WAV too short: {} bytes (minimum 12)", wav_bytes.len()));
    }

    let mut sample_rate: Option<u32> = None;
    let mut bits_per_sample: Option<u16> = None;
    let mut data_start: Option<usize> = None;
    let mut data_len: Option<usize> = None;

    let mut pos = 12; // skip RIFF header + "WAVE"
    while pos + 8 <= wav_bytes.len() {
        let chunk_id = &wav_bytes[pos..pos + 4];
        let chunk_size = u32::from_le_bytes(
            wav_bytes[pos + 4..pos + 8]
                .try_into()
                .map_err(|_| format!("Malformed WAV: invalid bytes at offset {}", pos))?,
        ) as usize;

        if chunk_id == b"fmt " && pos + 8 + 16 <= wav_bytes.len() {
            sample_rate = Some(u32::from_le_bytes(
                wav_bytes[pos + 12..pos + 16]
                    .try_into()
                    .map_err(|_| format!("Malformed WAV: invalid fmt sample_rate bytes at offset {}", pos + 12))?,
            ));
            bits_per_sample = Some(u16::from_le_bytes(
                wav_bytes[pos + 22..pos + 24]
                    .try_into()
                    .map_err(|_| format!("Malformed WAV: invalid fmt bits_per_sample bytes at offset {}", pos + 22))?,
            ));
        } else if chunk_id == b"data" {
            data_start = Some(pos + 8);
            data_len = Some(chunk_size);
            break; // data is always last meaningful chunk
        }

        // Advance to next chunk (word-aligned)
        pos += 8 + chunk_size;
        if chunk_size % 2 != 0 {
            pos += 1;
        }
    }

    let sr = sample_rate.ok_or("No fmt chunk found in WAV")?;
    let bps = bits_per_sample.ok_or("No bits_per_sample in fmt chunk")?;
    let start = data_start.ok_or("No data chunk found in WAV")?;
    let len = data_len.ok_or("No data chunk length")?;

    if bps != 16 {
        return Err(format!("Expected 16-bit WAV, got {}-bit", bps));
    }

    // Clamp data to what's actually available in the buffer, then truncate
    // to a whole number of samples (2 bytes per i16 sample).
    let available = wav_bytes.len().saturating_sub(start);
    let usable = std::cmp::min(len, available);
    let usable = usable - (usable % 2); // drop trailing odd byte

    let pcm_data = &wav_bytes[start..start + usable];
    let samples: Vec<i16> = pcm_data
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();

    info!(
        "[TELEGRAM] Manual WAV decode: {} samples at {}Hz (data_chunk_len={}, used={})",
        samples.len(),
        sr,
        len,
        usable
    );

    Ok((samples, sr))
}

/// Insert extra silence at natural pauses between sentences in PCM audio.
///
/// Scans for quiet sections (amplitude below threshold for ≥80ms) which
/// correspond to sentence boundaries, and adds ~280ms of silence after each.
#[allow(dead_code)]
fn add_sentence_pauses(pcm: &[i16], sample_rate: u32) -> Vec<i16> {
    let silence_threshold: i32 = 800;
    // 80ms of quiet = likely a sentence boundary (not just a word gap)
    let min_quiet_samples = (sample_rate as usize * 80) / 1000;
    // Add 280ms of extra silence (midpoint of user's 250–330ms request)
    let extra_pause_samples = (sample_rate as usize * 280) / 1000;

    let mut result = Vec::with_capacity(pcm.len().saturating_add(pcm.len() / 5));
    let mut i = 0;

    while i < pcm.len() {
        if (pcm[i] as i32).abs() <= silence_threshold {
            // Start of a quiet section
            let start = i;
            while i < pcm.len() && (pcm[i] as i32).abs() <= silence_threshold {
                i += 1;
            }
            let quiet_len = i - start;

            // Copy the original quiet section
            result.extend_from_slice(&pcm[start..i]);

            // If this is a sentence-level pause, add extra silence
            if quiet_len >= min_quiet_samples {
                result.resize(result.len() + extra_pause_samples, 0i16);
            }
        } else {
            result.push(pcm[i]);
            i += 1;
        }
    }

    result
}

/// Encode PCM i16 samples at the given sample rate into OGG Opus bytes.
///
/// Produces a valid OGG Opus stream with OpusHead/OpusTags headers that
/// Telegram accepts as a voice note.
fn encode_ogg_opus(pcm_i16: &[i16], input_sample_rate: u32) -> Result<Vec<u8>, String> {
    use audiopus::coder::Encoder as OpusEncoder;
    use audiopus::{Application, Bitrate, Channels, SampleRate, Signal};

    if input_sample_rate == 0 {
        return Err("Invalid audio: sample rate is 0".to_string());
    }

    // Opus requires one of: 8000, 12000, 16000, 24000, 48000
    let opus_rate = if input_sample_rate <= 8000 {
        SampleRate::Hz8000
    } else if input_sample_rate <= 16000 {
        SampleRate::Hz16000
    } else if input_sample_rate <= 24000 {
        SampleRate::Hz24000
    } else {
        SampleRate::Hz48000
    };

    // Use Audio mode for higher-fidelity TTS (preserves more frequency content than Voip)
    let mut encoder = OpusEncoder::new(opus_rate, Channels::Mono, Application::Audio)
        .map_err(|e| format!("Failed to create Opus encoder: {}", e))?;

    // Set bitrate — higher rate for high-fidelity TTS backends like Cartesia (24kHz)
    let bitrate = if input_sample_rate >= 24000 {
        96000
    } else {
        48000
    };
    let _ = encoder.set_bitrate(Bitrate::BitsPerSecond(bitrate));
    let _ = encoder.set_signal(Signal::Voice);

    // Opus frame size: 20ms at the configured sample rate
    let frame_size = (input_sample_rate as usize) * 20 / 1000; // 480 samples at 24kHz

    // Query encoder lookahead (algorithmic delay) for pre-skip.
    // The encoder reports lookahead at its configured rate, but OpusHead
    // pre_skip is always in 48kHz units (RFC 7845 §5.1).
    let lookahead = encoder.lookahead().unwrap_or(0) as u64;
    let pre_skip = (lookahead * 48000 / input_sample_rate as u64) as u16;

    // Append silence to flush the encoder's internal buffer so the tail
    // of the real audio isn't lost. One full frame of silence is enough.
    let mut padded_pcm = pcm_i16.to_vec();
    padded_pcm.resize(padded_pcm.len() + frame_size, 0i16);

    // OGG container
    let mut ogg_buf = Vec::new();
    {
        use ogg::writing::PacketWriter;
        let mut pw = PacketWriter::new(&mut ogg_buf);
        let serial = 1u32;

        // Write OpusHead header (RFC 7845 §5.1)
        let mut head = Vec::with_capacity(19);
        head.extend_from_slice(b"OpusHead"); // magic
        head.push(1); // version
        head.push(1); // channel count (mono)
        head.extend_from_slice(&pre_skip.to_le_bytes()); // pre-skip (encoder lookahead)
        head.extend_from_slice(&input_sample_rate.to_le_bytes()); // original sample rate
        head.extend_from_slice(&0i16.to_le_bytes()); // output gain
        head.push(0); // channel mapping family
        pw.write_packet(head, serial, ogg::PacketWriteEndInfo::EndPage, 0)
            .map_err(|e| format!("OGG write OpusHead: {}", e))?;

        // Write OpusTags header (RFC 7845 §5.2)
        let mut tags = Vec::new();
        tags.extend_from_slice(b"OpusTags");
        let vendor = b"NexiBot";
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes()); // no user comments
        pw.write_packet(tags, serial, ogg::PacketWriteEndInfo::EndPage, 0)
            .map_err(|e| format!("OGG write OpusTags: {}", e))?;

        // Encode audio frames (including the flush frame of silence at the end)
        let mut encode_buf = vec![0u8; 4000]; // max Opus packet ~4000 bytes
        let mut granule: u64 = 0;
        // OGG Opus granule positions are ALWAYS in 48kHz units (RFC 7845 §4),
        // regardless of the encoder's input sample rate.
        let granule_scale = 48000u64 / input_sample_rate as u64;
        let granule_per_frame = frame_size as u64 * granule_scale;
        // Final granule = (actual input samples + pre_skip) scaled to 48kHz
        let final_granule = (pcm_i16.len() as u64 + pre_skip as u64) * granule_scale;
        let total_frames = (padded_pcm.len() + frame_size - 1) / frame_size;

        for (frame_idx, chunk) in padded_pcm.chunks(frame_size).enumerate() {
            // Pad last chunk if needed
            let frame: Vec<i16> = if chunk.len() < frame_size {
                let mut padded = chunk.to_vec();
                padded.resize(frame_size, 0);
                padded
            } else {
                chunk.to_vec()
            };

            let encoded_len = encoder
                .encode(&frame, &mut encode_buf)
                .map_err(|e| format!("Opus encode error: {}", e))?;

            granule += granule_per_frame;
            let is_last = frame_idx + 1 == total_frames;
            let end_info = if is_last {
                ogg::PacketWriteEndInfo::EndStream
            } else {
                ogg::PacketWriteEndInfo::NormalPacket
            };
            // On the last packet, set granule to the true final position
            let pkt_granule = if is_last { final_granule } else { granule };

            pw.write_packet(
                encode_buf[..encoded_len].to_vec(),
                serial,
                end_info,
                pkt_granule,
            )
            .map_err(|e| format!("OGG write audio packet: {}", e))?;
        }
    }

    Ok(ogg_buf)
}

/// Handle API key input from user.
async fn handle_api_key_input(
    bot: &Bot,
    chat_id: ChatId,
    key: &str,
    msg: &Message,
    state: &TelegramBotState,
) {
    // Save the API key to config
    let mut config = state.app_state.config.write().await;
    let previous_config = config.clone();
    let should_intercept =
        config.key_vault.intercept_config && state.app_state.key_interceptor.is_enabled();
    let persisted_key = if should_intercept {
        state.app_state.key_interceptor.intercept_config_string(key)
    } else {
        key.to_string()
    };
    config.claude.api_key = Some(persisted_key);

    let save_result = config.save();
    if save_result.is_err() {
        *config = previous_config;
    }
    // Runtime should continue using real key material even when persisted
    // config stores a key-vault proxy token.
    config.resolve_key_vault_proxies();

    let mut saved = false;
    match save_result {
        Ok(()) => {
            saved = true;
            info!("[TELEGRAM] API key saved via Telegram chat {}", chat_id);
            if let Err(e) = bot
                .send_message(
                    chat_id,
                    "API key saved successfully! You can now send messages.",
                )
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send api-key-saved message to {}: {}",
                    chat_id, e
                );
            }
        }
        Err(e) => {
            error!("[TELEGRAM] Failed to save API key: {}", e);
            if let Err(send_err) = bot
                .send_message(chat_id, format!("Failed to save API key: {}", e))
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send api-key-error to {}: {}",
                    chat_id, send_err
                );
            }
        }
    }
    drop(config);

    if saved {
        let _ = state.app_state.config_changed.send(());
    }

    // Delete the message containing the API key for security
    if let Err(e) = bot.delete_message(chat_id, msg.id).await {
        warn!(
            "[TELEGRAM] Failed to delete API key message in {}: {}",
            chat_id, e
        );
    }
    state.set_awaiting_api_key(chat_id.0, false).await;
}

/// Handle an OAuth authorization code pasted by the user.
async fn handle_oauth_code_input(
    bot: &Bot,
    chat_id: ChatId,
    code_text: &str,
    msg: &Message,
    state: &TelegramBotState,
) {
    let pending = match state.take_pending_oauth(chat_id.0).await {
        Some(p) => p,
        None => {
            if let Err(e) = bot
                .send_message(chat_id, "No pending OAuth session. Please try again.")
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send no-oauth-session message to {}: {}",
                    chat_id, e
                );
            }
            return;
        }
    };

    // Check TTL: reject if older than 5 minutes
    if pending.created_at.elapsed() > std::time::Duration::from_secs(300) {
        if let Err(e) = bot
            .send_message(
                chat_id,
                "OAuth session expired (>5 minutes). Please send a message to get a new link.",
            )
            .await
        {
            warn!(
                "[TELEGRAM] Failed to send oauth-expired message to {}: {}",
                chat_id, e
            );
        }
        return;
    }

    // Parse code (format: "code#state" or just "code")
    let code_text = code_text.trim();
    let (auth_code, code_state) = if code_text.contains('#') {
        let parts: Vec<&str> = code_text.split('#').collect();
        (
            parts[0].to_string(),
            parts
                .get(1)
                .map(|s| s.to_string())
                .unwrap_or(pending.state.clone()),
        )
    } else {
        (code_text.to_string(), pending.state.clone())
    };

    if let Err(e) = bot
        .send_message(chat_id, "Exchanging authorization code for tokens...")
        .await
    {
        warn!(
            "[TELEGRAM] Failed to send oauth-progress message to {}: {}",
            chat_id, e
        );
    }

    // Exchange code for tokens
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
    let redirect_uri = "https://console.anthropic.com/oauth/code/callback";

    let response = match client
        .post("https://console.anthropic.com/v1/oauth/token")
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": client_id,
            "code": auth_code,
            "state": code_state,
            "redirect_uri": redirect_uri,
            "code_verifier": pending.code_verifier,
        }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            if let Err(send_err) = bot
                .send_message(chat_id, format!("Token exchange failed: {}", e))
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send token-exchange-failed to {}: {}",
                    chat_id, send_err
                );
            }
            return;
        }
    };

    if !response.status().is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        if let Err(e) = bot
            .send_message(
                chat_id,
                format!(
                    "Token exchange failed: {}\n\nSend a message to get a fresh auth link.",
                    error_text
                ),
            )
            .await
        {
            warn!(
                "[TELEGRAM] Failed to send token-exchange-status to {}: {}",
                chat_id, e
            );
        }
        return;
    }

    let token_data: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            if let Err(send_err) = bot
                .send_message(chat_id, format!("Failed to parse token response: {}", e))
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send token-parse-error to {}: {}",
                    chat_id, send_err
                );
            }
            return;
        }
    };

    let access_token = match token_data["access_token"].as_str() {
        Some(t) => t.to_string(),
        None => {
            if let Err(e) = bot
                .send_message(chat_id, "Missing access_token in response.")
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send missing-token message to {}: {}",
                    chat_id, e
                );
            }
            return;
        }
    };
    let refresh_token = token_data["refresh_token"].as_str().map(|s| s.to_string());
    let expires_in = token_data["expires_in"].as_u64().unwrap_or(28800);

    // Save OAuth profile
    match crate::oauth::AuthProfileManager::load() {
        Ok(mut manager) => {
            let profile = crate::oauth::AuthProfile::new(
                "anthropic",
                "default",
                access_token,
                refresh_token,
                expires_in,
            );
            manager.upsert_profile(profile);
            if let Err(e) = manager.save() {
                if let Err(send_err) = bot
                    .send_message(chat_id, format!("Failed to save profile: {}", e))
                    .await
                {
                    warn!(
                        "[TELEGRAM] Failed to send profile-save-error to {}: {}",
                        chat_id, send_err
                    );
                }
                return;
            }
        }
        Err(e) => {
            if let Err(send_err) = bot
                .send_message(chat_id, format!("Failed to load auth manager: {}", e))
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send auth-manager-error to {}: {}",
                    chat_id, send_err
                );
            }
            return;
        }
    }

    info!(
        "[TELEGRAM] OAuth re-authenticated via Telegram chat {}",
        chat_id
    );

    // Delete the message containing the auth code for security
    if let Err(e) = bot.delete_message(chat_id, msg.id).await {
        warn!(
            "[TELEGRAM] Failed to delete OAuth code message in {}: {}",
            chat_id, e
        );
    }

    if let Err(e) = bot
        .send_message(
            chat_id,
            "Authenticated successfully! You can now send messages.",
        )
        .await
    {
        warn!(
            "[TELEGRAM] Failed to send auth-success message to {}: {}",
            chat_id, e
        );
    }
}

/// Check if an error is an authentication error.
fn is_auth_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("no claude authentication configured")
        || lower.contains("authentication_error")
        || lower.contains("invalid x-api-key")
        || lower.contains("invalid api key")
        || lower.contains("could not resolve authentication")
        || lower.contains("oauth token expired")
        || lower.contains("token refresh failed")
}

/// Send an authentication prompt with PKCE OAuth URL and inline keyboard.
async fn send_auth_prompt(bot: &Bot, chat_id: ChatId, state: &TelegramBotState) {
    use crate::oauth_flow::{generate_code_challenge, generate_code_verifier};

    let code_verifier = generate_code_verifier();
    let code_challenge = generate_code_challenge(&code_verifier);
    let oauth_state = generate_code_verifier();

    let client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
    let redirect_uri = "https://console.anthropic.com/oauth/code/callback";
    let scopes = "org:create_api_key user:profile user:inference";

    let params = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", "true")
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", scopes)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &oauth_state)
        .finish();

    let oauth_url = format!("https://claude.ai/oauth/authorize?{}", params);

    // Ensure session exists so we can store PKCE
    state.ensure_session_exists(chat_id.0).await;
    state
        .set_pending_oauth(chat_id.0, code_verifier, oauth_state)
        .await;

    let parsed_url = match reqwest::Url::parse(&oauth_url) {
        Ok(url) => url,
        Err(e) => {
            warn!("[TELEGRAM] Failed to parse OAuth URL: {}", e);
            return;
        }
    };
    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
        "Sign in with Claude",
        parsed_url,
    )]]);

    if let Err(e) = bot
        .send_message(
            chat_id,
            "Your OAuth token has expired. To re-authenticate:\n\n\
         1. Click the button below to sign in with your Claude account\n\
         2. Copy the authorization code shown on the page\n\
         3. Paste it here in this chat\n\n\
         Or send an API key directly (starts with `sk-ant-`).",
        )
        .reply_markup(keyboard)
        .await
    {
        warn!(
            "[TELEGRAM] Failed to send auth prompt to {}: {}",
            chat_id, e
        );
    }
}

/// Returns true only if the chat is explicitly configured as a Telegram admin chat.
/// Empty admin list is treated as deny-all for privileged actions.
async fn is_telegram_admin_chat(state: &TelegramBotState, chat_id: i64) -> bool {
    let admin_chat_ids = {
        let config = state.app_state.config.read().await;
        config.telegram.admin_chat_ids.clone()
    };
    !admin_chat_ids.is_empty() && admin_chat_ids.contains(&chat_id)
}

/// Handle Telegram bot commands.
async fn handle_command(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    state: &TelegramBotState,
    msg_timestamp: i64,
) {
    let mut parts = text.split_whitespace();
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next(); // optional first argument
    match cmd {
        "/start" => {
            if let Err(e) = bot
                .send_message(
                    chat_id,
                    "Welcome to NexiBot! Send me a message and I'll respond using Claude AI.\n\n\
                 Commands:\n\
                 /new - Start a new conversation\n\
                 /resume - Resume a previous conversation\n\
                 /status - Check bot status\n\
                 /apikey - Set your API key\n\
                 /cancel - Cancel current operation\n\
                 /yolo [secs|off] - Enable/disable elevated access mode",
                )
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send /start response to {}: {}",
                    chat_id, e
                );
            }
        }
        "/new" => {
            // End current memory session and start fresh
            {
                let mut mm = state.app_state.memory_manager.write().await;
                mm.end_session().ok();
                let _ = mm.start_session();
            }
            // NOTE: Do NOT call claude_client.clear_history() here — the ClaudeClient
            // is shared across all channels. Clearing it would corrupt GUI and other
            // channel conversations. Per-chat state is cleared below.
            // Clear per-chat UI state
            state.clear_session(chat_id.0).await;
            if let Err(e) = bot
                .send_message(chat_id, "✅ New session started. Previous context cleared.")
                .await
            {
                warn!("[TELEGRAM] Failed to send /new ack to {}: {}", chat_id, e);
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

            let has_oauth = crate::oauth::AuthProfileManager::load()
                .ok()
                .and_then(|mut m| m.get_default_profile("anthropic").map(|_| true))
                .unwrap_or(false);

            let auth_status = if has_key || has_oauth {
                "configured"
            } else {
                "NOT configured"
            };
            if let Err(e) = bot
                .send_message(
                    chat_id,
                    format!(
                        "NexiBot is online.\nModel: {}\nAuth: {}",
                        model, auth_status
                    ),
                )
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send /status response to {}: {}",
                    chat_id, e
                );
            }
        }
        "/apikey" => {
            state.set_awaiting_api_key(chat_id.0, true).await;
            if let Err(e) = bot
                .send_message(
                    chat_id,
                    "Please send your Anthropic API key. It should start with `sk-ant-`.\n\n\
                 Your key will be stored securely in the NexiBot config.\n\
                 The message will be deleted after saving.",
                )
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send /apikey prompt to {}: {}",
                    chat_id, e
                );
            }
        }
        "/cancel" => {
            state.set_awaiting_api_key(chat_id.0, false).await;
            if let Err(e) = bot.send_message(chat_id, "Operation cancelled.").await {
                warn!(
                    "[TELEGRAM] Failed to send /cancel response to {}: {}",
                    chat_id, e
                );
            }
        }
        "/resume" => {
            let max_age = state
                .app_state
                .config
                .read()
                .await
                .claude
                .max_session_age_days;
            let sessions = {
                let mm = state.app_state.memory_manager.read().await;
                list_sessions_for_resume(&*mm, max_age)
            };
            if sessions.is_empty() {
                if let Err(e) = bot
                    .send_message(chat_id, "No previous sessions found.")
                    .await
                {
                    warn!(
                        "[TELEGRAM] Could not send /resume empty response to {}: {}",
                        chat_id, e
                    );
                }
                return;
            }
            let mut list = String::from("📋 Previous sessions:\n\n");
            for (i, s) in sessions.iter().enumerate() {
                list.push_str(&format!(
                    "{}) {} ({} msgs, {})\n",
                    i + 1,
                    s.title,
                    s.message_count,
                    s.last_activity.format("%Y-%m-%d %H:%M UTC")
                ));
            }
            list.push_str("\nReply with a number to resume, or any other message to cancel.");
            {
                let mut map = state.chat_sessions.write().await;
                let sess = map
                    .entry(chat_id.0)
                    .or_insert_with(TelegramChatSession::new);
                sess.awaiting_session_pick = Some(sessions);
            }
            if let Err(e) = bot.send_message(chat_id, list).await {
                warn!(
                    "[TELEGRAM] Could not send /resume list to {}: {}",
                    chat_id, e
                );
            }
        }
        "/yolo" => {
            if !is_telegram_admin_chat(state, chat_id.0).await {
                if let Err(e) = bot
                    .send_message(
                        chat_id,
                        "Only configured Telegram admin chats can use /yolo.",
                    )
                    .await
                {
                    warn!(
                        "[TELEGRAM] Failed to send unauthorized /yolo response to {}: {}",
                        chat_id, e
                    );
                }
                return;
            }

            let yolo_mgr = &state.app_state.yolo_manager;
            // "/yolo off"  → revoke
            if arg.map(|a| a.eq_ignore_ascii_case("off")).unwrap_or(false) {
                let status = yolo_mgr.revoke().await;
                let msg = if status.active {
                    "⚠️ Yolo mode is still active (revoke failed)."
                } else {
                    "✅ Yolo mode deactivated."
                };
                if let Err(e) = bot.send_message(chat_id, msg).await {
                    warn!(
                        "[TELEGRAM] Failed to send yolo-off response to {}: {}",
                        chat_id, e
                    );
                }
            } else {
                // "/yolo [duration_secs]"  → activate directly (human-authorized)
                //
                // Replay guard: Telegram re-delivers messages when the bot was offline.
                // Reject /yolo activations from messages older than 60 seconds.
                let msg_age_secs = {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    now.saturating_sub(msg_timestamp as u64)
                };
                if msg_age_secs > 60 {
                    if let Err(e) = bot
                        .send_message(
                            chat_id,
                            "⚠️ Yolo activation ignored: message is too old (possible replay). Send /yolo again.",
                        )
                        .await
                    {
                        warn!(
                            "[TELEGRAM] Failed to send yolo-replay-guard to {}: {}",
                            chat_id, e
                        );
                    }
                    return;
                }

                let duration_secs: Option<u64> = arg.and_then(|a| a.parse().ok());
                let status = yolo_mgr.direct_activate(duration_secs).await;
                let msg = match status.remaining_secs {
                    Some(secs) => format!(
                        "⚡ Yolo mode activated. Expires in {} seconds.\n\
                         Use /yolo off to deactivate early.",
                        secs
                    ),
                    None => "⚡ Yolo mode activated (no time limit).\n\
                             Use /yolo off to deactivate."
                        .to_string(),
                };
                if let Err(e) = bot.send_message(chat_id, msg).await {
                    warn!(
                        "[TELEGRAM] Failed to send yolo-on response to {}: {}",
                        chat_id, e
                    );
                }
            }
        }
        _ => {
            if let Err(e) = bot
                .send_message(
                    chat_id,
                    "Unknown command. Use /start, /new, /resume, /status, /apikey, /cancel, or /yolo.",
                )
                .await
            {
                warn!(
                    "[TELEGRAM] Failed to send unknown-command response to {}: {}",
                    chat_id, e
                );
            }
        }
    }
}

/// Maximum number of concurrent Telegram chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale Telegram chat sessions (>24h inactive).
async fn session_cleanup_loop(state: Arc<TelegramBotState>) {
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
                "[TELEGRAM] Cleaned up {} stale sessions ({} remaining)",
                removed,
                sessions.len()
            );
        }

        // Evict oldest sessions if still over the hard cap
        if sessions.len() > MAX_CHANNEL_SESSIONS {
            let mut entries: Vec<_> = sessions
                .iter()
                .map(|(k, s)| (*k, s.last_activity))
                .collect();
            entries.sort_by_key(|&(_, t)| t);
            let evict_count = sessions.len() - MAX_CHANNEL_SESSIONS;
            for (key, _) in entries.into_iter().take(evict_count) {
                sessions.remove(&key);
            }
            info!(
                "[TELEGRAM] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}

/// Handle Telegram inline keyboard button presses (callback queries).
///
/// Recognises `yolo:approve:<request_id>` and `yolo:deny:<request_id>` data
/// from the inline keyboard sent by `send_yolo_approval_request`.
async fn handle_callback_query(bot: Bot, cb: CallbackQuery, state: Arc<TelegramBotState>) {
    // Always answer the callback — Telegram shows a loading spinner until we do.
    if let Err(e) = bot.answer_callback_query(&cb.id).await {
        warn!(
            "[TELEGRAM] Failed to answer callback query {}: {}",
            cb.id, e
        );
    }

    let data = match cb.data.clone() {
        Some(d) => d,
        None => return,
    };

    // Tool approval buttons are keyed by (chat_id, user_id) in the callback data
    // and validated against pending_approvals — no admin check needed.
    if let Some(rest) = data.strip_prefix("tool:approve:") {
        handle_tool_approval_callback(&bot, cb, &state, rest, true).await;
        return;
    }
    if let Some(rest) = data.strip_prefix("tool:deny:") {
        handle_tool_approval_callback(&bot, cb, &state, rest, false).await;
        return;
    }

    // Yolo approval / denial requires admin.
    let callback_chat_id = cb.message.as_ref().map(|m| m.chat().id.0);
    let is_admin_chat = match callback_chat_id {
        Some(chat_id) => is_telegram_admin_chat(&state, chat_id).await,
        None => false,
    };
    if !is_admin_chat {
        warn!(
            "[TELEGRAM] Ignoring yolo callback from non-admin chat: {:?}",
            callback_chat_id
        );
        if let Some(msg) = cb.message {
            let _ = bot
                .edit_message_text(
                    msg.chat().id,
                    msg.id(),
                    "❌ This chat is not allowed to approve yolo mode.",
                )
                .await;
        }
        return;
    }

    // Route yolo approval / denial.
    if let Some(rest) = data.strip_prefix("yolo:approve:") {
        let request_id = rest.to_string();
        let yolo = &state.app_state.yolo_manager;

        let expires_label = match yolo.approve(&request_id).await {
            Ok(status) => match status.remaining_secs {
                Some(s) => format!("Expires in {}s.", s),
                None => "No time limit.".to_string(),
            },
            Err(e) => {
                warn!(
                    "[TELEGRAM] Yolo approval callback rejected (id={}): {}",
                    request_id, e
                );
                if let Some(msg) = cb.message {
                    let _ = bot
                        .edit_message_text(
                            msg.chat().id,
                            msg.id(),
                            "⚠️ Yolo approval failed: request is missing, mismatched, or already handled.",
                        )
                        .await;
                }
                return;
            }
        };

        info!(
            "[TELEGRAM] Yolo mode approved via phone button (id={})",
            request_id
        );
        // Edit the original message in-place so the buttons disappear.
        // cb.message is MaybeInaccessibleMessage in teloxide 0.13 — use methods.
        if let Some(msg) = cb.message {
            if let Err(e) = bot
                .edit_message_text(
                    msg.chat().id,
                    msg.id(),
                    format!("⚡ <b>Yolo mode activated.</b> {}", expires_label),
                )
                .parse_mode(teloxide::types::ParseMode::Html)
                .await
            {
                warn!("[TELEGRAM] Failed to edit yolo-approve message: {}", e);
            }
        }
    } else if let Some(rest) = data.strip_prefix("yolo:deny:") {
        let request_id = rest;
        info!(
            "[TELEGRAM] Yolo mode denied via phone button (id={})",
            request_id
        );
        state.app_state.yolo_manager.revoke().await;

        if let Some(msg) = cb.message {
            if let Err(e) = bot
                .edit_message_text(msg.chat().id, msg.id(), "❌ <b>Yolo mode request denied.</b>")
                .parse_mode(teloxide::types::ParseMode::Html)
                .await
            {
                warn!("[TELEGRAM] Failed to edit yolo-deny message: {}", e);
            }
        }
    }
}

/// Resolve a tool approval callback (✅ Approve or ❌ Deny button).
///
/// `key_str` is `"<chat_id>:<user_id>"` encoded in the callback data.
async fn handle_tool_approval_callback(
    bot: &Bot,
    cb: CallbackQuery,
    state: &Arc<TelegramBotState>,
    key_str: &str,
    approved: bool,
) {
    // Parse chat_id:user_id from the callback data.
    let (chat_id_val, user_id_val) = match key_str.split_once(':') {
        Some((c, u)) => match (c.parse::<i64>(), u.parse::<i64>()) {
            (Ok(c), Ok(u)) => (c, u),
            _ => {
                warn!(
                    "[TELEGRAM] tool approval callback: invalid key '{}'",
                    key_str
                );
                return;
            }
        },
        None => {
            warn!(
                "[TELEGRAM] tool approval callback: missing ':' in key '{}'",
                key_str
            );
            return;
        }
    };

    let key = (chat_id_val, user_id_val);
    let sender = state
        .app_state
        .telegram_pending_approvals
        .lock()
        .await
        .remove(&key);

    let reply_text = match sender {
        Some(tx) => {
            if tx.send(approved).is_ok() {
                if approved {
                    "✅ Approved. Continuing…".to_string()
                } else {
                    "❌ Denied.".to_string()
                }
            } else {
                warn!(
                    "[TELEGRAM] Tool approval oneshot dropped for chat {} user {}",
                    chat_id_val, user_id_val
                );
                "⚠️ This approval request is no longer active.".to_string()
            }
        }
        None => "⚠️ No active approval request (may have timed out).".to_string(),
    };

    // Edit the original message so the buttons disappear and the result is shown.
    if let Some(msg) = cb.message {
        if let Err(e) = bot
            .edit_message_text(msg.chat().id, msg.id(), &reply_text)
            .await
        {
            warn!("[TELEGRAM] Failed to edit tool-approval message: {}", e);
        }
    }
}

/// Send a yolo mode approval request to all allowed Telegram chat IDs.
///
/// Sends a message with Approve / Deny inline buttons. The user taps one on
/// their phone — the resulting callback query is handled by `handle_callback_query`.
pub async fn send_yolo_approval_request(
    state: &crate::commands::AppState,
    request_id: &str,
    duration_secs: Option<u64>,
    reason: Option<&str>,
) {
    fn sanitize_reason(raw: &str) -> String {
        let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut out: String = collapsed.chars().take(220).collect();
        if collapsed.chars().count() > 220 {
            out.push('…');
        }
        out
    }

    let (token, admin_chat_ids) = {
        let config = state.config.read().await;
        (
            config.telegram.bot_token.clone(),
            config.telegram.admin_chat_ids.clone(),
        )
    };

    if token.is_empty() {
        warn!("[TELEGRAM] Skipping yolo approval notification: bot token is empty");
        return;
    }
    if admin_chat_ids.is_empty() {
        warn!(
            "[TELEGRAM] Skipping yolo approval notification: no Telegram admin_chat_ids configured"
        );
        return;
    }

    let duration_label = match duration_secs {
        Some(s) if s < 60 => format!("{}s", s),
        Some(s) if s < 3600 => format!("{}m", s / 60),
        Some(s) => format!("{}h", s / 3600),
        None => "no time limit".to_string(),
    };

    let reason_line = reason
        .filter(|r| !r.is_empty())
        .map(|r| format!("\nReason: {}", sanitize_reason(r)))
        .unwrap_or_default();

    let text = format!(
        "⚡ Yolo Mode Request\n\
         The model is requesting elevated access ({}).{}\n\n\
         Tap Approve to allow, or Deny to reject.",
        duration_label, reason_line
    );

    let keyboard = serde_json::json!({
        "inline_keyboard": [[
            { "text": "✅ Approve", "callback_data": format!("yolo:approve:{}", request_id) },
            { "text": "❌ Deny",    "callback_data": format!("yolo:deny:{}", request_id) }
        ]]
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    for chat_id in &admin_chat_ids {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "reply_markup": keyboard,
        });
        if let Err(e) = client.post(&url).json(&body).send().await {
            warn!(
                "[TELEGRAM] Failed to send yolo approval request to {}: {}",
                chat_id, e
            );
        }
    }
}

/// Send a proactive Telegram notification using the Bot API directly.
/// Used by background tasks to notify the user when work is complete.
#[allow(dead_code)]
pub async fn send_telegram_notification(state: &crate::commands::AppState, message: &str) {
    let (token, chat_ids) = {
        let config = state.config.read().await;
        (
            config.telegram.bot_token.clone(),
            config.telegram.allowed_chat_ids.clone(),
        )
    };

    if token.is_empty() || chat_ids.is_empty() {
        return;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    for chat_id in &chat_ids {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": message,
            "parse_mode": "Markdown",
        });
        if let Err(e) = client.post(&url).json(&body).send().await {
            warn!("[TELEGRAM] Notification failed for {}: {}", chat_id, e);
        }
    }
}
