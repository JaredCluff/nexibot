//! Context window management with auto-compaction state machine.
//!
//! Manages conversation context lifecycle:
//! - Tracks token usage as percentage of context window
//! - Implements state machine (Normal → Approaching → Compact)
//! - Triggers compaction at configurable thresholds
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

/// Context manager for conversation lifecycle.
pub struct ContextManager {
    config: ContextManagerConfig,
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
            config,
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

        let new_state = if usage_percent >= (self.config.compaction_threshold * 100.0) {
            ContextState::Compact
        } else if usage_percent >= (self.config.approaching_threshold * 100.0) {
            ContextState::Approaching
        } else {
            ContextState::Normal
        };

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
        if !self.config.enabled {
            return false;
        }

        let state = *self.current_state.lock().unwrap_or_else(|e| e.into_inner());
        state == ContextState::Compact
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
        if metrics_today > self.config.max_compactions_per_day {
            warn!(
                "[CONTEXT] {} compactions today (exceeds limit of {})",
                metrics_today, self.config.max_compactions_per_day
            );
        }

        // Archive summary to dated memory file when configured to do so.
        if self.config.archive_summaries {
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
        self.compactions_today.load(Ordering::SeqCst) > self.config.max_compactions_per_day
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
}
