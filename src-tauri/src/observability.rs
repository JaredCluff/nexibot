//! OpenTelemetry monitoring and observability infrastructure.
//!
//! Provides comprehensive tracing, metrics, and logging:
//! - Distributed tracing with spans for all major operations
//! - Structured JSON logging with automatic redaction
//! - Metrics collection (tokens, cost, latency)
//! - Audit trail for security and compliance
//! - Cost tracking and budget alerts
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Per-model pricing table
// ---------------------------------------------------------------------------

/// Per-model pricing in USD per token.
#[derive(Debug, Clone, Copy)]
struct ModelPricing {
    /// Cost per input token in USD.
    input_per_token: f64,
    /// Cost per output token in USD.
    output_per_token: f64,
}

impl ModelPricing {
    /// Construct from per-million token rates (more readable at the call site).
    const fn from_per_million(input_per_million: f64, output_per_million: f64) -> Self {
        Self {
            input_per_token: input_per_million / 1_000_000.0,
            output_per_token: output_per_million / 1_000_000.0,
        }
    }
}

/// Look up per-model pricing by model name.
///
/// Falls back to Opus pricing (the most expensive tier) for unknown models so
/// that costs are never under-reported.
fn pricing_for_model(model: &str) -> ModelPricing {
    let lower = model.to_lowercase();

    // Claude tiers
    if lower.contains("haiku") {
        // Claude Haiku: $0.80/M input, $4/M output
        return ModelPricing::from_per_million(0.80, 4.0);
    }
    if lower.contains("sonnet") {
        // Claude Sonnet: $3/M input, $15/M output
        return ModelPricing::from_per_million(3.0, 15.0);
    }
    if lower.contains("opus") {
        // Claude Opus: $15/M input, $75/M output
        return ModelPricing::from_per_million(15.0, 75.0);
    }

    // OpenAI tiers
    if lower.starts_with("gpt-4o-mini") {
        return ModelPricing::from_per_million(0.15, 0.60);
    }
    if lower.starts_with("gpt-4o") {
        return ModelPricing::from_per_million(2.50, 10.0);
    }
    if lower.starts_with("gpt-4") {
        return ModelPricing::from_per_million(10.0, 30.0);
    }

    // Google Gemini tiers
    if lower.starts_with("gemini-2.0-flash") || lower.starts_with("gemini-flash") {
        return ModelPricing::from_per_million(0.075, 0.30);
    }
    if lower.starts_with("gemini") {
        return ModelPricing::from_per_million(1.25, 5.0);
    }

    // DeepSeek
    if lower.starts_with("deepseek") {
        return ModelPricing::from_per_million(0.14, 0.28);
    }

    // Unknown model: use Opus rates to avoid under-counting costs
    ModelPricing::from_per_million(15.0, 75.0)
}

// ---------------------------------------------------------------------------
// CostTracker
// ---------------------------------------------------------------------------

/// Cost tracking and metrics aggregator.
///
/// ## Daily reset
///
/// The daily budget counter is reset based on the **current UTC calendar date**,
/// not elapsed wall-clock time (`Instant`). An `Instant`-based 24-hour window
/// does not survive process restarts and drifts relative to midnight UTC.
///
/// `last_reset_date` stores the ISO-8601 date (`YYYY-MM-DD`) of the last reset
/// and is compared against the current UTC date on every `record_tokens*` call.
/// When the dates differ the daily counter is zeroed and the stored date is
/// updated.
#[derive(Debug, Clone)]
pub struct CostTracker {
    total_tokens_input: Arc<AtomicU64>,
    total_tokens_output: Arc<AtomicU64>,
    total_cost_usd: Arc<std::sync::Mutex<f64>>,
    daily_cost_usd: Arc<std::sync::Mutex<f64>>,
    /// UTC date string (`YYYY-MM-DD`) of the last daily reset.
    last_reset_date: Arc<std::sync::Mutex<String>>,
}

