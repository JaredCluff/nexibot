///! Browser-based OAuth Flow
///!
///! Implements seamless OAuth authentication similar to Claude Code:
///! 1. Starts local HTTP server for callback
///! 2. Opens browser to OAuth provider
///! 3. Receives authorization code
///! 4. Exchanges for tokens
///! 5. Saves OAuth profile automatically
use anyhow::{Context, Result};
use axum::{extract::Query, response::Html, routing::get, Router};
use base64::Engine;
use rand::{Rng, rngs::OsRng};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::{info, warn};

/// OAuth callback parameters
#[derive(Deserialize)]
struct OAuthCallback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Manual Debug implementation that redacts the authorization code so it is
/// never written to log output.
impl std::fmt::Debug for OAuthCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthCallback")
            .field("code", &self.code.as_ref().map(|_| "[REDACTED]"))
            .field("state", &self.state)
            .field("error", &self.error)
            .field("error_description", &self.error_description)
            .finish()
    }
}

/// OAuth token response
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
    #[allow(dead_code)]
    token_type: String,
}

/// OAuth flow result
#[derive(Clone, Serialize)]
pub struct OAuthResult {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
}

/// Manual Debug implementation that redacts token fields.
impl std::fmt::Debug for OAuthResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthResult")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// OAuth flow context passed through to token exchange
struct OAuthContext {
    #[allow(dead_code)]
    callback_url: String,
    code_verifier: String,
    expected_state: String,
}

/// Start OAuth flow for a provider
pub async fn start_oauth_flow(provider: &str) -> Result<OAuthResult> {
    info!("[OAUTH] Starting OAuth flow for {}", provider);

    // Generate random state for CSRF protection
    let state = generate_state();

    // Generate PKCE code verifier and challenge
    let code_verifier = generate_code_verifier();
    let code_challenge = generate_code_challenge(&code_verifier);

    // Find available port for callback server
    let port = find_available_port()?;
    let callback_url = format!("http://localhost:{}/callback", port);

    info!("[OAUTH] Using callback URL: {}", callback_url);

    // Create OAuth context with all needed info
    let oauth_context = Arc::new(OAuthContext {
        callback_url: callback_url.clone(),
        code_verifier: code_verifier.clone(),
        expected_state: state.clone(),
    });

    // Create channel to receive OAuth result
    let (tx, rx) = oneshot::channel();

    // Start local HTTP server for OAuth callback
    let server_task = tokio::spawn(run_callback_server(
        port,
        oauth_context.clone(),
        provider.to_string(),
        tx,
    ));

    // Build OAuth URL based on provider
    let oauth_url = match provider {
        "anthropic" => build_anthropic_oauth_url(&callback_url, &state, &code_challenge)?,
        "google" => build_google_oauth_url(&callback_url, &state, &code_challenge)?,
        "openai" => build_openai_oauth_url(&callback_url, &state)?,
        _ => anyhow::bail!("Unsupported OAuth provider: {}", provider),
    };

    // Open browser to OAuth URL
    info!("[OAUTH] Opening browser to: {}", oauth_url);
    open_browser(&oauth_url)?;

    // Wait for OAuth callback (with timeout)
    let result = tokio::select! {
        result = rx => {
            match result {
                Ok(result) => result?,
                Err(_) => anyhow::bail!("OAuth server closed unexpectedly"),
            }
        }
        _ = tokio::time::sleep(tokio::time::Duration::from_secs(300)) => {
            anyhow::bail!("OAuth flow timed out after 5 minutes")
        }
    };

    // Shutdown server
    server_task.abort();

    info!("[OAUTH] OAuth flow completed successfully");
    Ok(result)
}

/// Generate random state parameter using the OS RNG for cryptographic quality.
fn generate_state() -> String {
    let mut rng = OsRng;
    let random_bytes: Vec<u8> = (0..32).map(|_| rng.gen::<u8>()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&random_bytes)
}

