//! Authentication for the WebSocket gateway.
//!
//! Supports three modes:
//! - **Token**: pre-shared bearer tokens compared in constant time.
//! - **Password**: Argon2id hash comparison with random salt.
//! - **Open**: development mode — every connection is accepted as "anonymous".

use anyhow::Result;
use argon2::password_hash::SaltString;
use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::IpAddr;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::AuthMode;
use crate::security::constant_time::secure_compare;
use crate::security::rate_limit::RateLimiter;

// ---------------------------------------------------------------------------
// Credentials & result types
// ---------------------------------------------------------------------------

/// Credentials presented by a connecting client.
#[derive(Debug, Clone)]
pub enum AuthCredentials {
    /// A bearer token.
    Token(String),
    /// A plaintext password (will be hashed for comparison).
    Password(String),
    /// No credentials (only valid in Open mode).
    None,
    /// Tailscale identity extracted from HTTP upgrade headers by a trusted reverse proxy.
    TailscaleHeaders {
        /// Value of the `Tailscale-User-Login` header (email-format login).
        login: String,
        /// Value of the `Tailscale-User-Name` header (display name).
        #[allow(dead_code)]
        name: String,
    },
}

/// The outcome of an authentication attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResult {
    /// Whether the attempt succeeded.
    pub authenticated: bool,
    /// Opaque user identifier assigned to this connection.
    pub user_id: String,
    /// Granted permissions (currently informational).
    pub permissions: Vec<String>,
}

impl AuthResult {
    /// Convenience constructor for a successful authentication.
    fn success(user_id: String, permissions: Vec<String>) -> Self {
        Self {
            authenticated: true,
            user_id,
            permissions,
        }
    }

