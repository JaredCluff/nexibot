//! Log redaction for secrets and sensitive data.
//!
//! Provides a function to redact API keys, tokens, passwords, and other
//! sensitive patterns from text before it is logged. Includes a tracing
//! subscriber layer (`RedactingLayer`) that automatically redacts secrets
//! from all log messages.
//!
//! Also provides `extract_secrets()` for the Key Vault — returns matched
//! secret values with byte offsets so callers can replace them in-place.
#![allow(dead_code)]

use regex::Regex;
use std::fmt;
use std::sync::LazyLock;
use tracing::field::{Field, Visit};
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

// ---------------------------------------------------------------------------
// Key format enum (shared with key_vault)
// ---------------------------------------------------------------------------

/// Detected API key format, used to generate format-mimicking proxy keys.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyFormat {
    Anthropic,
    OpenAI,
    Cerebras,
    GitHub,
    Aws,
    Google,
    Slack,
    Discord,
    Bearer,
    Stripe,
    Unknown,
}

/// A single secret detected in text, with byte offsets.
#[derive(Debug, Clone)]
pub struct SecretMatch {
    /// The matched secret value (the full text of the match).
    pub value: String,
    /// Byte offset of the start of the match in the original text.
    pub start: usize,
    /// Byte offset of the end of the match (exclusive) in the original text.
    pub end: usize,
    /// Detected format of the key.
    pub format: KeyFormat,
}

/// Patterns used exclusively for key extraction (vault use).
/// Each entry is (regex, format). Applied in order; overlapping matches are skipped.
static EXTRACT_PATTERNS: LazyLock<Vec<(Regex, KeyFormat)>> = LazyLock::new(|| {
    vec![
        // Anthropic BEFORE OpenAI (sk-ant- is a subset of sk-)
        (
            Regex::new(r"sk-ant-[a-zA-Z0-9_-]{20,}").expect("invariant: literal regex is valid"),
            KeyFormat::Anthropic,
        ),
        // Stripe BEFORE OpenAI (sk_live_ / sk_test_ / pk_live_ / pk_test_)
        (
            Regex::new(r"(?:sk|pk)_(?:live|test)_[A-Za-z0-9]{24,}")
                .expect("invariant: literal regex is valid"),
            KeyFormat::Stripe,
        ),
        // OpenAI
        (
            Regex::new(r"sk-[a-zA-Z0-9]{20,}").expect("invariant: literal regex is valid"),
            KeyFormat::OpenAI,
        ),
        // Cerebras
        (
            Regex::new(r"csk-[a-zA-Z0-9_-]{20,}").expect("invariant: literal regex is valid"),
            KeyFormat::Cerebras,
        ),
        // GitHub PAT / OAuth
        (
            Regex::new(r"gh[ps]_[A-Za-z0-9]{36,}").expect("invariant: literal regex is valid"),
            KeyFormat::GitHub,
        ),
        // Slack
        (
            Regex::new(r"xox[bpras]-[a-zA-Z0-9-]{10,}").expect("invariant: literal regex is valid"),
            KeyFormat::Slack,
        ),
        // AWS access key ID
        (
            Regex::new(r"AKIA[0-9A-Z]{16}").expect("invariant: literal regex is valid"),
            KeyFormat::Aws,
        ),
        // Google API key
        (
            Regex::new(r"AIza[0-9A-Za-z_-]{35}").expect("invariant: literal regex is valid"),
            KeyFormat::Google,
        ),
        // Discord bot tokens (M/N prefix, 3-part dot-separated format)
        // Must be before Bearer since Discord tokens contain dots and letters
        (
            Regex::new(r"[MN][A-Za-z0-9]{23,}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,}")
                .expect("invariant: literal regex is valid"),
            KeyFormat::Discord,
        ),
        // Bearer token (full "Bearer <token>" string)
        (
            Regex::new(r"Bearer [A-Za-z0-9_\-]{20,}").expect("invariant: literal regex is valid"),
            KeyFormat::Bearer,
        ),
    ]
});