impl CostTracker {
    /// Create a new cost tracker, initialising the reset date to today (UTC).
    pub fn new() -> Self {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        Self {
            total_tokens_input: Arc::new(AtomicU64::new(0)),
            total_tokens_output: Arc::new(AtomicU64::new(0)),
            total_cost_usd: Arc::new(std::sync::Mutex::new(0.0)),
            daily_cost_usd: Arc::new(std::sync::Mutex::new(0.0)),
            last_reset_date: Arc::new(std::sync::Mutex::new(today)),
        }
    }

    /// Record token usage for a single API call using a generic fallback price.
    ///
    /// Prefer [`record_tokens_for_model`] when the model name is available so
    /// that accurate per-model pricing is applied.
    pub fn record_tokens(&self, input_tokens: u64, output_tokens: u64) {
        self.record_tokens_for_model(input_tokens, output_tokens, "");
    }

    /// Record token usage with an explicit model name for accurate pricing.
    ///
    /// `model` is matched against the per-model pricing table. Unknown models
    /// fall back to Opus rates so costs are never under-reported.
    pub fn record_tokens_for_model(&self, input_tokens: u64, output_tokens: u64, model: &str) {
        let pricing = pricing_for_model(model);
        let call_cost = (input_tokens as f64 * pricing.input_per_token)
            + (output_tokens as f64 * pricing.output_per_token);

        self.total_tokens_input
            .fetch_add(input_tokens, Ordering::SeqCst);
        self.total_tokens_output
            .fetch_add(output_tokens, Ordering::SeqCst);

        // Update total cost — recover from lock poisoning instead of panicking.
        {
            let mut total = self
                .total_cost_usd
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *total += call_cost;
        }

        // Update daily cost with UTC wall-clock date-based reset.
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        {
            let mut daily = self
                .daily_cost_usd
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut last_reset = self
                .last_reset_date
                .lock()
                .unwrap_or_else(|e| e.into_inner());

            if *last_reset != today {
                // New UTC calendar day — reset daily counter.
                *daily = 0.0;
                *last_reset = today.clone();
                info!("[COST] Daily budget reset for UTC date {}", today);
            }
            *daily += call_cost;
        }

        info!(
            "[COST] Usage: {} input + {} output tokens = ${:.6} (model: {})",
            input_tokens,
            output_tokens,
            call_cost,
            if model.is_empty() { "unknown" } else { model }
        );
    }

    /// Get current metrics
    pub fn get_metrics(&self) -> CostMetrics {
        CostMetrics {
            total_tokens_input: self.total_tokens_input.load(Ordering::SeqCst),
            total_tokens_output: self.total_tokens_output.load(Ordering::SeqCst),
            total_cost_usd: *self
                .total_cost_usd
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            daily_cost_usd: *self
                .daily_cost_usd
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        }
    }

    /// Check if daily cost exceeds budget
    pub fn check_daily_budget(&self, budget_usd: f64) -> bool {
        let daily_cost = *self
            .daily_cost_usd
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if daily_cost > budget_usd {
            warn!(
                "[BUDGET] Daily cost ${:.2} exceeds budget ${:.2}",
                daily_cost, budget_usd
            );
            true
        } else {
            false
        }
    }
}

/// Cost metrics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostMetrics {
    pub total_tokens_input: u64,
    pub total_tokens_output: u64,
    pub total_cost_usd: f64,
    pub daily_cost_usd: f64,
}

/// Span attribute helper for building structured attributes
#[derive(Debug, Clone)]
pub struct SpanAttributes {
    attrs: std::collections::HashMap<String, String>,
}

impl SpanAttributes {
    /// Create new span attributes
    pub fn new() -> Self {
        Self {
            attrs: std::collections::HashMap::new(),
        }
    }

    /// Add an attribute
    pub fn add(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attrs.insert(key.into(), value.into());
        self
    }

    /// Add token count attributes
    pub fn with_tokens(self, input: u64, output: u64) -> Self {
        self.add("tokens.input", input.to_string())
            .add("tokens.output", output.to_string())
    }

    /// Add latency attribute
    pub fn with_latency(self, millis: u64) -> Self {
        self.add("latency_ms", millis.to_string())
    }

    /// Add error attribute
    pub fn with_error(self, error: &str) -> Self {
        self.add("error", error.to_string())
    }

    /// Get all attributes
    pub fn build(self) -> std::collections::HashMap<String, String> {
        self.attrs
    }
}

