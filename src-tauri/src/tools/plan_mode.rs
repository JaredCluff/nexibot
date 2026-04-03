use crate::tool_registry::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct PlanModeState {
    pub active: bool,
    pub plan_file_path: PathBuf,
    pub entered_at: Option<Instant>,
    pub description: String,
}

impl Default for PlanModeState {
    fn default() -> Self {
        PlanModeState {
            active: false,
            plan_file_path: PathBuf::default(),
            entered_at: None,
            description: String::new(),
        }
    }
}

/// READ-ONLY CONSTRAINT injected when plan mode is active.
pub const PLAN_MODE_CONSTRAINT: &str = r#"
## PLAN MODE ACTIVE

You are in plan mode. You MUST NOT:
- Edit any files (except the plan file)
- Run any commands that modify state
- Make commits or push code
- Delete or move files

You SHOULD:
- Explore the codebase using nexibot_file_read, nexibot_grep, nexibot_glob, nexibot_lsp
- Write your plan to the plan file
- Ask clarifying questions if needed
- Call nexibot_exit_plan_mode when your plan is complete and ready for review

Only nexibot_file_read, nexibot_grep, nexibot_glob, nexibot_lsp, nexibot_web_search, nexibot_fetch, and writing to the plan file are permitted.
"#;

// ─── Enter Plan Mode ──────────────────────────────────────────────────────────

pub struct EnterPlanModeTool {
    pub state: Arc<RwLock<PlanModeState>>,
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str { "nexibot_enter_plan_mode" }
    fn description(&self) -> &str {
        "Enter plan mode: explore the codebase and write a plan before making any changes. All write operations are blocked until the plan is approved."
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": { "type": "string", "description": "What you are planning to do" }
            }
        })
    }
    async fn check_permissions(&self, _: &Value, _: &ToolContext) -> PermissionDecision {
        PermissionDecision::Allow
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> ToolResult {
        {
            let state = self.state.read().await;
            if state.active {
                return ToolResult::err(format!(
                    "Already in plan mode (task: {}); call nexibot_exit_plan_mode first",
                    state.description
                ));
            }
        }
        let description = input["description"].as_str().unwrap_or("coding task").to_string();
        let plan_dir = ctx.working_dir.join(".nexibot").join("plans");
        let plan_file = plan_dir.join(format!("plan-{}.md", chrono::Utc::now().timestamp_millis()));

        let _ = tokio::fs::create_dir_all(&plan_dir).await;

        let mut state = self.state.write().await;
        *state = PlanModeState {
            active: true,
            plan_file_path: plan_file.clone(),
            entered_at: Some(Instant::now()),
            description: description.clone(),
        };

        ToolResult::ok(format!(
            "Plan mode activated.\nTask: {}\nPlan file: {}\n\n{}",
            description,
            plan_file.display(),
            PLAN_MODE_CONSTRAINT
        ))
    }
}

// ─── Exit Plan Mode ───────────────────────────────────────────────────────────

pub struct ExitPlanModeTool {
    pub state: Arc<RwLock<PlanModeState>>,
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str { "nexibot_exit_plan_mode" }
    fn description(&self) -> &str {
        "Submit your plan for user approval. Call this when your plan is complete. Execution begins only after approval."
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "plan_content": { "type": "string", "description": "The complete plan text (if not already written to plan file)" }
            }
        })
    }
    async fn check_permissions(&self, _: &Value, _: &ToolContext) -> PermissionDecision {
        // TODO: In headless mode (no observer), Ask falls through to auto-approval
        // which bypasses the human-review contract. Future: return Deny in headless context.
        PermissionDecision::Ask {
            reason: "Ready to exit plan mode and begin execution?".to_string(),
            details: Some("Review the plan and approve to proceed.".to_string()),
        }
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        // Acquire write lock immediately to atomically check and deactivate (fixes TOCTOU).
        let plan_file = {
            let mut state = self.state.write().await;
            if !state.active {
                return ToolResult::err("Plan mode is not active.");
            }
            let plan_file = state.plan_file_path.clone();
            state.active = false;
            plan_file
        };

        // Write inline plan_content to file if provided
        if let Some(content) = input["plan_content"].as_str() {
            if !content.is_empty() {
                let _ = tokio::fs::write(&plan_file, content).await;
            }
        }

        // Read the plan from file
        let plan_text = tokio::fs::read_to_string(&plan_file)
            .await
            .unwrap_or_else(|_| "(plan file not found or empty)".to_string());

        ToolResult::ok(format!(
            "Plan approved. Beginning execution.\n\n## Approved Plan\n\n{}\n\nPlan file: {}",
            plan_text, plan_file.display()
        ))
    }
}

