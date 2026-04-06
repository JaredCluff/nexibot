//! OAuth authentication flow commands

use serde::{Deserialize, Serialize};
use std::time::Instant;
use tauri::State;
use tracing::{debug, info, warn};

use crate::oauth::{AuthProfile, AuthProfileManager};
use crate::oauth_flow;
use crate::oauth_manager::OAuthProviderStatus;

use super::{AppState, OAuthPendingState};

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthStatus {
    pub provider: String,
    pub profile_name: String,
    pub is_expiring: bool,
    pub expires_at: u64,
    pub has_refresh_token: bool,
}

/// Add or update an OAuth profile
#[tauri::command]
pub async fn add_oauth_profile(
    provider: String,
    profile_name: String,
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
) -> Result<(), String> {
    info!("Adding OAuth profile for provider: {}", provider);

    let mut manager = AuthProfileManager::load().map_err(|e| e.to_string())?;

    let profile = AuthProfile::new(
        provider,
        profile_name,
        access_token,
        refresh_token,
        expires_in,
    );

    manager.upsert_profile(profile);
    manager.save().map_err(|e| e.to_string())?;

    Ok(())
}

/// List OAuth profiles for a provider
#[tauri::command]
pub async fn list_oauth_profiles(provider: String) -> Result<Vec<AuthProfile>, String> {
    let manager = AuthProfileManager::load().map_err(|e| e.to_string())?;
    let profiles = manager.list_profiles(&provider);
    Ok(profiles.into_iter().cloned().collect())
}

/// Remove an OAuth profile
#[tauri::command]
pub async fn remove_oauth_profile(provider: String, profile_name: String) -> Result<(), String> {
    info!("Removing OAuth profile: {} - {}", provider, profile_name);

    let mut manager = AuthProfileManager::load().map_err(|e| e.to_string())?;
    manager.remove_profile(&provider, &profile_name);
    manager.save().map_err(|e| e.to_string())?;

    Ok(())
}

/// Start browser-based OAuth flow
#[tauri::command]
pub async fn start_oauth_flow(provider: String) -> Result<(), String> {
    info!("Starting OAuth flow for provider: {}", provider);

    // Start OAuth flow (opens browser, waits for callback)
    let result = oauth_flow::start_oauth_flow(&provider)
        .await
        .map_err(|e| e.to_string())?;

    // Save OAuth profile
    let mut manager = AuthProfileManager::load().map_err(|e| e.to_string())?;

    let profile = AuthProfile::new(
        provider.clone(),
        "default",
        result.access_token,
        result.refresh_token,
        result.expires_in,
    );

    manager.upsert_profile(profile);
    manager.save().map_err(|e| e.to_string())?;

    info!("OAuth flow completed and profile saved");
    Ok(())
}

/// Open browser for OAuth authorization (device code flow)
/// Returns the auth URL so the UI can show it as a fallback
#[tauri::command]
pub async fn open_oauth_browser(
    provider: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    info!("[OAUTH] Opening browser for OAuth authorization: {}", provider);

    if provider != "anthropic" {
        return Err("Only Anthropic OAuth is currently supported".to_string());
    }

    // Generate PKCE and build auth URL
    use crate::oauth_flow::{generate_code_challenge, generate_code_verifier};
    let code_verifier = generate_code_verifier();
    let code_challenge = generate_code_challenge(&code_verifier);
    let oauth_state = generate_code_verifier(); // Use as state too

    let client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
    let redirect_uri = "https://console.anthropic.com/oauth/code/callback";
    let scopes = "org:create_api_key user:profile user:inference";

    let params = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", "true")
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", scopes)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &oauth_state)
        .finish();

    let auth_url = format!("https://claude.ai/oauth/authorize?{}", params);

    // Store code_verifier in AppState (not env vars)
    {
        let mut oauth = state.oauth_state.write().await;
        *oauth = Some(OAuthPendingState {
            code_verifier,
            state: oauth_state,
            created_at: Instant::now(),
        });
    }

    // Open browser (cross-platform)
    info!("[OAUTH] Opening browser to: {}", auth_url);
    if let Err(e) = crate::platform::open_browser(&auth_url) {
        warn!("[OAUTH] Failed to open browser: {}", e);
        // Still return the URL so the UI can show it as a fallback link
    }

    info!("[OAUTH] OAuth flow initiated");
    Ok(auth_url)
}

