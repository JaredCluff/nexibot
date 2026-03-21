//! Per-agent workspace isolation.
//!
//! Provides isolated directories for sessions, skills, memory, and scratch
//! space for each agent. This prevents cross-agent data contamination and
//! enables fine-grained resource management.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Maximum scratch space size in megabytes (default).
const DEFAULT_MAX_SCRATCH_MB: u64 = 100;

/// Configuration for an agent's workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    /// Whether this agent has an isolated workspace.
    #[serde(default)]
    pub isolated: bool,
    /// Inherit skills from the global skills directory.
    #[serde(default = "default_true")]
    pub inherit_skills: bool,
    /// Inherit memory from the global memory store.
    #[serde(default)]
    pub inherit_memory: bool,
    /// Maximum scratch space size in megabytes.
    #[serde(default = "default_max_scratch_mb")]
    pub max_scratch_mb: u64,
}

fn default_true() -> bool {
    true
}

fn default_max_scratch_mb() -> u64 {
    DEFAULT_MAX_SCRATCH_MB
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            isolated: false,
            inherit_skills: true,
            inherit_memory: false,
            max_scratch_mb: DEFAULT_MAX_SCRATCH_MB,
        }
    }
}

/// Paths for an agent's isolated workspace.
#[derive(Debug, Clone)]
pub struct AgentWorkspace {
    /// Agent ID this workspace belongs to.
    pub agent_id: String,
    /// Root directory for this agent's workspace.
    pub root: PathBuf,
    /// Sessions directory.
    pub sessions_dir: PathBuf,
    /// Skills directory.
    pub skills_dir: PathBuf,
    /// Memory directory.
    pub memory_dir: PathBuf,
    /// Scratch space directory.
    pub scratch_dir: PathBuf,
    /// Workspace configuration.
    pub config: WorkspaceConfig,
}

impl AgentWorkspace {
    /// Create a new agent workspace.
    ///
    /// Creates all necessary directories if they don't exist.
    pub fn new(agent_id: &str, config: WorkspaceConfig) -> Result<Self> {
        let base_dir = Self::agents_base_dir()?;
        let root = base_dir.join(agent_id);

        let workspace = Self {
            agent_id: agent_id.to_string(),
            sessions_dir: root.join("sessions"),
            skills_dir: root.join("skills"),
            memory_dir: root.join("memory"),
            scratch_dir: root.join("scratch"),
            root,
            config,
        };

        workspace.ensure_directories()?;

        info!(
            "[WORKSPACE] Created agent workspace: {} at {:?}",
            agent_id, workspace.root
        );

        Ok(workspace)
    }

    /// Get the base directory for all agent workspaces.
    fn agents_base_dir() -> Result<PathBuf> {
        let config_dir = if cfg!(target_os = "macos") {
            dirs::home_dir()
                .map(|h| h.join("Library/Application Support/ai.nexibot.desktop/agents"))
        } else if cfg!(target_os = "windows") {
            dirs::data_dir().map(|d| d.join("nexibot/agents"))
        } else {
            dirs::home_dir().map(|h| h.join(".config/nexibot/agents"))
        };

        config_dir.context("Could not determine agent workspace base directory")
    }

    /// Ensure all workspace directories exist.
    fn ensure_directories(&self) -> Result<()> {
        for dir in [
            &self.sessions_dir,
            &self.skills_dir,
            &self.memory_dir,
            &self.scratch_dir,
        ] {
            if !dir.exists() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("Failed to create directory: {:?}", dir))?;
                debug!("[WORKSPACE] Created directory: {:?}", dir);
            }
        }
        Ok(())
    }

    /// Get the scratch space usage in bytes.
    pub fn scratch_usage_bytes(&self) -> u64 {
        dir_size(&self.scratch_dir)
    }

    /// Check if scratch space usage exceeds the configured limit.
    pub fn scratch_exceeds_limit(&self) -> bool {
        let usage_mb = self.scratch_usage_bytes() / (1024 * 1024);
        usage_mb > self.config.max_scratch_mb
    }

    /// Clean up the scratch directory.
    pub fn clean_scratch(&self) -> Result<usize> {
        let mut removed = 0;
        if self.scratch_dir.exists() {
            for entry in std::fs::read_dir(&self.scratch_dir)?.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if std::fs::remove_file(&path).is_ok() {
                        removed += 1;
                    }
                } else if path.is_dir() {
                    if std::fs::remove_dir_all(&path).is_ok() {
                        removed += 1;
                    }
                }
            }
        }
        if removed > 0 {
            info!(
                "[WORKSPACE] Cleaned {} items from scratch for agent {}",
                removed, self.agent_id
            );
        }
        Ok(removed)
    }

    /// Remove the entire workspace (for agent cleanup).
    pub fn remove(self) -> Result<()> {
        if self.root.exists() {
            std::fs::remove_dir_all(&self.root)
                .with_context(|| format!("Failed to remove workspace: {:?}", self.root))?;
            info!("[WORKSPACE] Removed workspace for agent: {}", self.agent_id);
        }
        Ok(())
    }
}

