use crate::tool_registry::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

// ─── Data Model ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    BackgroundCommand,
    BackgroundAgent,
    Scheduled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub task_type: TaskType,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub output_path: PathBuf,
    pub metadata: HashMap<String, Value>,
}

// ─── Task Store ─────────────────────────────────────────────────────────────

const MAX_FINISHED_TOOL_TASKS: usize = 500;

pub struct TaskStore {
    tasks: HashMap<String, BackgroundTask>,
    pub data_dir: PathBuf,
    next_id: u64,
}

impl TaskStore {
    pub fn new(data_dir: PathBuf) -> Self {
        TaskStore { tasks: HashMap::new(), data_dir, next_id: 1 }
    }

    pub fn next_id(&mut self) -> String {
        let id = format!("task_{:06}", self.next_id);
        self.next_id += 1;
        id
    }

    pub fn insert(&mut self, task: BackgroundTask) {
        self.tasks.insert(task.id.clone(), task);
        // Evict oldest finished tasks to bound memory growth
        let finished_count = self.tasks.values()
            .filter(|t| matches!(t.status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled))
            .count();
        if finished_count > MAX_FINISHED_TOOL_TASKS {
            let mut finished: Vec<_> = self.tasks.values()
                .filter(|t| matches!(t.status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled))
                .map(|t| (t.id.clone(), t.created_at))
                .collect();
            finished.sort_by_key(|(_, ts)| *ts);
            let to_remove = finished_count - MAX_FINISHED_TOOL_TASKS;
            for (id, _) in finished.into_iter().take(to_remove) {
                self.tasks.remove(&id);
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&BackgroundTask> {
        self.tasks.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut BackgroundTask> {
        self.tasks.get_mut(id)
    }

    pub fn list_all(&self) -> Vec<&BackgroundTask> {
        let mut v: Vec<_> = self.tasks.values().collect();
        v.sort_by_key(|t| &t.created_at);
        v
    }

    pub fn output_path_for(&self, id: &str) -> PathBuf {
        self.data_dir.join(id).join("output.log")
    }

    pub fn task_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join(id)
    }

    pub async fn persist(&self, task: &BackgroundTask) -> anyhow::Result<()> {
        let dir = self.task_dir(&task.id);
        tokio::fs::create_dir_all(&dir).await?;
        let json = serde_json::to_string_pretty(task)?;
        tokio::fs::write(dir.join("task.json"), json).await?;
        Ok(())
    }

    pub async fn load_from_disk(&mut self) -> anyhow::Result<()> {
        if !self.data_dir.exists() { return Ok(()); }
        let mut rd = tokio::fs::read_dir(&self.data_dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            let task_json = entry.path().join("task.json");
            if task_json.exists() {
                let data = tokio::fs::read_to_string(&task_json).await?;
                if let Ok(mut task) = serde_json::from_str::<BackgroundTask>(&data) {
                    if task.status == TaskStatus::Running {
                        task.status = TaskStatus::Failed;
                        task.metadata.insert(
                            "failure_reason".to_string(),
                            Value::String("orphaned on restart".to_string()),
                        );
                    }
                    self.tasks.insert(task.id.clone(), task);
                }
            }
        }
        Ok(())
    }
}

// ─── Tool: nexibot_task_create ───────────────────────────────────────────────

pub struct TaskCreateTool(pub Arc<RwLock<TaskStore>>);

#[async_trait]
impl Tool for TaskCreateTool {
    fn name(&self) -> &str { "nexibot_task_create" }
    fn description(&self) -> &str {
        "Spawn a background shell command. Returns a task ID for polling with nexibot_task_get."
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "subject": { "type": "string" },
                "command": { "type": "string", "description": "Shell command to run in background" },
                "description": { "type": "string" }
            },
            "required": ["subject", "command"]
        })
    }
    async fn check_permissions(&self, _: &Value, _: &ToolContext) -> PermissionDecision {
        PermissionDecision::Ask {
            reason: "This will spawn a background command".to_string(),
            details: None,
        }
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        let subject = input["subject"].as_str().unwrap_or("Background task").to_string();
        let command = match input["command"].as_str() {
            Some(c) => c.to_string(),
            None => return ToolResult::err("command is required"),
        };
        let description = input["description"].as_str().unwrap_or("").to_string();

        let mut store = self.0.write().await;
        let id = store.next_id();
        let output_path = store.output_path_for(&id);
        let task_dir = store.task_dir(&id);

        let task = BackgroundTask {
            id: id.clone(),
            subject,
            description,
            status: TaskStatus::Running,
            task_type: TaskType::BackgroundCommand,
            created_at: Utc::now(),
            completed_at: None,
            output_path: output_path.clone(),
            metadata: HashMap::new(),
        };
        store.insert(task.clone());
        drop(store);

        let task_dir_c = task_dir.clone();
        let store_c = self.0.clone();
        let id_c = id.clone();
        tokio::spawn(async move {
            let _ = tokio::fs::create_dir_all(&task_dir_c).await;
            let out_file = tokio::fs::File::create(&output_path).await;
            let status = if let Ok(file) = out_file {
                let file = file.into_std().await;
                if let Ok(stderr_file) = file.try_clone() {
                    tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&command)
                        .stdout(std::process::Stdio::from(file))
                        .stderr(std::process::Stdio::from(stderr_file))
                        .status()
                        .await
                        .ok()
                } else {
                    None
                }
            } else {
                None
            };

            let new_status = match status {
                Some(s) if s.success() => TaskStatus::Completed,
                _ => TaskStatus::Failed,
            };

            let mut store = store_c.write().await;
            if let Some(task) = store.get_mut(&id_c) {
                task.status = new_status;
                task.completed_at = Some(Utc::now());
            }
            if let Some(task) = store.get(&id_c) {
                let _ = store.persist(task).await;
            }
        });