/// Complete OAuth flow with authorization code from user
#[tauri::command]
pub async fn complete_oauth_flow(
    provider: String,
    code: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!("Completing OAuth flow with authorization code");

    if provider != "anthropic" {
        return Err("Only Anthropic OAuth is currently supported".to_string());
    }

    // Retrieve stored PKCE values from AppState
    let (code_verifier, oauth_state_str) = {
        let oauth = state.oauth_state.read().await;
        match oauth.as_ref() {
            Some(pending) => {
                // Check TTL: reject if older than 5 minutes
                if pending.created_at.elapsed() > std::time::Duration::from_secs(300) {
                    drop(oauth);
                    let mut oauth_w = state.oauth_state.write().await;
                    *oauth_w = None;
                    return Err("OAuth session expired (>5 minutes). Please try again.".to_string());
                }
                (pending.code_verifier.clone(), pending.state.clone())
            }
            None => return Err("OAuth session expired. Please try again.".to_string()),
        }
    };

    // Parse code (format: "code#state")
    let (auth_code, code_state) = if code.contains('#') {
        let parts: Vec<&str> = code.split('#').collect();
        (
            parts[0].to_string(),
            parts
                .get(1)
                .map(|s| s.to_string())
                .unwrap_or(oauth_state_str.clone()),
        )
    } else {
        (code, oauth_state_str.clone())
    };

    // Validate state to prevent CSRF / token substitution attacks.
    // The state returned in the callback must exactly match what we sent.
    if code_state != oauth_state_str {
        return Err("OAuth state mismatch — request may have been tampered with".to_string());
    }

    // Exchange code for tokens
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
    let redirect_uri = "https://console.anthropic.com/oauth/code/callback";

    let response = client
        .post("https://console.anthropic.com/v1/oauth/token")
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": client_id,
            "code": auth_code,
            "state": code_state,
            "redirect_uri": redirect_uri,
            "code_verifier": code_verifier,
        }))
        .send()
        .await
        .map_err(|e| format!("Token exchange request failed: {}", e))?;

    if !response.status().is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("Token exchange failed: {}", error_text));
    }

    let token_data: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))?;

    let access_token = token_data["access_token"]
        .as_str()
        .ok_or("Missing access_token in response")?
        .to_string();
    let refresh_token = token_data["refresh_token"].as_str().map(|s| s.to_string());
    let expires_in = token_data["expires_in"].as_u64().unwrap_or(28800);

    debug!(
        "[OAUTH] Received tokens - access_token prefix: {}...",
        &access_token[..8.min(access_token.len())]
    );
    debug!("[OAUTH] Token expires in: {} seconds", expires_in);
    debug!("[OAUTH] Has refresh token: {}", refresh_token.is_some());

    // Save OAuth profile
    let mut manager = AuthProfileManager::load().map_err(|e| {
        let err_msg = format!("Failed to load AuthProfileManager: {}", e);
        warn!("[OAUTH] {}", err_msg);
        err_msg
    })?;

    let profile = AuthProfile::new(
        provider.clone(),
        "default".to_string(),
        access_token.clone(),
        refresh_token.clone(),
        expires_in,
    );

    info!("[OAUTH] Created profile for provider: {}", provider);
    manager.upsert_profile(profile);

    let save_result = manager.save();
    if let Err(e) = &save_result {
        warn!("[OAUTH] Failed to save profile: {}", e);
    } else {
        info!("[OAUTH] Successfully saved OAuth profile");
    }
    save_result.map_err(|e| e.to_string())?;

    // Clean up OAuth pending state
    {
        let mut oauth = state.oauth_state.write().await;
        *oauth = None;
    }

    info!("OAuth flow completed successfully");
    Ok(())
}

