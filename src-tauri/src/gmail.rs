//! Gmail channel integration using Google's Gmail API.
//!
//! Polls for unread messages via the Gmail REST API and sends replies using
//! `messages.send`. Uses OAuth2 with a refresh token for authentication.
//!
//! Unlike the generic IMAP/SMTP `email.rs` channel, this uses Google's native
//! Gmail API which provides better reliability, label management, and push
//! notification support via Pub/Sub.

use anyhow::{Context, Result};
use base64::Engine;
use chrono::{DateTime, Utc};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use tracing::{debug, info, warn};

use crate::channel::{ChannelAdapter, ChannelSource};
use crate::commands::AppState;
use crate::router::{self, IncomingMessage, RouteOptions};
use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::ToolLoopConfig;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the Gmail channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailConfig {
    /// Whether the Gmail channel is enabled.
    pub enabled: bool,

    /// Google OAuth2 client ID.
    #[serde(default)]
    pub client_id: String,

    /// Google OAuth2 client secret.
    #[serde(default)]
    pub client_secret: String,

    /// OAuth2 refresh token (long-lived, used to obtain access tokens).
    #[serde(default)]
    pub refresh_token: String,

    /// The email address this bot sends from (used in From header).
    #[serde(default)]
    pub from_address: String,

    /// Allow-list of sender addresses. Empty = accept all.
    #[serde(default)]
    pub allowed_senders: Vec<String>,

    /// Gmail label to monitor (default: "INBOX").
    #[serde(default = "default_label")]
    pub label: String,

    /// Seconds between Gmail inbox polls.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_seconds: u64,

    /// Maximum number of messages to fetch per poll cycle.
    #[serde(default = "default_max_messages_per_poll")]
    pub max_messages_per_poll: u32,

    /// DM policy controlling who may interact.
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,

    /// Per-channel tool access policy.
    #[serde(default)]
    pub tool_policy: crate::config::ChannelToolPolicy,
}

fn default_label() -> String {
    "INBOX".to_string()
}
fn default_poll_interval() -> u64 {
    30
}
fn default_max_messages_per_poll() -> u32 {
    10
}

impl Default for GmailConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            client_id: String::new(),
            client_secret: String::new(),
            refresh_token: String::new(),
            from_address: String::new(),
            allowed_senders: Vec::new(),
            label: default_label(),
            poll_interval_seconds: default_poll_interval(),
            max_messages_per_poll: default_max_messages_per_poll(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Gmail API response types
// ---------------------------------------------------------------------------

/// Response from `messages.list`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListMessagesResponse {
    #[serde(default)]
    messages: Vec<MessageRef>,
    #[serde(default)]
    #[allow(dead_code)]
    next_page_token: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    result_size_estimate: u32,
}

/// Minimal message reference from `messages.list`.
#[derive(Debug, Deserialize)]
struct MessageRef {
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    thread_id: String,
}

/// Full message from `messages.get`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GmailMessage {
    id: String,
    thread_id: String,
    #[serde(default)]
    #[allow(dead_code)]
    label_ids: Vec<String>,
    #[allow(dead_code)]
    snippet: Option<String>,
    payload: Option<MessagePayload>,
    internal_date: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessagePayload {
    #[serde(default)]
    headers: Vec<Header>,
    #[serde(default)]
    parts: Vec<MessagePart>,
    mime_type: Option<String>,
    body: Option<MessageBody>,
}

#[derive(Debug, Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessagePart {
    mime_type: Option<String>,
    body: Option<MessageBody>,
    #[serde(default)]
    parts: Vec<MessagePart>,
}

#[derive(Debug, Deserialize)]
struct MessageBody {
    #[serde(default)]
    #[allow(dead_code)]
    size: u64,
    #[serde(default)]
    data: Option<String>,
}

/// OAuth2 token response from Google.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    #[allow(dead_code)]
    token_type: String,
}

// ---------------------------------------------------------------------------
// Parsed email
// ---------------------------------------------------------------------------

