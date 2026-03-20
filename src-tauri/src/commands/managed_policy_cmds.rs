//! Tauri commands for Knowledge Nexus Central Management policy status.

use tauri::State;

use crate::managed_policy::{PolicyStatus, TierCapabilities};

use super::AppState;

/// Return the current managed policy status.
///
/// Includes tier, policy version, last heartbeat age, which features are
/// restricted, and remaining credits / voice minutes.
#[tauri::command]
pub async fn get_managed_policy_status(
    state: State<'_, AppState>,
) -> Result<PolicyStatus, String> {
    Ok(state.managed_policy_manager.get_status().await)
}

/// Trigger an immediate heartbeat to pull the latest policy from the server.
///
/// Useful after a subscription upgrade or when the UI suspects the cached
/// policy is stale.
#[tauri::command]
pub async fn force_policy_refresh(state: State<'_, AppState>) -> Result<(), String> {
    state
        .managed_policy_manager
        .force_refresh()
        .await
        .map_err(|e| e.to_string())
}

/// Return the feature set available at the current subscription tier.
///
/// The frontend uses this to show or hide capability toggles without needing
/// to parse the raw policy.
#[tauri::command]
pub async fn get_tier_capabilities(
    state: State<'_, AppState>,
) -> Result<TierCapabilities, String> {
    Ok(state.managed_policy_manager.get_tier_capabilities().await)
}
