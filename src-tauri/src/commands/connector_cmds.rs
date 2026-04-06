//! Connector wizard Tauri commands.
//!
//! These commands back the `ConnectorWizard` UI component.  They call the
//! Knowledge Nexus API gateway (`/connectors/*`) using the user's stored
//! auth token.  OAuth happens server-side; NexiBot only opens a browser window
//! and then listens for the `nexibot://oauth-complete` deep-link callback.

use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::{info, warn};

use super::AppState;

// ── URL encoding helper ───────────────────────────────────────────────────────

/// URL-encode a path segment, refusing to allow path traversal via
/// un-encoded slashes or other characters that have special meaning in URLs.
fn encode_path_segment(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

// ── Response types ────────────────────────────────────────────────────────────

/// Metadata for a single supported connector type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorMeta {
    pub connector_type: String,
    pub name: String,
    pub icon: String,
    pub category: String,
    pub capabilities: Vec<String>,
    pub auth_provider: String,
}

/// A user's configured connector row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConnector {
    pub id: String,
    pub connector_type: String,
    pub name: String,
    pub status: String,
    pub sync_enabled: bool,
    pub last_auth_at: Option<String>,
    pub last_error: Option<String>,
}

/// Sync status for a connector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorSyncStatus {
    pub id: String,
    pub connector_type: String,
    pub status: String,
    pub items_synced: u64,
    pub last_sync_at: Option<String>,
    pub error: Option<String>,
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Read the KN API base URL and auth token from config.
/// Returns (base_url, bearer_token).
async fn kn_api_creds(state: &AppState) -> Result<(String, String), String> {
    let cfg = state.config.read().await;
    let base = cfg.k2k.kn_base_url
        .as_deref()
        .ok_or_else(|| "Knowledge Nexus API base URL not configured. Set kn_base_url in your K2K config.".to_string())?
        .trim_end_matches('/')
        .to_string();
    let token = cfg.k2k.kn_auth_token.clone().ok_or_else(|| {
        "Not authenticated with Knowledge Nexus. Please sign in first.".to_string()
    })?;
    Ok((base, token))
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Return all connectors supported by the wizard.
#[tauri::command]
pub async fn get_supported_connectors(
    state: State<'_, AppState>,
) -> Result<Vec<ConnectorMeta>, String> {
    let (base, token) = kn_api_creds(&state).await?;
    let url = format!("{base}/connectors/supported");
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("API error {status}: {body}"));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("Parse error: {e}"))?;
    let connectors: Vec<ConnectorMeta> = serde_json::from_value(
        body.get("connectors").cloned().unwrap_or(serde_json::Value::Array(vec![])),
    )
    .map_err(|e| format!("Deserialize error: {e}"))?;
    Ok(connectors)
}

/// Begin OAuth flow for `connector_type`.
/// Returns the authorization URL — the caller opens it in the system browser.
#[tauri::command]
pub async fn start_connector_oauth(
    connector_type: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let (base, token) = kn_api_creds(&state).await?;
    let url = format!("{base}/connectors/oauth/start");
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .post(&url)
        .bearer_auth(&token)
        .query(&[("connector_type", &connector_type)])
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("API error {status}: {body}"));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("Parse error: {e}"))?;
    let auth_url = body
        .get("auth_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing auth_url in response".to_string())?
        .to_string();
    info!(
        "[CONNECTOR] Starting OAuth for '{}', opening browser",
        connector_type
    );
    // Open the URL in the system browser — the OAuth server will redirect back
    // to nexibot:// deep-link when complete.
    if let Err(e) = open::that(&auth_url) {
        warn!("[CONNECTOR] Failed to open browser: {e}");
    }
    Ok(auth_url)
}

/// Poll sync status for a connector.
#[tauri::command]
pub async fn poll_connector_status(
    connector_id: String,
    state: State<'_, AppState>,
) -> Result<ConnectorSyncStatus, String> {
    let (base, token) = kn_api_creds(&state).await?;
    let safe_id = encode_path_segment(&connector_id);
    let url = format!("{base}/connectors/{safe_id}/sync-status");
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("API error {status}: {body}"));
    }
    resp.json::<ConnectorSyncStatus>()
        .await
        .map_err(|e| format!("Deserialize error: {e}"))
}

/// List the user's configured connectors.
#[tauri::command]
pub async fn list_user_connectors(
    state: State<'_, AppState>,
) -> Result<Vec<UserConnector>, String> {
    let (base, token) = kn_api_creds(&state).await?;
    let url = format!("{base}/connectors/");
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("API error {status}: {body}"));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("Parse error: {e}"))?;
    let connectors: Vec<UserConnector> = serde_json::from_value(
        body.get("connectors").cloned().unwrap_or(serde_json::Value::Array(vec![])),
    )
    .map_err(|e| format!("Deserialize error: {e}"))?;
    Ok(connectors)
}

/// Remove a connector.
#[tauri::command]
pub async fn delete_connector(
    connector_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (base, token) = kn_api_creds(&state).await?;
    let safe_id = encode_path_segment(&connector_id);
    let url = format!("{base}/connectors/{safe_id}");
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .delete(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("API error {status}: {body}"));
    }
    info!("[CONNECTOR] Deleted connector {connector_id}");
    Ok(())
}
