//! OAuth Manager with enhanced token management and multi-provider support.
//!
//! Provides centralized OAuth profile management with:
//! - Automatic token refresh with retry logic
//! - Multi-provider support (Anthropic, Google, OpenAI)
//! - Team/family mode with multiple profiles per provider
//! - Graceful fallback when refresh fails
//! - Per-provider credentials rotation
#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::oauth::{AuthProfile, AuthProfileManager};

/// OAuth configuration for different providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProviderConfig {
    /// Provider ID (anthropic, google, openai)
    pub provider: String,
    /// OAuth client ID
    pub client_id: String,
    /// OAuth client secret (store in keyring, not config)
    pub client_secret: Option<String>,
    /// Redirect URI
    pub redirect_uri: String,
    /// Supported scopes
    pub scopes: Vec<String>,
    /// Token refresh endpoint
    pub token_endpoint: String,
    /// Authorization endpoint
    pub auth_endpoint: String,
}

impl OAuthProviderConfig {
    /// Google OAuth provider config
    pub fn google(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self {
            provider: "google".to_string(),
            client_id,
            client_secret: Some(client_secret),
            redirect_uri,
            scopes: vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ],
            token_endpoint: "https://oauth2.googleapis.com/token".to_string(),
            auth_endpoint: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
        }
    }

    /// Anthropic OAuth provider config
    pub fn anthropic(client_id: String, redirect_uri: String) -> Self {
        Self {
            provider: "anthropic".to_string(),
            client_id,
            client_secret: None,
            redirect_uri,
            scopes: vec!["messages".to_string(), "models".to_string()],
            token_endpoint: "https://console.anthropic.com/v1/oauth/token".to_string(),
            auth_endpoint: "https://console.anthropic.com/oauth/authorize".to_string(),
        }
    }

    /// OpenAI OAuth provider config
    pub fn openai(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self {
            provider: "openai".to_string(),
            client_id,
            client_secret: Some(client_secret),
            redirect_uri,
            scopes: vec![
                "openid".to_string(),
                "profile".to_string(),
                "email".to_string(),
            ],
            token_endpoint: "https://api.openai.com/v1/oauth/token".to_string(),
            auth_endpoint: "https://accounts.openai.com/o/oauth2/v2/auth".to_string(),
        }
    }
}

/// OAuth manager state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthManagerState {
    /// Active provider (e.g., "anthropic", "google", "openai")
    pub active_provider: String,
    /// Active profile name for the provider
    pub active_profile: String,
}

/// OAuth Manager for centralized OAuth profile management
pub struct OAuthManager {
    profile_manager: Arc<RwLock<AuthProfileManager>>,
    state: Arc<RwLock<OAuthManagerState>>,
    /// Serialises token refresh operations so that concurrent callers that
    /// both observe an expired token do not each initiate a separate refresh
    /// exchange (which would invalidate the first refresh token before the
    /// second call can use it).
    refresh_lock: Arc<Mutex<()>>,
}