/// Start Claude CLI authentication (automatic browser-based OAuth)
#[tauri::command]
pub async fn start_claude_cli_auth(provider: String) -> Result<(), String> {
    info!(
        "Starting Claude CLI authentication for provider: {}",
        provider
    );

    if provider != "anthropic" {
        return Err("Claude CLI auth only supports Anthropic".to_string());
    }

    // Check if claude CLI is installed
    let check = std::process::Command::new("claude")
        .arg("--version")
        .output();

    if check.is_err() {
        return Err(
            "Claude CLI not installed. Install with: npm install -g @anthropic-ai/claude"
                .to_string(),
        );
    }

    // Run claude auth (opens browser automatically)
    info!("Running 'claude auth' - this will open your browser");
    let auth_result = std::process::Command::new("claude")
        .arg("auth")
        .status()
        .map_err(|e| format!("Failed to run claude auth: {}", e))?;

    if !auth_result.success() {
        return Err("Claude authentication failed or was cancelled".to_string());
    }

    // Extract tokens from keychain (macOS)
    #[cfg(target_os = "macos")]
    {
        let tokens = extract_claude_tokens_from_keychain()
            .map_err(|e| format!("Failed to extract tokens: {}", e))?;

        // Save OAuth profile
        let mut manager = AuthProfileManager::load().map_err(|e| e.to_string())?;

        let profile = AuthProfile::new(
            "anthropic".to_string(),
            "default",
            tokens.access_token,
            tokens.refresh_token,
            tokens.expires_in,
        );

        manager.upsert_profile(profile);
        manager.save().map_err(|e| e.to_string())?;

        info!("Claude CLI authentication completed and profile saved");
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let tokens = extract_claude_tokens_from_credentials_file()
            .map_err(|e| format!("Failed to extract tokens: {}", e))?;

        // Save OAuth profile
        let mut manager = AuthProfileManager::load().map_err(|e| e.to_string())?;

        let profile = AuthProfile::new(
            "anthropic".to_string(),
            "default",
            tokens.access_token,
            tokens.refresh_token,
            tokens.expires_in,
        );

        manager.upsert_profile(profile);
        manager.save().map_err(|e| e.to_string())?;

        info!("Claude CLI authentication completed and profile saved (credentials file)");
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn extract_claude_tokens_from_keychain() -> anyhow::Result<oauth_flow::OAuthResult> {
    use std::process::Command;

    // Try multiple keychain service names to support both Claude Code and npm @anthropic-ai/claude
    let service_names = vec![
        "Claude Code-credentials", // Claude Code (official Anthropic CLI)
        "claude-cli",              // npm @anthropic-ai/claude package
        "anthropic-claude",        // Alternative name
    ];

    for service_name in service_names {
        let output = Command::new("security")
            .args(["find-generic-password", "-s", service_name, "-w"])
            .output()?;

        if output.status.success() {
            let token_data = String::from_utf8(output.stdout)?;
            let token_data = token_data.trim();

            // Try to parse as JSON
            if let Ok(tokens) = serde_json::from_str::<serde_json::Value>(token_data) {
                // Claude Code nests tokens under claudeAiOauth
                let token_obj = tokens.get("claudeAiOauth").unwrap_or(&tokens);

                // Extract tokens - try different field names used by different CLIs
                let session_key = token_obj
                    .get("sessionKey")
                    .or_else(|| token_obj.get("access_token"))
                    .or_else(|| token_obj.get("accessToken"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing token in keychain data from {}", service_name)
                    })?
                    .to_string();

                // Calculate expires_in from expiresAt timestamp if available
                let expires_in =
                    if let Some(expires_at) = token_obj.get("expiresAt").and_then(|v| v.as_u64()) {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        let expires_in_ms = expires_at.saturating_sub(now);
                        (expires_in_ms / 1000).max(300) // At least 5 minutes
                    } else {
                        token_obj
                            .get("expiresIn")
                            .or_else(|| token_obj.get("expires_in"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(28800)
                    };

                return Ok(oauth_flow::OAuthResult {
                    access_token: session_key,
                    refresh_token: token_obj
                        .get("refreshToken")
                        .or_else(|| token_obj.get("refresh_token"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    expires_in,
                });
            }
        }
    }

    anyhow::bail!(
        "Failed to retrieve Claude tokens from keychain.\n\
         \n\
         Please run 'claude auth' first to authenticate.\n\
         \n\
         Supported CLIs:\n\
         - Claude Code (official): already installed\n\
         - npm package: npm install -g @anthropic-ai/claude"
    )
}

/// Extract Claude tokens from credentials file (Windows/Linux)
///
/// Claude Code stores credentials at `~/.claude/.credentials.json` on Windows and Linux.
/// The file contains OAuth tokens nested under a `claudeAiOauth` key.
#[cfg(not(target_os = "macos"))]
fn extract_claude_tokens_from_credentials_file() -> anyhow::Result<oauth_flow::OAuthResult> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    // Claude Code stores credentials at ~/.claude/.credentials.json
    let credentials_path = home.join(".claude").join(".credentials.json");

    if !credentials_path.exists() {
        anyhow::bail!(
            "Claude credentials file not found at {:?}.\n\
             \n\
             Please run 'claude auth' first to authenticate.\n\
             \n\
             Supported CLIs:\n\
             - Claude Code (official): npm install -g @anthropic-ai/claude-code",
            credentials_path
        );
    }

    let contents = std::fs::read_to_string(&credentials_path)
        .map_err(|e| anyhow::anyhow!("Failed to read credentials file: {}", e))?;

    let tokens: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|e| anyhow::anyhow!("Failed to parse credentials JSON: {}", e))?;

    // Claude Code nests tokens under claudeAiOauth
    let token_obj = tokens.get("claudeAiOauth").unwrap_or(&tokens);

    // Extract access token - try different field names
    let session_key = token_obj.get("sessionKey")
        .or_else(|| token_obj.get("access_token"))
        .or_else(|| token_obj.get("accessToken"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!(
            "Missing token in credentials file. The file exists but contains no recognized token fields.\n\
             Please re-run 'claude auth' to refresh your authentication."
        ))?
        .to_string();

    // Calculate expires_in from expiresAt timestamp if available
    let expires_in = if let Some(expires_at) = token_obj.get("expiresAt").and_then(|v| v.as_u64()) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let expires_in_ms = expires_at.saturating_sub(now);
        (expires_in_ms / 1000).max(300) // At least 5 minutes
    } else {
        token_obj
            .get("expiresIn")
            .or_else(|| token_obj.get("expires_in"))
            .and_then(|v| v.as_u64())
            .unwrap_or(28800)
    };

    Ok(oauth_flow::OAuthResult {
        access_token: session_key,
        refresh_token: token_obj
            .get("refreshToken")
            .or_else(|| token_obj.get("refresh_token"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        expires_in,
    })
}

/// Get OAuth authentication status
#[tauri::command]
pub async fn get_oauth_status(provider: String) -> Result<Option<OAuthStatus>, String> {
    let mut manager = AuthProfileManager::load().map_err(|e| e.to_string())?;

    if let Some(profile) = manager.get_default_profile(&provider) {
        Ok(Some(OAuthStatus {
            provider: profile.provider.clone(),
            profile_name: profile.profile_name.clone(),
            is_expiring: profile.is_expiring(),
            expires_at: profile.expires_at,
            has_refresh_token: profile.refresh_token.is_some(),
        }))
    } else {
        Ok(None)
    }
}

// --- Enhanced OAuth Management Commands (via OAuthManager) ---

#[derive(Debug, Serialize)]
pub struct ActiveOAuthProfile {
    pub provider: String,
    pub profile_name: String,
    pub expires_at: u64,
    pub is_expiring: bool,
}

/// Get the current active OAuth profile and provider
#[tauri::command]
pub async fn get_active_oauth_profile(
    state: State<'_, AppState>,
) -> Result<Option<ActiveOAuthProfile>, String> {
    let oauth_mgr = state.oauth_manager.read().await;

    match oauth_mgr.get_active_profile().await {
        Ok(Some(profile)) => {
            let is_expiring = profile.is_expiring();
            Ok(Some(ActiveOAuthProfile {
                provider: profile.provider,
                profile_name: profile.profile_name,
                expires_at: profile.expires_at,
                is_expiring,
            }))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Refresh the active OAuth token
#[tauri::command]
pub async fn refresh_active_oauth_token(state: State<'_, AppState>) -> Result<(), String> {
    let oauth_mgr = state.oauth_manager.read().await;
    let (provider, profile_name) = oauth_mgr.get_active().await;

    oauth_mgr
        .refresh_token(&provider, &profile_name)
        .await
        .map_err(|e| e.to_string())
}

/// Set the active OAuth provider and profile
#[tauri::command]
pub async fn set_active_oauth_profile(
    provider: String,
    profile_name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let oauth_mgr = state.oauth_manager.read().await;

    oauth_mgr
        .set_active(provider, profile_name)
        .await
        .map_err(|e| e.to_string())
}

/// Get provider status (available profiles, refresh needed, etc.)
#[tauri::command]
pub async fn get_oauth_provider_status(
    provider: String,
    state: State<'_, AppState>,
) -> Result<OAuthProviderStatus, String> {
    let oauth_mgr = state.oauth_manager.read().await;

    oauth_mgr
        .get_provider_status(&provider)
        .await
        .map_err(|e| e.to_string())
}

/// Check if a provider has any OAuth profiles configured
#[tauri::command]
pub async fn oauth_provider_has_profiles(
    provider: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let oauth_mgr = state.oauth_manager.read().await;

    Ok(oauth_mgr.has_profile(&provider).await)
}

/// Get valid access token (refreshes if needed)
#[tauri::command]
pub async fn get_oauth_access_token(state: State<'_, AppState>) -> Result<String, String> {
    let oauth_mgr = state.oauth_manager.read().await;

    oauth_mgr.get_valid_token().await.map_err(|e| e.to_string())
}

// --- OpenAI Device Code Flow (Codex-style) ---
// Uses OpenAI's auth system at auth.openai.com (same as Codex CLI).
// Three-step flow:
//   1. POST /api/accounts/deviceauth/usercode → device_auth_id + user_code
//   2. Poll POST /api/accounts/deviceauth/token → authorization_code + code_verifier
//   3. Exchange POST /oauth/token → access_token + refresh_token
// NOTE: The client_id is a public client (no secret). OpenAI could restrict
// third-party use in the future. API key entry always works as fallback.

/// OpenAI Codex public client ID (same as official Codex CLI — public client, no secret)
const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_AUTH_BASE: &str = "https://auth.openai.com";

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIDeviceFlowResponse {
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIDeviceFlowPollResult {
    pub status: String, // "pending" | "complete" | "expired" | "denied"
    pub error: Option<String>,
}

/// Start OpenAI device code flow.
/// POSTs to auth.openai.com to get a user_code + device_auth_id.
#[tauri::command]
pub async fn start_openai_device_flow(
    state: State<'_, AppState>,
) -> Result<OpenAIDeviceFlowResponse, String> {
    info!("[OAUTH] Starting OpenAI device code flow (Codex-style)");

    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let response = client
        .post(format!("{}/api/accounts/deviceauth/usercode", OPENAI_AUTH_BASE))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "client_id": OPENAI_CODEX_CLIENT_ID,
        }))
        .send()
        .await
        .map_err(|e| format!("Failed to start device flow: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        if status.as_u16() == 404 {
            return Err("Device code login is not enabled. Please enable it in your ChatGPT security settings at https://chatgpt.com/settings/security".to_string());
        }
        return Err(format!("Device code request failed ({}): {}", status, error_text));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse device code response: {}", e))?;

    let device_auth_id = body["device_auth_id"]
        .as_str()
        .ok_or("Missing device_auth_id in response")?
        .to_string();
    // OpenAI returns either "user_code" or "usercode"
    let user_code = body["user_code"]
        .as_str()
        .or_else(|| body["usercode"].as_str())
        .ok_or("Missing user_code in response")?
        .to_string();
    // interval may be returned as string or number
    let interval = body["interval"]
        .as_u64()
        .or_else(|| body["interval"].as_str().and_then(|s| s.parse::<u64>().ok()))
        .unwrap_or(5);

    let verification_uri = format!("{}/codex/device", OPENAI_AUTH_BASE);

    // Store state for polling
    {
        let mut flow = state.openai_device_flow.write().await;
        *flow = Some(super::OpenAIDeviceFlowState {
            device_auth_id,
            user_code: user_code.clone(),
            interval,
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(15 * 60),
        });
    }

    info!("[OAUTH] Device code flow started, user_code: {}", user_code);

    Ok(OpenAIDeviceFlowResponse {
        user_code,
        verification_uri,
        interval,
    })
}

/// Poll OpenAI device code flow for completion.
/// Step 2: polls /api/accounts/deviceauth/token for authorization_code.
/// Step 3: exchanges authorization_code at /oauth/token for access/refresh tokens.
#[tauri::command]
pub async fn poll_openai_device_flow(
    state: State<'_, AppState>,
) -> Result<OpenAIDeviceFlowPollResult, String> {
    // Read stored flow state
    let (device_auth_id, user_code, expires_at) = {
        let flow = state.openai_device_flow.read().await;
        match flow.as_ref() {
            Some(f) => (f.device_auth_id.clone(), f.user_code.clone(), f.expires_at),
            None => {
                return Ok(OpenAIDeviceFlowPollResult {
                    status: "expired".to_string(),
                    error: Some("No active device flow. Please start a new one.".to_string()),
                });
            }
        }
    };

    // Check expiry (15 min)
    if std::time::Instant::now() > expires_at {
        let mut flow = state.openai_device_flow.write().await;
        *flow = None;
        return Ok(OpenAIDeviceFlowPollResult {
            status: "expired".to_string(),
            error: Some("Device code expired. Please start a new flow.".to_string()),
        });
    }

    // Step 2: Poll for authorization code
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let response = client
        .post(format!("{}/api/accounts/deviceauth/token", OPENAI_AUTH_BASE))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "device_auth_id": device_auth_id,
            "user_code": user_code,
        }))
        .send()
        .await
        .map_err(|e| format!("Failed to poll device flow: {}", e))?;

    let status_code = response.status();

    // 403 or 404 = authorization still pending
    if status_code.as_u16() == 403 || status_code.as_u16() == 404 {
        return Ok(OpenAIDeviceFlowPollResult {
            status: "pending".to_string(),
            error: None,
        });
    }

    if !status_code.is_success() {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        let mut flow = state.openai_device_flow.write().await;
        *flow = None;
        return Ok(OpenAIDeviceFlowPollResult {
            status: "denied".to_string(),
            error: Some(format!("Device auth failed ({}): {}", status_code, error_text)),
        });
    }

    // Success on step 2 — we got the authorization code + code_verifier
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse poll response: {}", e))?;

    let authorization_code = body["authorization_code"]
        .as_str()
        .ok_or("Missing authorization_code in response")?
        .to_string();
    let code_verifier = body["code_verifier"]
        .as_str()
        .ok_or("Missing code_verifier in response")?
        .to_string();

    info!("[OAUTH] Device auth approved, exchanging for tokens...");

    // Step 3: Exchange authorization_code for tokens
    let redirect_uri = format!("{}/deviceauth/callback", OPENAI_AUTH_BASE);
    let form_body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", &authorization_code)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("client_id", OPENAI_CODEX_CLIENT_ID)
        .append_pair("code_verifier", &code_verifier)
        .finish();
    let token_response = client
        .post(format!("{}/oauth/token", OPENAI_AUTH_BASE))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body)
        .send()
        .await
        .map_err(|e| format!("Token exchange failed: {}", e))?;

    if !token_response.status().is_success() {
        let error_text = token_response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        let mut flow = state.openai_device_flow.write().await;
        *flow = None;
        return Ok(OpenAIDeviceFlowPollResult {
            status: "denied".to_string(),
            error: Some(format!("Token exchange failed: {}", error_text)),
        });
    }

    let tokens: serde_json::Value = token_response
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))?;

    let access_token = tokens["access_token"]
        .as_str()
        .ok_or("Missing access_token in token response")?
        .to_string();
    let refresh_token = tokens["refresh_token"].as_str().map(|s| s.to_string());
    let expires_in = tokens["expires_in"].as_u64().unwrap_or(3600);

    info!("[OAUTH] OpenAI device code flow completed successfully");

    // Save as OAuth profile
    let mut manager = AuthProfileManager::load().map_err(|e| e.to_string())?;
    let profile = AuthProfile::new(
        "openai".to_string(),
        "default".to_string(),
        access_token,
        refresh_token,
        expires_in,
    );
    manager.upsert_profile(profile);
    manager.save().map_err(|e| e.to_string())?;

    // Clean up device flow state
    {
        let mut flow = state.openai_device_flow.write().await;
        *flow = None;
    }

    Ok(OpenAIDeviceFlowPollResult {
        status: "complete".to_string(),
        error: None,
    })
}