/// A parsed Gmail message ready for processing.
#[derive(Debug, Clone)]
pub struct ParsedGmailMessage {
    /// Gmail message ID.
    pub id: String,
    /// Gmail thread ID.
    pub thread_id: String,
    /// Sender email address.
    pub from: String,
    /// Subject line.
    pub subject: String,
    /// Plain-text body.
    pub body: String,
    /// Message-ID header.
    pub message_id: String,
    /// When the message was received.
    #[allow(dead_code)]
    pub received_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// ChannelAdapter
// ---------------------------------------------------------------------------

/// Adapter that delivers NexiBot responses back via Gmail API.
pub struct GmailAdapter {
    config: GmailConfig,
    /// Access token for API calls.
    access_token: String,
    /// The original sender we are replying to.
    reply_to: String,
    /// Subject of the original message.
    original_subject: String,
    /// Gmail thread ID for threading the reply.
    thread_id: String,
    /// Message-ID header of the original message (for References header).
    original_message_id: String,
    /// Channel source tag.
    #[allow(dead_code)]
    source: ChannelSource,
}

impl GmailAdapter {
    pub fn new(
        config: &GmailConfig,
        access_token: &str,
        msg: &ParsedGmailMessage,
    ) -> Self {
        Self {
            config: config.clone(),
            access_token: access_token.to_string(),
            reply_to: msg.from.clone(),
            original_subject: msg.subject.clone(),
            thread_id: msg.thread_id.clone(),
            original_message_id: msg.message_id.clone(),
            source: ChannelSource::Gmail {
                thread_id: msg.thread_id.clone(),
            },
        }
    }
}

#[async_trait::async_trait]
impl ChannelAdapter for GmailAdapter {
    async fn send_response(&self, text: &str) -> Result<(), String> {
        let subject = if self.original_subject.to_lowercase().starts_with("re:") {
            self.original_subject.clone()
        } else {
            format!("Re: {}", self.original_subject)
        };

        info!(
            "[GMAIL] Sending reply to {} (subject: {})",
            self.reply_to, subject
        );

        send_gmail_reply(
            &self.access_token,
            &self.config.from_address,
            &self.reply_to,
            &subject,
            text,
            &self.thread_id,
            &self.original_message_id,
        )
        .await
        .map_err(|e| format!("Failed to send Gmail reply: {}", e))
    }

    async fn send_error(&self, error: &str) -> Result<(), String> {
        let subject = format!("Re: {} [Error]", self.original_subject);
        let body = format!(
            "An error occurred while processing your message:\n\n{}\n\nPlease try again.",
            error
        );

        send_gmail_reply(
            &self.access_token,
            &self.config.from_address,
            &self.reply_to,
            &subject,
            &body,
            &self.thread_id,
            &self.original_message_id,
        )
        .await
        .map_err(|e| format!("Failed to send Gmail error: {}", e))
    }

    fn source(&self) -> &ChannelSource {
        &self.source
    }
}

// ---------------------------------------------------------------------------
// GmailManager
// ---------------------------------------------------------------------------

/// Manages Gmail API polling, OAuth2 token refresh, and message sending.
pub struct GmailManager {
    config: GmailConfig,
    http_client: reqwest::Client,
    /// Cached OAuth2 access token.
    access_token: Option<String>,
    /// When the current access token expires.
    token_expires_at: Option<DateTime<Utc>>,
    /// Per-sender rate limiter.
    rate_limiter: RateLimiter,
    /// Recently-processed message IDs for deduplication.
    msg_dedup: LruCache<String, ()>,
    /// History ID for incremental polling (Gmail API optimization).
    #[allow(dead_code)]
    last_history_id: Option<String>,
}

impl GmailManager {
    pub fn new(config: GmailConfig) -> Self {
        Self {
            config,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            access_token: None,
            token_expires_at: None,
            rate_limiter: RateLimiter::new(RateLimitConfig {
                max_attempts: 10,
                window_seconds: 60,
                lockout_seconds: 30,
            }),
            msg_dedup: LruCache::new(NonZeroUsize::new(10_000).unwrap()),
            last_history_id: None,
        }
    }

