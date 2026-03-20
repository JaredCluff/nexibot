//! Outbound rate limiter for Deepgram STT API calls.
//!
//! Enforces two independent limits:
//!
//! 1. **Per-minute token bucket** — caps burst usage, configurable via
//!    `stt.deepgram_calls_per_minute` (default: 10).  At 10 calls/min an
//!    average utterance takes ~3 s of audio, so you'd use ~30 s of quota per
//!    minute — well within Deepgram's free tier.
//!
//! 2. **Monthly audio budget** — tracks accumulated audio seconds sent to
//!    Deepgram this calendar month and warns / blocks when approaching the
//!    free-tier ceiling (200 hrs = 720 000 s).  Persisted to
//!    `~/.config/nexibot/deepgram_usage.json` so it survives
//!    restarts.
//!
//! Both limits are independently configurable and can be disabled entirely.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, warn};

pub use crate::config::voice::DeepgramRateLimitConfig;

// ── Free-tier constants ───────────────────────────────────────────────────────

/// Deepgram free tier: 200 hours/month expressed in seconds.
pub const DEEPGRAM_FREE_TIER_SECONDS: f32 = 200.0 * 3600.0; // 720 000 s

/// Warn the user when this fraction of the monthly budget is consumed.
const WARN_THRESHOLD: f32 = 0.80;

/// Block calls once this fraction is consumed (leaves a small safety margin).
const BLOCK_THRESHOLD: f32 = 0.98;

// ── Persisted usage record ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UsageRecord {
    /// Calendar month this record covers, e.g. "2026-03"
    month: String,
    /// Total audio seconds sent to Deepgram this month
    audio_seconds: f32,
}

impl UsageRecord {
    fn current_month() -> String {
        let now = chrono::Utc::now();
        format!("{}", now.format("%Y-%m"))
    }

    fn for_this_month() -> Self {
        Self {
            month: Self::current_month(),
            audio_seconds: 0.0,
        }
    }
}

// ── Per-minute token bucket ───────────────────────────────────────────────────

struct TokenBucket {
    /// Maximum tokens (= calls per refill period)
    capacity: u32,
    /// Tokens currently available
    available: u32,
    /// When the bucket was last refilled
    last_refill: Instant,
    /// How often the bucket fully refills (1 minute)
    refill_interval: Duration,
}

impl TokenBucket {
    fn new(calls_per_minute: u32) -> Self {
        Self {
            capacity: calls_per_minute,
            available: calls_per_minute,
            last_refill: Instant::now(),
            refill_interval: Duration::from_secs(60),
        }
    }

    /// Attempt to consume one token. Returns false if rate-limited.
    fn try_consume(&mut self) -> bool {
        // Refill if the interval has elapsed
        let elapsed = self.last_refill.elapsed();
        if elapsed >= self.refill_interval {
            self.available = self.capacity;
            self.last_refill = Instant::now();
        }

        if self.available > 0 {
            self.available -= 1;
            true
        } else {
            false
        }
    }

    fn seconds_until_refill(&self) -> u64 {
        let elapsed = self.last_refill.elapsed();
        if elapsed >= self.refill_interval {
            0
        } else {
            (self.refill_interval - elapsed).as_secs() + 1
        }
    }
}

// ── Public rate limiter ───────────────────────────────────────────────────────

struct Inner {
    bucket: TokenBucket,
    usage: UsageRecord,
    usage_path: PathBuf,
    config: DeepgramRateLimitConfig,
    /// Whether we have already emitted the 80% warning this month
    warned_80pct: bool,
}

impl Inner {
    fn load(config: DeepgramRateLimitConfig, usage_path: PathBuf) -> Self {
        let usage = Self::load_usage(&usage_path);
        let bucket = TokenBucket::new(config.calls_per_minute);
        Self {
            bucket,
            usage,
            usage_path,
            config,
            warned_80pct: false,
        }
    }

    fn load_usage(path: &PathBuf) -> UsageRecord {
        let current_month = UsageRecord::current_month();

        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(record) = serde_json::from_str::<UsageRecord>(&raw) {
                if record.month == current_month {
                    return record;
                }
                // New month — reset
            }
        }

        UsageRecord::for_this_month()
    }

    fn persist_usage(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.usage) {
            if let Some(parent) = self.usage_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&self.usage_path, json);
        }
    }

    fn monthly_fraction(&self) -> f32 {
        self.usage.audio_seconds / self.config.monthly_budget_secs
    }
}

/// Thread-safe Deepgram outbound rate limiter.
#[derive(Clone)]
pub struct DeepgramRateLimiter {
    inner: Arc<Mutex<Inner>>,
    enabled: bool,
}

impl DeepgramRateLimiter {
    /// Create a new rate limiter. `usage_path` should be inside the NexiBot
    /// config directory, e.g. `~/.config/nexibot/deepgram_usage.json`.
    pub fn new(config: DeepgramRateLimitConfig, usage_path: PathBuf) -> Self {
        let enabled = config.enabled;
        Self {
            inner: Arc::new(Mutex::new(Inner::load(config, usage_path))),
            enabled,
        }
    }

    /// Check rate limits before a Deepgram call.
    ///
    /// Returns `Ok(())` if the call is allowed.
    /// Returns `Err` with a human-readable message if it should be blocked.
    pub async fn check(&self, audio_duration_secs: f32) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let mut inner = self.inner.lock().await;

        // ── Monthly budget check ──────────────────────────────────────────────
        let fraction = inner.monthly_fraction();

        if fraction >= BLOCK_THRESHOLD && inner.config.block_on_budget_exhausted {
            let used = inner.usage.audio_seconds / 3600.0;
            let budget = inner.config.monthly_budget_secs / 3600.0;
            return Err(anyhow::anyhow!(
                "Deepgram monthly budget exhausted ({:.1}/{:.1} hrs used). \
                 Set block_on_budget_exhausted = false in config to allow overage.",
                used,
                budget
            ));
        }

        if fraction >= WARN_THRESHOLD && !inner.warned_80pct {
            let used = inner.usage.audio_seconds / 3600.0;
            let budget = inner.config.monthly_budget_secs / 3600.0;
            warn!(
                "[STT/DEEPGRAM] Monthly budget at {:.0}%: {:.1}/{:.1} hrs used this month.",
                fraction * 100.0,
                used,
                budget
            );
            inner.warned_80pct = true;
        }

        // ── Per-minute token bucket ───────────────────────────────────────────
        if !inner.bucket.try_consume() {
            let wait = inner.bucket.seconds_until_refill();
            return Err(anyhow::anyhow!(
                "Deepgram rate limit: {} calls/min exceeded. Retry in {}s.",
                inner.config.calls_per_minute,
                wait
            ));
        }

        // ── Charge the monthly budget (optimistic — charge on allow) ──────────
        inner.usage.audio_seconds += audio_duration_secs;
        debug!(
            "[STT/DEEPGRAM] Charged {:.1}s — monthly total: {:.0}s / {:.0}s ({:.1}%)",
            audio_duration_secs,
            inner.usage.audio_seconds,
            inner.config.monthly_budget_secs,
            inner.monthly_fraction() * 100.0,
        );
        inner.persist_usage();

        Ok(())
    }

    /// Monthly usage summary for status display.
    pub async fn usage_summary(&self) -> (f32, f32, f32) {
        let inner = self.inner.lock().await;
        let used_secs = inner.usage.audio_seconds;
        let budget_secs = inner.config.monthly_budget_secs;
        let pct = inner.monthly_fraction() * 100.0;
        (used_secs, budget_secs, pct)
    }
}
