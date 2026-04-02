//! PII Redaction
//!
//! Scans text for common PII patterns and replaces each match with a typed
//! placeholder token before the text is forwarded to an LLM provider.
//!
//! Supported types:
//! - Email addresses         → `[EMAIL]`
//! - Phone numbers (US/intl) → `[PHONE]`
//! - US Social Security Nos  → `[SSN]`
//! - Credit card numbers     → `[CREDIT_CARD]`
//! - IPv4 addresses          → `[IP_ADDRESS]`

use regex::Regex;
use std::sync::OnceLock;

// ─────────────────────────────────────────────────────────────────────────────
// Compiled regex cache
// ─────────────────────────────────────────────────────────────────────────────

struct PiiPatterns {
    email:       Regex,
    phone:       Regex,
    ssn:         Regex,
    credit_card: Regex,
    ip_address:  Regex,
}

static PATTERNS: OnceLock<PiiPatterns> = OnceLock::new();

fn patterns() -> &'static PiiPatterns {
    PATTERNS.get_or_init(|| PiiPatterns {
        // RFC-5321-ish: local@domain.tld
        email: Regex::new(
            r"(?i)\b[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,}\b",
        )
        .expect("email regex"),

        // US domestic (10-digit) and E.164 international (+1…) patterns.
        // Separators: spaces, dashes, dots, parentheses.
        phone: Regex::new(
            r"(?x)
            (?:\+?1[\s.\-]?)?                  # optional country code
            (?:\(\d{3}\)|\d{3})                # area code
            [\s.\-]?
            \d{3}
            [\s.\-]?
            \d{4}
            \b",
        )
        .expect("phone regex"),

        // US SSN: 3-2-4 format with separators
        // Note: Rust's regex crate doesn't support lookahead, so we match
        // any 3-2-4 digit pattern and accept minor false positives on
        // invalid SSN prefixes (000, 666, 9xx).
        ssn: Regex::new(
            r"\b\d{3}[-\s]\d{2}[-\s]\d{4}\b",
        )
        .expect("ssn regex"),

        // Luhn-plausible 13–19 digit sequences with optional spaces/dashes
        // between groups of 4.  We don't run Luhn here — false negatives are
        // acceptable; false positives of random long numbers are not critical.
        credit_card: Regex::new(
            r"(?x)
            \b
            (?:
              4[0-9]{3}  |                         # Visa
              5[1-5][0-9]{2} |                     # MC
              3[47][0-9]{2}  |                     # Amex
              6(?:011|5[0-9]{2})[0-9]              # Discover
            )
            (?:[-\s]?\d{4}){2,3}
            (?:[-\s]?\d{3,4})?
            \b",
        )
        .expect("credit card regex"),

        // IPv4 dotted-decimal
        ip_address: Regex::new(
            r"\b(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\b",
        )
        .expect("ip address regex"),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Replace all detected PII in `text` with placeholder tokens.
/// Returns a new `String`; the original is never modified.
pub fn redact(text: &str) -> String {
    let p = patterns();

    // Apply substitutions in order — least destructive first so that
    // phone numbers aren't partially consumed by an SSN match.
    let s = p.ssn.replace_all(text, "[SSN]");
    let s = p.credit_card.replace_all(&s, "[CREDIT_CARD]");
    let s = p.email.replace_all(&s, "[EMAIL]");
    let s = p.phone.replace_all(&s, "[PHONE]");
    let s = p.ip_address.replace_all(&s, "[IP_ADDRESS]");

    s.into_owned()
}

/// Return `true` if any PII pattern matches in `text`.
pub fn contains_pii(text: &str) -> bool {
    let p = patterns();
    p.email.is_match(text)
        || p.phone.is_match(text)
        || p.ssn.is_match(text)
        || p.credit_card.is_match(text)
        || p.ip_address.is_match(text)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_email() {
        let out = redact("Contact me at alice@example.com for details.");
        assert_eq!(out, "Contact me at [EMAIL] for details.");
    }

    #[test]
    fn test_redact_phone() {
        let out = redact("Call me at (555) 867-5309 anytime.");
        assert!(out.contains("[PHONE]"), "output: {}", out);
    }

    #[test]
    fn test_redact_ssn() {
        let out = redact("My SSN is 123-45-6789.");
        assert!(out.contains("[SSN]"), "output: {}", out);
    }

    #[test]
    fn test_redact_credit_card() {
        let out = redact("Visa: 4111 1111 1111 1111");
        assert!(out.contains("[CREDIT_CARD]"), "output: {}", out);
    }

    #[test]
    fn test_redact_ip() {
        let out = redact("Server at 192.168.1.100 is down.");
        assert_eq!(out, "Server at [IP_ADDRESS] is down.");
    }

    #[test]
    fn test_no_false_positive_plain_text() {
        let clean = "The quick brown fox jumps over the lazy dog.";
        assert_eq!(redact(clean), clean);
    }

    #[test]
    fn test_contains_pii_positive() {
        assert!(contains_pii("user@host.io"));
    }

    #[test]
    fn test_contains_pii_negative() {
        assert!(!contains_pii("no pii here at all"));
    }
}
