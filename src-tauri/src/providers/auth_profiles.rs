//! Multi-auth-profile rotation with rate-limit-aware failover.
//!
//! Manages multiple API key profiles per provider, automatically rotating
//! to the next available profile when one hits rate limits or fails.
//! Cooldown periods are tracked per-profile so that rate-limited keys
//! are automatically retried once their cooldown expires.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

/// A single authentication profile for a provider.
///
/// Each profile references a keyring key (not the raw secret) and tracks
/// its own rate-limit / failure state so the manager can skip it during
/// rotation and come back once the cooldown expires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthProfile {
    /// Unique identifier for this profile.
    pub id: String,
    /// Provider name this profile belongs to (e.g. "anthropic", "openai").
    pub provider: String,
    /// Keyring key reference for the API key (never the raw key itself).
    pub api_key_ref: String,
    /// Current rate-limit status.
    pub rate_limit_status: RateLimitStatus,
    /// If rate-limited or failed, the earliest time this profile may be retried.
    pub cooldown_until: Option<DateTime<Utc>>,
    /// Consecutive failure count (reset on success).
    pub failure_count: u32,
    /// Timestamp of the most recent successful or attempted use.
    pub last_used: Option<DateTime<Utc>>,
}

/// Rate-limit status for an auth profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RateLimitStatus {
    /// Profile is available for use.
    Available,
    /// Profile is temporarily rate-limited and should be skipped until cooldown expires.
    RateLimited,
    /// Profile has experienced repeated failures and should not be used.
    Failed,
}

/// Manages a pool of [`AuthProfile`]s grouped by provider, with cooldown-aware
/// rotation and automatic failover.
pub struct AuthProfileManager {
    /// Profiles grouped by provider name.
    profiles: HashMap<String, Vec<AuthProfile>>,
    /// Default cooldown duration in seconds when a profile is rate-limited.
    default_cooldown_seconds: i64,
}

impl AuthProfileManager {
    /// Create a new, empty manager with the given default cooldown.
    pub fn new(default_cooldown_seconds: i64) -> Self {
        Self {
            profiles: HashMap::new(),
            default_cooldown_seconds,
        }
    }

    /// Add a profile. Duplicates (same provider + id) are silently replaced.
    pub fn add_profile(&mut self, profile: AuthProfile) {
        let provider = profile.provider.clone();
        let entries = self.profiles.entry(provider.clone()).or_default();

        // Replace existing profile with same id, or push new.
        if let Some(pos) = entries.iter().position(|p| p.id == profile.id) {
            info!(
                "[AUTH_PROFILES] Replaced profile '{}' for provider '{}'",
                profile.id, provider
            );
            entries[pos] = profile;
        } else {
            info!(
                "[AUTH_PROFILES] Added profile '{}' for provider '{}'",
                profile.id, provider
            );
            entries.push(profile);
        }
    }

    /// Remove a profile by provider and profile id.
    /// Returns `true` if a profile was actually removed.
    pub fn remove_profile(&mut self, provider: &str, profile_id: &str) -> bool {
        if let Some(entries) = self.profiles.get_mut(provider) {
            let before = entries.len();
            entries.retain(|p| p.id != profile_id);
            let removed = entries.len() < before;
            if removed {
                info!(
                    "[AUTH_PROFILES] Removed profile '{}' from provider '{}'",
                    profile_id, provider
                );
            }
            // Clean up empty provider bucket.
            if entries.is_empty() {
                self.profiles.remove(provider);
            }
            removed
        } else {
            false
        }
    }

