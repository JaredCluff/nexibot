//! Local WorkflowSpec executor.
//!
//! Executes a WorkflowSpec in-process, supporting:
//! - `condition` evaluation (skip steps when conditions are false)
//! - `output_var` (store step results in the execution context)
//! - `on_failure` (retry / skip / abort)
//! - `parallel` (gather sibling steps)
//! - `loop_over` (repeat a step for each item in a list variable)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::{error, info, warn};

use super::capability_dispatch::{ExecutionContext, LocalCapabilityDispatch};
use super::workflow_spec::{AgentRunStatus, FailureAction, StepResult, WorkflowSpec, WorkflowStep, evaluate_condition, substitute_vars};

// ── Run state ────────────────────────────────────────────────────────────────

/// Status of a workflow step during execution.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
enum StepStatus {
    Pending,
    Skipped,
    Running,
    Completed,
    Failed,
}

impl StepStatus {
    #[allow(dead_code)]
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Skipped => "skipped",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, Self::Skipped | Self::Completed | Self::Failed)
    }
}

// ── Executor ─────────────────────────────────────────────────────────────────

/// In-process executor for a `WorkflowSpec`.
pub struct WorkflowExecutor {
    dispatch: Arc<LocalCapabilityDispatch>,
}

impl WorkflowExecutor {
    pub fn new(dispatch: Arc<LocalCapabilityDispatch>) -> Self {
        Self { dispatch }
    }

    /// Execute a workflow spec and return a final `AgentRunStatus`.
    pub async fn execute(
        &self,
        run_id: &str,
        spec: &WorkflowSpec,
    ) -> AgentRunStatus {
        let start = Instant::now();
        let mut ctx = ExecutionContext::from_inputs(&spec.inputs);
        let mut step_statuses: HashMap<String, StepStatus> = spec
            .steps
            .iter()
            .map(|s| (s.id.clone(), StepStatus::Pending))
            .collect();
        let mut step_results: HashMap<String, StepResult> = HashMap::new();
        let mut final_error: Option<String> = None;

        // Topological round-based execution
        let max_rounds = spec.steps.len() + 1;
        for _round in 0..max_rounds {
            // Find steps that are ready to run
            let ready: Vec<&WorkflowStep> = spec
                .steps
                .iter()
                .filter(|s| {
                    if step_statuses[&s.id] != StepStatus::Pending {
                        return false; // Already processed
                    }
                    // All dependencies must be in a terminal state that is not Failed
                    // (unless the failed dep is skip-policy)
                    s.depends_on.iter().all(|dep| {
                        matches!(
                            step_statuses.get(dep),
                            Some(StepStatus::Completed) | Some(StepStatus::Skipped)
                        )
                    })
                })
                .collect();

            if ready.is_empty() {
                // Check if all steps are terminal
                if step_statuses
                    .values()
                    .all(|s| s.is_terminal())
                {
                    break;
                }
                // Check for deadlock (pending steps with failed deps)
                let blocked: Vec<&str> = spec
                    .steps
                    .iter()
                    .filter(|s| step_statuses[&s.id] == StepStatus::Pending)
                    .map(|s| s.id.as_str())
                    .collect();
                if !blocked.is_empty() {
                    let msg = format!(
                        "Workflow deadlock: steps {:?} are blocked (dependency failed or cycle)",
                        blocked
                    );
                    error!("[WF_EXECUTOR] {}", msg);
                    final_error = Some(msg.clone());
                    // Mark blocked steps as failed
                    for id in &blocked {
                        step_statuses.insert(id.to_string(), StepStatus::Failed);
                        step_results.insert(
                            id.to_string(),
                            StepResult {
                                step_id: id.to_string(),
                                status: "failed".to_string(),
                                output: None,
                                error: Some(msg.clone()),
                                duration_ms: 0,
                            },
                        );
                    }
                }
                break;
            }

            // Partition ready into parallel and serial groups.
            // All steps with `parallel=true` that are ready at the same time run concurrently.
            // Serial steps run one at a time (still in this round but sequentially).
            let (parallel, serial): (Vec<_>, Vec<_>) =
                ready.into_iter().partition(|s| s.parallel);

            // Execute parallel batch
            if !parallel.is_empty() {
                let outcomes = self
                    .execute_parallel(run_id, parallel, &ctx)
                    .await;
                for (id, outcome) in outcomes {
                    let _aborted = outcome.error.is_some()
                        && step_results
                            .get(&id)
                            .map(|r| r.status == "failed")
                            .unwrap_or(false);
                    step_statuses.insert(id.clone(), outcome_status(&outcome));
                    if let Some(ref var) = spec
                        .steps
                        .iter()
                        .find(|s| s.id == id)
                        .and_then(|s| s.output_var.as_ref())
                    {
                        if let Some(ref out) = outcome.output {
                            ctx.set(var, out.clone());
                        }
                    }
                    // Check abort policy
                    if outcome.status == "failed" {
                        if let Some(step) = spec.steps.iter().find(|s| s.id == id) {
                            if step.on_failure.action == FailureAction::Abort {
                                final_error = outcome.error.clone();
                            }
                        }
                    }
                    step_results.insert(id, outcome);
                }
            }

            // Execute serial steps
            for step in serial {
                let outcome = self.execute_step(run_id, step, &ctx).await;
                let status = outcome_status(&outcome);
                if let Some(ref var) = step.output_var {
                    if let Some(ref out) = outcome.output {
                        ctx.set(var, out.clone());
                    }
                }
                if status == StepStatus::Failed && step.on_failure.action == FailureAction::Abort {
                    final_error = outcome.error.clone();
                    step_statuses.insert(step.id.clone(), StepStatus::Failed);
                    step_results.insert(step.id.clone(), outcome);
                    break;
                }
                step_statuses.insert(step.id.clone(), status);
                step_results.insert(step.id.clone(), outcome);
            }

            if final_error.is_some() {
                break;
            }
        }

        let overall_status = if final_error.is_some() {
            "failed"
        } else if step_results.values().any(|r| r.status == "failed") {
            "failed"
        } else {
            "completed"
        };

        let now_ms = chrono::Utc::now().timestamp_millis();
        let start_ms = now_ms - start.elapsed().as_millis() as i64;

        AgentRunStatus {
            run_id: run_id.to_string(),
            status: overall_status.to_string(),
            step_results,
            started_at: Some(start_ms),
            completed_at: Some(now_ms),
            error: final_error,
        }
    }

