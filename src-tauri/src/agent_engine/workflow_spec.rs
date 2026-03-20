//! Shared workflow spec format used by both local DAG executor and the backend agent-engine.
//!
//! Local execution and remote delegation use the same JSON schema so specs are portable.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── On-failure policy ────────────────────────────────────────────────────────

/// What to do when a step fails.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureAction {
    Retry,
    Skip,
    Abort,
}

impl Default for FailureAction {
    fn default() -> Self {
        Self::Abort
    }
}

/// Retry / skip / abort policy attached to a step.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OnFailure {
    /// What to do when the step fails.
    pub action: FailureAction,
    /// Maximum number of retries before giving up (only meaningful for `retry`).
    #[serde(default)]
    pub max_retries: u32,
}

// ── Step definition ──────────────────────────────────────────────────────────

/// A single step in a workflow spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Stable identifier for this step (unique within the spec).
    pub id: String,
    /// Capability to invoke (e.g. "llm.complete", "kb.read", "code.execute").
    pub capability: String,
    /// Input parameters for the capability.  Supports `{{var}}` substitution.
    #[serde(default)]
    pub input: serde_json::Value,
    /// Steps that must complete before this one can run.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Optional condition expression.  The step is skipped when the expression
    /// evaluates to false.  Supports `{{var}}` substitution plus simple
    /// operators: `==`, `!=`, `>`, `<`, `&&`, `||`.
    #[serde(default)]
    pub condition: Option<String>,
    /// Store the step output under this variable name so downstream steps can
    /// reference it via `{{output_var}}`.
    #[serde(default)]
    pub output_var: Option<String>,
    /// Run this step in parallel with other parallel-flagged sibling steps
    /// that share the same dependencies.
    #[serde(default)]
    pub parallel: bool,
    /// Repeat this step once for each item in the list stored in `{{var}}`.
    /// Each iteration receives `{{item}}` and `{{index}}` substitutions.
    #[serde(default)]
    pub loop_over: Option<String>,
    /// Failure handling policy.
    #[serde(default)]
    pub on_failure: OnFailure,
}

// ── Workflow spec ────────────────────────────────────────────────────────────

/// A complete workflow specification, portable between local and remote execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSpec {
    /// Stable identifier for the spec (UUID generated on save, or provided by the user).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Ordered list of steps.  Dependencies are declared on each step via
    /// `depends_on`; the executor resolves ordering automatically.
    pub steps: Vec<WorkflowStep>,
    /// Input variables injected into the execution context before any step runs.
    #[serde(default)]
    pub inputs: HashMap<String, serde_json::Value>,
}

// ── Run status (returned from run_agent) ────────────────────────────────────

/// Status of a running or completed workflow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunStatus {
    pub run_id: String,
    /// "pending" | "running" | "completed" | "failed" | "cancelled"
    pub status: String,
    /// Per-step results keyed by step ID.
    pub step_results: HashMap<String, StepResult>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
}

/// Outcome of a single step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step_id: String,
    pub status: String,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

// ── Summary for list_agents ──────────────────────────────────────────────────

/// Summary of a saved agent definition (for the list_agents command).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub agent_id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

// ── Frontend event payload ───────────────────────────────────────────────────

/// Tauri event emitted for each step lifecycle change during local execution.
#[derive(Debug, Clone, Serialize)]
pub struct AgentStepEvent {
    pub run_id: String,
    pub step_id: String,
    /// "started" | "completed" | "failed" | "skipped"
    pub status: String,
    /// First 500 chars of the step output.
    pub output_preview: Option<String>,
    pub error: Option<String>,
    /// Unix milliseconds.
    pub timestamp: i64,
}

// ── Condition evaluator ──────────────────────────────────────────────────────

/// Evaluate a simple condition expression after applying `{{var}}` substitution.
///
/// Supported operators: `==`, `!=`, `>`, `<`, `&&`, `||`.
/// Values are compared as strings (or parsed as f64 for numeric comparisons).
///
/// Returns `true` if the condition is satisfied, `false` otherwise.
pub fn evaluate_condition(expr: &str, ctx: &HashMap<String, serde_json::Value>) -> bool {
    let substituted = substitute_vars(expr, ctx);
    eval_expr(substituted.trim())
}

