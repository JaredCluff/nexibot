//! Subagent orchestration with depth controls and result routing.
//!
//! Manages spawning of nested subagents with isolated sessions,
//! enforcing depth limits and tracking active subagent trees.
#![allow(dead_code)]

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tracing::{info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Status of a spawned subagent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    DepthExceeded,
}

/// Result produced by a subagent upon completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    pub text: String,
    pub tool_calls_made: u32,
    pub model_used: String,
    pub duration_ms: u64,
}

/// A single subagent spawn record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSpawn {
    pub id: String,
    pub parent_id: Option<String>,
    pub agent_id: String,
    pub task: String,
    pub depth: u32,
    pub max_depth: u32,
    pub status: SubagentStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub result: Option<SubagentResult>,
    /// Worktree path if this spawn was created with `Isolation::Worktree`.
    /// None for non-isolated spawns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<std::path::PathBuf>,
    /// Branch name for the worktree (used for cleanup).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
}

/// Configuration knobs for the orchestration subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationConfig {
    /// Maximum nesting depth for subagent chains.
    pub max_depth: u32,
    /// Maximum number of concurrently active (Pending | Running) spawns.
    pub max_concurrent: usize,
    /// Per-spawn timeout in seconds.
    pub timeout_seconds: u64,
    /// Whether subagent spawning is enabled at all.
    pub enabled: bool,
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_concurrent: 5,
            timeout_seconds: 300,
            enabled: true,
        }
    }
}

/// Isolation mode for a spawned subagent.
#[derive(Debug, Clone, Default)]
pub enum Isolation {
    #[default]
    None,
    Worktree,
}

/// Configuration for a single subagent spawn.
#[derive(Debug, Clone, Default)]
pub struct SpawnConfig {
    pub isolation: Isolation,
}

// ---------------------------------------------------------------------------
// OrchestrationManager
// ---------------------------------------------------------------------------

/// Manages the lifecycle of spawned subagents, enforcing depth and
/// concurrency limits while tracking parent-child relationships.
pub struct OrchestrationManager {
    /// Currently active (non-completed/non-failed) spawns keyed by spawn ID.
    active_spawns: HashMap<String, SubagentSpawn>,
    /// Completed / failed / cancelled spawns retained for history.
    completed_spawns: Vec<SubagentSpawn>,
    /// Configuration for the orchestration subsystem.
    config: OrchestrationConfig,
    /// Mapping from root spawn ID to all transitive descendant IDs (including root).
    tree_roots: HashMap<String, Vec<String>>,
}

impl OrchestrationManager {
    /// Create a new manager with the given configuration.
    pub fn new(config: OrchestrationConfig) -> Self {
        info!(
            "[ORCHESTRATION] Manager initialized (max_depth={}, max_concurrent={}, enabled={})",
            config.max_depth, config.max_concurrent, config.enabled
        );
        Self {
            active_spawns: HashMap::new(),
            completed_spawns: Vec::new(),
            config,
            tree_roots: HashMap::new(),
        }
    }