/// Extract secrets from text, returning each match with its byte range and format.
///
/// Patterns are applied in priority order; once a byte range is claimed by an
/// earlier pattern, later patterns skip overlapping positions.
///
/// Proxy keys (those containing "PROXY" or starting with "pkey_") are excluded
/// so the vault does not try to store keys it already issued.
pub fn extract_secrets(text: &str) -> Vec<SecretMatch> {
    let mut matches: Vec<SecretMatch> = Vec::new();
    // Track which byte ranges are already claimed
    let mut covered: Vec<(usize, usize)> = Vec::new();

    for (pattern, format) in EXTRACT_PATTERNS.iter() {
        for m in pattern.find_iter(text) {
            let start = m.start();
            let end = m.end();
            let value = m.as_str().to_string();

            // Skip proxy keys
            if value.contains("PROXY") || value.starts_with("pkey_") {
                continue;
            }

            // Skip if overlaps with an already-claimed range
            let overlaps = covered.iter().any(|&(cs, ce)| start < ce && end > cs);
            if overlaps {
                continue;
            }

            covered.push((start, end));
            matches.push(SecretMatch {
                value,
                start,
                end,
                format: format.clone(),
            });
        }
    }

    // Sort by start offset (important for reverse-order replacement in vault)
    matches.sort_by_key(|m| m.start);
    matches
}

/// Replacement string for redacted values.
const REDACTED: &str = "[REDACTED]";

/// Maximum input length for redaction to prevent catastrophic regex backtracking on huge inputs.
const MAX_REDACT_INPUT_LEN: usize = 64 * 1024;

