//! Push notification hooks for APNs (Apple Push Notification service).
//!
//! Implements JWT-based APNs HTTP/2 delivery using the Provider Authentication Token
//! (ES256) scheme. Requires `apns_key_path`, `apns_key_id`, `apns_team_id`, and
//! `apns_bundle_id` to be set in the mobile config. If any field is missing, push
//! delivery is skipped with a warning.
//!
//! APNs endpoint: POST https://api.push.apple.com/3/device/{token}
//! JWT is re-signed every 55 minutes (Apple requires refresh before 60 min).
#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{info, warn};

// ============================================================================
// APNs JWT Token Cache
// ============================================================================

/// Cached APNs provider token (valid for 60 min; we refresh every 55).
#[derive(Debug)]
struct ApnsTokenCache {
    token: String,
    issued_at: Instant,
}

impl ApnsTokenCache {
    /// Return true if the token should be refreshed (issued more than 55 min ago).
    fn is_stale(&self) -> bool {
        self.issued_at.elapsed() > Duration::from_secs(55 * 60)
    }
}

// ============================================================================
// APNs JWT Claims
// ============================================================================

#[derive(Serialize)]
struct ApnsJwtClaims {
    /// Issuer: the team ID (10-character string from Apple Developer account).
    iss: String,
    /// Issued-at epoch timestamp.
    iat: i64,
}

// ============================================================================
// APNs Client
// ============================================================================

/// APNs error codes that indicate the device token is invalid.
const APNS_INVALID_TOKEN_REASONS: &[&str] = &[
    "BadDeviceToken",
    "Unregistered",
    "MissingDeviceToken",
    "InvalidProviderToken",
];

/// Reasons that indicate an APNs rate-limit response.
const APNS_RATE_LIMIT_STATUS: u16 = 429;

/// Production APNs host (HTTP/2 required).
const APNS_HOST: &str = "https://api.push.apple.com";

/// Configuration for the APNs client (subset of MobileConfig).
#[derive(Debug, Clone)]
pub struct ApnsConfig {
    /// Path to the `.p8` private key file downloaded from Apple Developer Portal.
    pub key_path: String,
    /// 10-character Key ID from Apple Developer Portal.
    pub key_id: String,
    /// 10-character Team ID from Apple Developer Portal.
    pub team_id: String,
    /// The app's bundle identifier (e.g. `ai.nexibot.companion`).
    pub bundle_id: String,
}

/// Real APNs HTTP/2 client with JWT-based provider authentication.
pub struct ApnsClient {
    config: ApnsConfig,
    http: reqwest::Client,
    token_cache: Arc<Mutex<Option<ApnsTokenCache>>>,
}

impl ApnsClient {
    /// Create a new `ApnsClient`.
    ///
    /// Returns `Err` if the `.p8` key file cannot be read or parsed.
    pub fn new(config: ApnsConfig) -> Result<Self> {
        // HTTP/2 is mandatory for APNs. reqwest with rustls uses HTTP/2 by default
        // when the server advertises it via ALPN, which APNs always does.
        let http = reqwest::Client::builder()
            .http2_prior_knowledge()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build APNs HTTP client")?;

        Ok(Self {
            config,
            http,
            token_cache: Arc::new(Mutex::new(None)),
        })
    }

    /// Build or return a cached APNs JWT provider token.
    async fn provider_token(&self) -> Result<String> {
        let mut cache = self.token_cache.lock().await;
        if let Some(ref cached) = *cache {
            if !cached.is_stale() {
                return Ok(cached.token.clone());
            }
        }

        // (Re-)sign the JWT. Reading the .p8 key file is blocking I/O, so
        // run it off the async executor to avoid stalling other tasks.
        let key_path = self.config.key_path.clone();
        let key_id = self.config.key_id.clone();
        let team_id = self.config.team_id.clone();
        let token = tokio::task::spawn_blocking(move || {
            ApnsClient::sign_jwt_with(&key_path, &key_id, &team_id)
        })
        .await
        .context("APNs JWT signing task panicked")?
        .context("APNs JWT signing failed")?;

        info!("[APNs] Issued new provider JWT for team {}", self.config.team_id);
        *cache = Some(ApnsTokenCache {
            token: token.clone(),
            issued_at: Instant::now(),
        });
        Ok(token)
    }

    /// Sign a new APNs JWT using ES256 and the `.p8` key.
    fn sign_jwt(&self) -> Result<String> {
        Self::sign_jwt_with(&self.config.key_path, &self.config.key_id, &self.config.team_id)
    }

