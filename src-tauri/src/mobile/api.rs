//! REST API endpoints for mobile companion apps.
//!
//! Provides a lightweight HTTP API that iOS/watchOS apps can use
//! to send messages and receive responses.
#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::info;

/// Type of mobile device connecting to the API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    IPhone,
    AppleWatch,
    IPad,
    Android,
    Other,
}

/// Incoming request from a mobile client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileRequest {
    /// The message text from the user.
    pub message: String,
    /// Optional session ID to continue a conversation.
    pub session_id: Option<String>,
    /// Unique identifier for the device.
    pub device_id: String,
    /// Type of device making the request.
    pub device_type: DeviceType,
    /// Optional push notification token for async replies.
    pub push_token: Option<String>,
}

/// Full response sent back to mobile clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileResponse {
    /// The response text.
    pub text: String,
    /// Session ID for continuing the conversation.
    pub session_id: String,
    /// ISO 8601 timestamp of the response.
    pub timestamp: String,
    /// Whether the response was truncated to fit device limits.
    pub truncated: bool,
}

/// Compact response optimised for watchOS (max 500 chars).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactResponse {
    /// Shortened response text (max 500 characters).
    pub text: String,
    /// Session ID for continuing the conversation.
    pub session_id: String,
}

impl CompactResponse {
    /// Create a compact response from a full response.
    pub fn from_full(response: &MobileResponse) -> Self {
        let text = if response.text.len() > 500 {
            let mut truncated = response.text[..497].to_string();
            truncated.push_str("...");
            truncated
        } else {
            response.text.clone()
        };
        Self {
            text,
            session_id: response.session_id.clone(),
        }
    }
}

/// Registration record for a connected mobile device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRegistration {
    pub device_id: String,
    pub device_type: DeviceType,
    pub push_token: Option<String>,
    pub registered_at: chrono::DateTime<chrono::Utc>,
    pub last_seen: chrono::DateTime<chrono::Utc>,
}

/// Shared state for the mobile API.
#[derive(Debug, Clone)]
pub struct MobileApiState {
    /// Map of device_id -> registration info.
    pub registered_devices: HashMap<String, DeviceRegistration>,
    /// Map of device_id -> push token (quick lookup).
    pub push_tokens: HashMap<String, String>,
}

impl MobileApiState {
    pub fn new() -> Self {
        Self {
            registered_devices: HashMap::new(),
            push_tokens: HashMap::new(),
        }
    }

    /// Register a device with the mobile API.
    pub fn register_device(
        &mut self,
        device_id: &str,
        device_type: DeviceType,
        push_token: Option<String>,
    ) -> Result<()> {
        let now = chrono::Utc::now();

        if let Some(token) = &push_token {
            self.push_tokens
                .insert(device_id.to_string(), token.clone());
        }

        let registration = DeviceRegistration {
            device_id: device_id.to_string(),
            device_type,
            push_token,
            registered_at: now,
            last_seen: now,
        };

        self.registered_devices
            .insert(device_id.to_string(), registration);

        info!(
            "[MOBILE] Registered device: {} ({:?})",
            device_id,
            self.registered_devices
                .get(device_id)
                .map(|r| &r.device_type)
        );

        Ok(())
    }

    /// Unregister a device from the mobile API.
    pub fn unregister_device(&mut self, device_id: &str) {
        self.registered_devices.remove(device_id);
        self.push_tokens.remove(device_id);
        info!("[MOBILE] Unregistered device: {}", device_id);
    }
}

/// Format a response string for a specific device type.
///
/// - For `AppleWatch`: strips markdown, truncates to 500 chars.
/// - For `IPhone` / `IPad`: keeps markdown, truncates to `max_length`.
/// - For everything else: truncates to `max_length`.
pub fn format_for_device(text: &str, device_type: &DeviceType, max_length: usize) -> String {
    match device_type {
        DeviceType::AppleWatch => {
            // Strip basic markdown formatting for watch display
            let stripped = strip_markdown(text);
            let limit = 500.min(max_length);
            if stripped.len() > limit {
                let mut result = stripped[..limit.saturating_sub(3)].to_string();
                result.push_str("...");
                result
            } else {
                stripped
            }
        }
        DeviceType::IPhone | DeviceType::IPad => {
            if text.len() > max_length {
                let mut result = text[..max_length.saturating_sub(3)].to_string();
                result.push_str("...");
                result
            } else {
                text.to_string()
            }
        }
        DeviceType::Android | DeviceType::Other => {
            if text.len() > max_length {
                let mut result = text[..max_length.saturating_sub(3)].to_string();
                result.push_str("...");
                result
            } else {
                text.to_string()
            }
        }
    }
}

/// Strip basic markdown formatting from text.
fn strip_markdown(text: &str) -> String {
    let mut result = text.to_string();
    // Remove bold markers
    result = result.replace("**", "");
    result = result.replace("__", "");
    // Remove italic markers (single * or _)
    // Be careful not to remove list bullets
    result = result.replace(" *", " ");
    result = result.replace("* ", " ");
    // Remove code blocks
    result = result.replace("```", "");
    result = result.replace('`', "");
    // Remove heading markers
    for _ in 0..6 {
        result = result.replace("# ", "");
    }
    // Remove link syntax: [text](url) -> text
    while let Some(start) = result.find('[') {
        if let Some(mid) = result[start..].find("](") {
            if let Some(end) = result[start + mid..].find(')') {
                let link_text = &result[start + 1..start + mid].to_string();
                let full_link = &result[start..start + mid + end + 1].to_string();
                result = result.replace(full_link.as_str(), link_text.as_str());
                continue;
            }
        }
        break;
    }
    result
}

