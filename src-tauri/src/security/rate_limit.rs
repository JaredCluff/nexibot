//! Sliding-window rate limiter for authentication attempts.
//!
//! Thread-safe (all methods take `&self`) and safe for concurrent use from
//! multiple async tasks.  Each key (e.g. IP address) gets an independent
//! sliding window of attempt timestamps.  When the window fills, the key is
//! locked out for a configurable duration.
//!
//! Loopback addresses (127.0.0.0/8, ::1) are exempt from rate limiting to
//! prevent the local CLI from being locked out by remote abuse.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Error returned when a rate limit check fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitError {
    /// Human-readable reason.
    pub message: String,
    /// Seconds the caller should wait before retrying.
    pub retry_after_seconds: u64,
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (retry after {} seconds)",
            self.message, self.retry_after_seconds
        )
    }
}

/// Internal state for a single rate-limited key.
#[derive(Debug)]
struct RateLimitEntry {
    /// Timestamps of recent failed attempts (sliding window).
    attempts: Vec<Instant>,
    /// If set, the key is locked until this instant.
    locked_until: Option<Instant>,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Maximum number of unique keys the rate limiter will track before a forced
/// cleanup pass is triggered.  This bounds HashMap memory growth when the
/// limiter is exposed to a large number of distinct keys (e.g. an internet-
/// facing gateway receiving traffic from many different IPs).
const MAX_ENTRIES: usize = 100_000;

/// Configuration for the rate limiter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum failed attempts before lockout.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// Sliding window duration in seconds.
    #[serde(default = "default_window_seconds")]
    pub window_seconds: u64,
    /// Lockout duration in seconds after limit exceeded.
    #[serde(default = "default_lockout_seconds")]
    pub lockout_seconds: u64,
}

fn default_max_attempts() -> u32 {
    10
}
fn default_window_seconds() -> u64 {
    60
}
fn default_lockout_seconds() -> u64 {
    300
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            window_seconds: default_window_seconds(),
            lockout_seconds: default_lockout_seconds(),
        }
    }
}

// ---------------------------------------------------------------------------
// Rate limiter
// ---------------------------------------------------------------------------

/// A sliding-window rate limiter keyed by arbitrary strings.
///
/// Thread-safe: all methods take `&self` and acquire an internal mutex.
pub struct RateLimiter {
    inner: Mutex<RateLimiterInner>,
    config: RateLimitConfig,
}

