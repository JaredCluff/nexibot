//! Structured logging infrastructure with file output, in-memory ring buffer,
//! and dynamic level control.
//!
//! Provides:
//! - File-based logging with daily rotation via `tracing-appender`
//! - In-memory ring buffer of recent entries for the UI log viewer
//! - Runtime log-level changes without restart
//! - Secret redaction via the existing `RedactingLayer`

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::Context;
use tracing_subscriber::{reload, EnvFilter, Layer};

use crate::config::LoggingConfig;
use crate::security::log_redactor::redact_secrets;

// ---------------------------------------------------------------------------
// Log entry
// ---------------------------------------------------------------------------

/// A single log entry stored in the ring buffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub subsystem: Option<String>,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Ring buffer
// ---------------------------------------------------------------------------

/// Thread-safe, bounded ring buffer of recent log entries.
#[derive(Debug, Clone)]
pub struct LogRingBuffer {
    inner: Arc<Mutex<VecDeque<LogEntry>>>,
    capacity: usize,
}

impl LogRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    /// Push a new entry, evicting the oldest if at capacity.
    pub fn push(&self, entry: LogEntry) {
        if let Ok(mut buf) = self.inner.lock() {
            if buf.len() >= self.capacity {
                buf.pop_front();
            }
            buf.push_back(entry);
        }
    }

    /// Return the most recent `n` entries (or all if `n` is 0).
    /// Optionally filter by level and subsystem.
    pub fn recent(
        &self,
        count: usize,
        level_filter: Option<&str>,
        subsystem_filter: Option<&str>,
    ) -> Vec<LogEntry> {
        let buf = match self.inner.lock() {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };

        let iter = buf.iter().rev().filter(|e| {
            if let Some(lf) = level_filter {
                if !e.level.eq_ignore_ascii_case(lf) {
                    return false;
                }
            }
            if let Some(sf) = subsystem_filter {
                match &e.subsystem {
                    Some(sub) => {
                        if !sub.eq_ignore_ascii_case(sf) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            true
        });

        let entries: Vec<LogEntry> = if count > 0 {
            iter.take(count).cloned().collect()
        } else {
            iter.cloned().collect()
        };

        // Return in chronological order
        entries.into_iter().rev().collect()
    }

    /// Clear all entries.
    pub fn clear(&self) {
        if let Ok(mut buf) = self.inner.lock() {
            buf.clear();
        }
    }

    /// Current entry count.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.lock().map(|b| b.len()).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Ring buffer tracing Layer
// ---------------------------------------------------------------------------

/// A `tracing_subscriber::Layer` that captures events into a `LogRingBuffer`.
pub struct RingBufferLayer {
    buffer: LogRingBuffer,
    redact: bool,
}

impl RingBufferLayer {
    pub fn new(buffer: LogRingBuffer, redact: bool) -> Self {
        Self { buffer, redact }
    }
}

/// Extract the `[TAG]` prefix from a log message (e.g., `[TELEGRAM] ...` → `TELEGRAM`).
fn extract_subsystem(msg: &str) -> Option<String> {
    let trimmed = msg.trim_start();
    if trimmed.starts_with('[') {
        if let Some(end) = trimmed.find(']') {
            let tag = &trimmed[1..end];
            if !tag.is_empty() && tag.len() <= 30 {
                return Some(tag.to_string());
            }
        }
    }
    None
}

/// Simple visitor that collects the message field from a tracing event.
struct MessageVisitor {
    message: String,
}

impl MessageVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
        }
    }
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        }
    }
}

impl<S: Subscriber> Layer<S> for RingBufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);

        let msg = if self.redact {
            redact_secrets(&visitor.message)
        } else {
            visitor.message
        };

        let level = match *event.metadata().level() {
            Level::TRACE => "TRACE",
            Level::DEBUG => "DEBUG",
            Level::INFO => "INFO",
            Level::WARN => "WARN",
            Level::ERROR => "ERROR",
        };

        let subsystem = extract_subsystem(&msg);

        self.buffer.push(LogEntry {
            timestamp: Utc::now(),
            level: level.to_string(),
            subsystem,
            message: msg,
        });
    }
}

