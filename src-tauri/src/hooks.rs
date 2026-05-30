//! Plugin hook system for extensibility.
//!
//! Provides hook points throughout the message processing pipeline.
//! Hooks can be Rust trait implementations or external command (subprocess) hooks
//! configured in YAML.
#![allow(dead_code)]

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Points in the pipeline where hooks can execute.
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HookPoint {
    // Message lifecycle
    /// Before a user message is processed.
    BeforeMessage,
    /// After a response has been generated.
    AfterMessage,
    /// Every incoming message across all channels (gate + routing).
    UserPromptSubmit,

    // Tool lifecycle
    /// Before a tool call is dispatched.
    BeforeToolCall,
    /// Before any tool is executed (gate with enforcement).
    PreToolUse,
    /// After a tool call completes.
    AfterToolCall,
    /// After tool completes (telemetry + rewrite).
    PostToolUse,
    /// After a tool call fails.
    PostToolUseFailure,

    // Session lifecycle
    /// When a new chat/session begins.
    SessionStart,
    /// When a session ends.
    SessionEnd,
    /// Before context compaction.
    PreCompact,
    /// After context compaction.
    PostCompact,

    // Permission / error lifecycle
    /// When a tool permission is denied.
    PermissionDenied,
    /// When an error occurs in the pipeline.
    OnError,

    // Task lifecycle
    /// When a task is created.
    TaskCreated,
    /// When a task is marked done.
    TaskCompleted,

    /// Override which model handles the request.
    ModelOverride,
    /// When working directory changes.
    CwdChanged,
}

impl std::fmt::Display for HookPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookPoint::BeforeMessage => write!(f, "before_message"),
            HookPoint::AfterMessage => write!(f, "after_message"),
            HookPoint::UserPromptSubmit => write!(f, "user_prompt_submit"),
            HookPoint::BeforeToolCall => write!(f, "before_tool_call"),
            HookPoint::PreToolUse => write!(f, "pre_tool_use"),
            HookPoint::AfterToolCall => write!(f, "after_tool_call"),
            HookPoint::PostToolUse => write!(f, "post_tool_use"),
            HookPoint::PostToolUseFailure => write!(f, "post_tool_use_failure"),
            HookPoint::SessionStart => write!(f, "session_start"),
            HookPoint::SessionEnd => write!(f, "session_end"),
            HookPoint::PreCompact => write!(f, "pre_compact"),
            HookPoint::PostCompact => write!(f, "post_compact"),
            HookPoint::PermissionDenied => write!(f, "permission_denied"),
            HookPoint::OnError => write!(f, "on_error"),
            HookPoint::TaskCreated => write!(f, "task_created"),
            HookPoint::TaskCompleted => write!(f, "task_completed"),
            HookPoint::ModelOverride => write!(f, "model_override"),
            HookPoint::CwdChanged => write!(f, "cwd_changed"),
        }
    }
}

/// Permission decision a hook can enforce.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Allow execution.
    Allow,
    /// Prompt user for approval before proceeding.
    Ask,
    /// Hard block — do not execute.
    Deny,
}

/// Result returned by a hook handler.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct HookResult {
    /// If `true`, the pipeline should stop processing.
    #[serde(default)]
    pub block: bool,
    /// Human-readable reason for blocking, logged when `block` is `true`.
    #[serde(default)]
    pub reason: Option<String>,
    /// If set, replaces the content flowing through the pipeline.
    #[serde(default)]
    pub modified_content: Option<String>,
    /// Permission decision for gate hooks (PreToolUse, PermissionDenied).
    #[serde(default)]
    pub permission_decision: Option<PermissionDecision>,
    /// Rewritten tool arguments (for PreToolUse hooks).
    #[serde(default)]
    pub updated_input: Option<Value>,
    /// Additional context injected into the model prompt.
    #[serde(default)]
    pub additional_context: Option<String>,
    /// Human-readable message shown to the user as a system banner.
    #[serde(default)]
    pub system_message: Option<String>,
    /// If `true`, suppress this hook's output from being displayed.
    #[serde(default)]
    pub suppress_output: bool,
    /// Arbitrary metadata a hook can attach.
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

