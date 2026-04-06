//! Microsoft Teams integration via Bot Framework REST API.
//!
//! The Teams bot receives incoming Activity objects via webhook callbacks
//! and replies through the Bot Framework REST API. Token acquisition uses
//! the standard OAuth2 client-credentials flow against the Microsoft identity
//! platform.
//!
//! Webhook endpoint: POST /api/teams/messages
//! This route should be mounted on the existing webhook server (see webhooks.rs).

use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use axum::{
    extract::State as AxumState,
    http::{HeaderMap, StatusCode},
    Json,
};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::commands::AppState;
use crate::router::{self, IncomingMessage, RouteOptions, RouterError};
use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::ToolLoopConfig;

// ---------------------------------------------------------------------------
// Teams adapter (ChannelAdapter implementation)
// ---------------------------------------------------------------------------

/// Teams adapter that sends messages via the Bot Framework REST API.
#[allow(dead_code)]
pub struct TeamsAdapter {
    source: ChannelSource,
    service_url: String,
    conversation_id: String,
    app_id: String,
    token: String,
}

impl TeamsAdapter {
    #[allow(dead_code)]
    pub fn new(
        service_url: String,
        conversation_id: String,
        app_id: String,
        token: String,
    ) -> Self {
        Self {
            source: ChannelSource::Teams {
                conversation_id: conversation_id.clone(),
            },
            service_url,
            conversation_id,
            app_id,
            token,
        }
    }
}

#[async_trait::async_trait]
impl crate::channel::ChannelAdapter for TeamsAdapter {
    async fn send_response(&self, text: &str) -> Result<(), String> {
        let response = router::extract_text_from_response(text);
        if response.is_empty() {
            send_teams_reply(
                &self.service_url,
                &self.conversation_id,
                "(No response)",
                &self.app_id,
                &self.token,
            )
            .await
            .map_err(|e| e.to_string())?;
            return Ok(());
        }

        // Teams has a ~28KB limit per message; split at 4096 chars for readability.
        for chunk in router::split_message(&response, 4096) {
            if let Err(e) = send_teams_reply(
                &self.service_url,
                &self.conversation_id,
                &chunk,
                &self.app_id,
                &self.token,
            )
            .await
            {
                warn!("[TEAMS] Failed to send chunk: {}", e);
            }
        }
        Ok(())
    }

