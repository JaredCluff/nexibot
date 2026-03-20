//! Tauri commands for the Smart Key Vault settings UI.

use tauri::State;
use tracing::info;

use crate::security::key_vault::VaultEntry;

use super::AppState;

/// List all vault entries (proxy key, format, label, use count, timestamps).
/// Real keys are never included.
#[tauri::command]
pub async fn list_vault_entries(state: State<'_, AppState>) -> Result<Vec<VaultEntry>, String> {
    let interceptor = &state.key_interceptor;
    Ok(interceptor.vault().list())
}

/// Revoke a proxy key. After revocation, tool calls using this proxy key
/// will receive an empty/error key instead of the real one.
#[tauri::command]
pub async fn revoke_vault_entry(
    proxy_key: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let interceptor = &state.key_interceptor;
    interceptor
        .vault()
        .revoke(&proxy_key)
        .map_err(|e| e.to_string())?;
    info!(
        "[KEY_VAULT] Revoked via UI: {}",
        &proxy_key[..proxy_key.len().min(24)]
    );
    Ok(())
}

/// Assign or update the user-defined label for a vault entry.
#[tauri::command]
pub async fn label_vault_entry(
    proxy_key: String,
    label: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let interceptor = &state.key_interceptor;
    interceptor
        .vault()
        .set_label(&proxy_key, &label)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Check whether a proxy key is still active (i.e., resolves to a real key).
#[tauri::command]
pub async fn test_vault_resolve(
    proxy_key: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let interceptor = &state.key_interceptor;
    let result = interceptor
        .vault()
        .resolve(&proxy_key)
        .map_err(|e| e.to_string())?;
    Ok(result.is_some())
}
