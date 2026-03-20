//! Token bucket rate limiter for webhook processing.
//!
//! Prevents webhook floods and API cost explosions through fine-grained
//! rate limiting at multiple scopes:
//! - Global: 1000 req/min total
//! - Per-user: 100 req/min per user
//! - Per-channel: 500 req/min per channel
//! - Per-IP: 50 req/min (brute force detection)
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Configuration for webhook rate limiting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookRateLimitConfig {
    /// Global limit: total requests per minute
    #[serde(default = "default_global_limit")]
    pub global_limit: u32,
    /// Per-user limit: requests per minute per user
    #[serde(default = "default_per_user_limit")]
    pub per_user_limit: u32,
    /// Per-channel limit: requests per minute per channel
    #[serde(default = "default_per_channel_limit")]
    pub per_channel_limit: u32,
    /// Per-IP limit: requests per minute per IP (brute force detection)
    #[serde(default = "default_per_ip_limit")]
    pub per_ip_limit: u32,
    /// Burst allowance: allow 1.5x normal rate for short bursts
    #[serde(default = "default_burst_allowance")]
    pub burst_allowance: f32,
}

fn default_global_limit() -> u32 {
    1000
}
fn default_per_user_limit() -> u32 {
    100
}
fn default_per_channel_limit() -> u32 {
    500
}
fn default_per_ip_limit() -> u32 {
    50
}
fn default_burst_allowance() -> f32 {
    1.5
}

impl Default for WebhookRateLimitConfig {
    fn default() -> Self {
        Self {
            global_limit: default_global_limit(),
            per_user_limit: default_per_user_limit(),
            per_channel_limit: default_per_channel_limit(),
            per_ip_limit: default_per_ip_limit(),
            burst_allowance: default_burst_allowance(),
        }
    }
}