    async fn send_error(&self, error: &str) -> Result<(), String> {
        send_teams_reply(
            &self.service_url,
            &self.conversation_id,
            &format!("Error: {}", error),
            &self.app_id,
            &self.token,
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
// Activity structures (Bot Framework v3 schema)
// ---------------------------------------------------------------------------

/// A Bot Framework Activity (simplified).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsActivity {
    /// Activity type: "message", "conversationUpdate", etc.
    #[serde(rename = "type")]
    pub type_: String,
    /// Unique activity ID.
    #[serde(default)]
    pub id: String,
    /// Text content (for message activities).
    #[serde(default)]
    pub text: String,
    /// Who sent this activity.
    #[serde(default)]
    pub from: TeamsFrom,
    /// Conversation context.
    #[serde(default)]
    pub conversation: TeamsConversation,
    /// The Bot Framework service URL to reply to.
    #[serde(default)]
    pub service_url: String,
    /// Channel ID (e.g., "msteams").
    #[serde(default)]
    pub channel_id: String,
    /// Recipient (the bot account).
    #[serde(default)]
    pub recipient: Option<TeamsFrom>,
}

/// Sender/recipient identity.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TeamsFrom {
    /// AAD object ID or Teams user ID.
    #[serde(default)]
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
}

/// Conversation reference.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TeamsConversation {
    /// Conversation ID.
    #[serde(default)]
    pub id: String,
    /// Conversation type: "personal", "groupChat", "channel".
    #[serde(default)]
    pub conversation_type: Option<String>,
    /// Tenant ID (for multi-tenant bots).
    #[serde(default)]
    pub tenant_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Per-conversation session state
// ---------------------------------------------------------------------------

/// Per-conversation session state for Teams conversations.
struct TeamsChatSession {
    /// Dedicated Claude client with its own conversation history
    claude_client: ClaudeClient,
    /// Last activity timestamp
    last_activity: Instant,
}

/// Shared state for the Teams bot.
pub struct TeamsBotState {
    /// Reference to the global app state
    pub app_state: AppState,
    /// Per-conversation sessions (conversation_id -> session)
    chat_sessions: RwLock<HashMap<String, TeamsChatSession>>,
    /// Cached bot token (refreshed automatically)
    bot_token: RwLock<Option<CachedToken>>,
    /// Cached Bot Framework signing keys for validating incoming JWTs.
    jwks_cache: RwLock<Option<CachedJwks>>,
    /// Per-user rate limiter (10 messages per 60 seconds, 30-second lockout)
    rate_limiter: Arc<RateLimiter>,
    /// Recently-processed activity IDs for deduplication
    msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for Teams tool execution flow, including in-channel approvals.
pub(crate) struct TeamsObserver {
    app_state: AppState,
    conversation_id: String,
    requester_user_id: String,
    service_url: String,
    has_send_config: bool,
    pending_approvals:
        Arc<tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>>,
}

impl TeamsObserver {
    pub(crate) fn new(
        app_state: AppState,
        conversation_id: String,
        requester_user_id: String,
        service_url: String,
        has_send_config: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            conversation_id,
            requester_user_id,
            service_url,
            has_send_config,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for TeamsObserver {
    fn supports_approval(&self) -> bool {
        self.has_send_config
            && !self.conversation_id.trim().is_empty()
            && !self.requester_user_id.trim().is_empty()
            && !self.service_url.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = (self.conversation_id.clone(), self.requester_user_id.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                send_teams_message(
                    &self.app_state,
                    &self.service_url,
                    &self.conversation_id,
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
        if !send_teams_message_checked(
            &self.app_state,
            &self.service_url,
            &self.conversation_id,
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
                send_teams_message(
                    &self.app_state,
                    &self.service_url,
                    &self.conversation_id,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

/// A cached OAuth2 token with expiry.
struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

/// Cached Bot Framework JWKs.
struct CachedJwks {
    fetched_at: Instant,
    keys: HashMap<String, (String, String)>, // kid -> (n, e)
}

#[derive(Debug, Deserialize)]
struct BotFrameworkOpenIdConfig {
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct BotFrameworkJwks {
    keys: Vec<BotFrameworkJwk>,
}

#[derive(Debug, Deserialize)]
struct BotFrameworkJwk {
    kid: String,
    kty: String,
    n: Option<String>,
    e: Option<String>,
}

fn is_allowed_botframework_issuer(issuer: &str) -> bool {
    let normalized = issuer.trim_end_matches('/');
    matches!(
        normalized,
        "https://api.botframework.com" | "https://api.botframework.us"
    )
}

fn is_allowed_teams_service_url(service_url: &str) -> bool {
    let parsed = match url::Url::parse(service_url) {
        Ok(u) => u,
        Err(_) => return false,
    };

    let host = match parsed.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return false,
    };

    match parsed.scheme() {
        "https" => true,
        // Allow local Bot Framework emulator development over HTTP.
        "http" => host == "localhost" || host == "127.0.0.1" || host == "::1",
        _ => false,
    }
}

impl TeamsBotState {
    pub fn new(app_state: AppState) -> Self {
        Self {
            app_state,
            chat_sessions: RwLock::new(HashMap::new()),
            bot_token: RwLock::new(None),
            jwks_cache: RwLock::new(None),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig {
                max_attempts: 10,
                window_seconds: 60,
                lockout_seconds: 30,
            })),
            msg_dedup: Mutex::new(LruCache::new(NonZeroUsize::new(10_000).unwrap())),
        }
    }

    /// Get or create a Claude client for the given conversation ID.
    async fn get_or_create_client(&self, conversation_id: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(conversation_id) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            conversation_id.to_string(),
            TeamsChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }

    /// Clear conversation history.
    async fn clear_session(&self, conversation_id: &str) {
        let mut sessions = self.chat_sessions.write().await;
        sessions.remove(conversation_id);
    }

    /// Get a valid bot token, refreshing if expired.
    pub async fn get_token(&self) -> Result<String> {
        // Check cached token
        {
            let cached = self.bot_token.read().await;
            if let Some(ref tok) = *cached {
                if tok.expires_at > Instant::now() {
                    return Ok(tok.access_token.clone());
                }
            }
        }

        // Refresh token
        let config = self.app_state.config.read().await;
        let app_id = config.teams.app_id.clone();
        let app_password = self
            .app_state
            .key_interceptor
            .restore_config_string(&config.teams.app_password);
        let tenant_id = config.teams.tenant_id.clone();
        drop(config);

        let token = get_bot_token(&app_id, &app_password, tenant_id.as_deref()).await?;

        // Cache for 50 minutes (tokens are typically valid for 60 min)
        let cached = CachedToken {
            access_token: token.clone(),
            expires_at: Instant::now() + std::time::Duration::from_secs(3000),
        };
        *self.bot_token.write().await = Some(cached);

        Ok(token)
    }

    /// Validate incoming Teams activity bearer token against Bot Framework JWKs.
    async fn verify_incoming_bearer(&self, headers: &HeaderMap) -> Result<()> {
        let auth_header = headers
            .get("authorization")
            .or_else(|| headers.get("Authorization"))
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let token = auth_header
            .strip_prefix("Bearer ")
            .or_else(|| auth_header.strip_prefix("bearer "))
            .filter(|t| !t.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing Authorization: Bearer token"))?;

        let header =
            decode_header(token).map_err(|e| anyhow::anyhow!("Invalid JWT header: {}", e))?;
        if header.alg != Algorithm::RS256 {
            return Err(anyhow::anyhow!(
                "Unsupported JWT algorithm for Teams webhook: {:?}",
                header.alg
            ));
        }

        let kid = header
            .kid
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("JWT header missing key id (kid)"))?;

        let (n, e) = self.get_botframework_jwk(kid).await?;
        let key =
            DecodingKey::from_rsa_components(&n, &e).map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let app_id = {
            let config = self.app_state.config.read().await;
            config.teams.app_id.clone()
        };
        if app_id.is_empty() {
            return Err(anyhow::anyhow!("Teams app_id not configured"));
        }

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[app_id.as_str()]);
        validation.validate_exp = true;
        validation.validate_nbf = true;

        let claims = decode::<serde_json::Value>(token, &key, &validation)
            .map_err(|e| anyhow::anyhow!("JWT validation failed: {}", e))?;

        let issuer = claims
            .claims
            .get("iss")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !is_allowed_botframework_issuer(issuer) {
            return Err(anyhow::anyhow!("Unexpected token issuer: {}", issuer));
        }
        Ok(())
    }

    async fn get_botframework_jwk(&self, kid: &str) -> Result<(String, String)> {
        const JWKS_CACHE_TTL_SECS: u64 = 3600;

        {
            let cache = self.jwks_cache.read().await;
            if let Some(cached) = cache.as_ref() {
                if cached.fetched_at.elapsed().as_secs() < JWKS_CACHE_TTL_SECS {
                    if let Some((n, e)) = cached.keys.get(kid) {
                        return Ok((n.clone(), e.clone()));
                    }
                }
            }
        }

        let keys = fetch_botframework_jwks().await?;
        let key = keys
            .get(kid)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No Bot Framework JWK found for kid={}", kid))?;

        let mut cache = self.jwks_cache.write().await;
        *cache = Some(CachedJwks {
            fetched_at: Instant::now(),
            keys,
        });
        Ok(key)
    }
}

async fn fetch_botframework_jwks() -> Result<HashMap<String, (String, String)>> {
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let openid_cfg = client
        .get("https://login.botframework.com/v1/.well-known/openidconfiguration")
        .send()
        .await?
        .error_for_status()?
        .json::<BotFrameworkOpenIdConfig>()
        .await?;

    let jwks = client
        .get(openid_cfg.jwks_uri)
        .send()
        .await?
        .error_for_status()?
        .json::<BotFrameworkJwks>()
        .await?;

    let mut keys = HashMap::new();
    for key in jwks.keys {
        if key.kty == "RSA" {
            if let (Some(n), Some(e)) = (key.n, key.e) {
                keys.insert(key.kid, (n, e));
            }
        }
    }

    if keys.is_empty() {
        return Err(anyhow::anyhow!("Bot Framework JWK set was empty"));
    }

    Ok(keys)
}

// ---------------------------------------------------------------------------
// Webhook handler (to be mounted on the webhook server)
// ---------------------------------------------------------------------------

/// Start the Teams webhook handler.
///
/// This does not start its own HTTP server. Instead, it validates config and
/// returns immediately. The actual webhook route (`POST /api/teams/messages`)
/// is handled by `handle_teams_activity`, which should be called from the
/// webhook server when it receives a request on that path.
pub async fn start_teams_webhook(app_state: AppState) -> Result<()> {
    let _state = create_teams_state(app_state).await?;
    info!("[TEAMS] Teams webhook handler ready at /api/teams/messages");
    Ok(())
}

/// Build Teams shared state for webhook route mounting.
pub async fn create_teams_state(app_state: AppState) -> Result<Arc<TeamsBotState>> {
    let config = app_state.config.read().await;
    if !config.teams.enabled {
        info!("[TEAMS] Teams integration disabled in config");
        return Err(anyhow::anyhow!("Teams integration disabled"));
    }

    if config.teams.app_id.is_empty() || config.teams.app_password.is_empty() {
        warn!("[TEAMS] Teams enabled but app_id or app_password not configured");
        return Err(anyhow::anyhow!(
            "Teams app_id and app_password must be configured"
        ));
    }

    info!(
        "[TEAMS] Teams integration enabled (app_id: {})",
        config.teams.app_id
    );
    drop(config);

    // Spawn session cleanup task
    let state = Arc::new(TeamsBotState::new(app_state));
    let cleanup_state = state.clone();
    tokio::spawn(session_cleanup_loop(cleanup_state));

    Ok(state)
}

/// Handle an incoming Teams Activity from the webhook.
///
/// Called from the webhook server when `POST /api/teams/messages` is hit.
/// Returns the response text (if any) or an error.
pub async fn handle_teams_activity(activity: TeamsActivity, state: &TeamsBotState) -> Result<()> {
    // Only handle "message" activities
    if activity.type_ != "message" {
        info!(
            "[TEAMS] Ignoring activity type '{}' from {}",
            activity.type_, activity.from.name
        );
        return Ok(());
    }

    let text = activity.text.trim().to_string();
    if text.is_empty() {
        return Ok(());
    }

    let conversation_id = activity.conversation.id.clone();
    let service_url = activity.service_url.clone();
    let sender_name = activity.from.name.clone();

    // Message deduplication using activity id
    if !activity.id.is_empty() {
        let mut dedup = state.msg_dedup.lock().await;
        if dedup.put(activity.id.clone(), ()).is_some() {
            debug!("[TEAMS] Duplicate activity {}, skipping", activity.id);
            return Ok(());
        }
    }

    // Per-user rate limiting
    {
        let rate_key = format!("teams:{}", activity.from.id);
        if let Err(e) = state.rate_limiter.check(&rate_key) {
            warn!("[TEAMS] Rate limit hit for user {}: {}", activity.from.id, e);
            let token = state.get_token().await.unwrap_or_default();
            let config = state.app_state.config.read().await;
            let app_id = config.teams.app_id.clone();
            drop(config);
            if let Err(e) = send_teams_reply(
                &service_url,
                &conversation_id,
                "Too many messages. Please wait a moment.",
                &app_id,
                &token,
            )
            .await
            {
                warn!("[TEAMS] Failed to send reply: {}", e);
            }
            return Ok(());
        }
    }

    if !is_allowed_teams_service_url(&service_url) {
        warn!(
            "[TEAMS] Rejecting activity with invalid service_url: {}",
            service_url
        );
        return Ok(());
    }

    info!(
        "[TEAMS] Message from {} in {}: {}",
        sender_name,
        conversation_id,
        &text[..text.len().min(80)]
    );

    // Check team allowlist
    {
        let config = state.app_state.config.read().await;
        let tenant_id = activity.conversation.tenant_id.as_deref();
        if !is_tenant_allowed(&config.teams.allowed_team_ids, tenant_id) {
            match tenant_id {
                Some(tenant) => {
                    warn!(
                        "[TEAMS] Tenant {} not in allowed_team_ids, ignoring",
                        tenant
                    );
                }
                None => {
                    warn!(
                        "[TEAMS] Missing tenant_id while allowed_team_ids is configured, ignoring"
                    );
                }
            }
            return Ok(());
        }
    }

    // --- DM policy enforcement ---
    // Admins (admin_user_ids) are always allowed regardless of policy.
    let sender_id = activity.from.id.clone();
    {
        let (dm_policy, admin_user_ids, app_id) = {
            let config = state.app_state.config.read().await;
            (
                config.teams.dm_policy,
                config.teams.admin_user_ids.clone(),
                config.teams.app_id.clone(),
            )
        };
        if !admin_user_ids.contains(&sender_id) {
            match dm_policy {
                crate::pairing::DmPolicy::Open => {}
                crate::pairing::DmPolicy::Allowlist => {
                    let allowed = {
                        let mgr = state.app_state.pairing_manager.read().await;
                        mgr.is_channel_allowed("teams", &sender_id, &admin_user_ids)
                    };
                    if !allowed {
                        let token = state.get_token().await.unwrap_or_default();
                        if let Err(e) = send_teams_reply(
                            &service_url,
                            &conversation_id,
                            "You are not authorized to use this bot.",
                            &app_id,
                            &token,
                        )
                        .await
                        {
                            warn!("[TEAMS] Failed to send reply: {}", e);
                        }
                        return Ok(());
                    }
                }
                crate::pairing::DmPolicy::Pairing => {
                    let allowed = {
                        let mgr = state.app_state.pairing_manager.read().await;
                        mgr.is_channel_allowed("teams", &sender_id, &admin_user_ids)
                    };
                    if !allowed {
                        let result = state
                            .app_state
                            .pairing_manager
                            .write()
                            .await
                            .create_pairing_request("teams", &sender_id, Some(sender_name.clone()));
                        match result {
                            Ok(code) => {
                                let token = state.get_token().await.unwrap_or_default();
                                if let Err(e) = send_teams_reply(
                                    &service_url,
                                    &conversation_id,
                                    &format!(
                                        "Pairing request created. Share this code with an admin: {}",
                                        code
                                    ),
                                    &app_id,
                                    &token,
                                )
                                .await
                                {
                                    warn!("[TEAMS] Failed to send reply: {}", e);
                                }
                            }
                            Err(e) => {
                                warn!("[TEAMS] Pairing request failed for {}: {}", sender_id, e);
                            }
                        }
                        return Ok(());
                    }
                }
            }
        }
    }

    // Keep latest reply endpoint for background approvals.
    state
        .app_state
        .teams_conversation_service_urls
        .write()
        .await
        .insert(conversation_id.clone(), service_url.clone());

    // Strip bot mention from text (Teams includes @mention in message text).
    let clean_text = strip_bot_mention(&text);
    if clean_text.trim().is_empty() {
        return Ok(());
    }

    let text_lc = clean_text.trim().to_lowercase();
    if matches!(
        text_lc.as_str(),
        "approve" | "deny" | "/approve" | "/deny" | "!approve" | "!deny"
    ) {
        let key = (conversation_id.clone(), activity.from.id.clone());
        let (approval_tx, owner_mismatch) = {
            let mut map = state.app_state.teams_pending_approvals.lock().await;
            if let Some(tx) = map.remove(&key) {
                (Some(tx), false)
            } else {
                let mismatch = map.keys().any(|(pending_conversation_id, _)| {
                    pending_conversation_id == &conversation_id
                });
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

            let token = state.get_token().await.unwrap_or_default();
            let config = state.app_state.config.read().await;
            let app_id = config.teams.app_id.clone();
            drop(config);
            if let Err(e) = send_teams_reply(&service_url, &conversation_id, reply, &app_id, &token).await {
                warn!("[TEAMS] Failed to send reply: {}", e);
            }
            return Ok(());
        }
        if owner_mismatch {
            let token = state.get_token().await.unwrap_or_default();
            let config = state.app_state.config.read().await;
            let app_id = config.teams.app_id.clone();
            drop(config);
            if let Err(e) = send_teams_reply(
                &service_url,
                &conversation_id,
                "This approval request belongs to another user in this conversation.",
                &app_id,
                &token,
            )
            .await
            {
                warn!("[TEAMS] Failed to send reply: {}", e);
            }
            return Ok(());
        }
    }

    // Handle bot commands
    if clean_text.starts_with('/') || clean_text.starts_with('!') {
        handle_command(&clean_text, &conversation_id, &service_url, state).await;
        return Ok(());
    }

    // Route through the unified pipeline
    let claude_client = state.get_or_create_client(&conversation_id).await;

    let message = IncomingMessage {
        text: clean_text,
        channel: ChannelSource::Teams {
            conversation_id: conversation_id.clone(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_send_config = {
        let config = state.app_state.config.read().await;
        !config.teams.app_id.trim().is_empty() && !config.teams.app_password.trim().is_empty()
    };

    let observer = TeamsObserver::new(
        state.app_state.clone(),
        conversation_id.clone(),
        activity.from.id.clone(),
        service_url.clone(),
        has_send_config,
        state.app_state.teams_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::teams(conversation_id.clone(), activity.from.id.clone()),
        observer: &observer,
        streaming: false,
        window: None,
        on_stream_chunk: None,
        auto_compact: true,
        save_to_memory: true,
        sync_supermemory: true,
        check_sensitive_data: true,
    };

    let token = state.get_token().await.unwrap_or_default();
    let config = state.app_state.config.read().await;
    let app_id = config.teams.app_id.clone();
    drop(config);

    match router::route_message(&message, options, &state.app_state).await {
        Ok(routed) => {
            let response = router::extract_text_from_response(&routed.text);
            if response.is_empty() {
                if let Err(e) = send_teams_reply(
                    &service_url,
                    &conversation_id,
                    "(No response)",
                    &app_id,
                    &token,
                )
                .await
                {
                    warn!("[TEAMS] Failed to send reply: {}", e);
                }
            } else {
                for chunk in router::split_message(&response, 4096) {
                    if let Err(e) =
                        send_teams_reply(&service_url, &conversation_id, &chunk, &app_id, &token)
                            .await
                    {
                        warn!("[TEAMS] Failed to send reply: {}", e);
                    }
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            if let Err(e) = send_teams_reply(&service_url, &conversation_id, &msg, &app_id, &token).await {
                warn!("[TEAMS] Failed to send reply: {}", e);
            }
        }
        Err(e) => {
            if let Err(e) = send_teams_reply(
                &service_url,
                &conversation_id,
                &format!("Error: {}", e),
                &app_id,
                &token,
            )
            .await
            {
                warn!("[TEAMS] Failed to send reply: {}", e);
            }
        }
    }

    Ok(())
}

fn is_tenant_allowed(allowed_team_ids: &[String], tenant_id: Option<&str>) -> bool {
    if allowed_team_ids.is_empty() {
        return true;
    }

    match tenant_id {
        Some(tenant) => allowed_team_ids.iter().any(|id| id == tenant),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_allowed_botframework_issuer, is_allowed_teams_service_url, is_tenant_allowed};

    #[test]
    fn tenant_allowed_when_allowlist_empty() {
        let allowlist: Vec<String> = Vec::new();
        assert!(is_tenant_allowed(&allowlist, Some("tenant-1")));
        assert!(is_tenant_allowed(&allowlist, None));
    }

    #[test]
    fn tenant_allowed_when_match_found() {
        let allowlist = vec!["tenant-1".to_string(), "tenant-2".to_string()];
        assert!(is_tenant_allowed(&allowlist, Some("tenant-2")));
    }

    #[test]
    fn tenant_rejected_when_missing_or_not_listed() {
        let allowlist = vec!["tenant-1".to_string()];
        assert!(!is_tenant_allowed(&allowlist, None));
        assert!(!is_tenant_allowed(&allowlist, Some("tenant-3")));
    }

    #[test]
    fn botframework_issuer_allowlist_accepts_known_issuers() {
        assert!(is_allowed_botframework_issuer(
            "https://api.botframework.com"
        ));
        assert!(is_allowed_botframework_issuer(
            "https://api.botframework.com/"
        ));
        assert!(is_allowed_botframework_issuer(
            "https://api.botframework.us"
        ));
    }

    #[test]
    fn botframework_issuer_allowlist_rejects_unknown_issuers() {
        assert!(!is_allowed_botframework_issuer(
            "https://login.microsoftonline.com"
        ));
        assert!(!is_allowed_botframework_issuer(""));
    }

    #[test]
    fn teams_service_url_allows_https_and_local_emulator_http() {
        assert!(is_allowed_teams_service_url(
            "https://smba.trafficmanager.net/amer/"
        ));
        assert!(is_allowed_teams_service_url("http://localhost:3978"));
        assert!(is_allowed_teams_service_url("http://127.0.0.1:3978"));
    }

    #[test]
    fn teams_service_url_rejects_insecure_or_invalid_remote_urls() {
        assert!(!is_allowed_teams_service_url("http://example.com"));
        assert!(!is_allowed_teams_service_url("ftp://example.com"));
        assert!(!is_allowed_teams_service_url("not-a-url"));
    }
}

/// Axum route handler for incoming Teams activities.
pub async fn teams_activity_webhook_handler(
    AxumState(state): AxumState<Arc<TeamsBotState>>,
    headers: HeaderMap,
    Json(activity): Json<TeamsActivity>,
) -> StatusCode {
    {
        let config = state.app_state.config.read().await;
        if !config.teams.enabled {
            return StatusCode::NOT_FOUND;
        }
        if config.teams.app_id.is_empty() || config.teams.app_password.is_empty() {
            warn!("[TEAMS] app_id/app_password missing — rejecting webhook activity");
            return StatusCode::UNAUTHORIZED;
        }
    }

    if let Err(e) = state.verify_incoming_bearer(&headers).await {
        warn!("[TEAMS] Invalid incoming bearer token: {}", e);
        return StatusCode::UNAUTHORIZED;
    }

    match handle_teams_activity(activity, &state).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            warn!("[TEAMS] Failed to process activity: {}", e);
            StatusCode::BAD_REQUEST
        }
    }
}

/// Strip bot @mention from the message text.
///
/// Teams includes `<at>BotName</at>` in the message text when the bot is mentioned.
fn strip_bot_mention(text: &str) -> String {
    // Remove <at>...</at> tags
    let re_pattern = "<at>[^<]*</at>";
    if let Ok(re) = regex::Regex::new(re_pattern) {
        re.replace_all(text, "").trim().to_string()
    } else {
        text.to_string()
    }
}

async fn send_teams_message(
    app_state: &AppState,
    service_url: &str,
    conversation_id: &str,
    text: &str,
) {
    let _ = send_teams_message_checked(app_state, service_url, conversation_id, text).await;
}

async fn send_teams_message_checked(
    app_state: &AppState,
    service_url: &str,
    conversation_id: &str,
    text: &str,
) -> bool {
    let (app_id, app_password, tenant_id) = {
        let config = app_state.config.read().await;
        (
            config.teams.app_id.clone(),
            app_state
                .key_interceptor
                .restore_config_string(&config.teams.app_password),
            config.teams.tenant_id.clone(),
        )
    };

    if app_id.is_empty() || app_password.is_empty() {
        warn!("[TEAMS] Cannot send approval prompt: app_id/app_password missing");
        return false;
    }

    let token = match get_bot_token(&app_id, &app_password, tenant_id.as_deref()).await {
        Ok(token) => token,
        Err(e) => {
            warn!("[TEAMS] Failed to acquire token for approval prompt: {}", e);
            return false;
        }
    };

    if let Err(e) = send_teams_reply(service_url, conversation_id, text, &app_id, &token).await {
        warn!("[TEAMS] Failed to send approval prompt: {}", e);
        false
    } else {
        true
    }
}

/// Handle Teams bot commands.
async fn handle_command(
    text: &str,
    conversation_id: &str,
    service_url: &str,
    state: &TeamsBotState,
) {
    let cmd = text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('/')
        .trim_start_matches('!');

    let token = state.get_token().await.unwrap_or_default();
    let config = state.app_state.config.read().await;
    let app_id = config.teams.app_id.clone();
    drop(config);

    match cmd {
        "help" | "start" => {
            if let Err(e) = send_teams_reply(
                service_url,
                conversation_id,
                "**NexiBot Commands:**\n\n\
                 - `/new` - Start a new conversation\n\
                 - `/status` - Check bot status\n\
                 - `/help` - Show this help message\n\n\
                 Mention the bot or send a DM to chat.",
                &app_id,
                &token,
            )
            .await
            {
                warn!("[TEAMS] Failed to send reply: {}", e);
            }
        }
        "new" => {
            state.clear_session(conversation_id).await;
            if let Err(e) = send_teams_reply(
                service_url,
                conversation_id,
                "Conversation cleared. Starting fresh!",
                &app_id,
                &token,
            )
            .await
            {
                warn!("[TEAMS] Failed to send reply: {}", e);
            }
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
            if let Err(e) = send_teams_reply(
                service_url,
                conversation_id,
                &format!(
                    "NexiBot is online.\nModel: {}\nAuth: {}",
                    model, auth_status
                ),
                &app_id,
                &token,
            )
            .await
            {
                warn!("[TEAMS] Failed to send reply: {}", e);
            }
        }
        _ => {
            if let Err(e) = send_teams_reply(
                service_url,
                conversation_id,
                "Unknown command. Use `/help` for available commands.",
                &app_id,
                &token,
            )
            .await
            {
                warn!("[TEAMS] Failed to send reply: {}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Bot Framework REST API communication
// ---------------------------------------------------------------------------

/// Send a reply to a Teams conversation via the Bot Framework REST API.
pub async fn send_teams_reply(
    service_url: &str,
    conversation_id: &str,
    text: &str,
    _app_id: &str,
    token: &str,
) -> Result<()> {
    if !is_allowed_teams_service_url(service_url) {
        return Err(anyhow::anyhow!(
            "Blocked Teams reply: invalid service_url {}",
            service_url
        ));
    }

    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());

    // Bot Framework v3 API endpoint for sending activities
    let url = format!(
        "{}v3/conversations/{}/activities",
        ensure_trailing_slash(service_url),
        conversation_id
    );

    let body = json!({
        "type": "message",
        "text": text,
        "textFormat": "markdown",
    });

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        warn!("[TEAMS] Failed to send reply ({}): {}", status, body);
        return Err(anyhow::anyhow!("Teams reply failed: {}", status));
    }

    Ok(())
}

/// Acquire a Bot Framework OAuth2 token from Microsoft identity platform.
///
/// Uses the client credentials flow:
/// POST https://login.microsoftonline.com/{tenantId}/oauth2/v2.0/token
pub async fn get_bot_token(
    app_id: &str,
    app_password: &str,
    tenant_id: Option<&str>,
) -> Result<String> {
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());

    let tenant = tenant_id.unwrap_or("botframework.com");
    let url = format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
        tenant
    );

    let params = [
        ("grant_type", "client_credentials"),
        ("client_id", app_id),
        ("client_secret", app_password),
        ("scope", "https://api.botframework.com/.default"),
    ];

    let response = client
        .post(&url)
        .form(&params)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Failed to acquire bot token ({}): {}",
            status,
            body
        ));
    }

    let token_response: Value = response.json().await?;
    let access_token = token_response["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing access_token in token response"))?
        .to_string();

    info!("[TEAMS] Acquired bot framework token");
    Ok(access_token)
}

/// Ensure a URL ends with a trailing slash.
fn ensure_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{}/", url)
    }
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum number of concurrent Teams chat sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically clean up stale Teams chat sessions (>24h inactive).
pub async fn session_cleanup_loop(state: Arc<TeamsBotState>) {
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
                "[TEAMS] Cleaned up {} stale sessions ({} remaining)",
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
                "[TEAMS] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}