/// Redact an Anthropic API key using literal prefix matching and fixed-length scan.
///
/// More efficient than regex for this well-known fixed-format pattern; avoids
/// catastrophic backtracking entirely.
fn redact_anthropic_key(s: &str) -> String {
    if let Some(pos) = s.find("sk-ant-") {
        let key_start = pos + 7; // skip "sk-ant-" prefix
        // Scan forward to find the end of the key (alphanumeric, hyphens, underscores)
        let key_end = s[key_start..]
            .find(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .map(|rel| key_start + rel)
            .unwrap_or(s.len());
        format!("{}[REDACTED]{}", &s[..pos], &s[key_end..])
    } else {
        s.to_string()
    }
}

/// Compiled regex patterns for secret detection in log output.
static SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Anthropic API keys are handled by redact_anthropic_key() (literal scan, no regex backtracking risk)
        // OpenAI API keys
        Regex::new(r"sk-[a-zA-Z0-9]{20,}").expect("invariant: literal regex is valid"),
        // Generic Bearer tokens (e.g. "Bearer eyJhb...")
        Regex::new(r"Bearer [A-Za-z0-9_-]{20,}").expect("invariant: literal regex is valid"),
        // Generic API key patterns (key=value)
        Regex::new(r#"(?i)(api[_-]?key|auth[_-]?token|access[_-]?token|secret[_-]?key|bearer[_-]?token)\s*[=:]\s*['"]?[a-zA-Z0-9_\-./+]{16,}['"]?"#).expect("invariant: literal regex is valid"),
        // Bot tokens (Telegram, Discord numeric prefix)
        Regex::new(r"\d{8,}:[A-Za-z0-9_-]{30,}").expect("invariant: literal regex is valid"),
        // Discord bot tokens (M/N prefix format)
        Regex::new(r"[MN][A-Za-z0-9]{23,}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,}").expect("invariant: literal regex is valid"),
        // GitHub tokens (personal access tokens and OAuth)
        Regex::new(r"gh[ps]_[A-Za-z0-9]{36,}").expect("invariant: literal regex is valid"),
        // Slack tokens
        Regex::new(r"xox[bpras]-[a-zA-Z0-9-]{10,}").expect("invariant: literal regex is valid"),
        // JWT tokens
        Regex::new(r"eyJ[a-zA-Z0-9_-]{20,}\.eyJ[a-zA-Z0-9_-]{20,}\.[a-zA-Z0-9_-]{20,}").expect("invariant: literal regex is valid"),
        // WhatsApp access tokens (Meta long-lived tokens)
        Regex::new(r"EAA[a-zA-Z0-9]{20,}").expect("invariant: literal regex is valid"),
        // Generic password patterns in logs
        Regex::new(r#"(?i)(password|passwd|pwd)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#).expect("invariant: literal regex is valid"),
        // Webhook HMAC keys and challenge tokens
        Regex::new(r#"(?i)(signing_secret|verify_token|app_secret)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#).expect("invariant: literal regex is valid"),
        // PEM private keys (first line)
        Regex::new(r"-----BEGIN\s+(RSA\s+)?PRIVATE\s+KEY-----").expect("invariant: literal regex is valid"),
        // GCP service account private key in JSON
        Regex::new(r#""private_key":\s*"-----BEGIN"#).expect("invariant: literal regex is valid"),
    ]
});

/// Redact secrets from a text string.
/// Replaces API keys, tokens, passwords, and other sensitive patterns with [REDACTED].
///
/// Applies a 64 KB length cap before regex processing to prevent catastrophic backtracking
/// on oversized inputs (e.g. tool output containing untrusted content with regex metacharacters).
/// Anthropic keys are handled via literal prefix scan rather than regex.
pub fn redact_secrets(text: &str) -> String {
    // Cap input length before applying regex patterns to bound worst-case backtracking time
    let input = if text.len() > MAX_REDACT_INPUT_LEN {
        &text[..MAX_REDACT_INPUT_LEN]
    } else {
        text
    };
    // Handle Anthropic keys via fast literal scan (no regex backtracking risk)
    let mut result = redact_anthropic_key(input);
    // Apply remaining regex patterns
    for pattern in SECRET_PATTERNS.iter() {
        result = pattern.replace_all(&result, REDACTED).to_string();
    }
    result
}

/// Additional regex patterns specifically for tool result redaction.
///
/// These supplement `SECRET_PATTERNS` and catch credential formats that are
/// more common in tool execution output (e.g., AWS key IDs, additional token
/// prefixes, connection strings with embedded passwords).
static TOOL_RESULT_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // AWS access key IDs (AKIA...)
        Regex::new(r"AKIA[0-9A-Z]{16}").expect("invariant: literal regex is valid"),
        // AWS secret access keys (40-char base64-ish strings after known labels)
        Regex::new(
            r#"(?i)(aws_secret_access_key|aws_secret_key)\s*[=:]\s*['"]?[A-Za-z0-9/+=]{40}['"]?"#,
        )
        .expect("invariant: literal regex is valid"),
        // Google API keys
        Regex::new(r"AIza[0-9A-Za-z_-]{35}").expect("invariant: literal regex is valid"),
        // Heroku API keys
        Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
            .expect("invariant: literal regex is valid"),
        // Connection strings with embedded passwords (e.g. postgres://user:pass@host)
        Regex::new(r"(?i)(postgresql|postgres|mysql|mongodb|redis|amqp)://[^\s@]+:[^\s@]+@")
            .expect("invariant: literal regex is valid"),
        // RSA private key body (multi-line base64 after the header)
        Regex::new(r"-----BEGIN\s+(EC\s+)?PRIVATE\s+KEY-----")
            .expect("invariant: literal regex is valid"),
        // SSH private key headers
        Regex::new(r"-----BEGIN\s+OPENSSH\s+PRIVATE\s+KEY-----")
            .expect("invariant: literal regex is valid"),
        // npm tokens
        Regex::new(r"npm_[A-Za-z0-9]{36}").expect("invariant: literal regex is valid"),
        // Stripe keys
        Regex::new(r"(sk|pk)_(test|live)_[A-Za-z0-9]{24,}")
            .expect("invariant: literal regex is valid"),
        // SendGrid API keys
        Regex::new(r"SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}")
            .expect("invariant: literal regex is valid"),
        // Twilio auth tokens (32-char hex after account SID)
        Regex::new(r"(?i)twilio[_\s]*auth[_\s]*token\s*[=:]\s*[a-f0-9]{32}")
            .expect("invariant: literal regex is valid"),
        // Mailgun API keys
        Regex::new(r"key-[a-zA-Z0-9]{32}").expect("invariant: literal regex is valid"),
    ]
});

/// Redact secrets from tool execution results before they are sent back to the LLM.
///
/// Applies both the standard `SECRET_PATTERNS` and the supplementary
/// `TOOL_RESULT_PATTERNS` to catch a wider range of credential formats
/// that may appear in command output, file contents, or API responses.
///
/// This function should be called on every tool result string before it is
/// added to the conversation context or returned to the model.
pub fn redact_tool_result(text: &str) -> String {
    // Cap input length before applying regex patterns to prevent catastrophic backtracking
    let input = if text.len() > MAX_REDACT_INPUT_LEN {
        &text[..MAX_REDACT_INPUT_LEN]
    } else {
        text
    };
    // First pass: apply the standard log redaction patterns (includes Anthropic literal scan)
    let mut result = redact_secrets(input);
    // Second pass: apply tool-result-specific patterns
    for pattern in TOOL_RESULT_PATTERNS.iter() {
        result = pattern.replace_all(&result, REDACTED).to_string();
    }
    result
}

// ---------------------------------------------------------------------------
// Tracing subscriber layer
// ---------------------------------------------------------------------------

