//! OAuth authentication for Claude Pro/Max and ChatGPT Plus subscriptions

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

#[derive(Clone, Serialize, Deserialize)]
pub struct AuthProfile {
    pub provider: String, // "anthropic" or "openai"
    pub profile_name: String,
    /// OAuth access token — never include in log output.
    pub access_token: String,
    /// OAuth refresh token — never include in log output.
    pub refresh_token: Option<String>,
    pub expires_at: u64,    // Unix timestamp
    #[allow(dead_code)]
    pub token_type: String, // "Bearer"
    pub scope: Option<String>,
}

/// Manual Debug implementation that redacts sensitive token fields so that
/// `{:?}` formatting never leaks token values into log output.
impl std::fmt::Debug for AuthProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthProfile")
            .field("provider", &self.provider)
            .field("profile_name", &self.profile_name)
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_at", &self.expires_at)
            .field("token_type", &self.token_type)
            .field("scope", &self.scope)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
pub struct TokenRefreshResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    #[allow(dead_code)]
    pub token_type: String,
}

impl AuthProfile {
    /// Create a new auth profile from OAuth tokens
    pub fn new(
        provider: impl Into<String>,
        profile_name: impl Into<String>,
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        expires_in: u64,
    ) -> Self {
        let now = chrono::Utc::now().timestamp() as u64;
        Self {
            provider: provider.into(),
            profile_name: profile_name.into(),
            access_token: access_token.into(),
            refresh_token,
            expires_at: now + expires_in,
            token_type: "Bearer".to_string(),
            scope: None,
        }
    }

    /// Check if the access token is expired or expiring soon (within 5 minutes)
    pub fn is_expiring(&self) -> bool {
        let now = chrono::Utc::now().timestamp() as u64;
        let buffer = 300; // 5 minutes
        self.expires_at <= (now + buffer)
    }

    /// Refresh the access token using the refresh token
    pub async fn refresh(&mut self) -> Result<()> {
        let refresh_token = self
            .refresh_token
            .as_ref()
            .context("No refresh token available")?
            .clone();

        match self.provider.as_str() {
            "anthropic" => {
                self.refresh_anthropic(&refresh_token).await?;
            }
            "openai" => {
                self.refresh_openai(&refresh_token).await?;
            }
            _ => {
                anyhow::bail!("Unknown provider: {}", self.provider);
            }
        }

        Ok(())
    }

    /// Refresh Anthropic OAuth token
    async fn refresh_anthropic(&mut self, refresh_token: &str) -> Result<()> {
        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());

        let response = client
            .post("https://console.anthropic.com/v1/oauth/token")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            }))
            .send()
            .await
            .context("Failed to refresh Anthropic token")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Token refresh failed: {}", error_text);
        }

        let token_response: TokenRefreshResponse = response
            .json()
            .await
            .context("Failed to parse token response")?;

        // Update token info
        let now = chrono::Utc::now().timestamp() as u64;
        self.access_token = token_response.access_token;
        if let Some(new_refresh) = token_response.refresh_token {
            self.refresh_token = Some(new_refresh);
        }
        self.expires_at = now + token_response.expires_in;

        info!("Anthropic OAuth token refreshed successfully");
        Ok(())
    }

    /// Refresh OpenAI OAuth token via Auth0
    async fn refresh_openai(&mut self, refresh_token: &str) -> Result<()> {
        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());

        // OpenAI uses auth.openai.com — same as Codex CLI
        let response = client
            .post("https://auth.openai.com/oauth/token")
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "client_id": "app_EMoamEEZ73f0CkXaXp7hrann",
                "refresh_token": refresh_token,
            }))
            .send()
            .await
            .context("Failed to refresh OpenAI token")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Token refresh failed: {}", error_text);
        }

        let token_response: TokenRefreshResponse = response
            .json()
            .await
            .context("Failed to parse token response")?;

        // Update token info
        let now = chrono::Utc::now().timestamp() as u64;
        self.access_token = token_response.access_token;
        if let Some(new_refresh) = token_response.refresh_token {
            self.refresh_token = Some(new_refresh);
        }
        self.expires_at = now + token_response.expires_in;

        info!("OpenAI OAuth token refreshed successfully");
        Ok(())
    }

    /// Get a valid access token, refreshing if necessary
    pub async fn get_valid_token(&mut self) -> Result<String> {
        if self.is_expiring() {
            warn!("Access token expiring soon, refreshing...");
            self.refresh().await?;
        }

        Ok(self.access_token.clone())
    }
}

/// Auth profiles manager
pub struct AuthProfileManager {
    profiles: Vec<AuthProfile>,
}

impl AuthProfileManager {
    /// Create a new auth profile manager
    pub fn new() -> Self {
        Self {
            profiles: Vec::new(),
        }
    }

    /// Load auth profiles from disk
    pub fn load() -> Result<Self> {
        let path = Self::profiles_path();

        if !path.exists() {
            return Ok(Self::new());
        }

        let content = std::fs::read_to_string(&path).context("Failed to read auth profiles")?;

        let profiles: Vec<AuthProfile> =
            serde_json::from_str(&content).context("Failed to parse auth profiles")?;

        Ok(Self { profiles })
    }

