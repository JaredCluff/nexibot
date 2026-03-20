///! Heartbeat System (OpenClaw-inspired)
///!
///! Provides periodic autonomy:
///! - Wake up at configured intervals
///! - Check for pending tasks or notifications
///! - Perform proactive actions
///! - User-configurable schedule
use anyhow::Result;
use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::dag::store::DagStore;
use crate::notifications::NotificationDispatcher;

/// Heartbeat configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    /// Whether heartbeat is enabled
    pub enabled: bool,
    /// Interval between heartbeats in seconds
    pub interval_seconds: u64,
    /// Whether to run proactive actions
    pub proactive_actions: bool,
    /// Time of day constraints (24-hour format)
    pub active_hours: Option<ActiveHours>,
}

/// Time constraints for when heartbeat should be active
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveHours {
    /// Start hour (0-23)
    pub start_hour: u8,
    /// End hour (0-23)
    pub end_hour: u8,
}

/// Minimum allowed heartbeat interval: 1 second.
const MIN_INTERVAL_SECONDS: u64 = 1;
/// Maximum allowed heartbeat interval: 24 hours (86400 seconds).
const MAX_INTERVAL_SECONDS: u64 = 86_400;
/// Default heartbeat interval: 5 minutes (300 seconds).
const DEFAULT_INTERVAL_SECONDS: u64 = 300;

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default for safety
            interval_seconds: DEFAULT_INTERVAL_SECONDS,
            proactive_actions: false,
            active_hours: None, // Always active if None
        }
    }
}

impl HeartbeatConfig {
    /// Load heartbeat configuration from a file path.
    ///
    /// If the file does not exist or cannot be parsed, returns the default
    /// configuration with a warning log instead of failing.
    #[allow(dead_code)]
    pub fn load_from_file(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str::<HeartbeatConfig>(&contents) {
                Ok(mut config) => {
                    config.validate_and_clamp();
                    info!("[HEARTBEAT] Loaded config from {}", path.display());
                    config
                }
                Err(e) => {
                    warn!(
                        "[HEARTBEAT] Failed to parse config from {} ({}), using defaults",
                        path.display(),
                        e
                    );
                    Self::default()
                }
            },
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    info!(
                        "[HEARTBEAT] Config file not found at {}, using defaults",
                        path.display()
                    );
                } else {
                    warn!(
                        "[HEARTBEAT] Could not read config from {} ({}), using defaults",
                        path.display(),
                        e
                    );
                }
                Self::default()
            }
        }
    }

    /// Validate and clamp configuration values to safe ranges.
    ///
    /// - `interval_seconds` is clamped to [1, 86400] (1 second to 24 hours).
    /// - Invalid values are corrected with a warning rather than causing errors.
    pub fn validate_and_clamp(&mut self) {
        if self.interval_seconds < MIN_INTERVAL_SECONDS
            || self.interval_seconds > MAX_INTERVAL_SECONDS
        {
            let original = self.interval_seconds;
            self.interval_seconds = self
                .interval_seconds
                .max(MIN_INTERVAL_SECONDS)
                .min(MAX_INTERVAL_SECONDS);
            warn!(
                "[HEARTBEAT] Interval {}s out of range [{}, {}], clamped to {}s",
                original, MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS, self.interval_seconds
            );
        }
    }
}

/// Action to perform during heartbeat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HeartbeatAction {
    /// Check for pending tasks
    CheckTasks,
    /// Check for notifications
    CheckNotifications,
    /// Perform maintenance
    Maintenance,
    /// Custom action with description
    Custom(String),
}

/// Result of a heartbeat check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResult {
    /// When the heartbeat occurred
    pub timestamp: DateTime<Utc>,
    /// Actions performed
    pub actions_performed: Vec<HeartbeatAction>,
    /// Any messages or notifications
    pub messages: Vec<String>,
    /// Whether any actions were taken
    pub actions_taken: bool,
}

/// Heartbeat manager
pub struct HeartbeatManager {
    config: Arc<RwLock<HeartbeatConfig>>,
    is_running: Arc<RwLock<bool>>,
    last_heartbeat: Arc<RwLock<Option<DateTime<Utc>>>>,
    /// Injected after AppState is constructed (deferred to break the init cycle).
    services: Arc<RwLock<Option<HeartbeatServices>>>,
}