/// Find an available port for the callback server
fn find_available_port() -> Result<u16> {
    // Try a few random ports in the ephemeral range
    let mut rng = rand::thread_rng();
    for _ in 0..10 {
        let port = rng.gen_range(49152..65535);
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    anyhow::bail!("Could not find available port for OAuth callback")
}

/// Run local HTTP server to receive OAuth callback
async fn run_callback_server(
    port: u16,
    oauth_context: Arc<OAuthContext>,
    provider: String,
    result_tx: oneshot::Sender<Result<OAuthResult>>,
) {
    let result_tx = Arc::new(tokio::sync::Mutex::new(Some(result_tx)));

    let app = Router::new().route(
        "/callback",
        get({
            let oauth_context = oauth_context.clone();
            let provider = provider.clone();
            let result_tx = result_tx.clone();
            move |query: Query<OAuthCallback>| {
                handle_oauth_callback(
                    query,
                    oauth_context.clone(),
                    provider.clone(),
                    result_tx.clone(),
                )
            }
        }),
    );

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            warn!("[OAUTH] Failed to bind callback server on {}: {}", addr, e);
            return;
        }
    };

    info!("[OAUTH] Callback server listening on {}", addr);

    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| warn!("[OAUTH] Server error: {}", e));
}

/// Handle OAuth callback request
async fn handle_oauth_callback(
    Query(params): Query<OAuthCallback>,
    oauth_context: Arc<OAuthContext>,
    provider: String,
    result_tx: Arc<tokio::sync::Mutex<Option<oneshot::Sender<Result<OAuthResult>>>>>,
) -> Html<String> {
    info!("[OAUTH] Received callback with params: {:?}", params);

    // Check for OAuth error
    if let Some(error) = params.error {
        let error_desc = params
            .error_description
            .unwrap_or_else(|| "Unknown error".to_string());
        let error_msg = format!("OAuth error: {} - {}", error, error_desc);
        warn!("[OAUTH] {}", error_msg);

        if let Some(tx) = result_tx.lock().await.take() {
            let _ = tx.send(Err(anyhow::anyhow!(error_msg)));
        }

        return Html(format!(
            r#"
            <!DOCTYPE html>
            <html>
            <head><title>Authentication Failed</title></head>
            <body style="font-family: sans-serif; text-align: center; padding: 50px;">
                <h1 style="color: #e53e3e;">❌ Authentication Failed</h1>
                <p>{}</p>
                <p>You can close this window and try again.</p>
            </body>
            </html>
            "#,
            error_desc
        ));
    }

    // Verify state parameter
    if params.state.as_ref() != Some(&oauth_context.expected_state) {
        warn!("[OAUTH] State mismatch - possible CSRF attack");
        if let Some(tx) = result_tx.lock().await.take() {
            let _ = tx.send(Err(anyhow::anyhow!("Invalid state parameter")));
        }

        return Html(
            r#"
            <!DOCTYPE html>
            <html>
            <head><title>Authentication Failed</title></head>
            <body style="font-family: sans-serif; text-align: center; padding: 50px;">
                <h1 style="color: #e53e3e;">❌ Security Error</h1>
                <p>Invalid state parameter. Please try again.</p>
            </body>
            </html>
            "#
            .to_string(),
        );
    }

    // Get authorization code (comes in format "code#state" from Anthropic)
    let code_with_state = match params.code {
        Some(code) => code,
        None => {
            warn!("[OAUTH] No authorization code received");
            if let Some(tx) = result_tx.lock().await.take() {
                let _ = tx.send(Err(anyhow::anyhow!("No authorization code received")));
            }

            return Html(
                r#"
                <!DOCTYPE html>
                <html>
                <head><title>Authentication Failed</title></head>
                <body style="font-family: sans-serif; text-align: center; padding: 50px;">
                    <h1 style="color: #e53e3e;">❌ Authentication Failed</h1>
                    <p>No authorization code received.</p>
                </body>
                </html>
                "#
                .to_string(),
            );
        }
    };

    // Parse code and state (Anthropic returns "code#state" as the code parameter).
    // When both a `state` query parameter AND an embedded state in the code are
    // present, the embedded one is used for the token-exchange call; however we
    // must still verify it matches the expected state to prevent CSRF bypass.
    let (code, state_from_code) = if code_with_state.contains('#') {
        let parts: Vec<&str> = code_with_state.split('#').collect();
        let embedded_state = parts
            .get(1)
            .map(|s| s.to_string())
            .unwrap_or_default();

        // Validate the embedded state — don't silently fall back to the expected
        // value if it is absent or mismatched.
        if !embedded_state.is_empty() && embedded_state != oauth_context.expected_state {
            warn!("[OAUTH] Embedded state in code does not match expected state — possible CSRF");
            if let Some(tx) = result_tx.lock().await.take() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "State mismatch in authorization code parameter"
                )));
            }
            return Html(
                r#"
                <!DOCTYPE html>
                <html>
                <head><title>Authentication Failed</title></head>
                <body style="font-family: sans-serif; text-align: center; padding: 50px;">
                    <h1 style="color: #e53e3e;">&#x274C; Security Error</h1>
                    <p>State parameter mismatch. Please try again.</p>
                </body>
                </html>
                "#
                .to_string(),
            );
        }

        (parts[0].to_string(), oauth_context.expected_state.clone())
    } else {
        (code_with_state, oauth_context.expected_state.clone())
    };

    // Do NOT log the authorization code value itself.
    info!("[OAUTH] Authorization code received, exchanging for tokens...");

    // Exchange code for tokens
    let token_result = match provider.as_str() {
        "anthropic" => {
            exchange_anthropic_code(&code, &oauth_context.code_verifier, &state_from_code).await
        }
        "google" => exchange_google_code(&code, &oauth_context.code_verifier, &oauth_context.callback_url).await,
        "openai" => exchange_openai_code(&code).await,
        _ => Err(anyhow::anyhow!(
            "Token exchange not implemented for provider: {}",
            provider
        )),
    };

    match token_result {
        Ok(result) => {
            info!("[OAUTH] Token exchange successful");
            if let Some(tx) = result_tx.lock().await.take() {
                let _ = tx.send(Ok(result));
            }

            Html(
                r#"
                <!DOCTYPE html>
                <html>
                <head>
                    <title>Authentication Successful</title>
                    <script>
                        // Auto-close window after 3 seconds
                        setTimeout(() => {
                            window.close();
                        }, 3000);
                    </script>
                </head>
                <body style="font-family: sans-serif; text-align: center; padding: 50px;">
                    <h1 style="color: #10b981;">✅ Authentication Successful!</h1>
                    <p>You can now return to NexiBot.</p>
                    <p><small>This window will close automatically...</small></p>
                </body>
                </html>
                "#
                .to_string(),
            )
        }
        Err(e) => {
            warn!("[OAUTH] Token exchange failed: {}", e);
            if let Some(tx) = result_tx.lock().await.take() {
                let _ = tx.send(Err(e));
            }

            Html(
                r#"
                <!DOCTYPE html>
                <html>
                <head><title>Authentication Failed</title></head>
                <body style="font-family: sans-serif; text-align: center; padding: 50px;">
                    <h1 style="color: #e53e3e;">❌ Token Exchange Failed</h1>
                    <p>Could not complete authentication. Please try again.</p>
                </body>
                </html>
                "#
                .to_string(),
            )
        }
    }
}