    /// Return the first available profile for `provider`.
    ///
    /// Profiles that are rate-limited but whose cooldown has expired are
    /// automatically promoted back to [`RateLimitStatus::Available`] before
    /// selection. Failed profiles are skipped entirely.
    ///
    /// The selected profile's `last_used` timestamp is updated to `Utc::now()`.
    pub fn get_active_profile(&mut self, provider: &str) -> Option<&AuthProfile> {
        let now = Utc::now();

        // First pass: clear any expired cooldowns in-place.
        if let Some(entries) = self.profiles.get_mut(provider) {
            for profile in entries.iter_mut() {
                if profile.rate_limit_status == RateLimitStatus::RateLimited {
                    if let Some(until) = profile.cooldown_until {
                        if now >= until {
                            info!(
                                "[AUTH_PROFILES] Cooldown expired for profile '{}' (provider '{}')",
                                profile.id, provider
                            );
                            profile.rate_limit_status = RateLimitStatus::Available;
                            profile.cooldown_until = None;
                        }
                    }
                }
            }
        }

        // Second pass: find first available and stamp last_used.
        if let Some(entries) = self.profiles.get_mut(provider) {
            if let Some(profile) = entries
                .iter_mut()
                .find(|p| p.rate_limit_status == RateLimitStatus::Available)
            {
                profile.last_used = Some(now);
                // Return a shared reference by reborrowing.
                let id = profile.id.clone();
                return self
                    .profiles
                    .get(provider)
                    .and_then(|es| es.iter().find(|p| p.id == id));
            }
        }

        warn!(
            "[AUTH_PROFILES] No available profile for provider '{}'",
            provider
        );
        None
    }

    /// Mark a profile as rate-limited with a cooldown of `default_cooldown_seconds`.
    pub fn mark_rate_limited(&mut self, provider: &str, profile_id: &str) {
        let cooldown = chrono::Duration::seconds(self.default_cooldown_seconds);
        if let Some(profile) = self.find_profile_mut(provider, profile_id) {
            profile.rate_limit_status = RateLimitStatus::RateLimited;
            profile.cooldown_until = Some(Utc::now() + cooldown);
            warn!(
                "[AUTH_PROFILES] Profile '{}' (provider '{}') rate-limited for {}s",
                profile_id, provider, self.default_cooldown_seconds
            );
        }
    }

    /// Mark a profile as failed. Increments the failure counter and sets
    /// status to [`RateLimitStatus::Failed`].
    pub fn mark_failed(&mut self, provider: &str, profile_id: &str) {
        if let Some(profile) = self.find_profile_mut(provider, profile_id) {
            profile.failure_count += 1;
            profile.rate_limit_status = RateLimitStatus::Failed;
            profile.cooldown_until = None;
            warn!(
                "[AUTH_PROFILES] Profile '{}' (provider '{}') marked failed (count: {})",
                profile_id, provider, profile.failure_count
            );
        }
    }

    /// Mark a profile as having succeeded. Resets failure count and status
    /// back to [`RateLimitStatus::Available`].
    pub fn mark_success(&mut self, provider: &str, profile_id: &str) {
        if let Some(profile) = self.find_profile_mut(provider, profile_id) {
            profile.failure_count = 0;
            profile.rate_limit_status = RateLimitStatus::Available;
            profile.cooldown_until = None;
            info!(
                "[AUTH_PROFILES] Profile '{}' (provider '{}') marked successful",
                profile_id, provider
            );
        }
    }

    /// Walk every profile across all providers and promote rate-limited
    /// profiles whose cooldown has expired back to [`RateLimitStatus::Available`].
    pub fn clear_expired_cooldowns(&mut self) {
        let now = Utc::now();
        for (provider, entries) in self.profiles.iter_mut() {
            for profile in entries.iter_mut() {
                if profile.rate_limit_status == RateLimitStatus::RateLimited {
                    if let Some(until) = profile.cooldown_until {
                        if now >= until {
                            info!(
                                "[AUTH_PROFILES] Cooldown expired for profile '{}' (provider '{}')",
                                profile.id, provider
                            );
                            profile.rate_limit_status = RateLimitStatus::Available;
                            profile.cooldown_until = None;
                        }
                    }
                }
            }
        }
    }

