//! Model selection and failover routing across providers.
//!
//! The [`ModelRouter`] maintains a priority-ordered list of model routes and
//! a set of currently-failed model IDs. When asked to select a model it
//! walks the routes by ascending priority, skipping disabled or failed
//! entries, and returns a [`RoutingDecision`] that indicates whether a
//! fallback was used.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tracing::{error, info, warn};

/// A single route entry mapping a model to its provider with a priority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoute {
    /// The model identifier (e.g. "claude-sonnet-4-5-20250929", "gpt-4o").
    pub model_id: String,
    /// The provider that serves this model (e.g. "anthropic", "openai").
    pub provider: String,
    /// Routing priority. Lower values are tried first.
    pub priority: u32,
    /// Whether this route is currently enabled.
    pub enabled: bool,
}

/// The result of a routing decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// The model that was selected.
    pub selected_model: String,
    /// The provider that serves the selected model.
    pub selected_provider: String,
    /// `true` if the original preferred model was unavailable and a fallback
    /// was chosen instead.
    pub fallback_attempted: bool,
    /// Human-readable explanation of why this model was selected.
    pub reason: String,
}

/// Event emitted to the frontend when a model fallback occurs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelFallbackEvent {
    pub original_model: String,
    pub fallback_model: String,
    pub reason: String,
    pub timestamp: String,
}

/// Priority-based model router with failover tracking.
///
/// Routes are sorted by `priority` (ascending) at selection time.
/// Failed models are tracked in a separate set and automatically
/// skipped during selection.
pub struct ModelRouter {
    /// All registered routes.
    routes: Vec<ModelRoute>,
    /// Model IDs that are currently considered failed / unavailable.
    failed_routes: HashSet<String>,
}