/// Replace every `{{var}}` token in `template` with the string representation
/// of the matching value in `ctx`.  Unknown variables are left as-is.
pub fn substitute_vars(template: &str, ctx: &HashMap<String, serde_json::Value>) -> String {
    let mut result = template.to_string();
    for (key, val) in ctx {
        let placeholder = format!("{{{{{}}}}}", key);
        let replacement = match val {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => "null".to_string(),
            other => other.to_string(),
        };
        result = result.replace(&placeholder, &replacement);
    }
    result
}

/// Recursively evaluate a substituted boolean expression.
fn eval_expr(expr: &str) -> bool {
    // Handle `||` (lowest precedence)
    if let Some((lhs, rhs)) = split_at_operator(expr, "||") {
        return eval_expr(lhs.trim()) || eval_expr(rhs.trim());
    }
    // Handle `&&`
    if let Some((lhs, rhs)) = split_at_operator(expr, "&&") {
        return eval_expr(lhs.trim()) && eval_expr(rhs.trim());
    }
    // Comparison operators
    for op in &["==", "!=", ">=", "<=", ">", "<"] {
        if let Some((lhs, rhs)) = split_at_operator(expr, op) {
            let l = lhs.trim().trim_matches('"').trim_matches('\'');
            let r = rhs.trim().trim_matches('"').trim_matches('\'');
            return match *op {
                "==" => l == r,
                "!=" => l != r,
                ">" => parse_f64(l) > parse_f64(r),
                "<" => parse_f64(l) < parse_f64(r),
                ">=" => parse_f64(l) >= parse_f64(r),
                "<=" => parse_f64(l) <= parse_f64(r),
                _ => false,
            };
        }
    }
    // Bare boolean literals
    match expr.to_lowercase().as_str() {
        "true" | "1" | "yes" => true,
        // Unrecognised non-empty strings default to false (conservative / secure).
        // Returning true for arbitrary strings would allow context variables whose
        // substituted values happen to contain operators (e.g. "ok || true") to
        // bypass condition guards unintentionally.
        _ => false,
    }
}

/// Find the first occurrence of `op` in `expr` that is not inside quotes,
/// returning `(left, right)` slices excluding the operator.
fn split_at_operator<'a>(expr: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let bytes = expr.as_bytes();
    let op_bytes = op.as_bytes();
    let op_len = op_bytes.len();
    let mut in_double = false;
    let mut in_single = false;

    let mut i = 0usize;
    while i + op_len <= bytes.len() {
        match bytes[i] {
            b'"' if !in_single => in_double = !in_double,
            b'\'' if !in_double => in_single = !in_single,
            _ => {}
        }
        if !in_double && !in_single && bytes[i..i + op_len] == *op_bytes {
            return Some((&expr[..i], &expr[i + op_len..]));
        }
        i += 1;
    }
    None
}

fn parse_f64(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(0.0)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(pairs: &[(&str, &str)]) -> HashMap<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn test_substitute_vars() {
        let c = ctx(&[("name", "Alice"), ("age", "30")]);
        assert_eq!(
            substitute_vars("Hello {{name}}, you are {{age}}", &c),
            "Hello Alice, you are 30"
        );
    }

    #[test]
    fn test_eval_equality() {
        let c = ctx(&[("status", "ok")]);
        assert!(evaluate_condition("{{status}} == ok", &c));
        assert!(!evaluate_condition("{{status}} != ok", &c));
    }

    #[test]
    fn test_eval_numeric() {
        let c = ctx(&[("count", "5")]);
        assert!(evaluate_condition("{{count}} > 3", &c));
        assert!(!evaluate_condition("{{count}} < 3", &c));
    }

    #[test]
    fn test_eval_and_or() {
        let c = ctx(&[("a", "1"), ("b", "2")]);
        assert!(evaluate_condition("{{a}} == 1 && {{b}} == 2", &c));
        assert!(evaluate_condition("{{a}} == 0 || {{b}} == 2", &c));
        assert!(!evaluate_condition("{{a}} == 0 && {{b}} == 2", &c));
    }

    #[test]
    fn test_eval_bare_bool() {
        let c = HashMap::new();
        assert!(evaluate_condition("true", &c));
        assert!(!evaluate_condition("false", &c));
        assert!(!evaluate_condition("", &c));
    }
}