/// Build Anthropic OAuth URL with PKCE
fn build_anthropic_oauth_url(
    _callback_url: &str,
    state: &str,
    code_challenge: &str,
) -> Result<String> {
    // Use Claude CLI's client ID for subscription access
    let client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

    // Use console.anthropic.com as redirect_uri (required by Anthropic's OAuth)
    let redirect_uri = "https://console.anthropic.com/oauth/code/callback";

    let params = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", "true")
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", "org:create_api_key user:profile user:inference")
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .finish();

    Ok(format!("https://claude.ai/oauth/authorize?{}", params))
}

/// Exchange Anthropic authorization code for tokens
async fn exchange_anthropic_code(
    code: &str,
    code_verifier: &str,
    state: &str,
) -> Result<OAuthResult> {
    info!("[OAUTH] Exchanging authorization code for tokens");

    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());

    // Use Claude CLI's client ID
    let client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

    // Must match the redirect_uri used in authorization
    let redirect_uri = "https://console.anthropic.com/oauth/code/callback";

    // CRITICAL: Use /v1/oauth/token endpoint (not /api/oauth/token)
    // and send JSON body (not form-urlencoded)
    info!("[OAUTH] Attempting token exchange with console.anthropic.com/v1/oauth/token");
    let response = client
        .post("https://console.anthropic.com/v1/oauth/token")
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": client_id,
            "code": code,
            "state": state,  // Send state separately (not in code)
            "redirect_uri": redirect_uri,
            "code_verifier": code_verifier,
        }))
        .send()
        .await
        .context("Failed to send token exchange request")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Could not read error".to_string());
        warn!(
            "[OAUTH] Token exchange failed with status {}: {}",
            status, error_text
        );

        // Return detailed error to user
        anyhow::bail!("Token exchange failed (HTTP {}): {}", status, error_text);
    }

    let token_response: TokenResponse = response
        .json()
        .await
        .context("Failed to parse token response")?;

    info!("[OAUTH] Successfully exchanged code for access token");

    Ok(OAuthResult {
        access_token: token_response.access_token,
        refresh_token: token_response.refresh_token,
        expires_in: token_response.expires_in,
    })
}