impl OAuthManager {
    /// Create a new OAuth manager
    pub fn new() -> Result<Self> {
        let profile_manager = AuthProfileManager::load()?;

        let state = OAuthManagerState {
            active_provider: "anthropic".to_string(),
            active_profile: "default".to_string(),
        };

        Ok(Self {
            profile_manager: Arc::new(RwLock::new(profile_manager)),
            state: Arc::new(RwLock::new(state)),
            refresh_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Get the active OAuth profile
    pub async fn get_active_profile(&self) -> Result<Option<AuthProfile>> {
        let state = self.state.read().await;
        let mut manager = self.profile_manager.write().await;

        Ok(manager
            .get_profile(&state.active_provider, &state.active_profile)
            .cloned())
    }

    /// Get a valid access token for the active profile, refreshing if necessary.
    ///
    /// Uses a dedicated mutex around the refresh operation to prevent the
    /// TOCTOU race where two concurrent callers both observe an expired token,
    /// both initiate a refresh, and the second exchange fails because the
    /// refresh token was already consumed by the first.
    ///
    /// Pattern: check → lock → double-check → refresh (if still expired) → unlock.
    pub async fn get_valid_token(&self) -> Result<String> {
        let state = self.state.read().await;
        let provider = state.active_provider.clone();
        let profile_name = state.active_profile.clone();
        drop(state);

        // Fast path: if not expiring, return immediately without taking the refresh lock.
        {
            let mut manager = self.profile_manager.write().await;
            let profile = manager
                .get_profile(&provider, &profile_name)
                .context("Profile not found")?;
            if !profile.is_expiring() {
                return Ok(profile.access_token.clone());
            }
        }

        // Slow path: acquire the refresh lock and double-check inside.
        // Only one goroutine / task proceeds with the actual network call;
        // subsequent waiters will find a fresh token on the second check.
        let _refresh_guard = self.refresh_lock.lock().await;

        // Double-check: a previous waiter may have already refreshed.
        {
            let mut manager = self.profile_manager.write().await;
            let profile = manager
                .get_profile(&provider, &profile_name)
                .context("Profile not found")?;
            if !profile.is_expiring() {
                return Ok(profile.access_token.clone());
            }
        }

        // Token is still expired — perform the refresh under the lock.
        self.refresh_token(&provider, &profile_name).await?;

        // Return the freshly-refreshed token.
        let mut manager = self.profile_manager.write().await;
        let profile = manager
            .get_profile(&provider, &profile_name)
            .context("Profile not found after refresh")?;
        Ok(profile.access_token.clone())
    }

    /// Refresh a token with retry logic
    pub async fn refresh_token(&self, provider: &str, profile_name: &str) -> Result<()> {
        const MAX_RETRIES: u32 = 3;
        let mut retry_count = 0;

        loop {
            let mut manager = self.profile_manager.write().await;

            let previous_profile = {
                let profile = manager
                    .get_profile(provider, profile_name)
                    .context("Profile not found")?;
                let previous = profile.clone();

                match profile.refresh().await {
                    Ok(_) => previous,
                    Err(e) => {
                        drop(manager);

                        retry_count += 1;
                        if retry_count >= MAX_RETRIES {
                            warn!(
                                "[OAUTH] Token refresh failed after {} retries: {}",
                                MAX_RETRIES, e
                            );
                            return Err(e);
                        }

                        debug!(
                            "[OAUTH] Token refresh failed, retrying... (attempt {}/{})",
                            retry_count, MAX_RETRIES
                        );

                        // Exponential backoff: 1s, 2s, 4s (exponent capped at 63)
                        let backoff = std::time::Duration::from_secs(2_u64.pow((retry_count - 1).min(63)));
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                }
            };

            match manager.save() {
                Ok(_) => {
                    info!(
                        "[OAUTH] Token refreshed successfully: {}:{}",
                        provider, profile_name
                    );
                    return Ok(());
                }
                Err(e) => {
                    if let Some(profile) = manager.get_profile(provider, profile_name) {
                        *profile = previous_profile;
                    }
                    debug!(
                        "[OAUTH] Failed to persist refreshed token, restored in-memory profile: {}",
                        e
                    );
                    return Err(e);
                }
            }
        }
    }

    /// Add or update a profile
    pub async fn upsert_profile(&self, profile: AuthProfile) -> Result<()> {
        let mut manager = self.profile_manager.write().await;
        manager.upsert_profile_persisted(profile)?;
        Ok(())
    }

    /// List profiles for a provider
    pub async fn list_profiles(&self, provider: &str) -> Result<Vec<AuthProfile>> {
        let manager = self.profile_manager.read().await;
        Ok(manager
            .list_profiles(provider)
            .into_iter()
            .cloned()
            .collect())
    }

    /// Remove a profile
    pub async fn remove_profile(&self, provider: &str, profile_name: &str) -> Result<()> {
        let mut manager = self.profile_manager.write().await;
        manager.remove_profile_persisted(provider, profile_name)?;
        Ok(())
    }

    /// Set the active provider and profile
    pub async fn set_active(&self, provider: String, profile: String) -> Result<()> {
        let mut manager = self.profile_manager.write().await;

        // Verify profile exists
        if manager.get_profile(&provider, &profile).is_none() {
            anyhow::bail!("Profile not found: {}:{}", provider, profile);
        }

        drop(manager);

        let mut state = self.state.write().await;
        state.active_provider = provider;
        state.active_profile = profile;

        info!(
            "[OAUTH] Active profile set to: {}:{}",
            state.active_provider, state.active_profile
        );

        Ok(())
    }

    /// Get the active provider and profile
    pub async fn get_active(&self) -> (String, String) {
        let state = self.state.read().await;
        (state.active_provider.clone(), state.active_profile.clone())
    }

    /// Check if a profile is available for a provider
    pub async fn has_profile(&self, provider: &str) -> bool {
        let manager = self.profile_manager.read().await;
        !manager.list_profiles(provider).is_empty()
    }

    /// Get provider status
    pub async fn get_provider_status(&self, provider: &str) -> Result<OAuthProviderStatus> {
        let manager = self.profile_manager.read().await;
        let profiles = manager.list_profiles(provider);

        if profiles.is_empty() {
            return Ok(OAuthProviderStatus {
                provider: provider.to_string(),
                has_profiles: false,
                profile_count: 0,
                default_profile: None,
                needs_refresh: false,
            });
        }

        let default = profiles.first().map(|p| p.profile_name.clone());
        let needs_refresh = profiles.iter().any(|p| p.is_expiring());

        Ok(OAuthProviderStatus {
            provider: provider.to_string(),
            has_profiles: true,
            profile_count: profiles.len(),
            default_profile: default,
            needs_refresh,
        })
    }
}

/// OAuth provider status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProviderStatus {
    pub provider: String,
    pub has_profiles: bool,
    pub profile_count: usize,
    pub default_profile: Option<String>,
    pub needs_refresh: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_oauth_manager_creation() {
        let manager = OAuthManager::new();
        assert!(manager.is_ok());
    }

    #[test]
    fn test_provider_config_google() {
        let config = OAuthProviderConfig::google(
            "client-id".to_string(),
            "client-secret".to_string(),
            "http://localhost/callback".to_string(),
        );

        assert_eq!(config.provider, "google");
        assert_eq!(config.client_id, "client-id");
        assert!(config.client_secret.is_some());
        assert_eq!(config.scopes.len(), 3);
    }

    #[test]
    fn test_provider_config_anthropic() {
        let config = OAuthProviderConfig::anthropic(
            "client-id".to_string(),
            "http://localhost/callback".to_string(),
        );

        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.client_id, "client-id");
        assert!(config.client_secret.is_none());
        assert_eq!(config.scopes.len(), 2);
    }

    #[test]
    fn test_provider_config_openai() {
        let config = OAuthProviderConfig::openai(
            "client-id".to_string(),
            "client-secret".to_string(),
            "http://localhost/callback".to_string(),
        );

        assert_eq!(config.provider, "openai");
        assert_eq!(config.scopes.len(), 3);
    }
}