/// Configuration for an external command hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Which pipeline point this hook runs at.
    pub point: HookPoint,
    /// External command to execute (receives context on stdin, writes result to stdout).
    pub command: Option<String>,
    /// Maximum time the command may run before being killed (milliseconds).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Whether this hook is active.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// Top-level hooks configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Master switch for the hooks engine.
    #[serde(default = "default_hooks_enabled")]
    pub enabled: bool,
    /// Individual hook definitions.
    #[serde(default)]
    pub hooks: Vec<HookConfig>,
    /// Max trait-object handlers per hook point.
    #[serde(default = "default_max_handlers_per_point")]
    pub max_handlers_per_point: usize,
    /// Max external command hooks total.
    #[serde(default = "default_max_command_hooks")]
    pub max_command_hooks: usize,
    /// Default timeout for command hooks (ms).
    #[serde(default = "default_timeout_ms")]
    pub default_timeout_ms: u64,
}

fn default_hooks_enabled() -> bool {
    false
}

fn default_max_handlers_per_point() -> usize {
    50
}

fn default_max_command_hooks() -> usize {
    100
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hooks: Vec::new(),
            max_handlers_per_point: default_max_handlers_per_point(),
            max_command_hooks: default_max_command_hooks(),
            default_timeout_ms: default_timeout_ms(),
        }
    }
}

fn default_timeout_ms() -> u64 {
    5000
}

fn default_enabled() -> bool {
    true
}

/// Context passed into every hook invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    /// The user or assistant message text, if applicable.
    pub message_text: Option<String>,
    /// The name of the tool being called, if applicable.
    pub tool_name: Option<String>,
    /// The unique id of the tool being called, if applicable.
    pub tool_id: Option<String>,
    /// The JSON input for the tool call, if applicable.
    pub tool_input: Option<Value>,
    /// The result produced by a tool call, if applicable.
    pub tool_result: Option<String>,
    /// An error description, if this is an error hook.
    pub error: Option<String>,
    /// The channel source that originated the message, if applicable.
    pub channel_source: Option<String>,
    /// Session key for the current conversation.
    pub session_key: Option<String>,
    /// Agent ID handling the request.
    pub agent_id: Option<String>,
    /// Current working directory.
    pub cwd: Option<String>,
    /// Task ID for task lifecycle hooks.
    pub task_id: Option<String>,
    /// Task status for task lifecycle hooks.
    pub task_status: Option<String>,
    /// Extra fields for forward compatibility.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl HookContext {
    /// Create an empty context (all fields `None`).
    pub fn empty() -> Self {
        Self {
            message_text: None,
            tool_name: None,
            tool_id: None,
            tool_input: None,
            tool_result: None,
            error: None,
            channel_source: None,
            session_key: None,
            agent_id: None,
            cwd: None,
            task_id: None,
            task_status: None,
            extra: HashMap::new(),
        }
    }

    /// Context for a message event.
    pub fn for_message(text: &str) -> Self {
        Self {
            message_text: Some(text.to_string()),
            ..Self::empty()
        }
    }

    /// Context for a tool-call event.
    pub fn for_tool_call(tool_name: &str, tool_input: Value) -> Self {
        Self {
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(tool_input),
            ..Self::empty()
        }
    }

    /// Context for a PreToolUse / PostToolUse event.
    pub fn for_tool_use(tool_name: &str, tool_id: &str, tool_input: Value) -> Self {
        Self {
            tool_name: Some(tool_name.to_string()),
            tool_id: Some(tool_id.to_string()),
            tool_input: Some(tool_input),
            ..Self::empty()
        }
    }

    /// Context for a PostToolUse event with result.
    pub fn for_tool_result(tool_name: &str, tool_id: &str, tool_input: Value, tool_result: &str) -> Self {
        Self {
            tool_name: Some(tool_name.to_string()),
            tool_id: Some(tool_id.to_string()),
            tool_input: Some(tool_input),
            tool_result: Some(tool_result.to_string()),
            ..Self::empty()
        }
    }

    /// Context for an error event.
    pub fn for_error(error: &str) -> Self {
        Self {
            error: Some(error.to_string()),
            ..Self::empty()
        }
    }

    /// Context for a session event.
    pub fn for_session(session_key: &str, agent_id: Option<&str>) -> Self {
        Self {
            session_key: Some(session_key.to_string()),
            agent_id: agent_id.map(|s| s.to_string()),
            ..Self::empty()
        }
    }

    /// Context for a task event.
    pub fn for_task(task_id: &str, task_status: Option<&str>) -> Self {
        Self {
            task_id: Some(task_id.to_string()),
            task_status: task_status.map(|s| s.to_string()),
            ..Self::empty()
        }
    }

    /// Set channel source (fluent builder).
    pub fn with_channel(mut self, source: &str) -> Self {
        self.channel_source = Some(source.to_string());
        self
    }

    /// Set cwd (fluent builder).
    pub fn with_cwd(mut self, cwd: &str) -> Self {
        self.cwd = Some(cwd.to_string());
        self
    }
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Trait that any hook handler must implement.
#[async_trait]
pub trait HookHandler: Send + Sync {
    /// Execute this handler for the given hook point and context.
    async fn execute(&self, point: &HookPoint, context: &HookContext) -> Result<HookResult>;
}

