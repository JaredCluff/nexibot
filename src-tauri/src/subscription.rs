//! Knowledge Nexus Subscription Service
//!
//! Provides seamless API credential provisioning through Knowledge Nexus subscriptions.
//! Similar to OpenClaw/Codex subscription experience - no manual API key entry required.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::security::ssrf::{self, SsrfPolicy};

/// Subscription status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SubscriptionStatus {
    /// No active subscription
    Inactive,
    /// Subscription active
    Active,
    /// Subscription expired
    Expired,
    /// Subscription pending activation
    Pending,
}

/// Service provider
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceProvider {
    Anthropic,
    OpenAI,
    Deepgram,
    ElevenLabs,
    Cartesia,
}

impl ServiceProvider {
    pub fn as_str(&self) -> &str {
        match self {
            ServiceProvider::Anthropic => "anthropic",
            ServiceProvider::OpenAI => "openai",
            ServiceProvider::Deepgram => "deepgram",
            ServiceProvider::ElevenLabs => "elevenlabs",
            ServiceProvider::Cartesia => "cartesia",
        }
    }
}

/// Subscription details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub provider: ServiceProvider,
    pub status: SubscriptionStatus,
    pub tier: String,            // "basic", "pro", "enterprise"
    pub expires_at: Option<u64>, // Unix timestamp
}

/// API credentials provisioned through subscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisionedCredentials {
    pub provider: ServiceProvider,
    pub api_key: String,
    pub api_url: Option<String>,
    pub rate_limit: Option<u32>,
    pub expires_at: Option<u64>,
}

/// Knowledge Nexus subscription manager
pub struct SubscriptionManager {
    /// K2K router URL for subscription API
    router_url: String,
    /// Device ID for authentication
    device_id: String,
    /// Active subscriptions
    subscriptions: Arc<RwLock<Vec<Subscription>>>,
    /// Provisioned credentials cache
    credentials: Arc<RwLock<Vec<ProvisionedCredentials>>>,
}

