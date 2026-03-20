//! Bidirectional filter layer for NexiGate.
//!
//! Inbound (command → PTY):  proxy token → real value
//! Outbound (PTY → agent):   real value → proxy token
//!
//! Real keys are kept in `secrecy::SecretString` so they zero memory on drop
//! and cannot be printed via Debug.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Direction of a filter substitution event.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilterDirection {
    /// proxy → real (inbound to PTY)
    Inbound,
    /// real → proxy (outbound to agent)
    Outbound,
}

/// A single substitution event recorded during filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterEvent {
    pub direction: FilterDirection,
    /// The proxy key that was matched or substituted.
    pub proxy_key: String,
    /// Best-guess format label ("anthropic", "openai", "generic", etc.)
    pub key_format: String,
    /// Byte offset of the substitution in the original string.
    pub position: usize,
    /// Length of the matched/inserted span.
    pub length: usize,
    pub timestamp_ms: u64,
}

/// Bidirectional filter layer.
///
/// Thread-safe: all maps are behind RwLock; debug_mode uses an AtomicBool.
pub struct FilterLayer {
    /// proxy_key → real_value  (for inbound: restore real value for PTY)
    proxy_to_real: RwLock<HashMap<String, String>>,
    /// real_value → proxy_key  (for outbound: mask real value from agent)
    real_to_proxy: RwLock<HashMap<String, String>>,
    /// Compiled outbound patterns: (real_value_regex, proxy_key)
    outbound_patterns: RwLock<Vec<(Regex, String)>>,
    /// Compiled inbound patterns: (proxy_key_regex, real_value)
    inbound_patterns: RwLock<Vec<(Regex, String)>>,
    pub debug_mode: AtomicBool,
}

impl FilterLayer {
    pub fn new() -> Self {
        Self {
            proxy_to_real: RwLock::new(HashMap::new()),
            real_to_proxy: RwLock::new(HashMap::new()),
            outbound_patterns: RwLock::new(Vec::new()),
            inbound_patterns: RwLock::new(Vec::new()),
            debug_mode: AtomicBool::new(false),
        }
    }