/// Sensitive data redaction for logs
pub struct Redactor;

impl Redactor {
    /// Redact API keys and tokens from text
    pub fn redact_secrets(text: &str) -> String {
        let mut result = text.to_string();

        // Redact API keys (pattern: sk-ant-... or sk-proj-...)
        result = regex::Regex::new(r"sk-[a-zA-Z0-9_-]{20,}")
            .ok()
            .and_then(|re| {
                Some(
                    re.replace_all(&result, "***REDACTED_API_KEY***")
                        .to_string(),
                )
            })
            .unwrap_or(result);

        // Redact OAuth tokens (pattern: Bearer ...)
        result = regex::Regex::new(r"Bearer [a-zA-Z0-9_-]+")
            .ok()
            .and_then(|re| {
                Some(
                    re.replace_all(&result, "Bearer ***REDACTED_TOKEN***")
                        .to_string(),
                )
            })
            .unwrap_or(result);

        // Redact email addresses (basic pattern)
        result = regex::Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}")
            .ok()
            .and_then(|re| Some(re.replace_all(&result, "***EMAIL***").to_string()))
            .unwrap_or(result);

        result
    }

    /// Redact sensitive fields from JSON
    pub fn redact_json(json: &serde_json::Value) -> serde_json::Value {
        match json {
            serde_json::Value::Object(map) => {
                let mut new_map = serde_json::Map::new();
                for (key, value) in map {
                    let new_value = if Self::is_sensitive_field(key) {
                        serde_json::Value::String("***REDACTED***".to_string())
                    } else {
                        Self::redact_json(value)
                    };
                    new_map.insert(key.clone(), new_value);
                }
                serde_json::Value::Object(new_map)
            }
            serde_json::Value::Array(arr) => {
                let new_arr: Vec<_> = arr.iter().map(|v| Self::redact_json(v)).collect();
                serde_json::Value::Array(new_arr)
            }
            _ => json.clone(),
        }
    }

    fn is_sensitive_field(field: &str) -> bool {
        matches!(
            field.to_lowercase().as_str(),
            "api_key"
                | "apikey"
                | "token"
                | "password"
                | "secret"
                | "authorization"
                | "access_token"
                | "refresh_token"
                | "client_secret"
        )
    }
}

/// Audit trail entry for compliance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Event type (e.g., "api_call", "token_refresh", "config_change")
    pub event_type: String,
    /// User or system identifier
    pub actor: String,
    /// Resource being accessed (e.g., API endpoint, config setting)
    pub resource: String,
    /// Action performed (e.g., "GET", "POST", "UPDATE")
    pub action: String,
    /// Status (e.g., "success", "failure")
    pub status: String,
    /// Optional error message (redacted)
    pub error: Option<String>,
    /// Request metadata (redacted)
    pub metadata: serde_json::Value,
}

impl AuditLogEntry {
    /// Create a new audit log entry
    pub fn new(
        event_type: impl Into<String>,
        actor: impl Into<String>,
        resource: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: chrono::Utc::now().to_rfc3339(),
            event_type: event_type.into(),
            actor: actor.into(),
            resource: resource.into(),
            action: action.into(),
            status: "success".to_string(),
            error: None,
            metadata: serde_json::json!({}),
        }
    }

    /// Mark entry as failed
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.status = "failure".to_string();
        self.error = Some(Redactor::redact_secrets(&error.into()));
        self
    }

    /// Add metadata (will be redacted)
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Redactor::redact_json(&metadata);
        self
    }
}

/// Audit logger
pub struct AuditLog {
    entries: Arc<std::sync::Mutex<Vec<AuditLogEntry>>>,
    max_entries: usize,
}