    /// List all profiles for a given provider (read-only).
    pub fn list_profiles(&self, provider: &str) -> &[AuthProfile] {
        self.profiles
            .get(provider)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Return the total number of profiles across all providers.
    pub fn profile_count(&self) -> usize {
        self.profiles.values().map(|v| v.len()).sum()
    }

    // ---- internal helpers ----

    fn find_profile_mut(&mut self, provider: &str, profile_id: &str) -> Option<&mut AuthProfile> {
        self.profiles
            .get_mut(provider)
            .and_then(|entries| entries.iter_mut().find(|p| p.id == profile_id))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_profile(id: &str, provider: &str) -> AuthProfile {
        AuthProfile {
            id: id.to_string(),
            provider: provider.to_string(),
            api_key_ref: format!("keyring://{}/{}", provider, id),
            rate_limit_status: RateLimitStatus::Available,
            cooldown_until: None,
            failure_count: 0,
            last_used: None,
        }
    }

    #[test]
    fn test_add_and_list_profiles() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));
        mgr.add_profile(make_profile("key-2", "anthropic"));
        mgr.add_profile(make_profile("key-1", "openai"));

        assert_eq!(mgr.list_profiles("anthropic").len(), 2);
        assert_eq!(mgr.list_profiles("openai").len(), 1);
        assert_eq!(mgr.list_profiles("google").len(), 0);
        assert_eq!(mgr.profile_count(), 3);
    }

    #[test]
    fn test_add_replaces_duplicate() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));

        let mut updated = make_profile("key-1", "anthropic");
        updated.api_key_ref = "keyring://anthropic/key-1-v2".to_string();
        mgr.add_profile(updated);

        assert_eq!(mgr.list_profiles("anthropic").len(), 1);
        assert_eq!(
            mgr.list_profiles("anthropic")[0].api_key_ref,
            "keyring://anthropic/key-1-v2"
        );
    }

    #[test]
    fn test_remove_profile() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));
        mgr.add_profile(make_profile("key-2", "anthropic"));

        assert!(mgr.remove_profile("anthropic", "key-1"));
        assert_eq!(mgr.list_profiles("anthropic").len(), 1);
        assert_eq!(mgr.list_profiles("anthropic")[0].id, "key-2");

        // Removing non-existent returns false.
        assert!(!mgr.remove_profile("anthropic", "key-999"));
        assert!(!mgr.remove_profile("google", "key-1"));
    }

    #[test]
    fn test_remove_last_profile_cleans_provider() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));
        mgr.remove_profile("anthropic", "key-1");

        assert_eq!(mgr.profile_count(), 0);
        assert_eq!(mgr.list_profiles("anthropic").len(), 0);
    }

    #[test]
    fn test_get_active_profile_basic() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));
        mgr.add_profile(make_profile("key-2", "anthropic"));

        let active = mgr.get_active_profile("anthropic").unwrap();
        assert_eq!(active.id, "key-1");
        assert!(active.last_used.is_some());
    }

    #[test]
    fn test_get_active_skips_rate_limited() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));
        mgr.add_profile(make_profile("key-2", "anthropic"));

        mgr.mark_rate_limited("anthropic", "key-1");

        let active = mgr.get_active_profile("anthropic").unwrap();
        assert_eq!(active.id, "key-2");
    }

    #[test]
    fn test_get_active_skips_failed() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));
        mgr.add_profile(make_profile("key-2", "anthropic"));

        mgr.mark_failed("anthropic", "key-1");

        let active = mgr.get_active_profile("anthropic").unwrap();
        assert_eq!(active.id, "key-2");
    }

    #[test]
    fn test_get_active_returns_none_when_all_unavailable() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));

        mgr.mark_failed("anthropic", "key-1");

        assert!(mgr.get_active_profile("anthropic").is_none());
    }

    #[test]
    fn test_get_active_returns_none_for_unknown_provider() {
        let mut mgr = AuthProfileManager::new(60);
        assert!(mgr.get_active_profile("nonexistent").is_none());
    }

    #[test]
    fn test_mark_rate_limited_sets_cooldown() {
        let mut mgr = AuthProfileManager::new(120);
        mgr.add_profile(make_profile("key-1", "anthropic"));

        mgr.mark_rate_limited("anthropic", "key-1");

        let profiles = mgr.list_profiles("anthropic");
        assert_eq!(profiles[0].rate_limit_status, RateLimitStatus::RateLimited);
        assert!(profiles[0].cooldown_until.is_some());
    }

    #[test]
    fn test_mark_failed_increments_counter() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));

        mgr.mark_failed("anthropic", "key-1");
        mgr.mark_failed("anthropic", "key-1");

        let profiles = mgr.list_profiles("anthropic");
        assert_eq!(profiles[0].failure_count, 2);
        assert_eq!(profiles[0].rate_limit_status, RateLimitStatus::Failed);
    }

    #[test]
    fn test_mark_success_resets_state() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));

        mgr.mark_failed("anthropic", "key-1");
        mgr.mark_failed("anthropic", "key-1");
        assert_eq!(mgr.list_profiles("anthropic")[0].failure_count, 2);

        mgr.mark_success("anthropic", "key-1");
        let profiles = mgr.list_profiles("anthropic");
        assert_eq!(profiles[0].failure_count, 0);
        assert_eq!(profiles[0].rate_limit_status, RateLimitStatus::Available);
        assert!(profiles[0].cooldown_until.is_none());
    }

    #[test]
    fn test_expired_cooldown_auto_recovers() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));

        // Manually set a cooldown that has already expired.
        if let Some(profile) = mgr.find_profile_mut("anthropic", "key-1") {
            profile.rate_limit_status = RateLimitStatus::RateLimited;
            profile.cooldown_until = Some(Utc::now() - Duration::seconds(10));
        }

        // get_active_profile should auto-recover the profile.
        let active = mgr.get_active_profile("anthropic").unwrap();
        assert_eq!(active.id, "key-1");
        assert_eq!(active.rate_limit_status, RateLimitStatus::Available);
    }

    #[test]
    fn test_clear_expired_cooldowns() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("key-1", "anthropic"));
        mgr.add_profile(make_profile("key-2", "openai"));

        // Set expired cooldown on key-1.
        if let Some(profile) = mgr.find_profile_mut("anthropic", "key-1") {
            profile.rate_limit_status = RateLimitStatus::RateLimited;
            profile.cooldown_until = Some(Utc::now() - Duration::seconds(10));
        }

        // Set future cooldown on key-2.
        if let Some(profile) = mgr.find_profile_mut("openai", "key-2") {
            profile.rate_limit_status = RateLimitStatus::RateLimited;
            profile.cooldown_until = Some(Utc::now() + Duration::seconds(600));
        }

        mgr.clear_expired_cooldowns();

        assert_eq!(
            mgr.list_profiles("anthropic")[0].rate_limit_status,
            RateLimitStatus::Available
        );
        assert_eq!(
            mgr.list_profiles("openai")[0].rate_limit_status,
            RateLimitStatus::RateLimited
        );
    }

    #[test]
    fn test_rotation_falls_through_to_second_profile() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("primary", "anthropic"));
        mgr.add_profile(make_profile("secondary", "anthropic"));
        mgr.add_profile(make_profile("tertiary", "anthropic"));

        // primary is rate-limited, secondary is failed -> should get tertiary.
        mgr.mark_rate_limited("anthropic", "primary");
        mgr.mark_failed("anthropic", "secondary");

        let active = mgr.get_active_profile("anthropic").unwrap();
        assert_eq!(active.id, "tertiary");
    }

    #[test]
    fn test_multiple_providers_independent() {
        let mut mgr = AuthProfileManager::new(60);
        mgr.add_profile(make_profile("a-key", "anthropic"));
        mgr.add_profile(make_profile("o-key", "openai"));

        mgr.mark_failed("anthropic", "a-key");

        // Anthropic has no available profiles, but OpenAI should be fine.
        assert!(mgr.get_active_profile("anthropic").is_none());
        assert!(mgr.get_active_profile("openai").is_some());
    }
}
