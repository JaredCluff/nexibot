//! Tauri commands exposed to frontend
//!
//! Lock ordering (always acquire in this order to prevent deadlocks):
//! 1. config
//! 2. guardrails
//! 3. defense_pipeline
//! 4. claude_client
//! 5. mcp_manager
//! 6. computer_use
//! 7. browser
//! 8. voice_service
//! 9. All others (oauth_state, skills_manager, memory_manager, etc.)

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, RwLock};
use tracing::warn;

use crate::bridge::BridgeManager;
use crate::browser::BrowserManager;
use crate::claude::ClaudeClient;
use crate::computer_use::ComputerUseManager;
use crate::config::NexiBotConfig;
use crate::defense::DefensePipeline;
use crate::gated_shell::GatedShell;
use crate::guardrails::Guardrails;
use crate::heartbeat::HeartbeatManager;
use crate::k2k_client::K2KIntegration;
use crate::mcp::MCPManager;
use crate::memory::MemoryManager;
use crate::providers::ModelRegistry;
use crate::scheduler::Scheduler;
use crate::session_overrides::SessionOverrides;
use crate::skills::SkillsManager;
use crate::subscription::SubscriptionManager;
use crate::voice::{PttCaptureHandle, VoiceService};

mod agent_cmds;
pub mod agent_control_cmds;
pub mod audit_cmds;
pub mod autonomous_cmds;
pub mod autonomous_agent_cmds;
mod bridge;
pub(crate) mod chat;
mod clawhub_cmds;
pub mod connector_cmds;
pub mod cli_audit;
mod computer_use_cmds;
mod config_cmds;
mod credential_cmds;
mod dag_cmds;
mod dashboard_cmds;
mod db_maintenance_cmds;
mod defense;
pub mod execute_tool;
mod family_mode_cmds;
pub mod fetch_tool;
pub mod filesystem_tool;
pub mod gated_shell_cmds;
mod guardrails_cmds;
mod heartbeat;
pub mod interactive_tool;
mod k2k_cmds;
mod key_rotation_cmds;
mod key_vault_cmds;
mod log_cmds;
pub mod managed_policy_cmds;
mod mcp_cmds;
pub(crate) mod memory;
mod memory_advanced_cmds;
pub mod memory_tool;
mod oauth;
pub mod observability;
mod pairing_cmds;
mod scheduler_cmds;
pub mod search_tool;
mod session_cmds;
pub mod session_mgmt;
pub mod settings_tool;
mod skills;
mod soul;
pub mod soul_tool;
mod startup_cmds;
mod subscription;
mod task_cmds;
mod telegram_cmds;
mod updater_cmds;
mod voice;
mod webhook_cmds;
mod whatsapp_cmds;
pub mod yolo_cmds;

// Re-export all commands and types for registration in main.rs
pub use agent_cmds::*;
pub use audit_cmds::*;
pub use autonomous_cmds::*;
pub use autonomous_agent_cmds::*;
pub use bridge::*;
pub use chat::*;
pub use clawhub_cmds::*;
pub use connector_cmds::*;
pub use computer_use_cmds::*;
pub use config_cmds::*;
pub use credential_cmds::*;
pub use dag_cmds::*;
pub use dashboard_cmds::*;
pub use db_maintenance_cmds::*;
pub use defense::*;
pub use family_mode_cmds::*;
pub use gated_shell_cmds::*;
pub use guardrails_cmds::*;
pub use heartbeat::*;
pub use k2k_cmds::*;
pub use key_rotation_cmds::*;
pub use key_vault_cmds::*;
pub use log_cmds::*;
pub use managed_policy_cmds::*;
pub use mcp_cmds::*;
pub use memory::*;
pub use memory_advanced_cmds::*;
pub use oauth::*;
pub use pairing_cmds::*;
pub use scheduler_cmds::*;
pub use session_cmds::*;
pub use session_mgmt::*;
pub use skills::*;
pub use soul::*;
pub use startup_cmds::*;
pub use subscription::*;
pub use task_cmds::*;
pub use telegram_cmds::*;
pub use updater_cmds::*;
pub use voice::*;
pub use webhook_cmds::*;
pub use whatsapp_cmds::*;
pub use yolo_cmds::*;