/// Services needed by the heartbeat catch-up scan.
#[derive(Clone)]
struct HeartbeatServices {
    dag_store: Arc<Mutex<DagStore>>,
    notification_dispatcher: Arc<NotificationDispatcher>,
}

impl HeartbeatManager {
    /// Create a new heartbeat manager
    pub fn new(config: HeartbeatConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            is_running: Arc::new(RwLock::new(false)),
            last_heartbeat: Arc::new(RwLock::new(None)),
            services: Arc::new(RwLock::new(None)),
        }
    }

    /// Inject services after AppState is fully constructed.
    /// Must be called before `start()` to enable catch-up notifications.
    pub async fn set_services(
        &self,
        dag_store: Arc<Mutex<DagStore>>,
        notification_dispatcher: Arc<NotificationDispatcher>,
    ) {
        *self.services.write().await = Some(HeartbeatServices {
            dag_store,
            notification_dispatcher,
        });
        info!("[HEARTBEAT] Services injected — catch-up scan enabled");
    }

    /// Start the heartbeat loop
    pub async fn start(&self) -> Result<()> {
        let mut config = self.config.write().await;
        if !config.enabled {
            info!("[HEARTBEAT] Heartbeat is disabled, not starting");
            return Ok(());
        }

        // Validate and clamp the interval before starting
        config.validate_and_clamp();
        let interval = config.interval_seconds;
        drop(config);

        let mut is_running = self.is_running.write().await;
        if *is_running {
            warn!("[HEARTBEAT] Heartbeat already running");
            return Ok(());
        }

        *is_running = true;
        drop(is_running);

        info!(
            "[HEARTBEAT] Starting heartbeat loop (interval: {}s)",
            interval
        );

        // Spawn the heartbeat loop in the background
        let config_clone = self.config.clone();
        let is_running_clone = self.is_running.clone();
        let last_heartbeat_clone = self.last_heartbeat.clone();
        let services_clone = self.services.clone();

        tokio::spawn(async move {
            // Don't let individual heartbeat failures stop the loop
            loop {
                // Check if still running
                if !*is_running_clone.read().await {
                    info!("[HEARTBEAT] Heartbeat loop stopped");
                    break;
                }

                // Get current config (validate interval each iteration in case it changed)
                let config = config_clone.read().await;
                let interval = config
                    .interval_seconds
                    .max(MIN_INTERVAL_SECONDS)
                    .min(MAX_INTERVAL_SECONDS);
                let active_hours = config.active_hours.clone();
                drop(config);

                // Check if we should run based on active hours
                if !Self::is_active_time(&active_hours) {
                    debug!("[HEARTBEAT] Outside active hours, skipping");
                    sleep(Duration::from_secs(60)).await; // Check again in 1 minute
                    continue;
                }

                // Read services for this tick (Option; may not be injected yet)
                let services = services_clone.read().await.clone();

                // Perform heartbeat -- never let a failure stop the loop
                match Self::perform_heartbeat(services.as_ref()).await {
                    Ok(result) => {
                        if result.actions_taken {
                            info!(
                                "[HEARTBEAT] Heartbeat completed: {} actions performed",
                                result.actions_performed.len()
                            );
                        } else {
                            debug!("[HEARTBEAT] Tick successful");
                        }

                        // Update last heartbeat time
                        *last_heartbeat_clone.write().await = Some(result.timestamp);
                    }
                    Err(e) => {
                        warn!("[HEARTBEAT] Tick failed (will retry): {}", e);
                        // Continue the loop -- do not break or return
                    }
                }

                // Sleep until next heartbeat
                sleep(Duration::from_secs(interval)).await;
            }
        });

        Ok(())
    }

    /// Stop the heartbeat loop
    pub async fn stop(&self) -> Result<()> {
        let mut is_running = self.is_running.write().await;
        *is_running = false;
        info!("[HEARTBEAT] Stopping heartbeat loop");
        Ok(())
    }

    /// Check if current time is within active hours
    fn is_active_time(active_hours: &Option<ActiveHours>) -> bool {
        match active_hours {
            None => true, // Always active if no constraints
            Some(hours) => {
                let now = Utc::now();
                let current_hour = now.time().hour() as u8;

                if hours.start_hour <= hours.end_hour {
                    // Normal case: start < end (e.g., 9-17)
                    current_hour >= hours.start_hour && current_hour < hours.end_hour
                } else {
                    // Wrap around midnight case (e.g., 22-6)
                    current_hour >= hours.start_hour || current_hour < hours.end_hour
                }
            }
        }
    }

    /// Perform a single heartbeat check.
    /// `services` is `None` when the heartbeat starts before services are injected.
    async fn perform_heartbeat(services: Option<&HeartbeatServices>) -> Result<HeartbeatResult> {
        let mut actions_performed = Vec::new();
        let mut messages = Vec::new();
        let mut actions_taken = false;

        debug!("[HEARTBEAT] Tick — scanning for catch-up notifications");

        // Catch-up: send notifications for DAG runs that completed while we were
        // offline (notification_sent = 0 but status = completed/failed).
        if let Some(svc) = services {
            let unsent = {
                let store = svc.dag_store.lock().unwrap_or_else(|e| e.into_inner());
                match store.get_unsent_completed_runs() {
                    Ok(rows) => rows,
                    Err(e) => {
                        warn!(
                            "[HEARTBEAT] Failed to query unsent DAG notifications: {}",
                            e
                        );
                        Vec::new()
                    }
                }
            };

            if !unsent.is_empty() {
                info!(
                    "[HEARTBEAT] Found {} DAG run(s) with unsent notifications",
                    unsent.len()
                );
            }

            for (run_id, run_name, status) in &unsent {
                let msg = if status == "failed" {
                    format!(
                        "❌ DAG run `{}` ({}) failed (catch-up notification)",
                        &run_id[..8.min(run_id.len())],
                        run_name
                    )
                } else {
                    format!(
                        "✅ DAG run `{}` ({}) completed (catch-up notification)",
                        &run_id[..8.min(run_id.len())],
                        run_name
                    )
                };

                let attempted = svc.notification_dispatcher.broadcast(&msg).await;
                if attempted {
                    let store = svc.dag_store.lock().unwrap_or_else(|e| e.into_inner());
                    match store.mark_notification_sent(run_id) {
                        Ok(()) => {
                            actions_performed.push(HeartbeatAction::CheckNotifications);
                            messages.push(msg);
                            actions_taken = true;
                        }
                        Err(e) => {
                            warn!(
                                "[HEARTBEAT] Delivered catch-up notification for run {} but failed to mark as sent: {}",
                                run_id, e
                            );
                        }
                    }
                } else {
                    warn!(
                        "[HEARTBEAT] No notification delivery targets available for run {} ({}); leaving as unsent",
                        run_id, run_name
                    );
                }
            }
        }

        Ok(HeartbeatResult {
            timestamp: Utc::now(),
            actions_performed,
            messages,
            actions_taken,
        })
    }

    /// Get current heartbeat configuration
    pub async fn get_config(&self) -> HeartbeatConfig {
        self.config.read().await.clone()
    }

    /// Update heartbeat configuration
    pub async fn update_config(&self, new_config: HeartbeatConfig) -> Result<()> {
        let mut validated_config = new_config.clone();
        validated_config.validate_and_clamp();

        let mut config = self.config.write().await;
        let was_enabled = config.enabled;
        *config = validated_config.clone();

        info!(
            "[HEARTBEAT] Configuration updated: enabled={}, interval={}s",
            validated_config.enabled, validated_config.interval_seconds
        );

        // If enabled state changed, restart the loop
        if was_enabled != validated_config.enabled {
            drop(config); // Release the lock before restarting
            if validated_config.enabled {
                self.start().await?;
            } else {
                self.stop().await?;
            }
        }

        Ok(())
    }

    /// Check if heartbeat is currently running
    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }

    /// Get the timestamp of the last heartbeat
    #[allow(dead_code)]
    pub async fn get_last_heartbeat(&self) -> Option<DateTime<Utc>> {
        *self.last_heartbeat.read().await
    }

    /// Manually trigger a heartbeat (for testing)
    pub async fn trigger_now(&self) -> Result<HeartbeatResult> {
        info!("[HEARTBEAT] Manual heartbeat triggered");
        let services = self.services.read().await.clone();
        Self::perform_heartbeat(services.as_ref()).await
    }
}