    fn sign_jwt_with(key_path: &str, key_id: &str, team_id: &str) -> Result<String> {
        let key_pem = std::fs::read_to_string(key_path)
            .with_context(|| format!("Cannot read APNs key file: {}", key_path))?;

        let encoding_key = EncodingKey::from_ec_pem(key_pem.as_bytes())
            .context("Failed to parse APNs ES256 key")?;

        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(key_id.to_string());

        let claims = ApnsJwtClaims {
            iss: team_id.to_string(),
            iat: Utc::now().timestamp(),
        };

        jsonwebtoken::encode(&header, &claims, &encoding_key)
            .context("Failed to sign APNs JWT")
    }

    /// Send a push notification to a single device token.
    ///
    /// - Returns `Ok(())` on success (HTTP 200).
    /// - Returns `Err` with a descriptive message on failure.
    /// - The caller can inspect the error message to detect `"BadDeviceToken"` /
    ///   `"Unregistered"` and remove the token from the registry.
    pub async fn send_push(
        &self,
        device_token: &str,
        notification: &PushNotification,
        extra_data: Option<Value>,
    ) -> Result<()> {
        let provider_token = self.provider_token().await?;
        let payload = build_payload(notification, extra_data);

        let url = format!("{}/3/device/{}", APNS_HOST, device_token);

        let response = self
            .http
            .post(&url)
            .header("authorization", format!("bearer {}", provider_token))
            .header("apns-push-type", "alert")
            .header("apns-topic", &self.config.bundle_id)
            .header("apns-expiration", "0")
            .header("apns-priority", "10")
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&payload)?)
            .send()
            .await
            .context("APNs HTTP request failed")?;

        let status = response.status();

        if status.is_success() {
            info!(
                "[APNs] Delivered to device {} (status {})",
                &device_token[..device_token.len().min(8)],
                status.as_u16()
            );
            return Ok(());
        }

        // Parse APNs error body.
        let body: Value = response.json().await.unwrap_or_default();
        let reason = body.get("reason").and_then(|v| v.as_str()).unwrap_or("Unknown");

        if status.as_u16() == APNS_RATE_LIMIT_STATUS {
            anyhow::bail!(
                "APNs rate-limited (429) for device {}: {}",
                &device_token[..device_token.len().min(8)],
                reason
            );
        }

        anyhow::bail!(
            "APNs delivery failed for device {} (HTTP {}): {}",
            &device_token[..device_token.len().min(8)],
            status.as_u16(),
            reason
        );
    }

    /// Returns `true` when the APNs error reason indicates the device token is
    /// permanently invalid and should be removed from the registry.
    pub fn is_invalid_token_error(err: &anyhow::Error) -> bool {
        let msg = err.to_string();
        APNS_INVALID_TOKEN_REASONS.iter().any(|r| msg.contains(r))
    }
}

/// Build the complete APNs JSON payload from a `PushNotification` plus optional
/// extra data key-value pairs (passed through as top-level keys).
fn build_payload(notification: &PushNotification, extra: Option<Value>) -> Value {
    let mut payload = format_apns_payload(notification);

    // Merge extra data into the payload root (not inside "aps").
    if let Some(Value::Object(extra_map)) = extra {
        if let Value::Object(ref mut root) = payload {
            for (k, v) in extra_map {
                root.insert(k, v);
            }
        }
    }

    payload
}

// ============================================================================
// Convenience send function (used by the queue drainer)
// ============================================================================

/// Try to construct an `ApnsClient` from `MobileConfig`-style optional fields.
///
/// Returns `None` with a warning log if any required field is absent.
pub fn try_build_apns_client(
    key_path: Option<&str>,
    key_id: Option<&str>,
    team_id: Option<&str>,
    bundle_id: Option<&str>,
) -> Option<ApnsClient> {
    let (Some(key_path), Some(key_id), Some(team_id), Some(bundle_id)) =
        (key_path, key_id, team_id, bundle_id)
    else {
        warn!(
            "[APNs] Missing config (apns_key_path / apns_key_id / apns_team_id / \
             apns_bundle_id). Push delivery disabled."
        );
        return None;
    };

    match ApnsClient::new(ApnsConfig {
        key_path: key_path.to_string(),
        key_id: key_id.to_string(),
        team_id: team_id.to_string(),
        bundle_id: bundle_id.to_string(),
    }) {
        Ok(client) => Some(client),
        Err(e) => {
            warn!("[APNs] Failed to initialise client: {}. Push delivery disabled.", e);
            None
        }
    }
}