        ToolResult::ok(format!(
            "Task created: {}\nUse nexibot_task_get with id=\"{}\" to check progress.",
            id, id
        ))
    }
}

// ─── Tool: nexibot_task_get ──────────────────────────────────────────────────

pub struct TaskGetTool(pub Arc<RwLock<TaskStore>>);

#[async_trait]
impl Tool for TaskGetTool {
    fn name(&self) -> &str { "nexibot_task_get" }
    fn description(&self) -> &str { "Get task status and last 4KB of output." }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" }
            },
            "required": ["id"]
        })
    }
    fn is_read_only(&self, _: &Value) -> bool { true }
    fn is_concurrency_safe(&self) -> bool { true }
    async fn check_permissions(&self, _: &Value, _: &ToolContext) -> PermissionDecision {
        PermissionDecision::Allow
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        let id = match input["id"].as_str() {
            Some(i) => i,
            None => return ToolResult::err("id is required"),
        };
        let store = self.0.read().await;
        let task = match store.get(id) {
            Some(t) => t.clone(),
            None => return ToolResult::err(format!("Task {} not found", id)),
        };
        drop(store);

        let tail = read_tail(&task.output_path, 4096).await;
        let status_str = match task.status {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        };

        ToolResult::ok(format!(
            "Task: {}\nSubject: {}\nStatus: {}\nCreated: {}\n\nRecent output:\n{}",
            task.id, task.subject, status_str,
            task.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
            tail
        ))
    }
}

// ─── Tool: nexibot_task_list ─────────────────────────────────────────────────

pub struct TaskListTool(pub Arc<RwLock<TaskStore>>);

#[async_trait]
impl Tool for TaskListTool {
    fn name(&self) -> &str { "nexibot_task_list" }
    fn description(&self) -> &str { "List all background tasks with their status." }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "status": { "type": "string", "description": "Filter by status: running, completed, failed, pending" }
            }
        })
    }
    fn is_read_only(&self, _: &Value) -> bool { true }
    fn is_concurrency_safe(&self) -> bool { true }
    async fn check_permissions(&self, _: &Value, _: &ToolContext) -> PermissionDecision {
        PermissionDecision::Allow
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        let status_filter = input["status"].as_str();
        let store = self.0.read().await;
        let tasks = store.list_all();
        if tasks.is_empty() {
            return ToolResult::ok("No tasks.");
        }
        let mut lines = Vec::new();
        for task in tasks {
            let status_str = format!("{:?}", task.status).to_lowercase();
            if let Some(filter) = status_filter {
                if !status_str.contains(filter) { continue; }
            }
            lines.push(format!(
                "[{}] {} — {} ({})",
                task.id, task.subject, status_str,
                task.created_at.format("%H:%M:%S")
            ));
        }
        ToolResult::ok(if lines.is_empty() {
            "No tasks match the filter.".to_string()
        } else {
            lines.join("\n")
        })
    }
}

// ─── Tool: nexibot_task_output ───────────────────────────────────────────────

pub struct TaskOutputTool(pub Arc<RwLock<TaskStore>>);

#[async_trait]
impl Tool for TaskOutputTool {
    fn name(&self) -> &str { "nexibot_task_output" }
    fn description(&self) -> &str { "Read full output of a task with optional offset/limit (in bytes)." }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "offset": { "type": "integer" },
                "limit": { "type": "integer" }
            },
            "required": ["id"]
        })
    }
    fn is_read_only(&self, _: &Value) -> bool { true }
    fn is_concurrency_safe(&self) -> bool { true }
    async fn check_permissions(&self, _: &Value, _: &ToolContext) -> PermissionDecision {
        PermissionDecision::Allow
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        let id = match input["id"].as_str() {
            Some(i) => i,
            None => return ToolResult::err("id is required"),
        };
        let offset = input["offset"].as_u64().and_then(|v| usize::try_from(v).ok()).unwrap_or(0);
        let limit = input["limit"].as_u64().and_then(|v| usize::try_from(v).ok()).unwrap_or(65536);

        let store = self.0.read().await;
        let task = match store.get(id) {
            Some(t) => t.clone(),
            None => return ToolResult::err(format!("Task {} not found", id)),
        };
        drop(store);

        match tokio::fs::read(&task.output_path).await {
            Ok(bytes) => {
                let end = (offset + limit).min(bytes.len());
                let slice = if offset < bytes.len() { &bytes[offset..end] } else { &[] };
                let text = String::from_utf8_lossy(slice);
                ToolResult::ok(format!(
                    "Output for {} (bytes {}-{} of {}):\n{}",
                    id, offset, end, bytes.len(), text
                ))
            }
            Err(_) => ToolResult::ok(format!("No output yet for task {}", id)),
        }
    }
}