    /// Spawn a new subagent. Returns the spawn ID on success.
    ///
    /// Validates depth and concurrency limits before creating the spawn.
    pub fn spawn_subagent(
        &mut self,
        parent_id: Option<&str>,
        agent_id: &str,
        task: &str,
        config: Option<SpawnConfig>,
    ) -> Result<String> {
        if !self.config.enabled {
            anyhow::bail!("Subagent orchestration is disabled");
        }

        // Calculate depth from parent chain.
        let depth = match parent_id {
            Some(pid) => self.get_depth(pid) + 1,
            None => 0,
        };

        // Enforce depth limit.
        if depth >= self.config.max_depth {
            warn!(
                "[ORCHESTRATION] Depth exceeded: depth={} >= max_depth={} (parent={:?}, agent={})",
                depth, self.config.max_depth, parent_id, agent_id
            );
            anyhow::bail!(
                "Subagent depth limit exceeded: depth {} >= max_depth {}",
                depth,
                self.config.max_depth
            );
        }

        // Enforce concurrency limit.
        let active_count = self.get_active_count();
        if active_count >= self.config.max_concurrent {
            warn!(
                "[ORCHESTRATION] Concurrency limit reached: {} >= {} (agent={})",
                active_count, self.config.max_concurrent, agent_id
            );
            anyhow::bail!(
                "Max concurrent subagents reached: {} >= {}",
                active_count,
                self.config.max_concurrent
            );
        }

        let spawn_id = Uuid::new_v4().to_string();

        // Pre-compute worktree path/branch from spawn_id so it can be stored in the
        // record before the async creation task runs.  The path is deterministic and
        // matches the logic inside `create_agent_worktree`.
        let (worktree_path, worktree_branch) =
            if config.as_ref().map(|c| matches!(c.isolation, Isolation::Worktree)).unwrap_or(false) {
                let cwd = std::env::current_dir().unwrap_or_default();
                // Derive the path the same way create_agent_worktree does.
                // We can't call find_git_root synchronously here, so we use cwd as a
                // best-effort root anchor; find_git_root will resolve it properly in
                // the async task.
                let slug = format!("agent-{}", &spawn_id[..8.min(spawn_id.len())]);
                let branch = format!("worktree-{}", slug);
                let path = cwd.join(".nexibot").join("worktrees").join(&slug);
                (Some(path), Some(branch))
            } else {
                (None, None)
            };

        let spawn = SubagentSpawn {
            id: spawn_id.clone(),
            parent_id: parent_id.map(|s| s.to_string()),
            agent_id: agent_id.to_string(),
            task: task.to_string(),
            depth,
            max_depth: self.config.max_depth,
            status: SubagentStatus::Pending,
            created_at: Utc::now(),
            completed_at: None,
            result: None,
            worktree_path: worktree_path.clone(),
            worktree_branch: worktree_branch.clone(),
        };

        // Track in tree_roots: find the root of this spawn's chain and register.
        let root_id = match parent_id {
            Some(pid) => self.find_root(pid),
            None => spawn_id.clone(), // This spawn is its own root.
        };
        self.tree_roots
            .entry(root_id.clone())
            .or_default()
            .push(spawn_id.clone());

        info!(
            "[ORCHESTRATION] Spawned subagent id={} agent={} depth={} parent={:?} root={}",
            spawn_id, agent_id, depth, parent_id, root_id
        );

        self.active_spawns.insert(spawn_id.clone(), spawn);

        // Apply isolation if configured
        if let Some(cfg) = config {
            if matches!(cfg.isolation, Isolation::Worktree) {
                let cwd = std::env::current_dir().unwrap_or_default();
                // Spawn async worktree creation as best-effort (non-blocking)
                let spawn_id_for_wt = spawn_id.clone();
                tokio::spawn(async move {
                    if let Some(git_root) = crate::tools::worktree::find_git_root(&cwd).await {
                        match crate::tools::worktree::create_agent_worktree(&git_root, &spawn_id_for_wt).await {
                            Ok(path) => {
                                tracing::info!("Agent {} using worktree: {}", spawn_id_for_wt, path.display());
                            }
                            Err(e) => {
                                tracing::warn!("Failed to create worktree for agent {}: {}", spawn_id_for_wt, e);
                            }
                        }
                    }
                });
            }
        }

        Ok(spawn_id)
    }

