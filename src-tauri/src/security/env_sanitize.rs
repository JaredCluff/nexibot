//! Environment variable sanitization for child processes.
//!
//! Prevents accidental leakage of API keys, tokens, and passwords when
//! spawning child processes.

use regex::Regex;
use std::collections::HashMap;
use tracing::debug;

/// Environment variables that must **never** be overridden by skills or config,
/// even in non-strict mode. These are critical system and dynamic-linker vars
/// that could be used for privilege escalation or library injection attacks.
const HARD_BLOCKED_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
];

/// Patterns that match environment variable names containing secrets.
const BLOCKED_NAME_PATTERNS: &[&str] = &[
    r"^ANTHROPIC_API_KEY$",
    r"^OPENAI_API_KEY$",
    r"^GEMINI_API_KEY$",
    r"^OPENROUTER_API_KEY$",
    r"^MINIMAX_API_KEY$",
    r"^ELEVENLABS_API_KEY$",
    r"^TELEGRAM_BOT_TOKEN$",
    r"^DISCORD_BOT_TOKEN$",
    r"^SLACK_(BOT|APP)_TOKEN$",
    r"^AWS_(SECRET_ACCESS_KEY|SECRET_KEY|SESSION_TOKEN)$",
    r"^(GH|GITHUB)_TOKEN$",
    r"^(AZURE|AZURE_OPENAI|COHERE|AI_GATEWAY)_API_KEY$",
    r"_?(API_KEY|TOKEN|PASSWORD|PRIVATE_KEY|SECRET|CREDENTIALS)$",
];

/// Patterns that match safe environment variable names.
const ALLOWED_NAME_PATTERNS: &[&str] = &[
    r"^PATH$",
    r"^HOME$",
    r"^USER$",
    r"^SHELL$",
    r"^TERM$",
    r"^TZ$",
    r"^LANG$",
    r"^LC_.*$",
    r"^RUST_LOG$",
    r"^XDG_.*$",
    r"^TMPDIR$",
    r"^PWD$",
    r"^NODE_ENV$",
    r"^EDITOR$",
    r"^VISUAL$",
    r"^COLORTERM$",
    r"^DISPLAY$",
];

/// Maximum length for a single environment variable value.
const MAX_VALUE_LENGTH: usize = 32_768;

/// Result of environment variable sanitization.
#[derive(Debug)]
pub struct SanitizeResult {
    /// Allowed environment variables.
    pub allowed: HashMap<String, String>,
    /// Names of blocked environment variables.
    pub blocked: Vec<String>,
    /// Warning messages about suspicious values.
    pub warnings: Vec<String>,
}

/// Options for sanitization behavior.
#[derive(Debug, Default)]
pub struct SanitizeOptions {
    /// When true, only explicitly allowed variables pass through.
    pub strict_mode: bool,
}

fn matches_any(name: &str, patterns: &[Regex]) -> bool {
    patterns.iter().any(|re| re.is_match(name))
}

/// Minimum length of a base64-like string to trigger credential heuristic warning.
const BASE64_CREDENTIAL_MIN_LENGTH: usize = 80;

fn looks_like_base64_credential(value: &str) -> bool {
    if value.len() < BASE64_CREDENTIAL_MIN_LENGTH {
        return false;
    }
    // Check if the value is predominantly base64 characters
    let base64_chars = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '=')
        .count();
    base64_chars >= BASE64_CREDENTIAL_MIN_LENGTH && base64_chars == value.len()
}

fn validate_value(value: &str) -> Option<&'static str> {
    if value.contains('\0') {
        return Some("Contains null bytes");
    }
    if value.len() > MAX_VALUE_LENGTH {
        return Some("Value exceeds maximum length");
    }
    if looks_like_base64_credential(value) {
        return Some("Value looks like a base64-encoded credential");
    }
    None
}

/// Check if a variable name is in the hard-blocked list.
///
/// Hard-blocked variables are never allowed to be overridden, regardless of
/// strict mode or any other option. This prevents dynamic-linker injection
/// and other system-level attacks.
fn is_hard_blocked(name: &str) -> bool {
    HARD_BLOCKED_VARS
        .iter()
        .any(|&blocked| blocked.eq_ignore_ascii_case(name))
}

