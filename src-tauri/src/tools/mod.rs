//! New trait-based tools for NexiBot v0.9.0.
//! Each submodule implements crate::tool_registry::Tool.

pub mod file_edit;
pub mod file_read;
pub mod file_read_state;
pub mod send_message;
pub mod tasks;

/// Register all v0.9.0 tools into the registry.
/// Called once at startup from AppState initialization.
pub fn register_all(registry: &mut crate::tool_registry::ToolRegistry) {
    registry.register(Box::new(file_read::FileReadTool));
    registry.register(Box::new(file_edit::FileEditTool));

    // Background task tools — shared TaskStore
    let task_store = std::sync::Arc::new(tokio::sync::RwLock::new(
        tasks::TaskStore::new(
            dirs::config_dir()
                .unwrap_or_default()
                .join("nexibot")
                .join("tasks")
        )
    ));
    registry.register(Box::new(tasks::TaskCreateTool(task_store.clone())));
    registry.register(Box::new(tasks::TaskGetTool(task_store.clone())));
    registry.register(Box::new(tasks::TaskListTool(task_store.clone())));
    registry.register(Box::new(tasks::TaskOutputTool(task_store.clone())));
    registry.register(Box::new(tasks::TaskStopTool(task_store)));

    // Inter-agent messaging
    let task_inbox = std::sync::Arc::new(send_message::AgentInbox::new(
        dirs::config_dir().unwrap_or_default().join("nexibot").join("tasks")
    ));
    let agent_registry = std::sync::Arc::new(tokio::sync::RwLock::new(
        send_message::AgentNameRegistry::default()
    ));
    let in_process_queues = std::sync::Arc::new(tokio::sync::RwLock::new(
        std::collections::HashMap::new()
    ));
    registry.register(Box::new(send_message::SendMessageTool {
        registry: agent_registry,
        inbox: task_inbox,
        in_process_queues,
    }));
}