    /// Walk the parent chain to determine the depth of a given spawn.
    ///
    /// Includes cycle detection via a visited set and a hard max-depth guard
    /// to prevent infinite loops from circular parent references.
    pub fn get_depth(&self, spawn_id: &str) -> u32 {
        let mut depth: u32 = 0;
        let mut current_id = spawn_id.to_string();
        let mut visited: HashSet<String> = HashSet::new();

        loop {
            // Guard: hard cap to prevent runaway traversal even without a
            // detectable cycle (e.g. extremely deep but legitimate chains).
            if depth > 100 {
                tracing::error!(
                    "[ORCHESTRATION] get_depth: max traversal depth (100) exceeded at id='{}'; breaking to prevent infinite loop",
                    current_id
                );
                break;
            }

            // Cycle detection: if we have already visited this node, a circular
            // parent reference exists. Log a warning and stop.
            if !visited.insert(current_id.clone()) {
                warn!(
                    "[ORCHESTRATION] get_depth: cycle detected at id='{}'; breaking to prevent infinite loop",
                    current_id
                );
                break;
            }

            let parent_id = self
                .active_spawns
                .get(&current_id)
                .or_else(|| self.completed_spawns.iter().find(|s| s.id == current_id))
                .and_then(|s| s.parent_id.clone());

            match parent_id {
                Some(pid) => {
                    depth += 1;
                    current_id = pid;
                }
                None => break,
            }
        }

        depth
    }

    /// Transition a spawn from Pending to Running.
    pub fn mark_running(&mut self, spawn_id: &str) -> Result<()> {
        let spawn = self
            .active_spawns
            .get_mut(spawn_id)
            .ok_or_else(|| anyhow::anyhow!("Spawn not found: {}", spawn_id))?;

        if spawn.status != SubagentStatus::Pending {
            anyhow::bail!(
                "Cannot mark spawn {} as Running: current status is {:?}",
                spawn_id,
                spawn.status
            );
        }

        spawn.status = SubagentStatus::Running;
        info!("[ORCHESTRATION] Spawn {} now Running", spawn_id);
        Ok(())
    }

    /// Mark a spawn as completed with its result, moving it to the completed list.
    pub fn mark_completed(&mut self, spawn_id: &str, result: SubagentResult) -> Result<()> {
        let mut spawn = self
            .active_spawns
            .remove(spawn_id)
            .ok_or_else(|| anyhow::anyhow!("Spawn not found: {}", spawn_id))?;

        spawn.status = SubagentStatus::Completed;
        spawn.completed_at = Some(Utc::now());
        spawn.result = Some(result);

        info!(
            "[ORCHESTRATION] Spawn {} completed (agent={}, depth={})",
            spawn_id, spawn.agent_id, spawn.depth
        );

        self.completed_spawns.push(spawn);
        Ok(())
    }

    /// Mark a spawn as failed, moving it to the completed list.
    pub fn mark_failed(&mut self, spawn_id: &str, error: &str) -> Result<()> {
        let mut spawn = self
            .active_spawns
            .remove(spawn_id)
            .ok_or_else(|| anyhow::anyhow!("Spawn not found: {}", spawn_id))?;

        spawn.status = SubagentStatus::Failed;
        spawn.completed_at = Some(Utc::now());
        spawn.result = Some(SubagentResult {
            text: format!("Error: {}", error),
            tool_calls_made: 0,
            model_used: String::new(),
            duration_ms: 0,
        });

        warn!(
            "[ORCHESTRATION] Spawn {} failed: {} (agent={}, depth={})",
            spawn_id, error, spawn.agent_id, spawn.depth
        );

        self.completed_spawns.push(spawn);
        Ok(())
    }

    /// Cancel a single spawn.
    pub fn cancel_spawn(&mut self, spawn_id: &str) -> Result<()> {
        let mut spawn = self
            .active_spawns
            .remove(spawn_id)
            .ok_or_else(|| anyhow::anyhow!("Spawn not found: {}", spawn_id))?;

        spawn.status = SubagentStatus::Cancelled;
        spawn.completed_at = Some(Utc::now());

        info!(
            "[ORCHESTRATION] Spawn {} cancelled (agent={})",
            spawn_id, spawn.agent_id
        );

        self.completed_spawns.push(spawn);
        Ok(())
    }

