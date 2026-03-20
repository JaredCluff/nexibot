//! Yolo Mode — time-limited elevated access for LLM-initiated changes.
//!
//! The model may **request** yolo mode; only a human (via the UI) may **approve** it.
//! Approval is entirely out-of-band from the model's tool-call path.
//!
//! When active, callers can check `is_active()` before allowing privileged
//! operations such as direct config writes.  Auto-expiry is enforced by a
//! background task; the mode can also be manually revoked at any time.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::YoloModeConfig;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Serialisable status returned to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoloStatus {
    pub active: bool,
    pub approved_at_ms: Option<u64>,
    pub expires_at_ms: Option<u64>,
    pub remaining_secs: Option<u64>,
    pub pending_request: Option<YoloRequest>,
}

/// A pending request from the model for yolo mode elevation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoloRequest {
    pub id: String,
    pub requested_at_ms: u64,
    pub duration_secs: Option<u64>,
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct YoloState {
    active: bool,
    approved_at: Option<SystemTime>,
    expires_at: Option<SystemTime>,
    pending_request: Option<YoloRequest>,
    config: YoloModeConfig,
}

impl YoloState {
    fn new(config: YoloModeConfig) -> Self {
        Self {
            active: false,
            approved_at: None,
            expires_at: None,
            pending_request: None,
            config,
        }
    }

    fn status(&self) -> YoloStatus {
        let now = SystemTime::now();

        let approved_at_ms = self
            .approved_at
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64);

