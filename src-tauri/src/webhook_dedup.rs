//! Webhook deduplication system.
//!
//! Prevents duplicate webhook events from triggering multiple agent responses.
//! Uses a two-stage approach:
//! 1. Event ID tracking (Telegram message_id, Discord message_id, etc.)
//! 2. Content hash fallback (for events without unique IDs)
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Configuration for webhook deduplication.
#[derive(Debug, Clone)]
pub struct DedupConfig {
    /// Whether deduplication is enabled.
    pub enabled: bool,
    /// Event ID cache TTL in seconds (default: 86400 = 24 hours).
    pub event_id_ttl_secs: u64,
    /// Content hash window in seconds (default: 600 = 10 minutes).
    pub content_hash_window_secs: u64,
    /// Maximum entries in event ID cache before cleanup.
    pub max_cache_size: usize,
}

impl Default for DedupConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            event_id_ttl_secs: 86_400,     // 24 hours
            content_hash_window_secs: 600, // 10 minutes
            max_cache_size: 100_000,
        }
    }
}

/// Cached event for deduplication.
#[derive(Debug, Clone)]
struct CachedEvent {
    timestamp: Instant,
    event_id: String,
}

/// Content hash entry.
#[derive(Debug, Clone)]
struct ContentHashEntry {
    timestamp: Instant,
}

/// Webhook deduplicator using two-stage approach.
pub struct WebhookDeduplicator {
    config: DedupConfig,
    /// Event ID cache: "channel:user_id:event_id" → Instant
    event_id_cache: Arc<RwLock<HashMap<String, CachedEvent>>>,
    /// Content hash cache: hash(u64) → Instant
    content_hash_cache: Arc<RwLock<HashMap<u64, ContentHashEntry>>>,
}

