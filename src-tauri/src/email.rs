//! Email channel integration.
//!
//! Polls an IMAP inbox for incoming messages and sends responses via SMTP.
//! Thread-based conversation tracking uses In-Reply-To and References headers.
//! Uses `rustls` for IMAP TLS connections and `lettre` for SMTP sending.

use anyhow::Result;
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

/// Configuration for the email channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    /// Whether the email channel is enabled.
    pub enabled: bool,

    // -- IMAP settings --
    /// IMAP server host (e.g. "imap.gmail.com").
    pub imap_host: String,
    /// IMAP server port (993 for TLS).
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    /// IMAP login username.
    pub imap_username: String,
    /// IMAP login password (or app-specific password).
    pub imap_password: String,

    // -- SMTP settings --
    /// SMTP server host (e.g. "smtp.gmail.com").
    pub smtp_host: String,
    /// SMTP server port (587 for STARTTLS).
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    /// SMTP login username.
    pub smtp_username: String,
    /// SMTP login password (or app-specific password).
    pub smtp_password: String,

    // -- Addressing --
    /// The "From" address for outgoing replies.
    pub from_address: String,
    /// Allow-list of sender addresses. Empty = accept all.
    #[serde(default)]
    pub allowed_senders: Vec<String>,

    // -- Polling --
    /// Seconds between IMAP inbox polls.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_seconds: u64,
    /// IMAP folder to poll.
    #[serde(default = "default_folder")]
    pub folder: String,

    /// DM authorization policy.
    #[serde(default)]
    pub dm_policy: crate::pairing::DmPolicy,

    /// Per-channel tool access policy.
    #[serde(default)]
    pub tool_policy: crate::config::ChannelToolPolicy,
}

fn default_imap_port() -> u16 {
    993
}
fn default_smtp_port() -> u16 {
    587
}
fn default_poll_interval() -> u64 {
    30
}
fn default_folder() -> String {
    "INBOX".to_string()
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            imap_host: String::new(),
            imap_port: default_imap_port(),
            imap_username: String::new(),
            imap_password: String::new(),
            smtp_host: String::new(),
            smtp_port: default_smtp_port(),
            smtp_username: String::new(),
            smtp_password: String::new(),
            from_address: String::new(),
            allowed_senders: Vec::new(),
            poll_interval_seconds: default_poll_interval(),
            folder: default_folder(),
            dm_policy: crate::pairing::DmPolicy::default(),
            tool_policy: crate::config::ChannelToolPolicy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Message & thread models
// ---------------------------------------------------------------------------

/// A single email message (incoming or outgoing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailMessage {
    /// RFC-2822 Message-ID.
    pub message_id: String,
    /// Sender address.
    pub from: String,
    /// Recipient address.
    pub to: String,
    /// Subject line.
    pub subject: String,
    /// Plain-text body.
    pub body: String,
    /// In-Reply-To header value (if this is a reply).
    pub in_reply_to: Option<String>,
    /// References header values (full thread chain).
    pub references: Vec<String>,
    /// When the message was received / created.
    pub received_at: DateTime<Utc>,
}

/// A conversation thread tracked by header references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailThread {
    /// Unique thread identifier (derived from the first Message-ID).
    pub thread_id: String,
    /// Normalised subject (without "Re:" prefixes).
    pub subject: String,
    /// Ordered list of messages in this thread.
    pub messages: Vec<EmailMessage>,
    /// Timestamp of the most recent message.
    pub last_activity: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// ChannelAdapter implementation
// ---------------------------------------------------------------------------

/// Adapter that delivers NexiBot responses back to an email sender.
pub struct EmailAdapter {
    config: EmailConfig,
    /// Pre-resolved "From" address for replies.
    #[allow(dead_code)]
    from_addr: String,
    /// The original sender we are replying to.
    reply_to: String,
    /// Subject of the original message (used for reply subject).
    original_subject: String,
    /// Message-ID of the original message (for In-Reply-To header).
    original_message_id: String,
    /// Channel source tag.
    #[allow(dead_code)]
    source: ChannelSource,
}