/// Drain the push queue, delivering each notification via `client`.
///
/// Invalid device tokens are returned in `invalid_tokens` so the caller can
/// remove them from the device registry. Rate-limit errors apply exponential
/// backoff (up to 60 s) before continuing.
pub async fn drain_queue_with_client(
    queue: &mut PushQueue,
    client: &ApnsClient,
    extra_data: Option<Value>,
) -> Vec<String> {
    let mut invalid_tokens = Vec::new();
    let mut _consecutive_rate_limits: u32 = 0;

    while let Some(notification) = queue.dequeue() {
        // Exponential backoff on consecutive rate-limit responses.
        if _consecutive_rate_limits > 0 {
            let backoff = Duration::from_secs(
                2_u64.pow(_consecutive_rate_limits.min(6)) // cap at 64 s
            );
            warn!(
                "[APNs] Rate-limit backoff: sleeping {}s before next delivery",
                backoff.as_secs()
            );
            tokio::time::sleep(backoff).await;
        }

        match client.send_push(&notification.device_token, &notification, extra_data.clone()).await {
            Ok(()) => {
                queue.record_sent();
                _consecutive_rate_limits = 0;
            }
            Err(ref e) if e.to_string().contains("429") => {
                // Rate limited: put the notification back, back off, and stop.
                warn!("[APNs] Rate-limited — re-queuing notification and backing off");
                let _ = queue.enqueue(notification);
                queue.record_failed();
                _consecutive_rate_limits += 1;
                // Stop draining this pass; caller will retry later.
                break;
            }
            Err(ref e) if ApnsClient::is_invalid_token_error(e) => {
                warn!(
                    "[APNs] Invalid token for device {}: {}. Removing from registry.",
                    &notification.device_token[..notification.device_token.len().min(8)],
                    e
                );
                invalid_tokens.push(notification.device_token.clone());
                queue.record_failed();
                _consecutive_rate_limits = 0;
            }
            Err(e) => {
                warn!("[APNs] Delivery error: {}", e);
                queue.record_failed();
                _consecutive_rate_limits = 0;
            }
        }
    }

    invalid_tokens
}

// ============================================================================
// Data structures
// ============================================================================

/// A push notification ready to be delivered via APNs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushNotification {
    /// The APNs device token for the target device.
    pub device_token: String,
    /// Notification title (displayed prominently).
    pub title: String,
    /// Notification body text.
    pub body: String,
    /// Badge count to display on the app icon.
    pub badge: Option<u32>,
    /// Sound file name or "default".
    pub sound: Option<String>,
    /// Notification category (for actionable notifications).
    pub category: Option<String>,
    /// Thread identifier for notification grouping.
    pub thread_id: Option<String>,
    /// When the notification was created.
    pub created_at: DateTime<Utc>,
}

/// Categories of push notifications sent by NexiBot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PushCategory {
    /// A new chat message has arrived.
    NewMessage,
    /// An agent task has completed.
    AgentComplete,
    /// An error occurred that the user should know about.
    Error,
    /// A scheduled reminder.
    Reminder,
}

impl PushCategory {
    /// Return the APNs category string for actionable notifications.
    pub fn as_apns_category(&self) -> &str {
        match self {
            PushCategory::NewMessage => "NEW_MESSAGE",
            PushCategory::AgentComplete => "AGENT_COMPLETE",
            PushCategory::Error => "ERROR",
            PushCategory::Reminder => "REMINDER",
        }
    }
}

/// Queue for outbound push notifications.
///
/// Notifications are buffered here until an APNs delivery transport picks
/// them up. If the queue reaches capacity, the oldest notifications are
/// dropped to make room for new ones.
#[derive(Debug)]
pub struct PushQueue {
    pending: VecDeque<PushNotification>,
    max_queue_size: usize,
    sent_count: u64,
    failed_count: u64,
}

impl PushQueue {
    /// Create a new push queue with the given maximum capacity.
    pub fn new(max_queue_size: usize) -> Self {
        Self {
            pending: VecDeque::new(),
            max_queue_size,
            sent_count: 0,
            failed_count: 0,
        }
    }

    /// Add a notification to the queue.
    ///
    /// If the queue is at capacity, the oldest notification is dropped.
    pub fn enqueue(&mut self, notification: PushNotification) -> Result<()> {
        if self.pending.len() >= self.max_queue_size {
            let dropped = self.pending.pop_front();
            if let Some(dropped) = dropped {
                warn!(
                    "[PUSH] Queue at capacity ({}), dropping oldest notification for device {}",
                    self.max_queue_size, dropped.device_token
                );
            }
        }
        info!(
            "[PUSH] Enqueued notification for device {} (queue size: {})",
            notification.device_token,
            self.pending.len() + 1
        );
        self.pending.push_back(notification);
        Ok(())
    }

