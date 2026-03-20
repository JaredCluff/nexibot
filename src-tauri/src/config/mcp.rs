//! MCP, Computer Use, and Scheduled Tasks configurations.

use super::MCPServerConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}
fn default_tool_search_top_k() -> usize {
    15
}
fn default_tool_search_threshold() -> f64 {
    0.15
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPConfig {
    /// Enable MCP integration
    pub enabled: bool,

    /// Configured MCP servers
    pub servers: Vec<MCPServerConfig>,

    /// Semantic tool search configuration
    #[serde(default)]
    pub tool_search: ToolSearchConfig,
}

impl Default for MCPConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            servers: Vec::new(),
            tool_search: ToolSearchConfig::default(),
        }
    }
}

/// Configuration for semantic tool search over MCP tools.
/// When enabled, only the most relevant MCP tools are sent to the LLM
/// per query, reducing context window usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSearchConfig {
    /// Enable semantic tool search (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum number of MCP tools to include per query
    #[serde(default = "default_tool_search_top_k")]
    pub top_k: usize,

    /// Minimum cosine similarity threshold (0.0-1.0)
    #[serde(default = "default_tool_search_threshold")]
    pub similarity_threshold: f64,
}

impl Default for ToolSearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            top_k: default_tool_search_top_k(),
            similarity_threshold: default_tool_search_threshold(),
        }
    }
}

/// Computer Use configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputerUseConfig {
    /// Enable Computer Use capabilities
    pub enabled: bool,
    /// Display width for screenshots
    pub display_width: u32,
    /// Display height for screenshots
    pub display_height: u32,
    /// Require user confirmation before executing actions
    pub require_confirmation: bool,
}

impl Default for ComputerUseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            display_width: 1280,
            display_height: 800,
            require_confirmation: true,
        }
    }
}

/// Scheduled tasks configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScheduledTasksConfig {
    /// Whether the scheduler is enabled
    #[serde(default)]
    pub enabled: bool,
    /// List of scheduled tasks
    #[serde(default)]
    pub tasks: Vec<ScheduledTask>,
}

/// A scheduled task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// Unique task ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Schedule format: "daily HH:MM", "hourly", "every Nm", "weekly DAY HH:MM"
    pub schedule: String,
    /// Prompt to send to Claude
    pub prompt: String,
    /// Whether this task is enabled
    pub enabled: bool,
    /// Run if missed while app was closed
    pub run_if_missed: bool,
    /// Last execution time
    pub last_run: Option<DateTime<Utc>>,
}