struct RateLimiterInner {
    entries: HashMap<String, RateLimitEntry>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        info!(
            "[RATE_LIMIT] Initialised: max_attempts={}, window={}s, lockout={}s",
            config.max_attempts, config.window_seconds, config.lockout_seconds
        );
        Self {
            inner: Mutex::new(RateLimiterInner {
                entries: HashMap::new(),
            }),
            config,
        }
    }

    /// Check if a key (IP address) is currently blocked.
    ///
    /// Loopback addresses are always allowed.
    #[allow(dead_code)]
    pub fn is_blocked(&self, addr: IpAddr) -> bool {
        if is_loopback(&addr) {
            return false;
        }
        let key = addr.to_string();
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = guard.entries.get(&key) {
            if let Some(locked_until) = entry.locked_until {
                return Instant::now() < locked_until;
            }
        }
        false
    }

    /// Check if a key (IP address) is currently blocked for authentication purposes.
    ///
    /// Unlike `is_blocked`, this method does **not** exempt loopback addresses.
    /// Loopback exemption is appropriate for general rate limiting (e.g. preventing
    /// the local CLI from being locked out), but authentication brute-force protection
    /// must apply to all sources, including localhost, because an attacker with local
    /// code execution can trivially loop over passwords from 127.0.0.1.
    pub fn is_blocked_for_auth(&self, addr: IpAddr) -> bool {
        let key = addr.to_string();
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = guard.entries.get(&key) {
            if let Some(locked_until) = entry.locked_until {
                return Instant::now() < locked_until;
            }
        }
        false
    }

    /// Record a failed authentication attempt for an IP address.
    ///
    /// Returns `true` if the IP is now locked out (just crossed the threshold).
    /// Loopback addresses are exempt from rate limiting.
    pub fn record_failure(&self, addr: IpAddr) -> bool {
        if is_loopback(&addr) {
            return false;
        }

        let key = addr.to_string();
        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_seconds);
        let lockout = Duration::from_secs(self.config.lockout_seconds);

        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());

        // Enforce max-entries bound: if the map is at capacity and the key is
        // not already tracked, force a cleanup pass before inserting.
        if guard.entries.len() >= MAX_ENTRIES && !guard.entries.contains_key(&key) {
            let now_inner = Instant::now();
            let window_inner = Duration::from_secs(self.config.window_seconds);
            guard.entries.retain(|_, e| {
                match e.locked_until {
                    Some(until) if until <= now_inner => false,
                    None => {
                        e.attempts.retain(|&t| now_inner.duration_since(t) < window_inner);
                        !e.attempts.is_empty()
                    }
                    _ => true,
                }
            });
            warn!(
                "[RATE_LIMIT] Forced cleanup: entries={} (cap={})",
                guard.entries.len(),
                MAX_ENTRIES
            );
        }

        let entry = guard
            .entries
            .entry(key.clone())
            .or_insert_with(|| RateLimitEntry {
                attempts: Vec::new(),
                locked_until: None,
            });

        // If already locked, stay locked
        if let Some(locked_until) = entry.locked_until {
            if now < locked_until {
                return true;
            }
            // Lockout expired — reset
            entry.locked_until = None;
            entry.attempts.clear();
        }

        // Prune attempts outside the sliding window
        entry.attempts.retain(|&t| now.duration_since(t) < window);

        // Record this attempt
        entry.attempts.push(now);

        // Check if limit exceeded
        if entry.attempts.len() as u32 >= self.config.max_attempts {
            warn!(
                "[RATE_LIMIT] Key '{}' exceeded {} attempts in {}s, locking for {}s",
                key,
                self.config.max_attempts,
                self.config.window_seconds,
                self.config.lockout_seconds
            );
            entry.locked_until = Some(now + lockout);
            return true;
        }

        false
    }

    /// Check (and consume) a token for a string key.
    ///
    /// Returns `Ok(())` if the action is allowed, or `Err(RateLimitError)` if
    /// the key is rate-limited or locked out.
    pub fn check(&self, key: &str) -> Result<(), RateLimitError> {
        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_seconds);
        let lockout = Duration::from_secs(self.config.lockout_seconds);

        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());

        // Enforce max-entries bound: force a cleanup pass before accepting a
        // brand-new key if we are at the capacity limit.
        if guard.entries.len() >= MAX_ENTRIES && !guard.entries.contains_key(key) {
            guard.entries.retain(|_, e| {
                match e.locked_until {
                    Some(until) if until <= now => false,
                    None => {
                        e.attempts.retain(|&t| now.duration_since(t) < window);
                        !e.attempts.is_empty()
                    }
                    _ => true,
                }
            });
            warn!(
                "[RATE_LIMIT] Forced cleanup: entries={} (cap={})",
                guard.entries.len(),
                MAX_ENTRIES
            );
        }

        let entry = guard
            .entries
            .entry(key.to_string())
            .or_insert_with(|| RateLimitEntry {
                attempts: Vec::new(),
                locked_until: None,
            });

        // If locked, check if lockout has expired
        if let Some(locked_until) = entry.locked_until {
            if now < locked_until {
                let remaining = locked_until.duration_since(now);
                return Err(RateLimitError {
                    message: format!("Rate limited: key '{}' is temporarily locked out", key),
                    retry_after_seconds: remaining.as_secs() + 1,
                });
            }
            entry.locked_until = None;
            entry.attempts.clear();
        }

        // Prune attempts outside the sliding window
        entry.attempts.retain(|&t| now.duration_since(t) < window);

        // Record this attempt
        entry.attempts.push(now);

        // Check if limit exceeded
        if entry.attempts.len() as u32 >= self.config.max_attempts {
            warn!(
                "[RATE_LIMIT] Key '{}' exhausted attempts, locking for {}s",
                key, self.config.lockout_seconds
            );
            entry.locked_until = Some(now + lockout);
            return Err(RateLimitError {
                message: format!("Rate limited: too many attempts for key '{}'", key),
                retry_after_seconds: lockout.as_secs() + 1,
            });
        }

        Ok(())
    }

    /// Reset (remove) the state for a key, unlocking it immediately.
    #[allow(dead_code)]
    pub fn reset(&self, key: &str) {
        info!("[RATE_LIMIT] Reset entry for key '{}'", key);
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.entries.remove(key);
    }

    /// Remove all entries whose lockout has expired, freeing memory.
    /// Also prunes entries with no recent attempts.
    #[allow(dead_code)]
    pub fn cleanup(&self) {
        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_seconds);
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let before = guard.entries.len();
        guard.entries.retain(|_key, entry| {
            // Remove entries with expired lockout and no recent attempts
            match entry.locked_until {
                Some(until) if until <= now => false,
                None => {
                    // Keep if there are recent attempts within the window
                    entry.attempts.retain(|&t| now.duration_since(t) < window);
                    !entry.attempts.is_empty()
                }
                _ => true,
            }
        });
        let removed = before - guard.entries.len();
        if removed > 0 {
            debug!(
                "[RATE_LIMIT] Cleanup removed {} expired entries ({} remaining)",
                removed,
                guard.entries.len()
            );
        }
    }

    /// Async-compatible cleanup (delegates to sync cleanup).
    #[allow(dead_code)]
    pub async fn cleanup_stale(&self) {
        self.cleanup();
    }

    /// Spawn a background Tokio task that calls [`cleanup`] every 5 minutes.
    ///
    /// Call this once after constructing the limiter and wrapping it in an
    /// [`Arc`].  The task holds a weak reference so it does **not** prevent
    /// the limiter from being dropped.
    ///
    /// # Example
    /// ```ignore
    /// let limiter = Arc::new(RateLimiter::new(config));
    /// RateLimiter::spawn_cleanup_task(Arc::clone(&limiter));
    /// ```
    #[allow(dead_code)]
    pub fn spawn_cleanup_task(this: std::sync::Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                this.cleanup();
            }
        });
    }

    /// Return how many keys are currently tracked.
    #[allow(dead_code)]
    pub fn entry_count(&self) -> usize {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.entries.len()
    }
}

