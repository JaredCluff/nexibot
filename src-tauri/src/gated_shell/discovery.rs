//! Dynamic secret discovery for NexiGate.
//!
//! Two-phase discovery runs after every PTY command:
//!
//! **Phase A — Output scan**: Every line of raw PTY output is tested against 12 built-in
//! regex patterns (Anthropic keys, JWTs, GitHub tokens, etc.) plus any user-defined
//! extra patterns. Matches that are not already in the known set are registered.
//!
//! **Phase B — Env diff**: If `track_env_changes` is enabled, a side-channel `printenv`
//! is run before and after the user command. New/changed variables that look like secrets
//! (long enough OR match a pattern) are registered automatically.
//!
//! Proxy tokens are deterministic per process: `HMAC-SHA256(hmac_key, real_bytes)`,
//! first 8 bytes rendered as lowercase hex, prefixed by format.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use hmac::{Hmac, Mac};
use regex::Regex;
use sha2::Sha256;
use tracing::debug;

use crate::config::DiscoveryConfig;

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A secret found by the discovery engine.
#[derive(Debug, Clone)]
pub struct DiscoveredSecret {
    /// The actual secret value (never log this).
    pub real_value: String,
    /// The proxy token that will stand in for the real value.
    pub proxy_token: String,
    /// Format label, e.g. "anthropic", "github", "jwt".
    pub format: String,
    /// How this secret was discovered.
    pub source: DiscoverySource,
}

/// How a secret was discovered.
#[derive(Debug, Clone)]
pub enum DiscoverySource {
    /// Matched a regex pattern in PTY output.
    OutputScan { pattern_name: String },
    /// Appeared in the env diff (new or changed variable).
    EnvDiff { var_name: String },
}