/// Helper to acquire a read lock with a timeout
pub(crate) async fn timed_read<'a, T>(
    lock: &'a RwLock<T>,
    name: &'a str,
) -> Result<tokio::sync::RwLockReadGuard<'a, T>, String> {
    match tokio::time::timeout(std::time::Duration::from_secs(5), lock.read()).await {
        Ok(guard) => Ok(guard),
        Err(_) => {
            warn!(
                "[LOCK] Timed out acquiring read lock on '{}' after 5s",
                name
            );
            Err(format!("Lock acquisition timeout on '{}'", name))
        }
    }
}

/// Helper to acquire a write lock with a timeout
#[allow(dead_code)]
pub(crate) async fn timed_write<'a, T>(
    lock: &'a RwLock<T>,
    name: &'a str,
) -> Result<tokio::sync::RwLockWriteGuard<'a, T>, String> {
    match tokio::time::timeout(std::time::Duration::from_secs(5), lock.write()).await {
        Ok(guard) => Ok(guard),
        Err(_) => {
            warn!(
                "[LOCK] Timed out acquiring write lock on '{}' after 5s",
                name
            );
            Err(format!("Lock acquisition timeout on '{}'", name))
        }
    }
}

/// Pending OAuth flow state (replaces env vars for PKCE storage)
pub struct OAuthPendingState {
    pub code_verifier: String,
    pub state: String,
    pub created_at: Instant,
}

/// Pending OpenAI device code flow state (Codex-style device auth)
#[allow(dead_code)]
pub struct OpenAIDeviceFlowState {
    pub device_auth_id: String,
    pub user_code: String,
    #[allow(dead_code)]
    pub interval: u64,
    pub expires_at: Instant,
}

/// Core services: config, notifications, user identity, agent orchestration.
#[derive(Clone)]
#[allow(dead_code)]
pub struct CoreServices {
    #[allow(dead_code)]
    pub config: Arc<RwLock<NexiBotConfig>>,
    #[allow(dead_code)]
    pub config_changed: broadcast::Sender<()>,
    #[allow(dead_code)]
    pub user_manager: Arc<RwLock<crate::user_identity::UserIdentityManager>>,
    #[allow(dead_code)]
    pub orchestrator: Arc<crate::agent_team::AgentOrchestrator>,
}

/// LLM-related services: model registry, agents, sessions, overrides.
#[derive(Clone)]
#[allow(dead_code)]
pub struct LlmServices {
    #[allow(dead_code)]
    pub claude_client: Arc<RwLock<ClaudeClient>>,
    #[allow(dead_code)]
    pub model_registry: Arc<RwLock<ModelRegistry>>,
    #[allow(dead_code)]
    pub agent_manager: Arc<RwLock<crate::agent::AgentManager>>,
    #[allow(dead_code)]
    pub session_overrides: Arc<RwLock<SessionOverrides>>,
    #[allow(dead_code)]
    pub session_manager: Arc<RwLock<crate::sessions::SessionManager>>,
    #[allow(dead_code)]
    pub model_cache: Arc<RwLock<Option<(Vec<session_cmds::AvailableModel>, std::time::Instant)>>>,
}

/// Safety services: guardrails and defense pipeline.
#[derive(Clone)]
#[allow(dead_code)]
pub struct SafetyServices {
    #[allow(dead_code)]
    pub guardrails: Arc<RwLock<Guardrails>>,
    #[allow(dead_code)]
    pub defense_pipeline: Arc<RwLock<DefensePipeline>>,
}

/// External integration services: K2K, MCP, Bridge.
#[derive(Clone)]
#[allow(dead_code)]
pub struct IntegrationServices {
    #[allow(dead_code)]
    pub k2k_client: Arc<RwLock<K2KIntegration>>,
    #[allow(dead_code)]
    pub mcp_manager: Arc<RwLock<MCPManager>>,
    #[allow(dead_code)]
    pub bridge_manager: Arc<RwLock<BridgeManager>>,
}

/// Channel-specific services: voice, push-to-talk.
#[derive(Clone)]
#[allow(dead_code)]
pub struct ChannelServices {
    #[allow(dead_code)]
    pub voice_service: Arc<RwLock<VoiceService>>,
    #[allow(dead_code)]
    pub ptt_capture: Arc<RwLock<Option<PttCaptureHandle>>>,
}

/// Application state shared across commands.
///
/// Organized into service groups for semantic clarity. All fields are also
/// accessible as flat fields for backward compatibility -- the flat fields
/// are cheap Arc clones pointing to the same underlying data.
#[derive(Clone)]
#[allow(dead_code)]
pub struct AppState {
    // --- Service groups (for new code / organizational access) ---
    pub core: CoreServices,
    pub llm: LlmServices,
    pub safety: SafetyServices,
    pub integrations: IntegrationServices,
    pub channels: ChannelServices,