    // -- OAuth2 token management ---------------------------------------------

    /// Get a valid access token, refreshing if expired or missing.
    pub async fn get_access_token(&mut self) -> Result<String> {
        // Return cached token if still valid (with 60s buffer)
        if let (Some(ref token), Some(expires)) = (&self.access_token, self.token_expires_at) {
            if Utc::now() < expires - chrono::Duration::seconds(60) {
                return Ok(token.clone());
            }
        }

        info!("[GMAIL] Refreshing OAuth2 access token");

        let resp = self
            .http_client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
                ("refresh_token", self.config.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .context("Failed to request Gmail OAuth2 token")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Gmail OAuth2 token refresh failed ({}): {}", status, body);
        }

        let token_resp: TokenResponse = resp
            .json()
            .await
            .context("Failed to parse Gmail OAuth2 token response")?;

        let expires_at = Utc::now() + chrono::Duration::seconds(token_resp.expires_in as i64);
        self.access_token = Some(token_resp.access_token.clone());
        self.token_expires_at = Some(expires_at);

        info!("[GMAIL] Access token refreshed, expires in {}s", token_resp.expires_in);
        Ok(token_resp.access_token)
    }

    // -- Polling loop --------------------------------------------------------

    /// Start the Gmail polling loop.
    pub async fn start_polling(&mut self, _state: AppState) -> Result<()> {
        if !self.config.enabled {
            info!("[GMAIL] Gmail channel is disabled, skipping polling");
            return Ok(());
        }

        if self.config.refresh_token.is_empty() {
            warn!("[GMAIL] No refresh_token configured, cannot start Gmail polling");
            return Ok(());
        }

        info!(
            "[GMAIL] Starting Gmail polling loop (interval: {}s, label: {})",
            self.config.poll_interval_seconds, self.config.label
        );

        loop {
            match self.poll_inbox().await {
                Ok(messages) => {
                    let access_token = match self.get_access_token().await {
                        Ok(t) => t,
                        Err(e) => {
                            warn!("[GMAIL] Token refresh failed during processing: {}", e);
                            tokio::time::sleep(tokio::time::Duration::from_secs(
                                self.config.poll_interval_seconds,
                            ))
                            .await;
                            continue;
                        }
                    };

                    for msg in messages {
                        // DM policy enforcement
                        match self.config.dm_policy {
                            crate::pairing::DmPolicy::Allowlist => {
                                if !self.config.allowed_senders.is_empty()
                                    && !self
                                        .config
                                        .allowed_senders
                                        .iter()
                                        .any(|s| s.eq_ignore_ascii_case(&msg.from))
                                {
                                    warn!("[GMAIL] Ignoring message from non-allowed sender: {}", msg.from);
                                    continue;
                                }
                            }
                            crate::pairing::DmPolicy::Open => {}
                            crate::pairing::DmPolicy::Pairing => {
                                if !self.config.allowed_senders.is_empty()
                                    && !self
                                        .config
                                        .allowed_senders
                                        .iter()
                                        .any(|s| s.eq_ignore_ascii_case(&msg.from))
                                {
                                    warn!("[GMAIL] Ignoring message from non-allowed sender (pairing not supported for Gmail): {}", msg.from);
                                    continue;
                                }
                            }
                        }

                        // Deduplication
                        if self.msg_dedup.put(msg.id.clone(), ()).is_some() {
                            debug!("[GMAIL] Duplicate message {}, skipping", msg.id);
                            continue;
                        }

                        // Per-sender rate limiting
                        {
                            let rate_key = format!("gmail:{}", msg.from);
                            if let Err(e) = self.rate_limiter.check(&rate_key) {
                                warn!("[GMAIL] Rate limit hit for {}: {}", msg.from, e);
                                continue;
                            }
                        }

                        info!(
                            "[GMAIL] New message: from={}, subject={}, thread={}",
                            msg.from, msg.subject, msg.thread_id
                        );

                        // Mark as read
                        if let Err(e) = self.mark_as_read(&access_token, &msg.id).await {
                            warn!("[GMAIL] Failed to mark message {} as read: {}", msg.id, e);
                        }

                        // Route message through NexiBot pipeline
                        let adapter = GmailAdapter::new(&self.config, &access_token, &msg);
                        let incoming = IncomingMessage {
                            text: msg.body.clone(),
                            channel: ChannelSource::Gmail {
                                thread_id: msg.thread_id.clone(),
                            },
                            agent_id: None,
                            metadata: HashMap::new(),
                        };

                        let observer = crate::tool_loop::NoOpObserver;
                        let result = {
                            let client_guard = _state.claude_client.read().await;
                            let options = RouteOptions {
                                claude_client: &*client_guard,
                                overrides: SessionOverrides::default(),
                                loop_config: ToolLoopConfig {
                                    max_iterations: 10,
                                    timeout: Some(std::time::Duration::from_secs(300)),
                                    max_output_bytes: 10 * 1024 * 1024,
                                    max_tool_result_bytes: Some(8_000),
                                    force_summary_on_exhaustion: true,
                                    channel: Some(ChannelSource::Gmail {
                                        thread_id: msg.thread_id.clone(),
                                    }),
                                    run_defense_checks: true,
                                    streaming: false,
                                    sender_id: Some(msg.from.clone()),
                                    between_tool_delay_ms: 0,
                                },
                                observer: &observer,
                                streaming: false,
                                window: None,
                                on_stream_chunk: None,
                                auto_compact: true,
                                save_to_memory: true,
                                sync_supermemory: true,
                                check_sensitive_data: true,
                            };
                            router::route_message(&incoming, options, &_state).await
                        };

                        match result {
                            Ok(routed) => {
                                let response_text = router::extract_text_from_response(&routed.text);
                                if let Err(e) = adapter.send_response(&response_text).await {
                                    warn!("[GMAIL] Failed to send reply to {}: {}", msg.from, e);
                                }
                            }
                            Err(e) => {
                                warn!("[GMAIL] Pipeline error for message from {}: {}", msg.from, e);
                                if let Err(send_err) = adapter.send_error(&e.to_string()).await {
                                    warn!("[GMAIL] Failed to send error reply: {}", send_err);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("[GMAIL] Inbox poll failed: {}", e);
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(
                self.config.poll_interval_seconds,
            ))
            .await;
        }
    }

    // -- Gmail API: list unread messages ------------------------------------

    /// Poll for unread messages in the configured label.
    async fn poll_inbox(&mut self) -> Result<Vec<ParsedGmailMessage>> {
        let access_token = self.get_access_token().await?;

        let query = format!("is:unread label:{}", self.config.label);

        let resp = self
            .http_client
            .get("https://gmail.googleapis.com/gmail/v1/users/me/messages")
            .query(&[
                ("q", query.as_str()),
                ("maxResults", &self.config.max_messages_per_poll.to_string()),
            ])
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await
            .context("Failed to list Gmail messages")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Gmail messages.list failed ({}): {}", status, body);
        }

        let list_resp: ListMessagesResponse = resp
            .json()
            .await
            .context("Failed to parse Gmail messages.list response")?;

        if list_resp.messages.is_empty() {
            return Ok(Vec::new());
        }

        debug!(
            "[GMAIL] Found {} unread message(s)",
            list_resp.messages.len()
        );

        let mut parsed = Vec::new();
        for msg_ref in &list_resp.messages {
            match self.get_message(&access_token, &msg_ref.id).await {
                Ok(msg) => parsed.push(msg),
                Err(e) => {
                    warn!("[GMAIL] Failed to fetch message {}: {}", msg_ref.id, e);
                }
            }
        }

        Ok(parsed)
    }

    // -- Gmail API: get full message ----------------------------------------

    /// Fetch and parse a single message by ID.
    async fn get_message(&self, access_token: &str, message_id: &str) -> Result<ParsedGmailMessage> {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}?format=full",
            message_id
        );

        let resp = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await
            .context("Failed to get Gmail message")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Gmail messages.get failed ({}): {}", status, body);
        }

        let gmail_msg: GmailMessage = resp
            .json()
            .await
            .context("Failed to parse Gmail message")?;

        let payload = gmail_msg
            .payload
            .as_ref()
            .context("Gmail message has no payload")?;

        // Extract headers
        let from = get_header(&payload.headers, "From").unwrap_or_default();
        let subject = get_header(&payload.headers, "Subject").unwrap_or_default();
        let msg_id = get_header(&payload.headers, "Message-ID").unwrap_or_default();

        // Extract plain-text body
        let body = extract_plain_text(payload);

        // Parse internal date (milliseconds since epoch)
        let received_at = gmail_msg
            .internal_date
            .as_ref()
            .and_then(|d| d.parse::<i64>().ok())
            .and_then(|ms| DateTime::from_timestamp_millis(ms))
            .unwrap_or_else(Utc::now);

        Ok(ParsedGmailMessage {
            id: gmail_msg.id,
            thread_id: gmail_msg.thread_id,
            from: extract_email_address(&from),
            subject,
            body,
            message_id: msg_id,
            received_at,
        })
    }

    // -- Gmail API: mark as read --------------------------------------------

    /// Mark a message as read by removing the UNREAD label.
    async fn mark_as_read(&self, access_token: &str, message_id: &str) -> Result<()> {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}/modify",
            message_id
        );