impl AuditLog {
    /// Create a new audit logger
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Arc::new(std::sync::Mutex::new(Vec::new())),
            max_entries,
        }
    }

    /// Log an audit entry
    pub fn log(&self, entry: AuditLogEntry) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.push(entry);

        // Trim to max size (keep newest)
        if entries.len() > self.max_entries {
            entries.remove(0);
        }
    }

    /// Get recent audit entries
    pub fn get_recent(&self, limit: usize) -> Vec<AuditLogEntry> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.iter().rev().take(limit).cloned().collect()
    }

    /// Filter audit entries by type
    pub fn filter_by_type(&self, event_type: &str) -> Vec<AuditLogEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|e| e.event_type == event_type)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_tracker() {
        let tracker = CostTracker::new();
        tracker.record_tokens(1000, 500);

        let metrics = tracker.get_metrics();
        assert_eq!(metrics.total_tokens_input, 1000);
        assert_eq!(metrics.total_tokens_output, 500);
        assert!(metrics.total_cost_usd > 0.0);
    }

    #[test]
    fn test_cost_tracker_per_model_pricing() {
        // Haiku should be much cheaper than Opus.
        let tracker_opus = CostTracker::new();
        let tracker_haiku = CostTracker::new();
        tracker_opus.record_tokens_for_model(1_000_000, 0, "claude-opus-4-6");
        tracker_haiku.record_tokens_for_model(1_000_000, 0, "claude-haiku-4-5");

        let opus_cost = tracker_opus.get_metrics().total_cost_usd;
        let haiku_cost = tracker_haiku.get_metrics().total_cost_usd;
        assert!(
            opus_cost > haiku_cost,
            "Opus should cost more than Haiku per token: opus={}, haiku={}",
            opus_cost,
            haiku_cost
        );
    }

    #[test]
    fn test_daily_reset_uses_utc_date() {
        let tracker = CostTracker::new();
        tracker.record_tokens_for_model(1000, 500, "claude-sonnet-4-5-20250929");
        let before = tracker.get_metrics().daily_cost_usd;
        assert!(before > 0.0);

        // Simulate a day rollover by forcing last_reset_date to yesterday.
        {
            let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string();
            let mut last_reset = tracker
                .last_reset_date
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *last_reset = yesterday;
        }

        // The next record call should trigger a reset.
        tracker.record_tokens_for_model(100, 50, "claude-sonnet-4-5-20250929");
        let after = tracker.get_metrics().daily_cost_usd;

        // Daily cost should be just the new call, not accumulated with before.
        assert!(
            after < before,
            "After day rollover, daily cost should reset (before={}, after={})",
            before,
            after
        );
    }

    #[test]
    fn test_pricing_tiers_ordered_correctly() {
        let haiku = pricing_for_model("claude-haiku-4-5-20251001");
        let sonnet = pricing_for_model("claude-sonnet-4-5-20250929");
        let opus = pricing_for_model("claude-opus-4-6");

        assert!(haiku.input_per_token < sonnet.input_per_token);
        assert!(sonnet.input_per_token < opus.input_per_token);
        assert!(haiku.output_per_token < sonnet.output_per_token);
        assert!(sonnet.output_per_token < opus.output_per_token);
    }

    #[test]
    fn test_unknown_model_uses_opus_rates() {
        // Unknown models must use the most expensive tier to avoid under-counting.
        let unknown = pricing_for_model("some-unknown-model-xyz");
        let opus = pricing_for_model("claude-opus-4-6");
        assert_eq!(unknown.input_per_token, opus.input_per_token);
        assert_eq!(unknown.output_per_token, opus.output_per_token);
    }

    #[test]
    fn test_redactor() {
        let text = "API key: sk-ant-v0123456789abcdef API key hidden";
        let redacted = Redactor::redact_secrets(text);
        assert!(!redacted.contains("sk-ant-"));
        assert!(redacted.contains("***REDACTED_API_KEY***"));
    }

    #[test]
    fn test_audit_log() {
        let audit = AuditLog::new(100);

        let entry = AuditLogEntry::new("api_call", "user123", "/api/messages", "POST")
            .with_metadata(serde_json::json!({ "tokens": 100 }));

        audit.log(entry);

        let recent = audit.get_recent(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].event_type, "api_call");
    }

    #[test]
    fn test_span_attributes() {
        let attrs = SpanAttributes::new()
            .with_tokens(1000, 500)
            .with_latency(1240)
            .build();

        assert_eq!(attrs.get("tokens.input").unwrap(), "1000");
        assert_eq!(attrs.get("tokens.output").unwrap(), "500");
        assert_eq!(attrs.get("latency_ms").unwrap(), "1240");
    }
}
