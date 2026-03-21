//! Context window management with auto-compaction state machine.
//!
//! Manages conversation context lifecycle:
//! - Tracks token usage as percentage of context window
//! - Implements state machine (Normal → Approaching → PreCompactFlush → Compact)
//! - Triggers compaction at configurable thresholds
//! - Pre-compaction memory flush extracts important info before compacting
//! - Archives summaries to dated memory files
//! - Provides observability hooks for monitoring

use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Context window usage states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextState {
    /// 0-70%: Normal operation, no action needed.
    Normal,
    /// 70-85%: Approaching limit, monitor closely.
    Approaching,
    /// Pre-compaction: flushing important memories before compaction.
    PreCompactFlush,
    /// 85%+: At capacity, trigger compaction immediately.
    Compact,
}

/// Configuration for context management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextManagerConfig {
    /// Enable context auto-compaction.
    pub enabled: bool,
    /// Trigger compaction at this threshold (default: 0.85 = 85%).
    pub compaction_threshold: f64,
    /// Enter "Approaching" state at this threshold (default: 0.70 = 70%).
    pub approaching_threshold: f64,
    /// Number of recent messages to preserve after compaction (default: 20).
    pub preserve_recent_messages: usize,
    /// Archive summaries to dated memory files (default: true).
    pub archive_summaries: bool,
    /// Maximum compaction operations per day before warning (default: 5).
    pub max_compactions_per_day: u32,
    /// Maximum JSONL transcript entries to keep after truncation.
    #[serde(default = "default_max_transcript_entries")]
    pub max_transcript_entries: usize,
    /// Whether to truncate transcripts after compaction.
    #[serde(default = "default_true")]
    pub truncate_after_compaction: bool,
    /// Enable pre-compaction memory flush (extract important info before compacting).
    #[serde(default = "default_true")]
    pub pre_compaction_flush: bool,
    /// Number of recent messages to send to LLM for memory extraction.
    #[serde(default = "default_flush_message_window")]
    pub flush_message_window: usize,
    /// Maximum time in seconds to wait for the flush LLM call.
    #[serde(default = "default_flush_timeout")]
    pub flush_timeout_seconds: u64,
    /// Optional model override for the flush call (use cheaper model).
    #[serde(default)]
    pub flush_model: Option<String>,
}

fn default_max_transcript_entries() -> usize {
    200
}

fn default_true() -> bool {
    true
}

fn default_flush_message_window() -> usize {
    50
}

fn default_flush_timeout() -> u64 {
    30
}

impl Default for ContextManagerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            compaction_threshold: 0.85,
            approaching_threshold: 0.70,
            preserve_recent_messages: 20,
            archive_summaries: true,
            max_compactions_per_day: 5,
            max_transcript_entries: default_max_transcript_entries(),
            truncate_after_compaction: true,
            pre_compaction_flush: true,
            flush_message_window: default_flush_message_window(),
            flush_timeout_seconds: default_flush_timeout(),
            flush_model: None,
        }
    }
}

/// Metrics for context compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionMetrics {
    /// Total number of compaction operations.
    pub total_compactions: u32,
    /// Number of compactions today.
    pub compactions_today: u32,
    /// Total messages removed across all compactions.
    pub total_messages_removed: u32,
    /// Total tokens freed across all compactions.
    pub total_tokens_freed: u32,
    /// Average compression ratio (messages_after / messages_before).
    pub avg_compression_ratio: f32,
    /// Timestamp of last compaction.
    pub last_compaction_time: Option<DateTime<Utc>>,
}

/// Context usage snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextUsage {
    /// Current token count.
    pub tokens: usize,
    /// Total context window size.
    pub window_size: usize,
    /// Usage as percentage (0-100).
    pub usage_percent: f64,
    /// Current state (Normal/Approaching/Compact).
    pub state: ContextState,
    /// Timestamp of this measurement.
    pub timestamp: DateTime<Utc>,
}

