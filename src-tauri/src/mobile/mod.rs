//! Mobile companion app support.
//!
//! Provides REST API endpoints and push notification hooks
//! for iOS/watchOS companion apps to connect to NexiBot.
#![allow(dead_code)]

pub mod api;
pub mod push;

use serde::{Deserialize, Serialize};

/// Mobile gateway configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileConfig {
    pub enabled: bool,
    pub api_port: u16,
    pub require_auth: bool,
    pub apns_key_id: Option<String>,
    pub apns_team_id: Option<String>,
    pub apns_key_path: Option<String>,
    pub apns_bundle_id: Option<String>,
    pub max_message_length: usize,
}

impl Default for MobileConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_port: 18793,
            require_auth: true,
            apns_key_id: None,
            apns_team_id: None,
            apns_key_path: None,
            apns_bundle_id: None,
            max_message_length: 4096,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mobile_config_defaults() {
        let config = MobileConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.api_port, 18793);
        assert!(config.require_auth);
        assert!(config.apns_key_id.is_none());
        assert!(config.apns_team_id.is_none());
        assert!(config.apns_key_path.is_none());
        assert_eq!(config.max_message_length, 4096);
    }

    #[test]
    fn test_mobile_config_serde_roundtrip() {
        let config = MobileConfig {
            enabled: true,
            api_port: 9999,
            require_auth: false,
            apns_key_id: Some("KEY123".to_string()),
            apns_team_id: Some("TEAM456".to_string()),
            apns_key_path: Some("/path/to/key.p8".to_string()),
            apns_bundle_id: None,
            max_message_length: 8192,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: MobileConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.enabled, true);
        assert_eq!(deserialized.api_port, 9999);
        assert_eq!(deserialized.apns_key_id.as_deref(), Some("KEY123"));
    }
}
