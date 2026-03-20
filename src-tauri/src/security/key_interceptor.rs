//! Thin coordinator for Smart Key Vault interception.
//!
//! Owns a `KeyVault` reference and exposes the four interception functions
//! that are called at the model boundary:
//!
//! | Point            | Direction | Action                          |
//! |------------------|-----------|---------------------------------|
//! | Chat ingress     | inbound   | scan message, real → proxy      |
//! | Config save      | inbound   | scan config values, real → proxy|
//! | Tool input       | outbound  | restore proxy → real in JSON    |
//! | Tool result      | inbound   | scan result, real → proxy       |

use std::sync::Arc;
use tracing::debug;

use super::key_vault::KeyVault;

/// Coordinator for all key vault interception points.
#[derive(Clone)]
pub struct KeyInterceptor {
    vault: Arc<KeyVault>,
}

impl KeyInterceptor {
    pub fn new(vault: Arc<KeyVault>) -> Self {
        Self { vault }
    }

    /// Whether the vault is currently enabled.
    pub fn is_enabled(&self) -> bool {
        self.vault.enabled
    }

    /// Intercept an inbound text (chat message or tool result).
    ///
    /// Scans `text` for real API keys, stores them in the vault, and returns
    /// a sanitized string where real keys have been replaced by proxy keys.
    pub fn intercept_message(&self, text: &str) -> String {
        if !self.vault.enabled {
            return text.to_string();
        }
        let (sanitized, interceptions) = self.vault.scan_and_replace(text);
        if !interceptions.is_empty() {
            debug!(
                "[KEY_INTERCEPTOR] Intercepted {} key(s) in inbound text",
                interceptions.len()
            );
        }
        sanitized
    }

    /// Restore proxy keys in outbound tool input JSON before tool execution.
    pub fn restore_tool_input(&self, input: &mut serde_json::Value) {
        if !self.vault.enabled {
            return;
        }
        self.vault.restore_in_json(input);
    }

    /// Intercept a config string value (for the Settings UI / config save path).
    ///
    /// If the value looks like a real API key, stores it in the vault and
    /// returns the proxy key instead.
    pub fn intercept_config_string(&self, value: &str) -> String {
        if !self.vault.enabled {
            return value.to_string();
        }
        let (sanitized, _) = self.vault.scan_and_replace(value);
        sanitized
    }

    /// Restore a config value that may contain a vault proxy token.
    ///
    /// This is intentionally available even when interception is disabled so
    /// older configs containing proxy values still resolve at runtime.
    pub fn restore_config_string(&self, value: &str) -> String {
        self.vault.restore_in_text(value)
    }

    /// Access the underlying vault (for Tauri commands: list, revoke, label).
    pub fn vault(&self) -> &KeyVault {
        &self.vault
    }
}