    /// Register a real↔proxy key pair.
    ///
    /// Both regex patterns are compiled eagerly so repeated filtering is fast.
    /// Special regex characters in key values are escaped before compiling.
    pub fn register_secret(&self, real: &str, proxy: &str) {
        if real.is_empty() || proxy.is_empty() {
            return;
        }

        let real_escaped = regex::escape(real);
        let proxy_escaped = regex::escape(proxy);

        let outbound_re = match Regex::new(&real_escaped) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "[NEXIGATE] Failed to compile outbound pattern for secret: {}",
                    e
                );
                return;
            }
        };
        let inbound_re = match Regex::new(&proxy_escaped) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "[NEXIGATE] Failed to compile inbound pattern for proxy: {}",
                    e
                );
                return;
            }
        };

        {
            let mut p2r = self
                .proxy_to_real
                .write()
                .unwrap_or_else(|e| e.into_inner());
            let mut r2p = self
                .real_to_proxy
                .write()
                .unwrap_or_else(|e| e.into_inner());
            p2r.insert(proxy.to_string(), real.to_string());
            r2p.insert(real.to_string(), proxy.to_string());
        }
        {
            let mut out = self
                .outbound_patterns
                .write()
                .unwrap_or_else(|e| e.into_inner());
            let mut inp = self
                .inbound_patterns
                .write()
                .unwrap_or_else(|e| e.into_inner());
            // Remove stale entry for same proxy/real if re-registering
            out.retain(|(_, p)| p != proxy);
            inp.retain(|(_, r)| r != real);
            out.push((outbound_re, proxy.to_string()));
            inp.push((inbound_re, real.to_string()));
        }

        debug!(
            "[NEXIGATE] Registered secret pair (proxy prefix: {})",
            &proxy[..proxy.len().min(20)]
        );
    }

    /// Sync filter maps from a key vault.
    ///
    /// Calls `register_secret` for every proxy→real pair in the vault.
    pub fn sync_from_vault(&self, all_proxy_to_real: &HashMap<String, String>) {
        for (proxy, real) in all_proxy_to_real {
            self.register_secret(real, proxy);
        }
        debug!(
            "[NEXIGATE] Synced {} secrets from vault",
            all_proxy_to_real.len()
        );
    }

    /// Filter an inbound command string: replace proxy tokens with real values.
    ///
    /// Returns the filtered string and a list of substitution events.
    pub fn filter_inbound(&self, command: &str) -> (String, Vec<FilterEvent>) {
        let patterns = self
            .inbound_patterns
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let mut result = command.to_string();
        let mut events = Vec::new();
        let ts = now_ms();

        for (proxy_re, real_val) in patterns.iter() {
            let mut offset = 0usize;
            let mut new_result = String::with_capacity(result.len());
            let mut last_end = 0;

            for m in proxy_re.find_iter(&result) {
                new_result.push_str(&result[last_end..m.start()]);
                let proxy_matched = &result[m.start()..m.end()];
                let format = detect_key_format(proxy_matched);
                events.push(FilterEvent {
                    direction: FilterDirection::Inbound,
                    proxy_key: proxy_matched.to_string(),
                    key_format: format,
                    position: m.start() + offset,
                    length: real_val.len(),
                    timestamp_ms: ts,
                });
                new_result.push_str(real_val);
                offset = offset
                    .saturating_add(real_val.len())
                    .saturating_sub(m.len());
                last_end = m.end();
            }
            new_result.push_str(&result[last_end..]);
            result = new_result;
        }

        (result, events)
    }

    /// Filter an outbound output string: replace real values with proxy tokens.
    ///
    /// Returns the filtered string and a list of substitution events.
    pub fn filter_outbound(&self, output: &str) -> (String, Vec<FilterEvent>) {
        let patterns = self
            .outbound_patterns
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let mut result = output.to_string();
        let mut events = Vec::new();
        let ts = now_ms();

        for (real_re, proxy_key) in patterns.iter() {
            let mut offset: i64 = 0;
            let mut new_result = String::with_capacity(result.len());
            let mut last_end = 0;

            for m in real_re.find_iter(&result) {
                new_result.push_str(&result[last_end..m.start()]);
                let real_matched = &result[m.start()..m.end()];
                let format = detect_key_format(proxy_key);
                let pos = (m.start() as i64 + offset).max(0) as usize;
                events.push(FilterEvent {
                    direction: FilterDirection::Outbound,
                    proxy_key: proxy_key.clone(),
                    key_format: format,
                    position: pos,
                    length: proxy_key.len(),
                    timestamp_ms: ts,
                });
                new_result.push_str(proxy_key);
                offset += proxy_key.len() as i64 - real_matched.len() as i64;
                last_end = m.end();
            }
            new_result.push_str(&result[last_end..]);
            result = new_result;
        }

        (result, events)
    }

    /// Total number of registered secrets.
    pub fn secret_count(&self) -> usize {
        self.proxy_to_real
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Return the set of all known real values (for discovery deduplication).
    pub fn known_real_values(&self) -> HashSet<String> {
        self.real_to_proxy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }
}

impl Default for FilterLayer {
    fn default() -> Self {
        Self::new()
    }
}