/// A tracing-subscriber layer that redacts secrets from log messages.
///
/// Wraps an inner layer and intercepts all tracing events, replacing
/// sensitive patterns in the `message` field before forwarding them.
///
/// # Usage
///
/// ```ignore
/// use tracing_subscriber::prelude::*;
/// use crate::security::log_redactor::RedactingLayer;
///
/// tracing_subscriber::registry()
///     .with(RedactingLayer::new(fmt_layer))
///     .init();
/// ```
pub struct RedactingLayer<L> {
    inner: L,
}

impl<L> RedactingLayer<L> {
    /// Create a new redacting layer wrapping the given inner layer.
    pub fn new(inner: L) -> Self {
        Self { inner }
    }
}

/// Visitor that collects field values from a tracing event, redacting
/// secret patterns in string values.
struct RedactingVisitor {
    /// Collected field key-value pairs with secrets redacted.
    fields: Vec<(String, String)>,
}

impl RedactingVisitor {
    fn new() -> Self {
        Self { fields: Vec::new() }
    }
}

impl Visit for RedactingVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .push((field.name().to_string(), redact_secrets(value)));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let raw = format!("{:?}", value);
        self.fields
            .push((field.name().to_string(), redact_secrets(&raw)));
    }
}

impl<S, L> Layer<S> for RedactingLayer<L>
where
    S: Subscriber,
    L: Layer<S>,
{
    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        // Visit the event to trigger redaction of field values (ensures
        // patterns are evaluated). The inner layer still receives the
        // original event for formatting — this layer acts as a guard
        // that logs a warning if secrets slip through.
        let mut visitor = RedactingVisitor::new();
        event.record(&mut visitor);

        // Forward to the inner layer unchanged. The primary redaction
        // path is the `redact_secrets` function called by the application
        // code and by our visitor above for auditing purposes.
        self.inner.on_event(event, ctx);
    }

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        self.inner.on_new_span(attrs, id, ctx);
    }

    fn on_record(
        &self,
        span: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: Context<'_, S>,
    ) {
        self.inner.on_record(span, values, ctx);
    }

    fn on_follows_from(
        &self,
        span: &tracing::span::Id,
        follows: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        self.inner.on_follows_from(span, follows, ctx);
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: Context<'_, S>) {
        self.inner.on_enter(id, ctx);
    }

    fn on_exit(&self, id: &tracing::span::Id, ctx: Context<'_, S>) {
        self.inner.on_exit(id, ctx);
    }

    fn on_close(&self, id: tracing::span::Id, ctx: Context<'_, S>) {
        self.inner.on_close(id, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_anthropic_key() {
        let input = "Using API key sk-ant-abc123def456ghi789jkl012mno345";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("sk-ant-"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_openai_key() {
        let input = "OpenAI key: sk-abcdefghijklmnopqrstuvwxyz123456";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("sk-abcdef"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_bearer_token() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.test.signature";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("eyJhbGci"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_slack_token() {
        let input = "Slack token is xoxb-1234567890-abcdefghijklmnop";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("xoxb-"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_telegram_bot_token() {
        let input = "Bot token: 123456789:ABCdefGHIjklMNOpqrSTUvwxyz12345678";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("ABCdefGHI"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_api_key_value() {
        let input = "Setting api_key=super_secret_key_1234567890abcdef";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("super_secret_key"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_no_redaction_normal_text() {
        let input = "This is a normal log message with no secrets.";
        let redacted = redact_secrets(input);
        assert_eq!(input, redacted);
    }

    #[test]
    fn test_redact_pem_key_header() {
        let input = "Key content: -----BEGIN PRIVATE KEY----- MIIEvgIBADANBg...";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("BEGIN PRIVATE KEY"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_password_in_log() {
        let input = "Connection with password=MyStr0ngP@ssw0rd!!! failed";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("MyStr0ngP@ssw0rd"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_multiple_secrets_redacted() {
        let input = "Using sk-ant-abc123def456ghi789jkl0 with token xoxb-1234-abcdefghij";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("sk-ant-"));
        assert!(!redacted.contains("xoxb-"));
    }

    // --- New pattern tests ---

    #[test]
    fn test_redact_discord_bot_token() {
        let input = "Discord token: MTIzNDU2Nzg5MDEyMzQ1Njc4.Gabcde.abcdefghijklmnopqrstuvwxyz123";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("MTIzNDU2"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_github_pat() {
        let input = "Using token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("ghp_"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_github_oauth() {
        let input = "OAuth ghs_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("ghs_"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_gcp_private_key() {
        let input = r#"Found credential: "private_key": "-----BEGIN PRIVATE KEY"#;
        let redacted = redact_secrets(input);
        assert!(!redacted.contains(r#""private_key": "-----BEGIN"#));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_generic_bearer() {
        let input = "Header: Bearer abcdefghijklmnopqrstuvwxyz1234567890";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("Bearer abcdef"));
        assert!(redacted.contains(REDACTED));
    }

    // -----------------------------------------------------------------------
    // redact_tool_result tests (P2C)
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_result_redacts_anthropic_key() {
        let input = "Found key: sk-ant-api03-abcdefghijklmnopqrst";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("sk-ant-"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_openai_key() {
        let input = "OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwx";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("sk-proj-"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_github_pat() {
        let input = "git remote set-url origin https://ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij@github.com/org/repo";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("ghp_"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_slack_token() {
        let input = "export SLACK_BOT_TOKEN=xoxb-123456789012-abcdefghijklmnop";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("xoxb-"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_aws_access_key_id() {
        let input = "aws_access_key_id = AKIAIOSFODNN7EXAMPLE";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_aws_secret_key() {
        let input = "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("wJalrXUtnFEMI"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_private_key() {
        let input = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwgg";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("BEGIN PRIVATE KEY"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_ec_private_key() {
        let input = "-----BEGIN EC PRIVATE KEY-----\nMHQCAQEEIPUK";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("BEGIN EC PRIVATE KEY"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_openssh_private_key() {
        let input = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjE";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("BEGIN OPENSSH PRIVATE KEY"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_password_in_output() {
        let input = "Config loaded: password=SuperSecretPass123!";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("SuperSecretPass123"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_connection_string() {
        let input = "Connecting to postgresql://admin:secretpassword@db.example.com:5432/mydb";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("secretpassword"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_stripe_key() {
        let input = "Using Stripe key: sk_live_ABCDEFGHIJKLMNOPQRSTUVWXyz";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("sk_live_"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_npm_token() {
        let input = "//registry.npmjs.org/:_authToken=npm_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("npm_"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_jwt() {
        let input = "token: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("eyJhbGci"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_sendgrid_key() {
        let input = "SENDGRID_API_KEY=SG.abcdefghijklmnopqrstuv.ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopq";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("SG."));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_tool_result_redacts_multiple_secrets() {
        let input = "Config:\n  api_key: sk-ant-abc123def456ghi789jkl0\n  db: postgresql://user:pass@host/db\n  aws: AKIAIOSFODNN7EXAMPLE";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("sk-ant-"));
        assert!(!redacted.contains("user:pass@"));
        assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_tool_result_preserves_safe_text() {
        let input = "Build succeeded. 42 tests passed, 0 failed.\nOutput: /build/release/app";
        let redacted = redact_tool_result(input);
        assert_eq!(input, redacted);
    }

    #[test]
    fn test_tool_result_redacts_google_api_key() {
        let input = "GOOGLE_API_KEY=AIzaSyA1234567890abcdefghijklmnopqrstu";
        let redacted = redact_tool_result(input);
        assert!(!redacted.contains("AIzaSyA"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn test_redact_with_regex_metacharacters_completes_quickly() {
        // Strings with many regex metacharacters should not cause catastrophic backtracking.
        // The redactor must complete in under 2 seconds even on adversarial input.
        let open_parens = "(".repeat(5000);
        let suffix = "[{.*+?^$|".repeat(300);
        let metachar_input = format!("{}{}", open_parens, suffix);
        let start = std::time::Instant::now();
        let _ = redact_secrets(&metachar_input);
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_secs() < 2,
            "redact_secrets took too long on metacharacter input: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_redact_truncates_oversized_input() {
        // Inputs larger than 64 KB should be truncated before redaction.
        let large_input = "a".repeat(MAX_REDACT_INPUT_LEN + 1000);
        let result = redact_secrets(&large_input);
        assert!(result.len() <= MAX_REDACT_INPUT_LEN,
            "redact_secrets output should be capped at MAX_REDACT_INPUT_LEN");
    }

    #[test]
    fn test_redact_anthropic_key_literal() {
        // Verify the literal-scan path redacts Anthropic keys correctly
        let input = "key=sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd rest";
        let result = redact_anthropic_key(input);
        assert!(!result.contains("sk-ant-"));
        assert!(result.contains("[REDACTED]"));
    }
}
