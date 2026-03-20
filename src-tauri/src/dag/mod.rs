//! Agent Task DAG — persistent, retryable multi-agent workflow execution.
//!
//! A DAG (Directed Acyclic Graph) defines a set of tasks with dependency ordering.
//! Tasks execute in parallel rounds: all tasks whose dependencies are satisfied
//! run concurrently, and results propagate to downstream tasks as context.

pub mod executor;
pub mod store;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Status enums ─────────────────────────────────────────────────────

/// Overall status of a DAG run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DagRunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Status of an individual task within a DAG run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DagTaskStatus {
    Pending,
    Blocked,
    Running,
    Completed,
    Failed,
    Cancelled,
    Retrying,
}

// ── Definition types (templates / inline specs) ──────────────────────

/// A single task definition within a DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagTaskDefinition {
    /// User-facing ID within the DAG (e.g. "research", "summarize").
    pub key: String,
    /// Which agent executes this task.
    pub agent_id: String,
    /// Task instructions sent to the agent.
    pub description: String,
    /// Keys of tasks that must complete before this one can start.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Maximum retry attempts (0 = no retry).
    #[serde(default)]
    pub max_retries: u32,
    /// Initial retry delay in milliseconds (doubles each attempt).
    #[serde(default = "default_retry_delay")]
    pub retry_delay_ms: u64,
}

fn default_retry_delay() -> u64 {
    1000
}

/// A complete DAG definition (can be saved as a reusable template).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagDefinition {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub tasks: Vec<DagTaskDefinition>,
    #[serde(default)]
    pub is_template: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ── Runtime types (run instances) ────────────────────────────────────

/// A single execution (run) of a DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagRun {
    pub id: String,
    pub definition_id: Option<String>,
    pub name: String,
    pub status: DagRunStatus,
    pub workspace_id: String,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub error: Option<String>,
}

/// A task instance within a running DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagTask {
    pub id: String,
    pub run_id: String,
    pub task_key: String,
    pub agent_id: String,
    pub description: String,
    pub status: DagTaskStatus,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    pub attempt: u32,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// A history event recorded during DAG execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagHistoryEntry {
    pub run_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub event_type: String,
    #[serde(default)]
    pub details: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Complete summary of a DAG run (run + tasks + history).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagRunSummary {
    pub run: DagRun,
    pub tasks: Vec<DagTask>,
    pub history: Vec<DagHistoryEntry>,
}

// ── Display impls ────────────────────────────────────────────────────

impl std::fmt::Display for DagRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::fmt::Display for DagTaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Blocked => write!(f, "blocked"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Retrying => write!(f, "retrying"),
        }
    }
}

impl DagRunStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Pending,
        }
    }
}

impl DagTaskStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "blocked" => Self::Blocked,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "retrying" => Self::Retrying,
            _ => Self::Pending,
        }
    }
}