// ─── Guard helpers ────────────────────────────────────────────────────────────

/// Returns Some(error message) if plan mode is active and tool is write-restricted.
/// Async version — takes the Arc<RwLock<PlanModeState>> and locks it.
/// Only used in tests; production code uses check_plan_mode_restriction_sync.
#[cfg(test)]
pub async fn check_plan_mode_restriction(
    state: &Arc<RwLock<PlanModeState>>,
    tool_name: &str,
    target_path: Option<&std::path::Path>,
) -> Option<String> {
    let s = state.read().await;
    if !s.active { return None; }

    // Read-only tools are always allowed
    const ALLOWED_IN_PLAN_MODE: &[&str] = &[
        "nexibot_file_read", "nexibot_grep", "nexibot_glob",
        "nexibot_lsp", "nexibot_exit_plan_mode",
    ];
    if ALLOWED_IN_PLAN_MODE.contains(&tool_name) { return None; }

    // The plan file itself is always writable
    if let Some(target) = target_path {
        if target == s.plan_file_path { return None; }
    }

    Some(format!(
        "BLOCKED: Plan mode is active. '{}' is not allowed in plan mode. \
         Explore the codebase and write your plan, then call nexibot_exit_plan_mode.",
        tool_name
    ))
}

/// Sync version — caller must already hold a read lock on PlanModeState.
pub fn check_plan_mode_restriction_sync(
    state: &PlanModeState,
    tool_name: &str,
    target_path: Option<&std::path::Path>,
) -> Option<String> {
    if !state.active { return None; }
    const ALLOWED: &[&str] = &[
        "nexibot_file_read", "nexibot_grep", "nexibot_glob",
        "nexibot_lsp", "nexibot_exit_plan_mode",
        // Legacy read-only tools also allowed:
        "nexibot_web_search", "nexibot_fetch",
    ];
    if ALLOWED.contains(&tool_name) { return None; }
    if let Some(target) = target_path {
        if target == state.plan_file_path { return None; }
    }
    Some(format!(
        "BLOCKED: Plan mode is active. '{}' is not allowed. \
         Complete your plan and call nexibot_exit_plan_mode.",
        tool_name
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state() -> Arc<RwLock<PlanModeState>> {
        Arc::new(RwLock::new(PlanModeState::default()))
    }

    #[tokio::test]
    async fn test_enter_plan_mode_activates_state() {
        let state = make_state();
        let tool = EnterPlanModeTool { state: state.clone() };
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            session_key: "s".into(), agent_id: "a".into(),
            working_dir: tmp.path().to_path_buf(),
        };
        let result = tool.call(serde_json::json!({"description": "fix login bug"}), ctx).await;
        assert!(result.success);
        assert!(state.read().await.active);
    }

    #[tokio::test]
    async fn test_exit_plan_mode_when_inactive_returns_error() {
        let state = make_state();
        let tool = ExitPlanModeTool { state };
        let ctx = ToolContext {
            session_key: "s".into(), agent_id: "a".into(),
            working_dir: PathBuf::from("/tmp"),
        };
        let result = tool.call(serde_json::json!({}), ctx).await;
        assert!(!result.success);
        assert!(result.content.contains("not active"));
    }

    #[tokio::test]
    async fn test_plan_mode_blocks_write_tools() {
        let state = make_state();
        {
            let mut s = state.write().await;
            s.active = true;
            s.plan_file_path = PathBuf::from("/tmp/plan.md");
        }
        let block = check_plan_mode_restriction(&state, "nexibot_file_edit", None).await;
        assert!(block.is_some());
        assert!(block.unwrap().contains("BLOCKED"));
    }

    #[tokio::test]
    async fn test_plan_mode_allows_read_tools() {
        let state = make_state();
        {
            let mut s = state.write().await;
            s.active = true;
            s.plan_file_path = PathBuf::from("/tmp/plan.md");
        }
        let block = check_plan_mode_restriction(&state, "nexibot_file_read", None).await;
        assert!(block.is_none());
    }

    #[tokio::test]
    async fn test_plan_mode_allows_plan_file_write() {
        let state = make_state();
        let plan_file = PathBuf::from("/tmp/plan.md");
        {
            let mut s = state.write().await;
            s.active = true;
            s.plan_file_path = plan_file.clone();
        }
        let block = check_plan_mode_restriction(&state, "nexibot_file_edit", Some(&plan_file)).await;
        assert!(block.is_none());
    }

    #[test]
    fn test_plan_mode_constraint_not_empty() {
        assert!(PLAN_MODE_CONSTRAINT.contains("plan mode"));
        assert!(PLAN_MODE_CONSTRAINT.contains("nexibot_exit_plan_mode"));
    }
}
