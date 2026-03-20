//! OS-native credential storage via the system keyring.
//!
//! Stores secrets (API keys, tokens) in the OS keyring instead of
//! plaintext YAML. Config files use `"__keyring__"` as a placeholder
//! when the real value is stored in the keyring.

use tracing::debug;

/// The service name used for all keyring entries.
const SERVICE_NAME: &str = "ai.nexibot.desktop";

/// Placeholder value written to config YAML when the secret is in the keyring.
pub const KEYRING_PLACEHOLDER: &str = "__keyring__";

/// Secret keys that should be stored in the keyring.
#[allow(dead_code)]
pub const MANAGED_SECRETS: &[&str] = &[
    "claude.api_key",
    "telegram.bot_token",
    "whatsapp.access_token",
    "webhooks.auth_token",
];

/// Check if the OS keyring is available.
///
/// DISABLED — keyring integration caused persistent macOS Keychain dialog
/// spam and token loss. Secrets are now kept in the config YAML file.
/// To re-enable in the future, gate behind a user opt-in config flag.
pub fn is_keyring_available() -> bool {
    false
}

/// Emit a prominent startup warning when the OS keyring is disabled.
///
/// Because keyring support is intentionally disabled (to avoid macOS Keychain
/// dialog spam and token loss), API keys and tokens are stored in plaintext in
/// config.yaml. This warning ensures the situation is visible in logs.
pub fn warn_if_keyring_disabled() {
    if !is_keyring_available() {
        let config_path = crate::config::NexiBotConfig::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".to_string());
        tracing::warn!(
            "[SECURITY] OS keyring is disabled. API keys and tokens are stored in plaintext \
             in config.yaml. Ensure the config file has restrictive permissions (0600) and \
             is not accessible to other users or processes. \
             File: {}",
            config_path
        );
    }
}

/// Store a secret in the OS keyring.
pub fn store_secret(key: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, key)
        .map_err(|e| format!("Failed to create keyring entry for '{}': {}", key, e))?;

    entry
        .set_password(value)
        .map_err(|e| format!("Failed to store secret '{}': {}", key, e))?;

    debug!("[CREDENTIALS] Stored secret '{}' in keyring", key);
    Ok(())
}

/// Retrieve a secret from the OS keyring.
pub fn get_secret(key: &str) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(SERVICE_NAME, key)
        .map_err(|e| format!("Failed to create keyring entry for '{}': {}", key, e))?;

    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Failed to retrieve secret '{}': {}", key, e)),
    }
}

/// Delete a secret from the OS keyring.
pub fn delete_secret(key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, key)
        .map_err(|e| format!("Failed to create keyring entry for '{}': {}", key, e))?;

    match entry.delete_credential() {
        Ok(()) => {
            debug!("[CREDENTIALS] Deleted secret '{}' from keyring", key);
            Ok(())
        }
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Failed to delete secret '{}': {}", key, e)),
    }
}

/// Resolve keyring placeholders in config values.
///
/// DISABLED — keyring is no longer used. If a field contains `__keyring__`,
/// it means the secret was lost. Clear the placeholder so the user can
/// re-enter the value via the UI.
pub fn resolve_keyring_secrets(
    claude_api_key: &mut Option<String>,
    telegram_bot_token: &mut String,
    whatsapp_access_token: &mut String,
    webhooks_auth_token: &mut Option<String>,
) {
    // Keyring disabled. Clear any stale __keyring__ placeholders.
    if claude_api_key.as_deref() == Some(KEYRING_PLACEHOLDER) {
        debug!("[CREDENTIALS] Clearing stale keyring placeholder for claude.api_key");
        *claude_api_key = None;
    }
    if telegram_bot_token == KEYRING_PLACEHOLDER {
        debug!("[CREDENTIALS] Clearing stale keyring placeholder for telegram.bot_token");
        *telegram_bot_token = String::new();
    }
    if whatsapp_access_token == KEYRING_PLACEHOLDER {
        debug!("[CREDENTIALS] Clearing stale keyring placeholder for whatsapp.access_token");
        *whatsapp_access_token = String::new();
    }
    if webhooks_auth_token.as_deref() == Some(KEYRING_PLACEHOLDER) {
        debug!("[CREDENTIALS] Clearing stale keyring placeholder for webhooks.auth_token");
        *webhooks_auth_token = None;
    }
}

// ─── Integration credential helpers ──────────────────────────────────────────

/// Metadata about an integration credential (never contains the secret value).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IntegrationCredentialInfo {
    /// Service identifier (e.g., "clickup", "google-workspace").
    pub service: String,
    /// Key name within the service (e.g., "api_key", "client_secret").
    pub key_name: String,
    /// Access scope: "readonly" or "readwrite".
    pub scope: String,
    /// Human-readable label (e.g., "ClickUp API Key").
    pub label: String,
    /// When the credential was stored (ISO 8601).
    pub stored_at: String,
}

/// Integration key prefix in the keyring.
const INTEGRATION_PREFIX: &str = "integration.";

/// Path to the integration credentials metadata sidecar file.
fn integration_metadata_path() -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(".config/nexibot/integration-keys.json")
}

