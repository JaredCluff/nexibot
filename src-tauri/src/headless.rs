//! Headless entry point for NexiBot server deployment (Podman, Docker, etc.).
//!
//! Runs the same services as the Tauri desktop app but without a GUI window:
//! - Claude API client
//! - Gateway WebSocket server (for frontend or mobile connections)
//! - HTTP API server (/api/chat/send, /api/config, /api/skills, ...)
//! - Telegram, Discord, Signal, Matrix, Slack, WhatsApp, Teams bots
//! - Webhook server (WhatsApp / Slack inbound)
//! - MCP servers
//! - Skills system with execution engine
//! - Memory (SQLite FTS5 + vector hybrid)
//! - Scheduler (cron tasks)
//! - Config hot-reload via file watcher
//! - K2K integration
//! - Defense pipeline (DeBERTa + Llama Guard)
//!
//! # Activation
//! Set `NEXIBOT_HEADLESS=1` before launching the binary, or pass `--headless` on
//! the command line.  The binary detects this before Tauri is started and routes
//! to this module instead:
//!
//! ```bash
//! NEXIBOT_HEADLESS=1 ./nexibot-tauri
//! NEXIBOT_HEADLESS=1 podman run ghcr.io/jaredcluff/nexibot:latest
//! ```
//!
//! # Architecture
//!
//! ```
//! ┌───────────────────────────────────────────────┐
//! │  headless::run()                              │
//! │  - Config loaded from file / env vars         │
//! │  - All core services started (tokio::spawn)   │
//! │  - Gateway WebSocket on $NEXIBOT_GATEWAY_PORT │
//! │  - HTTP API on $NEXIBOT_API_PORT (default 18791)│
//! │  - Graceful shutdown on SIGTERM / SIGINT      │
//! └───────────────────────────────────────────────┘
//!        │
//!        ├── config.rs          (config + hot-reload)
//!        ├── gateway/           (WebSocket gateway, auth)
//!        ├── api_server.rs      (REST API for mobile/scripts)
//!        ├── claude.rs          (LLM client)
//!        ├── telegram.rs / discord.rs / slack.rs / ...
//!        ├── webhooks.rs        (inbound HTTP webhooks)
//!        ├── mcp.rs             (MCP tool servers)
//!        ├── skills.rs          (skills + execution engine)
//!        ├── memory.rs          (memory store)
//!        ├── scheduler.rs       (cron tasks)
//!        └── defense/           (prompt injection + safety)
//! ```

use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn};

use crate::agent;
use crate::agent_control;
use crate::agent_team;
use crate::bridge::BridgeManager;
use crate::browser::BrowserManager;
use crate::circuit_breaker::{CircuitBreakerConfig, CircuitBreakerRegistry};
use crate::claude::ClaudeClient;
use crate::commands::{
    AgentRunRegistry, AppState, ChannelServices, CoreServices, IntegrationServices, LlmServices,
    SafetyServices,
};
use crate::computer_use::ComputerUseManager;
use crate::config::{self, NexiBotConfig};
use crate::context_manager;
use crate::dag;
use crate::dashboard;
use crate::db_maintenance;
use crate::defense::DefensePipeline;
use crate::family_mode;
use crate::guardrails::Guardrails;
use crate::heartbeat::{HeartbeatConfig, HeartbeatManager};
use crate::k2k_client::K2KIntegration;
use crate::key_rotation;
use crate::mcp::MCPManager;
use crate::memory::MemoryManager;
use crate::memory_advanced;
use crate::oauth_manager;
use crate::observability;
use crate::orchestration;
use crate::pairing;
use crate::providers::ModelRegistry;
use crate::scheduler::Scheduler;
use crate::security;
use crate::session_overrides::SessionOverrides;
use crate::sessions;
use crate::shared_workspace::SharedWorkspace;
use crate::skills::SkillsManager;
use crate::subagent_executor::{SubagentExecutor, SubagentExecutorConfig};
use crate::subscription::SubscriptionManager;
use crate::task_manager;
use crate::user_identity;
use crate::voice::VoiceService;