// ---------------------------------------------------------------------------
// Dynamic level handle
// ---------------------------------------------------------------------------

/// Handle for changing the log level at runtime.
pub type LevelReloadHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;

/// Parse a level string into a `LevelFilter`.
pub fn parse_level(level: &str) -> LevelFilter {
    match level.to_lowercase().as_str() {
        "trace" => LevelFilter::TRACE,
        "debug" => LevelFilter::DEBUG,
        "info" => LevelFilter::INFO,
        "warn" | "warning" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        "off" => LevelFilter::OFF,
        _ => LevelFilter::INFO,
    }
}

/// Build a new `EnvFilter` from a level string, respecting RUST_LOG env var.
pub fn build_filter(level: &str) -> EnvFilter {
    let lf = parse_level(level);
    // If RUST_LOG is set, it takes precedence; otherwise use the configured level
    EnvFilter::builder()
        .with_default_directive(lf.into())
        .from_env_lossy()
}

// ---------------------------------------------------------------------------
// Log state (shared across the app)
// ---------------------------------------------------------------------------

/// Shared logging state accessible from AppState.
#[derive(Clone)]
pub struct LogState {
    pub ring_buffer: LogRingBuffer,
    pub level_handle: Arc<LevelReloadHandle>,
    pub current_level: Arc<Mutex<String>>,
    /// Keep the file writer guard alive for the lifetime of the app.
    /// Dropping this flushes and closes the log file.
    _file_guard: Option<Arc<WorkerGuard>>,
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Default log directory: ~/.config/nexibot/logs/
pub fn default_log_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".config/nexibot/logs")
}

/// Initialize the full logging infrastructure and return a `LogState`.
///
/// Must be called **before** any tracing macros are used (replaces the
/// old `tracing_subscriber::registry()...init()` in main.rs).
pub fn init(config: &LoggingConfig) -> LogState {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let level_str = config.level.clone();

    // Build reloadable filter layer
    let filter = build_filter(&level_str);
    let (filter_layer, reload_handle) = reload::Layer::new(filter);

    // Console (fmt) layer
    let console_layer = if config.console_enabled {
        Some(tracing_subscriber::fmt::layer().with_target(false))
    } else {
        None
    };

    // File appender layer
    let (file_layer, file_guard) = if config.file_enabled {
        let log_dir = config
            .file_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(default_log_dir);

        // Ensure directory exists
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!(
                "[LOGGING] Failed to create log directory {:?}: {}",
                log_dir, e
            );
        }

        let file_appender = tracing_appender::rolling::daily(&log_dir, "nexibot.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        let layer = tracing_subscriber::fmt::layer()
            .with_writer(non_blocking)
            .with_target(false)
            .with_ansi(false);

        (Some(layer), Some(Arc::new(guard)))
    } else {
        (None, None)
    };

    // Ring buffer layer
    let ring_buffer = LogRingBuffer::new(config.ring_buffer_size);
    let ring_layer = RingBufferLayer::new(ring_buffer.clone(), config.redact_secrets);

    // Assemble subscriber
    tracing_subscriber::registry()
        .with(filter_layer)
        .with(console_layer)
        .with(file_layer)
        .with(ring_layer)
        .init();

    LogState {
        ring_buffer,
        level_handle: Arc::new(reload_handle),
        current_level: Arc::new(Mutex::new(level_str)),
        _file_guard: file_guard,
    }
}

/// Change the log level at runtime.
pub fn set_level(state: &LogState, level: &str) -> Result<(), String> {
    let new_filter = build_filter(level);
    state
        .level_handle
        .reload(new_filter)
        .map_err(|e| format!("Failed to reload log filter: {}", e))?;
    if let Ok(mut current) = state.current_level.lock() {
        *current = level.to_string();
    }
    Ok(())
}