/// Token bucket for rate limiting.
///
/// Tokens refill at a constant rate (tokens_per_second).
/// Each request costs 1 token. If bucket is empty, request is rejected.
#[derive(Debug, Clone)]
struct TokenBucket {
    /// Maximum tokens the bucket can hold
    capacity: u32,
    /// Current tokens available
    tokens: f32,
    /// Tokens added per second
    tokens_per_second: f32,
    /// Last time tokens were refilled
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new token bucket.
    fn new(capacity: u32, tokens_per_second: f32) -> Self {
        Self {
            capacity,
            tokens: capacity as f32,
            tokens_per_second,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens based on elapsed time.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f32();
        let new_tokens = self.tokens + (elapsed * self.tokens_per_second);

        self.tokens = new_tokens.min(self.capacity as f32);
        self.last_refill = now;
    }

    /// Try to consume 1 token. Returns true if successful, false if bucket is empty.
    fn try_consume(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Get remaining tokens (refilled as of now).
    fn remaining(&mut self) -> u32 {
        self.refill();
        self.tokens.floor() as u32
    }
}

/// Rate limit check result.
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    pub allowed: bool,
    pub global_remaining: u32,
    pub user_remaining: u32,
    pub channel_remaining: u32,
    pub ip_remaining: u32,
    pub limit_exceeded: Option<String>, // Which limit was exceeded (if any)
}

/// Multi-scope webhook rate limiter.
pub struct WebhookRateLimiter {
    config: WebhookRateLimitConfig,
    /// Global bucket (all requests)
    global_bucket: Arc<RwLock<TokenBucket>>,
    /// Per-user buckets
    user_buckets: Arc<RwLock<HashMap<String, TokenBucket>>>,
    /// Per-channel buckets
    channel_buckets: Arc<RwLock<HashMap<String, TokenBucket>>>,
    /// Per-IP buckets
    ip_buckets: Arc<RwLock<HashMap<IpAddr, TokenBucket>>>,
}

impl WebhookRateLimiter {
    /// Create a new webhook rate limiter.
    pub fn new(config: WebhookRateLimitConfig) -> Self {
        // Refill rate = limit / 60 (convert per-minute to per-second)
        let global_refill = config.global_limit as f32 / 60.0;
        let _user_refill = config.per_user_limit as f32 / 60.0;
        let _channel_refill = config.per_channel_limit as f32 / 60.0;
        let _ip_refill = config.per_ip_limit as f32 / 60.0;

        Self {
            global_bucket: Arc::new(RwLock::new(TokenBucket::new(
                config.global_limit,
                global_refill,
            ))),
            config,
            user_buckets: Arc::new(RwLock::new(HashMap::new())),
            channel_buckets: Arc::new(RwLock::new(HashMap::new())),
            ip_buckets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Check if a request is allowed under rate limiting.
    pub async fn check_limit(&self, user_id: &str, channel: &str, ip: IpAddr) -> RateLimitResult {
        // Check global limit
        let mut global = self.global_bucket.write().await;
        if !global.try_consume() {
            debug!(
                "[RATELIMIT] Global limit exceeded, remaining={}",
                global.remaining()
            );
            return RateLimitResult {
                allowed: false,
                global_remaining: 0,
                user_remaining: 0,
                channel_remaining: 0,
                ip_remaining: 0,
                limit_exceeded: Some("global".to_string()),
            };
        }
        let global_remaining = global.remaining();
        drop(global);

        // Check per-user limit
        let mut users = self.user_buckets.write().await;
        let user_bucket = users.entry(user_id.to_string()).or_insert_with(|| {
            TokenBucket::new(
                self.config.per_user_limit,
                self.config.per_user_limit as f32 / 60.0,
            )
        });

        if !user_bucket.try_consume() {
            debug!(
                "[RATELIMIT] User limit exceeded for user={}, remaining={}",
                user_id,
                user_bucket.remaining()
            );
            return RateLimitResult {
                allowed: false,
                global_remaining,
                user_remaining: 0,
                channel_remaining: 0,
                ip_remaining: 0,
                limit_exceeded: Some(format!("user:{}", user_id)),
            };
        }
        let user_remaining = user_bucket.remaining();
        drop(users);

        // Check per-channel limit
        let mut channels = self.channel_buckets.write().await;
        let channel_bucket = channels.entry(channel.to_string()).or_insert_with(|| {
            TokenBucket::new(
                self.config.per_channel_limit,
                self.config.per_channel_limit as f32 / 60.0,
            )
        });

        if !channel_bucket.try_consume() {
            debug!(
                "[RATELIMIT] Channel limit exceeded for channel={}, remaining={}",
                channel,
                channel_bucket.remaining()
            );
            return RateLimitResult {
                allowed: false,
                global_remaining,
                user_remaining,
                channel_remaining: 0,
                ip_remaining: 0,
                limit_exceeded: Some(format!("channel:{}", channel)),
            };
        }
        let channel_remaining = channel_bucket.remaining();
        drop(channels);

        // Check per-IP limit (brute force detection)
        let mut ips = self.ip_buckets.write().await;
        let ip_bucket = ips.entry(ip).or_insert_with(|| {
            TokenBucket::new(
                self.config.per_ip_limit,
                self.config.per_ip_limit as f32 / 60.0,
            )
        });

        if !ip_bucket.try_consume() {
            warn!(
                "[RATELIMIT] IP limit exceeded (possible brute force), ip={}, remaining={}",
                ip,
                ip_bucket.remaining()
            );
            return RateLimitResult {
                allowed: false,
                global_remaining,
                user_remaining,
                channel_remaining,
                ip_remaining: 0,
                limit_exceeded: Some(format!("ip:{}", ip)),
            };
        }
        let ip_remaining = ip_bucket.remaining();
        drop(ips);

        debug!(
            "[RATELIMIT] Request allowed, global={}, user={}, channel={}, ip={}",
            global_remaining, user_remaining, channel_remaining, ip_remaining
        );

        RateLimitResult {
            allowed: true,
            global_remaining,
            user_remaining,
            channel_remaining,
            ip_remaining,
            limit_exceeded: None,
        }
    }

    /// Cleanup stale buckets (to prevent unbounded memory growth).
    pub async fn cleanup_stale(&self) {
        let stale_threshold = Duration::from_secs(3600); // 1 hour

        // Cleanup user buckets
        {
            let mut users = self.user_buckets.write().await;
            let before = users.len();
            users.retain(|_, bucket| bucket.last_refill.elapsed() < stale_threshold);
            if users.len() < before {
                debug!(
                    "[RATELIMIT] Cleaned user buckets: before={}, after={}",
                    before,
                    users.len()
                );
            }
        }

        // Cleanup channel buckets
        {
            let mut channels = self.channel_buckets.write().await;
            let before = channels.len();
            channels.retain(|_, bucket| bucket.last_refill.elapsed() < stale_threshold);
            if channels.len() < before {
                debug!(
                    "[RATELIMIT] Cleaned channel buckets: before={}, after={}",
                    before,
                    channels.len()
                );
            }
        }

        // Cleanup IP buckets
        {
            let mut ips = self.ip_buckets.write().await;
            let before = ips.len();
            ips.retain(|_, bucket| bucket.last_refill.elapsed() < stale_threshold);
            if ips.len() < before {
                debug!(
                    "[RATELIMIT] Cleaned IP buckets: before={}, after={}",
                    before,
                    ips.len()
                );
            }
        }
    }

    /// Get current statistics.
    pub async fn get_stats(&self) -> RateLimitStats {
        RateLimitStats {
            user_buckets: self.user_buckets.read().await.len(),
            channel_buckets: self.channel_buckets.read().await.len(),
            ip_buckets: self.ip_buckets.read().await.len(),
        }
    }
}

/// Rate limiting statistics.
#[derive(Debug, Clone)]
pub struct RateLimitStats {
    pub user_buckets: usize,
    pub channel_buckets: usize,
    pub ip_buckets: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_bucket_refill() {
        let mut bucket = TokenBucket::new(10, 2.0); // 2 tokens/sec, capacity 10

        // Consume all tokens
        assert!(bucket.try_consume()); // 9 left
        assert!(bucket.try_consume()); // 8 left
        assert_eq!(bucket.tokens.floor() as u32, 8);

        // Sleep 1 second to refill
        std::thread::sleep(Duration::from_secs(1));
        bucket.refill();

        // Should have ~10 tokens (8 + 2)
        assert!(bucket.tokens >= 9.0);
    }

    #[test]
    fn test_bucket_capacity_limit() {
        let mut bucket = TokenBucket::new(5, 10.0); // 10 tokens/sec, capacity 5

        // Sleep to allow refill, should cap at capacity
        std::thread::sleep(Duration::from_millis(100));
        bucket.refill();

        // Should be capped at 5 (capacity), not 5 + (10 tokens/sec * 0.1sec)
        assert!(bucket.tokens <= 5.0);
    }

    #[tokio::test]
    async fn test_webhook_rate_limiter() {
        let config = WebhookRateLimitConfig {
            global_limit: 100,
            per_user_limit: 20,
            per_channel_limit: 50,
            per_ip_limit: 100, // must exceed per_user_limit so the user limit is what triggers
            burst_allowance: 1.5,
        };
        let limiter = WebhookRateLimiter::new(config);

        // First request should pass
        let result = limiter
            .check_limit("user1", "telegram", "127.0.0.1".parse().unwrap())
            .await;
        assert!(result.allowed, "First request should be allowed");
        assert_eq!(result.user_remaining, 19); // 20 - 1

        // Multiple requests from same user
        for _ in 0..19 {
            let result = limiter
                .check_limit("user1", "telegram", "127.0.0.1".parse().unwrap())
                .await;
            assert!(result.allowed);
        }

        // 20th request should exceed per-user limit
        let result = limiter
            .check_limit("user1", "telegram", "127.0.0.1".parse().unwrap())
            .await;
        assert!(!result.allowed);
        assert_eq!(result.limit_exceeded.as_deref(), Some("user:user1"));
    }

    #[tokio::test]
    async fn test_multi_user_isolation() {
        let limiter = WebhookRateLimiter::new(WebhookRateLimitConfig {
            global_limit: 1000,
            per_user_limit: 10,
            per_channel_limit: 500,
            per_ip_limit: 50,
            burst_allowance: 1.5,
        });

        let ip = "127.0.0.1".parse().unwrap();

        // User1 makes 10 requests (hits limit)
        for _ in 0..10 {
            let result = limiter.check_limit("user1", "telegram", ip).await;
            assert!(result.allowed);
        }

        // User1's 11th request should fail
        let result = limiter.check_limit("user1", "telegram", ip).await;
        assert!(!result.allowed);

        // User2 should still be able to make requests
        let result = limiter.check_limit("user2", "telegram", ip).await;
        assert!(
            result.allowed,
            "User2 should not be affected by User1's limit"
        );
    }

    #[tokio::test]
    async fn test_cleanup_stale() {
        let limiter = WebhookRateLimiter::new(WebhookRateLimitConfig::default());

        // Create some buckets
        for i in 0..5 {
            let user = format!("user{}", i);
            limiter
                .check_limit(&user, "telegram", "127.0.0.1".parse().unwrap())
                .await;
        }

        let stats = limiter.get_stats().await;
        assert_eq!(stats.user_buckets, 5);

        // Cleanup (won't remove recent buckets)
        limiter.cleanup_stale().await;

        let stats = limiter.get_stats().await;
        assert_eq!(
            stats.user_buckets, 5,
            "Recent buckets should not be cleaned"
        );
    }
}
