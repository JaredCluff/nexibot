//! DM Pairing Security for Telegram and WhatsApp.
//!
//! Provides a pairing code workflow so unknown senders get a code,
//! which an admin can approve or deny via the Settings UI.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use subtle::ConstantTimeEq;
use tracing::{info, warn};

/// DM policy for a messaging channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DmPolicy {
    /// Classic allowlist behavior: empty allowlist = allow all, non-empty = only listed IDs.
    Allowlist,
    /// Open mode: always allow all direct messages.
    Open,
    /// Pairing mode: unknown senders receive a code; admin approves to add to runtime allowlist.
    #[default]
    Pairing,
}

/// A pending pairing request from an unknown sender.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequest {
    /// Sender ID (chat_id for Telegram, phone for WhatsApp)
    pub id: String,
    /// 12-char human-friendly pairing code (~60 bits of entropy)
    pub code: String,
    /// Channel: "telegram" or "whatsapp"
    pub channel: String,
    /// Display name of the sender (if available)
    pub display_name: Option<String>,
    /// When the request was created
    pub created_at: DateTime<Utc>,
    /// When this code expires (15 minutes after creation)
    #[serde(default = "default_far_future")]
    pub expires_at: DateTime<Utc>,
}

fn default_far_future() -> DateTime<Utc> {
    // For backward compat with old serialized requests that lack expires_at
    Utc::now() + chrono::Duration::seconds(TTL_SECONDS)
}

/// Persisted allowlist data.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AllowlistData {
    telegram: Vec<i64>,
    whatsapp: Vec<String>,
    /// Generic per-channel allowlists (signal, teams, matrix, email, etc.)
    #[serde(default)]
    channels: HashMap<String, Vec<String>>,
}

/// Shadow state for a single rate-limited key, persisted across restarts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RateLimitShadow {
    /// Number of attempts recorded in the current window.
    count: u32,
    /// Wall-clock lockout expiry (RFC 3339), if the key is currently locked out.
    locked_until: Option<String>,
    /// Wall-clock window start (RFC 3339) — the timestamp of the first attempt in this window.
    window_start: String,
}

/// Manages DM pairing state: pending requests and runtime allowlists.
pub struct PairingManager {
    pending: Vec<PairingRequest>,
    runtime_telegram_allowlist: HashSet<i64>,
    runtime_whatsapp_allowlist: HashSet<String>,
    /// Generic per-channel allowlists for Signal, Teams, Matrix, Email, etc.
    runtime_channel_allowlists: HashMap<String, HashSet<String>>,
    data_dir: PathBuf,
    /// Rate limiter for pairing code generation (max 5 per sender per 15 minutes)
    rate_limiter: crate::security::rate_limit::RateLimiter,
    /// Shadow map tracking per-key attempt counts and lockout times for persistence.
    rate_limit_shadow: HashMap<String, RateLimitShadow>,
}

/// Human-friendly code alphabet (no ambiguous chars: 0, 1, I, O removed)
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
/// 12 chars from 31-char alphabet = ~60 bits of entropy (up from 8 chars / ~38 bits)
const CODE_LENGTH: usize = 12;
const MAX_PENDING_PER_SENDER: usize = 3;
/// Pairing codes expire after 15 minutes (down from 1 hour for security)
const TTL_SECONDS: i64 = 900;
/// Maximum total active pairing requests system-wide.
/// Prevents resource exhaustion from a flood of pairing requests.
const MAX_ACTIVE_PAIRING_REQUESTS: usize = 20;
/// Maximum entries in the rate-limit shadow map (in-memory).
/// Prevents unbounded growth when many unique senders attempt pairing.
const MAX_SHADOW_ENTRIES: usize = 1000;
/// Maximum pairing attempts per IP/user per 15 minutes.
const PAIRING_RATE_LIMIT_MAX_ATTEMPTS: u32 = 5;
/// Sliding window for pairing rate limit (15 minutes).
const PAIRING_RATE_LIMIT_WINDOW_SECONDS: u64 = 900;
/// Lockout duration when pairing rate limit is exceeded (15 minutes).
const PAIRING_RATE_LIMIT_LOCKOUT_SECONDS: u64 = 900;
/// Validate that a code matches the expected pairing code format.
/// Rejects codes before any lookup to prevent oracle attacks on the pending list.
fn is_valid_code_format(code: &str) -> bool {
    code.len() == CODE_LENGTH && code.bytes().all(|b| CODE_ALPHABET.contains(&b))
}