    /// Remove and return the next notification from the queue.
    pub fn dequeue(&mut self) -> Option<PushNotification> {
        self.pending.pop_front()
    }

    /// Return the current number of pending notifications.
    pub fn queue_size(&self) -> usize {
        self.pending.len()
    }

    /// Record that a notification was sent successfully.
    pub fn record_sent(&mut self) {
        self.sent_count += 1;
    }

    /// Record that a notification delivery failed.
    pub fn record_failed(&mut self) {
        self.failed_count += 1;
    }

    /// Total notifications successfully sent.
    pub fn sent_count(&self) -> u64 {
        self.sent_count
    }

    /// Total notifications that failed to send.
    pub fn failed_count(&self) -> u64 {
        self.failed_count
    }
}

/// Format a notification into the standard APNs JSON payload.
///
/// Returns a `serde_json::Value` matching the APNs payload format:
/// ```json
/// {
///   "aps": {
///     "alert": { "title": "...", "body": "..." },
///     "badge": 1,
///     "sound": "default"
///   }
/// }
/// ```
pub fn format_apns_payload(notification: &PushNotification) -> Value {
    let mut aps = json!({
        "alert": {
            "title": notification.title,
            "body": notification.body,
        }
    });

    if let Some(badge) = notification.badge {
        aps["badge"] = json!(badge);
    }

    if let Some(ref sound) = notification.sound {
        aps["sound"] = json!(sound);
    }

    if let Some(ref category) = notification.category {
        aps["category"] = json!(category);
    }

    if let Some(ref thread_id) = notification.thread_id {
        aps["thread-id"] = json!(thread_id);
    }

    json!({ "aps": aps })
}

/// Create a push notification for a new message.
pub fn create_message_notification(
    device_token: &str,
    sender: &str,
    preview: &str,
) -> PushNotification {
    let body = if preview.len() > 100 {
        let mut truncated = preview[..97].to_string();
        truncated.push_str("...");
        truncated
    } else {
        preview.to_string()
    };

    PushNotification {
        device_token: device_token.to_string(),
        title: format!("New message from {}", sender),
        body,
        badge: Some(1),
        sound: Some("default".to_string()),
        category: Some(PushCategory::NewMessage.as_apns_category().to_string()),
        thread_id: None,
        created_at: Utc::now(),
    }
}

/// Create a push notification for a completed agent task.
pub fn create_completion_notification(device_token: &str, task_name: &str) -> PushNotification {
    PushNotification {
        device_token: device_token.to_string(),
        title: "Task Complete".to_string(),
        body: format!("\"{}\" has finished.", task_name),
        badge: None,
        sound: Some("default".to_string()),
        category: Some(PushCategory::AgentComplete.as_apns_category().to_string()),
        thread_id: None,
        created_at: Utc::now(),
    }
}