/// Calculate the total size of a directory in bytes.
fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }

    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_file() {
                total += meta.len();
            } else if meta.is_dir() {
                total += dir_size(&entry.path());
            }
        }
    }
    total
}

/// Manager for all agent workspaces.
pub struct WorkspaceManager {
    workspaces: std::collections::HashMap<String, AgentWorkspace>,
}

impl WorkspaceManager {
    /// Create a new workspace manager.
    pub fn new() -> Self {
        Self {
            workspaces: std::collections::HashMap::new(),
        }
    }

    /// Get or create a workspace for an agent.
    pub fn get_or_create(
        &mut self,
        agent_id: &str,
        config: WorkspaceConfig,
    ) -> Result<&AgentWorkspace> {
        if !self.workspaces.contains_key(agent_id) {
            let workspace = AgentWorkspace::new(agent_id, config)?;
            self.workspaces.insert(agent_id.to_string(), workspace);
        }
        Ok(&self.workspaces[agent_id])
    }

    /// Get a workspace if it exists.
    pub fn get(&self, agent_id: &str) -> Option<&AgentWorkspace> {
        self.workspaces.get(agent_id)
    }

    /// List all managed workspaces.
    pub fn list(&self) -> Vec<&AgentWorkspace> {
        self.workspaces.values().collect()
    }

    /// Remove a workspace.
    pub fn remove(&mut self, agent_id: &str) -> Result<()> {
        if let Some(workspace) = self.workspaces.remove(agent_id) {
            workspace.remove()?;
        }
        Ok(())
    }

    /// Clean scratch space for all agents that exceed their limits.
    pub fn enforce_scratch_limits(&mut self) -> usize {
        let mut total_cleaned = 0;
        let exceeding: Vec<String> = self
            .workspaces
            .iter()
            .filter(|(_, ws)| ws.scratch_exceeds_limit())
            .map(|(id, _)| id.clone())
            .collect();

        for agent_id in exceeding {
            if let Some(ws) = self.workspaces.get(&agent_id) {
                match ws.clean_scratch() {
                    Ok(count) => total_cleaned += count,
                    Err(e) => warn!(
                        "[WORKSPACE] Failed to clean scratch for {}: {}",
                        agent_id, e
                    ),
                }
            }
        }

        total_cleaned
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_workspace_config_defaults() {
        let config = WorkspaceConfig::default();
        assert!(!config.isolated);
        assert!(config.inherit_skills);
        assert!(!config.inherit_memory);
        assert_eq!(config.max_scratch_mb, DEFAULT_MAX_SCRATCH_MB);
    }

    #[test]
    fn test_workspace_config_serialization() {
        let config = WorkspaceConfig {
            isolated: true,
            inherit_skills: true,
            inherit_memory: false,
            max_scratch_mb: 50,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: WorkspaceConfig = serde_json::from_str(&json).unwrap();
        assert!(deserialized.isolated);
        assert_eq!(deserialized.max_scratch_mb, 50);
    }

    #[test]
    fn test_dir_size_empty() {
        let dir = TempDir::new().unwrap();
        assert_eq!(dir_size(dir.path()), 0);
    }

    #[test]
    fn test_dir_size_with_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("file1.txt"), "hello").unwrap();
        std::fs::write(dir.path().join("file2.txt"), "world!").unwrap();
        let size = dir_size(dir.path());
        assert!(size > 0);
        assert_eq!(size, 11); // "hello" (5) + "world!" (6)
    }

    #[test]
    fn test_workspace_manager_new() {
        let manager = WorkspaceManager::new();
        assert!(manager.list().is_empty());
    }

    #[test]
    fn test_default_workspace_config_backward_compat() {
        // Test that missing fields deserialize correctly (backward compat)
        let json = "{}";
        let config: WorkspaceConfig = serde_json::from_str(json).unwrap();
        assert!(!config.isolated);
        assert!(config.inherit_skills);
        assert!(!config.inherit_memory);
        assert_eq!(config.max_scratch_mb, DEFAULT_MAX_SCRATCH_MB);
    }
}
