//! Tool error classification and retry backoff for the tool-use loop.
//!
//! Transient errors (network, rate-limit, server 5xx, timeout) are
//! automatically retried with exponential back-off.  Permanent errors
//! (auth failure, explicit block) are not retried.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Broad category of tool error — drives retry behaviour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorKind {
    /// Request timed out before a response was received.
    Timeout,
    /// Provider rate-limit (HTTP 429). May carry a retry-after header.
    RateLimit,
    /// Authentication failure (HTTP 401/403, invalid API key).  No retry.
    AuthFailed,
    /// Low-level network error (connection refused, DNS failure, reset).
    NetworkError,
    /// Remote server error (HTTP 5xx).
    ServerError,
    /// Catch-all for errors that don't fit the above categories.
    Other,
}

/// Rich description of a single tool error emitted to the observer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolErrorInfo {
    /// Broad error category.
    pub kind: ToolErrorKind,
    /// Plain-English description shown to the user.
    pub message: String,
    /// Suggested seconds to wait before retrying (from Retry-After header or
    /// the backoff table).  Zero means "retry immediately".
    pub retry_after_secs: u64,
    /// 1-based attempt number (1 = first failure, 2 = second, …).
    pub attempt: u32,
    /// Maximum attempts for this error kind (may be 1 for non-retryable).
    pub max_attempts: u32,
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Classify a raw error string into a [`ToolErrorKind`].
///
/// The string may come from `reqwest`, `anyhow`, or any tool returning
/// `"Error: …"`.  Matching is intentionally broad and case-insensitive.
pub fn classify_error(err: &str) -> ToolErrorKind {
    let lower = err.to_lowercase();

    // Auth failures — never retry
    if lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("invalid api key")
        || lower.contains("invalid_api_key")
        || lower.contains("authentication_error")
        || lower.contains("auth failed")
        || lower.contains("api key")
    {
        return ToolErrorKind::AuthFailed;
    }

    // Rate limit
    if lower.contains("429") || lower.contains("rate limit") || lower.contains("rate_limit") || lower.contains("too many requests") {
        return ToolErrorKind::RateLimit;
    }

    // Server errors
    if lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("504")
        || lower.contains("internal server error")
        || lower.contains("bad gateway")
        || lower.contains("service unavailable")
        || lower.contains("gateway timeout")
    {
        return ToolErrorKind::ServerError;
    }

    // Network errors
    if lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("network")
        || lower.contains("dns")
        || lower.contains("no route to host")
        || lower.contains("host unreachable")
        || lower.contains("broken pipe")
        || lower.contains("eof")
    {
        return ToolErrorKind::NetworkError;
    }

    // Timeout
    if lower.contains("timeout") || lower.contains("timed out") || lower.contains("elapsed") {
        return ToolErrorKind::Timeout;
    }

    ToolErrorKind::Other
}

/// Maximum retry attempts for each error kind.
/// AuthFailed and Other return 1 (no retry — the first attempt is the last).
pub fn max_attempts(kind: &ToolErrorKind) -> u32 {
    match kind {
        ToolErrorKind::Timeout => 3,
        ToolErrorKind::RateLimit => 3,
        ToolErrorKind::AuthFailed => 1, // no retry
        ToolErrorKind::NetworkError => 3,
        ToolErrorKind::ServerError => 2,
        ToolErrorKind::Other => 1, // no retry by default
    }
}

/// Seconds to wait before the given retry attempt.
///
/// `attempt` is the attempt about to be made (2 = first retry, 3 = second, …).
/// For `RateLimit`, `provider_retry_after` overrides the table.
pub fn backoff_secs(attempt: u32, kind: &ToolErrorKind, provider_retry_after: Option<u64>) -> u64 {
    if let Some(secs) = provider_retry_after {
        if *kind == ToolErrorKind::RateLimit {
            return secs.min(120); // cap at 2 min
        }
    }
    match kind {
        // attempt 2 → 0s, attempt 3 → 5s (immediate, then short wait)
        ToolErrorKind::Timeout | ToolErrorKind::NetworkError => match attempt {
            2 => 0,
            3 => 5,
            _ => 30,
        },
        ToolErrorKind::RateLimit => match attempt {
            2 => 5,
            3 => 30,
            _ => 60,
        },
        ToolErrorKind::ServerError => match attempt {
            2 => 5,
            _ => 30,
        },
        ToolErrorKind::AuthFailed | ToolErrorKind::Other => 0,
    }
}