    // ── Execute a single step ─────────────────────────────────────────────────

    async fn execute_step(
        &self,
        run_id: &str,
        step: &WorkflowStep,
        ctx: &ExecutionContext,
    ) -> StepResult {
        let step_start = Instant::now();

        // Evaluate condition
        if let Some(ref cond) = step.condition {
            let substituted = substitute_vars(cond, ctx.vars());
            if !evaluate_condition(&substituted, ctx.vars()) {
                info!(
                    "[WF_EXECUTOR] Step '{}' skipped (condition false: '{}')",
                    step.id, cond
                );
                return StepResult {
                    step_id: step.id.clone(),
                    status: "skipped".to_string(),
                    output: None,
                    error: None,
                    duration_ms: step_start.elapsed().as_millis() as u64,
                };
            }
        }

        // Handle loop_over
        if let Some(ref loop_var) = step.loop_over {
            return self
                .execute_loop(run_id, step, loop_var, ctx, step_start)
                .await;
        }

        // Substitute input vars
        let input = substitute_value(&step.input, ctx.vars());

        // Execute with retry
        self.execute_with_retry(run_id, step, input, step_start)
            .await
    }

    // ── Loop execution ────────────────────────────────────────────────────────

    async fn execute_loop(
        &self,
        _run_id: &str,
        step: &WorkflowStep,
        loop_var: &str,
        ctx: &ExecutionContext,
        step_start: Instant,
    ) -> StepResult {
        // Resolve the list variable
        let list = match ctx.vars().get(loop_var) {
            Some(Value::Array(arr)) => arr.clone(),
            Some(other) => {
                let msg = format!(
                    "loop_over variable '{}' is not an array (got {:?})",
                    loop_var,
                    other.type_str()
                );
                return StepResult {
                    step_id: step.id.clone(),
                    status: "failed".to_string(),
                    output: None,
                    error: Some(msg),
                    duration_ms: step_start.elapsed().as_millis() as u64,
                };
            }
            None => {
                let msg = format!("loop_over variable '{}' not found in context", loop_var);
                return StepResult {
                    step_id: step.id.clone(),
                    status: "failed".to_string(),
                    output: None,
                    error: Some(msg),
                    duration_ms: step_start.elapsed().as_millis() as u64,
                };
            }
        };

        let mut iteration_outputs = Vec::new();
        let mut any_failed = false;

        for (idx, item) in list.iter().enumerate() {
            // Inject {{item}} and {{index}} into a local context overlay
            let mut loop_ctx_vars = ctx.vars().clone();
            loop_ctx_vars.insert("item".to_string(), item.clone());
            loop_ctx_vars.insert(
                "index".to_string(),
                Value::Number(serde_json::Number::from(idx as u64)),
            );
            let input = substitute_value(&step.input, &loop_ctx_vars);

            match self.dispatch.invoke(&step.capability, input).await {
                Ok(out) => iteration_outputs.push(out),
                Err(e) => {
                    warn!(
                        "[WF_EXECUTOR] Loop step '{}' iteration {} failed: {}",
                        step.id, idx, e
                    );
                    match step.on_failure.action {
                        FailureAction::Abort => {
                            let msg = format!("Loop iteration {} failed: {}", idx, e);
                            return StepResult {
                                step_id: step.id.clone(),
                                status: "failed".to_string(),
                                output: None,
                                error: Some(msg),
                                duration_ms: step_start.elapsed().as_millis() as u64,
                            };
                        }
                        FailureAction::Skip => {
                            iteration_outputs.push(Value::Null);
                        }
                        FailureAction::Retry => {
                            // Retry this iteration up to max_retries times with exponential backoff
                            let max_retries = step.on_failure.max_retries;
                            let mut succeeded = false;
                            for attempt in 1..=max_retries {
                                let delay_ms = 1000u64 * 2u64.pow(attempt.saturating_sub(1));
                                warn!("[WF_EXECUTOR] Loop step '{}' iteration {} retry {}/{} in {}ms", step.id, idx, attempt, max_retries, delay_ms);
                                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                                let retry_input = substitute_value(&step.input, &loop_ctx_vars);
                                match self.dispatch.invoke(&step.capability, retry_input).await {
                                    Ok(out) => {
                                        iteration_outputs.push(out);
                                        succeeded = true;
                                        break;
                                    }
                                    Err(re) => {
                                        warn!("[WF_EXECUTOR] Loop step '{}' iteration {} retry {}/{} failed: {}", step.id, idx, attempt, max_retries, re);
                                    }
                                }
                            }
                            if !succeeded {
                                any_failed = true;
                                iteration_outputs.push(Value::Null);
                            }
                        }
                    }
                }
            }
        }

        let output = Value::Array(iteration_outputs);

        StepResult {
            step_id: step.id.clone(),
            status: if any_failed { "failed" } else { "completed" }.to_string(),
            output: Some(output),
            error: None,
            duration_ms: step_start.elapsed().as_millis() as u64,
        }
    }