/// Specification for a single API route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiRoute {
    /// HTTP method (GET, POST, DELETE, etc.).
    pub method: String,
    /// URL path pattern.
    pub path: String,
    /// Human-readable description.
    pub description: String,
}

/// Build the list of route specifications for the mobile API.
pub fn build_api_routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute {
            method: "POST".to_string(),
            path: "/api/mobile/message".to_string(),
            description: "Send a message and receive a response".to_string(),
        },
        ApiRoute {
            method: "POST".to_string(),
            path: "/api/mobile/register".to_string(),
            description: "Register a mobile device".to_string(),
        },
        ApiRoute {
            method: "DELETE".to_string(),
            path: "/api/mobile/register/{device_id}".to_string(),
            description: "Unregister a mobile device".to_string(),
        },
        ApiRoute {
            method: "GET".to_string(),
            path: "/api/mobile/status".to_string(),
            description: "Health check / status endpoint".to_string(),
        },
        ApiRoute {
            method: "GET".to_string(),
            path: "/api/mobile/sessions".to_string(),
            description: "List active conversation sessions".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_for_watch_truncates() {
        let long_text = "a".repeat(600);
        let result = format_for_device(&long_text, &DeviceType::AppleWatch, 4096);
        assert!(result.len() <= 500);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_format_for_watch_strips_markdown() {
        let md = "**bold** and `code` and # heading";
        let result = format_for_device(md, &DeviceType::AppleWatch, 4096);
        assert!(!result.contains("**"));
        assert!(!result.contains('`'));
    }

    #[test]
    fn test_format_for_iphone_keeps_markdown() {
        let md = "**bold** text";
        let result = format_for_device(md, &DeviceType::IPhone, 4096);
        assert!(result.contains("**bold**"));
    }

    #[test]
    fn test_format_for_iphone_truncates() {
        let long_text = "a".repeat(5000);
        let result = format_for_device(&long_text, &DeviceType::IPhone, 4096);
        assert!(result.len() <= 4096);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_format_short_text_unchanged() {
        let text = "Hello, world!";
        let result = format_for_device(text, &DeviceType::IPhone, 4096);
        assert_eq!(result, text);
    }

    #[test]
    fn test_compact_response_from_full() {
        let full = MobileResponse {
            text: "a".repeat(600),
            session_id: "sess-1".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            truncated: false,
        };
        let compact = CompactResponse::from_full(&full);
        assert!(compact.text.len() <= 500);
        assert!(compact.text.ends_with("..."));
        assert_eq!(compact.session_id, "sess-1");
    }

    #[test]
    fn test_compact_response_short_text() {
        let full = MobileResponse {
            text: "short".to_string(),
            session_id: "sess-2".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            truncated: false,
        };
        let compact = CompactResponse::from_full(&full);
        assert_eq!(compact.text, "short");
    }

    #[test]
    fn test_device_registration() {
        let mut state = MobileApiState::new();
        state
            .register_device(
                "dev-001",
                DeviceType::IPhone,
                Some("push-token-abc".to_string()),
            )
            .unwrap();

        assert!(state.registered_devices.contains_key("dev-001"));
        assert_eq!(
            state.push_tokens.get("dev-001").map(|s| s.as_str()),
            Some("push-token-abc")
        );
    }

    #[test]
    fn test_device_unregistration() {
        let mut state = MobileApiState::new();
        state
            .register_device("dev-002", DeviceType::AppleWatch, None)
            .unwrap();
        assert!(state.registered_devices.contains_key("dev-002"));

        state.unregister_device("dev-002");
        assert!(!state.registered_devices.contains_key("dev-002"));
    }

    #[test]
    fn test_build_api_routes() {
        let routes = build_api_routes();
        assert_eq!(routes.len(), 5);
        assert_eq!(routes[0].method, "POST");
        assert_eq!(routes[0].path, "/api/mobile/message");
        assert_eq!(routes[2].method, "DELETE");
        assert!(routes[2].path.contains("{device_id}"));
    }

    #[test]
    fn test_device_type_serde() {
        let dt = DeviceType::AppleWatch;
        let json = serde_json::to_string(&dt).unwrap();
        assert_eq!(json, "\"apple_watch\"");
        let deserialized: DeviceType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, DeviceType::AppleWatch);
    }

    #[test]
    fn test_mobile_request_deserialization() {
        let json = r#"{
            "message": "Hello",
            "session_id": null,
            "device_id": "dev-123",
            "device_type": "i_phone",
            "push_token": "token-abc"
        }"#;
        let req: MobileRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "Hello");
        assert_eq!(req.device_id, "dev-123");
        assert_eq!(req.device_type, DeviceType::IPhone);
        assert_eq!(req.push_token.as_deref(), Some("token-abc"));
    }
}