        let expires_at_ms = self
            .expires_at
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64);

        let remaining_secs = self
            .expires_at
            .and_then(|exp| exp.duration_since(now).ok().map(|d| d.as_secs()));

        YoloStatus {
            active: self.active,
            approved_at_ms,
            expires_at_ms,
            remaining_secs,
            pending_request: self.pending_request.clone(),
        }
    }

    /// Check whether the session has expired and revoke if so.
    /// Returns true if it was active and just expired.
    fn check_expiry(&mut self) -> bool {
        if !self.active {
            return false;
        }
        if let Some(exp) = self.expires_at {
            if SystemTime::now() >= exp {
                self.active = false;
                self.approved_at = None;
                self.expires_at = None;
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// Broadcast event kinds for UI listeners.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum YoloEvent {
    /// The model has submitted a new request for yolo mode.
    RequestPending { request: YoloRequest },
    /// A pending request was approved by the human.
    Approved { expires_at_ms: Option<u64> },
    /// Yolo mode was manually revoked.
    Revoked,
    /// Yolo mode expired automatically.
    Expired,
    /// A pending request was cancelled without approval.
    RequestCancelled,
}

pub struct YoloModeManager {
    state: Mutex<YoloState>,
    /// Broadcast channel — UI windows subscribe to receive real-time events.
    events: broadcast::Sender<YoloEvent>,
}

impl YoloModeManager {
    /// Create the manager.
    ///
    /// **Does not spawn the expiry watcher** — call `start_expiry_watcher()` from
    /// inside an async context (Tauri setup uses `tauri::async_runtime::spawn`,
    /// headless mode uses `tokio::spawn`) after a Tokio reactor is running.
    pub fn new(config: YoloModeConfig) -> Arc<Self> {
        let (events, _) = broadcast::channel(32);
        Arc::new(Self {
            state: Mutex::new(YoloState::new(config)),
            events,
        })
    }

    /// Start the background auto-expiry watcher.  Must be called once from within
    /// a live Tokio runtime (e.g. from a `tauri::async_runtime::spawn` closure).
    pub fn start_expiry_watcher(self: &Arc<Self>) {
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                let mut state = mgr.state.lock().await;
                if state.check_expiry() {
                    warn!("[YOLO] Yolo mode expired automatically");
                    let _ = mgr.events.send(YoloEvent::Expired);
                }
            }
        });
    }

    // -----------------------------------------------------------------------
    // Model-facing API
    // -----------------------------------------------------------------------

    /// Request yolo mode on behalf of the model.  Returns an error string if
    /// requests are disabled or one is already pending.
    pub async fn request(
        &self,
        duration_secs: Option<u64>,
        reason: Option<String>,
    ) -> Result<YoloRequest, String> {
        let mut state = self.state.lock().await;

        if !state.config.allow_model_request {
            return Err("Yolo mode requests from the model are disabled in config \
                 (yolo_mode.allow_model_request = false)"
                .to_string());
        }

        if state.active {
            return Err("Yolo mode is already active.".to_string());
        }

        if state.pending_request.is_some() {
            return Err("A yolo mode request is already pending. \
                 Ask the user to approve or reject it first."
                .to_string());
        }

        // Effective duration: explicit arg > config default > None (unlimited).
        let effective_duration = duration_secs.or(state.config.default_duration_secs);

        let req = YoloRequest {
            id: Uuid::new_v4().to_string(),
            requested_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            duration_secs: effective_duration,
            reason,
        };

        state.pending_request = Some(req.clone());
        let _ = self.events.send(YoloEvent::RequestPending {
            request: req.clone(),
        });
        info!(
            "[YOLO] Model requested yolo mode (id={}, duration={:?})",
            req.id, req.duration_secs
        );
        Ok(req)
    }

    // -----------------------------------------------------------------------
    // Human-facing API (UI only — never reachable from model tool calls)
    // -----------------------------------------------------------------------

    /// Directly activate yolo mode without a pending request.
    ///
    /// Used when a trusted human channel (e.g. Telegram `/yolo` command,
    /// verified DM) directly authorizes elevation without going through the
    /// request/approve flow.  This is still out-of-band from the model.
    pub async fn direct_activate(&self, duration_secs: Option<u64>) -> YoloStatus {
        let mut state = self.state.lock().await;

        // Effective duration: explicit arg > config default > None (unlimited).
        let effective_duration = duration_secs.or(state.config.default_duration_secs);

        let now = SystemTime::now();
        state.active = true;
        state.approved_at = Some(now);
        state.expires_at = effective_duration.map(|secs| now + Duration::from_secs(secs));
        state.pending_request = None; // clear any pending request

        let expires_at_ms = state
            .expires_at
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64);

        info!(
            "[YOLO] Direct activation (duration={:?}, expires_at_ms={:?})",
            effective_duration, expires_at_ms
        );
        let _ = self.events.send(YoloEvent::Approved { expires_at_ms });
        state.status()
    }

    /// Approve a pending request.  Only callable from the Tauri UI layer.
    pub async fn approve(&self, request_id: &str) -> Result<YoloStatus, String> {
        let mut state = self.state.lock().await;

        let req = state
            .pending_request
            .take()
            .ok_or_else(|| "No pending yolo mode request to approve.".to_string())?;

        if req.id != request_id {
            // Capture id before moving req back into state.
            let expected_id = req.id.clone();
            state.pending_request = Some(req);
            return Err(format!(
                "Request ID mismatch. Expected '{}', got '{}'.",
                expected_id,
                request_id
            ));
        }

        let now = SystemTime::now();
        state.active = true;
        state.approved_at = Some(now);
        state.expires_at = req
            .duration_secs
            .map(|secs| now + Duration::from_secs(secs));

        let expires_at_ms = state
            .expires_at
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64);

        info!(
            "[YOLO] Approved (id={}, expires_at_ms={:?})",
            req.id, expires_at_ms
        );
        let _ = self.events.send(YoloEvent::Approved { expires_at_ms });

        Ok(state.status())
    }

    /// Revoke yolo mode immediately.
    pub async fn revoke(&self) -> YoloStatus {
        let mut state = self.state.lock().await;
        let was_active = state.active;
        state.active = false;
        state.approved_at = None;
        state.expires_at = None;
        state.pending_request = None;
        if was_active {
            warn!("[YOLO] Yolo mode revoked by user");
            let _ = self.events.send(YoloEvent::Revoked);
        } else {
            info!("[YOLO] Revoke called but yolo mode was not active; clearing pending request");
            let _ = self.events.send(YoloEvent::RequestCancelled);
        }
        state.status()
    }

    // -----------------------------------------------------------------------
    // Read-only access
    // -----------------------------------------------------------------------

    /// Returns true if yolo mode is currently active (and not expired).
    pub async fn is_active(&self) -> bool {
        let mut state = self.state.lock().await;
        state.check_expiry(); // keep in-sync without waiting for the watcher
        state.active
    }

    /// Full status snapshot.
    pub async fn status(&self) -> YoloStatus {
        let mut state = self.state.lock().await;
        state.check_expiry();
        state.status()
    }

    /// Subscribe to live events (UI windows).
    pub fn subscribe(&self) -> broadcast::Receiver<YoloEvent> {
        self.events.subscribe()
    }

    // -----------------------------------------------------------------------
    // Config hotloading
    // -----------------------------------------------------------------------

    pub async fn update_config(&self, config: YoloModeConfig) {
        let mut state = self.state.lock().await;
        state.config = config;
        info!("[YOLO] Config updated via hotloading");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn permissive_config() -> YoloModeConfig {
        YoloModeConfig {
            default_duration_secs: Some(60),
            allow_model_request: true,
        }
    }

    fn restricted_config() -> YoloModeConfig {
        YoloModeConfig {
            default_duration_secs: Some(60),
            allow_model_request: false,
        }
    }

    // -----------------------------------------------------------------------
    // request()
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_request_creates_pending_state() {
        let mgr = YoloModeManager::new(permissive_config());
        let req = mgr
            .request(Some(30), Some("need to edit config".to_string()))
            .await
            .unwrap();

        assert!(!req.id.is_empty(), "Request ID must not be empty");
        assert_eq!(req.duration_secs, Some(30));
        assert_eq!(req.reason.as_deref(), Some("need to edit config"));

        let status = mgr.status().await;
        assert!(!status.active, "Should not be active yet");
        let pending = status
            .pending_request
            .expect("Should have a pending request");
        assert_eq!(pending.id, req.id);
    }

    #[tokio::test]
    async fn test_request_uses_config_default_duration_when_none() {
        let mgr = YoloModeManager::new(permissive_config()); // default_duration_secs = 60
        let req = mgr.request(None, None).await.unwrap();
        assert_eq!(req.duration_secs, Some(60));
    }

    #[tokio::test]
    async fn test_request_explicit_duration_overrides_config_default() {
        let mgr = YoloModeManager::new(permissive_config()); // config default = 60
        let req = mgr.request(Some(120), None).await.unwrap();
        assert_eq!(req.duration_secs, Some(120));
    }

    #[tokio::test]
    async fn test_request_blocked_when_allow_model_request_false() {
        let mgr = YoloModeManager::new(restricted_config());
        let result = mgr.request(None, None).await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("allow_model_request"),
            "Error message should mention the config key; got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_request_blocked_when_already_active() {
        let mgr = YoloModeManager::new(permissive_config());
        mgr.direct_activate(Some(30)).await;
        let result = mgr.request(None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already active"));
    }

    #[tokio::test]
    async fn test_request_blocked_when_already_pending() {
        let mgr = YoloModeManager::new(permissive_config());
        mgr.request(None, Some("first".into())).await.unwrap();
        let result = mgr.request(None, Some("second".into())).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already pending"));
    }

    // -----------------------------------------------------------------------
    // approve()
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_approve_activates_yolo_mode() {
        let mgr = YoloModeManager::new(permissive_config());
        let req = mgr.request(Some(30), None).await.unwrap();
        let status = mgr.approve(&req.id).await.unwrap();

        assert!(status.active);
        assert!(status.expires_at_ms.is_some(), "Should have an expiry");
        assert!(mgr.is_active().await);

        // Pending request must be cleared after approval
        assert!(mgr.status().await.pending_request.is_none());
    }

    #[tokio::test]
    async fn test_approve_with_wrong_id_returns_error() {
        let mgr = YoloModeManager::new(permissive_config());
        mgr.request(None, None).await.unwrap();
        let result = mgr.approve("wrong-id-00000000").await;

        assert!(result.is_err(), "Should fail with mismatched ID");
        // Pending request must still be in place after a failed approval
        assert!(mgr.status().await.pending_request.is_some());
    }

    #[tokio::test]
    async fn test_approve_with_no_pending_request_returns_error() {
        let mgr = YoloModeManager::new(permissive_config());
        let result = mgr.approve("any-id").await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // direct_activate()
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_direct_activate_sets_active_with_expiry() {
        let mgr = YoloModeManager::new(permissive_config());
        let status = mgr.direct_activate(Some(60)).await;

        assert!(status.active);
        assert!(status.expires_at_ms.is_some());
        assert!(mgr.is_active().await);
    }

    #[tokio::test]
    async fn test_direct_activate_clears_pending_request() {
        let mgr = YoloModeManager::new(permissive_config());
        mgr.request(None, None).await.unwrap();
        mgr.direct_activate(Some(30)).await;

        let status = mgr.status().await;
        assert!(status.active);
        assert!(
            status.pending_request.is_none(),
            "Pending request should be cleared"
        );
    }

    #[tokio::test]
    async fn test_direct_activate_with_no_duration_is_unlimited() {
        let cfg = YoloModeConfig {
            default_duration_secs: None,
            allow_model_request: true,
        };
        let mgr = YoloModeManager::new(cfg);
        let status = mgr.direct_activate(None).await;

        assert!(status.active);
        assert!(
            status.expires_at_ms.is_none(),
            "No expiry for unlimited session"
        );
        assert!(status.remaining_secs.is_none());
    }

    // -----------------------------------------------------------------------
    // revoke()
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_revoke_deactivates_active_session() {
        let mgr = YoloModeManager::new(permissive_config());
        mgr.direct_activate(Some(60)).await;
        assert!(mgr.is_active().await);

        mgr.revoke().await;
        assert!(!mgr.is_active().await);
    }

    #[tokio::test]
    async fn test_revoke_clears_pending_request() {
        let mgr = YoloModeManager::new(permissive_config());
        mgr.request(None, None).await.unwrap();
        assert!(mgr.status().await.pending_request.is_some());

        mgr.revoke().await;
        assert!(mgr.status().await.pending_request.is_none());
    }

    #[tokio::test]
    async fn test_revoke_when_already_inactive_is_safe() {
        let mgr = YoloModeManager::new(permissive_config());
        let status = mgr.revoke().await; // Should not panic
        assert!(!status.active);
    }

    // -----------------------------------------------------------------------
    // Auto-expiry via is_active() inline check
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_expired_session_is_not_active() {
        let mgr = YoloModeManager::new(permissive_config());
        mgr.direct_activate(Some(0)).await; // 0-second session = instant expiry

        // Allow at least 1ms to pass so the expiry instant is definitely in the past.
        tokio::time::sleep(Duration::from_millis(5)).await;

        // is_active() calls check_expiry() inline.
        assert!(
            !mgr.is_active().await,
            "Zero-duration session should have expired"
        );
    }

    // -----------------------------------------------------------------------
    // Broadcast events
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_request_broadcasts_request_pending_event() {
        let mgr = YoloModeManager::new(permissive_config());
        let mut rx = mgr.subscribe();

        mgr.request(None, Some("broadcast test".into()))
            .await
            .unwrap();

        let event = rx.try_recv().expect("Should have received an event");
        assert!(
            matches!(event, YoloEvent::RequestPending { .. }),
            "Expected RequestPending event"
        );
    }

    #[tokio::test]
    async fn test_approve_broadcasts_approved_event() {
        let mgr = YoloModeManager::new(permissive_config());
        let req = mgr.request(None, None).await.unwrap();
        let mut rx = mgr.subscribe();

        mgr.approve(&req.id).await.unwrap();

        let event = rx.try_recv().expect("Should have received an event");
        assert!(matches!(event, YoloEvent::Approved { .. }));
    }

    #[tokio::test]
    async fn test_revoke_active_broadcasts_revoked_event() {
        let mgr = YoloModeManager::new(permissive_config());
        mgr.direct_activate(Some(60)).await;
        let mut rx = mgr.subscribe();

        mgr.revoke().await;

        let event = rx.try_recv().expect("Should have received an event");
        assert!(matches!(event, YoloEvent::Revoked));
    }

    #[tokio::test]
    async fn test_revoke_inactive_broadcasts_request_cancelled_event() {
        let mgr = YoloModeManager::new(permissive_config());
        let mut rx = mgr.subscribe();

        mgr.revoke().await;

        let event = rx.try_recv().expect("Should have received an event");
        assert!(matches!(event, YoloEvent::RequestCancelled));
    }

    // -----------------------------------------------------------------------
    // update_config() hotloading
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_update_config_disables_model_requests() {
        let mgr = YoloModeManager::new(permissive_config());

        // Works before the config update.
        mgr.request(None, Some("before".into())).await.unwrap();
        mgr.revoke().await; // clear the pending request

        // Now disable model requests.
        mgr.update_config(restricted_config()).await;

        let result = mgr.request(None, None).await;
        assert!(result.is_err(), "Should be blocked after config update");
    }
}