// ─── Tool: nexibot_task_stop ─────────────────────────────────────────────────

pub struct TaskStopTool(pub Arc<RwLock<TaskStore>>);

#[async_trait]
impl Tool for TaskStopTool {
    fn name(&self) -> &str { "nexibot_task_stop" }
    fn description(&self) -> &str { "Stop a running background task." }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" }
            },
            "required": ["id"]
        })
    }
    async fn check_permissions(&self, _: &Value, _: &ToolContext) -> PermissionDecision {
        PermissionDecision::Allow
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        let id = match input["id"].as_str() {
            Some(i) => i,
            None => return ToolResult::err("id is required"),
        };
        let mut store = self.0.write().await;
        match store.get_mut(id) {
            None => ToolResult::err(format!("Task {} not found", id)),
            Some(task) => {
                if task.status == TaskStatus::Running {
                    task.status = TaskStatus::Cancelled;
                    task.completed_at = Some(Utc::now());
                    ToolResult::ok(format!("Task {} cancelled.", id))
                } else {
                    ToolResult::ok(format!("Task {} is not running (status: {:?}).", id, task.status))
                }
            }
        }
    }
}

// ─── Helper: read last N bytes ───────────────────────────────────────────────

async fn read_tail(path: &PathBuf, max_bytes: usize) -> String {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let start = bytes.len().saturating_sub(max_bytes);
            String::from_utf8_lossy(&bytes[start..]).to_string()
        }
        Err(_) => "(no output yet)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> Arc<RwLock<TaskStore>> {
        let dir = tempfile::tempdir().unwrap().into_path();
        Arc::new(RwLock::new(TaskStore::new(dir)))
    }

    #[test]
    fn test_task_store_next_id_increments() {
        let dir = tempfile::tempdir().unwrap().into_path();
        let mut store = TaskStore::new(dir);
        assert_eq!(store.next_id(), "task_000001");
        assert_eq!(store.next_id(), "task_000002");
    }

    #[test]
    fn test_task_store_insert_and_get() {
        let dir = tempfile::tempdir().unwrap().into_path();
        let mut store = TaskStore::new(dir.clone());
        let id = store.next_id();
        let task = BackgroundTask {
            id: id.clone(),
            subject: "test".into(),
            description: "".into(),
            status: TaskStatus::Pending,
            task_type: TaskType::BackgroundCommand,
            created_at: Utc::now(),
            completed_at: None,
            output_path: dir.join(&id).join("output.log"),
            metadata: HashMap::new(),
        };
        store.insert(task);
        assert!(store.get(&id).is_some());
    }

    #[tokio::test]
    async fn test_task_list_empty() {
        let store = make_store();
        let tool = TaskListTool(store);
        let ctx = ToolContext {
            session_key: "s".into(), agent_id: "a".into(),
            working_dir: std::path::PathBuf::from("/tmp"),
        };
        let result = tool.call(serde_json::json!({}), ctx).await;
        assert!(result.content.contains("No tasks"));
    }

    #[tokio::test]
    async fn test_task_get_unknown_id() {
        let store = make_store();
        let tool = TaskGetTool(store);
        let ctx = ToolContext {
            session_key: "s".into(), agent_id: "a".into(),
            working_dir: std::path::PathBuf::from("/tmp"),
        };
        let result = tool.call(serde_json::json!({"id": "task_999999"}), ctx).await;
        assert!(!result.success);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_task_stop_non_running() {
        let store = make_store();
        {
            let mut s = store.write().await;
            let id = s.next_id();
            let dir = s.data_dir.clone();
            s.insert(BackgroundTask {
                id: id.clone(),
                subject: "done".into(),
                description: "".into(),
                status: TaskStatus::Completed,
                task_type: TaskType::BackgroundCommand,
                created_at: Utc::now(),
                completed_at: Some(Utc::now()),
                output_path: dir.join(&id).join("output.log"),
                metadata: HashMap::new(),
            });
        }
        let tool = TaskStopTool(store.clone());
        let ctx = ToolContext {
            session_key: "s".into(), agent_id: "a".into(),
            working_dir: std::path::PathBuf::from("/tmp"),
        };
        let id = store.read().await.list_all()[0].id.clone();
        let result = tool.call(serde_json::json!({"id": id}), ctx).await;
        assert!(result.success);
        assert!(result.content.contains("not running"));
    }
}