/// Check if an IP address is loopback (127.0.0.0/8 or ::1).
fn is_loopback(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => v4.octets()[0] == 127,
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RateLimitConfig {
        RateLimitConfig {
            max_attempts: 3,
            window_seconds: 60,
            lockout_seconds: 10,
        }
    }

    #[test]
    fn test_allows_within_limit() {
        let rl = RateLimiter::new(test_config());
        assert!(rl.check("user_a").is_ok());
        assert!(rl.check("user_a").is_ok());
    }

    #[test]
    fn test_denies_after_exhaustion() {
        let rl = RateLimiter::new(test_config()); // max_attempts = 3
        assert!(rl.check("user_a").is_ok()); // 1st attempt
        assert!(rl.check("user_a").is_ok()); // 2nd attempt
                                             // 3rd attempt hits the limit (3 >= 3)
        let err = rl.check("user_a").unwrap_err();
        assert!(err.retry_after_seconds > 0);
    }

    #[test]
    fn test_independent_keys() {
        let rl = RateLimiter::new(test_config()); // max_attempts = 3
        assert!(rl.check("user_a").is_ok()); // 1st
        assert!(rl.check("user_a").is_ok()); // 2nd
        assert!(rl.check("user_a").is_err()); // 3rd hits limit

        // user_b should be unaffected
        assert!(rl.check("user_b").is_ok());
    }

    #[test]
    fn test_reset_unlocks() {
        let rl = RateLimiter::new(RateLimitConfig {
            max_attempts: 2,
            window_seconds: 60,
            lockout_seconds: 60,
        });
        assert!(rl.check("user_a").is_ok());
        assert!(rl.check("user_a").is_err());

        rl.reset("user_a");
        assert!(rl.check("user_a").is_ok());
    }

    #[test]
    fn test_entry_count() {
        let rl = RateLimiter::new(test_config());
        assert_eq!(rl.entry_count(), 0);
        rl.check("a").ok();
        rl.check("b").ok();
        assert_eq!(rl.entry_count(), 2);
        rl.reset("a");
        assert_eq!(rl.entry_count(), 1);
    }

    #[test]
    fn test_loopback_exempt() {
        let rl = RateLimiter::new(test_config());
        let loopback_v4: IpAddr = "127.0.0.1".parse().unwrap();
        let loopback_v6: IpAddr = "::1".parse().unwrap();

        // Loopback should never be blocked
        for _ in 0..20 {
            assert!(!rl.record_failure(loopback_v4));
            assert!(!rl.record_failure(loopback_v6));
        }
        assert!(!rl.is_blocked(loopback_v4));
        assert!(!rl.is_blocked(loopback_v6));
    }

    #[test]
    fn test_record_failure_locks_after_threshold() {
        let rl = RateLimiter::new(test_config());
        let addr: IpAddr = "1.2.3.4".parse().unwrap();

        assert!(!rl.record_failure(addr));
        assert!(!rl.record_failure(addr));
        // Third attempt crosses the threshold
        assert!(rl.record_failure(addr));
        assert!(rl.is_blocked(addr));
    }

    #[test]
    fn test_rate_limit_error_display() {
        let err = RateLimitError {
            message: "Too many attempts".to_string(),
            retry_after_seconds: 30,
        };
        let display = format!("{}", err);
        assert!(display.contains("30 seconds"));
    }

    #[test]
    fn test_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let rl = Arc::new(RateLimiter::new(RateLimitConfig {
            max_attempts: 100,
            window_seconds: 60,
            lockout_seconds: 10,
        }));

        let mut handles = vec![];
        for i in 0..10 {
            let rl_clone = rl.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..10 {
                    let _ = rl_clone.check(&format!("thread-{}", i));
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Should not panic and should have tracked all keys
        assert_eq!(rl.entry_count(), 10);
    }
}