    /// Convenience constructor for a failed authentication.
    fn failure() -> Self {
        Self {
            authenticated: false,
            user_id: String::new(),
            permissions: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// GatewayAuth
// ---------------------------------------------------------------------------

/// Authentication manager for the gateway.
pub struct GatewayAuth {
    /// Active authentication mode.
    mode: AuthMode,
    /// Set of valid bearer tokens (only used in Token mode).
    tokens: HashSet<String>,
    /// Argon2id password hash string (PHC format, only used in Password mode).
    password_hash: Option<String>,
    /// Rate limiter for failed authentication attempts (keyed by client IP).
    auth_rate_limiter: RateLimiter,
}

impl GatewayAuth {
    /// Create a new auth manager with the given mode.
    pub fn new(mode: AuthMode) -> Self {
        use crate::security::rate_limit::RateLimitConfig;

        info!("[GATEWAY_AUTH] Initialized with mode: {:?}", mode);
        Self {
            mode,
            tokens: HashSet::new(),
            password_hash: None,
            auth_rate_limiter: RateLimiter::new(RateLimitConfig {
                max_attempts: 10,
                window_seconds: 60,
                lockout_seconds: 300,
            }),
        }
    }

    /// Register a valid bearer token.
    #[allow(dead_code)]
    pub fn add_token(&mut self, token: String) {
        info!(
            "[GATEWAY_AUTH] Token added (tokens registered: {})",
            self.tokens.len() + 1
        );
        self.tokens.insert(token);
    }

    /// Revoke a previously registered token.
    #[allow(dead_code)]
    pub fn remove_token(&mut self, token: &str) -> bool {
        let removed = self.tokens.remove(token);
        if removed {
            info!(
                "[GATEWAY_AUTH] Token removed (tokens registered: {})",
                self.tokens.len()
            );
        } else {
            debug!("[GATEWAY_AUTH] Attempted to remove unknown token");
        }
        removed
    }

    /// Set the password for Password mode.
    ///
    /// The password is hashed with Argon2id and a random salt.
    #[allow(dead_code)]
    pub fn set_password(&mut self, password: &str) {
        match hash_password_argon2(password) {
            Ok(hash) => {
                self.password_hash = Some(hash);
                info!("[GATEWAY_AUTH] Password hash set (Argon2id)");
            }
            Err(e) => {
                warn!("[GATEWAY_AUTH] Failed to hash password: {}", e);
            }
        }
    }

    /// Authenticate a set of credentials against the current mode.
    pub fn authenticate(&self, credentials: &AuthCredentials) -> Result<AuthResult> {
        match self.mode {
            AuthMode::Token => self.authenticate_token(credentials),
            AuthMode::Password => self.authenticate_password(credentials),
            AuthMode::Open => self.authenticate_open(),
            AuthMode::TailscaleProxy => self.authenticate_tailscale_proxy(credentials),
        }
    }

    /// Authenticate with rate limiting based on client IP address.
    ///
    /// Checks whether the client IP is already rate-limited before attempting
    /// authentication. On failure, records the attempt against the rate limiter.
    pub fn authenticate_with_rate_limit(
        &self,
        credentials: &AuthCredentials,
        client_addr: IpAddr,
    ) -> Result<AuthResult> {
        // Check if the client is currently blocked.
        // NOTE: We use is_blocked_for_auth() instead of is_blocked() here because
        // is_blocked() unconditionally exempts loopback addresses (127.x/::1) to
        // avoid locking out the local CLI from general rate limits.  That exemption
        // is NOT appropriate for authentication: an attacker with local code execution
        // can trivially brute-force credentials from 127.0.0.1, so loopback must be
        // rate-limited for auth just like any other source address.
        if self.auth_rate_limiter.is_blocked_for_auth(client_addr) {
            warn!(
                "[GATEWAY_AUTH] Rate limited: {} is temporarily blocked",
                client_addr
            );
            return Ok(AuthResult::failure());
        }

        let result = self.authenticate(credentials)?;

        if !result.authenticated {
            let locked = self.auth_rate_limiter.record_failure(client_addr);
            if locked {
                warn!(
                    "[GATEWAY_AUTH] Client {} locked out after too many failed attempts",
                    client_addr
                );
            }
        }

        Ok(result)
    }

    /// Return the current authentication mode.
    #[allow(dead_code)]
    pub fn mode(&self) -> &AuthMode {
        &self.mode
    }

    /// Return the number of registered tokens.
    #[allow(dead_code)]
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    // -- private helpers ----------------------------------------------------

    fn authenticate_token(&self, credentials: &AuthCredentials) -> Result<AuthResult> {
        match credentials {
            AuthCredentials::Token(supplied) => {
                // Constant-time comparison against each registered token.
                let matched = self
                    .tokens
                    .iter()
                    .any(|valid| secure_compare(valid, supplied));
                if matched {
                    debug!("[GATEWAY_AUTH] Token authentication succeeded");
                    Ok(AuthResult::success(
                        format!("token-user-{}", &Uuid::new_v4().to_string()[..8]),
                        vec!["chat".to_string(), "tools".to_string()],
                    ))
                } else {
                    warn!("[GATEWAY_AUTH] Token authentication failed");
                    Ok(AuthResult::failure())
                }
            }
            _ => {
                warn!("[GATEWAY_AUTH] Token mode requires Token credentials");
                Ok(AuthResult::failure())
            }
        }
    }

    fn authenticate_password(&self, credentials: &AuthCredentials) -> Result<AuthResult> {
        match credentials {
            AuthCredentials::Password(supplied) => {
                let matched = match &self.password_hash {
                    Some(stored) => verify_password_argon2(supplied, stored),
                    None => {
                        warn!(
                            "[GATEWAY_AUTH] Password mode active but no password hash configured"
                        );
                        false
                    }
                };

                if matched {
                    debug!("[GATEWAY_AUTH] Password authentication succeeded");
                    Ok(AuthResult::success(
                        format!("pw-user-{}", &Uuid::new_v4().to_string()[..8]),
                        vec!["chat".to_string()],
                    ))
                } else {
                    warn!("[GATEWAY_AUTH] Password authentication failed");
                    Ok(AuthResult::failure())
                }
            }
            _ => {
                warn!("[GATEWAY_AUTH] Password mode requires Password credentials");
                Ok(AuthResult::failure())
            }
        }
    }

    fn authenticate_open(&self) -> Result<AuthResult> {
        debug!("[GATEWAY_AUTH] Open mode — accepting connection as anonymous");
        Ok(AuthResult::success(
            "anonymous".to_string(),
            vec!["chat".to_string(), "tools".to_string()],
        ))
    }

    /// Authenticate using identity headers injected by a Tailscale trusted reverse proxy.
    ///
    /// Requires `Tailscale-User-Login` to be non-empty. The login value is sanitized
    /// into a stable user_id string. Should only be called for loopback connections.
    fn authenticate_tailscale_proxy(&self, credentials: &AuthCredentials) -> Result<AuthResult> {
        match credentials {
            AuthCredentials::TailscaleHeaders { login, .. } => {
                if login.is_empty() {
                    warn!(
                        "[GATEWAY_AUTH] TailscaleProxy: empty Tailscale-User-Login header, rejecting"
                    );
                    return Ok(AuthResult::failure());
                }
                // Sanitize login into a safe user_id (e.g. "user@example.com" → "ts-user-example-com")
                let user_id = format!(
                    "ts-{}",
                    login.replace(|c: char| !c.is_alphanumeric() && c != '-', "-")
                );
                info!("[GATEWAY_AUTH] TailscaleProxy: authenticated '{}'", login);
                Ok(AuthResult::success(
                    user_id,
                    vec!["chat".to_string(), "tools".to_string()],
                ))
            }
            _ => {
                warn!(
                    "[GATEWAY_AUTH] TailscaleProxy mode requires TailscaleHeaders credentials \
                     (HTTP headers set by nginx/tailscale-nginx-auth proxy)"
                );
                Ok(AuthResult::failure())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Argon2id password hashing
// ---------------------------------------------------------------------------

/// Hash a password using Argon2id with a random salt. Returns PHC-format string.
#[allow(dead_code)]
fn argon2_strong() -> Argon2<'static> {
    // 64 MiB memory, 3 iterations, 1-way parallelism — stronger than the
    // argon2 crate's default (19 MiB / 2 iter) for a desktop app in 2026.
    let params = Params::new(64 * 1024, 3, 1, None)
        .expect("invariant: hard-coded Argon2 params are valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

fn hash_password_argon2(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon2_strong()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("Argon2id hashing failed: {}", e))?;
    Ok(hash.to_string())
}

/// Verify a password against an Argon2id PHC-format hash string.
fn verify_password_argon2(password: &str, hash_str: &str) -> bool {
    let parsed = match PasswordHash::new(hash_str) {
        Ok(h) => h,
        Err(e) => {
            warn!("[GATEWAY_AUTH] Failed to parse password hash: {}", e);
            return false;
        }
    };
    // Argon2::verify_password reads params from the PHC string, so existing
    // hashes created with the old default params remain verifiable.
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_mode_always_succeeds() {
        let auth = GatewayAuth::new(AuthMode::Open);
        let result = auth.authenticate(&AuthCredentials::None).unwrap();
        assert!(result.authenticated);
        assert_eq!(result.user_id, "anonymous");
        assert!(!result.permissions.is_empty());
    }

    #[test]
    fn test_open_mode_accepts_any_credentials() {
        let auth = GatewayAuth::new(AuthMode::Open);
        // Even token credentials work in open mode (mode overrides credential type)
        let result = auth
            .authenticate(&AuthCredentials::Token("anything".into()))
            .unwrap();
        assert!(result.authenticated);
    }

    #[test]
    fn test_token_mode_valid_token() {
        let mut auth = GatewayAuth::new(AuthMode::Token);
        auth.add_token("secret-token-123".to_string());

        let result = auth
            .authenticate(&AuthCredentials::Token("secret-token-123".into()))
            .unwrap();
        assert!(result.authenticated);
        assert!(result.user_id.starts_with("token-user-"));
    }

    #[test]
    fn test_token_mode_invalid_token() {
        let mut auth = GatewayAuth::new(AuthMode::Token);
        auth.add_token("secret-token-123".to_string());

        let result = auth
            .authenticate(&AuthCredentials::Token("wrong-token".into()))
            .unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_token_mode_no_tokens_registered() {
        let auth = GatewayAuth::new(AuthMode::Token);
        let result = auth
            .authenticate(&AuthCredentials::Token("any".into()))
            .unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_token_mode_wrong_credential_type() {
        let auth = GatewayAuth::new(AuthMode::Token);
        let result = auth.authenticate(&AuthCredentials::None).unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_token_add_and_remove() {
        let mut auth = GatewayAuth::new(AuthMode::Token);
        auth.add_token("tok-1".to_string());
        assert_eq!(auth.token_count(), 1);

        assert!(auth.remove_token("tok-1"));
        assert_eq!(auth.token_count(), 0);

        // Removing non-existent token returns false
        assert!(!auth.remove_token("tok-1"));
    }

    #[test]
    fn test_password_mode_valid() {
        let mut auth = GatewayAuth::new(AuthMode::Password);
        auth.set_password("my-password");

        let result = auth
            .authenticate(&AuthCredentials::Password("my-password".into()))
            .unwrap();
        assert!(result.authenticated);
        assert!(result.user_id.starts_with("pw-user-"));
    }

    #[test]
    fn test_password_mode_invalid() {
        let mut auth = GatewayAuth::new(AuthMode::Password);
        auth.set_password("correct-horse-battery-staple");

        let result = auth
            .authenticate(&AuthCredentials::Password("wrong".into()))
            .unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_password_mode_no_password_set() {
        let auth = GatewayAuth::new(AuthMode::Password);
        let result = auth
            .authenticate(&AuthCredentials::Password("any".into()))
            .unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_password_mode_wrong_credential_type() {
        let mut auth = GatewayAuth::new(AuthMode::Password);
        auth.set_password("pw");

        let result = auth
            .authenticate(&AuthCredentials::Token("pw".into()))
            .unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_argon2_hash_is_phc_format() {
        let hash = hash_password_argon2("test-password").unwrap();
        assert!(
            hash.starts_with("$argon2"),
            "Hash should be PHC format: {}",
            hash
        );
    }

    #[test]
    fn test_argon2_different_passwords_different_hashes() {
        let h1 = hash_password_argon2("password1").unwrap();
        let h2 = hash_password_argon2("password2").unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_argon2_same_password_different_salts() {
        let h1 = hash_password_argon2("same-password").unwrap();
        let h2 = hash_password_argon2("same-password").unwrap();
        // Different random salts produce different hashes
        assert_ne!(h1, h2);
        // But both verify correctly
        assert!(verify_password_argon2("same-password", &h1));
        assert!(verify_password_argon2("same-password", &h2));
    }

    #[test]
    fn test_multiple_tokens() {
        let mut auth = GatewayAuth::new(AuthMode::Token);
        auth.add_token("tok-a".to_string());
        auth.add_token("tok-b".to_string());
        auth.add_token("tok-c".to_string());

        assert!(
            auth.authenticate(&AuthCredentials::Token("tok-b".into()))
                .unwrap()
                .authenticated
        );
        assert!(
            !auth
                .authenticate(&AuthCredentials::Token("tok-d".into()))
                .unwrap()
                .authenticated
        );
    }

    #[test]
    fn test_tailscale_proxy_valid_login() {
        let auth = GatewayAuth::new(AuthMode::TailscaleProxy);
        let result = auth
            .authenticate(&AuthCredentials::TailscaleHeaders {
                login: "alice@example.com".into(),
                name: "Alice".into(),
            })
            .unwrap();
        assert!(result.authenticated);
        assert!(result.user_id.starts_with("ts-"));
        assert!(!result.permissions.is_empty());
    }

    #[test]
    fn test_tailscale_proxy_empty_login_rejected() {
        let auth = GatewayAuth::new(AuthMode::TailscaleProxy);
        let result = auth
            .authenticate(&AuthCredentials::TailscaleHeaders {
                login: "".into(),
                name: "".into(),
            })
            .unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_tailscale_proxy_wrong_credential_type() {
        let auth = GatewayAuth::new(AuthMode::TailscaleProxy);
        let result = auth
            .authenticate(&AuthCredentials::Token("some-token".into()))
            .unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_tailscale_proxy_user_id_sanitization() {
        let auth = GatewayAuth::new(AuthMode::TailscaleProxy);
        let result = auth
            .authenticate(&AuthCredentials::TailscaleHeaders {
                login: "user@domain.com".into(),
                name: "User".into(),
            })
            .unwrap();
        assert!(result.authenticated);
        // '@' and '.' should be replaced with '-'
        assert!(
            result.user_id.starts_with("ts-user-domain-com"),
            "unexpected user_id: {}",
            result.user_id
        );
    }
}
