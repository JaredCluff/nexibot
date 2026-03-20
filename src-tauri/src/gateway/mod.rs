//! WebSocket gateway for multi-user server mode.
//!
//! Provides WebSocket-based access to NexiBot for multiple concurrent users,
//! with authentication, session management, and a JSON message protocol.

pub mod admin;
pub mod auth;
pub mod method_scopes;
pub mod metrics;
pub mod protocol;
pub mod session_mgr;
pub mod ws_server;

/// Control-plane rate limiter for admin write operations.
/// Limits config.apply, config.patch, update.run to 3 per minute.
pub mod control_plane_rate_limit {
    use crate::security::rate_limit::{RateLimitConfig, RateLimiter};
    use std::sync::{Mutex, OnceLock};

    #[allow(dead_code)]
    fn limiter() -> &'static Mutex<RateLimiter> {
        static INSTANCE: OnceLock<Mutex<RateLimiter>> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            Mutex::new(RateLimiter::new(RateLimitConfig {
                max_attempts: 3,
                window_seconds: 60,
                lockout_seconds: 120,
            }))
        })
    }

    /// Check if a control-plane write operation is allowed.
    /// Key should be "{device_id}:{client_ip}".
    #[allow(dead_code)]
    pub fn check_control_plane_rate(key: &str) -> Result<(), String> {
        let l = limiter().lock().map_err(|e| format!("Lock error: {}", e))?;
        l.check(key)
            .map_err(|e| format!("Control-plane rate limited: {}", e))
    }
}

use serde::{Deserialize, Serialize};

/// Authentication mode for the gateway.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AuthMode {
    /// Bearer-token authentication (each client supplies a pre-shared token).
    #[default]
    Token,
    /// Password authentication (Argon2id hash comparison).
    Password,
    /// Open access for development — every connection is authenticated as "anonymous".
    Open,
    /// Tailscale trusted-proxy mode: the client connects through nginx/tailscale-nginx-auth
    /// which injects `Tailscale-User-Login` and `Tailscale-User-Name` HTTP headers on the
    /// WebSocket upgrade request. The gateway trusts these headers from loopback connections.
    TailscaleProxy,
}

/// Top-level configuration for the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Whether the gateway is active.
    #[serde(default)]
    pub enabled: bool,
    /// TCP port the WebSocket server listens on.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Bind address for the WebSocket server. Default: "127.0.0.1" (loopback only).
    /// Set to "0.0.0.0" to accept connections from all interfaces (requires TLS for remote).
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    /// Authentication mode.
    #[serde(default)]
    pub auth_mode: AuthMode,
    /// Maximum number of concurrent WebSocket connections.
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    /// Whether TLS is enabled for the gateway.
    #[serde(default)]
    pub tls_enabled: bool,
}

impl GatewayConfig {
    /// Validate the gateway configuration before starting the server.
    ///
    /// Returns `Err` with a descriptive message if the configuration is unsafe:
    /// - Binding to a non-loopback address without TLS is rejected.
    /// - `AuthMode::Open` requires the `NEXIBOT_UNSAFE_OPEN_AUTH=1` env var.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }

        // Reject non-loopback bind without TLS
        let is_loopback = self.bind_address == "127.0.0.1" || self.bind_address == "localhost";
        if !is_loopback && !self.tls_enabled {
            return Err(format!(
                "Gateway bind_address is '{}' (non-loopback) but TLS is not enabled. \
                 Refusing to start: remote connections without TLS expose credentials \
                 and session data in plaintext. Either set bind_address to '127.0.0.1', \
                 or enable TLS (gateway.tls_enabled = true).",
                self.bind_address
            ));
        }