/// Compaction event record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionEvent {
    /// When this compaction occurred.
    pub timestamp: DateTime<Utc>,
    /// Messages before compaction.
    pub messages_before: usize,
    /// Messages after compaction.
    pub messages_after: usize,
    /// Tokens freed by this compaction.
    pub tokens_freed: u32,
    /// Summary text (first 200 chars for archival).
    pub summary_preview: String,
}

/// Configuration for the pre-compaction memory flush.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlushConfig {
    /// Number of recent messages to analyze.
    pub message_window: usize,
    /// Maximum seconds to wait for the LLM call.
    pub timeout_seconds: u64,
    /// Optional model override for the flush call.
    pub model: Option<String>,
}

/// Context manager for conversation lifecycle.
pub struct ContextManager {
    config: std::sync::Mutex<ContextManagerConfig>,
    current_state: std::sync::Arc<std::sync::Mutex<ContextState>>,
    metrics: Arc<std::sync::Mutex<CompactionMetrics>>,
    compactions_today: Arc<AtomicU32>,
    last_reset_date: Arc<std::sync::Mutex<chrono::NaiveDate>>,
    compaction_history: Arc<std::sync::Mutex<Vec<CompactionEvent>>>,
}

impl ContextManager {
    /// Create a new context manager.
    pub fn new(config: ContextManagerConfig) -> Self {
        Self {
            config: std::sync::Mutex::new(config),
            current_state: Arc::new(std::sync::Mutex::new(ContextState::Normal)),
            metrics: Arc::new(std::sync::Mutex::new(CompactionMetrics {
                total_compactions: 0,
                compactions_today: 0,
                total_messages_removed: 0,
                total_tokens_freed: 0,
                avg_compression_ratio: 1.0,
                last_compaction_time: None,
            })),
            compactions_today: Arc::new(AtomicU32::new(0)),
            last_reset_date: Arc::new(std::sync::Mutex::new(Local::now().naive_local().date())),
            compaction_history: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Update context usage and return the new state.
    /// Should be called after each message to track usage.
    pub fn update_usage(&self, tokens: usize, window_size: usize) -> Result<ContextUsage> {
        let usage_percent = (tokens as f64 / window_size as f64) * 100.0;

        let cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
        let new_state = if usage_percent >= (cfg.compaction_threshold * 100.0) {
            ContextState::Compact
        } else if usage_percent >= (cfg.approaching_threshold * 100.0) {
            ContextState::Approaching
        } else {
            ContextState::Normal
        };
        drop(cfg);

        // Update state
        {
            let mut state = self.current_state.lock().unwrap_or_else(|e| e.into_inner());
            let old_state = *state;
            *state = new_state;

            if old_state != new_state {
                debug!(
                    "[CONTEXT] State transition: {:?} -> {:?} ({}%)",
                    old_state, new_state, usage_percent as u32
                );
            }
        }

        Ok(ContextUsage {
            tokens,
            window_size,
            usage_percent,
            state: new_state,
            timestamp: Utc::now(),
        })
    }

    /// Check if compaction should be triggered.
    pub fn should_compact(&self) -> bool {
        let cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
        if !cfg.enabled {
            return false;
        }
        drop(cfg);

        let state = *self.current_state.lock().unwrap_or_else(|e| e.into_inner());
        state == ContextState::Compact
    }

    /// Get truncation config for use after compaction.
    pub fn truncation_config(&self) -> Option<(usize, bool)> {
        let cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
        if cfg.truncate_after_compaction {
            Some((cfg.max_transcript_entries, true))
        } else {
            None
        }
    }

    /// Get the flush configuration for pre-compaction memory extraction.
    pub fn flush_config(&self) -> Option<FlushConfig> {
        let cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
        if cfg.pre_compaction_flush {
            Some(FlushConfig {
                message_window: cfg.flush_message_window,
                timeout_seconds: cfg.flush_timeout_seconds,
                model: cfg.flush_model.clone(),
            })
        } else {
            None
        }
    }

    /// Mark that flush is in progress (prevents re-triggering).
    pub fn set_flush_in_progress(&self) {
        let mut state = self.current_state.lock().unwrap_or_else(|e| e.into_inner());
        if *state == ContextState::Approaching || *state == ContextState::Compact {
            debug!("[CONTEXT] Entering PreCompactFlush state");
            *state = ContextState::PreCompactFlush;
        }
    }

    /// Mark that flush is complete, transition to Compact.
    pub fn set_flush_complete(&self) {
        let mut state = self.current_state.lock().unwrap_or_else(|e| e.into_inner());
        if *state == ContextState::PreCompactFlush {
            debug!("[CONTEXT] Flush complete, transitioning to Compact");
            *state = ContextState::Compact;
        }
    }

    /// Record a successful compaction event.
    pub fn record_compaction(
        &self,
        messages_before: usize,
        messages_after: usize,
        tokens_before: usize,
        tokens_after: usize,
        summary: &str,
    ) -> Result<()> {
        let tokens_freed = (tokens_before as i32 - tokens_after as i32).max(0) as u32;

        let event = CompactionEvent {
            timestamp: Utc::now(),
            messages_before,
            messages_after,
            tokens_freed,
            summary_preview: summary.chars().take(200).collect(),
        };

        // Record event
        {
            let mut history = self
                .compaction_history
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            history.push(event);
            // Keep only last 100 events in memory
            if history.len() > 100 {
                history.remove(0);
            }
        }

        // Update metrics
        {
            let mut metrics = self.metrics.lock().unwrap_or_else(|e| e.into_inner());
            metrics.total_compactions += 1;
            metrics.compactions_today += 1;
            metrics.total_messages_removed += (messages_before - messages_after) as u32;
            metrics.total_tokens_freed += tokens_freed;

            if metrics.total_compactions > 0 {
                let total_before: u64 = self
                    .compaction_history
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|e| e.messages_before as u64)
                    .sum();
                let total_after: u64 = self
                    .compaction_history
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|e| e.messages_after as u64)
                    .sum();

                if total_before > 0 {
                    metrics.avg_compression_ratio = (total_after as f32) / (total_before as f32);
                }
            }

            metrics.last_compaction_time = Some(Utc::now());
        }

        // Sync the AtomicU32 to the metrics value rather than double-incrementing.
        // metrics.compactions_today was already incremented inside the metrics lock
        // above.  The atomic mirror is used only for lock-free reads (exceeds_daily_limit).
        let metrics_today = self
            .metrics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .compactions_today;
        self.compactions_today
            .store(metrics_today, Ordering::SeqCst);
        let (max_per_day, should_archive) = {
            let cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
            (cfg.max_compactions_per_day, cfg.archive_summaries)
        };
        if metrics_today > max_per_day {
            warn!(
                "[CONTEXT] {} compactions today (exceeds limit of {})",
                metrics_today, max_per_day
            );
        }

        // Archive summary to dated memory file when configured to do so.
        if should_archive {
            let archive_block = self.archive_summary(summary, "context-manager");
            if let Some(home) = dirs::home_dir() {
                let date_str = chrono::Local::now().format("%Y-%m-%d").to_string();
                let memory_dir = home.join(".config/nexibot/memory");
                let _ = std::fs::create_dir_all(&memory_dir);
                let memory_file = memory_dir.join(format!("{}.md", date_str));
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&memory_file)
                {
                    let _ = writeln!(file, "\n{}", archive_block);
                }
            }
        }

