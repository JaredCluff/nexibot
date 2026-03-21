//! SubagentExecutor — runs spawned agent tasks through the LLM tool loop.
//!
//! This bridges the gap between `OrchestrationManager` (which tracks spawn
//! records) and actual LLM execution. Each subagent gets its own temporary
//! Claude client, runs through the unified tool loop, and returns results
//! via a oneshot channel.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, TryAcquireError};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::commands::AppState;
use crate::config::AgentConfig;
use crate::router::{self, IncomingMessage, RouteOptions};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::{self, ToolLoopConfig};

/// Result produced by a subagent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentExecutionResult {
    /// The text output from the subagent.
    pub output: String,
    /// Number of tool calls made during execution.
    pub tool_calls_made: u32,
    /// Which model was used.
    pub model_used: String,
    /// How long the execution took.
    pub elapsed_ms: u64,
    /// Whether the execution succeeded.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Configuration for the SubagentExecutor.
#[derive(Debug, Clone)]
pub struct SubagentExecutorConfig {
    /// Maximum number of concurrent subagent executions.
    pub max_concurrent: usize,
    /// Default timeout per subagent execution.
    pub timeout: Duration,
    /// Maximum tool-loop iterations per subagent.
    pub max_iterations: usize,
}

impl Default for SubagentExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 5,
            timeout: Duration::from_secs(120),
            max_iterations: 10,
        }
    }
}

/// Executor that runs subagent tasks through the LLM tool loop.
pub struct SubagentExecutor {
    config: SubagentExecutorConfig,
    /// Semaphore used to enforce the max_concurrent limit correctly under async concurrency.
    semaphore: Arc<Semaphore>,
}

impl SubagentExecutor {
    pub fn new(config: SubagentExecutorConfig) -> Self {
        info!(
            "[SUBAGENT_EXECUTOR] Initialized (max_concurrent={}, timeout={}s, max_iterations={})",
            config.max_concurrent,
            config.timeout.as_secs(),
            config.max_iterations
        );
        Self {
            semaphore: Arc::new(Semaphore::new(config.max_concurrent)),
            config,
        }
    }

    /// Check if we can accept another subagent execution.
    pub fn can_accept(&self) -> bool {
        self.semaphore.available_permits() > 0
    }

    /// Get the number of currently active executions.
    #[allow(dead_code)]
    pub fn active_count(&self) -> usize {
        self.config.max_concurrent - self.semaphore.available_permits()
    }

    /// Execute a subagent task. Blocks until completion or timeout.
    ///
    /// This creates a temporary Claude client for the agent, builds its
    /// system prompt, and runs the tool loop.
    pub async fn execute(
        &self,
        agent_config: &AgentConfig,
        task: &str,
        parent_session: &str,
        state: &AppState,
        workspace_id: Option<&str>,
    ) -> SubagentExecutionResult {
        // Acquire a semaphore permit to enforce the concurrency limit.
        // try_acquire() is non-blocking and avoids the TOCTOU race that a
        // load-then-increment pattern has under async concurrency.
        let _permit = match self.semaphore.try_acquire() {
            Ok(permit) => permit,
            Err(TryAcquireError::NoPermits) => {
                return SubagentExecutionResult {
                    output: String::new(),
                    tool_calls_made: 0,
                    model_used: String::new(),
                    elapsed_ms: 0,
                    success: false,
                    error: Some(format!(
                        "Concurrency limit reached: max_concurrent={}",
                        self.config.max_concurrent
                    )),
                };
            }
            Err(TryAcquireError::Closed) => {
                return SubagentExecutionResult {
                    output: String::new(),
                    tool_calls_made: 0,
                    model_used: String::new(),
                    elapsed_ms: 0,
                    success: false,
                    error: Some("Executor semaphore closed".to_string()),
                };
            }
        };
        // _permit is held for the duration of the execution and released on drop.

        let start = Instant::now();
        let result = self
            .execute_inner(agent_config, task, parent_session, state, workspace_id)
            .await;
        // _permit is automatically released here when it drops.

        match result {
            Ok(mut r) => {
                r.elapsed_ms = start.elapsed().as_millis() as u64;
                r
            }
            Err(e) => SubagentExecutionResult {
                output: String::new(),
                tool_calls_made: 0,
                model_used: String::new(),
                elapsed_ms: start.elapsed().as_millis() as u64,
                success: false,
                error: Some(e.to_string()),
            },
        }
    }