    /// Cancel a root spawn and all of its descendants. Returns the number
    /// of spawns cancelled.
    pub fn cancel_tree(&mut self, root_id: &str) -> usize {
        let ids_to_cancel: Vec<String> = self.tree_roots.get(root_id).cloned().unwrap_or_default();

        let mut cancelled = 0usize;

        for id in &ids_to_cancel {
            if let Some(mut spawn) = self.active_spawns.remove(id) {
                spawn.status = SubagentStatus::Cancelled;
                spawn.completed_at = Some(Utc::now());
                self.completed_spawns.push(spawn);
                cancelled += 1;
            }
        }

        if cancelled > 0 {
            info!(
                "[ORCHESTRATION] Cancelled tree rooted at {}: {} spawns cancelled",
                root_id, cancelled
            );
        }

        cancelled
    }

    /// Look up an active spawn by ID.
    pub fn get_spawn(&self, spawn_id: &str) -> Option<&SubagentSpawn> {
        self.active_spawns.get(spawn_id)
    }

    /// Return all active spawns whose parent_id matches the given ID.
    pub fn get_children(&self, parent_id: &str) -> Vec<&SubagentSpawn> {
        self.active_spawns
            .values()
            .filter(|s| s.parent_id.as_deref() == Some(parent_id))
            .collect()
    }

    /// Count of spawns that are Pending or Running.
    pub fn get_active_count(&self) -> usize {
        self.active_spawns
            .values()
            .filter(|s| s.status == SubagentStatus::Pending || s.status == SubagentStatus::Running)
            .count()
    }

    /// Remove completed spawns older than `max_age`.
    pub fn cleanup_completed(&mut self, max_age: std::time::Duration) {
        let cutoff =
            Utc::now() - chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::zero());
        let before = self.completed_spawns.len();

        self.completed_spawns
            .retain(|s| s.completed_at.map(|t| t > cutoff).unwrap_or(true));

        let removed = before - self.completed_spawns.len();
        if removed > 0 {
            info!(
                "[ORCHESTRATION] Cleaned up {} old completed spawns",
                removed
            );
        }
    }

    /// Get the orchestration configuration.
    pub fn config(&self) -> &OrchestrationConfig {
        &self.config
    }

    /// Get a completed spawn by ID (from history).
    pub fn get_completed_spawn(&self, spawn_id: &str) -> Option<&SubagentSpawn> {
        self.completed_spawns.iter().find(|s| s.id == spawn_id)
    }

    /// Produce an ASCII tree visualization of the spawn hierarchy rooted
    /// at `root_id`, useful for debugging.
    pub fn get_tree_visualization(&self, root_id: &str) -> String {
        let mut lines = Vec::new();
        let mut visited = std::collections::HashSet::new();
        self.build_tree_lines(root_id, "", true, &mut lines, &mut visited);
        if lines.is_empty() {
            return format!("(no tree found for root {})", root_id);
        }
        lines.join("\n")
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Walk the parent chain from `start_id` upward to find the tree root.
    /// Includes cycle detection to prevent infinite loops on corrupted data.
    fn find_root(&self, start_id: &str) -> String {
        let mut current = start_id.to_string();
        let mut visited = std::collections::HashSet::new();
        visited.insert(current.clone());
        loop {
            let parent = self
                .active_spawns
                .get(&current)
                .or_else(|| self.completed_spawns.iter().find(|s| s.id == current))
                .and_then(|s| s.parent_id.clone());

            match parent {
                Some(p) if visited.contains(&p) => {
                    warn!("[ORCHESTRATION] Cycle detected in parent chain at '{}', stopping", p);
                    break;
                }
                Some(p) => {
                    visited.insert(p.clone());
                    current = p;
                }
                None => break,
            }
        }
        current
    }

    /// Recursively build indented tree lines for visualization.
    fn build_tree_lines(
        &self,
        spawn_id: &str,
        prefix: &str,
        is_last: bool,
        lines: &mut Vec<String>,
        visited: &mut std::collections::HashSet<String>,
    ) {
        if !visited.insert(spawn_id.to_string()) {
            lines.push(format!("{}[cycle: {}]", prefix, spawn_id));
            return;
        }
        let spawn = self
            .active_spawns
            .get(spawn_id)
            .or_else(|| self.completed_spawns.iter().find(|s| s.id == spawn_id));

        let connector = if lines.is_empty() {
            "" // root has no connector
        } else if is_last {
            "└── "
        } else {
            "├── "
        };

        if let Some(s) = spawn {
            let short_id = if s.id.len() > 8 { &s.id[..8] } else { &s.id };
            lines.push(format!(
                "{}{}[{}] {} ({:?}) depth={}",
                prefix, connector, short_id, s.agent_id, s.status, s.depth
            ));
        } else {
            let short_id = if spawn_id.len() > 8 {
                &spawn_id[..8]
            } else {
                spawn_id
            };
            lines.push(format!("{}{}[{}] <missing>", prefix, connector, short_id));
        }

        // Gather children (from both active and completed).
        let children: Vec<String> = self
            .active_spawns
            .values()
            .chain(self.completed_spawns.iter())
            .filter(|s| s.parent_id.as_deref() == Some(spawn_id))
            .map(|s| s.id.clone())
            .collect();

        let new_prefix = if lines.len() <= 1 {
            String::new()
        } else if is_last {
            format!("{}    ", prefix)
        } else {
            format!("{}│   ", prefix)
        };

        for (i, child_id) in children.iter().enumerate() {
            let last = i == children.len() - 1;
            self.build_tree_lines(child_id, &new_prefix, last, lines, visited);
        }
    }
}

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