/// Check whether headless mode should be activated.
///
/// Returns `true` when:
/// - The environment variable `NEXIBOT_HEADLESS` is set to any non-empty value, or
/// - The `--headless` flag appears anywhere in `std::env::args()`.
pub fn is_headless() -> bool {
    if std::env::var("NEXIBOT_HEADLESS")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    std::env::args().any(|a| a == "--headless")
}

/// Run NexiBot in headless (server) mode.
///
/// This function blocks until the process receives SIGTERM or SIGINT.
pub async fn run() {
    info!("=== NexiBot Headless Server starting ===");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));

    // -----------------------------------------------------------------
    // Logging
    // -----------------------------------------------------------------
    let logging_config = NexiBotConfig::load().map(|c| c.logging).unwrap_or_default();
    let _log_state = crate::logging::init(&logging_config);

    // -----------------------------------------------------------------
    // Configuration
    // -----------------------------------------------------------------
    let config = match NexiBotConfig::load() {
        Ok(cfg) => {
            info!("[CONFIG] Configuration loaded");
            Arc::new(RwLock::new(cfg))
        }
        Err(e) => {
            error!(
                "[CONFIG] Failed to load configuration: {}. Using defaults.",
                e
            );
            Arc::new(RwLock::new(NexiBotConfig::default()))
        }
    };

    let config_changed_tx = config::start_config_watcher(config.clone()).unwrap_or_else(|e| {
        warn!("[HOT_RELOAD] Failed to start config watcher: {}", e);
        broadcast::channel::<()>(16).0
    });

    // -----------------------------------------------------------------
    // Core services
    // -----------------------------------------------------------------
    let claude_client = Arc::new(RwLock::new(ClaudeClient::new(config.clone())));

    let k2k_client = Arc::new(RwLock::new(K2KIntegration::new(config.clone())));
    {
        let k2k = k2k_client.clone();
        tokio::spawn(async move {
            if let Err(e) = k2k.read().await.initialize().await {
                warn!("[K2K] Failed to initialize K2K client: {}", e);
            }
        });
    }

    let (router_url, device_id) = {
        let cfg = config.read().await;
        (
            cfg.k2k
                .router_url
                .clone()
                .unwrap_or_default(),
            cfg.k2k.client_id.clone(),
        )
    };
    let subscription_manager =
        Arc::new(RwLock::new(SubscriptionManager::new(router_url, device_id)));

    let voice_service = Arc::new(RwLock::new(VoiceService::new(
        config.clone(),
        claude_client.clone(),
    )));

    let guardrails_config = { config.read().await.guardrails.clone() };
    let guardrails = Arc::new(RwLock::new(Guardrails::new(guardrails_config.clone())));
    info!(
        "[GUARDRAILS] Security system initialized with {:?} protection",
        guardrails_config.security_level
    );

    // Skills
    let skills_manager = {
        match SkillsManager::new() {
            Ok(mut manager) => {
                if let Ok(n) = manager.extract_bundled_skills() {
                    if n > 0 {
                        info!("[SKILLS] Extracted {} bundled skills", n);
                    }
                }
                info!("[SKILLS] Skills system initialized");
                Arc::new(RwLock::new(manager))
            }
            Err(e) => {
                error!("[SKILLS] Failed to initialize skills manager: {}", e);
                // Retry once before giving up cleanly
                match SkillsManager::new() {
                    Ok(m) => Arc::new(RwLock::new(m)),
                    Err(e2) => {
                        error!("[SKILLS] Skills system init failed twice: {}. Exiting.", e2);
                        std::process::exit(1);
                    }
                }
            }
        }
    };
    if let Err(e) = crate::skills::start_skills_watcher(skills_manager.clone()) {
        warn!("[SKILLS] Failed to start skills file watcher: {}", e);
    }

    // Memory
    let memory_manager = {
        match MemoryManager::new() {
            Ok(mgr) => {
                info!("[MEMORY] Memory system initialized");
                Arc::new(RwLock::new(mgr))
            }
            Err(e) => {
                error!("[MEMORY] Failed to initialize: {}", e);
                match MemoryManager::new() {
                    Ok(m) => Arc::new(RwLock::new(m)),
                    Err(e2) => {
                        error!("[MEMORY] Memory system init failed twice: {}. Exiting.", e2);
                        std::process::exit(1);
                    }
                }
            }
        }
    };

    let heartbeat_manager = Arc::new(HeartbeatManager::new(HeartbeatConfig::default()));
    let bridge_manager = Arc::new(RwLock::new(BridgeManager::new()));

    let computer_use_config = config.read().await.computer_use.clone();
    let computer_use = Arc::new(RwLock::new(ComputerUseManager::new(
        computer_use_config.enabled,
        crate::computer_use::DisplayConfig {
            width: computer_use_config.display_width,
            height: computer_use_config.display_height,
        },
        computer_use_config.require_confirmation,
    )));

    let browser_config = config.read().await.browser.clone();
    let browser_manager = Arc::new(RwLock::new(BrowserManager::new(browser_config.clone())));

    // Defense pipeline
    let defense_config = config.read().await.defense.clone();
    let guardrails_security_level = config.read().await.guardrails.security_level;
    let defense_pipeline = Arc::new(RwLock::new(DefensePipeline::new(
        defense_config,
        guardrails_security_level,
    )));
    {
        let dp = defense_pipeline.clone();
        tokio::spawn(async move {
            info!("[DEFENSE] Initializing defense pipeline...");
            if let Err(e) = dp.write().await.initialize().await {
                warn!("[DEFENSE] Failed to initialize: {}", e);
            }
        });
    }

    // MCP
    let mcp_manager = Arc::new(RwLock::new(MCPManager::new(config.clone())));
    {
        let mcp = mcp_manager.clone();
        tokio::spawn(async move {
            if let Err(e) = mcp.write().await.initialize().await {
                warn!("[MCP] Failed to initialize MCP servers: {}", e);
            }
        });
    }

    // Bridge (Anthropic Node.js bridge)
    {
        let bridge = bridge_manager.clone();
        tokio::spawn(async move {
            info!("[BRIDGE] Starting bridge service...");
            match bridge.read().await.ensure_running().await {
                Ok(_) => info!("[BRIDGE] Bridge service is ready"),
                Err(e) => warn!("[BRIDGE] Failed to start bridge: {}", e),
            }
        });
    }

    let session_overrides = Arc::new(RwLock::new(SessionOverrides::default()));
    let notification_dispatcher = Arc::new(crate::notifications::NotificationDispatcher::new(
        config.clone(),
        None,
    ));
    let scheduler = Arc::new(Scheduler::new(
        config.clone(),
        claude_client.clone(),
        notification_dispatcher.clone(),
    ));
    let session_manager = {
        let mut mgr = sessions::SessionManager::new();
        let enc_cfg = config.read().await.session_encryption.clone();
        let encryptor = crate::build_session_encryptor(&enc_cfg);
        mgr.set_encryptor(encryptor);
        Arc::new(RwLock::new(mgr))
    };

    let pairing_manager = Arc::new(RwLock::new(
        pairing::PairingManager::new().unwrap_or_else(|e| {
            error!("[PAIRING] Failed to initialize pairing manager: {}. Exiting.", e);
            std::process::exit(1);
        }),
    ));

    let agent_manager = {
        let cfg = config.read().await;
        Arc::new(RwLock::new(agent::AgentManager::new(&cfg, config.clone())))
    };
    info!("[AGENT] Agent manager initialized");

    let model_registry = Arc::new(RwLock::new(ModelRegistry::new(config.clone())));
    {
        let reg = model_registry.clone();
        tokio::spawn(async move {
            reg.write().await.initialize().await;
        });
    }

    let user_manager = Arc::new(RwLock::new(
        user_identity::UserIdentityManager::new().unwrap_or_else(|e| {
            error!("[USER-IDENTITY] Failed to initialize: {}. Exiting.", e);
            std::process::exit(1);
        }),
    ));

    let orchestrator = Arc::new(agent_team::AgentOrchestrator::new());

    let orchestration_manager = Arc::new(RwLock::new(orchestration::OrchestrationManager::new(
        orchestration::OrchestrationConfig::default(),
    )));
    let subagent_executor = Arc::new(RwLock::new(SubagentExecutor::new(
        SubagentExecutorConfig::default(),
    )));
    let shared_workspace = Arc::new(RwLock::new(SharedWorkspace::new()));
    let circuit_breaker = Arc::new(RwLock::new(CircuitBreakerRegistry::new(
        CircuitBreakerConfig::default(),
    )));

    let dag_db_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("nexibot/dag/dags.db");
    let dag_store = Arc::new(std::sync::Mutex::new(
        dag::store::DagStore::new(dag_db_path).unwrap_or_else(|e| {
            error!("[DAG] Failed to initialize DAG store: {}. Exiting.", e);
            std::process::exit(1);
        }),
    ));
    let dag_executor = Arc::new(dag::executor::DagExecutor::new(
        dag_store.clone(),
        subagent_executor.clone(),
    ));

    let model_cache = Arc::new(RwLock::new(None));
    let ptt_capture = Arc::new(RwLock::new(None));
    let oauth_state = Arc::new(RwLock::new(None));
    let task_manager = Arc::new(RwLock::new(task_manager::TaskManager::new()));
    let tool_policy_manager =
        Arc::new(RwLock::new(security::tool_policy::ToolPolicyManager::new()));
    let exec_approval_manager = Arc::new(RwLock::new(
        security::exec_approval::ExecApprovalManager::new(
            security::exec_approval::ApprovalMode::default(),
        ),
    ));

    let agent_control = Arc::new(agent_control::AgentControl::new());

    let context_manager = {
        let cfg = config.read().await;
        let ctx_config = context_manager::ContextManagerConfig {
            enabled: cfg.claude.auto_compact_enabled,
            compaction_threshold: cfg.claude.auto_compact_threshold,
            approaching_threshold: 0.70,
            preserve_recent_messages: 20,
            archive_summaries: true,
            max_compactions_per_day: 5,
        };
        Arc::new(context_manager::ContextManager::new(ctx_config))
    };

    let oauth_manager = Arc::new(RwLock::new(
        oauth_manager::OAuthManager::new().unwrap_or_else(|e| {
            tracing::error!("[OAUTH] Could not initialize OAuth system: {}", e);
            std::process::exit(1);
        }),
    ));

    let cost_tracker = Arc::new(observability::CostTracker::new());
    let audit_log = Arc::new(observability::AuditLog::new(10_000));
    let key_rotation_manager = Arc::new(key_rotation::KeyRotationManager::new());
    let family_mode_manager = Arc::new(family_mode::FamilyModeManager::new());

    let backup_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("nexibot/backups");
    let db_maintenance_manager = Arc::new(
        db_maintenance::DbMaintenanceManager::new(
            backup_dir,
            db_maintenance::MaintenanceConfig::default(),
        )
        .unwrap_or_else(|e| {
            error!("[DB_MAINTENANCE] Failed to initialize: {}. Exiting.", e);
            std::process::exit(1);
        }),
    );

    let dashboard_manager = Arc::new(dashboard::DashboardManager::new());
    let advanced_memory_manager = Arc::new(memory_advanced::AdvancedMemoryManager::new());

    // -----------------------------------------------------------------
    // Build AppState (identical struct used by all commands)
    // -----------------------------------------------------------------
    let core = CoreServices {
        config: config.clone(),
        config_changed: config_changed_tx.clone(),
        user_manager: user_manager.clone(),
        orchestrator: orchestrator.clone(),
    };
    let llm = LlmServices {
        claude_client: claude_client.clone(),
        model_registry: model_registry.clone(),
        agent_manager: agent_manager.clone(),
        session_overrides: session_overrides.clone(),
        session_manager: session_manager.clone(),
        model_cache: model_cache.clone(),
    };
    let safety = SafetyServices {
        guardrails: guardrails.clone(),
        defense_pipeline: defense_pipeline.clone(),
    };
    let integrations_svc = IntegrationServices {
        k2k_client: k2k_client.clone(),
        mcp_manager: mcp_manager.clone(),
        bridge_manager: bridge_manager.clone(),
    };
    let channels_svc = ChannelServices {
        voice_service: voice_service.clone(),
        ptt_capture: ptt_capture.clone(),
    };

    let app_state = AppState {
        core,
        llm,
        safety,
        integrations: integrations_svc,
        channels: channels_svc,
        config: config.clone(),
        config_changed: config_changed_tx.clone(),
        user_manager,
        orchestrator,
        claude_client,
        model_registry,
        agent_manager,
        session_overrides,
        session_manager,
        model_cache,
        guardrails,
        defense_pipeline,
        k2k_client,
        mcp_manager,
        bridge_manager,
        voice_service,
        ptt_capture,
        memory_manager,
        skills_manager,
        subscription_manager,
        heartbeat_manager,
        computer_use,
        browser: browser_manager,
        oauth_state,
        openai_device_flow: Arc::new(RwLock::new(None)),
        scheduler: scheduler.clone(),
        pairing_manager,
        task_manager,
        tool_policy_manager,
        exec_approval_manager,
        orchestration_manager,
        subagent_executor,
        shared_workspace,
        circuit_breaker,
        dag_store,
        dag_executor,
        agent_run_registry: Arc::new(RwLock::new(AgentRunRegistry::new())),
        agent_control,
        context_manager,
        oauth_manager,
        cost_tracker,
        audit_log,
        key_rotation_manager,
        family_mode_manager,
        db_maintenance_manager,
        dashboard_manager,
        advanced_memory_manager,
        key_interceptor: {
            use security::key_interceptor::KeyInterceptor;
            use security::key_vault::KeyVault;
            use sha2::{Digest, Sha256};
            let home = dirs::home_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown-home".to_string());
            let username = std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "unknown-user".to_string());
            let derivation_input = format!(
                "nexibot-vault-v1:{}:{}:ai.nexibot.desktop",
                home, username
            );
            let passphrase = format!("{:x}", Sha256::digest(derivation_input.as_bytes()));
            let vault_enabled = config
                .try_read()
                .map(|c| c.key_vault.enabled)
                .unwrap_or(true);
            let vault_db_path = dirs::config_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("nexibot/vault/key_vault.sqlite");
            let vault = match KeyVault::new(vault_db_path, &passphrase, vault_enabled) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "[KEY_VAULT] Failed to initialize vault: {}. Keys will not be intercepted.",
                        e
                    );
                    KeyVault::new(std::path::PathBuf::from(":memory:"), &passphrase, false)
                        .expect("In-memory vault must always succeed")
                }
            };
            KeyInterceptor::new(Arc::new(vault))
        },
        // Headless has no GUI, so app_handle is None (GUI notifications are silently dropped).
        notification_dispatcher,
        // Gated shell requires a Tauri AppHandle for event emission; unavailable in headless mode.
        gated_shell: None,
        // Yolo mode runs in headless mode too (no GUI, but state is tracked).
        yolo_manager: crate::yolo_mode::YoloModeManager::new(
            config
                .try_read()
                .map(|c| c.yolo_mode.clone())
                .unwrap_or_default(),
        ),
        // KN Central Management — runs in headless mode too.
        managed_policy_manager: std::sync::Arc::new(
            crate::managed_policy::ManagedPolicyManager::new(config.clone())
        ),
        telegram_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        telegram_last_error: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        telegram_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        discord_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        slack_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        signal_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        whatsapp_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        teams_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        teams_conversation_service_urls: std::sync::Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        )),
        matrix_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        bluebubbles_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        mattermost_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        google_chat_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        twilio_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        messenger_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        instagram_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        line_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        mastodon_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        mastodon_account_handles: std::sync::Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        )),
        rocketchat_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        rocketchat_rest_auth: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        webchat_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        webchat_session_senders: std::sync::Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        )),
        gui_pending_approvals: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        // Native plugin registry
        plugin_registry: std::sync::Arc::new(RwLock::new(crate::plugins::PluginRegistry::new())),
        // Per-agent workspace isolation
        workspace_manager: std::sync::Arc::new(std::sync::Mutex::new(crate::agent_workspace::WorkspaceManager::new())),
        log_state: None,
    };

    // Start yolo mode expiry watcher
    app_state.yolo_manager.start_expiry_watcher();

    // Start KN Central Management (register + heartbeat loop).
    {
        let mp_mgr = app_state.managed_policy_manager.clone();
        tokio::spawn(async move {
            mp_mgr.start().await;
        });
    }

    // Inject services into heartbeat manager for catch-up notification scan.
    {
        let hb = app_state.heartbeat_manager.clone();
        let dag_store = app_state.dag_store.clone();
        let dispatcher = app_state.notification_dispatcher.clone();
        tokio::spawn(async move {
            hb.set_services(dag_store, dispatcher).await;
        });
    }

    // -----------------------------------------------------------------
    // Start channel bots
    // -----------------------------------------------------------------

    // Telegram: capture the stop handle so the hot-reload subscriber can
    // restart the bot when bot_token or enabled changes in config.
    let telegram_shutdown: Arc<tokio::sync::Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    {
        let s = app_state.clone();
        let telegram_shutdown_init = telegram_shutdown.clone();
        tokio::spawn(async move {
            match crate::telegram::start_telegram_bot(s.clone()).await {
                Ok(Some(token)) => {
                    *s.telegram_last_error.lock().await = None;
                    s.telegram_running.store(true, std::sync::atomic::Ordering::SeqCst);
                    *telegram_shutdown_init.lock().await = Some(token);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("[TELEGRAM] Failed to start Telegram bot: {}", e);
                    *s.telegram_last_error.lock().await = Some(e);
                }
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::discord::start_discord_bot(s).await {
                warn!("[DISCORD] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::signal::start_signal_listener(s).await {
                warn!("[SIGNAL] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::matrix::start_matrix_sync(s).await {
                warn!("[MATRIX] {}", e);
            }
        });
    }

    // Start BlueBubbles (iMessage) listener
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::bluebubbles::start_bluebubbles_listener(s).await {
                warn!("[BLUEBUBBLES] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::mattermost::start_mattermost_bot(s).await {
                warn!("[MATTERMOST] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::mastodon::start_mastodon_bot(s).await {
                warn!("[MASTODON] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::rocketchat::start_rocketchat_bot(s).await {
                warn!("[ROCKETCHAT] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::webchat::start_webchat_server(s).await {
                warn!("[WEBCHAT] {}", e);
            }
        });
    }

    // -----------------------------------------------------------------
    // Teams / Messenger / Instagram bots
    // -----------------------------------------------------------------
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::teams::start_teams_webhook(s).await {
                warn!("[TEAMS] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::messenger::start_messenger(s).await {
                warn!("[MESSENGER] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::instagram::start_instagram(s).await {
                warn!("[INSTAGRAM] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::google_chat::start_google_chat(s).await {
                warn!("[GOOGLE_CHAT] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::line::start_line(s).await {
                warn!("[LINE] {}", e);
            }
        });
    }
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::twilio::start_twilio(s).await {
                warn!("[TWILIO] {}", e);
            }
        });
    }

    // Start Email (IMAP/SMTP) polling
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::email::start_email_polling(s).await {
                warn!("[EMAIL] {}", e);
            }
        });
    }

    // Start Gmail polling
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::gmail::start_gmail_polling(s).await {
                warn!("[GMAIL] {}", e);
            }
        });
    }

    // -----------------------------------------------------------------
    // Webhook server (also hosts WhatsApp / Slack inbound)
    // -----------------------------------------------------------------
    {
        let s = app_state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::webhooks::start_webhook_server(
                s.config.clone(),
                s.scheduler.clone(),
                s.claude_client.clone(),
                Some(s),
            )
            .await
            {
                warn!("[WEBHOOK] {}", e);
            }
        });
    }

    // -----------------------------------------------------------------
    // Gateway WebSocket server
    // -----------------------------------------------------------------
    {
        let gw_config = config.read().await.gateway.clone();
        if gw_config.enabled {
            let port = gw_config.port;
            let server = Arc::new(crate::gateway::ws_server::GatewayServer::new(gw_config));
            tokio::spawn(async move {
                info!("[GATEWAY] Starting WebSocket gateway on port {}", port);
                if let Err(e) = server.start().await {
                    warn!("[GATEWAY] Gateway server error: {}", e);
                }
            });
        } else {
            info!("[GATEWAY] WebSocket gateway is disabled (set gateway.enabled=true to enable)");
        }
    }

    // -----------------------------------------------------------------
    // Hot-reload subscriptions
    // -----------------------------------------------------------------
    {
        let s = app_state.clone();
        let mut rx = app_state.config_changed.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.recv().await.is_err() {
                    break;
                }
                let mut mcp = s.mcp_manager.write().await;
                if let Err(e) = mcp.initialize().await {
                    warn!("[HOT_RELOAD] MCP reinitialization failed: {}", e);
                }
            }
        });
    }

    // --- BrowserManager: reload domain allowlist on config change ---
    {
        let s = app_state.clone();
        let mut rx = app_state.config_changed.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.recv().await.is_err() { break; }
                let new_cfg = s.config.read().await.browser.clone();
                s.browser.write().await.update_config(new_cfg);
                info!("[HOT_RELOAD] Browser config updated");
            }
        });
    }

    {
        let s = app_state.clone();
        let mut rx = app_state.config_changed.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.recv().await.is_err() {
                    break;
                }
                let new_config = s.config.read().await.defense.clone();
                s.defense_pipeline.write().await.update_config(new_config);
            }
        });
    }
    {
        let s = app_state.clone();
        let mut rx = app_state.config_changed.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.recv().await.is_err() {
                    break;
                }
                s.voice_service.read().await.reinit_backends().await;
            }
        });
    }

    // ClaudeClient: update max_history_messages on config change
    {
        let reload_state = app_state.clone();
        let mut rx = config_changed_tx.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.recv().await.is_err() {
                    break;
                }
                let max = reload_state.config.read().await.claude.max_history_messages;
                reload_state
                    .claude_client
                    .read()
                    .await
                    .set_max_history_messages(max)
                    .await;
            }
        });
    }

    // --- YoloMode: update config on change ---
    {
        let s = app_state.clone();
        let mut rx = app_state.config_changed.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.recv().await.is_err() { break; }
                let new_cfg = s.config.read().await.yolo_mode.clone();
                s.yolo_manager.update_config(new_cfg).await;
            }
        });
    }

    // --- ModelRegistry: reload API keys and model defaults on config change ---
    {
        let s = app_state.clone();
        let mut rx = app_state.config_changed.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.recv().await.is_err() { break; }
                info!("[HOT_RELOAD] Config changed — reloading model registry");
                s.model_registry.write().await.reload().await;
            }
        });
    }

    // --- Guardrails: update security level and config on change ---
    {
        let s = app_state.clone();
        let mut rx = app_state.config_changed.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.recv().await.is_err() { break; }
                let new_cfg = s.config.read().await.guardrails.clone();
                let mut g = s.guardrails.write().await;
                if let Err(e) = g.update_config(new_cfg) {
                    warn!("[HOT_RELOAD] Guardrails config update failed: {}", e);
                } else {
                    info!("[HOT_RELOAD] Guardrails config updated");
                }
            }
        });
    }

    // --- GatedShell: update enabled/debug/record flags on change ---
    {
        let s = app_state.clone();
        let mut rx = app_state.config_changed.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.recv().await.is_err() { break; }
                if let Some(gs) = &s.gated_shell {
                    let new_cfg = s.config.read().await.gated_shell.clone();
                    gs.update_config(new_cfg).await;
                }
            }
        });
    }

    // --- Key Vault → GatedShell: re-sync filter when vault entries change ---
    {
        let s = app_state.clone();
        let mut rx = app_state.config_changed.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.recv().await.is_err() { break; }
                if let Some(gs) = &s.gated_shell {
                    let mappings = s.key_interceptor.vault().all_proxy_to_real();
                    gs.sync_vault(&mappings);
                    info!("[HOT_RELOAD] Key vault synced to GatedShell filter ({} secrets)", mappings.len());
                }
            }
        });
    }

    // --- Telegram: restart bot when token or enabled flag changes ---
    {
        let telegram_restart_state = app_state.clone();
        let telegram_shutdown_sub = telegram_shutdown.clone();
        let mut telegram_rx = app_state.config_changed.subscribe();
        tokio::spawn(async move {
            let init_cfg = telegram_restart_state.config.read().await;
            let mut last_token = init_cfg.telegram.bot_token.clone();
            let mut last_enabled = init_cfg.telegram.enabled;
            drop(init_cfg);
            loop {
                if telegram_rx.recv().await.is_err() { break; }
                let (new_token, new_enabled) = {
                    let cfg = telegram_restart_state.config.read().await;
                    (cfg.telegram.bot_token.clone(), cfg.telegram.enabled)
                };
                if new_token == last_token && new_enabled == last_enabled {
                    continue;
                }
                info!("[HOT_RELOAD] Telegram config changed — restarting bot");
                let mut handle = telegram_shutdown_sub.lock().await;
                if let Some(old) = handle.take() {
                    old.store(true, Ordering::SeqCst);
                }
                telegram_restart_state.telegram_running.store(false, Ordering::SeqCst);
                if new_enabled && !new_token.is_empty() {
                    match crate::telegram::start_telegram_bot(telegram_restart_state.clone()).await {
                        Ok(Some(t)) => {
                            *telegram_restart_state.telegram_last_error.lock().await = None;
                            telegram_restart_state.telegram_running.store(true, Ordering::SeqCst);
                            *handle = Some(t);
                            info!("[HOT_RELOAD] Telegram bot restarted");
                        }
                        Ok(None) => {
                            info!("[HOT_RELOAD] Telegram bot not started (disabled or no token)");
                        }
                        Err(e) => {
                            warn!("[HOT_RELOAD] Telegram restart failed: {}", e);
                            *telegram_restart_state.telegram_last_error.lock().await = Some(e);
                        }
                    }
                } else {
                    *telegram_restart_state.telegram_last_error.lock().await = None;
                }
                last_token = new_token;
                last_enabled = new_enabled;
            }
        });
    }

    // -----------------------------------------------------------------
    // Scheduler
    // -----------------------------------------------------------------
    tokio::spawn(async move {
        // In headless mode, the scheduler tick loop doesn't need an app handle.
        scheduler.run_headless_loop().await;
    });

    // -----------------------------------------------------------------
    // Wait for shutdown signal (SIGTERM / SIGINT)
    // -----------------------------------------------------------------
    info!("=== NexiBot Headless Server ready ===");
    info!("Send SIGTERM or SIGINT to shut down gracefully.");

    wait_for_shutdown().await;
    info!("=== NexiBot Headless Server shutting down ===");
}

/// Wait for SIGTERM or SIGINT (Ctrl-C).
async fn wait_for_shutdown() {
    use tokio::signal;

    #[cfg(unix)]
    {
        use signal::unix::{signal as unix_signal, SignalKind};
        let mut sigterm = match unix_signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                error!("[SHUTDOWN] Failed to install SIGTERM handler: {}. Use SIGKILL to stop.", e);
                std::future::pending::<()>().await;
                return;
            }
        };
        let mut sigint = match unix_signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                error!("[SHUTDOWN] Failed to install SIGINT handler: {}. Use SIGKILL to stop.", e);
                std::future::pending::<()>().await;
                return;
            }
        };
        tokio::select! {
            _ = sigterm.recv() => { info!("Received SIGTERM"); }
            _ = sigint.recv()  => { info!("Received SIGINT");  }
        }
    }

    #[cfg(not(unix))]
    {
        if let Err(e) = signal::ctrl_c().await {
            error!("[SHUTDOWN] Failed to install Ctrl-C handler: {}. Use task manager to stop.", e);
            std::future::pending::<()>().await;
            return;
        }
        info!("Received Ctrl-C");
    }
}
