//! Discord Bot integration for NexiBot.
//!
//! Runs a Discord bot via the serenity gateway that routes messages
//! through the same Claude pipeline as the GUI chat.

use lru::LruCache;
use serenity::all as serenity_model;
use serenity::async_trait;
use serenity::client::{Client, Context, EventHandler};
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::TypeMapKey;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Per-channel session state for Discord conversations.
struct DiscordChatSession {
    /// Dedicated Claude client with its own conversation history
    claude_client: ClaudeClient,
    /// Last activity timestamp
    last_activity: Instant,
}

/// Shared state for the Discord bot.
pub struct DiscordBotState {
    /// Reference to the global app state
    app_state: AppState,
    /// Per-channel sessions (channel_id -> session)
    chat_sessions: RwLock<HashMap<u64, DiscordChatSession>>,
    /// Per-user rate limiter (10 messages per 60 seconds, 60-second lockout)
    rate_limiter: Arc<RateLimiter>,
    /// Recently-processed message IDs for deduplication
    msg_dedup: Mutex<LruCache<u64, ()>>,
}

impl DiscordBotState {
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

    /// Get or create a Claude client for the given Discord channel.
    async fn get_or_create_client(&self, channel_id: u64) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(&channel_id) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            channel_id,
            DiscordChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }

    /// Clear conversation history for a channel.
    async fn clear_session(&self, channel_id: u64) {
        let mut sessions = self.chat_sessions.write().await;
        sessions.remove(&channel_id);
    }
}

/// Observer for Discord tool execution flow, including in-channel approvals.
pub(crate) struct DiscordObserver {
    http: Arc<serenity::http::Http>,
    channel_id: u64,
    requester_user_id: u64,
    pending_approvals:
        Arc<tokio::sync::Mutex<HashMap<(u64, u64), tokio::sync::oneshot::Sender<bool>>>>,
}

impl DiscordObserver {
    pub(crate) fn new(
        http: Arc<serenity::http::Http>,
        channel_id: u64,
        requester_user_id: u64,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<(u64, u64), tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            http,
            channel_id,
            requester_user_id,
            pending_approvals,
        }
    }
}

#[async_trait]
impl crate::tool_loop::ToolLoopObserver for DiscordObserver {
    fn supports_approval(&self) -> bool {
        self.channel_id != 0 && self.requester_user_id != 0
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = (self.channel_id, self.requester_user_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let channel = serenity::all::ChannelId::new(self.channel_id);
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                let _ = channel
                    .say(
                        &self.http,
                        "⚠️ Another approval is already pending for this user. Denying this request.",
                    )
                    .await;
                return false;
            }
            map.insert(key, tx);
        }

        let prompt = format!(
            "🔐 Tool approval required\n\nTool: {}\nReason: {}\n\nReply `!approve` to allow or `!deny` to block (5 min timeout).",
            tool_name, reason
        );
        if channel.say(&self.http, &prompt).await.is_err() {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                let _ = channel
                    .say(&self.http, "⏰ Approval timed out. Tool blocked.")
                    .await;
                false
            }
        }
    }
}

/// TypeMap key for accessing DiscordBotState from serenity Context.
struct BotStateKey;

impl TypeMapKey for BotStateKey {
    type Value = Arc<DiscordBotState>;
}

static DISCORD_BOT_RUNNING: AtomicBool = AtomicBool::new(false);

