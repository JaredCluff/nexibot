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
    /// Before a user message is processed.
    BeforeMessage,
    /// After a response has been generated.
    AfterMessage,
    /// Before a tool call is dispatched.
    BeforeToolCall,
    /// After a tool call completes.
    AfterToolCall,
    /// Override which model handles the request.
    ModelOverride,
    /// When an error occurs in the pipeline.
    OnError,
}

impl std::fmt::Display for HookPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookPoint::BeforeMessage => write!(f, "before_message"),
            HookPoint::AfterMessage => write!(f, "after_message"),
            HookPoint::BeforeToolCall => write!(f, "before_tool_call"),
            HookPoint::AfterToolCall => write!(f, "after_tool_call"),
            HookPoint::ModelOverride => write!(f, "model_override"),
            HookPoint::OnError => write!(f, "on_error"),
        }
    }
}

/// Result returned by a hook handler.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookResult {
    /// If set, replaces the content flowing through the pipeline.
    pub modified_content: Option<String>,
    /// If `true`, the pipeline should stop processing.
    pub block: bool,
    /// Human-readable reason for blocking, logged when `block` is `true`.
    #[serde(default)]
    pub reason: Option<String>,
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
    /// The JSON input for the tool call, if applicable.
    pub tool_input: Option<Value>,
    /// The result produced by a tool call, if applicable.
    pub tool_result: Option<String>,
    /// An error description, if this is an error hook.
    pub error: Option<String>,
}

impl HookContext {
    /// Create an empty context (all fields `None`).
    pub fn empty() -> Self {
        Self {
            message_text: None,
            tool_name: None,
            tool_input: None,
            tool_result: None,
            error: None,
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

    /// Context for an error event.
    pub fn for_error(error: &str) -> Self {
        Self {
            error: Some(error.to_string()),
            ..Self::empty()
        }
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

        // Write context to stdin
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(context_json.as_bytes()).await?;
            // Drop stdin to signal EOF
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

/// Maximum trait-object handlers per hook point.
const MAX_HANDLERS_PER_POINT: usize = 50;
/// Maximum external command hooks.
const MAX_COMMAND_HOOKS: usize = 100;

/// Central registry that dispatches hook invocations.
pub struct HookManager {
    /// Trait-object handlers registered per hook point.
    handlers: HashMap<HookPoint, Vec<Box<dyn HookHandler>>>,
    /// External command hooks loaded from configuration.
    command_hooks: Vec<HookConfig>,
}

impl HookManager {
    /// Create an empty hook manager with no registered handlers.
    pub fn new() -> Self {
        info!("[HOOKS] Hook manager initialized");
        Self {
            handlers: HashMap::new(),
            command_hooks: Vec::new(),
        }
    }

    /// Register a trait-object handler for a specific hook point.
    pub fn register_handler(&mut self, point: HookPoint, handler: Box<dyn HookHandler>) {
        let vec = self.handlers.entry(point).or_default();
        if vec.len() >= MAX_HANDLERS_PER_POINT {
            warn!("[HOOKS] Handler limit ({}) reached for {:?}, ignoring", MAX_HANDLERS_PER_POINT, point);
            return;
        }
        info!("[HOOKS] Registered handler for {:?}", point);
        vec.push(handler);
    }

    /// Add an external command hook from configuration.
    pub fn add_command_hook(&mut self, config: HookConfig) {
        if self.command_hooks.len() >= MAX_COMMAND_HOOKS {
            warn!("[HOOKS] Command hook limit ({}) reached, ignoring {:?}", MAX_COMMAND_HOOKS, config.command);
            return;
        }
        info!(
            "[HOOKS] Added command hook for {:?}: {:?}",
            config.point, config.command
        );
        self.command_hooks.push(config);
    }

    /// Execute all hooks (trait handlers + command hooks) for the given point.
    ///
    /// Returns a `Vec<HookResult>`, one per handler that ran. If any result
    /// has `block == true`, callers should stop the pipeline.
    pub async fn execute_hooks(&self, point: &HookPoint, context: &HookContext) -> Vec<HookResult> {
        let mut results = Vec::new();

        // Run registered trait-object handlers.
        // Stop immediately if any handler signals block: true — subsequent
        // handlers must not run when a blocking result has been issued.
        if let Some(handlers) = self.handlers.get(point) {
            for handler in handlers {
                match handler.execute(point, context).await {
                    Ok(result) => {
                        debug!(
                            "[HOOKS] Handler for {:?} returned (block={})",
                            point, result.block
                        );
                        if result.block {
                            warn!(
                                "[HOOKS] Handler blocked pipeline at {:?}: {}",
                                point,
                                result.reason.as_deref().unwrap_or("no reason given")
                            );
                            results.push(result);
                            return results;
                        }
                        results.push(result);
                    }
                    Err(e) => {
                        warn!("[HOOKS] Handler for {:?} failed: {}", point, e);
                    }
                }
            }
        }

        // Run command hooks whose point matches and that are enabled.
        // Same early-exit rule: if a command hook blocks, stop processing.
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
                            "[HOOKS] Command hook '{}' for {:?} returned (block={})",
                            command, point, result.block
                        );
                        if result.block {
                            warn!(
                                "[HOOKS] Command hook '{}' blocked pipeline at {:?}: {}",
                                command,
                                point,
                                result.reason.as_deref().unwrap_or("no reason given")
                            );
                            results.push(result);
                            return results;
                        }
                        results.push(result);
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

        results
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

    /// A simple in-process test handler.
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
                reason: None,
                metadata: HashMap::new(),
            })
        }
    }