impl EmailAdapter {
    /// Create a new adapter that will reply to the given inbound message.
    pub fn new(config: &EmailConfig, inbound: &EmailMessage, thread_id: &str) -> Self {
        Self {
            config: config.clone(),
            from_addr: config.from_address.clone(),
            reply_to: inbound.from.clone(),
            original_subject: inbound.subject.clone(),
            original_message_id: inbound.message_id.clone(),
            source: ChannelSource::Email {
                thread_id: thread_id.to_string(),
            },
        }
    }
}

#[async_trait::async_trait]
impl ChannelAdapter for EmailAdapter {
    async fn send_response(&self, text: &str) -> Result<(), String> {
        let reply =
            EmailManager::format_reply(&self.original_subject, text, &self.original_message_id);
        info!(
            "[EMAIL] Sending reply to {} (subject: {})",
            self.reply_to, reply.subject
        );

        // Stub: log what would be sent. Real implementation uses `send_email`.
        let manager = EmailManager::new(self.config.clone());
        manager
            .send_email(
                &self.reply_to,
                &reply.subject,
                &reply.body,
                Some(&self.original_message_id),
            )
            .await
            .map_err(|e| format!("Failed to send email reply: {}", e))
    }

    async fn send_error(&self, error: &str) -> Result<(), String> {
        let subject = format!("Re: {} [Error]", self.original_subject);
        let body = format!(
            "An error occurred while processing your message:\n\n{}\n\nPlease try again.",
            error
        );
        info!(
            "[EMAIL] Sending error notification to {} (subject: {})",
            self.reply_to, subject
        );

        let manager = EmailManager::new(self.config.clone());
        manager
            .send_email(
                &self.reply_to,
                &subject,
                &body,
                Some(&self.original_message_id),
            )
            .await
            .map_err(|e| format!("Failed to send error email: {}", e))
    }

    fn source(&self) -> &ChannelSource {
        &self.source
    }
}

// ---------------------------------------------------------------------------
// EmailManager -- orchestrates polling, threading, and sending
// ---------------------------------------------------------------------------

/// Manages IMAP polling, thread tracking, and SMTP sending.
pub struct EmailManager {
    config: EmailConfig,
    /// Active conversation threads keyed by thread_id.
    threads: HashMap<String, EmailThread>,
    /// Timestamp of the last successful inbox check.
    last_check: Option<DateTime<Utc>>,
    /// Per-sender rate limiter (10 messages per 60 seconds, 30-second lockout)
    rate_limiter: RateLimiter,
    /// Recently-processed message IDs for deduplication
    msg_dedup: LruCache<String, ()>,
}

impl EmailManager {
    /// Create a new email manager with the given configuration.
    pub fn new(config: EmailConfig) -> Self {
        Self {
            config,
            threads: HashMap::new(),
            last_check: None,
            rate_limiter: RateLimiter::new(RateLimitConfig {
                max_attempts: 10,
                window_seconds: 60,
                lockout_seconds: 30,
            }),
            msg_dedup: LruCache::new(NonZeroUsize::new(10_000).unwrap()),
        }
    }

    // -- Polling loop -------------------------------------------------------