/// Load all integration credential metadata from the sidecar file.
fn load_integration_metadata() -> Vec<IntegrationCredentialInfo> {
    let path = integration_metadata_path();
    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Save integration credential metadata to the sidecar file.
fn save_integration_metadata(entries: &[IntegrationCredentialInfo]) -> Result<(), String> {
    let path = integration_metadata_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create metadata directory: {}", e))?;
    }
    let json = serde_json::to_string_pretty(entries)
        .map_err(|e| format!("Failed to serialize metadata: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write metadata: {}", e))?;
    // Restrict to owner-only (0600) — file may contain credential metadata
    if let Err(e) = crate::platform::file_security::restrict_file_permissions(&path) {
        tracing::warn!("[CREDENTIALS] Failed to restrict permissions on metadata file: {}", e);
    }
    Ok(())
}

/// Store an integration credential in the OS keyring with metadata.
pub fn store_integration_credential(
    service: &str,
    key_name: &str,
    value: &str,
    scope: &str,
    label: &str,
) -> Result<(), String> {
    let keyring_key = format!("{}{}.{}", INTEGRATION_PREFIX, service, key_name);
    store_secret(&keyring_key, value)?;

    let mut entries = load_integration_metadata();
    // Remove existing entry for this service+key_name
    entries.retain(|e| !(e.service == service && e.key_name == key_name));
    entries.push(IntegrationCredentialInfo {
        service: service.to_string(),
        key_name: key_name.to_string(),
        scope: scope.to_string(),
        label: label.to_string(),
        stored_at: chrono::Utc::now().to_rfc3339(),
    });
    save_integration_metadata(&entries)?;

    debug!(
        "[CREDENTIALS] Stored integration credential: {}.{}",
        service, key_name
    );
    Ok(())
}

/// Delete an integration credential from the keyring and metadata.
pub fn delete_integration_credential(service: &str, key_name: &str) -> Result<(), String> {
    let keyring_key = format!("{}{}.{}", INTEGRATION_PREFIX, service, key_name);
    delete_secret(&keyring_key)?;

    let mut entries = load_integration_metadata();
    entries.retain(|e| !(e.service == service && e.key_name == key_name));
    save_integration_metadata(&entries)?;

    debug!(
        "[CREDENTIALS] Deleted integration credential: {}.{}",
        service, key_name
    );
    Ok(())
}

/// List all integration credentials (metadata only, never the secret value).
pub fn list_integration_credentials() -> Vec<IntegrationCredentialInfo> {
    load_integration_metadata()
}

/// Retrieve an integration credential value from the keyring.
pub fn get_integration_credential(service: &str, key_name: &str) -> Result<Option<String>, String> {
    let keyring_key = format!("{}{}.{}", INTEGRATION_PREFIX, service, key_name);
    get_secret(&keyring_key)
}

/// Resolve a set of declared environment variable names to their integration values.
///
/// Given env var names like `["CLICKUP_API_KEY"]`, maps each to the corresponding
/// integration keyring entry using the convention:
///   CLICKUP_API_KEY -> integration.clickup.api_key
///
/// Returns only the variables that have stored values.
pub fn resolve_skill_env_vars(
    declared_vars: &[String],
) -> std::collections::HashMap<String, String> {
    let mut resolved = std::collections::HashMap::new();

    for var_name in declared_vars {
        // Convention: ENV_VAR_NAME -> service.key_name
        // e.g., CLICKUP_API_KEY -> clickup.api_key
        //       GOOGLE_CLIENT_SECRET -> google.client_secret
        //       SERVICENOW_PASSWORD -> servicenow.password
        let lower = var_name.to_lowercase();

        // Try to split into service + key: find the first underscore that separates service from key
        // Known service prefixes to try (longest first for correct matching)
        let service_prefixes = [
            ("microsoft365_", "microsoft365"),
            ("google_workspace_", "google-workspace"),
            ("google_", "google-workspace"),
            ("servicenow_", "servicenow"),
            ("salesforce_", "salesforce"),
            ("atlassian_", "atlassian"),
            ("clickup_", "clickup"),
            ("monday_", "monday"),
        ];

        let mut found = false;
        for (prefix, service) in &service_prefixes {
            if lower.starts_with(prefix) {
                let key_name = &lower[prefix.len()..];
                if !key_name.is_empty() {
                    if let Ok(Some(value)) = get_integration_credential(service, key_name) {
                        resolved.insert(var_name.clone(), value);
                        found = true;
                    }
                    break;
                }
            }
        }

        if !found {
            // Generic fallback: split on first underscore
            if let Some(idx) = lower.find('_') {
                let service = &lower[..idx];
                let key_name = &lower[idx + 1..];
                if !key_name.is_empty() {
                    if let Ok(Some(value)) = get_integration_credential(service, key_name) {
                        resolved.insert(var_name.clone(), value);
                    }
                }
            }
        }
    }

    resolved
}

/// Move secrets from config values into the keyring, replacing them with placeholders.
///
/// DISABLED — keyring integration is turned off. Secrets stay in the YAML config file.
/// Returns 0 (no secrets stored).
pub fn store_secrets_to_keyring(
    _claude_api_key: &mut Option<String>,
    _telegram_bot_token: &mut String,
    _whatsapp_access_token: &mut String,
    _webhooks_auth_token: &mut Option<String>,
) -> usize {
    // Keyring disabled — secrets remain in config YAML.
    0
}