    /// Handler that always blocks.
    struct BlockingHandler;

    #[async_trait]
    impl HookHandler for BlockingHandler {
        async fn execute(&self, _point: &HookPoint, _context: &HookContext) -> Result<HookResult> {
            Ok(HookResult {
                modified_content: None,
                block: true,
                reason: None,
                metadata: HashMap::new(),
            })
        }
    }

    #[test]
    fn test_hook_manager_new() {
        let manager = HookManager::new();
        assert_eq!(manager.handler_count(), 0);
        assert_eq!(manager.command_hook_count(), 0);
    }

    #[test]
    fn test_register_handler() {
        let mut manager = HookManager::new();
        manager.register_handler(
            HookPoint::BeforeMessage,
            Box::new(EchoHandler {
                suffix: "!".to_string(),
            }),
        );
        assert_eq!(manager.handler_count(), 1);
    }

    #[test]
    fn test_add_command_hook() {
        let mut manager = HookManager::new();
        manager.add_command_hook(HookConfig {
            point: HookPoint::AfterMessage,
            command: Some("echo ok".to_string()),
            timeout_ms: 1000,
            enabled: true,
        });
        assert_eq!(manager.command_hook_count(), 1);
    }

    #[tokio::test]
    async fn test_execute_hooks_with_trait_handler() {
        let mut manager = HookManager::new();
        manager.register_handler(
            HookPoint::BeforeMessage,
            Box::new(EchoHandler {
                suffix: " [hooked]".to_string(),
            }),
        );

        let context = HookContext::for_message("hello");
        let results = manager
            .execute_hooks(&HookPoint::BeforeMessage, &context)
            .await;

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].modified_content.as_deref(),
            Some("hello [hooked]")
        );
        assert!(!results[0].block);
    }

    #[tokio::test]
    async fn test_execute_hooks_no_handlers_for_point() {
        let manager = HookManager::new();
        let context = HookContext::for_message("hello");
        let results = manager.execute_hooks(&HookPoint::OnError, &context).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_execute_hooks_blocking() {
        let mut manager = HookManager::new();
        manager.register_handler(HookPoint::BeforeMessage, Box::new(BlockingHandler));

        let context = HookContext::for_message("test");
        let results = manager
            .execute_hooks(&HookPoint::BeforeMessage, &context)
            .await;

        assert_eq!(results.len(), 1);
        assert!(results[0].block);
    }

    #[tokio::test]
    async fn test_execute_hooks_multiple_handlers() {
        let mut manager = HookManager::new();
        manager.register_handler(
            HookPoint::AfterToolCall,
            Box::new(EchoHandler {
                suffix: " A".to_string(),
            }),
        );
        manager.register_handler(
            HookPoint::AfterToolCall,
            Box::new(EchoHandler {
                suffix: " B".to_string(),
            }),
        );

        let context = HookContext::for_message("result");
        let results = manager
            .execute_hooks(&HookPoint::AfterToolCall, &context)
            .await;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].modified_content.as_deref(), Some("result A"));
        assert_eq!(results[1].modified_content.as_deref(), Some("result B"));
    }

    #[test]
    fn test_hook_context_helpers() {
        let ctx = HookContext::empty();
        assert!(ctx.message_text.is_none());

        let ctx = HookContext::for_message("hi");
        assert_eq!(ctx.message_text.as_deref(), Some("hi"));

        let ctx = HookContext::for_tool_call("search", serde_json::json!({"q": "rust"}));
        assert_eq!(ctx.tool_name.as_deref(), Some("search"));

        let ctx = HookContext::for_error("something broke");
        assert_eq!(ctx.error.as_deref(), Some("something broke"));
    }

    #[test]
    fn test_hook_result_default() {
        let result = HookResult::default();
        assert!(result.modified_content.is_none());
        assert!(!result.block);
        assert!(result.metadata.is_empty());
    }

    #[test]
    fn test_hook_point_display() {
        assert_eq!(HookPoint::BeforeMessage.to_string(), "before_message");
        assert_eq!(HookPoint::AfterToolCall.to_string(), "after_tool_call");
        assert_eq!(HookPoint::OnError.to_string(), "on_error");
    }

    #[test]
    fn test_hook_point_serde_roundtrip() {
        let point = HookPoint::ModelOverride;
        let json = serde_json::to_string(&point).unwrap();
        let deserialized: HookPoint = serde_json::from_str(&json).unwrap();
        assert_eq!(point, deserialized);
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
            command: Some("echo should_not_run".to_string()),
            timeout_ms: 1000,
            enabled: false,
        });
        // The hook is registered but disabled; handler count stays zero for trait handlers
        assert_eq!(manager.command_hook_count(), 1);
    }
}