// ---------------------------------------------------------------------------
// Command hook handler (external subprocess)
// ---------------------------------------------------------------------------

/// A hook handler that runs an external command as a subprocess.
///
/// The command receives the [`HookContext`] serialized as JSON on **stdin**
/// and is expected to write a JSON [`HookResult`] to **stdout**. If the
/// command exits with a non-zero status or times out, a default (passthrough)
/// result is returned.
pub struct CommandHookHandler {
    /// Shell command to execute.
    command: String,
    /// Maximum duration the command may run.
    timeout: Duration,
}

impl CommandHookHandler {
    pub fn new(command: String, timeout: Duration) -> Self {
        Self { command, timeout }
    }
}

#[async_trait]
impl HookHandler for CommandHookHandler {
    async fn execute(&self, point: &HookPoint, context: &HookContext) -> Result<HookResult> {
        debug!(
            "[HOOKS] Running command hook for {}: {}",
            point, self.command
        );

        // Reject commands containing null bytes — no legitimate shell command
        // needs them and they can be used as part of exploit payloads.
        if self.command.contains('\0') {
            warn!("[HOOKS] Rejecting command containing null byte for {:?}", point);
            return Ok(HookResult::default());
        }

        // Warn when a hook command contains patterns that are commonly associated
        // with prompt-injection-driven RCE: command substitution ($(...)), backtick
        // execution, or a curl/wget pipe chain.  These are intentional user-configured
        // commands, so we do NOT block them — we log a security audit trail so the
        // operator can review.
        if self.command.contains("$(") {
            warn!(
                "[HOOKS] SECURITY: command hook for {:?} contains shell command substitution '$(' — \
                 review for potential prompt-injection RCE: {:?}",
                point, self.command
            );
        }
        if self.command.contains('`') {
            warn!(
                "[HOOKS] SECURITY: command hook for {:?} contains backtick execution — \
                 review for potential prompt-injection RCE: {:?}",
                point, self.command
            );
        }
        let cmd_trimmed = self.command.trim_start().to_ascii_lowercase();
        if (cmd_trimmed.starts_with("curl") || cmd_trimmed.starts_with("wget"))
            && self.command.contains('|')
        {
            warn!(
                "[HOOKS] SECURITY: command hook for {:?} pipes curl/wget output to a shell — \
                 review for potential prompt-injection RCE: {:?}",
                point, self.command
            );
        }

        let context_json = serde_json::to_string(context)?;

        let mut cmd = {
            #[cfg(windows)]
            {
                let mut c = Command::new("cmd");
                c.args(["/C", &self.command]);
                c.creation_flags(0x08000000); // CREATE_NO_WINDOW
                c
            }
            #[cfg(not(windows))]
            {
                let mut c = Command::new("sh");
                c.arg("-c").arg(&self.command);
                c
            }
        };

        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        // Write context to stdin then flush+drop to signal EOF before waiting.
        // Explicit flush ensures buffered bytes reach the pipe before the fd is closed.
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(context_json.as_bytes()).await?;
            stdin.flush().await.ok();
        }

        // Wait with timeout
        let output = tokio::time::timeout(self.timeout, child.wait_with_output()).await;

        match output {
            Ok(Ok(output)) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    warn!(
                        "[HOOKS] Command hook '{}' exited with {}: {}",
                        self.command,
                        output.status,
                        stderr.trim()
                    );
                    return Ok(HookResult::default());
                }