    async fn execute_inner(
        &self,
        agent_config: &AgentConfig,
        task: &str,
        _parent_session: &str,
        state: &AppState,
        workspace_id: Option<&str>,
    ) -> Result<SubagentExecutionResult> {
        info!(
            "[SUBAGENT_EXECUTOR] Starting execution: agent='{}', task='{}'",
            agent_config.id,
            if task.len() > 100 { &task[..100] } else { task }
        );

        // Validate workspace_id: must be a valid UUID to prevent injection of
        // arbitrary strings into the system prompt and workspace scope lookup.
        let validated_workspace_id = workspace_id.and_then(|id| {
            if uuid::Uuid::parse_str(id).is_ok() {
                Some(id)
            } else {
                warn!("[SUBAGENT] Invalid workspace_id format: {:?}, ignoring", id);
                None
            }
        });

        // Create a fresh Claude client for this subagent
        let client = ClaudeClient::new(state.config.clone());

        // Build system prompt with agent context
        let system_prompt = self.build_system_prompt(agent_config, task, validated_workspace_id);

        // Build the prompt message
        let prompt = format!("{}\n\nYour task:\n{}", system_prompt, task);

        let message = IncomingMessage {
            text: prompt,
            channel: ChannelSource::InterAgent {
                agent_id: agent_config.id.clone(),
            },
            agent_id: Some(agent_config.id.clone()),
            metadata: HashMap::new(),
        };

        let observer = tool_loop::NoOpObserver;
        let loop_config = ToolLoopConfig {
            max_iterations: self.config.max_iterations,
            timeout: Some(self.config.timeout),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: true,
            channel: Some(ChannelSource::InterAgent {
                agent_id: agent_config.id.clone(),
            }),
            run_defense_checks: false, // Subagent output is internal
            streaming: false,
            sender_id: None,
            between_tool_delay_ms: 0,
        };

        let options = RouteOptions {
            claude_client: &client,
            overrides: SessionOverrides::default(),
            loop_config,
            observer: &observer,
            streaming: false,
            window: None,
            on_stream_chunk: None,
            auto_compact: false,
            save_to_memory: false,
            sync_supermemory: false,
            check_sensitive_data: false,
        };

        // Execute with timeout
        let timeout_result = tokio::time::timeout(
            self.config.timeout,
            router::route_message(&message, options, state),
        )
        .await;

        match timeout_result {
            Ok(Ok(routed)) => {
                info!(
                    "[SUBAGENT_EXECUTOR] Agent '{}' completed successfully ({} chars output)",
                    agent_config.id,
                    routed.text.len()
                );
                Ok(SubagentExecutionResult {
                    output: routed.text,
                    tool_calls_made: routed.tool_calls_made as u32,
                    model_used: agent_config
                        .primary_model
                        .clone()
                        .or_else(|| agent_config.model.clone())
                        .unwrap_or_else(|| "default".to_string()),
                    elapsed_ms: 0, // Filled in by caller
                    success: true,
                    error: None,
                })
            }
            Ok(Err(e)) => {
                warn!(
                    "[SUBAGENT_EXECUTOR] Agent '{}' failed: {}",
                    agent_config.id, e
                );
                Ok(SubagentExecutionResult {
                    output: String::new(),
                    tool_calls_made: 0,
                    model_used: String::new(),
                    elapsed_ms: 0,
                    success: false,
                    error: Some(e.to_string()),
                })
            }
            Err(_) => {
                warn!(
                    "[SUBAGENT_EXECUTOR] Agent '{}' timed out after {}s",
                    agent_config.id,
                    self.config.timeout.as_secs()
                );
                Ok(SubagentExecutionResult {
                    output: String::new(),
                    tool_calls_made: 0,
                    model_used: String::new(),
                    elapsed_ms: 0,
                    success: false,
                    error: Some(format!(
                        "Subagent timed out after {}s",
                        self.config.timeout.as_secs()
                    )),
                })
            }
        }
    }

    fn build_system_prompt(
        &self,
        agent_config: &AgentConfig,
        _task: &str,
        workspace_id: Option<&str>,
    ) -> String {
        let mut parts = Vec::new();

        parts.push(format!(
            "You are '{}', a specialized subagent.",
            agent_config.name
        ));

        if let Some(ref system_prompt) = agent_config.system_prompt {
            parts.push(system_prompt.clone());
        }

        if !agent_config.capabilities.is_empty() {
            let caps: Vec<String> = agent_config
                .capabilities
                .iter()
                .map(|c| format!("- {}: {}", c.name, c.description))
                .collect();
            parts.push(format!("Your capabilities:\n{}", caps.join("\n")));
        }

        if let Some(wid) = workspace_id {
            parts.push(format!(
                "You have access to a shared workspace (ID: {}). \
                 Use nexibot_workspace_read and nexibot_workspace_write to \
                 share data with other agents in this orchestration.",
                wid
            ));
        }

        parts.push(
            "Complete the assigned task thoroughly and provide a clear, \
             structured response. Use available tools when needed."
                .to_string(),
        );

        parts.join("\n\n")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SubagentExecutorConfig::default();
        assert_eq!(config.max_concurrent, 5);
        assert_eq!(config.timeout, Duration::from_secs(120));
        assert_eq!(config.max_iterations, 10);
    }

    #[test]
    fn test_can_accept() {
        let executor = SubagentExecutor::new(SubagentExecutorConfig {
            max_concurrent: 2,
            ..Default::default()
        });
        assert!(executor.can_accept());
        assert_eq!(executor.active_count(), 0);
    }

    #[test]
    fn test_build_system_prompt() {
        let executor = SubagentExecutor::new(SubagentExecutorConfig::default());
        let agent = AgentConfig {
            id: "test".to_string(),
            name: "Test Agent".to_string(),
            avatar: None,
            model: None,
            primary_model: None,
            backup_model: None,
            provider: None,
            soul_path: None,
            system_prompt: Some("You are a helpful test agent.".to_string()),
            is_default: false,
            channel_bindings: Vec::new(),
            capabilities: vec![crate::config::AgentCapabilityConfig {
                name: "testing".to_string(),
                category: "skill".to_string(),
                description: "Run tests".to_string(),
            }],
            workspace: Default::default(),
        };

        let prompt = executor.build_system_prompt(&agent, "Run all tests", Some("ws-123"));
        assert!(prompt.contains("Test Agent"));
        assert!(prompt.contains("helpful test agent"));
        assert!(prompt.contains("testing"));
        assert!(prompt.contains("ws-123"));
    }
}
