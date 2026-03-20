//! API Key Rotation and Management
//!
//! Manages multiple API keys per provider with automatic rotation scheduling,
//! expiry tracking, and fallback mechanisms.
//!
//! # Features
//!
//! - Multiple keys per provider with rotation schedule
//! - Automatic expiry detection and notification
//! - Fallback key selection on primary key failure
//! - Key metadata tracking (created, expires, last used)
//! - Audit logging for all key operations
//! - Manual and scheduled rotation triggers

use chrono::{DateTime, Duration, Utc};
use secrecy::{ExposeSecret, Secret};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Supported API key providers
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyProvider {
    Claude,
    OpenAI,
    Anthropic,
    Deepgram,
    ElevenLabs,
    Custom(String),
}

impl std::fmt::Display for KeyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyProvider::Claude => write!(f, "claude"),
            KeyProvider::OpenAI => write!(f, "openai"),
            KeyProvider::Anthropic => write!(f, "anthropic"),
            KeyProvider::Deepgram => write!(f, "deepgram"),
            KeyProvider::ElevenLabs => write!(f, "elevenlabs"),
            KeyProvider::Custom(name) => write!(f, "{}", name),
        }
    }
}

/// Returns an empty `Secret<String>` used as the serde `default` for the
/// `ApiKey::key` field, which is skipped during (de)serialization.
fn default_secret_key() -> Secret<String> {
    Secret::new(String::new())
}

/// API Key metadata and rotation information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    /// Unique key ID
    pub id: String,

    /// Provider name
    pub provider: KeyProvider,

    /// The actual API key, wrapped in a `Secret` so that accidental `{:?}`
    /// formatting never exposes the raw value in log output.
    ///
    /// The field is skipped during serialisation/deserialisation — key
    /// material must be loaded via a separate secure path (keyring, vault).
    #[serde(skip, default = "default_secret_key")]
    pub key: Secret<String>,

    /// When this key was created
    pub created_at: DateTime<Utc>,

    /// When this key expires (None = no expiry)
    pub expires_at: Option<DateTime<Utc>>,

    /// When this key was last used
    pub last_used: Option<DateTime<Utc>>,

    /// How many times this key has been used
    pub usage_count: u64,

    /// Is this the currently active key?
    pub is_active: bool,

    /// Is this key enabled?
    pub is_enabled: bool,

    /// Human-readable label
    pub label: Option<String>,

    /// Metadata tags (e.g., "production", "staging")
    pub tags: Vec<String>,
}

/// Key rotation schedule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationSchedule {
    /// Provider to rotate
    pub provider: KeyProvider,

    /// Rotate every N days
    pub rotate_days: u32,

    /// Days before expiry to warn
    pub warn_days: u32,

    /// Automatically rotate on expiry
    pub auto_rotate: bool,

    /// Cron expression for custom schedules (optional)
    pub cron_expression: Option<String>,

    /// Next scheduled rotation
    pub next_rotation: DateTime<Utc>,
}

/// Key rotation status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationStatus {
    pub provider: KeyProvider,
    pub current_key_id: String,
    pub active_key_age_days: u32,
    pub next_rotation: DateTime<Utc>,
    pub expiry_warning: Option<String>,
    pub fallback_keys_available: usize,
}

/// API Key Manager
pub struct KeyRotationManager {
    keys: Arc<RwLock<HashMap<KeyProvider, Vec<ApiKey>>>>,
    schedules: Arc<RwLock<HashMap<KeyProvider, RotationSchedule>>>,
    audit_log: Arc<RwLock<Vec<KeyAuditEntry>>>,
}

/// Audit entry for key operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyAuditEntry {
    pub timestamp: DateTime<Utc>,
    pub provider: KeyProvider,
    pub key_id: String,
    pub action: String,
    pub details: String,
}

