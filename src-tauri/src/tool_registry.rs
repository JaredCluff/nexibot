use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

/// Decision returned by a tool's permission check.
#[derive(Debug)]
pub enum PermissionDecision {
    Allow,
    Deny(String),
    Ask { reason: String, details: Option<String> },
}

/// Progress event emitted by a streaming tool.
#[derive(Debug, Clone)]
pub enum ToolProgress {
    Stdout(String),
    Stderr(String),
    Status(String),
    PartialResult(String),
}

/// Final result from a tool call.
#[derive(Debug)]
pub struct ToolResult {
    pub content: String,
    pub success: bool,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        ToolResult { content: content.into(), success: true }
    }
    pub fn err(content: impl Into<String>) -> Self {
        ToolResult { content: content.into(), success: false }
    }
}

/// Contextual state passed to every tool call.
/// Owns cloned/Arc'd references to app-wide state.
#[derive(Clone)]
pub struct ToolContext {
    pub session_key: String,
    pub agent_id: String,
    pub working_dir: std::path::PathBuf,
}

/// Core trait every new tool implements.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;

    /// Human-readable description injected into the system prompt.
    fn prompt_description(&self) -> String {
        format!("- {}: {}", self.name(), self.description())
    }

    /// True if this tool never modifies state (affects Smart approval mode).
    fn is_read_only(&self, _input: &Value) -> bool { false }

    /// True if this tool is safe to run concurrently with other tools.
    fn is_concurrency_safe(&self) -> bool { false }

    /// Decide whether to allow, deny, or ask before running.
    async fn check_permissions(&self, input: &Value, ctx: &ToolContext) -> PermissionDecision;

    /// Execute the tool synchronously (no streaming).
    async fn call(&self, input: Value, ctx: ToolContext) -> ToolResult;

    /// Execute with streaming progress. Default delegates to call().
    async fn call_streaming(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress_tx: tokio::sync::mpsc::UnboundedSender<ToolProgress>,
    ) -> ToolResult {
        self.call(input, ctx).await
    }
}

/// Registry mapping tool names to boxed trait objects.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        ToolRegistry { tools: HashMap::new() }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn all_tools(&self) -> impl Iterator<Item = &dyn Tool> {
        self.tools.values().map(|t| t.as_ref())
    }

    pub fn tool_definitions(&self) -> Vec<Value> {
        self.tools.values().map(|t| {
            serde_json::json!({
                "name": t.name(),
                "description": t.description(),
                "input_schema": t.input_schema(),
            })
        }).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echoes input" }
        fn input_schema(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {"msg": {"type": "string"}}})
        }
        async fn check_permissions(&self, _: &Value, _: &ToolContext) -> PermissionDecision {
            PermissionDecision::Allow
        }
        async fn call(&self, input: Value, _: ToolContext) -> ToolResult {
            ToolResult::ok(input["msg"].as_str().unwrap_or("").to_string())
        }
    }

    #[tokio::test]
    async fn test_registry_register_and_get() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        assert!(reg.get("echo").is_some());
        assert!(reg.get("missing").is_none());
    }

    #[tokio::test]
    async fn test_tool_call_returns_result() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        let tool = reg.get("echo").unwrap();
        let ctx = ToolContext {
            session_key: "s".into(),
            agent_id: "a".into(),
            working_dir: std::path::PathBuf::from("/tmp"),
        };
        let result = tool.call(serde_json::json!({"msg": "hello"}), ctx).await;
        assert_eq!(result.content, "hello");
        assert!(result.success);
    }

    #[test]
    fn test_tool_definitions_includes_registered() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        let defs = reg.tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["name"], "echo");
    }
}

#[cfg(test)]
mod integration_tests {
    #[tokio::test]
    async fn test_all_v090_tools_registered() {
        let expected_tools = &[
            "nexibot_file_read",
            "nexibot_file_edit",
            "nexibot_lsp",
            "nexibot_worktree",
            "nexibot_enter_plan_mode",
            "nexibot_exit_plan_mode",
            "nexibot_task_create",
            "nexibot_task_list",
            "nexibot_task_get",
            "nexibot_task_output",
            "nexibot_task_stop",
            "nexibot_send_message",
            "nexibot_notebook_edit",
        ];
        let mut registry = crate::tool_registry::ToolRegistry::new();
        let plan_state = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::tools::plan_mode::PlanModeState::default()
        ));
        crate::tools::register_all(&mut registry, plan_state, crate::config::LspConfig::default());
        for tool_name in expected_tools {
            assert!(
                registry.get(tool_name).is_some(),
                "Tool '{}' not registered", tool_name
            );
        }
    }
}