impl DiscoverySource {
    /// Human-readable description for event payloads.
    pub fn as_str(&self) -> String {
        match self {
            DiscoverySource::OutputScan { pattern_name } => {
                format!("output_scan:{}", pattern_name)
            }
            DiscoverySource::EnvDiff { var_name } => {
                format!("env_diff:{}", var_name)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal pattern record
// ---------------------------------------------------------------------------

struct CompiledDiscoveryPattern {
    name: String,
    regex: Regex,
    format: String,
    /// Minimum match length for this pattern (extra safety gate).
    min_length: usize,
    /// Capture group index for the actual secret (0 = full match, 1 = first group).
    capture_group: usize,
}

// ---------------------------------------------------------------------------
// DiscoveryEngine
// ---------------------------------------------------------------------------

/// Heuristic pattern matching + environment diff for dynamic secret detection.
pub struct DiscoveryEngine {
    patterns: Vec<CompiledDiscoveryPattern>,
    config: DiscoveryConfig,
    /// Per-process random HMAC key — provides deterministic but un-guessable proxy tokens.
    hmac_key: [u8; 32],
}

impl DiscoveryEngine {
    /// Create a new engine with built-in patterns plus any user `extra_patterns`.
    pub fn new(config: DiscoveryConfig) -> Result<Self> {
        // Generate a random per-process HMAC key
        let mut hmac_key = [0u8; 32];
        use rand::RngCore;
        rand::rngs::OsRng.fill_bytes(&mut hmac_key);

        let mut patterns = builtin_patterns()?;

        // Compile user-defined extra patterns
        for ep in &config.extra_patterns {
            match Regex::new(&ep.pattern) {
                Ok(re) => patterns.push(CompiledDiscoveryPattern {
                    name: ep.name.clone(),
                    regex: re,
                    format: ep.format.clone(),
                    min_length: 8,
                    capture_group: 0,
                }),
                Err(e) => {
                    tracing::warn!(
                        "[NEXIGATE/DISCOVERY] Failed to compile extra pattern '{}': {}",
                        ep.name,
                        e
                    );
                }
            }
        }

        Ok(Self {
            patterns,
            config,
            hmac_key,
        })
    }

    /// Scan PTY output for secret patterns.
    ///
    /// Returns only secrets whose `real_value` is NOT already in `known`.
    pub fn scan_output(&self, output: &str, known: &HashSet<String>) -> Vec<DiscoveredSecret> {
        if !self.config.enabled || output.is_empty() {
            return Vec::new();
        }

        let mut found: Vec<DiscoveredSecret> = Vec::new();
        let mut seen_in_this_scan: HashSet<String> = HashSet::new();

        for pat in &self.patterns {
            for caps in pat.regex.captures_iter(output) {
                let matched = caps
                    .get(pat.capture_group)
                    .or_else(|| caps.get(0))
                    .map(|m| m.as_str())
                    .unwrap_or("");

                if matched.len() < pat.min_length {
                    continue;
                }
                if known.contains(matched) {
                    continue;
                }
                if seen_in_this_scan.contains(matched) {
                    continue;
                }
                seen_in_this_scan.insert(matched.to_string());

                let proxy_token = self.make_proxy_token(matched, &pat.format);
                debug!(
                    "[NEXIGATE/DISCOVERY] Discovered {} secret via pattern '{}' (proxy: {}...)",
                    pat.format,
                    pat.name,
                    &proxy_token[..proxy_token.len().min(20)]
                );
                found.push(DiscoveredSecret {
                    real_value: matched.to_string(),
                    proxy_token,
                    format: pat.format.clone(),
                    source: DiscoverySource::OutputScan {
                        pattern_name: pat.name.clone(),
                    },
                });
            }
        }

        found
    }

    /// Diff two env snapshots and find new or changed variables that look like secrets.
    ///
    /// Returns only secrets whose `real_value` is NOT already in `known`.
    pub fn diff_env(
        &self,
        before: &HashMap<String, String>,
        after: &HashMap<String, String>,
        known: &HashSet<String>,
    ) -> Vec<DiscoveredSecret> {
        if !self.config.enabled {
            return Vec::new();
        }

        let mut found: Vec<DiscoveredSecret> = Vec::new();

        for (var_name, new_val) in after {
            // Only process new or changed values
            let changed = match before.get(var_name) {
                Some(old) => old != new_val,
                None => true, // new variable
            };
            if !changed {
                continue;
            }
            if new_val.len() < self.config.min_secret_length {
                continue;
            }
            if known.contains(new_val.as_str()) {
                continue;
            }

            // Check if it matches any pattern OR meets length+entropy heuristic
            let matches_pattern = self
                .patterns
                .iter()
                .any(|p| p.regex.is_match(new_val) && new_val.len() >= p.min_length);

            let looks_like_secret = matches_pattern || looks_high_entropy(new_val);

            if !looks_like_secret {
                continue;
            }

            // Determine format
            let format = self
                .patterns
                .iter()
                .find(|p| p.regex.is_match(new_val))
                .map(|p| p.format.clone())
                .unwrap_or_else(|| "generic".to_string());

            let proxy_token = self.make_proxy_token(new_val, &format);
            debug!(
                "[NEXIGATE/DISCOVERY] Discovered env secret from var '{}' via diff (format: {})",
                var_name, format
            );

            found.push(DiscoveredSecret {
                real_value: new_val.clone(),
                proxy_token,
                format,
                source: DiscoverySource::EnvDiff {
                    var_name: var_name.clone(),
                },
            });
        }

        found
    }

    /// Generate a deterministic proxy token for a real value.
    ///
    /// Token = `{format_prefix}-NEXIGATE-{hex8}`
    /// where hex8 = first 8 bytes of HMAC-SHA256(hmac_key, real_value) as lowercase hex.
    pub fn make_proxy_token(&self, real: &str, format: &str) -> String {
        let mut mac =
            HmacSha256::new_from_slice(&self.hmac_key).expect("HMAC accepts any key length");
        mac.update(real.as_bytes());
        let result = mac.finalize().into_bytes();
        let hex8 = hex::encode(&result[..8]);
        let prefix = format_prefix(format);
        format!("{}-NEXIGATE-{}", prefix, hex8)
    }

    /// Whether env-change tracking is enabled.
    #[allow(dead_code)]
    pub fn track_env_changes(&self) -> bool {
        self.config.track_env_changes
    }
}

// ---------------------------------------------------------------------------
// Built-in patterns
// ---------------------------------------------------------------------------

fn builtin_patterns() -> Result<Vec<CompiledDiscoveryPattern>> {
    let defs: &[(&str, &str, &str, usize, usize)] = &[
        // (name, pattern, format, min_length, capture_group)
        (
            "anthropic_api_key",
            r"sk-ant-[A-Za-z0-9\-_]{80,}",
            "anthropic",
            84,
            0,
        ),
        ("openai_api_key", r"sk-[A-Za-z0-9]{48,}", "openai", 50, 0),
        (
            "github_token",
            r"(ghp|ghs|gho|ghu|ghr)_[A-Za-z0-9]{36}",
            "github",
            40,
            0,
        ),
        ("aws_access_key", r"AKIA[A-Z0-9]{16}", "aws", 20, 0),
        ("google_api_key", r"AIza[A-Za-z0-9\-_]{35}", "google", 39, 0),
        (
            "slack_token",
            r"xox[bpoa]-[A-Za-z0-9\-]{50,}",
            "slack",
            55,
            0,
        ),
        (
            "stripe_key",
            r"(sk|pk)_(test|live)_[A-Za-z0-9]{24,}",
            "stripe",
            32,
            0,
        ),
        (
            "jwt_token",
            r"ey[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]{43,}",
            "jwt",
            80,
            0,
        ),
        (
            "sendgrid_key",
            r"SG\.[A-Za-z0-9\-_]{22}\.[A-Za-z0-9\-_]{43}",
            "sendgrid",
            68,
            0,
        ),
        ("twilio_sid", r"SK[a-f0-9]{32}", "twilio", 34, 0),
        (
            "pem_private_key",
            r"-----BEGIN (?:RSA |EC )?PRIVATE KEY-----",
            "pem",
            27,
            0,
        ),
        (
            "bearer_token",
            r"[Bb]earer\s+([A-Za-z0-9\-._~+/]{40,})",
            "bearer",
            47,
            1,
        ),
    ];

    let mut patterns = Vec::with_capacity(defs.len());
    for (name, pattern, format, min_length, capture_group) in defs {
        let regex = Regex::new(pattern)
            .map_err(|e| anyhow::anyhow!("Built-in pattern '{}' failed to compile: {}", name, e))?;
        patterns.push(CompiledDiscoveryPattern {
            name: name.to_string(),
            regex,
            format: format.to_string(),
            min_length: *min_length,
            capture_group: *capture_group,
        });
    }
    Ok(patterns)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a format label to a human-readable proxy prefix.
fn format_prefix(format: &str) -> &str {
    match format {
        "anthropic" => "sk-ant",
        "openai" => "sk",
        "github" => "ghp",
        "aws" => "AKIA",
        "google" => "AIza",
        "slack" => "xoxb",
        "stripe" => "sk",
        "jwt" => "eyJ",
        "sendgrid" => "SG",
        "twilio" => "SK",
        "pem" => "PEM",
        "bearer" => "Bearer",
        _ => "TOKEN",
    }
}

/// Simple entropy heuristic: at least 20 chars, no whitespace, mixed char classes.
fn looks_high_entropy(s: &str) -> bool {
    if s.len() < 20 {
        return false;
    }
    if s.contains(char::is_whitespace) {
        return false;
    }
    let has_upper = s.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = s.chars().any(|c| c.is_ascii_lowercase());
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    let has_special = s
        .chars()
        .any(|c| matches!(c, '-' | '_' | '+' | '/' | '=' | '.'));
    // Require at least 3 of 4 char classes
    [has_upper, has_lower, has_digit, has_special]
        .iter()
        .filter(|&&b| b)
        .count()
        >= 3
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> DiscoveryEngine {
        DiscoveryEngine::new(DiscoveryConfig::default()).expect("engine creation failed")
    }

    fn known_empty() -> HashSet<String> {
        HashSet::new()
    }

    // ---- Pattern match tests ----

    #[test]
    fn test_anthropic_key_detected() {
        let e = engine();
        let key = format!("sk-ant-{}", "A".repeat(80));
        let found = e.scan_output(&format!("echo {}", key), &known_empty());
        assert!(!found.is_empty(), "anthropic key not detected");
        assert_eq!(found[0].format, "anthropic");
        assert_eq!(found[0].real_value, key);
    }

    #[test]
    fn test_openai_key_detected() {
        let e = engine();
        let key = format!("sk-{}", "B".repeat(48));
        let found = e.scan_output(&key, &known_empty());
        assert!(!found.is_empty(), "openai key not detected");
        assert_eq!(found[0].format, "openai");
    }

    #[test]
    fn test_github_token_detected() {
        let e = engine();
        let token = format!("ghp_{}", "C".repeat(36));
        let found = e.scan_output(&token, &known_empty());
        assert!(!found.is_empty(), "github token not detected");
        assert_eq!(found[0].format, "github");
    }

    #[test]
    fn test_aws_access_key_detected() {
        let e = engine();
        let key = format!("AKIA{}", "D".repeat(16));
        let found = e.scan_output(&key, &known_empty());
        assert!(!found.is_empty(), "AWS access key not detected");
        assert_eq!(found[0].format, "aws");
    }

    #[test]
    fn test_google_api_key_detected() {
        let e = engine();
        let key = format!("AIza{}", "E".repeat(35));
        let found = e.scan_output(&key, &known_empty());
        assert!(!found.is_empty(), "Google API key not detected");
        assert_eq!(found[0].format, "google");
    }

    #[test]
    fn test_jwt_token_detected() {
        // Minimal valid JWT structure: header.payload.signature
        let header = base64::engine::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            r#"{"alg":"HS256"}"#,
        );
        let payload = base64::engine::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            r#"{"sub":"1234567890"}"#,
        );
        let sig = "A".repeat(43);
        let jwt = format!("{}.{}.{}", header, payload, sig);
        let e = engine();
        let found = e.scan_output(&jwt, &known_empty());
        assert!(!found.is_empty(), "JWT not detected: {}", jwt);
        assert_eq!(found[0].format, "jwt");
    }

    #[test]
    fn test_no_false_positive_on_ls_output() {
        let e = engine();
        let output = "total 0\ndrwxr-xr-x  5 user group  160 Feb 28 14:23 .\ndrwxr-xr-x 10 user group  320 Feb 28 12:00 ..\n-rw-r--r--  1 user group    0 Feb 28 14:23 README.md\n";
        let found = e.scan_output(output, &known_empty());
        assert!(
            found.is_empty(),
            "false positive on ls output: {:?}",
            found.iter().map(|s| &s.real_value).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_no_false_positive_on_empty_output() {
        let e = engine();
        let found = e.scan_output("", &known_empty());
        assert!(found.is_empty());
    }

    #[test]
    fn test_known_secret_skipped() {
        let e = engine();
        let key = format!("sk-ant-{}", "A".repeat(80));
        let mut known = HashSet::new();
        known.insert(key.clone());
        let found = e.scan_output(&key, &known);
        assert!(found.is_empty(), "known secret should not be re-discovered");
    }

    #[test]
    fn test_proxy_token_deterministic() {
        let e = engine();
        let real = "test-real-value-for-determinism-check";
        let t1 = e.make_proxy_token(real, "generic");
        let t2 = e.make_proxy_token(real, "generic");
        assert_eq!(
            t1, t2,
            "proxy tokens must be deterministic within one process"
        );
    }

    #[test]
    fn test_diff_env_detects_new_var() {
        let e = engine();
        let before: HashMap<String, String> = HashMap::new();
        let mut after: HashMap<String, String> = HashMap::new();
        // Long enough (≥20 chars) and high-entropy
        after.insert(
            "MY_API_KEY".to_string(),
            "sK3-veryLongSecretValue123XYZ".to_string(),
        );
        let found = e.diff_env(&before, &after, &known_empty());
        assert!(!found.is_empty(), "new env var not detected");
        assert!(matches!(found[0].source, DiscoverySource::EnvDiff { .. }));
    }

    #[test]
    fn test_diff_env_ignores_unchanged() {
        let e = engine();
        let mut before: HashMap<String, String> = HashMap::new();
        before.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        before.insert("HOME".to_string(), dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/home/user")).to_string_lossy().to_string());
        let after = before.clone();
        let found = e.diff_env(&before, &after, &known_empty());
        assert!(found.is_empty(), "unchanged env should not be detected");
    }

    #[test]
    fn test_diff_env_skips_short_values() {
        let e = engine();
        let before: HashMap<String, String> = HashMap::new();
        let mut after: HashMap<String, String> = HashMap::new();
        after.insert("SHORT_VAR".to_string(), "abc123".to_string()); // too short
        let found = e.diff_env(&before, &after, &known_empty());
        assert!(found.is_empty(), "short value should not be detected");
    }

    #[test]
    fn test_user_extra_pattern_works() {
        let config = DiscoveryConfig {
            enabled: true,
            track_env_changes: false,
            min_secret_length: 20,
            extra_patterns: vec![crate::config::ExtraDiscoveryPattern {
                name: "custom_token".to_string(),
                pattern: r"CUSTOM-[A-Z0-9]{16}".to_string(),
                format: "custom".to_string(),
            }],
        };
        let e = DiscoveryEngine::new(config).expect("engine");
        let found = e.scan_output("CUSTOM-ABCDEF1234567890", &known_empty());
        assert!(!found.is_empty(), "custom pattern not detected");
        assert_eq!(found[0].format, "custom");
    }
}