    // ── Single invocation with retry ──────────────────────────────────────────

    async fn execute_with_retry(
        &self,
        _run_id: &str,
        step: &WorkflowStep,
        input: Value,
        step_start: Instant,
    ) -> StepResult {
        let max_retries = match step.on_failure.action {
            FailureAction::Retry => step.on_failure.max_retries,
            _ => 0,
        };

        let mut last_error = String::new();
        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay_ms = 1000u64 * 2u64.pow(attempt.saturating_sub(1));
                warn!(
                    "[WF_EXECUTOR] Retrying step '{}' (attempt {}/{}) in {}ms",
                    step.id, attempt, max_retries, delay_ms
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            match self.dispatch.invoke(&step.capability, input.clone()).await {
                Ok(output) => {
                    return StepResult {
                        step_id: step.id.clone(),
                        status: "completed".to_string(),
                        output: Some(output),
                        error: None,
                        duration_ms: step_start.elapsed().as_millis() as u64,
                    };
                }
                Err(e) => {
                    last_error = e.to_string();
                }
            }
        }

        // All attempts exhausted
        let final_status = match step.on_failure.action {
            FailureAction::Skip => "skipped",
            _ => "failed",
        };

        StepResult {
            step_id: step.id.clone(),
            status: final_status.to_string(),
            output: None,
            error: Some(last_error),
            duration_ms: step_start.elapsed().as_millis() as u64,
        }
    }