/// Strip hard-blocked variables from an environment variable map.
///
/// This is applied **after** the main sanitization pass to ensure these
/// critical variables are never included, even if they were explicitly
/// allowed by the caller's config. Returns the names of removed variables.
#[allow(dead_code)]
pub fn strip_hard_blocked(vars: &mut HashMap<String, String>) -> Vec<String> {
    let mut removed = Vec::new();
    vars.retain(|key, _| {
        if is_hard_blocked(key) {
            removed.push(key.clone());
            false
        } else {
            true
        }
    });
    if !removed.is_empty() {
        debug!("[ENV_SANITIZE] Hard-blocked vars stripped: {:?}", removed);
    }
    removed
}

/// Sanitize a set of environment variables, removing secrets.
///
/// Hard-blocked variables (`HARD_BLOCKED_VARS`) are **always** stripped from
/// the result, even in non-strict mode.
pub fn sanitize_env_vars(
    vars: &HashMap<String, String>,
    options: &SanitizeOptions,
) -> SanitizeResult {
    let blocked_regexes: Vec<Regex> = BLOCKED_NAME_PATTERNS
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect();

    let allowed_regexes: Vec<Regex> = ALLOWED_NAME_PATTERNS
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect();

    let mut result = SanitizeResult {
        allowed: HashMap::new(),
        blocked: Vec::new(),
        warnings: Vec::new(),
    };

    for (key, value) in vars {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }

        // Hard-blocked vars are always stripped, regardless of mode
        if is_hard_blocked(key) {
            result.blocked.push(key.to_string());
            continue;
        }

        // Check blocked patterns first
        if matches_any(key, &blocked_regexes) {
            result.blocked.push(key.to_string());
            continue;
        }

        // In strict mode, only explicitly allowed vars pass
        if options.strict_mode && !matches_any(key, &allowed_regexes) {
            result.blocked.push(key.to_string());
            continue;
        }

        // Validate the value
        let mut value = value.clone();
        if let Some(warning) = validate_value(&value) {
            if warning == "Contains null bytes" {
                result.blocked.push(key.to_string());
                continue;
            }
            if warning == "Value exceeds maximum length" {
                // Hard truncation instead of warn-and-pass-through: a multi-GB env
                // var can exhaust process memory when spawning subprocesses.  Truncate
                // to MAX_VALUE_LENGTH and record a warning so callers are informed.
                tracing::warn!(
                    "[ENV_SANITIZE] Value for key '{}' is {}KB (max {}KB), truncating to prevent memory exhaustion",
                    key,
                    value.len() / 1024,
                    MAX_VALUE_LENGTH / 1024
                );
                value.truncate(MAX_VALUE_LENGTH);
                result.warnings.push(format!("{}: {} (truncated to {}KB)", key, warning, MAX_VALUE_LENGTH / 1024));
            } else {
                result.warnings.push(format!("{}: {}", key, warning));
            }
        }

        result.allowed.insert(key.to_string(), value);
    }

    debug!(
        "[ENV_SANITIZE] Allowed {} vars, blocked {} vars",
        result.allowed.len(),
        result.blocked.len()
    );

    result
}