/// Get the current log level.
pub fn get_level(state: &LogState) -> String {
    state
        .current_level
        .lock()
        .map(|l| l.clone())
        .unwrap_or_else(|_| "info".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_push_and_recent() {
        let buf = LogRingBuffer::new(3);
        for i in 0..5 {
            buf.push(LogEntry {
                timestamp: Utc::now(),
                level: "INFO".to_string(),
                subsystem: None,
                message: format!("msg {}", i),
            });
        }
        // Should only have 3 entries (capacity)
        assert_eq!(buf.len(), 3);
        let entries = buf.recent(0, None, None);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].message, "msg 2");
        assert_eq!(entries[2].message, "msg 4");
    }

    #[test]
    fn test_ring_buffer_filter_by_level() {
        let buf = LogRingBuffer::new(10);
        buf.push(LogEntry {
            timestamp: Utc::now(),
            level: "INFO".to_string(),
            subsystem: None,
            message: "info msg".to_string(),
        });
        buf.push(LogEntry {
            timestamp: Utc::now(),
            level: "WARN".to_string(),
            subsystem: None,
            message: "warn msg".to_string(),
        });
        buf.push(LogEntry {
            timestamp: Utc::now(),
            level: "ERROR".to_string(),
            subsystem: None,
            message: "error msg".to_string(),
        });

        let warns = buf.recent(0, Some("WARN"), None);
        assert_eq!(warns.len(), 1);
        assert_eq!(warns[0].message, "warn msg");
    }

    #[test]
    fn test_ring_buffer_filter_by_subsystem() {
        let buf = LogRingBuffer::new(10);
        buf.push(LogEntry {
            timestamp: Utc::now(),
            level: "INFO".to_string(),
            subsystem: Some("TELEGRAM".to_string()),
            message: "[TELEGRAM] connected".to_string(),
        });
        buf.push(LogEntry {
            timestamp: Utc::now(),
            level: "INFO".to_string(),
            subsystem: Some("BRIDGE".to_string()),
            message: "[BRIDGE] ready".to_string(),
        });

        let tg = buf.recent(0, None, Some("TELEGRAM"));
        assert_eq!(tg.len(), 1);
        assert_eq!(tg[0].subsystem.as_deref(), Some("TELEGRAM"));
    }

    #[test]
    fn test_extract_subsystem() {
        assert_eq!(
            extract_subsystem("[DEFENSE] loaded"),
            Some("DEFENSE".to_string())
        );
        assert_eq!(
            extract_subsystem("  [BRIDGE] ok"),
            Some("BRIDGE".to_string())
        );
        assert_eq!(extract_subsystem("no tag here"), None);
        assert_eq!(extract_subsystem("[]empty"), None);
    }

    #[test]
    fn test_parse_level() {
        assert_eq!(parse_level("trace"), LevelFilter::TRACE);
        assert_eq!(parse_level("DEBUG"), LevelFilter::DEBUG);
        assert_eq!(parse_level("Info"), LevelFilter::INFO);
        assert_eq!(parse_level("WARN"), LevelFilter::WARN);
        assert_eq!(parse_level("warning"), LevelFilter::WARN);
        assert_eq!(parse_level("error"), LevelFilter::ERROR);
        assert_eq!(parse_level("off"), LevelFilter::OFF);
        assert_eq!(parse_level("garbage"), LevelFilter::INFO);
    }

    #[test]
    fn test_ring_buffer_clear() {
        let buf = LogRingBuffer::new(10);
        buf.push(LogEntry {
            timestamp: Utc::now(),
            level: "INFO".to_string(),
            subsystem: None,
            message: "test".to_string(),
        });
        assert_eq!(buf.len(), 1);
        buf.clear();
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_recent_with_count_limit() {
        let buf = LogRingBuffer::new(100);
        for i in 0..20 {
            buf.push(LogEntry {
                timestamp: Utc::now(),
                level: "INFO".to_string(),
                subsystem: None,
                message: format!("msg {}", i),
            });
        }
        let entries = buf.recent(5, None, None);
        assert_eq!(entries.len(), 5);
        // Should be the most recent 5 in chronological order
        assert_eq!(entries[0].message, "msg 15");
        assert_eq!(entries[4].message, "msg 19");
    }
}