        info!(
            "[CONTEXT] Compaction recorded: {} -> {} messages, {} tokens freed",
            messages_before, messages_after, tokens_freed
        );

        Ok(())
    }

    /// Get current metrics.
    pub fn get_metrics(&self) -> CompactionMetrics {
        self.metrics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Get current state.
    pub fn get_state(&self) -> ContextState {
        *self.current_state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Get compaction history (last N events).
    pub fn get_compaction_history(&self, limit: usize) -> Vec<CompactionEvent> {
        let history = self
            .compaction_history
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        history.iter().rev().take(limit).cloned().collect()
    }

    /// Reset daily counter (should be called once per day).
    pub fn reset_daily_counter(&self) -> Result<()> {
        let mut last_reset = self
            .last_reset_date
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let today = Local::now().naive_local().date();

        if today != *last_reset {
            self.compactions_today.store(0, Ordering::SeqCst);
            *last_reset = today;
            debug!("[CONTEXT] Daily counter reset");
        }

        Ok(())
    }

    /// Return whether daily compaction limit exceeded.
    #[allow(dead_code)]
    pub fn exceeds_daily_limit(&self) -> bool {
        let cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
        self.compactions_today.load(Ordering::SeqCst) > cfg.max_compactions_per_day
    }

    /// Hot-reload the context manager configuration.
    pub fn update_config(&self, new_config: ContextManagerConfig) {
        info!(
            "[CONTEXT] Config updated: enabled={}, threshold={}, flush={}",
            new_config.enabled, new_config.compaction_threshold, new_config.pre_compaction_flush
        );
        let mut cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
        *cfg = new_config;
    }

    /// Generate archival summary for memory files.
    /// Returns a markdown block suitable for dated memory files (YYYY-MM-DD.md).
    pub fn archive_summary(&self, summary_text: &str, session_id: &str) -> String {
        let now = Local::now();
        let date_str = now.format("%Y-%m-%d").to_string();
        let time_str = now.format("%H:%M:%S").to_string();

        format!(
            "## Compaction Summary — {} at {}\n\n\
             **Session:** `{}`\n\n\
             {}\n",
            date_str, time_str, session_id, summary_text
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions() {
        let manager = ContextManager::new(ContextManagerConfig::default());

        // Normal state (50% usage)
        let usage = manager.update_usage(10000, 200000).unwrap();
        assert_eq!(usage.state, ContextState::Normal);
        assert_eq!(usage.usage_percent as u32, 5);

        // Approaching state (75% usage)
        let usage = manager.update_usage(150000, 200000).unwrap();
        assert_eq!(usage.state, ContextState::Approaching);
        assert_eq!(usage.usage_percent as u32, 75);

        // Compact state (90% usage)
        let usage = manager.update_usage(180000, 200000).unwrap();
        assert_eq!(usage.state, ContextState::Compact);
        assert_eq!(usage.usage_percent as u32, 90);
    }

    #[test]
    fn test_should_compact() {
        let manager = ContextManager::new(ContextManagerConfig::default());

        // Normal: should not compact
        manager.update_usage(10000, 200000).unwrap();
        assert!(!manager.should_compact());

        // Approaching: should not compact yet
        manager.update_usage(150000, 200000).unwrap();
        assert!(!manager.should_compact());

        // Compact: should compact
        manager.update_usage(180000, 200000).unwrap();
        assert!(manager.should_compact());
    }

    #[test]
    fn test_compaction_metrics() {
        let manager = ContextManager::new(ContextManagerConfig::default());

        // Record first compaction
        manager
            .record_compaction(100, 25, 10000, 2500, "Summary of first compaction")
            .unwrap();

        let metrics = manager.get_metrics();
        assert_eq!(metrics.total_compactions, 1);
        assert_eq!(metrics.compactions_today, 1);
        assert_eq!(metrics.total_messages_removed, 75);
        assert_eq!(metrics.total_tokens_freed, 7500);

        // Record second compaction
        manager
            .record_compaction(50, 15, 5000, 1500, "Summary of second compaction")
            .unwrap();

        let metrics = manager.get_metrics();
        assert_eq!(metrics.total_compactions, 2);
        assert_eq!(metrics.compactions_today, 2);
        assert_eq!(metrics.total_messages_removed, 110); // 75 + 35
        assert_eq!(metrics.total_tokens_freed, 11000); // 7500 + 3500
    }

    #[test]
    fn test_compaction_history() {
        let manager = ContextManager::new(ContextManagerConfig::default());

        manager
            .record_compaction(100, 25, 10000, 2500, "First summary")
            .unwrap();
        manager
            .record_compaction(50, 15, 5000, 1500, "Second summary")
            .unwrap();

        let history = manager.get_compaction_history(10);
        assert_eq!(history.len(), 2);

        // History is returned in reverse chronological order
        assert_eq!(history[0].messages_before, 50);
        assert_eq!(history[1].messages_before, 100);
    }

    #[test]
    fn test_archive_summary() {
        let manager = ContextManager::new(ContextManagerConfig::default());
        let summary = manager.archive_summary("Test summary content", "session-123");

        assert!(summary.contains("Compaction Summary"));
        assert!(summary.contains("session-123"));
        assert!(summary.contains("Test summary content"));
    }

    #[test]
    fn test_disabled_compaction() {
        let mut config = ContextManagerConfig::default();
        config.enabled = false;
        let manager = ContextManager::new(config);

        manager.update_usage(180000, 200000).unwrap();
        assert!(!manager.should_compact()); // Should not compact even at 90%
    }

    #[test]
    fn test_flush_config_enabled() {
        let config = ContextManagerConfig {
            pre_compaction_flush: true,
            flush_message_window: 30,
            flush_timeout_seconds: 15,
            flush_model: Some("claude-haiku".to_string()),
            ..Default::default()
        };
        let manager = ContextManager::new(config);
        let flush = manager.flush_config();
        assert!(flush.is_some());
        let fc = flush.unwrap();
        assert_eq!(fc.message_window, 30);
        assert_eq!(fc.timeout_seconds, 15);
        assert_eq!(fc.model, Some("claude-haiku".to_string()));
    }

    #[test]
    fn test_flush_config_disabled() {
        let config = ContextManagerConfig {
            pre_compaction_flush: false,
            ..Default::default()
        };
        let manager = ContextManager::new(config);
        assert!(manager.flush_config().is_none());
    }

    #[test]
    fn test_pre_compact_flush_state_transitions() {
        let manager = ContextManager::new(ContextManagerConfig::default());

        // Start in Normal
        assert_eq!(manager.get_state(), ContextState::Normal);

        // Move to Approaching (75%)
        manager.update_usage(150000, 200000).unwrap();
        assert_eq!(manager.get_state(), ContextState::Approaching);

        // Set flush in progress
        manager.set_flush_in_progress();
        assert_eq!(manager.get_state(), ContextState::PreCompactFlush);

        // Complete flush -> Compact
        manager.set_flush_complete();
        assert_eq!(manager.get_state(), ContextState::Compact);
    }

    #[test]
    fn test_flush_in_progress_only_from_approaching_or_compact() {
        let manager = ContextManager::new(ContextManagerConfig::default());

        // Normal state should NOT transition to PreCompactFlush
        assert_eq!(manager.get_state(), ContextState::Normal);
        manager.set_flush_in_progress();
        assert_eq!(manager.get_state(), ContextState::Normal); // unchanged
    }

    #[test]
    fn test_update_config_changes_thresholds() {
        let manager = ContextManager::new(ContextManagerConfig::default());

        // Default: 85% threshold -> 80% usage is Approaching
        let usage = manager.update_usage(160000, 200000).unwrap();
        assert_eq!(usage.state, ContextState::Approaching);

        // Update to lower threshold: 75% -> 80% usage is now Compact
        let new_config = ContextManagerConfig {
            compaction_threshold: 0.75,
            ..Default::default()
        };
        manager.update_config(new_config);

        let usage = manager.update_usage(160000, 200000).unwrap();
        assert_eq!(usage.state, ContextState::Compact);
    }

    #[test]
    fn test_truncation_config() {
        let config = ContextManagerConfig {
            truncate_after_compaction: true,
            max_transcript_entries: 150,
            ..Default::default()
        };
        let manager = ContextManager::new(config);
        let tc = manager.truncation_config();
        assert!(tc.is_some());
        let (max_entries, enabled) = tc.unwrap();
        assert_eq!(max_entries, 150);
        assert!(enabled);
    }

    #[test]
    fn test_truncation_config_disabled() {
        let config = ContextManagerConfig {
            truncate_after_compaction: false,
            ..Default::default()
        };
        let manager = ContextManager::new(config);
        assert!(manager.truncation_config().is_none());
    }
}