impl WebhookDeduplicator {
    /// Create a new deduplicator with the given configuration.
    pub fn new(config: DedupConfig) -> Self {
        Self {
            config,
            event_id_cache: Arc::new(RwLock::new(HashMap::new())),
            content_hash_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Check if a webhook event is a duplicate.
    ///
    /// Returns:
    /// - `Ok(true)` if the event is NOT a duplicate (should process)
    /// - `Ok(false)` if the event IS a duplicate (should skip)
    /// - `Err(msg)` if deduplication check failed
    pub async fn check_and_record(
        &self,
        channel: &str,
        user_id: &str,
        event_id: Option<&str>,
        content: &str,
    ) -> Result<bool, String> {
        if !self.config.enabled {
            return Ok(true); // Dedup disabled, always process
        }

        // Stage 1: Event ID check (most specific, highest confidence)
        if let Some(id) = event_id {
            let cache_key = format!("{}:{}:{}", channel, user_id, id);
            return self.check_event_id_cache(&cache_key).await;
        }

        // Stage 2: Content hash fallback (for events without event_id)
        let content_hash = hash_content(content);
        self.check_content_hash_cache(content_hash).await
    }

    /// Check event ID cache (24-hour TTL).
    async fn check_event_id_cache(&self, key: &str) -> Result<bool, String> {
        let mut cache = self.event_id_cache.write().await;

        // Check if key exists and hasn't expired
        if let Some(entry) = cache.get(key) {
            let age = entry.timestamp.elapsed().as_secs();
            if age < self.config.event_id_ttl_secs {
                debug!(
                    "[DEDUP] Event ID found in cache, age_secs={}, status=duplicate",
                    age
                );
                return Ok(false); // Duplicate!
            }
        }

        // Not in cache or expired, add it
        cache.insert(
            key.to_string(),
            CachedEvent {
                timestamp: Instant::now(),
                event_id: key.to_string(),
            },
        );

        // Cleanup if cache is too large
        if cache.len() > self.config.max_cache_size {
            self.cleanup_event_id_cache(&mut cache);
        }

        debug!(
            "[DEDUP] Event ID not in cache, status=new, cache_size={}",
            cache.len()
        );
        Ok(true) // Not a duplicate
    }

    /// Check content hash cache (10-minute window).
    async fn check_content_hash_cache(&self, hash: u64) -> Result<bool, String> {
        let mut cache = self.content_hash_cache.write().await;

        // Check if hash exists within the time window
        if let Some(entry) = cache.get(&hash) {
            let age_secs = entry.timestamp.elapsed().as_secs();
            if age_secs < self.config.content_hash_window_secs {
                debug!(
                    "[DEDUP] Content hash found in window (age={}s), status=duplicate",
                    age_secs
                );
                return Ok(false); // Duplicate within window!
            }
        }

        // Not in window or expired, add it
        cache.insert(
            hash,
            ContentHashEntry {
                timestamp: Instant::now(),
            },
        );

        debug!(
            "[DEDUP] Content hash not in window, status=new, cache_size={}",
            cache.len()
        );
        Ok(true) // Not a duplicate
    }

    /// Cleanup event ID cache by removing oldest entries.
    fn cleanup_event_id_cache(&self, cache: &mut HashMap<String, CachedEvent>) {
        let target_size = self.config.max_cache_size / 2; // Remove oldest 50%
        let mut entries: Vec<_> = cache.iter().collect();
        entries.sort_by_key(|(_, entry)| entry.timestamp);

        let to_remove = entries.len() - target_size;
        let keys_to_remove: Vec<String> = entries
            .iter()
            .take(to_remove)
            .map(|(k, _)| (*k).clone())
            .collect();
        drop(entries);
        for key in keys_to_remove {
            cache.remove(&key);
        }

        info!(
            "[DEDUP] Cleaned up event ID cache, removed={}, remaining={}",
            to_remove,
            cache.len()
        );
    }

    /// Get cache statistics (for monitoring).
    pub async fn get_stats(&self) -> DedupStats {
        let event_cache = self.event_id_cache.read().await;
        let content_cache = self.content_hash_cache.read().await;

        DedupStats {
            event_id_cache_size: event_cache.len(),
            content_hash_cache_size: content_cache.len(),
            config_enabled: self.config.enabled,
        }
    }

    /// Cleanup stale entries from both caches.
    pub async fn cleanup_stale(&self) {
        // Event ID cache cleanup
        {
            let mut cache = self.event_id_cache.write().await;
            let before = cache.len();
            let now = Instant::now();
            let ttl = Duration::from_secs(self.config.event_id_ttl_secs);

            cache.retain(|_, entry| now.duration_since(entry.timestamp) < ttl);

            debug!(
                "[DEDUP] Event ID cache cleanup: before={}, after={}, removed={}",
                before,
                cache.len(),
                before - cache.len()
            );
        }

        // Content hash cache cleanup
        {
            let mut cache = self.content_hash_cache.write().await;
            let before = cache.len();
            let now = Instant::now();
            let window = Duration::from_secs(self.config.content_hash_window_secs);

            cache.retain(|_, entry| now.duration_since(entry.timestamp) < window);

            debug!(
                "[DEDUP] Content hash cleanup: before={}, after={}, removed={}",
                before,
                cache.len(),
                before - cache.len()
            );
        }
    }
}

/// Deduplication statistics.
#[derive(Debug, Clone)]
pub struct DedupStats {
    pub event_id_cache_size: usize,
    pub content_hash_cache_size: usize,
    pub config_enabled: bool,
}

/// Hash content using FNV-1a algorithm (fast, good distribution).
fn hash_content(content: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in content.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_id_dedup() {
        let config = DedupConfig {
            enabled: true,
            event_id_ttl_secs: 10,
            content_hash_window_secs: 10,
            max_cache_size: 1000,
        };
        let dedup = WebhookDeduplicator::new(config);

        // First event should pass
        let result = dedup
            .check_and_record("telegram", "user_123", Some("msg_001"), "Hello")
            .await;
        assert_eq!(result, Ok(true), "First event should not be duplicate");

        // Same event should be filtered
        let result = dedup
            .check_and_record("telegram", "user_123", Some("msg_001"), "Hello")
            .await;
        assert_eq!(result, Ok(false), "Same event should be duplicate");

        // Different user, same message ID should pass
        let result = dedup
            .check_and_record("telegram", "user_456", Some("msg_001"), "Hello")
            .await;
        assert_eq!(
            result,
            Ok(true),
            "Different user with same message ID should pass"
        );
    }

    #[tokio::test]
    async fn test_content_hash_dedup() {
        let config = DedupConfig {
            enabled: true,
            event_id_ttl_secs: 10,
            content_hash_window_secs: 10,
            max_cache_size: 1000,
        };
        let dedup = WebhookDeduplicator::new(config);

        // First message without event_id should pass
        let result = dedup
            .check_and_record("discord", "user_789", None, "Same content")
            .await;
        assert_eq!(result, Ok(true), "First message should pass");

        // Same content should be duplicate within window
        let result = dedup
            .check_and_record("discord", "user_789", None, "Same content")
            .await;
        assert_eq!(result, Ok(false), "Same content should be duplicate");

        // Different content should pass
        let result = dedup
            .check_and_record("discord", "user_789", None, "Different content")
            .await;
        assert_eq!(result, Ok(true), "Different content should pass");
    }

    #[tokio::test]
    async fn test_disabled_dedup() {
        let config = DedupConfig {
            enabled: false,
            ..Default::default()
        };
        let dedup = WebhookDeduplicator::new(config);

        // Should always return true when disabled
        let result1 = dedup
            .check_and_record("telegram", "user_123", Some("msg_001"), "Hello")
            .await;
        let result2 = dedup
            .check_and_record("telegram", "user_123", Some("msg_001"), "Hello")
            .await;

        assert_eq!(result1, Ok(true), "Should process when disabled");
        assert_eq!(result2, Ok(true), "Should process duplicate when disabled");
    }

    #[test]
    fn test_fnv_hash() {
        // FNV-1a should give consistent hashes
        let hash1 = hash_content("test message");
        let hash2 = hash_content("test message");
        assert_eq!(hash1, hash2, "Same content should hash the same");

        // Different content should (usually) hash differently
        let hash3 = hash_content("different message");
        assert_ne!(hash1, hash3, "Different content should hash differently");
    }

    #[tokio::test]
    async fn test_cleanup_stale() {
        let config = DedupConfig {
            enabled: true,
            event_id_ttl_secs: 1, // Very short TTL for testing
            content_hash_window_secs: 1,
            max_cache_size: 1000,
        };
        let dedup = WebhookDeduplicator::new(config);

        // Add an event
        dedup
            .check_and_record("telegram", "user_123", Some("msg_001"), "Hello")
            .await
            .ok();

        // Verify it's in cache
        let stats = dedup.get_stats().await;
        assert!(stats.event_id_cache_size > 0, "Event should be in cache");

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Cleanup stale entries
        dedup.cleanup_stale().await;

        // Verify stale entries are removed
        let stats = dedup.get_stats().await;
        assert_eq!(
            stats.event_id_cache_size, 0,
            "Stale entries should be cleaned"
        );
    }
}
