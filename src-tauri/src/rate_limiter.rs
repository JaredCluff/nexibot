//! Sliding-window rate limiter for webhook authentication failures.
//!
//! Tracks failed authentication attempts per IP address and blocks
//! IPs that exceed the configured threshold.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::warn;

/// Rate limit configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum failed attempts before lockout
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// Sliding window duration in seconds
    #[serde(default = "default_window_secs")]
    pub window_secs: u64,
    /// Lockout duration in seconds after exceeding max_attempts
    #[serde(default = "default_lockout_secs")]
    pub lockout_secs: u64,
}

fn default_max_attempts() -> u32 {
    10
}
fn default_window_secs() -> u64 {
    60
}
fn default_lockout_secs() -> u64 {
    300
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            window_secs: default_window_secs(),
            lockout_secs: default_lockout_secs(),
        }
    }
}

/// Per-IP tracking record.
struct IpRecord {
    /// Timestamps of failed attempts within the window.
    failures: Vec<Instant>,
    /// If locked out, when the lockout expires.
    locked_until: Option<Instant>,
}

/// Maximum number of tracked IP records before emergency cleanup.
const MAX_RECORDS: usize = 10_000;

/// Sliding-window rate limiter.
pub struct RateLimiter {
    config: RateLimitConfig,
    records: RwLock<HashMap<IpAddr, IpRecord>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            records: RwLock::new(HashMap::new()),
        }
    }

    /// Record an authentication failure from the given IP.
    /// Returns `true` if the IP is now blocked.
    pub async fn record_failure(&self, ip: IpAddr) -> bool {
        // Localhost is always exempt
        if Self::is_exempt(ip) {
            return false;
        }

        let now = Instant::now();
        let window = std::time::Duration::from_secs(self.config.window_secs);
        let mut records = self.records.write().await;

        let record = records.entry(ip).or_insert_with(|| IpRecord {
            failures: Vec::new(),
            locked_until: None,
        });

        // If already locked, check if lockout has expired
        if let Some(locked_until) = record.locked_until {
            if now < locked_until {
                return true; // still blocked
            }
            // Lockout expired, reset
            record.locked_until = None;
            record.failures.clear();
        }

        // Prune old failures outside the window
        record.failures.retain(|&t| now.duration_since(t) < window);

        // Record this failure
        record.failures.push(now);

        // Check if threshold exceeded
        if record.failures.len() >= self.config.max_attempts as usize {
            let lockout_duration = std::time::Duration::from_secs(self.config.lockout_secs);
            record.locked_until = Some(now + lockout_duration);
            warn!(
                "[RATE_LIMIT] IP {} locked out for {}s after {} failures",
                ip,
                self.config.lockout_secs,
                record.failures.len()
            );

            // Emergency cleanup if map exceeds hard cap
            Self::enforce_max_records(&mut records, &self.config, now);

            return true;
        }

        // Emergency cleanup if map exceeds hard cap
        Self::enforce_max_records(&mut records, &self.config, now);

        false
    }

    /// Check if an IP is currently blocked.
    pub async fn is_blocked(&self, ip: IpAddr) -> bool {
        if Self::is_exempt(ip) {
            return false;
        }

        let now = Instant::now();
        let records = self.records.read().await;

        if let Some(record) = records.get(&ip) {
            if let Some(locked_until) = record.locked_until {
                return now < locked_until;
            }
        }

        false
    }

    /// Periodically clean up stale records (call from a background task).
    pub async fn cleanup_stale(&self) {
        let now = Instant::now();
        let window = std::time::Duration::from_secs(self.config.window_secs);
        let mut records = self.records.write().await;

        records.retain(|_ip, record| {
            // Keep if locked out and lockout hasn't expired
            if let Some(locked_until) = record.locked_until {
                if now < locked_until {
                    return true;
                }
            }
            // Keep if there are recent failures
            record.failures.retain(|&t| now.duration_since(t) < window);
            !record.failures.is_empty()
        });
    }

    /// Enforce the MAX_RECORDS hard cap on the records map.
    /// First removes expired entries; if still over limit, logs a warning.
    fn enforce_max_records(
        records: &mut HashMap<IpAddr, IpRecord>,
        config: &RateLimitConfig,
        now: Instant,
    ) {
        if records.len() <= MAX_RECORDS {
            return;
        }

        let window = std::time::Duration::from_secs(config.window_secs);

        // Remove expired entries
        records.retain(|_ip, record| {
            if let Some(locked_until) = record.locked_until {
                if now < locked_until {
                    return true;
                }
            }
            record.failures.retain(|&t| now.duration_since(t) < window);
            !record.failures.is_empty()
        });

        if records.len() > MAX_RECORDS {
            warn!(
                "[RATE_LIMIT] Records map still at {} after emergency cleanup (cap: {})",
                records.len(),
                MAX_RECORDS
            );
        }
    }

    /// Check if an IP is exempt from rate limiting.
    fn is_exempt(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => v4.is_loopback(),
            IpAddr::V6(v6) => v6.is_loopback(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn test_localhost_exempt() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_attempts: 1,
            window_secs: 60,
            lockout_secs: 300,
        });
        let localhost = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(!limiter.record_failure(localhost).await);
        assert!(!limiter.is_blocked(localhost).await);
    }

    #[tokio::test]
    async fn test_blocking_after_threshold() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_attempts: 3,
            window_secs: 60,
            lockout_secs: 300,
        });
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        assert!(!limiter.record_failure(ip).await);
        assert!(!limiter.record_failure(ip).await);
        assert!(limiter.record_failure(ip).await);
        assert!(limiter.is_blocked(ip).await);
    }

    #[tokio::test]
    async fn test_default_config() {
        let config = RateLimitConfig::default();
        assert_eq!(config.max_attempts, 10);
        assert_eq!(config.window_secs, 60);
        assert_eq!(config.lockout_secs, 300);
    }
}