/// Serenity event handler.
struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore messages from bots (including ourselves)
        if msg.author.bot {
            return;
        }

        let state = {
            let data = ctx.data.read().await;
            match data.get::<BotStateKey>() {
                Some(s) => s.clone(),
                None => return,
            }
        };

        let config = state.app_state.config.read().await;
        let discord_config = config.discord.clone();
        drop(config);

        if !discord_config.enabled {
            return;
        }

        // Check guild/channel allowlists
        let guild_id = msg.guild_id.map(|g| g.get());
        let channel_id = msg.channel_id.get();
        let user_id = msg.author.id.get();

        // Admin bypass
        let is_admin = !discord_config.admin_user_ids.is_empty()
            && discord_config.admin_user_ids.contains(&user_id);

        if !is_admin {
            // Guild allowlist check
            if !discord_config.allowed_guild_ids.is_empty() {
                if let Some(gid) = guild_id {
                    if !discord_config.allowed_guild_ids.contains(&gid) {
                        return; // Silently ignore messages from non-allowed guilds
                    }
                }
            }

            // Channel allowlist check
            if !discord_config.allowed_channel_ids.is_empty()
                && !discord_config.allowed_channel_ids.contains(&channel_id)
            {
                return; // Silently ignore messages from non-allowed channels
            }

            // DM policy check (for DMs only, not guild messages)
            if guild_id.is_none() {
                match discord_config.dm_policy {
                    DmPolicy::Allowlist => {
                        if !discord_config.admin_user_ids.is_empty()
                            && !discord_config.admin_user_ids.contains(&user_id)
                        {
                            let _ = msg
                                .channel_id
                                .say(&ctx.http, "You are not authorized to use this bot.")
                                .await;
                            return;
                        }
                    }
                    DmPolicy::Open => {}
                    DmPolicy::Pairing => {
                        let pairing_mgr = state.app_state.pairing_manager.read().await;
                        let admin_ids: Vec<String> = discord_config
                            .admin_user_ids
                            .iter()
                            .map(|id| id.to_string())
                            .collect();
                        if !pairing_mgr.is_channel_allowed(
                            "discord",
                            &user_id.to_string(),
                            &admin_ids,
                        ) {
                            drop(pairing_mgr);
                            let display_name = Some(msg.author.name.clone());
                            let mut pairing_mgr = state.app_state.pairing_manager.write().await;
                            match pairing_mgr.create_pairing_request(
                                "discord",
                                &user_id.to_string(),
                                display_name,
                            ) {
                                Ok(code) => {
                                    let _ = msg
                                        .channel_id
                                        .say(
                                            &ctx.http,
                                            format!(
                                                "You are not yet authorized. Your pairing code is:\n\n`{}`\n\nAsk the admin to approve this code in NexiBot Settings.",
                                                code
                                            ),
                                        )
                                        .await;
                                }
                                Err(e) => {
                                    let _ = msg
                                        .channel_id
                                        .say(&ctx.http, format!("Authorization pending. {}", e))
                                        .await;
                                }
                            }
                            return;
                        }
                    }
                }
            }
        }

        let mut text = msg.content.clone();

        // Append attachment info so the LLM is aware of any files/images sent
        for attachment in &msg.attachments {
            text.push_str(&format!(
                "
[Attachment: {} - {}]",
                attachment.filename,
                attachment.url
            ));
        }

        if text.is_empty() {
            return;
        }

        // Fix 2: Message deduplication — skip already-processed messages
        {
            let mut dedup = state.msg_dedup.lock().await;
            if dedup.put(msg.id.get(), ()).is_some() {
                // Already processed this message ID
                return;
            }
        }

        // Fix 1: Per-user rate limiting
        let rate_key = format!("discord:{}", user_id);
        if let Err(e) = state.rate_limiter.check(&rate_key) {
            warn!("[DISCORD] Rate limit hit for user {}: {}", user_id, e);
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    "Please slow down — too many messages. Try again in a moment.",
                )
                .await;
            return;
        }

        // Handle approval responses before normal command routing.
        {
            let text_lc = text.trim().to_lowercase();
            if matches!(text_lc.as_str(), "!approve" | "!deny" | "approve" | "deny") {
                let key = (channel_id, user_id);
                let (sender, owner_mismatch) = {
                    let mut map = state.app_state.discord_pending_approvals.lock().await;
                    if let Some(sender) = map.remove(&key) {
                        (Some(sender), false)
                    } else {
                        let mismatch = map
                            .keys()
                            .any(|(pending_channel_id, _)| *pending_channel_id == channel_id);
                        (None, mismatch)
                    }
                };

                if let Some(sender) = sender {
                    let approved = matches!(text_lc.as_str(), "!approve" | "approve");
                    let reply = if sender.send(approved).is_ok() {
                        if approved {
                            "✅ Approved. Continuing…"
                        } else {
                            "❌ Denied."
                        }
                    } else {
                        warn!(
                            "[DISCORD] Approval response could not be delivered for channel {} user {}; request likely expired",
                            channel_id, user_id
                        );
                        "⚠️ This approval request is no longer active."
                    };
                    let _ = msg.channel_id.say(&ctx.http, reply).await;
                    return;
                }

                if owner_mismatch {
                    let _ = msg
                        .channel_id
                        .say(
                            &ctx.http,
                            "❌ This approval request belongs to another user in this channel.",
                        )
                        .await;
                    return;
                }

                let _ = msg
                    .channel_id
                    .say(&ctx.http, "⚠️ No active approval request.")
                    .await;
                return;
            }
        }

        // Handle bot commands
        if text.starts_with('!') {
            handle_command(&ctx, &msg, &text, &state).await;
            return;
        }

        // Check if the bot is mentioned (in guild messages, require mention)
        if guild_id.is_some() {
            let bot_user_id = ctx.cache.current_user().id;
            let mentioned = msg.mentions.iter().any(|u| u.id == bot_user_id);
            if !mentioned {
                return; // In guilds, only respond when mentioned
            }
        }

        // Send typing indicator
        let _ = msg.channel_id.broadcast_typing(&ctx.http).await;

        let claude_client = state.get_or_create_client(channel_id).await;

        let message = IncomingMessage {
            text: text.clone(),
            channel: ChannelSource::Discord {
                channel_id,
                guild_id,
            },
            agent_id: None,
            metadata: HashMap::new(),
        };

        let observer = DiscordObserver::new(
            ctx.http.clone(),
            channel_id,
            user_id,
            state.app_state.discord_pending_approvals.clone(),
        );
        let options = RouteOptions {
            claude_client: &claude_client,
            overrides: SessionOverrides::default(),
            loop_config: ToolLoopConfig::discord(channel_id, guild_id, user_id),
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
                    let _ = msg.channel_id.say(&ctx.http, "(No response)").await;
                } else {
                    // Discord has a 2000 char limit per message
                    for chunk in router::split_message(&response, 2000) {
                        let _ = msg.channel_id.say(&ctx.http, &chunk).await;
                    }
                }
            }
            Err(RouterError::Blocked(blocked_msg)) => {
                let _ = msg.channel_id.say(&ctx.http, blocked_msg).await;
            }
            Err(e) => {
                let _ = msg.channel_id.say(&ctx.http, format!("Error: {}", e)).await;
            }
        }
    }

    async fn ready(&self, _: Context, ready: Ready) {
        info!(
            "[DISCORD] Connected as {} (id: {})",
            ready.user.name, ready.user.id
        );
    }
}