/// Generate PKCE code verifier using the OS CSPRNG.
pub fn generate_code_verifier() -> String {
    let mut rng = OsRng;
    let random_bytes: Vec<u8> = (0..32).map(|_| rng.gen::<u8>()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&random_bytes)
}

/// Generate PKCE code challenge from verifier
pub fn generate_code_challenge(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
}

/// Build OpenAI OAuth URL — not used; OpenAI auth uses device code flow instead.
/// See `commands::oauth::start_openai_device_flow`.
fn build_openai_oauth_url(_callback_url: &str, _state: &str) -> Result<String> {
    anyhow::bail!(
        "OpenAI does not support browser-redirect OAuth for third-party apps. \
         Use the device code flow instead (start_openai_device_flow command)."
    )
}

/// Build Google OAuth authorization URL
fn build_google_oauth_url(callback_url: &str, state: &str, code_challenge: &str) -> Result<String> {
    let params = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", "google-client-id-placeholder")
        .append_pair("redirect_uri", callback_url)
        .append_pair("response_type", "code")
        .append_pair("scope", "openid profile email")
        .append_pair("state", state)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();

    Ok(format!(
        "https://accounts.google.com/o/oauth2/v2/auth?{}",
        params
    ))
}

/// Exchange Google authorization code for tokens
async fn exchange_google_code(code: &str, code_verifier: &str, callback_url: &str) -> Result<OAuthResult> {
    info!("[OAUTH] Exchanging Google authorization code for tokens");

    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());

    let response = client
        .post("https://oauth2.googleapis.com/token")
        .json(&serde_json::json!({
            "code": code,
            "client_id": "google-client-id-placeholder",
            "client_secret": "google-client-secret-placeholder",
            "redirect_uri": callback_url,
            "grant_type": "authorization_code",
            "code_verifier": code_verifier,
        }))
        .send()
        .await
        .context("Failed to exchange Google code")?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        anyhow::bail!("Google token exchange failed: {}", error_text);
    }

    let token_response: TokenResponse = response
        .json()
        .await
        .context("Failed to parse Google token response")?;

    info!("[OAUTH] Google token exchange successful");

    Ok(OAuthResult {
        access_token: token_response.access_token,
        refresh_token: token_response.refresh_token,
        expires_in: token_response.expires_in,
    })
}

/// Exchange OpenAI authorization code for tokens — not used; OpenAI auth uses
/// device code flow instead. See `commands::oauth::poll_openai_device_flow`.
async fn exchange_openai_code(_code: &str) -> Result<OAuthResult> {
    anyhow::bail!(
        "OpenAI does not support authorization-code exchange for third-party apps. \
         Use the device code flow instead (poll_openai_device_flow command)."
    )
}

/// Open browser to URL
fn open_browser(url: &str) -> Result<()> {
    crate::platform::open_browser(url)
}
