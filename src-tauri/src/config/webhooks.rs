//! Webhook server, TLS, and browser CDP configurations.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}
fn default_webhook_port() -> u16 {
    18791
}

/// TLS configuration for the webhook server.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TlsConfig {
    /// Whether TLS is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Auto-generate a self-signed certificate if cert files are missing
    #[serde(default)]
    pub auto_generate: bool,
    /// Path to PEM certificate file
    #[serde(default)]
    pub cert_path: Option<String>,
    /// Path to PEM private key file
    #[serde(default)]
    pub key_path: Option<String>,
}

/// Webhook server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Whether the webhook server is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Port for the webhook HTTP server
    #[serde(default = "default_webhook_port")]
    pub port: u16,
    /// Bearer token for authentication
    pub auth_token: Option<String>,
    /// Configured webhook endpoints
    #[serde(default)]
    pub endpoints: Vec<WebhookEndpoint>,
    /// TLS configuration
    #[serde(default)]
    pub tls: TlsConfig,
    /// Rate limiting configuration
    #[serde(default)]
    pub rate_limit: crate::rate_limiter::RateLimitConfig,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_webhook_port(),
            auth_token: None,
            endpoints: Vec::new(),
            tls: TlsConfig::default(),
            rate_limit: crate::rate_limiter::RateLimitConfig::default(),
        }
    }
}

/// A webhook endpoint configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEndpoint {
    /// Unique endpoint ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Action to perform when triggered
    pub action: WebhookAction,
    /// Target: task ID for TriggerTask, or prompt template with {{body}} for SendMessage
    pub target: String,
}

/// Action to perform when a webhook is triggered
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WebhookAction {
    /// Trigger a scheduled task by ID
    TriggerTask,
    /// Send a message to Claude (target is prompt template with {{body}})
    SendMessage,
}

/// Browser CDP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// Enable browser tool
    #[serde(default)]
    pub enabled: bool,
    /// Run browser in headless mode
    #[serde(default)]
    pub headless: bool,
    /// Default timeout for browser operations in milliseconds
    #[serde(default)]
    pub default_timeout_ms: u64,
    /// Path to Chrome/Chromium binary (None = auto-detect)
    #[serde(default)]
    pub chrome_path: Option<String>,
    /// Viewport width
    #[serde(default)]
    pub viewport_width: u32,
    /// Viewport height
    #[serde(default)]
    pub viewport_height: u32,
    /// Require user confirmation before non-read-only actions (default: true)
    #[serde(default = "default_true")]
    pub require_confirmation: bool,
    /// Allowed domains (empty = allow all). Only URLs matching these domains will be navigated to.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Run browser tool through guardrails check (default: true)
    #[serde(default = "default_true")]
    pub use_guardrails: bool,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            headless: true,
            default_timeout_ms: 30000,
            chrome_path: None,
            viewport_width: 1280,
            viewport_height: 720,
            require_confirmation: true,
            allowed_domains: Vec::new(),
            use_guardrails: true,
        }
    }
}
