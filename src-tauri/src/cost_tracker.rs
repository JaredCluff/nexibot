use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Per-model token usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
    pub request_count: u32,
}

/// Session-scoped cost tracker.
pub struct CostTracker {
    pub session_id: String,
    pub model_usage: HashMap<String, ModelUsage>,
    pub total_cost_usd: f64,
    pub total_api_duration: Duration,
    pub total_tool_duration: Duration,
    pub session_start: Instant,
}

impl CostTracker {
    pub fn new(session_id: impl Into<String>) -> Self {
        CostTracker {
            session_id: session_id.into(),
            model_usage: HashMap::new(),
            total_cost_usd: 0.0,
            total_api_duration: Duration::ZERO,
            total_tool_duration: Duration::ZERO,
            session_start: Instant::now(),
        }
    }

    /// Record usage from a single API response.
    pub fn record_api_call(
        &mut self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
        api_duration: Duration,
    ) {
        let cost = calculate_cost(model, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens);
        self.total_cost_usd += cost;
        self.total_api_duration += api_duration;

        let entry = self.model_usage.entry(model.to_string()).or_default();
        entry.input_tokens += input_tokens;
        entry.output_tokens += output_tokens;
        entry.cache_read_tokens += cache_read_tokens;
        entry.cache_creation_tokens += cache_creation_tokens;
        entry.cost_usd += cost;
        entry.request_count += 1;
    }

    pub fn record_tool_duration(&mut self, duration: Duration) {
        self.total_tool_duration += duration;
    }

    /// Total input tokens across all models (for context window estimation).
    pub fn total_input_tokens(&self) -> u64 {
        self.model_usage.values().map(|u| u.input_tokens).sum()
    }

    pub fn format_summary(&self) -> String {
        let wall = self.session_start.elapsed();
        let mut lines = vec![
            format!("Total cost: ${:.4}", self.total_cost_usd),
            format!("Duration (API): {}", format_duration(self.total_api_duration)),
            format!("Duration (wall): {}", format_duration(wall)),
        ];
        if !self.model_usage.is_empty() {
            lines.push("Usage by model:".to_string());
            let mut models: Vec<_> = self.model_usage.iter().collect();
            models.sort_by_key(|(k, _)| k.as_str());
            for (model, usage) in models {
                lines.push(format!(
                    "  {}: {}in {}out {}cache_r {}cache_w (${:.4})",
                    short_model_name(model),
                    usage.input_tokens, usage.output_tokens,
                    usage.cache_read_tokens, usage.cache_creation_tokens,
                    usage.cost_usd
                ));
            }
        }
        lines.join("\n")
    }
}

/// Pricing table (USD per million tokens).
/// Updated as of 2025; should be kept in sync with Anthropic pricing.
pub fn calculate_cost(
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) -> f64 {
    let (input_price, output_price, cache_read_price, cache_write_price) =
        get_pricing(model);

    (input as f64 * input_price
        + output as f64 * output_price
        + cache_read as f64 * cache_read_price
        + cache_write as f64 * cache_write_price)
        / 1_000_000.0
}

fn get_pricing(model: &str) -> (f64, f64, f64, f64) {
    // (input, output, cache_read, cache_write) per million tokens
    if model.contains("opus-4") {
        (15.0, 75.0, 1.50, 18.75)
    } else if model.contains("sonnet-4") || model.contains("sonnet-3-5") {
        (3.0, 15.0, 0.30, 3.75)
    } else if model.contains("haiku-4") || model.contains("haiku-3-5") {
        (0.80, 4.0, 0.08, 1.0)
    } else {
        // Conservative fallback
        (3.0, 15.0, 0.30, 3.75)
    }
}