/// Parse a `Retry-After` value from a provider error string.
/// Looks for patterns like "retry after 30s", "retry-after: 60", "wait 45 seconds".
pub fn parse_retry_after(err: &str) -> Option<u64> {
    // "Retry-After: 30"
    let lower = err.to_lowercase();
    for pattern in &["retry-after:", "retry after", "wait", "retry in"] {
        if let Some(pos) = lower.find(pattern) {
            let rest = &lower[pos + pattern.len()..].trim_start();
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u64>() {
                return Some(n);
            }
        }
    }
    None
}

/// Return a plain-English message for an error that will be shown in the UI.
pub fn plain_english_message(kind: &ToolErrorKind, raw: &str, attempt: u32, max: u32) -> String {
    let suffix = if max > 1 && attempt < max {
        format!(" (attempt {}/{})", attempt, max)
    } else {
        String::new()
    };
    match kind {
        ToolErrorKind::Timeout => format!("Request timed out{suffix}. Retrying…"),
        ToolErrorKind::RateLimit => format!("Rate limit reached{suffix}. Waiting before retry…"),
        ToolErrorKind::AuthFailed => format!(
            "Authentication failed. Check your API key or re-connect in Settings. ({})",
            &raw[..raw.len().min(120)]
        ),
        ToolErrorKind::NetworkError => format!("Network error{suffix}. Retrying…"),
        ToolErrorKind::ServerError => format!("Server error{suffix}. Retrying…"),
        ToolErrorKind::Other => format!(
            "Tool failed{suffix}: {}",
            &raw[..raw.len().min(200)]
        ),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_timeout() {
        assert_eq!(classify_error("request timed out"), ToolErrorKind::Timeout);
        assert_eq!(classify_error("deadline elapsed"), ToolErrorKind::Timeout);
    }

    #[test]
    fn test_classify_rate_limit() {
        assert_eq!(classify_error("HTTP 429 Too Many Requests"), ToolErrorKind::RateLimit);
        assert_eq!(classify_error("rate limit exceeded"), ToolErrorKind::RateLimit);
    }

    #[test]
    fn test_classify_auth() {
        assert_eq!(classify_error("HTTP 401 Unauthorized"), ToolErrorKind::AuthFailed);
        assert_eq!(classify_error("HTTP 403 Forbidden"), ToolErrorKind::AuthFailed);
        assert_eq!(classify_error("invalid api key"), ToolErrorKind::AuthFailed);
    }

    #[test]
    fn test_classify_server_error() {
        assert_eq!(classify_error("HTTP 500 Internal Server Error"), ToolErrorKind::ServerError);
        assert_eq!(classify_error("HTTP 503 Service Unavailable"), ToolErrorKind::ServerError);
    }

    #[test]
    fn test_classify_network() {
        assert_eq!(classify_error("connection refused"), ToolErrorKind::NetworkError);
        assert_eq!(classify_error("DNS resolution failed"), ToolErrorKind::NetworkError);
    }

    #[test]
    fn test_classify_other() {
        assert_eq!(classify_error("tool returned invalid JSON"), ToolErrorKind::Other);
    }

    #[test]
    fn test_max_attempts_auth_no_retry() {
        assert_eq!(max_attempts(&ToolErrorKind::AuthFailed), 1);
        assert_eq!(max_attempts(&ToolErrorKind::Other), 1);
    }

    #[test]
    fn test_max_attempts_retryable() {
        assert!(max_attempts(&ToolErrorKind::Timeout) > 1);
        assert!(max_attempts(&ToolErrorKind::RateLimit) > 1);
        assert!(max_attempts(&ToolErrorKind::NetworkError) > 1);
    }

    #[test]
    fn test_backoff_rate_limit_provider_override() {
        // Provider supplies retry-after → use it (capped at 120s)
        assert_eq!(backoff_secs(2, &ToolErrorKind::RateLimit, Some(45)), 45);
        assert_eq!(backoff_secs(2, &ToolErrorKind::RateLimit, Some(200)), 120);
    }

    #[test]
    fn test_backoff_timeout_immediate_then_wait() {
        assert_eq!(backoff_secs(2, &ToolErrorKind::Timeout, None), 0);
        assert_eq!(backoff_secs(3, &ToolErrorKind::Timeout, None), 5);
    }

    #[test]
    fn test_parse_retry_after() {
        assert_eq!(parse_retry_after("Retry-After: 30"), Some(30));
        assert_eq!(parse_retry_after("retry after 60 seconds"), Some(60));
        assert_eq!(parse_retry_after("no header here"), None);
    }
}
