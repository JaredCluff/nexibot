//! Tauri commands for managing integration credentials.
//!
//! All credential values are stored in the OS keyring. These commands
//! never return secret values — only metadata (service, key_name, scope, label).

use serde::Serialize;
use tracing::{info, warn};

use crate::security::credentials;

/// Strip newlines to prevent log injection via user-supplied strings.
fn sl(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Metadata returned to the frontend (never includes the secret value).
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationCredentialInfo {
    pub service: String,
    pub key_name: String,
    pub scope: String,
    pub label: String,
    pub stored_at: String,
}

impl From<credentials::IntegrationCredentialInfo> for IntegrationCredentialInfo {
    fn from(c: credentials::IntegrationCredentialInfo) -> Self {
        Self {
            service: c.service,
            key_name: c.key_name,
            scope: c.scope,
            label: c.label,
            stored_at: c.stored_at,
        }
    }
}

/// Store an integration credential in the OS keyring.
#[tauri::command]
pub fn store_integration_credential(
    service: String,
    key_name: String,
    value: String,
    scope: String,
    label: String,
) -> Result<(), String> {
    if value.is_empty() {
        return Err("Credential value cannot be empty".to_string());
    }
    credentials::store_integration_credential(&service, &key_name, &value, &scope, &label)?;
    info!(
        "[CREDENTIALS] Stored integration credential: {}.{}",
        sl(&service), sl(&key_name)
    );
    Ok(())
}

/// Delete an integration credential from the OS keyring.
#[tauri::command]
pub fn delete_integration_credential(service: String, key_name: String) -> Result<(), String> {
    credentials::delete_integration_credential(&service, &key_name)?;
    info!(
        "[CREDENTIALS] Deleted integration credential: {}.{}",
        sl(&service), sl(&key_name)
    );
    Ok(())
}

/// List all integration credentials (metadata only, never secret values).
#[tauri::command]
pub fn list_integration_credentials() -> Vec<IntegrationCredentialInfo> {
    credentials::list_integration_credentials()
        .into_iter()
        .map(IntegrationCredentialInfo::from)
        .collect()
}

/// Test whether an integration credential exists and is retrievable.
/// Returns "ok" on success or an error message on failure.
#[tauri::command]
pub fn test_integration_credential(service: String, key_name: String) -> Result<String, String> {
    match credentials::get_integration_credential(&service, &key_name) {
        Ok(Some(_)) => {
            info!(
                "[CREDENTIALS] Test passed for integration credential: {}.{}",
                sl(&service), sl(&key_name)
            );
            Ok("ok".to_string())
        }
        Ok(None) => {
            warn!(
                "[CREDENTIALS] No credential found for: {}.{}",
                sl(&service), sl(&key_name)
            );
            Err(format!("No credential found for {}.{}", sl(&service), sl(&key_name)))
        }
        Err(e) => {
            warn!(
                "[CREDENTIALS] Test failed for {}.{}: {}",
                sl(&service), sl(&key_name), e
            );
            Err(e)
        }
    }
}