    // --- Flat aliases for backward compatibility ---
    // Core
    pub config: Arc<RwLock<NexiBotConfig>>,
    pub config_changed: broadcast::Sender<()>,
    pub user_manager: Arc<RwLock<crate::user_identity::UserIdentityManager>>,
    pub orchestrator: Arc<crate::agent_team::AgentOrchestrator>,
    // LLM
    pub claude_client: Arc<RwLock<ClaudeClient>>,
    pub model_registry: Arc<RwLock<ModelRegistry>>,
    pub agent_manager: Arc<RwLock<crate::agent::AgentManager>>,
    pub session_overrides: Arc<RwLock<SessionOverrides>>,
    pub session_manager: Arc<RwLock<crate::sessions::SessionManager>>,
    pub model_cache: Arc<RwLock<Option<(Vec<session_cmds::AvailableModel>, std::time::Instant)>>>,
    // Safety
    pub guardrails: Arc<RwLock<Guardrails>>,
    pub defense_pipeline: Arc<RwLock<DefensePipeline>>,
    // Integrations
    pub k2k_client: Arc<RwLock<K2KIntegration>>,
    pub mcp_manager: Arc<RwLock<MCPManager>>,
    pub bridge_manager: Arc<RwLock<BridgeManager>>,
    // Channels
    pub voice_service: Arc<RwLock<VoiceService>>,
    pub ptt_capture: Arc<RwLock<Option<PttCaptureHandle>>>,

    // --- Standalone services ---
    pub agent_control: Arc<crate::agent_control::AgentControl>,
    pub memory_manager: Arc<RwLock<MemoryManager>>,
    pub skills_manager: Arc<RwLock<SkillsManager>>,
    pub subscription_manager: Arc<RwLock<SubscriptionManager>>,
    pub heartbeat_manager: Arc<HeartbeatManager>,
    pub computer_use: Arc<RwLock<ComputerUseManager>>,
    pub browser: Arc<RwLock<BrowserManager>>,
    pub oauth_state: Arc<RwLock<Option<OAuthPendingState>>>,
    pub openai_device_flow: Arc<RwLock<Option<OpenAIDeviceFlowState>>>,
    pub scheduler: Arc<Scheduler>,
    pub context_manager: Arc<crate::context_manager::ContextManager>,
    pub oauth_manager: Arc<RwLock<crate::oauth_manager::OAuthManager>>,
    pub cost_tracker: Arc<crate::observability::CostTracker>,
    pub audit_log: Arc<crate::observability::AuditLog>,
    pub pairing_manager: Arc<RwLock<crate::pairing::PairingManager>>,
    pub key_rotation_manager: Arc<crate::key_rotation::KeyRotationManager>,
    pub family_mode_manager: Arc<crate::family_mode::FamilyModeManager>,
    pub db_maintenance_manager: Arc<crate::db_maintenance::DbMaintenanceManager>,
    pub dashboard_manager: Arc<crate::dashboard::DashboardManager>,
    pub advanced_memory_manager: Arc<crate::memory_advanced::AdvancedMemoryManager>,
    pub task_manager: Arc<RwLock<crate::task_manager::TaskManager>>,
    pub tool_policy_manager: Arc<RwLock<crate::security::tool_policy::ToolPolicyManager>>,
    pub exec_approval_manager: Arc<RwLock<crate::security::exec_approval::ExecApprovalManager>>,

    // Orchestration subsystems
    pub orchestration_manager: Arc<RwLock<crate::orchestration::OrchestrationManager>>,
    pub subagent_executor: Arc<RwLock<crate::subagent_executor::SubagentExecutor>>,
    pub shared_workspace: Arc<RwLock<crate::shared_workspace::SharedWorkspace>>,
    pub circuit_breaker: Arc<RwLock<crate::circuit_breaker::CircuitBreakerRegistry>>,

    // DAG subsystems
    pub dag_store: Arc<std::sync::Mutex<crate::dag::store::DagStore>>,
    pub dag_executor: Arc<crate::dag::executor::DagExecutor>,

    // Agent engine run registry (in-memory, populated by run_agent / get_agent_run_status)
    pub agent_run_registry: Arc<RwLock<AgentRunRegistry>>,