impl SubscriptionManager {
    /// Create a new subscription manager
    pub fn new(router_url: impl Into<String>, device_id: impl Into<String>) -> Self {
        Self {
            router_url: router_url.into(),
            device_id: device_id.into(),
            subscriptions: Arc::new(RwLock::new(Vec::new())),
            credentials: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Check subscription status for a provider
    pub async fn check_subscription(
        &self,
        provider: ServiceProvider,
    ) -> Result<SubscriptionStatus> {
        info!("[SUBSCRIPTION] Checking {} subscription", provider.as_str());

        // Call Knowledge Nexus subscription API
        let client = reqwest::Client::new();
        let url = format!("{}/api/subscriptions/check", self.router_url);
        ssrf::validate_outbound_request(&url, &SsrfPolicy::default(), &[])
            .map_err(|e| anyhow::anyhow!("Subscription check URL blocked by SSRF policy: {}", e))?;

        let response = client
            .get(&url)
            .query(&[
                ("device_id", &self.device_id),
                ("provider", &provider.as_str().to_string()),
            ])
            .send()
            .await
            .context("Failed to check subscription")?;

        if !response.status().is_success() {
            warn!(
                "[SUBSCRIPTION] Subscription check failed: {}",
                response.status()
            );
            return Ok(SubscriptionStatus::Inactive);
        }

        #[derive(Deserialize)]
        struct StatusResponse {
            status: SubscriptionStatus,
            tier: Option<String>,
            expires_at: Option<u64>,
        }

        let status_response: StatusResponse = response
            .json()
            .await
            .context("Failed to parse subscription status")?;

        // Update local subscription cache
        let mut subscriptions = self.subscriptions.write().await;
        subscriptions.retain(|s| s.provider != provider);
        if status_response.status != SubscriptionStatus::Inactive {
            subscriptions.push(Subscription {
                provider: provider.clone(),
                status: status_response.status.clone(),
                tier: status_response.tier.unwrap_or_else(|| "basic".to_string()),
                expires_at: status_response.expires_at,
            });
        }

        Ok(status_response.status)
    }

    /// Provision API credentials for a subscribed service
    ///
    /// This is the key method that provides the "OpenClaw experience":
    /// - Automatic credential provisioning
    /// - No manual API key entry
    /// - Transparent subscription-based access
    pub async fn provision_credentials(
        &self,
        provider: ServiceProvider,
    ) -> Result<ProvisionedCredentials> {
        info!(
            "[SUBSCRIPTION] Provisioning credentials for {}",
            provider.as_str()
        );

        // First check if subscription is active
        let status = self.check_subscription(provider.clone()).await?;
        if status != SubscriptionStatus::Active {
            anyhow::bail!("No active subscription for {}. Please configure a subscription URL and subscribe.", provider.as_str());
        }

        // Request credential provisioning from Knowledge Nexus
        let client = reqwest::Client::new();
        let url = format!("{}/api/subscriptions/provision", self.router_url);
        ssrf::validate_outbound_request(&url, &SsrfPolicy::default(), &[])
            .map_err(|e| anyhow::anyhow!("Subscription provision URL blocked by SSRF policy: {}", e))?;

        let response = client
            .post(&url)
            .json(&serde_json::json!({
                "device_id": self.device_id,
                "provider": provider.as_str(),
            }))
            .send()
            .await
            .context("Failed to provision credentials")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Credential provisioning failed: {}", error_text);
        }

        #[derive(Deserialize)]
        struct ProvisionResponse {
            api_key: String,
            api_url: Option<String>,
            rate_limit: Option<u32>,
            expires_at: Option<u64>,
        }

        let provision_response: ProvisionResponse = response
            .json()
            .await
            .context("Failed to parse provision response")?;

        let credentials = ProvisionedCredentials {
            provider: provider.clone(),
            api_key: provision_response.api_key,
            api_url: provision_response.api_url,
            rate_limit: provision_response.rate_limit,
            expires_at: provision_response.expires_at,
        };

        // Cache credentials
        let mut creds = self.credentials.write().await;
        creds.retain(|c| c.provider != provider);
        creds.push(credentials.clone());

        info!(
            "[SUBSCRIPTION] Credentials provisioned successfully for {}",
            provider.as_str()
        );

        Ok(credentials)
    }

    /// Get cached credentials if available
    pub async fn get_cached_credentials(
        &self,
        provider: &ServiceProvider,
    ) -> Option<ProvisionedCredentials> {
        let creds = self.credentials.read().await;
        creds.iter().find(|c| &c.provider == provider).cloned()
    }

    /// Get or provision credentials
    ///
    /// Returns cached credentials if available and not expired,
    /// otherwise provisions new credentials
    pub async fn get_credentials(
        &self,
        provider: ServiceProvider,
    ) -> Result<ProvisionedCredentials> {
        // Check cache first
        if let Some(cached) = self.get_cached_credentials(&provider).await {
            // Check if expired
            if let Some(expires_at) = cached.expires_at {
                let now = chrono::Utc::now().timestamp() as u64;
                if expires_at > now + 300 {
                    // Valid for at least 5 more minutes
                    return Ok(cached);
                }
            } else {
                // No expiration, credentials are valid
                return Ok(cached);
            }
        }

        // Provision new credentials
        self.provision_credentials(provider).await
    }

    /// List all active subscriptions
    pub async fn list_subscriptions(&self) -> Vec<Subscription> {
        self.subscriptions.read().await.clone()
    }

    /// Open subscription portal in browser
    pub fn open_subscription_portal(&self, provider: Option<ServiceProvider>) -> Result<()> {
        if self.router_url.is_empty() {
            warn!("[SUBSCRIPTION] No subscription portal URL configured");
            anyhow::bail!("No subscription portal URL configured. Set a router_url in your K2K config.");
        }

        let url = if let Some(provider) = provider {
            format!(
                "{}/subscribe?provider={}",
                self.router_url.trim_end_matches('/'),
                provider.as_str()
            )
        } else {
            format!("{}/subscribe", self.router_url.trim_end_matches('/'))
        };

        info!("[SUBSCRIPTION] Opening subscription portal: {}", url);

        // Open URL in default browser
        crate::platform::open_browser(&url)?;

        Ok(())
    }

    /// Refresh all subscriptions
    pub async fn refresh_subscriptions(&self) -> Result<()> {
        info!("[SUBSCRIPTION] Refreshing all subscriptions");

        let providers = vec![
            ServiceProvider::Anthropic,
            ServiceProvider::OpenAI,
            ServiceProvider::Deepgram,
            ServiceProvider::ElevenLabs,
            ServiceProvider::Cartesia,
        ];

        for provider in providers {
            if let Err(e) = self.check_subscription(provider.clone()).await {
                warn!(
                    "[SUBSCRIPTION] Failed to check {} subscription: {}",
                    provider.as_str(),
                    e
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subscription_manager_creation() {
        let manager = SubscriptionManager::new("https://example.com", "test-device-123");
        assert_eq!(manager.router_url, "https://example.com");
        assert_eq!(manager.device_id, "test-device-123");
    }

    #[test]
    fn test_service_provider_str() {
        assert_eq!(ServiceProvider::Anthropic.as_str(), "anthropic");
        assert_eq!(ServiceProvider::OpenAI.as_str(), "openai");
    }
}