        let body = serde_json::json!({
            "removeLabelIds": ["UNREAD"]
        });

        let resp = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .json(&body)
            .send()
            .await
            .context("Failed to modify Gmail message")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Gmail messages.modify failed ({}): {}", status, err);
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Gmail API: send reply
// ---------------------------------------------------------------------------

/// Send a reply via the Gmail API `messages.send` endpoint.
async fn send_gmail_reply(
    access_token: &str,
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
    thread_id: &str,
    in_reply_to: &str,
) -> Result<()> {
    // Build RFC 2822 message
    let mut rfc2822 = String::new();
    rfc2822.push_str(&format!("From: {}\r\n", from));
    rfc2822.push_str(&format!("To: {}\r\n", to));
    rfc2822.push_str(&format!("Subject: {}\r\n", subject));
    rfc2822.push_str(&format!("In-Reply-To: {}\r\n", in_reply_to));
    rfc2822.push_str(&format!("References: {}\r\n", in_reply_to));
    rfc2822.push_str("Content-Type: text/plain; charset=UTF-8\r\n");
    rfc2822.push_str(&format!(
        "Message-ID: <{}.nexibot@gmail.com>\r\n",
        uuid::Uuid::new_v4()
    ));
    rfc2822.push_str("\r\n");
    rfc2822.push_str(body);

    // Base64url-encode the message (Gmail API requirement)
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rfc2822.as_bytes());