    /// Start the IMAP polling loop. Runs indefinitely, checking for new
    /// messages every `poll_interval_seconds` and routing them through the
    /// NexiBot pipeline.
    pub async fn start_polling(&mut self, _state: AppState) -> Result<()> {
        if !self.config.enabled {
            info!("[EMAIL] Email channel is disabled, skipping polling");
            return Ok(());
        }

        info!(
            "[EMAIL] Starting email polling loop (interval: {}s, folder: {})",
            self.config.poll_interval_seconds, self.config.folder
        );

        loop {
            match self.check_inbox().await {
                Ok(messages) => {
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
                                    warn!(
                                        "[EMAIL] Ignoring message from non-allowed sender: {}",
                                        msg.from
                                    );
                                    continue;
                                }
                            }
                            crate::pairing::DmPolicy::Open => {}
                            crate::pairing::DmPolicy::Pairing => {
                                // Email does not support interactive pairing;
                                // fall back to allowlist behavior
                                if !self.config.allowed_senders.is_empty()
                                    && !self
                                        .config
                                        .allowed_senders
                                        .iter()
                                        .any(|s| s.eq_ignore_ascii_case(&msg.from))
                                {
                                    warn!(
                                        "[EMAIL] Ignoring message from non-allowed sender (pairing not supported for email): {}",
                                        msg.from
                                    );
                                    continue;
                                }
                            }
                        }

                        // Message deduplication using message_id
                        if !msg.message_id.is_empty() {
                            if self.msg_dedup.put(msg.message_id.clone(), ()).is_some() {
                                debug!("[EMAIL] Duplicate message {}, skipping", msg.message_id);
                                continue;
                            }
                        }

                        // Per-sender rate limiting
                        {
                            let rate_key = format!("email:{}", msg.from);
                            if let Err(e) = self.rate_limiter.check(&rate_key) {
                                warn!("[EMAIL] Rate limit hit for user {}: {}", msg.from, e);
                                continue;
                            }
                        }

                        let thread_id = self.find_or_create_thread(&msg);
                        info!(
                            "[EMAIL] New message in thread {}: from={}, subject={}",
                            thread_id, msg.from, msg.subject
                        );

                        // Route through the NexiBot pipeline
                        let incoming = IncomingMessage {
                            text: msg.body.clone(),
                            channel: ChannelSource::Email {
                                thread_id: thread_id.clone(),
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
                                loop_config: ToolLoopConfig::email(thread_id.clone()),
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
                                let reply_text =
                                    router::extract_text_from_response(&routed.text);
                                if !reply_text.is_empty() {
                                    let adapter =
                                        EmailAdapter::new(&self.config, &msg, &thread_id);
                                    if let Err(e) = adapter.send_response(&reply_text).await {
                                        warn!(
                                            "[EMAIL] Failed to send reply to {}: {}",
                                            msg.from, e
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "[EMAIL] Pipeline routing failed for message from {}: {}",
                                    msg.from, e
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("[EMAIL] Inbox check failed: {}", e);
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(
                self.config.poll_interval_seconds,
            ))
            .await;
        }
    }

    // -- IMAP inbox check ---------------------------------------------------

    /// Check the IMAP inbox for new (unseen) messages.
    ///
    /// Connects via TLS to the configured IMAP server, searches for UNSEEN
    /// messages, fetches and parses them, marks them as SEEN, then disconnects.
    pub async fn check_inbox(&mut self) -> Result<Vec<EmailMessage>> {
        if self.config.imap_host.is_empty() {
            return Ok(Vec::new());
        }

        let host = self.config.imap_host.clone();
        let port = self.config.imap_port;
        let username = self.config.imap_username.clone();
        let password = self.config.imap_password.clone();
        let folder = self.config.folder.clone();

        // Run IMAP operations in a blocking task since async-imap uses its own
        // async runtime but the TLS handshake + IMAP protocol are I/O-bound.
        let messages = tokio::task::spawn_blocking(move || -> Result<Vec<EmailMessage>> {
            use std::net::TcpStream;

            // Build a rustls TLS connector
            let mut root_store = rustls::RootCertStore::empty();
            root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let tls_config = std::sync::Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth(),
            );

            let server_name: rustls_pki_types::ServerName<'_> = host
                .as_str()
                .try_into()
                .map_err(|e| anyhow::anyhow!("Invalid IMAP hostname '{}': {}", host, e))?;

            let tcp = TcpStream::connect(format!("{}:{}", host, port))
                .map_err(|e| anyhow::anyhow!("IMAP TCP connect to {}:{} failed: {}", host, port, e))?;

            let tls_conn = rustls::ClientConnection::new(tls_config, server_name.to_owned())
                .map_err(|e| anyhow::anyhow!("IMAP TLS handshake failed: {}", e))?;
            let mut tls_stream = rustls::StreamOwned::new(tls_conn, tcp);

            // Read IMAP greeting
            let mut buf = vec![0u8; 4096];
            let _ = std::io::Read::read(&mut tls_stream, &mut buf)?;

            // Helper to send an IMAP command and read the response
            let tag_counter = std::cell::Cell::new(1u32);
            let mut send_cmd = |cmd: &str| -> Result<String> {
                let tag = format!("A{:04}", tag_counter.get());
                tag_counter.set(tag_counter.get() + 1);
                let line = format!("{} {}\r\n", tag, cmd);
                std::io::Write::write_all(&mut tls_stream, line.as_bytes())?;
                std::io::Write::flush(&mut tls_stream)?;

                let mut response = String::new();
                let mut read_buf = [0u8; 8192];
                loop {
                    let n = std::io::Read::read(&mut tls_stream, &mut read_buf)?;
                    if n == 0 {
                        break;
                    }
                    response.push_str(&String::from_utf8_lossy(&read_buf[..n]));
                    if response.contains(&format!("{} OK", tag))
                        || response.contains(&format!("{} NO", tag))
                        || response.contains(&format!("{} BAD", tag))
                    {
                        break;
                    }
                }
                if response.contains(&format!("{} NO", tag))
                    || response.contains(&format!("{} BAD", tag))
                {
                    anyhow::bail!("IMAP command '{}' failed: {}", cmd, response.trim());
                }
                Ok(response)
            };

            // Reject credentials that contain CRLF or LF — IMAP is line-oriented and
            // quote-escaping cannot prevent injection if the literal bytes \r or \n appear
            // in the value (they would terminate the current command line and allow the
            // attacker to inject arbitrary IMAP commands into the stream).
            if username.contains('\r') || username.contains('\n') {
                anyhow::bail!("IMAP username must not contain line endings");
            }
            if password.contains('\r') || password.contains('\n') {
                anyhow::bail!("IMAP password must not contain line endings");
            }

            // LOGIN
            send_cmd(&format!(
                "LOGIN \"{}\" \"{}\"",
                username.replace('\\', "\\\\").replace('"', "\\\""),
                password.replace('\\', "\\\\").replace('"', "\\\""),
            ))?;

            // SELECT folder
            send_cmd(&format!("SELECT \"{}\"", folder.replace('\\', "\\\\").replace('"', "\\\"")))?;

            // SEARCH for UNSEEN messages
            let search_resp = send_cmd("SEARCH UNSEEN")?;
            let uids: Vec<u32> = search_resp
                .lines()
                .filter(|line| line.starts_with("* SEARCH"))
                .flat_map(|line| {
                    line.strip_prefix("* SEARCH")
                        .unwrap_or("")
                        .split_whitespace()
                        .filter_map(|s| s.parse::<u32>().ok())
                })
                .collect();

            let mut messages = Vec::new();
            for uid in &uids {
                // FETCH the full RFC822 message
                let fetch_resp = send_cmd(&format!("FETCH {} RFC822", uid))?;

                // Extract the raw message body between the first { and the closing )
                if let Some(start) = fetch_resp.find("}\r\n") {
                    let body_start = start + 3;
                    // Find the end — look for the closing line with the tag
                    let body_bytes = fetch_resp[body_start..].as_bytes();
                    if let Ok(parsed) = mailparse::parse_mail(body_bytes) {
                        let get_header = |name: &str| -> String {
                            parsed
                                .headers
                                .iter()
                                .find(|h| h.get_key().eq_ignore_ascii_case(name))
                                .map(|h| h.get_value())
                                .unwrap_or_default()
                        };

                        let body = extract_plain_text_from_parsed(&parsed);
                        let from = get_header("From");
                        let from_addr = extract_email_addr(&from);

                        messages.push(EmailMessage {
                            message_id: get_header("Message-ID"),
                            from: from_addr,
                            to: get_header("To"),
                            subject: get_header("Subject"),
                            body,
                            in_reply_to: {
                                let v = get_header("In-Reply-To");
                                if v.is_empty() { None } else { Some(v) }
                            },
                            references: get_header("References")
                                .split_whitespace()
                                .map(|s| s.to_string())
                                .collect(),
                            received_at: Utc::now(),
                        });
                    }
                }

                // Mark as SEEN
                let _ = send_cmd(&format!("STORE {} +FLAGS (\\Seen)", uid));
            }

            // LOGOUT
            let _ = send_cmd("LOGOUT");

            Ok(messages)
        })
        .await
        .map_err(|e| anyhow::anyhow!("IMAP task panicked: {}", e))??;

        info!(
            "[EMAIL] Fetched {} new message(s) from {}",
            messages.len(),
            self.config.imap_host
        );
        self.last_check = Some(Utc::now());
        Ok(messages)
    }

    // -- SMTP sending -------------------------------------------------------

    /// Send an email via SMTP using STARTTLS.
    pub async fn send_email(
        &self,
        to: &str,
        subject: &str,
        body: &str,
        in_reply_to: Option<&str>,
    ) -> Result<()> {
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

        if self.config.smtp_host.is_empty() {
            anyhow::bail!("SMTP host is not configured");
        }

        let from_mailbox: lettre::message::Mailbox = self
            .config
            .from_address
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid from_address '{}': {}", self.config.from_address, e))?;

        let to_mailbox: lettre::message::Mailbox = to
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid to address '{}': {}", to, e))?;

        let mut builder = Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject(subject);

        if let Some(reply_id) = in_reply_to {
            builder = builder.in_reply_to(reply_id.to_string());
        }

        let email = builder
            .body(body.to_string())
            .map_err(|e| anyhow::anyhow!("Failed to build email: {}", e))?;

        let creds = Credentials::new(
            self.config.smtp_username.clone(),
            self.config.smtp_password.clone(),
        );

        let mailer = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.smtp_host)
            .map_err(|e| anyhow::anyhow!("SMTP relay setup failed for '{}': {}", self.config.smtp_host, e))?
            .port(self.config.smtp_port)
            .credentials(creds)
            .build();

        mailer
            .send(email)
            .await
            .map_err(|e| anyhow::anyhow!("SMTP send failed: {}", e))?;

        info!(
            "[EMAIL] Email sent to {} (subject: {})",
            to, subject
        );
        Ok(())
    }

    // -- Thread management --------------------------------------------------

    /// Find an existing thread for the given message, or create a new one.
    ///
    /// Matching strategy (in order of priority):
    /// 1. In-Reply-To header matches a known Message-ID in any thread
    /// 2. References header contains a known Message-ID
    /// 3. Normalised subject matches an existing thread
    /// 4. No match found -- create a new thread
    pub fn find_or_create_thread(&mut self, message: &EmailMessage) -> String {
        // Strategy 1: Match by In-Reply-To header
        if let Some(ref reply_to) = message.in_reply_to {
            for (tid, thread) in &self.threads {
                if thread.messages.iter().any(|m| m.message_id == *reply_to) {
                    let tid = tid.clone();
                    // Add message to existing thread
                    if let Some(thread) = self.threads.get_mut(&tid) {
                        thread.last_activity = message.received_at;
                        thread.messages.push(message.clone());
                    }
                    return tid;
                }
            }
        }

        // Strategy 2: Match by References header
        for ref_id in &message.references {
            for (tid, thread) in &self.threads {
                if thread.messages.iter().any(|m| m.message_id == *ref_id) {
                    let tid = tid.clone();
                    if let Some(thread) = self.threads.get_mut(&tid) {
                        thread.last_activity = message.received_at;
                        thread.messages.push(message.clone());
                    }
                    return tid;
                }
            }
        }

        // Strategy 3: Match by normalised subject
        let normalised = normalise_subject(&message.subject);
        for (tid, thread) in &self.threads {
            if thread.subject == normalised {
                let tid = tid.clone();
                if let Some(thread) = self.threads.get_mut(&tid) {
                    thread.last_activity = message.received_at;
                    thread.messages.push(message.clone());
                }
                return tid;
            }
        }

        // Strategy 4: Create a new thread
        let thread_id = message.message_id.clone();
        let thread = EmailThread {
            thread_id: thread_id.clone(),
            subject: normalised,
            messages: vec![message.clone()],
            last_activity: message.received_at,
        };
        self.threads.insert(thread_id.clone(), thread);
        thread_id
    }

    // -- Reply formatting ---------------------------------------------------

    /// Format a reply email message with proper headers.
    pub fn format_reply(original_subject: &str, body: &str, in_reply_to: &str) -> EmailMessage {
        let subject = if original_subject.to_lowercase().starts_with("re:") {
            original_subject.to_string()
        } else {
            format!("Re: {}", original_subject)
        };

        EmailMessage {
            message_id: format!("<{}.nexibot@localhost>", uuid::Uuid::new_v4()),
            from: String::new(), // Filled in by the adapter / send_email
            to: String::new(),   // Filled in by the adapter / send_email
            subject,
            body: body.to_string(),
            in_reply_to: Some(in_reply_to.to_string()),
            references: vec![in_reply_to.to_string()],
            received_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the plain text body from a parsed email (single-part or multipart).
fn extract_plain_text_from_parsed(parsed: &mailparse::ParsedMail) -> String {
    if parsed.subparts.is_empty() {
        // Single-part message
        parsed.get_body().unwrap_or_default()
    } else {
        // Multipart — prefer text/plain
        for part in &parsed.subparts {
            if let Some(ct) = part.headers.iter().find(|h| h.get_key().eq_ignore_ascii_case("Content-Type")) {
                if ct.get_value().to_lowercase().contains("text/plain") {
                    return part.get_body().unwrap_or_default();
                }
            }
            // Recurse into nested multipart
            if !part.subparts.is_empty() {
                let nested = extract_plain_text_from_parsed(part);
                if !nested.is_empty() {
                    return nested;
                }
            }
        }
        // Fallback: first part body
        parsed.subparts.first().and_then(|p| p.get_body().ok()).unwrap_or_default()
    }
}

/// Extract a bare email address from a "Name <addr>" or plain "addr" string.
fn extract_email_addr(raw: &str) -> String {
    if let Some(start) = raw.find('<') {
        if let Some(end) = raw.find('>') {
            return raw[start + 1..end].trim().to_string();
        }
    }
    raw.trim().to_string()
}

/// Strip "Re:", "RE:", "re:", "Fwd:", etc. prefixes and normalise whitespace.
fn normalise_subject(subject: &str) -> String {
    let mut s = subject.trim().to_string();
    loop {
        let lower = s.to_lowercase();
        if lower.starts_with("re:") {
            s = s[3..].trim_start().to_string();
        } else if lower.starts_with("fwd:") {
            s = s[4..].trim_start().to_string();
        } else if lower.starts_with("fw:") {
            s = s[3..].trim_start().to_string();
        } else {
            break;
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// Start the email channel polling loop. Returns immediately if disabled.
pub async fn start_email_polling(app_state: AppState) -> Result<()> {
    let config = {
        let cfg = app_state.config.read().await;
        cfg.email.clone()
    };

    if !config.enabled {
        info!("[EMAIL] Email channel disabled in config");
        return Ok(());
    }

    if config.imap_host.is_empty() {
        warn!("[EMAIL] Email enabled but no IMAP host configured");
        return Ok(());
    }

    let mut manager = EmailManager::new(config);
    manager.start_polling(app_state).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_message(
        id: &str,
        from: &str,
        subject: &str,
        in_reply_to: Option<&str>,
        references: Vec<&str>,
    ) -> EmailMessage {
        EmailMessage {
            message_id: id.to_string(),
            from: from.to_string(),
            to: "bot@example.com".to_string(),
            subject: subject.to_string(),
            body: "test body".to_string(),
            in_reply_to: in_reply_to.map(|s| s.to_string()),
            references: references.iter().map(|s| s.to_string()).collect(),
            received_at: Utc::now(),
        }
    }

    #[test]
    fn test_thread_matching_by_in_reply_to() {
        let mut manager = EmailManager::new(EmailConfig::default());

        // First message creates a new thread
        let msg1 = make_message(
            "<msg1@example.com>",
            "alice@example.com",
            "Hello",
            None,
            vec![],
        );
        let tid1 = manager.find_or_create_thread(&msg1);
        assert_eq!(tid1, "<msg1@example.com>");

        // Reply matches by In-Reply-To
        let msg2 = make_message(
            "<msg2@example.com>",
            "bob@example.com",
            "Re: Hello",
            Some("<msg1@example.com>"),
            vec!["<msg1@example.com>"],
        );
        let tid2 = manager.find_or_create_thread(&msg2);
        assert_eq!(tid2, tid1);
        assert_eq!(manager.threads[&tid1].messages.len(), 2);
    }

    #[test]
    fn test_thread_matching_by_references() {
        let mut manager = EmailManager::new(EmailConfig::default());

        let msg1 = make_message(
            "<msg1@example.com>",
            "alice@example.com",
            "Topic",
            None,
            vec![],
        );
        let tid1 = manager.find_or_create_thread(&msg1);

        // Third-party reply that only has References (no In-Reply-To)
        let msg2 = make_message(
            "<msg3@example.com>",
            "charlie@example.com",
            "Re: Topic",
            None,
            vec!["<msg1@example.com>"],
        );
        let tid2 = manager.find_or_create_thread(&msg2);
        assert_eq!(tid2, tid1);
    }

    #[test]
    fn test_thread_matching_by_subject() {
        let mut manager = EmailManager::new(EmailConfig::default());

        let msg1 = make_message(
            "<msg1@example.com>",
            "alice@example.com",
            "Bug report",
            None,
            vec![],
        );
        let tid1 = manager.find_or_create_thread(&msg1);

        // Different Message-ID, no In-Reply-To/References, but same subject
        let msg2 = make_message(
            "<msg99@other.com>",
            "dave@example.com",
            "Re: Bug report",
            None,
            vec![],
        );
        let tid2 = manager.find_or_create_thread(&msg2);
        assert_eq!(tid2, tid1);
    }

    #[test]
    fn test_new_thread_for_unrelated_message() {
        let mut manager = EmailManager::new(EmailConfig::default());

        let msg1 = make_message(
            "<msg1@example.com>",
            "alice@example.com",
            "Hello",
            None,
            vec![],
        );
        let tid1 = manager.find_or_create_thread(&msg1);

        let msg2 = make_message(
            "<msg2@example.com>",
            "bob@example.com",
            "Different topic",
            None,
            vec![],
        );
        let tid2 = manager.find_or_create_thread(&msg2);

        assert_ne!(tid1, tid2);
        assert_eq!(manager.threads.len(), 2);
    }

    #[test]
    fn test_format_reply_adds_re_prefix() {
        let reply = EmailManager::format_reply("Hello", "Thanks!", "<orig@example.com>");
        assert_eq!(reply.subject, "Re: Hello");
        assert_eq!(reply.in_reply_to, Some("<orig@example.com>".to_string()));
        assert_eq!(reply.references, vec!["<orig@example.com>".to_string()]);
    }

    #[test]
    fn test_format_reply_does_not_double_re() {
        let reply = EmailManager::format_reply("Re: Hello", "Thanks!", "<orig@example.com>");
        assert_eq!(reply.subject, "Re: Hello");
    }

    #[test]
    fn test_normalise_subject() {
        assert_eq!(normalise_subject("Re: Hello"), "Hello");
        assert_eq!(normalise_subject("RE: RE: Hello"), "Hello");
        assert_eq!(normalise_subject("Fwd: Re: Topic"), "Topic");
        assert_eq!(normalise_subject("fw: Something"), "Something");
        assert_eq!(normalise_subject("Plain subject"), "Plain subject");
    }

    #[test]
    fn test_email_config_defaults() {
        let config = EmailConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.imap_port, 993);
        assert_eq!(config.smtp_port, 587);
        assert_eq!(config.poll_interval_seconds, 30);
        assert_eq!(config.folder, "INBOX");
        assert!(config.allowed_senders.is_empty());
    }
}
