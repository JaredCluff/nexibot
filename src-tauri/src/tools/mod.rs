//! New trait-based tools for NexiBot v0.9.0.
//! Each submodule implements crate::tool_registry::Tool.

pub mod file_edit;
pub mod file_read;
pub mod file_read_state;
pub mod lsp;
pub mod media_gen;
pub mod notebook_edit;
pub mod send_message;
pub mod tasks;
pub mod plan_mode;
pub mod worktree;

/// Register all v0.9.0 tools into the registry.
/// Called once at startup from AppState initialization.
/// `plan_state` must be the SAME Arc stored in AppState::plan_mode_state so
/// the gate in chat.rs and the tools see the same allocation.
/// `lsp_config` is the user-configured LSP server map from config.yaml.
/// `config` is the shared NexiBotConfig Arc used by media generation tools.
pub fn register_all(
    registry: &mut crate::tool_registry::ToolRegistry,
    plan_state: std::sync::Arc<tokio::sync::RwLock<plan_mode::PlanModeState>>,
    lsp_config: crate::config::LspConfig,
    config: std::sync::Arc<tokio::sync::RwLock<crate::config::NexiBotConfig>>,
) {
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

    // Worktree tool
    let worktree_state = std::sync::Arc::new(tokio::sync::RwLock::new(
        worktree::WorktreeState::default()
    ));
    registry.register(Box::new(worktree::WorktreeTool { state: worktree_state }));

    // Plan mode tools — use the caller-supplied Arc so tools and AppState share
    // the same allocation (fixes the gate-never-fires bug).
    registry.register(Box::new(plan_mode::EnterPlanModeTool { state: plan_state.clone() }));
    registry.register(Box::new(plan_mode::ExitPlanModeTool { state: plan_state }));

    registry.register(Box::new(notebook_edit::NotebookEditTool));

    // LSP tool — initialized with the user's config.yaml lsp section.
    let lsp_manager = std::sync::Arc::new(tokio::sync::RwLock::new(
        lsp::server_manager::LspServerManager::from_config(
            &lsp_config,
            std::env::current_dir().unwrap_or_default(),
        )
    ));
    registry.register(Box::new(lsp::LspTool { manager: lsp_manager }));

    // Media generation tools (DALL-E 3, ElevenLabs, Runway)
    registry.register(Box::new(media_gen::GenerateImageTool::new(config.clone())));
    registry.register(Box::new(media_gen::GenerateAudioTool::new(config.clone())));
    registry.register(Box::new(media_gen::GenerateVideoTool::new(config)));
}