fn short_model_name(model: &str) -> &str {
    // Return last meaningful segment
    if model.contains("opus") { "claude-opus" }
    else if model.contains("sonnet") { "claude-sonnet" }
    else if model.contains("haiku") { "claude-haiku" }
    else { model }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Context window manager: decides when to compact.
pub struct ContextManager {
    pub context_window: u64,
    pub compaction_threshold: f64,
    pub current_tokens: u64,
}

impl ContextManager {
    pub fn new(context_window: u64) -> Self {
        ContextManager {
            context_window,
            compaction_threshold: 0.8,
            current_tokens: 0,
        }
    }

    pub fn update_tokens(&mut self, tokens: u64) {
        self.current_tokens = tokens;
    }

    pub fn should_compact(&self) -> bool {
        self.current_tokens as f64 > self.context_window as f64 * self.compaction_threshold
    }

    pub fn usage_ratio(&self) -> f64 {
        if self.context_window == 0 { return 0.0; }
        self.current_tokens as f64 / self.context_window as f64
    }
}

/// Budget limits loaded from config.
#[derive(Debug, Clone)]
pub struct BudgetLimits {
    pub max_session_cost_usd: Option<f64>,
    pub warn_session_cost_usd: Option<f64>,
    pub max_tokens_per_turn: Option<u64>,
}

impl Default for BudgetLimits {
    fn default() -> Self {
        BudgetLimits {
            max_session_cost_usd: None,
            warn_session_cost_usd: None,
            max_tokens_per_turn: None,
        }
    }
}

impl BudgetLimits {
    pub fn check_cost(&self, cost: f64) -> BudgetStatus {
        if let Some(max) = self.max_session_cost_usd {
            if cost >= max {
                return BudgetStatus::Exceeded(format!(
                    "Session cost ${:.4} exceeded limit ${:.2}", cost, max
                ));
            }
        }
        if let Some(warn) = self.warn_session_cost_usd {
            if cost >= warn {
                return BudgetStatus::Warning(format!(
                    "Session cost ${:.4} is approaching limit ${:.2}", cost, warn
                ));
            }
        }
        BudgetStatus::Ok
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BudgetStatus {
    Ok,
    Warning(String),
    Exceeded(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_tracker_accumulates() {
        let mut tracker = CostTracker::new("session_1");
        tracker.record_api_call("claude-sonnet-4-6", 1000, 500, 0, 0, Duration::from_secs(1));
        tracker.record_api_call("claude-sonnet-4-6", 500, 200, 1000, 0, Duration::from_secs(1));
        assert_eq!(tracker.model_usage["claude-sonnet-4-6"].input_tokens, 1500);
        assert_eq!(tracker.model_usage["claude-sonnet-4-6"].request_count, 2);
        assert!(tracker.total_cost_usd > 0.0);
    }

    #[test]
    fn test_cost_tracker_total_input_tokens() {
        let mut tracker = CostTracker::new("s");
        tracker.record_api_call("claude-haiku-4-5-20251001", 5000, 100, 0, 0, Duration::ZERO);
        assert_eq!(tracker.total_input_tokens(), 5000);
    }

    #[test]
    fn test_calculate_cost_haiku_cheaper_than_opus() {
        let haiku_cost = calculate_cost("claude-haiku-4-5", 1000, 1000, 0, 0);
        let opus_cost = calculate_cost("claude-opus-4-6", 1000, 1000, 0, 0);
        assert!(haiku_cost < opus_cost);
    }

    #[test]
    fn test_context_manager_should_compact_at_80_percent() {
        let mut cm = ContextManager::new(200_000);
        cm.update_tokens(160_001);
        assert!(cm.should_compact());
        cm.update_tokens(159_999);
        assert!(!cm.should_compact());
    }

    #[test]
    fn test_budget_exceeded() {
        let limits = BudgetLimits {
            max_session_cost_usd: Some(5.0),
            warn_session_cost_usd: Some(2.0),
            max_tokens_per_turn: None,
        };
        assert_eq!(limits.check_cost(1.0), BudgetStatus::Ok);
        assert!(matches!(limits.check_cost(2.5), BudgetStatus::Warning(_)));
        assert!(matches!(limits.check_cost(5.5), BudgetStatus::Exceeded(_)));
    }

    #[test]
    fn test_format_summary_includes_model() {
        let mut tracker = CostTracker::new("s");
        tracker.record_api_call("claude-sonnet-4-6", 100, 50, 0, 0, Duration::from_millis(500));
        let summary = tracker.format_summary();
        assert!(summary.contains("Total cost"));
        assert!(summary.contains("claude-sonnet"));
    }
}