impl KeyRotationManager {
    /// Create a new key rotation manager
    pub fn new() -> Self {
        Self {
            keys: Arc::new(RwLock::new(HashMap::new())),
            schedules: Arc::new(RwLock::new(HashMap::new())),
            audit_log: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a new API key
    pub async fn add_key(
        &self,
        provider: KeyProvider,
        key: String,
        label: Option<String>,
        expires_at: Option<DateTime<Utc>>,
        tags: Vec<String>,
    ) -> Result<String, String> {
        let key_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        let api_key = ApiKey {
            id: key_id.clone(),
            provider: provider.clone(),
            key: Secret::new(key),
            created_at: now,
            expires_at,
            last_used: None,
            usage_count: 0,
            is_active: false, // Will be activated when needed
            is_enabled: true,
            label,
            tags,
        };

        let mut keys = self.keys.write().await;
        keys.entry(provider.clone())
            .or_insert_with(Vec::new)
            .push(api_key);

        // Log the action
        self.log_audit(KeyAuditEntry {
            timestamp: now,
            provider: provider.clone(),
            key_id: key_id.clone(),
            action: "add_key".to_string(),
            details: "New API key added".to_string(),
        })
        .await;

        info!("[KEY_ROTATION] Added new key for {}: {}", provider, key_id);
        Ok(key_id)
    }

    /// Get the active API key for a provider
    pub async fn get_active_key(&self, provider: &KeyProvider) -> Result<String, String> {
        let mut keys = self.keys.write().await;

        // Find active key
        if let Some(provider_keys) = keys.get_mut(provider) {
            // Look for active, enabled, non-expired key
            for key in provider_keys.iter_mut() {
                if key.is_active && key.is_enabled {
                    if let Some(expires) = key.expires_at {
                        if expires > Utc::now() {
                            // Update last used and usage count
                            key.last_used = Some(Utc::now());
                            key.usage_count += 1;
                            return Ok(key.key.expose_secret().clone());
                        }
                    } else {
                        // No expiry, use it
                        key.last_used = Some(Utc::now());
                        key.usage_count += 1;
                        return Ok(key.key.expose_secret().clone());
                    }
                }
            }

            // No active key, find best fallback
            for key in provider_keys.iter_mut() {
                if key.is_enabled {
                    if let Some(expires) = key.expires_at {
                        if expires > Utc::now() {
                            // Switch to fallback
                            warn!(
                                "[KEY_ROTATION] Active key expired, switching to fallback for {}",
                                provider
                            );
                            key.is_active = true;
                            key.last_used = Some(Utc::now());
                            key.usage_count += 1;
                            return Ok(key.key.expose_secret().clone());
                        }
                    } else {
                        // Switch to this key
                        key.is_active = true;
                        key.last_used = Some(Utc::now());
                        key.usage_count += 1;
                        return Ok(key.key.expose_secret().clone());
                    }
                }
            }
        }

        Err(format!("No valid API key found for provider: {}", provider))
    }

    /// Set a key as active
    pub async fn activate_key(&self, provider: &KeyProvider, key_id: &str) -> Result<(), String> {
        let mut keys = self.keys.write().await;

        if let Some(provider_keys) = keys.get_mut(provider) {
            // Deactivate others
            for key in provider_keys.iter_mut() {
                key.is_active = false;
            }

            // Activate this one
            if let Some(key) = provider_keys.iter_mut().find(|k| k.id == key_id) {
                if !key.is_enabled {
                    return Err("Cannot activate disabled key".to_string());
                }
                key.is_active = true;

                self.log_audit(KeyAuditEntry {
                    timestamp: Utc::now(),
                    provider: provider.clone(),
                    key_id: key_id.to_string(),
                    action: "activate_key".to_string(),
                    details: "Key activated".to_string(),
                })
                .await;

                info!("[KEY_ROTATION] Activated key {} for {}", key_id, provider);
                return Ok(());
            }
        }

        Err("Key not found".to_string())
    }

    /// Rotate to a new key
    pub async fn rotate_key(
        &self,
        provider: &KeyProvider,
        new_key: String,
    ) -> Result<String, String> {
        // Add new key
        let key_id = self
            .add_key(
                provider.clone(),
                new_key,
                None,
                None,
                vec!["rotated".to_string()],
            )
            .await?;

        // Activate it
        self.activate_key(provider, &key_id).await?;

        self.log_audit(KeyAuditEntry {
            timestamp: Utc::now(),
            provider: provider.clone(),
            key_id: key_id.clone(),
            action: "rotate_key".to_string(),
            details: "Key rotated".to_string(),
        })
        .await;

        info!("[KEY_ROTATION] Rotated key for {}", provider);
        Ok(key_id)
    }

    /// Disable a key (revoke)
    pub async fn disable_key(&self, provider: &KeyProvider, key_id: &str) -> Result<(), String> {
        let mut keys = self.keys.write().await;

        if let Some(provider_keys) = keys.get_mut(provider) {
            if let Some(key) = provider_keys.iter_mut().find(|k| k.id == key_id) {
                key.is_enabled = false;
                if key.is_active {
                    key.is_active = false;
                }

                self.log_audit(KeyAuditEntry {
                    timestamp: Utc::now(),
                    provider: provider.clone(),
                    key_id: key_id.to_string(),
                    action: "disable_key".to_string(),
                    details: "Key disabled".to_string(),
                })
                .await;

                info!("[KEY_ROTATION] Disabled key {} for {}", key_id, provider);
                return Ok(());
            }
        }

        Err("Key not found".to_string())
    }

    /// Get all keys for a provider (returns without key values)
    pub async fn list_keys(
        &self,
        provider: &KeyProvider,
    ) -> Result<Vec<(String, String, Option<DateTime<Utc>>, u64, bool)>, String> {
        let keys = self.keys.read().await;

        if let Some(provider_keys) = keys.get(provider) {
            let result = provider_keys
                .iter()
                .map(|k| {
                    (
                        k.id.clone(),
                        k.label.clone().unwrap_or_else(|| "Unnamed key".to_string()),
                        k.expires_at,
                        k.usage_count,
                        k.is_active,
                    )
                })
                .collect();
            Ok(result)
        } else {
            Ok(Vec::new())
        }
    }

    /// Get rotation status for all providers
    pub async fn get_rotation_status(&self) -> Vec<RotationStatus> {
        let keys = self.keys.read().await;
        let schedules = self.schedules.read().await;

        let mut statuses = Vec::new();

        for (provider, provider_keys) in keys.iter() {
            if provider_keys.is_empty() {
                continue;
            }

            if let Some(active_key) = provider_keys.iter().find(|k| k.is_active) {
                let age_days = (Utc::now() - active_key.created_at).num_days().max(0) as u32;

                let expiry_warning = if let Some(expires) = active_key.expires_at {
                    let days_until_expiry = (expires - Utc::now()).num_days();
                    if days_until_expiry <= 7 {
                        Some(format!("Key expires in {} days", days_until_expiry))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let fallback_count = provider_keys
                    .iter()
                    .filter(|k| k.is_enabled && !k.is_active)
                    .count();

                let next_rotation = schedules
                    .get(provider)
                    .map(|s| s.next_rotation)
                    .unwrap_or_else(|| Utc::now() + Duration::days(30));

                statuses.push(RotationStatus {
                    provider: provider.clone(),
                    current_key_id: active_key.id.clone(),
                    active_key_age_days: age_days,
                    next_rotation,
                    expiry_warning,
                    fallback_keys_available: fallback_count,
                });
            }
        }

        statuses
    }

    /// Set rotation schedule for a provider
    pub async fn set_rotation_schedule(
        &self,
        provider: KeyProvider,
        rotate_days: u32,
        warn_days: u32,
        auto_rotate: bool,
    ) -> Result<(), String> {
        let schedule = RotationSchedule {
            provider: provider.clone(),
            rotate_days,
            warn_days,
            auto_rotate,
            cron_expression: None,
            next_rotation: Utc::now() + Duration::days(rotate_days as i64),
        };

        let mut schedules = self.schedules.write().await;
        schedules.insert(provider.clone(), schedule);

        info!(
            "[KEY_ROTATION] Set rotation schedule for {}: every {} days",
            provider, rotate_days
        );
        Ok(())
    }

    /// Get audit log
    pub async fn get_audit_log(&self) -> Vec<KeyAuditEntry> {
        let log = self.audit_log.read().await;
        log.clone()
    }

    /// Log an audit entry
    async fn log_audit(&self, entry: KeyAuditEntry) {
        let mut log = self.audit_log.write().await;
        log.push(entry);

        // Keep only last 1000 entries — drain in bulk rather than
        // removing one-by-one (Vec::remove(0) is O(n) per call).
        if log.len() > 1000 {
            let excess = log.len() - 1000;
            log.drain(0..excess);
        }
    }

    /// Check rotation schedules and rotate keys that are past their scheduled
    /// rotation time.
    ///
    /// This is called periodically (e.g. daily) by the background scheduler
    /// spawned in `main.rs`.  For each provider with an `auto_rotate` schedule
    /// whose `next_rotation` timestamp has passed, the method emits a warning
    /// so that the monitoring layer can alert an admin.  Generating a new key
    /// automatically is not possible without external input, so an explicit
    /// alert is the appropriate action here.
    pub async fn check_rotation_due(&self) {
        let schedules = self.schedules.read().await;
        let now = Utc::now();

        for (provider, schedule) in schedules.iter() {
            if schedule.auto_rotate && now >= schedule.next_rotation {
                warn!(
                    "[KEY_ROTATION] Scheduled rotation is overdue for provider '{}' \
                     (was due at {}). Please rotate the key.",
                    provider, schedule.next_rotation
                );
            }
        }
    }

    /// Check for expired keys and issue warnings
    pub async fn check_expiry_warnings(&self) -> Vec<(KeyProvider, String)> {
        let keys = self.keys.read().await;
        let schedules = self.schedules.read().await;
        let mut warnings = Vec::new();

        for (provider, provider_keys) in keys.iter() {
            if let Some(schedule) = schedules.get(provider) {
                for key in provider_keys.iter() {
                    if !key.is_enabled {
                        continue;
                    }

                    if let Some(expires) = key.expires_at {
                        let days_until_expiry = (expires - Utc::now()).num_days();

                        if days_until_expiry <= schedule.warn_days as i64 && days_until_expiry > 0 {
                            let warning = format!(
                                "API key for {} expires in {} days",
                                provider, days_until_expiry
                            );
                            warnings.push((provider.clone(), warning));
                            warn!("[KEY_ROTATION] {}", warnings.last().unwrap().1);
                        } else if days_until_expiry <= 0 {
                            error!("[KEY_ROTATION] API key for {} has EXPIRED", provider);
                        }
                    }
                }
            }
        }

        warnings
    }
}

impl Default for KeyRotationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_add_and_get_key() {
        let manager = KeyRotationManager::new();

        let key_id = manager
            .add_key(
                KeyProvider::Claude,
                "sk-test-key".to_string(),
                Some("Test Key".to_string()),
                None,
                vec![],
            )
            .await
            .unwrap();

        manager
            .activate_key(&KeyProvider::Claude, &key_id)
            .await
            .unwrap();

        let key = manager.get_active_key(&KeyProvider::Claude).await.unwrap();
        assert_eq!(key, "sk-test-key");
    }

    #[tokio::test]
    async fn test_rotate_key() {
        let manager = KeyRotationManager::new();

        manager
            .add_key(
                KeyProvider::Claude,
                "old-key".to_string(),
                None,
                None,
                vec![],
            )
            .await
            .unwrap();

        manager
            .rotate_key(&KeyProvider::Claude, "new-key".to_string())
            .await
            .unwrap();

        let key = manager.get_active_key(&KeyProvider::Claude).await.unwrap();
        assert_eq!(key, "new-key");
    }

    #[tokio::test]
    async fn test_fallback_key() {
        let manager = KeyRotationManager::new();

        let key1_id = manager
            .add_key(KeyProvider::Claude, "key1".to_string(), None, None, vec![])
            .await
            .unwrap();

        let key2_id = manager
            .add_key(KeyProvider::Claude, "key2".to_string(), None, None, vec![])
            .await
            .unwrap();

        manager
            .activate_key(&KeyProvider::Claude, &key1_id)
            .await
            .unwrap();

        // Disable active key
        manager
            .disable_key(&KeyProvider::Claude, &key1_id)
            .await
            .unwrap();

        // Should fallback to key2
        let key = manager.get_active_key(&KeyProvider::Claude).await.unwrap();
        assert_eq!(key, "key2");
    }
}