impl PairingManager {
    /// Create a new PairingManager, loading persisted state.
    pub fn new() -> Result<Self> {
        let data_dir = Self::get_data_dir()?;
        std::fs::create_dir_all(&data_dir)?;

        let mut mgr = Self {
            pending: Vec::new(),
            runtime_telegram_allowlist: HashSet::new(),
            runtime_whatsapp_allowlist: HashSet::new(),
            runtime_channel_allowlists: HashMap::new(),
            data_dir,
            // Rate limit: 5 pairing attempts per sender per 15-minute window
            rate_limiter: crate::security::rate_limit::RateLimiter::new(
                crate::security::rate_limit::RateLimitConfig {
                    max_attempts: PAIRING_RATE_LIMIT_MAX_ATTEMPTS,
                    window_seconds: PAIRING_RATE_LIMIT_WINDOW_SECONDS,
                    lockout_seconds: PAIRING_RATE_LIMIT_LOCKOUT_SECONDS,
                },
            ),
            rate_limit_shadow: HashMap::new(),
        };

        mgr.load_persisted()?;
        mgr.load_rate_limit_state();
        Ok(mgr)
    }

    fn get_data_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Failed to get home directory")?;
        Ok(home.join(".config/nexibot/pairing"))
    }

    /// Check if a Telegram chat ID is allowed (config allowlist + runtime allowlist).
    pub fn is_telegram_allowed(&self, chat_id: i64, config_allowed: &[i64]) -> bool {
        // Pairing policy is closed-by-default: both empty => deny.
        config_allowed.contains(&chat_id) || self.runtime_telegram_allowlist.contains(&chat_id)
    }

    /// Check if a WhatsApp phone number is allowed (config allowlist + runtime allowlist).
    pub fn is_whatsapp_allowed(&self, phone: &str, config_allowed: &[String]) -> bool {
        // Pairing policy is closed-by-default: both empty => deny.
        config_allowed.contains(&phone.to_string())
            || self.runtime_whatsapp_allowlist.contains(phone)
    }

    /// Evict expired or excess entries from the rate-limit shadow map.
    /// Removes entries whose rate-limit window has expired first,
    /// then evicts oldest by window_start if still over MAX_SHADOW_ENTRIES.
    fn evict_shadow_if_needed(&mut self) {
        if self.rate_limit_shadow.len() < MAX_SHADOW_ENTRIES {
            return;
        }

        let window_secs = PAIRING_RATE_LIMIT_WINDOW_SECONDS as i64;
        let lockout_secs = PAIRING_RATE_LIMIT_LOCKOUT_SECONDS as i64;
        let now = Utc::now();

        // Remove entries that are no longer active (window expired and not locked)
        self.rate_limit_shadow.retain(|_, shadow| {
            let locked = shadow.locked_until.as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .is_some_and(|dt| dt.with_timezone(&Utc) > now);

            let window_start = DateTime::parse_from_rfc3339(&shadow.window_start)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or(now);
            let window_active = (now - window_start).num_seconds() < window_secs.max(lockout_secs);

            locked || window_active
        });

        // If still over limit, evict oldest by window_start
        if self.rate_limit_shadow.len() >= MAX_SHADOW_ENTRIES {
            let mut keys_by_age: Vec<(String, String)> = self.rate_limit_shadow
                .iter()
                .map(|(k, v)| (k.clone(), v.window_start.clone()))
                .collect();
            keys_by_age.sort_by(|a, b| a.1.cmp(&b.1));
            let evict_count = self.rate_limit_shadow.len().saturating_sub(MAX_SHADOW_ENTRIES / 2);
            for (key, _) in keys_by_age.into_iter().take(evict_count) {
                self.rate_limit_shadow.remove(&key);
            }
        }
    }

    /// Create a pairing request for an unknown sender.
    /// Returns the generated code on success.
    ///
    /// Enforces:
    /// - Per-sender rate limit: max 5 attempts per IP/user per 15 minutes
    /// - System-wide cap: max 20 active pairing requests total
    /// - Per-sender cap: max 3 pending requests per sender per channel
    pub fn create_pairing_request(
        &mut self,
        channel: &str,
        sender_id: &str,
        display_name: Option<String>,
    ) -> Result<String> {
        // Rate limit check: prevent brute-force pairing code generation
        let rate_key = format!("{}:{}", channel, sender_id);
        self.evict_shadow_if_needed();
        match self.rate_limiter.check(&rate_key) {
            Err(e) => {
                // Update shadow state to reflect lockout so it can be persisted
                let shadow = self.rate_limit_shadow.entry(rate_key.clone()).or_insert_with(|| RateLimitShadow {
                    count: PAIRING_RATE_LIMIT_MAX_ATTEMPTS,
                    locked_until: None,
                    window_start: Utc::now().to_rfc3339(),
                });
                shadow.count = PAIRING_RATE_LIMIT_MAX_ATTEMPTS;
                let locked_until = Utc::now() + chrono::Duration::seconds(e.retry_after_seconds as i64);
                shadow.locked_until = Some(locked_until.to_rfc3339());
                self.save_rate_limit_state();
                info!(
                    "[PAIRING] Rate limited pairing attempt for {} on {}: {}",
                    sender_id, channel, e
                );
                anyhow::bail!(
                    "Pairing rate limited for {}. {} (retry after {} seconds)",
                    sender_id,
                    e.message,
                    e.retry_after_seconds
                );
            }
            Ok(()) => {
                // Record this attempt in the shadow map and persist
                let now_str = Utc::now().to_rfc3339();
                let shadow = self.rate_limit_shadow.entry(rate_key.clone()).or_insert_with(|| RateLimitShadow {
                    count: 0,
                    locked_until: None,
                    window_start: now_str.clone(),
                });
                shadow.count += 1;
                shadow.locked_until = None;
                self.save_rate_limit_state();
            }
        }

        // Expire old requests first
        self.expire_pending();

        // System-wide cap on active pairing requests
        if self.pending.len() >= MAX_ACTIVE_PAIRING_REQUESTS {
            info!(
                "[PAIRING] System-wide pairing cap reached ({}/{}), rejecting request from {} on {}",
                self.pending.len(),
                MAX_ACTIVE_PAIRING_REQUESTS,
                sender_id,
                channel,
            );
            anyhow::bail!(
                "Too many active pairing requests system-wide (max {}). \
                 Please try again later (retry after {} seconds)",
                MAX_ACTIVE_PAIRING_REQUESTS,
                TTL_SECONDS
            );
        }

        // Check max pending per sender
        let existing = self
            .pending
            .iter()
            .filter(|r| r.id == sender_id && r.channel == channel)
            .count();
        if existing >= MAX_PENDING_PER_SENDER {
            anyhow::bail!(
                "Too many pending requests for this sender (max {})",
                MAX_PENDING_PER_SENDER
            );
        }

        let code = Self::generate_code();

        let now = Utc::now();
        let request = PairingRequest {
            id: sender_id.to_string(),
            code: code.clone(),
            channel: channel.to_string(),
            display_name,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(TTL_SECONDS),
        };

        let previous_pending = self.pending.clone();
        self.pending.push(request);
        if let Err(e) = self.save_pending() {
            self.pending = previous_pending;
            return Err(e);
        }

        info!(
            "[PAIRING] Created pairing request for {} on {}: code {} (active: {}/{})",
            sender_id,
            channel,
            code,
            self.pending.len(),
            MAX_ACTIVE_PAIRING_REQUESTS
        );
        Ok(code)
    }

    /// Return the current count of active (non-expired) pairing requests.
    #[allow(dead_code)]
    pub fn active_request_count(&mut self) -> usize {
        self.expire_pending();
        self.pending.len()
    }

    /// Approve a pairing code: move sender to the runtime allowlist.
    pub fn approve_code(&mut self, code: &str) -> Result<PairingRequest> {
        self.expire_pending();
        let previous_pending = self.pending.clone();
        let previous_telegram_allowlist = self.runtime_telegram_allowlist.clone();
        let previous_whatsapp_allowlist = self.runtime_whatsapp_allowlist.clone();
        let previous_channel_allowlists = self.runtime_channel_allowlists.clone();

        // Validate code format before any lookup (prevents oracle timing on the pending list)
        if !is_valid_code_format(code) {
            warn!("[PAIRING] Invalid code format attempted: {:?}", code);
            anyhow::bail!("Pairing code not found or expired: {}", code);
        }

        // Constant-time scan: compare all entries to avoid leaking which codes exist
        // expire_pending() above already removed stale entries, but we double-check expiry here.
        let code_bytes = code.as_bytes();
        let pos = {
            let now = Utc::now();
            let mut found: Option<usize> = None;
            for (i, r) in self.pending.iter().enumerate() {
                // Pad to the same length (both are CODE_LENGTH, but guard anyway)
                let pending_bytes = r.code.as_bytes();
                let len = code_bytes.len().max(pending_bytes.len());
                let mut a = vec![0u8; len];
                let mut b = vec![0u8; len];
                a[..code_bytes.len()].copy_from_slice(code_bytes);
                b[..pending_bytes.len()].copy_from_slice(pending_bytes);
                // Constant-time equality check — evaluates all entries regardless of match
                let matched = a.ct_eq(&b).into() && now <= r.expires_at;
                if matched && found.is_none() {
                    found = Some(i);
                }
            }
            found.ok_or_else(|| anyhow::anyhow!("Pairing code not found or expired: {}", code))?
        };

        let request = self.pending.remove(pos);

        match request.channel.as_str() {
            "telegram" => {
                if let Ok(chat_id) = request.id.parse::<i64>() {
                    self.runtime_telegram_allowlist.insert(chat_id);
                }
            }
            "whatsapp" => {
                self.runtime_whatsapp_allowlist.insert(request.id.clone());
            }
            channel => {
                self.runtime_channel_allowlists
                    .entry(channel.to_string())
                    .or_default()
                    .insert(request.id.clone());
            }
        }

        if let Err(e) = self.save_pending().and_then(|_| self.save_allowlist()) {
            self.pending = previous_pending;
            self.runtime_telegram_allowlist = previous_telegram_allowlist;
            self.runtime_whatsapp_allowlist = previous_whatsapp_allowlist;
            self.runtime_channel_allowlists = previous_channel_allowlists;
            return Err(e);
        }

        info!(
            "[PAIRING] Approved pairing code {} for {} ({})",
            code, request.id, request.channel
        );
        Ok(request)
    }

    /// Deny a pairing code: remove the pending request.
    pub fn deny_code(&mut self, code: &str) -> Result<()> {
        // Validate code format before any lookup (prevents oracle timing on the pending list)
        if !is_valid_code_format(code) {
            warn!("[PAIRING] Invalid code format attempted: {:?}", code);
            anyhow::bail!("Pairing code not found: {}", code);
        }

        let previous_pending = self.pending.clone();
        let now = Utc::now();
        // Constant-time scan across all entries to avoid leaking which codes exist
        let code_bytes = code.as_bytes();
        let pos = {
            let mut found: Option<usize> = None;
            for (i, r) in self.pending.iter().enumerate() {
                let pending_bytes = r.code.as_bytes();
                let len = code_bytes.len().max(pending_bytes.len());
                let mut a = vec![0u8; len];
                let mut b = vec![0u8; len];
                a[..code_bytes.len()].copy_from_slice(code_bytes);
                b[..pending_bytes.len()].copy_from_slice(pending_bytes);
                let matched: bool = a.ct_eq(&b).into() && now <= r.expires_at;
                if matched && found.is_none() {
                    found = Some(i);
                }
            }
            found.ok_or_else(|| anyhow::anyhow!("Pairing code not found: {}", code))?
        };

        let request = self.pending.remove(pos);
        if let Err(e) = self.save_pending() {
            self.pending = previous_pending;
            return Err(e);
        }

        info!(
            "[PAIRING] Denied pairing code {} for {} ({})",
            code, request.id, request.channel
        );
        Ok(())
    }

    /// List all pending pairing requests (after expiring stale ones).
    pub fn list_pending(&mut self) -> Vec<PairingRequest> {
        self.expire_pending();
        self.pending.clone()
    }

    /// Get the runtime Telegram allowlist.
    pub fn get_telegram_allowlist(&self) -> Vec<i64> {
        self.runtime_telegram_allowlist.iter().copied().collect()
    }

    /// Get the runtime WhatsApp allowlist.
    pub fn get_whatsapp_allowlist(&self) -> Vec<String> {
        self.runtime_whatsapp_allowlist.iter().cloned().collect()
    }

    /// Check if a sender is allowed for a generic channel (config allowlist + runtime allowlist).
    pub fn is_channel_allowed(
        &self,
        channel: &str,
        sender_id: &str,
        config_allowed: &[String],
    ) -> bool {
        let runtime = self.runtime_channel_allowlists.get(channel);
        // Pairing policy is closed-by-default: both empty => deny.
        config_allowed.contains(&sender_id.to_string())
            || runtime.is_some_and(|s| s.contains(sender_id))
    }

    /// Get the runtime allowlist for a generic channel.
    #[allow(dead_code)]
    pub fn get_channel_allowlist(&self, channel: &str) -> Vec<String> {
        self.runtime_channel_allowlists
            .get(channel)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get all generic channel runtime allowlists.
    pub fn get_all_channel_allowlists(&self) -> HashMap<String, Vec<String>> {
        self.runtime_channel_allowlists
            .iter()
            .map(|(channel, senders)| (channel.clone(), senders.iter().cloned().collect()))
            .collect()
    }

    /// Remove a sender from a generic channel's runtime allowlist.
    pub fn remove_channel_sender(&mut self, channel: &str, sender_id: &str) -> Result<()> {
        let previous_allowlist = self.runtime_channel_allowlists.clone();
        if let Some(set) = self.runtime_channel_allowlists.get_mut(channel) {
            set.remove(sender_id);
        }
        if let Err(e) = self.save_allowlist() {
            self.runtime_channel_allowlists = previous_allowlist;
            return Err(e);
        }
        info!(
            "[PAIRING] Removed {} sender {} from runtime allowlist",
            channel, sender_id
        );
        Ok(())
    }

    /// Remove a Telegram chat ID from the runtime allowlist.
    pub fn remove_telegram(&mut self, chat_id: i64) -> Result<()> {
        let previous_allowlist = self.runtime_telegram_allowlist.clone();
        self.runtime_telegram_allowlist.remove(&chat_id);
        if let Err(e) = self.save_allowlist() {
            self.runtime_telegram_allowlist = previous_allowlist;
            return Err(e);
        }
        info!(
            "[PAIRING] Removed Telegram chat {} from runtime allowlist",
            chat_id
        );
        Ok(())
    }

    /// Remove a WhatsApp phone number from the runtime allowlist.
    pub fn remove_whatsapp(&mut self, phone: &str) -> Result<()> {
        let previous_allowlist = self.runtime_whatsapp_allowlist.clone();
        self.runtime_whatsapp_allowlist.remove(phone);
        if let Err(e) = self.save_allowlist() {
            self.runtime_whatsapp_allowlist = previous_allowlist;
            return Err(e);
        }
        info!(
            "[PAIRING] Removed WhatsApp phone {} from runtime allowlist",
            phone
        );
        Ok(())
    }

    /// Generate an 8-char human-friendly code.
    fn generate_code() -> String {
        let mut rng = rand::rngs::OsRng;
        (0..CODE_LENGTH)
            .map(|_| {
                let idx = rng.gen_range(0..CODE_ALPHABET.len());
                CODE_ALPHABET[idx] as char
            })
            .collect()
    }

    /// Remove expired pending requests (older than TTL).
    fn expire_pending(&mut self) {
        let now = Utc::now();
        let previous_pending = self.pending.clone();
        let before = self.pending.len();
        self.pending
            .retain(|r| (now - r.created_at).num_seconds() < TTL_SECONDS);
        let removed = before - self.pending.len();
        if removed > 0 {
            if let Err(e) = self.save_pending() {
                self.pending = previous_pending;
                warn!(
                    "[PAIRING] Failed to persist pending-expiry cleanup; restoring in-memory pending set: {}",
                    e
                );
            } else {
                info!("[PAIRING] Expired {} stale pairing requests", removed);
            }
        }
    }

    /// Load persisted pending requests and allowlist.
    fn load_persisted(&mut self) -> Result<()> {
        // Load pending
        let pending_path = self.data_dir.join("pending.json");
        if pending_path.exists() {
            // std::fs::read_to_string is blocking I/O; use block_in_place when called
            // from within an async context (e.g. under a tokio RwLock write guard).
            let content = tokio::task::block_in_place(|| std::fs::read_to_string(&pending_path))?;
            self.pending = serde_json::from_str(&content).unwrap_or_default();
        }

        // Load allowlist
        let allowlist_path = self.data_dir.join("allowlist.json");
        if allowlist_path.exists() {
            let content = tokio::task::block_in_place(|| std::fs::read_to_string(&allowlist_path))?;
            let data: AllowlistData = serde_json::from_str(&content).unwrap_or_default();
            self.runtime_telegram_allowlist = data.telegram.into_iter().collect();
            self.runtime_whatsapp_allowlist = data.whatsapp.into_iter().collect();
            self.runtime_channel_allowlists = data
                .channels
                .into_iter()
                .map(|(k, v)| (k, v.into_iter().collect()))
                .collect();
        }

        Ok(())
    }

    /// Persist pending requests to disk.
    fn save_pending(&self) -> Result<()> {
        let path = self.data_dir.join("pending.json");
        let content = serde_json::to_string_pretty(&self.pending)?;
        // std::fs::write is blocking I/O; use block_in_place when called from within
        // an async context (e.g. under a tokio RwLock write guard).
        tokio::task::block_in_place(|| std::fs::write(&path, content))?;
        Ok(())
    }

    /// Persist runtime allowlist to disk.
    fn save_allowlist(&self) -> Result<()> {
        let path = self.data_dir.join("allowlist.json");
        let data = AllowlistData {
            telegram: self.runtime_telegram_allowlist.iter().copied().collect(),
            whatsapp: self.runtime_whatsapp_allowlist.iter().cloned().collect(),
            channels: self
                .runtime_channel_allowlists
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
        };
        let content = serde_json::to_string_pretty(&data)?;
        tokio::task::block_in_place(|| std::fs::write(&path, content))?;
        Ok(())
    }

    /// Persist rate limit state to disk so it survives app restarts.
    ///
    /// Stores per-key attempt counts and lockout expiry times as wall-clock
    /// RFC 3339 timestamps so they remain meaningful across restarts.
    fn save_rate_limit_state(&self) {
        #[derive(serde::Serialize)]
        #[allow(dead_code)]
        struct RateLimitEntry {
            key: String,
            /// Number of attempts recorded in the current window.
            attempts_in_window: u32,
            /// If the key is currently locked out, when the lockout expires.
            locked_until: Option<String>,
            /// Wall-clock time of the window start (oldest attempt timestamp as RFC 3339).
            window_start: String,
        }

        // We can only observe the current count indirectly. Instead of trying to
        // inspect internal Instant state, we persist the information we actually
        // need to reconstruct: whether a key is locked (and until when), and how
        // many attempts have been consumed so far in the current window.
        //
        // We determine "locked" by peeking: if check() returns an error with a
        // long retry_after we know the key is locked. We use a secondary RateLimiter
        // peek approach — but since we cannot inspect internals, we track this data
        // alongside every call and store it in a parallel HashMap.
        //
        // For now, we save the rate limit counts we track via the shadow map.
        // The shadow map is updated in create_pairing_request.
        let path = self.data_dir.join("rate_limit_state.json");
        let state = serde_json::json!({
            "attempts": self.rate_limit_shadow,
            "saved_at": Utc::now().to_rfc3339(),
        });
        let payload = state.to_string();
        if let Err(e) = tokio::task::block_in_place(|| std::fs::write(&path, payload)) {
            warn!("[PAIRING] Failed to persist rate limit state: {}", e);
        }
    }

    /// Load persisted rate limit state and reconstruct the in-memory RateLimiter.
    ///
    /// Only restores entries that are still within their rate limit window or
    /// still locked out.  Stale entries are silently discarded.
    fn load_rate_limit_state(&mut self) {
        let path = self.data_dir.join("rate_limit_state.json");
        let data = match tokio::task::block_in_place(|| std::fs::read_to_string(&path)) {
            Ok(d) => d,
            Err(_) => return, // No persisted state; start fresh.
        };
        let parsed: serde_json::Value = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(e) => {
                warn!("[PAIRING] Failed to parse rate limit state: {}", e);
                return;
            }
        };
        let now = Utc::now();
        let window_duration = chrono::Duration::seconds(PAIRING_RATE_LIMIT_WINDOW_SECONDS as i64);
        let _lockout_duration = chrono::Duration::seconds(PAIRING_RATE_LIMIT_LOCKOUT_SECONDS as i64);

        if let Some(attempts) = parsed["attempts"].as_object() {
            for (key, entry) in attempts {
                let attempt_count = entry["count"].as_u64().unwrap_or(0) as u32;
                let locked_until_str = entry["locked_until"].as_str();
                let window_start_str = entry["window_start"].as_str().unwrap_or("");

                // Parse the window start time
                let window_start = match DateTime::parse_from_rfc3339(window_start_str) {
                    Ok(dt) => dt.with_timezone(&Utc),
                    Err(_) => continue, // Unparseable entry; skip.
                };

                // Check if this window is still active
                let window_age = now - window_start;
                let is_locked = if let Some(locked_str) = locked_until_str {
                    match DateTime::parse_from_rfc3339(locked_str) {
                        Ok(dt) => dt.with_timezone(&Utc) > now,
                        Err(_) => false,
                    }
                } else {
                    false
                };

                if !is_locked && window_age > window_duration {
                    // Window has expired; no need to restore.
                    continue;
                }

                // Reconstruct rate limiter state by replaying check() calls.
                // For locked keys, replay max_attempts times to trigger lockout.
                // For in-window keys, replay only the recorded attempt count.
                let replay_count = if is_locked {
                    PAIRING_RATE_LIMIT_MAX_ATTEMPTS
                } else {
                    attempt_count.min(PAIRING_RATE_LIMIT_MAX_ATTEMPTS - 1)
                };

                for _ in 0..replay_count {
                    let _ = self.rate_limiter.check(key);
                }

                // Restore shadow state
                self.rate_limit_shadow.insert(key.clone(), RateLimitShadow {
                    count: attempt_count,
                    locked_until: if is_locked {
                        locked_until_str.map(|s| s.to_string())
                    } else {
                        None
                    },
                    window_start: window_start_str.to_string(),
                });

                if is_locked {
                    info!("[PAIRING] Restored rate limit lockout for key '{}' (survived restart)", key);
                } else if attempt_count > 0 {
                    info!("[PAIRING] Restored {} rate limit attempts for key '{}' (survived restart)", attempt_count, key);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PairingRateLimiter: standalone rate limiter for pairing requests
// ---------------------------------------------------------------------------

/// A dedicated rate limiter for pairing requests.
///
/// Wraps the generic [`RateLimiter`] with pairing-specific defaults:
/// - Max 5 attempts per key (IP/user) per 15-minute window
/// - 15-minute lockout after exhaustion
///
/// Thread-safe: delegates to the underlying [`RateLimiter`] which uses
/// internal `Mutex` synchronization.
#[allow(dead_code)]
pub struct PairingRateLimiter {
    inner: crate::security::rate_limit::RateLimiter,
}

impl PairingRateLimiter {
    /// Create a new pairing rate limiter with the default configuration.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            inner: crate::security::rate_limit::RateLimiter::new(
                crate::security::rate_limit::RateLimitConfig {
                    max_attempts: PAIRING_RATE_LIMIT_MAX_ATTEMPTS,
                    window_seconds: PAIRING_RATE_LIMIT_WINDOW_SECONDS,
                    lockout_seconds: PAIRING_RATE_LIMIT_LOCKOUT_SECONDS,
                },
            ),
        }
    }

    /// Check if a pairing attempt is allowed for the given key.
    ///
    /// The key should be formatted as `"{channel}:{sender_id}"` or as an
    /// IP address string.
    #[allow(dead_code)]
    pub fn check_allowed(
        &self,
        key: &str,
    ) -> std::result::Result<(), crate::security::rate_limit::RateLimitError> {
        self.inner.check(key)
    }

    /// Reset the rate limit state for a key (e.g. after a successful pairing).
    #[allow(dead_code)]
    pub fn reset(&self, key: &str) {
        self.inner.reset(key);
    }

    /// Remove expired entries to free memory.
    #[allow(dead_code)]
    pub fn cleanup(&self) {
        self.inner.cleanup();
    }

    /// Return how many keys are currently tracked.
    #[allow(dead_code)]
    pub fn entry_count(&self) -> usize {
        self.inner.entry_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a PairingManager backed by a temporary directory for tests.
    fn test_pairing_manager() -> (PairingManager, TempDir) {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let mgr = PairingManager {
            pending: Vec::new(),
            runtime_telegram_allowlist: HashSet::new(),
            runtime_whatsapp_allowlist: HashSet::new(),
            runtime_channel_allowlists: HashMap::new(),
            data_dir: tmp.path().to_path_buf(),
            rate_limit_shadow: HashMap::new(),
            rate_limiter: crate::security::rate_limit::RateLimiter::new(
                crate::security::rate_limit::RateLimitConfig {
                    max_attempts: PAIRING_RATE_LIMIT_MAX_ATTEMPTS,
                    window_seconds: PAIRING_RATE_LIMIT_WINDOW_SECONDS,
                    lockout_seconds: PAIRING_RATE_LIMIT_LOCKOUT_SECONDS,
                },
            ),
        };
        (mgr, tmp)
    }

    #[test]
    fn test_dm_policy_default() {
        assert_eq!(DmPolicy::default(), DmPolicy::Pairing);
    }

    #[test]
    fn test_code_generation() {
        let code = PairingManager::generate_code();
        assert_eq!(code.len(), CODE_LENGTH);
        for c in code.chars() {
            assert!(CODE_ALPHABET.contains(&(c as u8)));
        }
    }

    #[test]
    fn test_dm_policy_serde() {
        let policy = DmPolicy::Pairing;
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: DmPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, DmPolicy::Pairing);
    }

    #[test]
    fn test_create_pairing_request_success() {
        let (mut mgr, _tmp) = test_pairing_manager();
        let code = mgr
            .create_pairing_request("telegram", "user123", Some("Alice".to_string()))
            .unwrap();
        assert_eq!(code.len(), CODE_LENGTH);
        assert_eq!(mgr.pending.len(), 1);
        assert_eq!(mgr.pending[0].id, "user123");
        assert_eq!(mgr.pending[0].channel, "telegram");
    }

    #[test]
    fn test_per_sender_rate_limit() {
        let (mut mgr, _tmp) = test_pairing_manager();

        // To test rate limiting without hitting per-sender cap (3), we approve
        // pending requests between attempts. The rate limiter tracks attempts
        // independently of pending request count.
        // Note: check() records the attempt then checks >= max_attempts,
        // so the max_attempts-th call itself triggers lockout.
        for i in 0..(PAIRING_RATE_LIMIT_MAX_ATTEMPTS - 1) {
            let code = mgr
                .create_pairing_request("telegram", "spammer", Some(format!("attempt-{}", i)))
                .unwrap_or_else(|e| panic!("attempt {} should succeed, got: {}", i, e));

            // Approve to keep pending count below per-sender cap
            mgr.approve_code(&code).unwrap();
        }

        // Next attempt should fail due to rate limit (hits max_attempts threshold)
        let result =
            mgr.create_pairing_request("telegram", "spammer", Some("too-many".to_string()));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("rate limited") || err.contains("Rate limited"),
            "Error should mention rate limit, got: {}",
            err
        );
        assert!(
            err.contains("retry after"),
            "Error should mention retry-after, got: {}",
            err
        );
    }

    #[test]
    fn test_different_senders_independent_rate_limit() {
        let (mut mgr, _tmp) = test_pairing_manager();

        // Exhaust rate limit for sender A (approve between requests to avoid per-sender cap)
        // check() records then checks >= max_attempts, so (max-1) calls succeed
        for _ in 0..(PAIRING_RATE_LIMIT_MAX_ATTEMPTS - 1) {
            let code = mgr
                .create_pairing_request("telegram", "sender_a", None)
                .unwrap();
            mgr.approve_code(&code).unwrap();
        }
        // This call hits the limit
        assert!(mgr
            .create_pairing_request("telegram", "sender_a", None)
            .is_err());

        // Sender B should still be able to create requests
        let result = mgr.create_pairing_request("telegram", "sender_b", None);
        assert!(result.is_ok(), "Sender B should not be rate limited");
    }

    #[test]
    fn test_system_wide_pairing_cap() {
        let (mut mgr, _tmp) = test_pairing_manager();

        // Fill up to the system-wide cap with unique senders
        for i in 0..MAX_ACTIVE_PAIRING_REQUESTS {
            let result =
                mgr.create_pairing_request("telegram", &format!("unique-sender-{}", i), None);
            assert!(result.is_ok(), "Request {} should succeed", i);
        }

        assert_eq!(mgr.pending.len(), MAX_ACTIVE_PAIRING_REQUESTS);

        // Next request should fail due to system-wide cap
        let result = mgr.create_pairing_request("telegram", "one-more-sender", None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("system-wide"),
            "Error should mention system-wide cap, got: {}",
            err
        );
    }

    #[test]
    fn test_approve_frees_system_wide_slot() {
        let (mut mgr, _tmp) = test_pairing_manager();

        // Fill up to the system-wide cap
        let mut codes = Vec::new();
        for i in 0..MAX_ACTIVE_PAIRING_REQUESTS {
            let code = mgr
                .create_pairing_request("telegram", &format!("sender-{}", i), None)
                .unwrap();
            codes.push(code);
        }

        // Approve one request to free a slot
        mgr.approve_code(&codes[0]).unwrap();

        // Now a new request should succeed
        let result = mgr.create_pairing_request("telegram", "new-sender", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_deny_frees_system_wide_slot() {
        let (mut mgr, _tmp) = test_pairing_manager();

        // Fill up to the system-wide cap
        let mut codes = Vec::new();
        for i in 0..MAX_ACTIVE_PAIRING_REQUESTS {
            let code = mgr
                .create_pairing_request("telegram", &format!("sender-{}", i), None)
                .unwrap();
            codes.push(code);
        }

        // Deny one request to free a slot
        mgr.deny_code(&codes[0]).unwrap();

        // Now a new request should succeed
        let result = mgr.create_pairing_request("telegram", "new-sender", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_active_request_count() {
        let (mut mgr, _tmp) = test_pairing_manager();
        assert_eq!(mgr.active_request_count(), 0);

        mgr.create_pairing_request("telegram", "user1", None)
            .unwrap();
        assert_eq!(mgr.active_request_count(), 1);

        mgr.create_pairing_request("telegram", "user2", None)
            .unwrap();
        assert_eq!(mgr.active_request_count(), 2);
    }

    #[test]
    fn test_per_sender_cap_still_enforced() {
        let (mut mgr, _tmp) = test_pairing_manager();

        // Create MAX_PENDING_PER_SENDER requests for the same sender
        for _ in 0..MAX_PENDING_PER_SENDER {
            mgr.create_pairing_request("telegram", "same-user", None)
                .unwrap();
        }

        // Next should fail due to per-sender cap (not rate limit, not system cap)
        let result = mgr.create_pairing_request("telegram", "same-user", None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Too many pending requests"));
    }

    // -----------------------------------------------------------------------
    // PairingRateLimiter standalone tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_pairing_rate_limiter_allows_within_limit() {
        let rl = PairingRateLimiter::new();
        for _ in 0..(PAIRING_RATE_LIMIT_MAX_ATTEMPTS - 1) {
            assert!(rl.check_allowed("test-key").is_ok());
        }
    }

    #[test]
    fn test_pairing_rate_limiter_blocks_after_limit() {
        let rl = PairingRateLimiter::new();
        for _ in 0..PAIRING_RATE_LIMIT_MAX_ATTEMPTS {
            let _ = rl.check_allowed("exhaust-key");
        }
        let result = rl.check_allowed("exhaust-key");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.retry_after_seconds > 0);
    }

    #[test]
    fn test_pairing_rate_limiter_reset() {
        let rl = PairingRateLimiter::new();
        for _ in 0..PAIRING_RATE_LIMIT_MAX_ATTEMPTS {
            let _ = rl.check_allowed("reset-key");
        }
        assert!(rl.check_allowed("reset-key").is_err());

        rl.reset("reset-key");
        assert!(rl.check_allowed("reset-key").is_ok());
    }

    #[test]
    fn test_pairing_rate_limiter_entry_count() {
        let rl = PairingRateLimiter::new();
        assert_eq!(rl.entry_count(), 0);
        let _ = rl.check_allowed("a");
        let _ = rl.check_allowed("b");
        assert_eq!(rl.entry_count(), 2);
    }

    #[test]
    fn test_pairing_rate_limiter_independent_keys() {
        let rl = PairingRateLimiter::new();
        // Exhaust key A
        for _ in 0..PAIRING_RATE_LIMIT_MAX_ATTEMPTS {
            let _ = rl.check_allowed("key-a");
        }
        assert!(rl.check_allowed("key-a").is_err());

        // Key B should still be available
        assert!(rl.check_allowed("key-b").is_ok());
    }
}
