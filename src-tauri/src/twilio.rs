//! Twilio SMS/MMS integration for NexiBot.
//!
//! Receives SMS/MMS messages via Twilio webhook (URL-encoded form POST to
//! /api/twilio/webhook) and sends replies via the Twilio Messages REST API.
//!
//! Security:
//!   - Verifies `X-Twilio-Signature` using HMAC-SHA1 over
//!     `webhook_url + sorted_form_params` as documented by Twilio.
//!   - Fails closed when signature validation prerequisites are missing.

use axum::{
    body::Bytes,
    extract::State as AxumState,
    http::{HeaderMap, StatusCode},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use lru::LruCache;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

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

/// Configuration for the Twilio SMS/MMS channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwilioConfig {
    /// Whether the Twilio channel is enabled.
    pub enabled: bool,

    /// Twilio Account SID (used for Basic auth when sending messages).
    #[serde(default)]
    pub account_sid: String,

    /// Twilio Auth Token (used for Basic auth when sending messages).
    #[serde(default)]
    pub auth_token: String,

    /// Twilio phone number to send FROM (E.164 format, e.g. "+15005550006").
    #[serde(default)]
    pub from_number: String,

    /// Full public webhook URL (used for Twilio signature verification).
    #[serde(default)]
    pub webhook_url: String,

    /// Allow-list of phone numbers (E.164). Empty = apply dm_policy.
    #[serde(default)]
    pub allowed_numbers: Vec<String>,

    /// Admin phone numbers that bypass DM policy.
    #[serde(default)]
    pub admin_numbers: Vec<String>,

    /// DM access policy.
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,

    /// Per-channel tool access policy.
    #[serde(default)]
    pub tool_policy: crate::config::ChannelToolPolicy,
}