/// Returns the JSON tool definition for the `nexibot_orchestrate` tool.
/// This allows the lead agent to decompose a task and delegate subtasks
/// to specialized agents with dependency management.
pub fn nexibot_orchestrate_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "nexibot_orchestrate",
        "description": "Decompose a complex task and delegate subtasks to specialized agents. Subtasks without dependencies run in parallel. Each subtask is executed by its assigned agent and results are collected. Use this when a task benefits from multiple specialist agents working together.",
        "input_schema": {
            "type": "object",
            "properties": {
                "subtasks": {
                    "type": "array",
                    "description": "List of subtasks to execute",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for this subtask (used in depends_on references)"
                            },
                            "agent": {
                                "type": "string",
                                "description": "ID of the agent to handle this subtask"
                            },
                            "task": {
                                "type": "string",
                                "description": "Task description for the agent"
                            },
                            "depends_on": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "IDs of subtasks that must complete before this one starts"
                            }
                        },
                        "required": ["id", "agent", "task"]
                    }
                }
            },
            "required": ["subtasks"]
        }
    })
}

/// Tool definition for persistent DAG runs with retries and history.
pub fn nexibot_dag_run_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "nexibot_dag_run",
        "description": "Create and start a persistent multi-agent workflow (DAG). Unlike nexibot_orchestrate (ephemeral), DAG runs are persisted to SQLite, support retry policies with exponential backoff, maintain execution history, allow dynamic task addition during execution, and survive app restarts. Use for complex workflows that need reliability and auditability.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Human-readable name for this workflow run"
                },
                "tasks": {
                    "type": "array",
                    "description": "List of tasks to execute in dependency order",
                    "items": {
                        "type": "object",
                        "properties": {
                            "key": {
                                "type": "string",
                                "description": "Unique task key within this DAG (e.g. 'research', 'analyze')"
                            },
                            "agent_id": {
                                "type": "string",
                                "description": "ID of the agent to execute this task"
                            },
                            "description": {
                                "type": "string",
                                "description": "Task instructions for the agent"
                            },
                            "depends_on": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Keys of tasks that must complete before this one starts"
                            },
                            "max_retries": {
                                "type": "integer",
                                "description": "Maximum retry attempts on failure (0 = no retry, default 0)"
                            },
                            "retry_delay_ms": {
                                "type": "integer",
                                "description": "Initial retry delay in ms, doubles each attempt (default 1000)"
                            }
                        },
                        "required": ["key", "agent_id", "description"]
                    }
                }
            },
            "required": ["name", "tasks"]
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_manager() -> OrchestrationManager {
        OrchestrationManager::new(OrchestrationConfig::default())
    }

    #[test]
    fn test_depth_calculation_nested_spawns() {
        let mut mgr = default_manager();

        // Root spawn (depth 0).
        let root = mgr.spawn_subagent(None, "agent-a", "root task", None).unwrap();
        assert_eq!(mgr.get_depth(&root), 0);

        // Child spawn (depth 1).
        let child = mgr
            .spawn_subagent(Some(&root), "agent-b", "child task", None)
            .unwrap();
        assert_eq!(mgr.get_depth(&child), 1);

        // Grandchild spawn (depth 2).
        let grandchild = mgr
            .spawn_subagent(Some(&child), "agent-c", "grandchild task", None)
            .unwrap();
        assert_eq!(mgr.get_depth(&grandchild), 2);
    }

    #[test]
    fn test_max_depth_enforcement() {
        let config = OrchestrationConfig {
            max_depth: 2,
            ..Default::default()
        };
        let mut mgr = OrchestrationManager::new(config);

        let root = mgr.spawn_subagent(None, "agent-a", "task 0", None).unwrap();
        let child = mgr
            .spawn_subagent(Some(&root), "agent-b", "task 1", None)
            .unwrap();

        // Depth 2 should be rejected (>= max_depth of 2).
        let result = mgr.spawn_subagent(Some(&child), "agent-c", "task 2", None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("depth limit exceeded"),
            "Expected depth limit error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_max_concurrent_enforcement() {
        let config = OrchestrationConfig {
            max_concurrent: 2,
            ..Default::default()
        };
        let mut mgr = OrchestrationManager::new(config);

        let _s1 = mgr.spawn_subagent(None, "agent-a", "task 1", None).unwrap();
        let _s2 = mgr.spawn_subagent(None, "agent-b", "task 2", None).unwrap();

        // Third spawn should fail.
        let result = mgr.spawn_subagent(None, "agent-c", "task 3", None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Max concurrent"),
            "Expected concurrency error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_cancel_tree_cascading() {
        let mut mgr = default_manager();

        let root = mgr.spawn_subagent(None, "agent-a", "root", None).unwrap();
        let child1 = mgr
            .spawn_subagent(Some(&root), "agent-b", "child1", None)
            .unwrap();
        let child2 = mgr
            .spawn_subagent(Some(&root), "agent-c", "child2", None)
            .unwrap();
        let _grandchild = mgr
            .spawn_subagent(Some(&child1), "agent-d", "grandchild", None)
            .unwrap();

        assert_eq!(mgr.get_active_count(), 4);

        let cancelled = mgr.cancel_tree(&root);
        assert_eq!(cancelled, 4);
        assert_eq!(mgr.get_active_count(), 0);

        // Verify all moved to completed with Cancelled status.
        assert!(mgr
            .completed_spawns
            .iter()
            .all(|s| s.status == SubagentStatus::Cancelled));
    }

    #[test]
    fn test_mark_completed_and_result_storage() {
        let mut mgr = default_manager();

        let id = mgr.spawn_subagent(None, "agent-a", "some task", None).unwrap();
        mgr.mark_running(&id).unwrap();

        let result = SubagentResult {
            text: "Done!".to_string(),
            tool_calls_made: 3,
            model_used: "claude-opus-4-6".to_string(),
            duration_ms: 1500,
        };

        mgr.mark_completed(&id, result.clone()).unwrap();

        // Should no longer be in active spawns.
        assert!(mgr.get_spawn(&id).is_none());

        // Should be in completed spawns.
        let completed = mgr.completed_spawns.iter().find(|s| s.id == id).unwrap();
        assert_eq!(completed.status, SubagentStatus::Completed);
        assert!(completed.completed_at.is_some());

        let r = completed.result.as_ref().unwrap();
        assert_eq!(r.text, "Done!");
        assert_eq!(r.tool_calls_made, 3);
        assert_eq!(r.model_used, "claude-opus-4-6");
        assert_eq!(r.duration_ms, 1500);
    }

    #[test]
    fn test_mark_failed() {
        let mut mgr = default_manager();

        let id = mgr.spawn_subagent(None, "agent-a", "failing task", None).unwrap();
        mgr.mark_running(&id).unwrap();
        mgr.mark_failed(&id, "timeout reached").unwrap();

        let failed = mgr.completed_spawns.iter().find(|s| s.id == id).unwrap();
        assert_eq!(failed.status, SubagentStatus::Failed);
        assert!(failed
            .result
            .as_ref()
            .unwrap()
            .text
            .contains("timeout reached"));
    }

    #[test]
    fn test_get_children() {
        let mut mgr = default_manager();

        let root = mgr.spawn_subagent(None, "agent-a", "root", None).unwrap();
        let _c1 = mgr
            .spawn_subagent(Some(&root), "agent-b", "child1", None)
            .unwrap();
        let _c2 = mgr
            .spawn_subagent(Some(&root), "agent-c", "child2", None)
            .unwrap();

        let children = mgr.get_children(&root);
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn test_get_tree_visualization() {
        let mut mgr = default_manager();

        let root = mgr.spawn_subagent(None, "agent-root", "root task", None).unwrap();
        let child = mgr
            .spawn_subagent(Some(&root), "agent-child", "child task", None)
            .unwrap();
        let _leaf = mgr
            .spawn_subagent(Some(&child), "agent-leaf", "leaf task", None)
            .unwrap();

        let viz = mgr.get_tree_visualization(&root);

        // Should contain all three agents.
        assert!(
            viz.contains("agent-root"),
            "Visualization missing agent-root: {}",
            viz
        );
        assert!(
            viz.contains("agent-child"),
            "Visualization missing agent-child: {}",
            viz
        );
        assert!(
            viz.contains("agent-leaf"),
            "Visualization missing agent-leaf: {}",
            viz
        );

        // Should have tree connectors.
        assert!(
            viz.contains("──") || viz.contains("├") || viz.contains("└"),
            "Visualization missing tree connectors: {}",
            viz
        );
    }

    #[test]
    fn test_cleanup_completed() {
        let mut mgr = default_manager();

        let id = mgr.spawn_subagent(None, "agent-a", "task", None).unwrap();
        mgr.mark_running(&id).unwrap();
        mgr.mark_completed(
            &id,
            SubagentResult {
                text: "done".to_string(),
                tool_calls_made: 0,
                model_used: "test".to_string(),
                duration_ms: 100,
            },
        )
        .unwrap();

        assert_eq!(mgr.completed_spawns.len(), 1);

        // Cleanup with zero max_age should remove everything.
        mgr.cleanup_completed(std::time::Duration::from_secs(0));
        assert_eq!(mgr.completed_spawns.len(), 0);
    }

    #[test]
    fn test_disabled_orchestration() {
        let config = OrchestrationConfig {
            enabled: false,
            ..Default::default()
        };
        let mut mgr = OrchestrationManager::new(config);

        let result = mgr.spawn_subagent(None, "agent-a", "task", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("disabled"));
    }

    #[test]
    fn test_cancel_single_spawn() {
        let mut mgr = default_manager();

        let id = mgr.spawn_subagent(None, "agent-a", "cancellable", None).unwrap();
        assert_eq!(mgr.get_active_count(), 1);

        mgr.cancel_spawn(&id).unwrap();
        assert_eq!(mgr.get_active_count(), 0);
        assert_eq!(mgr.completed_spawns.len(), 1);
        assert_eq!(mgr.completed_spawns[0].status, SubagentStatus::Cancelled);
    }

    #[test]
    fn test_mark_running_wrong_status() {
        let mut mgr = default_manager();

        let id = mgr.spawn_subagent(None, "agent-a", "task", None).unwrap();
        mgr.mark_running(&id).unwrap();

        // Trying to mark Running again should fail.
        let result = mgr.mark_running(&id);
        assert!(result.is_err());
    }
}