/// Best-effort heuristic to identify the key format from its value or proxy.
fn detect_key_format(key: &str) -> String {
    if key.starts_with("sk-ant") {
        "anthropic".to_string()
    } else if key.starts_with("sk-") {
        "openai".to_string()
    } else if key.starts_with("ghp_") || key.starts_with("ghs_") {
        "github".to_string()
    } else if key.starts_with("AKIA") {
        "aws".to_string()
    } else if key.starts_with("AIza") {
        "google".to_string()
    } else if key.starts_with("xoxb-") || key.starts_with("xoxp-") {
        "slack".to_string()
    } else {
        "generic".to_string()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_filter() -> FilterLayer {
        let f = FilterLayer::new();
        f.register_secret(
            "sk-ant-api03-REALKEY",
            "sk-ant-PROXY-7f3a9b2c1234567890abcdef12345678",
        );
        f
    }

    #[test]
    fn test_filter_inbound_replaces_proxy() {
        let f = make_filter();
        let (out, events) = f.filter_inbound(
            "curl -H 'Authorization: sk-ant-PROXY-7f3a9b2c1234567890abcdef12345678'",
        );
        assert!(
            out.contains("sk-ant-api03-REALKEY"),
            "inbound should restore real key"
        );
        assert!(!out.contains("PROXY"), "inbound should remove proxy token");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].direction, FilterDirection::Inbound);
    }

    #[test]
    fn test_filter_outbound_masks_real() {
        let f = make_filter();
        let (out, events) = f.filter_outbound("API key: sk-ant-api03-REALKEY found");
        assert!(out.contains("PROXY"), "outbound should mask real key");
        assert!(
            !out.contains("sk-ant-api03-REALKEY"),
            "outbound should not expose real key"
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].direction, FilterDirection::Outbound);
    }

    #[test]
    fn test_no_double_substitution() {
        let f = FilterLayer::new();
        // Register A→B and B→C; inbound should only do A→B, not then B→C
        f.register_secret("REAL_A", "PROXY_A");
        f.register_secret("REAL_B", "PROXY_B");
        let (out, _) = f.filter_inbound("command PROXY_A PROXY_B");
        assert!(out.contains("REAL_A"), "PROXY_A should become REAL_A");
        assert!(out.contains("REAL_B"), "PROXY_B should become REAL_B");
    }

    #[test]
    fn test_multiple_secrets() {
        let f = FilterLayer::new();
        f.register_secret("REAL1", "PROXY1");
        f.register_secret("REAL2", "PROXY2");
        let (out, events) = f.filter_inbound("cmd PROXY1 and PROXY2");
        assert!(out.contains("REAL1"));
        assert!(out.contains("REAL2"));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_empty_string_passthrough() {
        let f = make_filter();
        let (out, events) = f.filter_inbound("");
        assert_eq!(out, "");
        assert!(events.is_empty());
    }

    #[test]
    fn test_no_match_passthrough() {
        let f = make_filter();
        let (out, events) = f.filter_inbound("ls -la /tmp");
        assert_eq!(out, "ls -la /tmp");
        assert!(events.is_empty());
    }

    #[test]
    fn test_special_regex_chars_escaped() {
        let f = FilterLayer::new();
        // Real key contains regex special characters
        f.register_secret("sk-ant+api.REAL*KEY", "PROXY_KEY");
        let (out, events) = f.filter_inbound("use PROXY_KEY here");
        assert!(
            out.contains("sk-ant+api.REAL*KEY"),
            "special chars should be literal"
        );
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_filter_event_accuracy() {
        let f = make_filter();
        let cmd = "curl 'sk-ant-PROXY-7f3a9b2c1234567890abcdef12345678'";
        let (_, events) = f.filter_inbound(cmd);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key_format, "anthropic");
    }

    #[test]
    fn test_empty_map_passthrough() {
        let f = FilterLayer::new();
        let (out, events) = f.filter_outbound("some output with no secrets");
        assert_eq!(out, "some output with no secrets");
        assert!(events.is_empty());
    }

    #[test]
    fn test_secret_count() {
        let f = FilterLayer::new();
        assert_eq!(f.secret_count(), 0);
        f.register_secret("REAL1", "PROXY1");
        assert_eq!(f.secret_count(), 1);
        f.register_secret("REAL2", "PROXY2");
        assert_eq!(f.secret_count(), 2);
        // Re-registering same proxy should not increase count
        f.register_secret("REAL1_UPDATED", "PROXY1");
        assert_eq!(f.secret_count(), 2);
    }

    #[test]
    fn test_sync_from_vault() {
        let f = FilterLayer::new();
        let mut map = HashMap::new();
        map.insert("PROXY_X".to_string(), "REAL_X".to_string());
        map.insert("PROXY_Y".to_string(), "REAL_Y".to_string());
        f.sync_from_vault(&map);
        assert_eq!(f.secret_count(), 2);
        let (out, _) = f.filter_inbound("cmd PROXY_X");
        assert!(out.contains("REAL_X"));
    }
}