        // Reject Open auth mode unless explicitly opted in via env var
        if self.auth_mode == AuthMode::Open {
            match std::env::var("NEXIBOT_UNSAFE_OPEN_AUTH") {
                Ok(val) if val == "1" => {
                    tracing::warn!(
                        "[GATEWAY] Open auth mode enabled via NEXIBOT_UNSAFE_OPEN_AUTH=1. \
                         Any client can connect without credentials."
                    );
                }
                _ => {
                    return Err(
                        "Gateway auth_mode is 'open' but NEXIBOT_UNSAFE_OPEN_AUTH=1 is not set. \
                         Open auth allows unauthenticated access and must be explicitly opted in. \
                         Set NEXIBOT_UNSAFE_OPEN_AUTH=1 to enable, or use 'token' or 'password' auth."
                            .to_string(),
                    );
                }
            }
        }

        Ok(())
    }
}

fn default_port() -> u16 {
    18792
}

fn default_bind_address() -> String {
    "127.0.0.1".to_string()
}

fn default_max_connections() -> usize {
    50
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_port(),
            bind_address: default_bind_address(),
            auth_mode: AuthMode::default(),
            max_connections: default_max_connections(),
            tls_enabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_config_defaults() {
        let config = GatewayConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.port, 18792);
        assert_eq!(config.bind_address, "127.0.0.1");
        assert_eq!(config.auth_mode, AuthMode::Token);
        assert_eq!(config.max_connections, 50);
        assert!(!config.tls_enabled);
    }

    #[test]
    fn test_auth_mode_serde_roundtrip() {
        for mode in [
            AuthMode::Token,
            AuthMode::Password,
            AuthMode::Open,
            AuthMode::TailscaleProxy,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            let deserialized: AuthMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, deserialized);
        }
    }

    #[test]
    fn test_gateway_config_from_json() {
        let json = r#"{"enabled": true, "port": 9090, "auth_mode": "open", "max_connections": 10}"#;
        let config: GatewayConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.port, 9090);
        assert_eq!(config.bind_address, "127.0.0.1"); // default when omitted
        assert_eq!(config.auth_mode, AuthMode::Open);
        assert_eq!(config.max_connections, 10);

        // With explicit bind_address
        let json2 = r#"{"enabled": true, "port": 9090, "bind_address": "0.0.0.0", "auth_mode": "token", "max_connections": 5}"#;
        let config2: GatewayConfig = serde_json::from_str(json2).unwrap();
        assert_eq!(config2.bind_address, "0.0.0.0");
        assert_eq!(config2.max_connections, 5);
    }

    #[test]
    fn test_validate_loopback_without_tls_ok() {
        let config = GatewayConfig {
            enabled: true,
            bind_address: "127.0.0.1".to_string(),
            tls_enabled: false,
            auth_mode: AuthMode::Token,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_non_loopback_without_tls_rejected() {
        let config = GatewayConfig {
            enabled: true,
            bind_address: "0.0.0.0".to_string(),
            tls_enabled: false,
            auth_mode: AuthMode::Token,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("TLS is not enabled"), "Expected TLS error, got: {}", err);
    }

    #[test]
    fn test_validate_non_loopback_with_tls_ok() {
        let config = GatewayConfig {
            enabled: true,
            bind_address: "0.0.0.0".to_string(),
            tls_enabled: true,
            auth_mode: AuthMode::Token,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_open_auth_rejected_without_env() {
        // Ensure the env var is not set for this test
        std::env::remove_var("NEXIBOT_UNSAFE_OPEN_AUTH");
        let config = GatewayConfig {
            enabled: true,
            bind_address: "127.0.0.1".to_string(),
            auth_mode: AuthMode::Open,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("NEXIBOT_UNSAFE_OPEN_AUTH"), "Expected env var error, got: {}", err);
    }

    #[test]
    fn test_validate_open_auth_allowed_with_env() {
        std::env::set_var("NEXIBOT_UNSAFE_OPEN_AUTH", "1");
        let config = GatewayConfig {
            enabled: true,
            bind_address: "127.0.0.1".to_string(),
            auth_mode: AuthMode::Open,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        std::env::remove_var("NEXIBOT_UNSAFE_OPEN_AUTH");
    }

    #[test]
    fn test_validate_disabled_gateway_always_ok() {
        let config = GatewayConfig {
            enabled: false,
            bind_address: "0.0.0.0".to_string(),
            tls_enabled: false,
            auth_mode: AuthMode::Open,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }
}