impl ModelRouter {
    /// Create a new, empty router.
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            failed_routes: HashSet::new(),
        }
    }

    /// Add a route. If a route with the same `model_id` already exists it is
    /// replaced (only one route per model ID is allowed).
    pub fn add_route(&mut self, route: ModelRoute) {
        if let Some(pos) = self
            .routes
            .iter()
            .position(|r| r.model_id == route.model_id)
        {
            info!(
                "[MODEL_ROUTER] Replaced route for model '{}' (provider '{}', priority {})",
                route.model_id, route.provider, route.priority
            );
            self.routes[pos] = route;
        } else {
            info!(
                "[MODEL_ROUTER] Added route for model '{}' (provider '{}', priority {})",
                route.model_id, route.provider, route.priority
            );
            self.routes.push(route);
        }
    }

    /// Select the best available model, optionally preferring a specific model.
    ///
    /// Selection logic:
    /// 1. If `preferred_model` is given, the route is enabled, and it is not
    ///    in the failed set, it is returned immediately.
    /// 2. Otherwise, routes are iterated by ascending priority. The first
    ///    enabled, non-failed route is returned with `fallback_attempted = true`
    ///    (if a preferred model was specified) or `false` (if none was).
    /// 3. Returns `None` only when no route is available.
    pub fn select_model(&self, preferred_model: Option<&str>) -> Option<RoutingDecision> {
        // 1. Try the preferred model if given.
        if let Some(preferred) = preferred_model {
            if let Some(route) = self.routes.iter().find(|r| r.model_id == preferred) {
                if route.enabled && !self.failed_routes.contains(preferred) {
                    return Some(RoutingDecision {
                        selected_model: route.model_id.clone(),
                        selected_provider: route.provider.clone(),
                        fallback_attempted: false,
                        reason: format!("Preferred model '{}' is available", preferred),
                    });
                }
                warn!(
                    "[MODEL_ROUTER] Preferred model '{}' unavailable (enabled={}, failed={})",
                    preferred,
                    route.enabled,
                    self.failed_routes.contains(preferred)
                );
            } else {
                warn!(
                    "[MODEL_ROUTER] Preferred model '{}' has no registered route",
                    preferred
                );
            }
        }

        // 2. Walk routes by priority (ascending).
        let mut sorted: Vec<&ModelRoute> = self.routes.iter().collect();
        sorted.sort_by_key(|r| r.priority);

        for route in sorted {
            if !route.enabled {
                continue;
            }
            if self.failed_routes.contains(&route.model_id) {
                continue;
            }

            let fallback_attempted = preferred_model.is_some();
            let reason = if fallback_attempted {
                format!(
                    "Fallback to '{}' (priority {}) after preferred model unavailable",
                    route.model_id, route.priority
                )
            } else {
                format!(
                    "Selected '{}' (priority {}) as highest-priority available route",
                    route.model_id, route.priority
                )
            };

            info!("[MODEL_ROUTER] {}", reason);

            return Some(RoutingDecision {
                selected_model: route.model_id.clone(),
                selected_provider: route.provider.clone(),
                fallback_attempted,
                reason,
            });
        }

        warn!("[MODEL_ROUTER] No available model route found");
        None
    }

    /// Mark a model as failed so it will be skipped during selection.
    pub fn mark_failed(&mut self, model_id: &str) {
        self.failed_routes.insert(model_id.to_string());
        warn!("[MODEL_ROUTER] Model '{}' marked as failed", model_id);
    }

    /// Mark a model as failed due to a specific error and attempt fallback selection.
    ///
    /// This method provides structured logging for the full fallback lifecycle:
    /// - Logs a warning when the model fails and fallback is attempted
    /// - Logs success when a fallback model is found
    /// - Logs an error when all fallback models are exhausted
    ///
    /// Returns `Some(RoutingDecision)` with the fallback model, or `None` if
    /// all routes are exhausted. Also returns a `ModelFallbackEvent` when a
    /// fallback is selected (for frontend emission).
    pub fn fail_and_fallback(
        &mut self,
        failed_model: &str,
        error_reason: &str,
    ) -> (Option<RoutingDecision>, Option<ModelFallbackEvent>) {
        // Mark the model as failed
        self.failed_routes.insert(failed_model.to_string());

        // Try to find a fallback
        let mut sorted: Vec<&ModelRoute> = self.routes.iter().collect();
        sorted.sort_by_key(|r| r.priority);

        let fallback = sorted
            .iter()
            .find(|r| r.enabled && !self.failed_routes.contains(&r.model_id));

        match fallback {
            Some(route) => {
                warn!(
                    "[MODEL_FALLBACK] {} failed ({}), falling back to {}",
                    failed_model, error_reason, route.model_id
                );
                info!(
                    "[MODEL_FALLBACK] Successfully fell back from {} to {}",
                    failed_model, route.model_id
                );

                let event = ModelFallbackEvent {
                    original_model: failed_model.to_string(),
                    fallback_model: route.model_id.clone(),
                    reason: error_reason.to_string(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                };

                let decision = RoutingDecision {
                    selected_model: route.model_id.clone(),
                    selected_provider: route.provider.clone(),
                    fallback_attempted: true,
                    reason: format!(
                        "Fallback to '{}' after '{}' failed: {}",
                        route.model_id, failed_model, error_reason
                    ),
                };

                (Some(decision), Some(event))
            }
            None => {
                let tried_models: Vec<&str> =
                    self.failed_routes.iter().map(|s| s.as_str()).collect();
                error!(
                    "[MODEL_FALLBACK] All fallback models exhausted. Tried: {:?}",
                    tried_models
                );
                (None, None)
            }
        }
    }

    /// Mark a previously-failed model as recovered / available again.
    pub fn mark_recovered(&mut self, model_id: &str) {
        if self.failed_routes.remove(model_id) {
            info!("[MODEL_ROUTER] Model '{}' marked as recovered", model_id);
        }
    }

    /// Read-only view of all registered routes.
    pub fn list_routes(&self) -> &[ModelRoute] {
        &self.routes
    }

    /// Clear all failure marks, making every enabled route available again.
    pub fn clear_failures(&mut self) {
        let count = self.failed_routes.len();
        self.failed_routes.clear();
        info!("[MODEL_ROUTER] Cleared {} failure mark(s)", count);
    }

    /// Return the number of routes currently marked as failed.
    pub fn failed_count(&self) -> usize {
        self.failed_routes.len()
    }
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn route(model: &str, provider: &str, priority: u32) -> ModelRoute {
        ModelRoute {
            model_id: model.to_string(),
            provider: provider.to_string(),
            priority,
            enabled: true,
        }
    }

    fn disabled_route(model: &str, provider: &str, priority: u32) -> ModelRoute {
        ModelRoute {
            model_id: model.to_string(),
            provider: provider.to_string(),
            priority,
            enabled: false,
        }
    }

    #[test]
    fn test_empty_router_returns_none() {
        let router = ModelRouter::new();
        assert!(router.select_model(None).is_none());
        assert!(router.select_model(Some("gpt-4o")).is_none());
    }

    #[test]
    fn test_select_preferred_model() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-sonnet-4-5-20250929", "anthropic", 1));
        router.add_route(route("gpt-4o", "openai", 2));

        let decision = router.select_model(Some("gpt-4o")).unwrap();
        assert_eq!(decision.selected_model, "gpt-4o");
        assert_eq!(decision.selected_provider, "openai");
        assert!(!decision.fallback_attempted);
    }

    #[test]
    fn test_select_by_priority_when_no_preference() {
        let mut router = ModelRouter::new();
        router.add_route(route("gpt-4o", "openai", 10));
        router.add_route(route("claude-sonnet-4-5-20250929", "anthropic", 1));
        router.add_route(route("gemini-2.0-flash", "google", 5));

        let decision = router.select_model(None).unwrap();
        assert_eq!(decision.selected_model, "claude-sonnet-4-5-20250929");
        assert!(!decision.fallback_attempted);
    }

    #[test]
    fn test_fallback_when_preferred_is_failed() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-sonnet-4-5-20250929", "anthropic", 1));
        router.add_route(route("gpt-4o", "openai", 2));

        router.mark_failed("claude-sonnet-4-5-20250929");

        let decision = router
            .select_model(Some("claude-sonnet-4-5-20250929"))
            .unwrap();
        assert_eq!(decision.selected_model, "gpt-4o");
        assert!(decision.fallback_attempted);
    }

    #[test]
    fn test_fallback_when_preferred_is_disabled() {
        let mut router = ModelRouter::new();
        router.add_route(disabled_route("claude-sonnet-4-5-20250929", "anthropic", 1));
        router.add_route(route("gpt-4o", "openai", 2));

        let decision = router
            .select_model(Some("claude-sonnet-4-5-20250929"))
            .unwrap();
        assert_eq!(decision.selected_model, "gpt-4o");
        assert!(decision.fallback_attempted);
    }

    #[test]
    fn test_fallback_when_preferred_not_registered() {
        let mut router = ModelRouter::new();
        router.add_route(route("gpt-4o", "openai", 2));

        let decision = router.select_model(Some("nonexistent-model")).unwrap();
        assert_eq!(decision.selected_model, "gpt-4o");
        assert!(decision.fallback_attempted);
    }

    #[test]
    fn test_skips_disabled_routes() {
        let mut router = ModelRouter::new();
        router.add_route(disabled_route("claude-sonnet-4-5-20250929", "anthropic", 1));
        router.add_route(route("gpt-4o", "openai", 2));

        let decision = router.select_model(None).unwrap();
        assert_eq!(decision.selected_model, "gpt-4o");
    }

    #[test]
    fn test_skips_failed_routes() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-sonnet-4-5-20250929", "anthropic", 1));
        router.add_route(route("gpt-4o", "openai", 2));
        router.add_route(route("gemini-2.0-flash", "google", 3));

        router.mark_failed("claude-sonnet-4-5-20250929");
        router.mark_failed("gpt-4o");

        let decision = router.select_model(None).unwrap();
        assert_eq!(decision.selected_model, "gemini-2.0-flash");
    }

    #[test]
    fn test_all_failed_returns_none() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-sonnet-4-5-20250929", "anthropic", 1));
        router.add_route(route("gpt-4o", "openai", 2));

        router.mark_failed("claude-sonnet-4-5-20250929");
        router.mark_failed("gpt-4o");

        assert!(router.select_model(None).is_none());
    }

    #[test]
    fn test_mark_recovered() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-sonnet-4-5-20250929", "anthropic", 1));

        router.mark_failed("claude-sonnet-4-5-20250929");
        assert!(router.select_model(None).is_none());

        router.mark_recovered("claude-sonnet-4-5-20250929");
        let decision = router.select_model(None).unwrap();
        assert_eq!(decision.selected_model, "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn test_clear_failures() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-sonnet-4-5-20250929", "anthropic", 1));
        router.add_route(route("gpt-4o", "openai", 2));

        router.mark_failed("claude-sonnet-4-5-20250929");
        router.mark_failed("gpt-4o");
        assert_eq!(router.failed_count(), 2);

        router.clear_failures();
        assert_eq!(router.failed_count(), 0);

        let decision = router.select_model(None).unwrap();
        assert_eq!(decision.selected_model, "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn test_add_route_replaces_existing() {
        let mut router = ModelRouter::new();
        router.add_route(route("gpt-4o", "openai", 10));

        // Replace with different priority.
        router.add_route(route("gpt-4o", "openai", 1));

        assert_eq!(router.list_routes().len(), 1);
        assert_eq!(router.list_routes()[0].priority, 1);
    }

    #[test]
    fn test_list_routes() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-sonnet-4-5-20250929", "anthropic", 1));
        router.add_route(route("gpt-4o", "openai", 2));

        assert_eq!(router.list_routes().len(), 2);
    }

    #[test]
    fn test_complex_failover_chain() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-opus-4-6", "anthropic", 1));
        router.add_route(route("claude-sonnet-4-5-20250929", "anthropic", 2));
        router.add_route(route("gpt-4o", "openai", 3));
        router.add_route(disabled_route("gemini-2.0-flash", "google", 4));
        router.add_route(route("deepseek-chat", "deepseek", 5));

        // Fail the top two.
        router.mark_failed("claude-opus-4-6");
        router.mark_failed("claude-sonnet-4-5-20250929");

        // Gemini is disabled, so it should land on gpt-4o.
        let decision = router.select_model(Some("claude-opus-4-6")).unwrap();
        assert_eq!(decision.selected_model, "gpt-4o");
        assert!(decision.fallback_attempted);

        // Fail gpt-4o too -- should land on deepseek.
        router.mark_failed("gpt-4o");
        let decision = router.select_model(None).unwrap();
        assert_eq!(decision.selected_model, "deepseek-chat");

        // Fail everything -- should return None.
        router.mark_failed("deepseek-chat");
        assert!(router.select_model(None).is_none());
    }

    #[test]
    fn test_default_trait() {
        let router = ModelRouter::default();
        assert_eq!(router.list_routes().len(), 0);
        assert_eq!(router.failed_count(), 0);
    }

    #[test]
    fn test_fail_and_fallback_success() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-opus-4-6", "anthropic", 1));
        router.add_route(route("gpt-4o", "openai", 2));

        let (decision, event) = router.fail_and_fallback("claude-opus-4-6", "HTTP 503");
        let decision = decision.unwrap();
        let event = event.unwrap();

        assert_eq!(decision.selected_model, "gpt-4o");
        assert!(decision.fallback_attempted);
        assert_eq!(event.original_model, "claude-opus-4-6");
        assert_eq!(event.fallback_model, "gpt-4o");
        assert_eq!(event.reason, "HTTP 503");
        assert!(!event.timestamp.is_empty());
    }

    #[test]
    fn test_fail_and_fallback_exhausted() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-opus-4-6", "anthropic", 1));

        let (decision, event) = router.fail_and_fallback("claude-opus-4-6", "HTTP 503");
        assert!(decision.is_none());
        assert!(event.is_none());
    }

    #[test]
    fn test_fail_and_fallback_chain() {
        let mut router = ModelRouter::new();
        router.add_route(route("claude-opus-4-6", "anthropic", 1));
        router.add_route(route("gpt-4o", "openai", 2));
        router.add_route(route("gemini-2.0-flash", "google", 3));

        // First fallback
        let (decision, _) = router.fail_and_fallback("claude-opus-4-6", "rate limited");
        assert_eq!(decision.unwrap().selected_model, "gpt-4o");

        // Second fallback
        let (decision, _) = router.fail_and_fallback("gpt-4o", "service unavailable");
        assert_eq!(decision.unwrap().selected_model, "gemini-2.0-flash");

        // All exhausted
        let (decision, event) = router.fail_and_fallback("gemini-2.0-flash", "timeout");
        assert!(decision.is_none());
        assert!(event.is_none());
    }
}