/// Convenience: gather the current process environment and sanitize it.
pub fn build_safe_env(options: &SanitizeOptions) -> SanitizeResult {
    let vars: HashMap<String, String> = std::env::vars().collect();
    sanitize_env_vars(&vars, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocks_api_keys() {
        let mut vars = HashMap::new();
        vars.insert("ANTHROPIC_API_KEY".into(), "sk-ant-secret".into());
        vars.insert("OPENAI_API_KEY".into(), "sk-openai-secret".into());
        vars.insert("TMPDIR".into(), std::env::temp_dir().to_string_lossy().to_string());

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        assert!(result.allowed.contains_key("TMPDIR"));
        assert!(!result.allowed.contains_key("ANTHROPIC_API_KEY"));
        assert!(!result.allowed.contains_key("OPENAI_API_KEY"));
        assert!(result.blocked.contains(&"ANTHROPIC_API_KEY".to_string()));
        assert!(result.blocked.contains(&"OPENAI_API_KEY".to_string()));
    }

    #[test]
    fn test_blocks_generic_secret_suffix() {
        let mut vars = HashMap::new();
        vars.insert("MY_CUSTOM_API_KEY".into(), "secret".into());
        vars.insert("DATABASE_PASSWORD".into(), "pass".into());
        vars.insert("SIGNING_SECRET".into(), "shhh".into());

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        assert!(result.blocked.contains(&"MY_CUSTOM_API_KEY".to_string()));
        assert!(result.blocked.contains(&"DATABASE_PASSWORD".to_string()));
        assert!(result.blocked.contains(&"SIGNING_SECRET".to_string()));
    }

    #[test]
    fn test_allows_safe_vars() {
        let mut vars = HashMap::new();
        vars.insert("LANG".into(), "en_US.UTF-8".into());
        vars.insert("LC_ALL".into(), "en_US.UTF-8".into());
        vars.insert("RUST_LOG".into(), "debug".into());
        vars.insert("TERM".into(), "xterm-256color".into());
        vars.insert("TZ".into(), "UTC".into());

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        assert_eq!(result.allowed.len(), 5);
        assert!(result.blocked.is_empty());
    }

    #[test]
    fn test_strict_mode_blocks_unknown() {
        let mut vars = HashMap::new();
        vars.insert("LANG".into(), "en_US.UTF-8".into());
        vars.insert("CUSTOM_VAR".into(), "value".into());

        let opts = SanitizeOptions { strict_mode: true };
        let result = sanitize_env_vars(&vars, &opts);
        assert!(result.allowed.contains_key("LANG"));
        assert!(!result.allowed.contains_key("CUSTOM_VAR"));
    }

    #[test]
    fn test_blocks_null_bytes_in_value() {
        let mut vars = HashMap::new();
        vars.insert("SAFE_NAME".into(), "has\0null".into());

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        assert!(result.blocked.contains(&"SAFE_NAME".to_string()));
    }

    #[test]
    fn test_warns_on_long_value() {
        let mut vars = HashMap::new();
        vars.insert("SAFE_NAME".into(), "x".repeat(40_000));

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        // Long values produce a warning but are still allowed
        assert!(result.allowed.contains_key("SAFE_NAME"));
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn test_blocks_credentials_suffix() {
        let mut vars = HashMap::new();
        vars.insert("DATABASE_CREDENTIALS".into(), "user:pass".into());
        vars.insert("MY_SERVICE_CREDENTIALS".into(), "creds".into());

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        assert!(result.blocked.contains(&"DATABASE_CREDENTIALS".to_string()));
        assert!(result
            .blocked
            .contains(&"MY_SERVICE_CREDENTIALS".to_string()));
    }

    #[test]
    fn test_warns_on_base64_credential() {
        let mut vars = HashMap::new();
        // 80+ chars of pure base64
        let b64_value = "A".repeat(100);
        vars.insert("SOME_VAR".into(), b64_value);

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        assert!(result.allowed.contains_key("SOME_VAR"));
        assert!(
            result.warnings.iter().any(|w| w.contains("base64")),
            "Expected base64 credential warning, got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_no_base64_warning_for_short_values() {
        let mut vars = HashMap::new();
        vars.insert("SHORT_VAR".into(), "AAAA".into());

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        assert!(result.allowed.contains_key("SHORT_VAR"));
        assert!(result.warnings.is_empty());
    }

    // -----------------------------------------------------------------------
    // Hard-blocked variable tests (P2A)
    // -----------------------------------------------------------------------

    #[test]
    fn test_hard_blocked_vars_always_stripped() {
        let mut vars = HashMap::new();
        for &name in &[
            "PATH",
            "HOME",
            "USER",
            "SHELL",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            "DYLD_FRAMEWORK_PATH",
        ] {
            vars.insert(name.into(), "/some/value".into());
        }
        // Also include a safe var that should pass through
        vars.insert("LANG".into(), "en_US.UTF-8".into());

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        // All hard-blocked vars should be blocked
        for name in &[
            "PATH",
            "HOME",
            "USER",
            "SHELL",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            "DYLD_FRAMEWORK_PATH",
        ] {
            assert!(
                !result.allowed.contains_key(*name),
                "{} should be hard-blocked but was allowed",
                name
            );
            assert!(
                result.blocked.contains(&name.to_string()),
                "{} should appear in blocked list",
                name
            );
        }
        // Safe var passes
        assert!(result.allowed.contains_key("LANG"));
    }

    #[test]
    fn test_hard_blocked_in_non_strict_mode() {
        // Even in non-strict mode, hard-blocked vars must be stripped
        let mut vars = HashMap::new();
        vars.insert("PATH".into(), "/usr/bin".into());
        vars.insert("LD_PRELOAD".into(), "/evil/lib.so".into());
        vars.insert("CUSTOM_VAR".into(), "hello".into());

        let result = sanitize_env_vars(&vars, &SanitizeOptions { strict_mode: false });
        assert!(!result.allowed.contains_key("PATH"));
        assert!(!result.allowed.contains_key("LD_PRELOAD"));
        assert!(result.allowed.contains_key("CUSTOM_VAR"));
    }

    #[test]
    fn test_hard_blocked_in_strict_mode() {
        let mut vars = HashMap::new();
        vars.insert("PATH".into(), "/usr/bin".into());
        vars.insert("HOME".into(), "/home/user".into());
        vars.insert("LANG".into(), "en_US.UTF-8".into());

        let result = sanitize_env_vars(&vars, &SanitizeOptions { strict_mode: true });
        // PATH and HOME are hard-blocked even though they match ALLOWED_NAME_PATTERNS
        assert!(!result.allowed.contains_key("PATH"));
        assert!(!result.allowed.contains_key("HOME"));
        // LANG is allowed (matches ALLOWED_NAME_PATTERNS, not hard-blocked)
        assert!(result.allowed.contains_key("LANG"));
    }

    #[test]
    fn test_hard_blocked_case_insensitive() {
        let mut vars = HashMap::new();
        vars.insert("path".into(), "/usr/bin".into());
        vars.insert("Path".into(), "/usr/bin".into());
        vars.insert("ld_preload".into(), "/evil/lib.so".into());

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        assert!(!result.allowed.contains_key("path"));
        assert!(!result.allowed.contains_key("Path"));
        assert!(!result.allowed.contains_key("ld_preload"));
    }

    #[test]
    fn test_strip_hard_blocked_from_map() {
        let mut vars = HashMap::new();
        vars.insert("PATH".into(), "/usr/bin".into());
        vars.insert("HOME".into(), "/home/user".into());
        vars.insert("LANG".into(), "en_US.UTF-8".into());
        vars.insert("LD_PRELOAD".into(), "/evil/lib.so".into());

        let removed = strip_hard_blocked(&mut vars);
        assert_eq!(vars.len(), 1);
        assert!(vars.contains_key("LANG"));
        assert_eq!(removed.len(), 3);
        assert!(removed.contains(&"PATH".to_string()));
        assert!(removed.contains(&"HOME".to_string()));
        assert!(removed.contains(&"LD_PRELOAD".to_string()));
    }

    #[test]
    fn test_strip_hard_blocked_empty_map() {
        let mut vars = HashMap::new();
        let removed = strip_hard_blocked(&mut vars);
        assert!(removed.is_empty());
        assert!(vars.is_empty());
    }

    #[test]
    fn test_dyld_injection_vars_blocked() {
        // Specific test for macOS dynamic linker injection vectors
        let mut vars = HashMap::new();
        vars.insert("DYLD_INSERT_LIBRARIES".into(), "/evil/inject.dylib".into());
        vars.insert("DYLD_LIBRARY_PATH".into(), "/evil/libs".into());
        vars.insert("DYLD_FRAMEWORK_PATH".into(), "/evil/frameworks".into());

        let result = sanitize_env_vars(&vars, &SanitizeOptions::default());
        assert!(result.allowed.is_empty());
        assert_eq!(result.blocked.len(), 3);
    }
}