/// Create a push notification for an error.
pub fn create_error_notification(device_token: &str, error: &str) -> PushNotification {
    let body = if error.len() > 100 {
        let mut truncated = error[..97].to_string();
        truncated.push_str("...");
        truncated
    } else {
        error.to_string()
    };

    PushNotification {
        device_token: device_token.to_string(),
        title: "NexiBot Error".to_string(),
        body,
        badge: None,
        sound: Some("default".to_string()),
        category: Some(PushCategory::Error.as_apns_category().to_string()),
        thread_id: None,
        created_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_queue_enqueue_dequeue() {
        let mut queue = PushQueue::new(10);
        assert_eq!(queue.queue_size(), 0);

        let notif = create_message_notification("token-1", "NexiBot", "Hello!");
        queue.enqueue(notif).unwrap();
        assert_eq!(queue.queue_size(), 1);

        let dequeued = queue.dequeue().unwrap();
        assert_eq!(dequeued.device_token, "token-1");
        assert_eq!(queue.queue_size(), 0);
    }

    #[test]
    fn test_push_queue_capacity_drops_oldest() {
        let mut queue = PushQueue::new(2);

        let n1 = create_message_notification("token-1", "A", "First");
        let n2 = create_message_notification("token-2", "B", "Second");
        let n3 = create_message_notification("token-3", "C", "Third");

        queue.enqueue(n1).unwrap();
        queue.enqueue(n2).unwrap();
        assert_eq!(queue.queue_size(), 2);

        // This should drop the oldest (token-1)
        queue.enqueue(n3).unwrap();
        assert_eq!(queue.queue_size(), 2);

        let first = queue.dequeue().unwrap();
        assert_eq!(first.device_token, "token-2");
    }

    #[test]
    fn test_push_queue_empty_dequeue() {
        let mut queue = PushQueue::new(10);
        assert!(queue.dequeue().is_none());
    }

    #[test]
    fn test_push_queue_counters() {
        let mut queue = PushQueue::new(10);
        assert_eq!(queue.sent_count(), 0);
        assert_eq!(queue.failed_count(), 0);

        queue.record_sent();
        queue.record_sent();
        queue.record_failed();

        assert_eq!(queue.sent_count(), 2);
        assert_eq!(queue.failed_count(), 1);
    }

    #[test]
    fn test_format_apns_payload_full() {
        let notif = PushNotification {
            device_token: "abc123".to_string(),
            title: "Hello".to_string(),
            body: "World".to_string(),
            badge: Some(3),
            sound: Some("default".to_string()),
            category: Some("NEW_MESSAGE".to_string()),
            thread_id: Some("thread-1".to_string()),
            created_at: Utc::now(),
        };

        let payload = format_apns_payload(&notif);
        let aps = &payload["aps"];

        assert_eq!(aps["alert"]["title"], "Hello");
        assert_eq!(aps["alert"]["body"], "World");
        assert_eq!(aps["badge"], 3);
        assert_eq!(aps["sound"], "default");
        assert_eq!(aps["category"], "NEW_MESSAGE");
        assert_eq!(aps["thread-id"], "thread-1");
    }

    #[test]
    fn test_format_apns_payload_minimal() {
        let notif = PushNotification {
            device_token: "abc".to_string(),
            title: "T".to_string(),
            body: "B".to_string(),
            badge: None,
            sound: None,
            category: None,
            thread_id: None,
            created_at: Utc::now(),
        };

        let payload = format_apns_payload(&notif);
        let aps = &payload["aps"];

        assert_eq!(aps["alert"]["title"], "T");
        assert_eq!(aps["alert"]["body"], "B");
        assert!(aps.get("badge").is_none() || aps["badge"].is_null());
        assert!(aps.get("sound").is_none() || aps["sound"].is_null());
    }

    #[test]
    fn test_create_message_notification() {
        let notif = create_message_notification("token-x", "Alice", "Hey there!");
        assert_eq!(notif.title, "New message from Alice");
        assert_eq!(notif.body, "Hey there!");
        assert_eq!(notif.sound.as_deref(), Some("default"));
        assert_eq!(notif.badge, Some(1));
        assert_eq!(notif.category.as_deref(), Some("NEW_MESSAGE"));
    }

    #[test]
    fn test_create_message_notification_long_preview() {
        let long = "x".repeat(200);
        let notif = create_message_notification("token-x", "Bob", &long);
        assert!(notif.body.len() <= 100);
        assert!(notif.body.ends_with("..."));
    }

    #[test]
    fn test_create_completion_notification() {
        let notif = create_completion_notification("token-y", "Research Task");
        assert_eq!(notif.title, "Task Complete");
        assert!(notif.body.contains("Research Task"));
        assert_eq!(notif.category.as_deref(), Some("AGENT_COMPLETE"));
    }

    #[test]
    fn test_create_error_notification() {
        let notif = create_error_notification("token-z", "Connection timed out");
        assert_eq!(notif.title, "NexiBot Error");
        assert_eq!(notif.body, "Connection timed out");
        assert_eq!(notif.category.as_deref(), Some("ERROR"));
    }

    #[test]
    fn test_create_error_notification_long_error() {
        let long_err = "e".repeat(200);
        let notif = create_error_notification("token-z", &long_err);
        assert!(notif.body.len() <= 100);
        assert!(notif.body.ends_with("..."));
    }

    #[test]
    fn test_push_category_serde() {
        let cat = PushCategory::NewMessage;
        let json = serde_json::to_string(&cat).unwrap();
        assert_eq!(json, "\"new_message\"");
        let deserialized: PushCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, PushCategory::NewMessage);
    }

    #[test]
    fn test_push_category_apns_string() {
        assert_eq!(PushCategory::NewMessage.as_apns_category(), "NEW_MESSAGE");
        assert_eq!(
            PushCategory::AgentComplete.as_apns_category(),
            "AGENT_COMPLETE"
        );
        assert_eq!(PushCategory::Error.as_apns_category(), "ERROR");
        assert_eq!(PushCategory::Reminder.as_apns_category(), "REMINDER");
    }
}