    // ── Parallel batch execution ──────────────────────────────────────────────

    async fn execute_parallel(
        &self,
        _run_id: &str,
        steps: Vec<&WorkflowStep>,
        ctx: &ExecutionContext,
    ) -> Vec<(String, StepResult)> {
        let mut handles: Vec<(String, tokio::task::JoinHandle<(String, StepResult)>)> = Vec::new();

        for step in steps {
            let dispatch = self.dispatch.clone();
            let step_id = step.id.clone();
            let step = step.clone();
            let ctx_vars = ctx.vars().clone();

            handles.push((step_id, tokio::spawn(async move {
                let step_start = Instant::now();
                let _exec_ctx = ExecutionContext::from_inputs(&ctx_vars);
                let input = substitute_value(&step.input, &ctx_vars);

                let max_retries = match step.on_failure.action {
                    FailureAction::Retry => step.on_failure.max_retries,
                    _ => 0,
                };

                let mut last_error = String::new();
                let mut result_output: Option<Value> = None;
                let mut succeeded = false;

                for attempt in 0..=max_retries {
                    if attempt > 0 {
                        let delay_ms = 1000u64 * 2u64.pow(attempt.saturating_sub(1));
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                    match dispatch.invoke(&step.capability, input.clone()).await {
                        Ok(out) => {
                            result_output = Some(out);
                            succeeded = true;
                            break;
                        }
                        Err(e) => {
                            last_error = e.to_string();
                        }
                    }
                }

                let final_status = if succeeded {
                    "completed"
                } else {
                    match step.on_failure.action {
                        FailureAction::Skip => "skipped",
                        _ => "failed",
                    }
                };

                (
                    step.id.clone(),
                    StepResult {
                        step_id: step.id.clone(),
                        status: final_status.to_string(),
                        output: result_output,
                        error: if succeeded { None } else { Some(last_error) },
                        duration_ms: step_start.elapsed().as_millis() as u64,
                    },
                )
            })));
        }

        let mut results = Vec::new();
        for (step_id, handle) in handles {
            match handle.await {
                Ok(r) => results.push(r),
                Err(e) => {
                    error!("[WF_EXECUTOR] Parallel step '{}' join error (task panicked or cancelled): {}", step_id, e);
                    results.push((
                        step_id.clone(),
                        StepResult {
                            step_id,
                            status: "failed".to_string(),
                            output: None,
                            error: Some(format!("task panicked or was cancelled: {}", e)),
                            duration_ms: 0,
                        },
                    ));
                }
            }
        }
        results
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn outcome_status(r: &StepResult) -> StepStatus {
    match r.status.as_str() {
        "completed" => StepStatus::Completed,
        "skipped" => StepStatus::Skipped,
        _ => StepStatus::Failed,
    }
}

/// Recursively substitute `{{var}}` in all string values of a JSON tree.
fn substitute_value(
    val: &Value,
    ctx: &HashMap<String, Value>,
) -> Value {
    match val {
        Value::String(s) => Value::String(substitute_vars(s, ctx)),
        Value::Object(map) => {
            let new_map: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), substitute_value(v, ctx)))
                .collect();
            Value::Object(new_map)
        }
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| substitute_value(v, ctx)).collect())
        }
        other => other.clone(),
    }
}

/// Render a preview (first N chars) of a JSON value.
#[allow(dead_code)]
fn preview_value(val: &Value, max_chars: usize) -> String {
    let s = match val {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.len() > max_chars {
        format!("{}...", &s[..max_chars])
    } else {
        s
    }
}

// Extension trait to get type name from Value
trait ValueExt {
    fn type_str(&self) -> &'static str;
}
impl ValueExt for Value {
    fn type_str(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }
}