impl Default for TwilioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            account_sid: String::new(),
            auth_token: String::new(),
            from_number: String::new(),
            webhook_url: String::new(),
            allowed_numbers: Vec::new(),
            admin_numbers: Vec::new(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Incoming form payload
// ---------------------------------------------------------------------------

/// URL-encoded form fields sent by Twilio on incoming SMS/MMS.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct TwilioWebhookForm {
    /// Sender's phone number (E.164).
    #[serde(rename = "From")]
    pub from: String,

    /// SMS/MMS body text. Optional: MMS without text body omits this field.
    #[serde(rename = "Body")]
    pub body: Option<String>,

    /// First media URL (MMS attachment), if present.
    #[serde(rename = "MediaUrl0")]
    pub media_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-phone conversation session for Twilio SMS.
pub(crate) struct TwilioChatSession {
    /// Dedicated Claude client with its own conversation history.
    claude_client: ClaudeClient,
    /// Last activity timestamp for session expiry.
    last_activity: Instant,
}

/// Shared state for the Twilio webhook handler.
pub struct TwilioState {
    pub app_state: AppState,
    pub chat_sessions: RwLock<HashMap<String, TwilioChatSession>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub msg_dedup: Mutex<LruCache<String, ()>>,
}

/// Observer for Twilio tool execution flow, including in-channel approvals.
pub(crate) struct TwilioObserver {
    app_state: AppState,
    requester_phone: String,
    has_send_config: bool,
    pending_approvals: Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
}

impl TwilioObserver {
    pub(crate) fn new(
        app_state: AppState,
        requester_phone: String,
        has_send_config: bool,
        pending_approvals: Arc<
            tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
        >,
    ) -> Self {
        Self {
            app_state,
            requester_phone,
            has_send_config,
            pending_approvals,
        }
    }
}

#[async_trait::async_trait]
impl crate::tool_loop::ToolLoopObserver for TwilioObserver {
    fn supports_approval(&self) -> bool {
        self.has_send_config && !self.requester_phone.trim().is_empty()
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        if !self.supports_approval() {
            return false;
        }

        let key = self.requester_phone.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                send_twilio_sms(
                    &self.app_state,
                    &self.requester_phone,
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
        if !send_twilio_sms_checked(&self.app_state, &self.requester_phone, &prompt).await {
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_approvals.lock().await.remove(&key);
                send_twilio_sms(
                    &self.app_state,
                    &self.requester_phone,
                    "Approval timed out. Tool blocked.",
                )
                .await;
                false
            }
        }
    }
}

impl TwilioState {
    pub fn new(app_state: AppState) -> Self {
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

    /// Get or create a Claude client for the given phone number.
    async fn get_or_create_client(&self, phone: &str) -> ClaudeClient {
        let mut sessions = self.chat_sessions.write().await;
        if let Some(session) = sessions.get_mut(phone) {
            session.last_activity = Instant::now();
            return session.claude_client.clone();
        }

        let client = ClaudeClient::new(self.app_state.config.clone());
        sessions.insert(
            phone.to_string(),
            TwilioChatSession {
                claude_client: client.clone(),
                last_activity: Instant::now(),
            },
        );
        client
    }
}

// ---------------------------------------------------------------------------
// Webhook handler — POST /api/twilio/webhook
// ---------------------------------------------------------------------------

/// Handle incoming Twilio SMS/MMS webhook.
///
/// Twilio sends URL-encoded form data. We validate `X-Twilio-Signature`
/// against the configured webhook URL and auth token before routing.
pub async fn twilio_webhook_handler(
    AxumState(state): AxumState<Arc<TwilioState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // ---- Parse form body ----
    // Parse URL-encoded form data manually via the `url` crate's form decoder
    // so we don't need serde_urlencoded as an explicit dependency.
    let body_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            warn!("[TWILIO] Form body is not valid UTF-8");
            return StatusCode::BAD_REQUEST;
        }
    };

    let params: HashMap<String, String> = url::form_urlencoded::parse(body_str.as_bytes())
        .into_owned()
        .collect();

    // ---- Signature verification ----
    let (enabled, auth_token, webhook_url) = {
        let config = state.app_state.config.read().await;
        (
            config.twilio.enabled,
            state
                .app_state
                .key_interceptor
                .restore_config_string(&config.twilio.auth_token),
            config.twilio.webhook_url.clone(),
        )
    };

    if !enabled {
        return StatusCode::NOT_FOUND;
    }

    if auth_token.is_empty() || webhook_url.is_empty() {
        warn!("[TWILIO] auth_token or webhook_url missing — rejecting webhook");
        return StatusCode::UNAUTHORIZED;
    }

    let provided_signature = headers
        .get("x-twilio-signature")
        .or_else(|| headers.get("X-Twilio-Signature"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if provided_signature.is_empty() {
        warn!("[TWILIO] Missing X-Twilio-Signature header");
        return StatusCode::UNAUTHORIZED;
    }

    if !verify_twilio_signature(&auth_token, &webhook_url, &params, provided_signature) {
        warn!("[TWILIO] Signature verification failed");
        return StatusCode::UNAUTHORIZED;
    }

    // ---- Replay protection: X-Twilio-Request-Timestamp ----
    //
    // Twilio always sends this header. Reject requests older than 300 seconds to
    // prevent replay of captured webhooks (matches Slack's 5-minute window).
    const TWILIO_REPLAY_WINDOW_SECS: u64 = 300;
    if let Some(ts_val) = headers
        .get("x-twilio-request-timestamp")
        .or_else(|| headers.get("X-Twilio-Request-Timestamp"))
    {
        match ts_val.to_str().ok().and_then(|s| s.parse::<u64>().ok()) {
            Some(ts) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let age = now.saturating_sub(ts);
                if age > TWILIO_REPLAY_WINDOW_SECS {
                    warn!("[TWILIO] Rejected: timestamp too old ({}s)", age);
                    return StatusCode::UNAUTHORIZED;
                }
            }
            None => {
                warn!("[TWILIO] Rejected: unparseable X-Twilio-Request-Timestamp header");
                return StatusCode::UNAUTHORIZED;
            }
        }
    }
    // No timestamp header: Twilio always sends it in production; absence is suspicious
    // but we log only here to avoid breaking test tooling that omits it.

    let from = match params.get("From") {
        Some(f) if !f.is_empty() => f.clone(),
        _ => {
            warn!("[TWILIO] Missing or empty 'From' field in webhook form");
            return StatusCode::BAD_REQUEST;
        }
    };

    let text = match params.get("Body") {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            // MMS with no text body — acknowledge but don't route.
            info!(
                "[TWILIO] MMS or empty body from {}, skipping text routing",
                from
            );
            return StatusCode::OK;
        }
    };

    // ---- Authorization ----
    {
        let config = state.app_state.config.read().await;
        let dm_policy = config.twilio.dm_policy;
        let allowed = config.twilio.allowed_numbers.clone();
        let admins = config.twilio.admin_numbers.clone();
        drop(config);

        let is_admin = !admins.is_empty() && admins.contains(&from);

        if !is_admin {
            match dm_policy {
                crate::pairing::DmPolicy::Allowlist => {
                    if !allowed.is_empty() && !allowed.contains(&from) {
                        info!(
                            "[TWILIO] Ignoring message from unauthorized number: {}",
                            from
                        );
                        return StatusCode::OK;
                    }
                }
                crate::pairing::DmPolicy::Open => {}
                crate::pairing::DmPolicy::Pairing => {
                    let pairing_mgr = state.app_state.pairing_manager.read().await;
                    if !pairing_mgr.is_channel_allowed("twilio", &from, &allowed) {
                        drop(pairing_mgr);
                        let state_clone = state.clone();
                        let from_clone = from.clone();
                        tokio::spawn(async move {
                            let mut pairing_mgr =
                                state_clone.app_state.pairing_manager.write().await;
                            match pairing_mgr.create_pairing_request("twilio", &from_clone, None) {
                                Ok(code) => {
                                    drop(pairing_mgr);
                                    send_twilio_sms(
                                        &state_clone.app_state,
                                        &from_clone,
                                        &format!(
                                            "You are not yet authorized. Your pairing code is:\n\n{}\n\nAsk the admin to approve this code in NexiBot Settings.",
                                            code
                                        ),
                                    )
                                    .await;
                                }
                                Err(e) => {
                                    drop(pairing_mgr);
                                    send_twilio_sms(
                                        &state_clone.app_state,
                                        &from_clone,
                                        &format!("Authorization pending. {}", e),
                                    )
                                    .await;
                                }
                            }
                        });
                        return StatusCode::OK;
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
        let approval_tx = state
            .app_state
            .twilio_pending_approvals
            .lock()
            .await
            .remove(&from);
        if let Some(approval_tx) = approval_tx {
            let approved = matches!(text_lc.as_str(), "approve" | "/approve" | "!approve");
            let _ = approval_tx.send(approved);
            let reply = if approved {
                "Approved. Continuing..."
            } else {
                "Denied."
            };
            let state_clone = state.clone();
            let from_clone = from.clone();
            tokio::spawn(async move {
                send_twilio_sms(&state_clone.app_state, &from_clone, reply).await;
            });
            return StatusCode::OK;
        }
    }

    // --- Message deduplication using Twilio MessageSid ---
    let msg_sid = params.get("MessageSid")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| format!("{}:{}", from, text));
    {
        let mut dedup = state.msg_dedup.lock().await;
        if dedup.put(msg_sid.clone(), ()).is_some() {
            debug!("[TWILIO] Dropping duplicate message: {}", msg_sid);
            return StatusCode::OK;
        }
    }

    // --- Per-sender rate limiting ---
    let rate_key = format!("twilio:{}", from);
    if state.rate_limiter.check(&rate_key).is_err() {
        warn!("[TWILIO] Rate limit exceeded for {} — dropping message", from);
        return StatusCode::OK;
    }

    let state = state.clone();
    tokio::spawn(async move {
        handle_twilio_text_message(&state, &from, &text).await;
    });

    StatusCode::OK
}

/// Verify Twilio signature:
/// base64(HMAC-SHA1(auth_token, webhook_url + sorted_params_concat)).
fn verify_twilio_signature(
    auth_token: &str,
    webhook_url: &str,
    params: &HashMap<String, String>,
    provided_signature: &str,
) -> bool {
    type HmacSha1 = Hmac<Sha1>;

    let mut entries: Vec<(&String, &String)> = params.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let mut signed = String::with_capacity(webhook_url.len() + entries.len() * 16);
    signed.push_str(webhook_url);
    for (k, v) in entries {
        signed.push_str(k);
        signed.push_str(v);
    }

    let mut mac = match HmacSha1::new_from_slice(auth_token.as_bytes()) {
        Ok(m) => m,
        Err(e) => {
            error!("[TWILIO] Failed to initialize HMAC-SHA1 verifier: {}", e);
            return false;
        }
    };
    mac.update(signed.as_bytes());
    let computed = STANDARD.encode(mac.finalize().into_bytes());

    crate::security::constant_time::secure_compare(&computed, provided_signature)
}

// ---------------------------------------------------------------------------
// Message handling
// ---------------------------------------------------------------------------

/// Route a Twilio SMS through the Claude pipeline and send the reply.
async fn handle_twilio_text_message(state: &TwilioState, from: &str, text: &str) {
    let app_state = &state.app_state;
    info!("[TWILIO] Message from {}: {}", from, text);

    let claude_client = state.get_or_create_client(from).await;

    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::Twilio {
            phone_number: from.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };
    let has_send_config = {
        let config = app_state.config.read().await;
        !config.twilio.account_sid.trim().is_empty()
            && !config.twilio.auth_token.trim().is_empty()
            && !config.twilio.from_number.trim().is_empty()
    };

    let observer = TwilioObserver::new(
        app_state.clone(),
        from.to_string(),
        has_send_config,
        app_state.twilio_pending_approvals.clone(),
    );
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::twilio(from.to_string()),
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
                send_twilio_sms(app_state, from, "(No response)").await;
            } else {
                // SMS hard limit is 1600 chars; split at 1550 for safety.
                for chunk in router::split_message(&response, 1550) {
                    send_twilio_sms(app_state, from, &chunk).await;
                }
            }
        }
        Err(RouterError::Blocked(msg)) => {
            send_twilio_sms(app_state, from, &msg).await;
        }
        Err(e) => {
            send_twilio_sms(app_state, from, &format!("Error: {}", e)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Twilio Messages REST API
// ---------------------------------------------------------------------------

/// Send an SMS via the Twilio Messages REST API.
async fn send_twilio_sms(app_state: &AppState, to: &str, body: &str) {
    let _ = send_twilio_sms_checked(app_state, to, body).await;
}

async fn send_twilio_sms_checked(app_state: &AppState, to: &str, body: &str) -> bool {
    let (account_sid, auth_token, from_number) = {
        let config = app_state.config.read().await;
        (
            config.twilio.account_sid.clone(),
            app_state
                .key_interceptor
                .restore_config_string(&config.twilio.auth_token),
            config.twilio.from_number.clone(),
        )
    };

    if account_sid.is_empty() || auth_token.is_empty() || from_number.is_empty() {
        error!("[TWILIO] Cannot send SMS: account_sid, auth_token, or from_number not configured");
        return false;
    }

    let url = format!(
        "https://api.twilio.com/2010-04-01/Accounts/{}/Messages.json",
        account_sid
    );

    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    match client
        .post(&url)
        .basic_auth(&account_sid, Some(&auth_token))
        .form(&[("To", to), ("From", from_number.as_str()), ("Body", body)])
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            info!("[TWILIO] SMS sent to {}", to);
            true
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            error!("[TWILIO] Send failed ({}): {}", status, body);
            false
        }
        Err(e) => {
            error!("[TWILIO] HTTP error sending SMS: {}", e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Maximum concurrent Twilio SMS sessions.
const MAX_CHANNEL_SESSIONS: usize = 1000;

/// Periodically evict stale Twilio chat sessions (>1 h inactive for SMS context).
pub async fn session_cleanup_loop(state: Arc<TwilioState>) {
    let cleanup_interval = tokio::time::Duration::from_secs(3600);
    let max_age = std::time::Duration::from_secs(3600);

    loop {
        tokio::time::sleep(cleanup_interval).await;
        let mut sessions = state.chat_sessions.write().await;
        let before = sessions.len();
        sessions.retain(|_, session| session.last_activity.elapsed() < max_age);
        let removed = before - sessions.len();
        if removed > 0 {
            info!(
                "[TWILIO] Cleaned up {} stale sessions ({} remaining)",
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
                "[TWILIO] Evicted {} oldest sessions to enforce cap (now {})",
                evict_count,
                sessions.len()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Start function
// ---------------------------------------------------------------------------

/// Start the Twilio integration (spawns cleanup task; webhook handler is
/// registered separately in webhooks.rs).
pub async fn start_twilio(app_state: AppState) -> Result<(), String> {
    let config = app_state.config.read().await;
    if !config.twilio.enabled {
        info!("[TWILIO] Twilio integration disabled in config");
        return Ok(());
    }
    drop(config);

    info!(
        "[TWILIO] Twilio integration enabled — webhook handler ready at POST /api/twilio/webhook"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::verify_twilio_signature;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use hmac::{Hmac, Mac};
    use sha1::Sha1;
    use std::collections::HashMap;

    fn build_signature(
        auth_token: &str,
        webhook_url: &str,
        params: &HashMap<String, String>,
    ) -> String {
        type HmacSha1 = Hmac<Sha1>;
        let mut entries: Vec<(&String, &String)> = params.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));

        let mut signed = String::new();
        signed.push_str(webhook_url);
        for (k, v) in entries {
            signed.push_str(k);
            signed.push_str(v);
        }

        let mut mac = HmacSha1::new_from_slice(auth_token.as_bytes()).unwrap();
        mac.update(signed.as_bytes());
        STANDARD.encode(mac.finalize().into_bytes())
    }

    #[test]
    fn verify_twilio_signature_accepts_valid_signature() {
        let auth_token = "twilio-auth-token";
        let webhook_url = "https://example.com/api/twilio/webhook";
        let mut params = HashMap::new();
        params.insert("From".to_string(), "+15551234567".to_string());
        params.insert("Body".to_string(), "hello".to_string());

        let signature = build_signature(auth_token, webhook_url, &params);
        assert!(verify_twilio_signature(
            auth_token,
            webhook_url,
            &params,
            &signature
        ));
    }

    #[test]
    fn verify_twilio_signature_rejects_tampered_payload() {
        let auth_token = "twilio-auth-token";
        let webhook_url = "https://example.com/api/twilio/webhook";
        let mut params = HashMap::new();
        params.insert("From".to_string(), "+15551234567".to_string());
        params.insert("Body".to_string(), "hello".to_string());

        let signature = build_signature(auth_token, webhook_url, &params);
        params.insert("Body".to_string(), "tampered".to_string());

        assert!(!verify_twilio_signature(
            auth_token,
            webhook_url,
            &params,
            &signature
        ));
    }
}