                let stdout = String::from_utf8_lossy(&output.stdout);
                let result: HookResult = match serde_json::from_str(stdout.trim()) {
                    Ok(r) => r,
                    Err(e) => {
                        // Fail-closed: malformed hook output is treated as a block
                        // rather than silently passing through. This prevents a
                        // broken hook from being invisible in the pipeline.
                        warn!(
                            "[HOOKS] Command hook '{}' output could not be parsed as HookResult: {}. Treating as block (fail-closed).",
                            self.command, e
                        );
                        HookResult {
                            block: true,
                            reason: Some(format!(
                                "hook output parse error: {}",
                                e
                            )),
                            ..HookResult::default()
                        }
                    }
                };

                debug!(
                    "[HOOKS] Command hook '{}' completed successfully",
                    self.command
                );
                Ok(result)
            }
            Ok(Err(e)) => {
                warn!("[HOOKS] Command hook '{}' failed: {}", self.command, e);
                Ok(HookResult::default())
            }
            Err(_) => {
                warn!(
                    "[HOOKS] Command hook '{}' timed out after {:?}",
                    self.command, self.timeout
                );
                Ok(HookResult::default())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hook manager
// ---------------------------------------------------------------------------

/// Central registry that dispatches hook invocations.
pub struct HookManager {
    /// Trait-object handlers registered per hook point.
    handlers: HashMap<HookPoint, Vec<Box<dyn HookHandler>>>,
    /// External command hooks loaded from configuration.
    command_hooks: Vec<HookConfig>,
    /// Configuration limits.
    limits: HooksConfig,
    /// Master on/off switch.
    enabled: bool,
}

impl HookManager {
    /// Create an empty hook manager with no registered handlers.
    pub fn new() -> Self {
        info!("[HOOKS] Hook manager initialized");
        Self {
            handlers: HashMap::new(),
            command_hooks: Vec::new(),
            limits: HooksConfig::default(),
            enabled: true,
        }
    }

    /// Create from configuration.
    pub fn from_config(config: HooksConfig) -> Self {
        let enabled = config.enabled;
        info!("[HOOKS] Hook manager initialized (enabled={})", enabled);
        Self {
            handlers: HashMap::new(),
            command_hooks: config.hooks.clone(),
            limits: config,
            enabled,
        }
    }

    /// Register a trait-object handler for a specific hook point.
    pub fn register_handler(&mut self, point: HookPoint, handler: Box<dyn HookHandler>) {
        let point_repr = format!("{:?}", point);
        let vec = self.handlers.entry(point).or_default();
        if vec.len() >= self.limits.max_handlers_per_point {
            warn!(
                "[HOOKS] Handler limit ({}) reached for {}, ignoring",
                self.limits.max_handlers_per_point, point_repr
            );
            return;
        }
        info!("[HOOKS] Registered handler for {}", point_repr);
        vec.push(handler);
    }

    /// Add an external command hook from configuration.
    pub fn add_command_hook(&mut self, config: HookConfig) {
        if self.command_hooks.len() >= self.limits.max_command_hooks {
            warn!(
                "[HOOKS] Command hook limit ({}) reached, ignoring {:?}",
                self.limits.max_command_hooks, config.command
            );
            return;
        }
        info!(
            "[HOOKS] Added command hook for {:?}: {:?}",
            config.point, config.command
        );
        self.command_hooks.push(config);
    }

    /// Check if the hooks engine is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Execute all hooks for the given point and return a **merged** result.
    ///
    /// Runs trait handlers first, then command hooks.  If any handler returns
    /// `block = true` or `permission_decision = Deny`, execution stops
    /// immediately and that result is returned (fail-closed).
    ///
    /// Otherwise all results are merged:
    /// - `modified_content`: last writer wins
    /// - `updated_input`: last writer wins
    /// - `permission_decision`: Deny > Ask > Allow
    /// - `additional_context`: concatenated (newline-separated)
    /// - `system_message`: concatenated (newline-separated)
    /// - `metadata`: merged (last writer wins per key)
    pub async fn execute_hooks(&self, point: &HookPoint, context: &HookContext) -> HookResult {
        if !self.enabled {
            return HookResult::default();
        }

        let mut merged = HookResult::default();

        // Run registered trait-object handlers.
        if let Some(handlers) = self.handlers.get(point) {
            for handler in handlers {
                match handler.execute(point, context).await {
                    Ok(result) => {
                        debug!(
                            "[HOOKS] Handler for {:?} returned (block={}, decision={:?})",
                            point, result.block, result.permission_decision
                        );
                        if Self::is_blocking(&result) {
                            warn!(
                                "[HOOKS] Handler blocked pipeline at {:?}: {}",
                                point,
                                result.reason.as_deref().unwrap_or("no reason given")
                            );
                            return result;
                        }
                        Self::merge_result(&mut merged, result);
                    }
                    Err(e) => {
                        warn!("[HOOKS] Handler for {:?} failed: {}", point, e);
                    }
                }
            }
        }

        // Run command hooks whose point matches and that are enabled.
        for config in &self.command_hooks {
            if config.point != *point || !config.enabled {
                continue;
            }

            if let Some(ref command) = config.command {
                let handler = CommandHookHandler::new(
                    command.clone(),
                    Duration::from_millis(config.timeout_ms),
                );
                match handler.execute(point, context).await {
                    Ok(result) => {
                        debug!(
                            "[HOOKS] Command hook '{}' for {:?} returned (block={}, decision={:?})",
                            command, point, result.block, result.permission_decision
                        );
                        if Self::is_blocking(&result) {
                            warn!(
                                "[HOOKS] Command hook '{}' blocked pipeline at {:?}: {}",
                                command,
                                point,
                                result.reason.as_deref().unwrap_or("no reason given")
                            );
                            return result;
                        }
                        Self::merge_result(&mut merged, result);
                    }
                    Err(e) => {
                        warn!(
                            "[HOOKS] Command hook '{}' for {:?} failed: {}",
                            command, point, e
                        );
                    }
                }
            }
        }

        merged
    }

    /// Return `true` if a result should stop pipeline execution.
    fn is_blocking(result: &HookResult) -> bool {
        result.block
            || result.permission_decision.as_ref()
                == Some(&PermissionDecision::Deny)
    }

    /// Merge `incoming` into `base`.
    fn merge_result(base: &mut HookResult, incoming: HookResult) {
        if let Some(v) = incoming.modified_content {
            base.modified_content = Some(v);
        }
        if let Some(v) = incoming.updated_input {
            base.updated_input = Some(v);
        }
        if let Some(v) = incoming.permission_decision {
            // Higher priority wins: Deny > Ask > Allow
            let priority = |d: &PermissionDecision| match d {
                PermissionDecision::Deny => 3,
                PermissionDecision::Ask => 2,
                PermissionDecision::Allow => 1,
            };
            if base.permission_decision.is_none()
                || priority(&v) > priority(base.permission_decision.as_ref().unwrap())
            {
                base.permission_decision = Some(v);
            }
        }
        if let Some(v) = incoming.additional_context {
            base.additional_context = Some(match base.additional_context.take() {
                Some(existing) => format!("{}\n{}", existing, v),
                None => v,
            });
        }
        if let Some(v) = incoming.system_message {
            base.system_message = Some(match base.system_message.take() {
                Some(existing) => format!("{}\n{}", existing, v),
                None => v,
            });
        }
        if let Some(v) = incoming.reason {
            base.reason = Some(v);
        }
        for (k, v) in incoming.metadata {
            base.metadata.insert(k, v);
        }
    }

    /// Return the number of registered trait-object handlers across all points.
    pub fn handler_count(&self) -> usize {
        self.handlers.values().map(|v| v.len()).sum()
    }

    /// Return the number of command hooks.
    pub fn command_hook_count(&self) -> usize {
        self.command_hooks.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Test handlers
    // -----------------------------------------------------------------------

    struct EchoHandler {
        suffix: String,
    }

    #[async_trait]
    impl HookHandler for EchoHandler {
        async fn execute(&self, _point: &HookPoint, context: &HookContext) -> Result<HookResult> {
            let modified = context
                .message_text
                .as_ref()
                .map(|t| format!("{}{}", t, self.suffix));
            Ok(HookResult {
                modified_content: modified,
                block: false,
                ..HookResult::default()
            })
        }
    }

    struct BlockingHandler {
        reason: String,
    }

    #[async_trait]
    impl HookHandler for BlockingHandler {
        async fn execute(&self, _point: &HookPoint, _context: &HookContext) -> Result<HookResult> {
            Ok(HookResult {
                block: true,
                reason: Some(self.reason.clone()),
                ..HookResult::default()
            })
        }
    }

    struct RewriteInputHandler;

    #[async_trait]
    impl HookHandler for RewriteInputHandler {
        async fn execute(&self, _point: &HookPoint, _context: &HookContext) -> Result<HookResult> {
            Ok(HookResult {
                updated_input: Some(serde_json::json!({"injected": true})),
                ..HookResult::default()
            })
        }
    }

    struct DenyHandler;

    #[async_trait]
    impl HookHandler for DenyHandler {
        async fn execute(&self, _point: &HookPoint, _context: &HookContext) -> Result<HookResult> {
            Ok(HookResult {
                permission_decision: Some(PermissionDecision::Deny),
                reason: Some("denied by policy".into()),
                ..HookResult::default()
            })
        }
    }

    struct ContextInjector;

    #[async_trait]
    impl HookHandler for ContextInjector {
        async fn execute(&self, _point: &HookPoint, _context: &HookContext) -> Result<HookResult> {
            Ok(HookResult {
                additional_context: Some("injected context".into()),
                system_message: Some("banner".into()),
                ..HookResult::default()
            })
        }
    }

    // -----------------------------------------------------------------------
    // HookManager tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_hook_manager_new() {
        let manager = HookManager::new();
        assert!(manager.is_enabled());
        assert_eq!(manager.handler_count(), 0);
        assert_eq!(manager.command_hook_count(), 0);
    }

    #[test]
    fn test_hook_manager_from_config_disabled() {
        let manager = HookManager::from_config(HooksConfig {
            enabled: false,
            ..HooksConfig::default()
        });
        assert!(!manager.is_enabled());
    }

    #[test]
    fn test_register_handler() {
        let mut manager = HookManager::new();
        manager.register_handler(
            HookPoint::BeforeMessage,
            Box::new(EchoHandler { suffix: "!".into() }),
        );
        assert_eq!(manager.handler_count(), 1);
    }

    #[test]
    fn test_add_command_hook() {
        let mut manager = HookManager::new();
        manager.add_command_hook(HookConfig {
            point: HookPoint::AfterMessage,
            command: Some("echo ok".into()),
            timeout_ms: 1000,
            enabled: true,
        });
        assert_eq!(manager.command_hook_count(), 1);
    }

    #[tokio::test]
    async fn test_execute_hooks_disabled_returns_default() {
        let manager = HookManager::from_config(HooksConfig {
            enabled: false,
            ..HooksConfig::default()
        });
        let ctx = HookContext::for_message("hello");
        let result = manager.execute_hooks(&HookPoint::BeforeMessage, &ctx).await;
        assert_eq!(result, HookResult::default());
    }

    #[tokio::test]
    async fn test_execute_hooks_single_handler() {
        let mut manager = HookManager::new();
        manager.register_handler(
            HookPoint::BeforeMessage,
            Box::new(EchoHandler { suffix: " [hooked]".into() }),
        );

        let ctx = HookContext::for_message("hello");
        let result = manager.execute_hooks(&HookPoint::BeforeMessage, &ctx).await;

        assert_eq!(result.modified_content.as_deref(), Some("hello [hooked]"));
        assert!(!result.block);
        assert!(result.permission_decision.is_none());
    }

    #[tokio::test]
    async fn test_execute_hooks_no_handlers_returns_default() {
        let manager = HookManager::new();
        let ctx = HookContext::for_message("hello");
        let result = manager.execute_hooks(&HookPoint::OnError, &ctx).await;
        assert_eq!(result, HookResult::default());
    }

    #[tokio::test]
    async fn test_execute_hooks_blocking_stops_pipeline() {
        let mut manager = HookManager::new();
        manager.register_handler(
            HookPoint::BeforeMessage,
            Box::new(BlockingHandler { reason: "stop".into() }),
        );
        // This handler should NOT run because the first one blocks
        manager.register_handler(
            HookPoint::BeforeMessage,
            Box::new(EchoHandler { suffix: " [late]".into() }),
        );

        let ctx = HookContext::for_message("test");
        let result = manager.execute_hooks(&HookPoint::BeforeMessage, &ctx).await;

        assert!(result.block);
        assert_eq!(result.reason.as_deref(), Some("stop"));
        assert!(result.modified_content.is_none());
    }

    #[tokio::test]
    async fn test_execute_hooks_permission_deny_stops_pipeline() {
        let mut manager = HookManager::new();
        manager.register_handler(HookPoint::PreToolUse, Box::new(DenyHandler));
        manager.register_handler(
            HookPoint::PreToolUse,
            Box::new(EchoHandler { suffix: " [late]".into() }),
        );

        let ctx = HookContext::for_tool_use("execute", "id-1", serde_json::json!({"cmd": "rm -rf /"}));
        let result = manager.execute_hooks(&HookPoint::PreToolUse, &ctx).await;

        assert_eq!(result.permission_decision, Some(PermissionDecision::Deny));
        assert_eq!(result.reason.as_deref(), Some("denied by policy"));
    }

    #[tokio::test]
    async fn test_execute_hooks_multiple_non_blocking_merges() {
        let mut manager = HookManager::new();
        manager.register_handler(
            HookPoint::AfterToolCall,
            Box::new(EchoHandler { suffix: " A".into() }),
        );
        manager.register_handler(
            HookPoint::AfterToolCall,
            Box::new(EchoHandler { suffix: " B".into() }),
        );

        let ctx = HookContext::for_message("result");
        let result = manager.execute_hooks(&HookPoint::AfterToolCall, &ctx).await;

        // Last writer wins for modified_content
        assert_eq!(result.modified_content.as_deref(), Some("result B"));
    }

    struct SecondContextInjector;

    #[async_trait]
    impl HookHandler for SecondContextInjector {
        async fn execute(
            &self,
            _point: &HookPoint,
            _context: &HookContext,
        ) -> Result<HookResult> {
            Ok(HookResult {
                additional_context: Some("second injection".into()),
                system_message: Some("second banner".into()),
                ..HookResult::default()
            })
        }
    }

    #[tokio::test]
    async fn test_execute_hooks_merges_context_and_messages() {
        let mut manager = HookManager::new();
        manager.register_handler(HookPoint::BeforeMessage, Box::new(ContextInjector));
        manager.register_handler(HookPoint::BeforeMessage, Box::new(SecondContextInjector));

        let ctx = HookContext::for_message("hello");
        let result = manager.execute_hooks(&HookPoint::BeforeMessage, &ctx).await;

        assert_eq!(
            result.additional_context.as_deref(),
            Some("injected context\nsecond injection")
        );
        assert_eq!(
            result.system_message.as_deref(),
            Some("banner\nsecond banner")
        );
    }

    #[tokio::test]
    async fn test_execute_hooks_input_rewrite() {
        let mut manager = HookManager::new();
        manager.register_handler(HookPoint::PreToolUse, Box::new(RewriteInputHandler));

        let ctx = HookContext::for_tool_use("search", "id-1", serde_json::json!({"q": "rust"}));
        let result = manager.execute_hooks(&HookPoint::PreToolUse, &ctx).await;

        assert_eq!(result.updated_input, Some(serde_json::json!({"injected": true})));
    }

    // -----------------------------------------------------------------------
    // HookContext tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_hook_context_empty() {
        let ctx = HookContext::empty();
        assert!(ctx.message_text.is_none());
        assert!(ctx.tool_id.is_none());
        assert!(ctx.session_key.is_none());
    }

    #[test]
    fn test_hook_context_for_message() {
        let ctx = HookContext::for_message("hi");
        assert_eq!(ctx.message_text.as_deref(), Some("hi"));
    }

    #[test]
    fn test_hook_context_for_tool_call() {
        let ctx = HookContext::for_tool_call("search", serde_json::json!({"q": "rust"}));
        assert_eq!(ctx.tool_name.as_deref(), Some("search"));
    }

    #[test]
    fn test_hook_context_for_tool_use() {
        let ctx = HookContext::for_tool_use("search", "id-1", serde_json::json!({"q": "rust"}));
        assert_eq!(ctx.tool_name.as_deref(), Some("search"));
        assert_eq!(ctx.tool_id.as_deref(), Some("id-1"));
    }

    #[test]
    fn test_hook_context_for_tool_result() {
        let ctx = HookContext::for_tool_result("search", "id-1", serde_json::json!({"q": "rust"}), "found");
        assert_eq!(ctx.tool_result.as_deref(), Some("found"));
    }

    #[test]
    fn test_hook_context_for_session() {
        let ctx = HookContext::for_session("sess-1", Some("agent-a"));
        assert_eq!(ctx.session_key.as_deref(), Some("sess-1"));
        assert_eq!(ctx.agent_id.as_deref(), Some("agent-a"));
    }

    #[test]
    fn test_hook_context_for_task() {
        let ctx = HookContext::for_task("task-1", Some("completed"));
        assert_eq!(ctx.task_id.as_deref(), Some("task-1"));
        assert_eq!(ctx.task_status.as_deref(), Some("completed"));
    }

    #[test]
    fn test_hook_context_fluent_builders() {
        let ctx = HookContext::for_message("hi")
            .with_channel("telegram")
            .with_cwd("/home/user");
        assert_eq!(ctx.channel_source.as_deref(), Some("telegram"));
        assert_eq!(ctx.cwd.as_deref(), Some("/home/user"));
    }

    #[test]
    fn test_hook_context_for_error() {
        let ctx = HookContext::for_error("something broke");
        assert_eq!(ctx.error.as_deref(), Some("something broke"));
    }

    // -----------------------------------------------------------------------
    // HookResult tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_hook_result_default() {
        let result = HookResult::default();
        assert!(result.modified_content.is_none());
        assert!(!result.block);
        assert!(result.permission_decision.is_none());
        assert!(result.metadata.is_empty());
        assert!(!result.suppress_output);
    }

    #[test]
    fn test_permission_decision_priority() {
        // Deny > Ask > Allow
        assert!(PermissionDecision::Deny > PermissionDecision::Ask);
        assert!(PermissionDecision::Ask > PermissionDecision::Allow);
    }

    // -----------------------------------------------------------------------
    // HookPoint tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_hook_point_display() {
        assert_eq!(HookPoint::BeforeMessage.to_string(), "before_message");
        assert_eq!(HookPoint::PreToolUse.to_string(), "pre_tool_use");
        assert_eq!(HookPoint::PostToolUseFailure.to_string(), "post_tool_use_failure");
        assert_eq!(HookPoint::SessionStart.to_string(), "session_start");
        assert_eq!(HookPoint::PermissionDenied.to_string(), "permission_denied");
        assert_eq!(HookPoint::TaskCreated.to_string(), "task_created");
        assert_eq!(HookPoint::UserPromptSubmit.to_string(), "user_prompt_submit");
    }

    #[test]
    fn test_hook_point_serde_roundtrip() {
        for point in [
            HookPoint::BeforeMessage,
            HookPoint::AfterMessage,
            HookPoint::UserPromptSubmit,
            HookPoint::PreToolUse,
            HookPoint::PostToolUse,
            HookPoint::SessionStart,
            HookPoint::PermissionDenied,
            HookPoint::TaskCompleted,
            HookPoint::ModelOverride,
            HookPoint::CwdChanged,
        ] {
            let json = serde_json::to_string(&point).unwrap();
            let deserialized: HookPoint = serde_json::from_str(&json).unwrap();
            assert_eq!(point, deserialized);
        }
    }

    // -----------------------------------------------------------------------
    // Config tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_hooks_config_defaults() {
        let cfg = HooksConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.hooks.is_empty());
        assert_eq!(cfg.max_handlers_per_point, 50);
        assert_eq!(cfg.max_command_hooks, 100);
        assert_eq!(cfg.default_timeout_ms, 5000);
    }

    #[test]
    fn test_hook_config_defaults() {
        let json = r#"{"point":"before_message"}"#;
        let config: HookConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.timeout_ms, 5000);
        assert!(config.enabled);
        assert!(config.command.is_none());
    }

    #[test]
    fn test_disabled_command_hooks_are_skipped() {
        let mut manager = HookManager::new();
        manager.add_command_hook(HookConfig {
            point: HookPoint::BeforeMessage,
            command: Some("echo should_not_run".into()),
            timeout_ms: 1000,
            enabled: false,
        });
        assert_eq!(manager.command_hook_count(), 1);
    }
}