    let request_body = serde_json::json!({
        "raw": encoded,
        "threadId": thread_id,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post("https://gmail.googleapis.com/gmail/v1/users/me/messages/send")
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&request_body)
        .send()
        .await
        .context("Failed to send Gmail message")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let err = resp.text().await.unwrap_or_default();
        anyhow::bail!("Gmail messages.send failed ({}): {}", status, err);
    }

    info!("[GMAIL] Reply sent successfully to {}", to);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get a header value by name (case-insensitive).
fn get_header(headers: &[Header], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.clone())
}

/// Extract plain text body from a Gmail message payload.
/// Handles both single-part and multipart messages.
fn extract_plain_text(payload: &MessagePayload) -> String {
    // Single-part message
    if payload.parts.is_empty() {
        if payload
            .mime_type
            .as_deref()
            .map(|m| m.starts_with("text/plain"))
            .unwrap_or(false)
        {
            if let Some(ref body) = payload.body {
                if let Some(ref data) = body.data {
                    return decode_base64url(data);
                }
            }
        }
        return String::new();
    }

    // Multipart: find text/plain part recursively
    find_plain_text_part(&payload.parts)
}

/// Recursively find a text/plain part in a multipart message.
fn find_plain_text_part(parts: &[MessagePart]) -> String {
    for part in parts {
        if part
            .mime_type
            .as_deref()
            .map(|m| m.starts_with("text/plain"))
            .unwrap_or(false)
        {
            if let Some(ref body) = part.body {
                if let Some(ref data) = body.data {
                    return decode_base64url(data);
                }
            }
        }
        // Recurse into nested parts
        if !part.parts.is_empty() {
            let result = find_plain_text_part(&part.parts);
            if !result.is_empty() {
                return result;
            }
        }
    }
    String::new()
}

/// Decode a base64url-encoded string (Gmail API encoding).
fn decode_base64url(data: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(data)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

/// Extract just the email address from a "Name <email>" formatted string.
fn extract_email_address(from: &str) -> String {
    if let Some(start) = from.find('<') {
        if let Some(end) = from.find('>') {
            return from[start + 1..end].to_string();
        }
    }
    from.trim().to_string()
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// Start the Gmail channel polling loop. Returns immediately if disabled.
pub async fn start_gmail_polling(app_state: AppState) -> Result<()> {
    let config = {
        let cfg = app_state.config.read().await;
        cfg.gmail.clone()
    };

    if !config.enabled {
        info!("[GMAIL] Gmail channel disabled in config");
        return Ok(());
    }

    if config.refresh_token.is_empty() {
        warn!("[GMAIL] Gmail enabled but no refresh_token configured");
        return Ok(());
    }

    let mut manager = GmailManager::new(config);
    manager.start_polling(app_state).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_email_address() {
        assert_eq!(
            extract_email_address("John Doe <john@example.com>"),
            "john@example.com"
        );
        assert_eq!(
            extract_email_address("jane@example.com"),
            "jane@example.com"
        );
        assert_eq!(
            extract_email_address("<bot@example.com>"),
            "bot@example.com"
        );
    }

    #[test]
    fn test_decode_base64url() {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(b"Hello, World!");
        assert_eq!(decode_base64url(&encoded), "Hello, World!");
    }

    #[test]
    fn test_decode_base64url_empty() {
        assert_eq!(decode_base64url(""), "");
    }

    #[test]
    fn test_get_header() {
        let headers = vec![
            Header {
                name: "From".to_string(),
                value: "alice@example.com".to_string(),
            },
            Header {
                name: "Subject".to_string(),
                value: "Test".to_string(),
            },
        ];
        assert_eq!(
            get_header(&headers, "from"),
            Some("alice@example.com".to_string())
        );
        assert_eq!(get_header(&headers, "Subject"), Some("Test".to_string()));
        assert_eq!(get_header(&headers, "X-Missing"), None);
    }

    #[test]
    fn test_config_defaults() {
        let config = GmailConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.label, "INBOX");
        assert_eq!(config.poll_interval_seconds, 30);
        assert_eq!(config.max_messages_per_poll, 10);
        assert!(config.allowed_senders.is_empty());
    }

    #[test]
    fn test_extract_plain_text_single_part() {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(b"Hello from Gmail");
        let payload = MessagePayload {
            headers: vec![],
            parts: vec![],
            mime_type: Some("text/plain".to_string()),
            body: Some(MessageBody {
                size: 16,
                data: Some(encoded),
            }),
        };
        assert_eq!(extract_plain_text(&payload), "Hello from Gmail");
    }

    #[test]
    fn test_extract_plain_text_multipart() {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(b"Plain text content");
        let payload = MessagePayload {
            headers: vec![],
            parts: vec![
                MessagePart {
                    mime_type: Some("text/html".to_string()),
                    body: Some(MessageBody {
                        size: 20,
                        data: Some("aHRtbA".to_string()),
                    }),
                    parts: vec![],
                },
                MessagePart {
                    mime_type: Some("text/plain".to_string()),
                    body: Some(MessageBody {
                        size: 18,
                        data: Some(encoded),
                    }),
                    parts: vec![],
                },
            ],
            mime_type: Some("multipart/alternative".to_string()),
            body: None,
        };
        assert_eq!(extract_plain_text(&payload), "Plain text content");
    }
}