/// Handle Discord bot commands.
async fn handle_command(ctx: &Context, msg: &Message, text: &str, state: &DiscordBotState) {
    let cmd = text.split_whitespace().next().unwrap_or("");
    match cmd {
        "!help" => {
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    "**NexiBot Commands:**\n\
                     `!new` - Start a new conversation\n\
                     `!status` - Check bot status\n\
                     `!help` - Show this help message\n\n\
                     In servers, mention the bot to chat. In DMs, just send a message.",
                )
                .await;
        }
        "!new" => {
            state.clear_session(msg.channel_id.get()).await;
            let _ = msg
                .channel_id
                .say(&ctx.http, "Conversation cleared. Starting fresh!")
                .await;
        }
        "!status" => {
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
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    format!(
                        "NexiBot is online.\nModel: {}\nAuth: {}",
                        model, auth_status
                    ),
                )
                .await;
        }
        _ => {
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    "Unknown command. Use `!help` for available commands.",
                )
                .await;
        }
    }
}

/// Start the Discord bot service.
pub async fn start_discord_bot(app_state: AppState) -> Result<(), String> {
    let config = app_state.config.read().await;
    if !config.discord.enabled {
        info!("[DISCORD] Discord bot disabled in config");
        return Ok(());
    }

    if config.discord.bot_token.is_empty() {
        warn!("[DISCORD] Discord bot enabled but no bot_token configured");
        return Err("Discord bot token not configured".to_string());
    }

    let bot_token = app_state
        .key_interceptor
        .restore_config_string(&config.discord.bot_token);
    drop(config);

    if DISCORD_BOT_RUNNING.swap(true, Ordering::SeqCst) {
        info!("[DISCORD] Discord bot already running, skipping duplicate start");
        return Ok(());
    }

    let bot_state = Arc::new(DiscordBotState::new(app_state));

    info!("[DISCORD] Starting Discord bot...");

    // Spawn session cleanup task; abort it when the bot exits to prevent stale
    // Arc references accumulating across bot restarts within the same session.
    let cleanup_state = bot_state.clone();
    let cleanup_handle = tokio::spawn(session_cleanup_loop(cleanup_state));

    let intents = serenity_model::GatewayIntents::GUILD_MESSAGES
        | serenity_model::GatewayIntents::DIRECT_MESSAGES
        | serenity_model::GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(&bot_token, intents)
        .event_handler(Handler)
        .await
        .map_err(|e| {
            DISCORD_BOT_RUNNING.store(false, Ordering::SeqCst);
            format!("Failed to create Discord client: {}", e)
        })?;

    {
        let mut data = client.data.write().await;
        data.insert::<BotStateKey>(bot_state);
    }

    let run_result = client.start().await;
    DISCORD_BOT_RUNNING.store(false, Ordering::SeqCst);
    cleanup_handle.abort();

    if let Err(e) = run_result {
        error!("[DISCORD] Discord bot error: {}", e);
        return Err(format!("Discord bot error: {}", e));
    }

    info!("[DISCORD] Discord bot stopped");
    Ok(())
}

/// Maximum number of concurrent Discord chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale Discord chat sessions (>24h inactive).
async fn session_cleanup_loop(state: Arc<DiscordBotState>) {
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
                "[DISCORD] Cleaned up {} stale sessions ({} remaining)",
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
                "[DISCORD] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}