    // Smart Key Vault interceptor
    pub key_interceptor: crate::security::key_interceptor::KeyInterceptor,

    // NexiGate gated shell (None in headless mode)
    pub gated_shell: Option<Arc<GatedShell>>,

    // Yolo mode manager
    pub yolo_manager: Arc<crate::yolo_mode::YoloModeManager>,

    // Knowledge Nexus Central Management manager
    pub managed_policy_manager: Arc<crate::managed_policy::ManagedPolicyManager>,

    // Notification dispatcher
    pub notification_dispatcher: Arc<crate::notifications::NotificationDispatcher>,

    // Logging
    pub log_state: Option<crate::logging::LogState>,

    // Telegram bot running flag — set to true when the bot is polling, false when stopped.
    pub telegram_running: Arc<std::sync::atomic::AtomicBool>,
    // Last Telegram startup error — surfaced to the UI so users know why the bot isn't running.
    pub telegram_last_error: Arc<tokio::sync::Mutex<Option<String>>>,

    // Pending Telegram tool-approval requests:
    // (chat_id, requester_user_id) → oneshot sender.
    pub telegram_pending_approvals: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(i64, i64), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
    // Pending Discord tool-approval requests:
    // (channel_id, requester_user_id) → oneshot sender.
    pub discord_pending_approvals: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(u64, u64), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
    // Pending Slack tool-approval requests:
    // (channel_id, requester_user_id) → oneshot sender.
    pub slack_pending_approvals: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
    // Pending Signal tool-approval requests:
    // sender_phone_number → oneshot sender.
    pub signal_pending_approvals: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
    // Pending WhatsApp tool-approval requests:
    // sender_phone_number → oneshot sender.
    pub whatsapp_pending_approvals: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
    // Pending Microsoft Teams tool-approval requests:
    // (conversation_id, requester_user_id) → oneshot sender.
    pub teams_pending_approvals: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
    // Recently seen Teams service URLs:
    // conversation_id → service_url.
    pub teams_conversation_service_urls:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    // Pending Matrix tool-approval requests:
    // (room_id, requester_user_id) → oneshot sender.
    pub matrix_pending_approvals: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
    // Pending BlueBubbles tool-approval requests:
    // (chat_guid, requester_handle) → oneshot sender.
    pub bluebubbles_pending_approvals: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
    // Pending Mattermost tool-approval requests:
    // (channel_id, requester_user_id) → oneshot sender.
    pub mattermost_pending_approvals: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
    // Pending Google Chat tool-approval requests:
    // (space_id, requester_user_id) → oneshot sender.
    pub google_chat_pending_approvals: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
    // Pending Twilio tool-approval requests:
    // requester_phone_number → oneshot sender.
    pub twilio_pending_approvals: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
    // Pending Messenger tool-approval requests:
    // requester_sender_id → oneshot sender.
    pub messenger_pending_approvals: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
    // Pending Instagram tool-approval requests:
    // requester_sender_id → oneshot sender.
    pub instagram_pending_approvals: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
    // Pending LINE tool-approval requests:
    // (conversation_target_id, requester_user_id) → oneshot sender.
    pub line_pending_approvals: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
    // Pending Mastodon tool-approval requests:
    // requester_account_id → oneshot sender.
    pub mastodon_pending_approvals: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
    // Recently seen Mastodon account handles:
    // account_id → acct handle.
    pub mastodon_account_handles:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    // Pending Rocket.Chat tool-approval requests:
    // (room_id, requester_user_id) → oneshot sender.
    pub rocketchat_pending_approvals: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(String, String), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
    // Cached Rocket.Chat REST auth from DDP login:
    // (auth_token, bot_user_id).
    pub rocketchat_rest_auth: Arc<tokio::sync::RwLock<Option<(String, String)>>>,
    // Pending WebChat tool-approval requests:
    // requester_session_id → oneshot sender.
    pub webchat_pending_approvals: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
    // Active WebChat outbound channels:
    // requester_session_id → websocket outbound sender.
    pub webchat_session_senders: Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<String, tokio::sync::mpsc::UnboundedSender<String>>,
        >,
    >,
    // Pending GUI tool-approval requests: request_id → oneshot sender.
    pub gui_pending_approvals: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,

    // Native plugin registry
    pub plugin_registry: Arc<RwLock<crate::plugins::PluginRegistry>>,

    // Per-agent workspace isolation
    pub workspace_manager: Arc<std::sync::Mutex<crate::agent_workspace::WorkspaceManager>>,
}