    /// Save auth profiles to disk.
    ///
    /// Tokens are stored in a JSON file with 0600 permissions. This is
    /// acceptable as a fallback when the OS keyring is unavailable, but the
    /// file must have the most restrictive permissions possible.
    ///
    /// IMPORTANT: Do NOT log token values or the serialized file contents.
    pub fn save(&self) -> Result<()> {
        let path = Self::profiles_path();
        // Log path and count only — never log token values or raw file content.
        tracing::info!(
            "[OAUTH] Saving {} auth profile(s) to: {:?}",
            self.profiles.len(),
            path
        );

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create auth profiles directory")?;
        }

        let content = serde_json::to_string_pretty(&self.profiles)
            .context("Failed to serialize auth profiles")?;

        std::fs::write(&path, &content).context("Failed to write auth profiles")?;

        // Set restrictive permissions on auth profiles (contains tokens)
        crate::platform::file_security::restrict_file_permissions(&path)
            .context("Failed to set permissions on auth profiles")?;

        tracing::info!("[OAUTH] Auth profiles saved successfully");

        Ok(())
    }

    /// Get path to auth profiles file.
    ///
    /// Uses the platform data-local directory so the file is always written to
    /// a user-writable location:
    ///   macOS:   ~/Library/Application Support/ai.nexibot.desktop/
    ///   Windows: %APPDATA%\nexibot\desktop\
    ///   Linux:   ~/.local/share/nexibot/desktop/
    ///
    /// NOTE: config_dir() is intentionally NOT used here — on macOS that maps
    /// to ~/Library/Preferences/ which is restricted to plist files and will
    /// cause a permission error when writing arbitrary JSON.
    fn profiles_path() -> PathBuf {
        directories::ProjectDirs::from("ai", "nexibot", "desktop")
            .map(|dirs| dirs.data_local_dir().to_path_buf())
            .unwrap_or_else(|| {
                // Fallback to home directory if ProjectDirs fails
                dirs::home_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join(".local")
                    .join("share")
                    .join("nexibot")
            })
            .join("auth-profiles.json")
    }

    /// Add or update an auth profile
    pub fn upsert_profile(&mut self, profile: AuthProfile) {
        // Remove existing profile with same provider and name
        self.profiles.retain(|p| {
            !(p.provider == profile.provider && p.profile_name == profile.profile_name)
        });

        self.profiles.push(profile);
    }

    /// Add/update a profile and persist atomically.
    ///
    /// Rolls back in-memory profile changes if disk persistence fails.
    #[allow(dead_code)]
    pub fn upsert_profile_persisted(&mut self, profile: AuthProfile) -> Result<()> {
        let previous_profiles = self.profiles.clone();
        self.upsert_profile(profile);
        if let Err(e) = self.save() {
            self.profiles = previous_profiles;
            return Err(e);
        }
        Ok(())
    }

    /// Get a profile by provider and name
    pub fn get_profile(&mut self, provider: &str, name: &str) -> Option<&mut AuthProfile> {
        self.profiles
            .iter_mut()
            .find(|p| p.provider == provider && p.profile_name == name)
    }

    /// Get the default profile for a provider
    pub fn get_default_profile(&mut self, provider: &str) -> Option<&mut AuthProfile> {
        self.profiles.iter_mut().find(|p| p.provider == provider)
    }

    /// Remove a profile
    pub fn remove_profile(&mut self, provider: &str, name: &str) {
        self.profiles
            .retain(|p| !(p.provider == provider && p.profile_name == name));
    }

    /// Remove a profile and persist atomically.
    ///
    /// Rolls back in-memory profile changes if disk persistence fails.
    #[allow(dead_code)]
    pub fn remove_profile_persisted(&mut self, provider: &str, name: &str) -> Result<()> {
        let previous_profiles = self.profiles.clone();
        self.remove_profile(provider, name);
        if let Err(e) = self.save() {
            self.profiles = previous_profiles;
            return Err(e);
        }
        Ok(())
    }

    /// List all profiles for a provider
    pub fn list_profiles(&self, provider: &str) -> Vec<&AuthProfile> {
        self.profiles
            .iter()
            .filter(|p| p.provider == provider)
            .collect()
    }

    /// List all profiles across all providers (immutable)
    pub fn all_profiles(&self) -> &[AuthProfile] {
        &self.profiles
    }

    /// List all profiles across all providers (mutable)
    pub fn all_profiles_mut(&mut self) -> &mut Vec<AuthProfile> {
        &mut self.profiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_expiring() {
        let now = chrono::Utc::now().timestamp() as u64;

        // Token expires in 10 minutes - should not be expiring
        let profile = AuthProfile {
            provider: "anthropic".to_string(),
            profile_name: "default".to_string(),
            access_token: "test".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: now + 600,
            token_type: "Bearer".to_string(),
            scope: None,
        };
        assert!(!profile.is_expiring());

        // Token expires in 2 minutes - should be expiring (within 5 min buffer)
        let profile2 = AuthProfile {
            expires_at: now + 120,
            ..profile
        };
        assert!(profile2.is_expiring());
    }
}
