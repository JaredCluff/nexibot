// Prevents additional console window on Windows in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Listener, Manager,
};
use tokio::sync::RwLock;
use tracing::{info, warn};

mod agent;
mod agent_control;
mod agent_engine;
mod agent_team;
mod agent_workspace;
mod api_server;
mod bluebubbles;
mod bridge;
mod browser;
mod canvas;
mod channel;
mod circuit_breaker;
mod claude;
mod clawhub;
mod commands;
mod computer_use;
mod config;
mod context_manager;
mod dag;
mod dashboard;
mod db_maintenance;
mod defense;
mod discord;
mod email;
mod embeddings;
mod gmail;
mod family_mode;
mod gated_shell;
mod gateway;
mod google_chat;
mod guardrails;
mod headless;
mod heartbeat;
mod hooks;
mod instagram;
mod k2k_client;
mod key_rotation;
mod line;
mod llm_provider;
mod logging;
mod mastodon;
mod matrix;
mod mattermost;
mod mcp;
mod memory;
mod nats;
mod memory_advanced;
mod memory_store;
mod messenger;
mod mobile;
mod native_control;
mod notifications;
mod oauth;
mod oauth_flow;
mod oauth_manager;
mod observability;
mod orchestration;
mod pairing;
mod platform;
mod plugins;
mod providers;
mod query_classifier;
mod rate_limiter;
mod rocketchat;
mod router;
mod sandbox;
mod scheduler;
mod security;
mod session_overrides;
mod sessions;
mod shared_workspace;
mod signal;
mod pii_redactor;
mod skill_format_adapters;
mod skill_lifecycle;
mod skill_security;
mod skills;
mod slack;
mod soul;
mod subagent_executor;
mod subscription;
mod task_manager;
mod teams;
mod telegram;
mod token_estimate;
mod tool_converter;
mod tool_loop;
mod tool_retry;
mod tool_search;
mod twilio;
mod user_identity;
mod voice;
mod webchat;
mod webhook_dedup;
mod webhook_rate_limit;
mod webhooks;
mod whatsapp;
mod yolo_mode;
mod tool_registry;
mod git_context;
mod cost_tracker;
mod tool_streaming;
mod tools;
#[cfg(feature = "connect")]
mod managed_policy;
#[cfg(not(feature = "connect"))]
#[path = "managed_policy_stub.rs"]
mod managed_policy;

#[cfg(test)]
mod test_utils;

use bridge::BridgeManager;
use browser::BrowserManager;
use claude::ClaudeClient;
use commands::AppState;
use computer_use::ComputerUseManager;
use config::NexiBotConfig;
use defense::DefensePipeline;
use guardrails::{Guardrails, GuardrailsConfig};
use heartbeat::{HeartbeatConfig, HeartbeatManager};
use k2k_client::K2KIntegration;
use mcp::MCPManager;
use memory::MemoryManager;
use scheduler::Scheduler;
use session_overrides::SessionOverrides;
use skills::SkillsManager;
use subscription::SubscriptionManager;
use voice::VoiceService;

use circuit_breaker::{CircuitBreakerConfig, CircuitBreakerRegistry};
use shared_workspace::SharedWorkspace;
use subagent_executor::{SubagentExecutor, SubagentExecutorConfig};

/// Build a `SessionEncryptor` from config: disabled → no-op; enabled → resolve passphrase
/// from keyring (custom key or auto-generated machine UUID stored under a default key).
pub(crate) fn build_session_encryptor(
    cfg: &security::session_encryption::SessionEncryptionConfig,
) -> security::session_encryption::SessionEncryptor {
    use security::session_encryption::SessionEncryptor;
    if !cfg.enabled {
        return SessionEncryptor::disabled();
    }

    const DEFAULT_KEY: &str = "nexibot.session.passphrase";
    let keyring_key = cfg
        .passphrase_keyring_key
        .as_deref()
        .unwrap_or(DEFAULT_KEY);

    let passphrase = match security::credentials::get_secret(keyring_key) {
        Ok(Some(p)) => p,
        Ok(None) => {
            // Generate and store a random passphrase on first run
            let new_passphrase = uuid::Uuid::new_v4().to_string();
            if let Err(e) = security::credentials::store_secret(keyring_key, &new_passphrase) {
                tracing::warn!("[SESSIONS] Could not store session passphrase in keyring: {}", e);
            }
            new_passphrase
        }
        Err(e) => {
            tracing::error!(
                "[SESSIONS] Could not retrieve session passphrase from keyring: {}. \
                 Session encryption is DISABLED — transcripts will be stored in plaintext. \
                 Fix keyring access to restore encryption.",
                e
            );
            return SessionEncryptor::disabled_degraded();
        }
    };

    SessionEncryptor::new(&passphrase, true)
}

fn main() {
    // Headless mode: NEXIBOT_HEADLESS=1 or --headless bypasses Tauri entirely.
    // Useful for running the backend in a container (Podman / Docker) without a display.
    if headless::is_headless() {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
        rt.block_on(headless::run());
        return;
    }

    // Initialize logging infrastructure (file output, ring buffer, dynamic level).
    // Load just the logging config from the config file; if it fails, use defaults.
    let logging_config = NexiBotConfig::load().map(|c| c.logging).unwrap_or_default();
    let log_state = logging::init(&logging_config);

    info!("Starting NexiBot...");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_deep_link::init())
        .setup(move |app| {
            // NexiBot is a tray/menubar app — hide from Dock and App Switcher.
            // LSUIElement in Info.plist is not sufficient; Tauri sets the activation
            // policy to Regular during initialization, overriding the plist value.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Initialize configuration
            let config = NexiBotConfig::load()
                .map_err(|e| format!("Failed to load config: {}", e))?;

            // Warn if credential storage falls back to plaintext config file.
            crate::security::credentials::warn_if_keyring_disabled();

            let config = Arc::new(RwLock::new(config));

            // Start config file watcher for hot reload
            let config_changed_tx = config::start_config_watcher(config.clone())
                .unwrap_or_else(|e| {
                    tracing::warn!("[HOT_RELOAD] Failed to start config watcher: {}", e);
                    tokio::sync::broadcast::channel::<()>(16).0
                });

            // Initialize Claude client
            let claude_client = Arc::new(RwLock::new(ClaudeClient::new(config.clone())));

            // Initialize K2K integration
            let k2k_client = Arc::new(RwLock::new(K2KIntegration::new(config.clone())));

            // Initialize K2K client asynchronously
            let k2k_client_clone = k2k_client.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = k2k_client_clone.read().await.initialize().await {
                    tracing::warn!("Failed to initialize K2K client: {}", e);
                }
            });

            // Initialize subscription manager
            let (router_url, device_id) = {
                match config.try_read() {
                    Ok(cfg) => (
                        cfg.k2k.router_url.clone().unwrap_or_default(),
                        cfg.k2k.client_id.clone()
                    ),
                    Err(_) => (
                        String::new(),
                        "default-device".to_string()
                    )
                }
            };
            let subscription_manager = Arc::new(RwLock::new(SubscriptionManager::new(router_url, device_id)));

            // Initialize voice service
            let voice_service = Arc::new(RwLock::new(VoiceService::new(
                config.clone(),
                claude_client.clone(),
            )));

            // Initialize guardrails from persisted config (fall back to defaults on lock/read errors)
            let guardrails_config = {
                match config.try_read() {
                    Ok(cfg) => cfg.guardrails.clone(),
                    Err(_) => GuardrailsConfig::default(),
                }
            };
            let guardrails = Arc::new(RwLock::new(Guardrails::new(guardrails_config.clone())));
            info!(
                "[GUARDRAILS] Security system initialized with {:?} protection",
                guardrails_config.security_level
            );

            // Initialize skills manager (graceful degradation — no panic)
            let skills_manager = match SkillsManager::new() {
                Ok(mut manager) => {
                    // Extract bundled skills on first run
                    match manager.extract_bundled_skills() {
                        Ok(n) if n > 0 => info!("[SKILLS] Extracted {} bundled skills on first run", n),
                        Ok(_) => {}
                        Err(e) => tracing::warn!("[SKILLS] Failed to extract bundled skills: {}", e),
                    }
                    info!("[SKILLS] Skills system initialized");
                    Arc::new(RwLock::new(manager))
                }
                Err(e) => {
                    tracing::error!("[SKILLS] Failed to initialize skills manager: {}. Skills will be unavailable.", e);
                    // Try once more; if it still fails, return the error to Tauri setup
                    match SkillsManager::new() {
                        Ok(manager) => Arc::new(RwLock::new(manager)),
                        Err(e2) => {
                            return Err(format!("Could not initialize skills system: {}", e2).into());
                        }
                    }
                }
            };

            // Start skills hot-reload watcher
            if let Err(e) = skills::start_skills_watcher(skills_manager.clone()) {
                tracing::warn!("[SKILLS] Failed to start skills file watcher: {}", e);
            }

            // Initialize skill lifecycle manager (self-learning skills pipeline)
            let skill_lifecycle_tx = match skill_lifecycle::SkillLifecycleManager::new() {
                Ok(mgr) => {
                    info!("[SKILL_LIFECYCLE] Skill lifecycle manager initialized");
                    let tx = mgr.start_background_task(
                        claude_client.clone(),
                        skills_manager.clone(),
                    );
                    tx
                }
                Err(e) => {
                    tracing::warn!(
                        "[SKILL_LIFECYCLE] Failed to initialize skill lifecycle manager: {}. \
                         Self-learning skills disabled.",
                        e
                    );
                    // Provide a no-op sender so the rest of the app compiles cleanly.
                    let (tx, _rx) = tokio::sync::mpsc::channel(1);
                    Arc::new(tx)
                }
            };

            // Initialize memory manager (graceful degradation — no panic)
            let memory_manager = match MemoryManager::new() {
                Ok(manager) => {
                    info!("[MEMORY] Memory system initialized");
                    Arc::new(RwLock::new(manager))
                }
                Err(e) => {
                    tracing::error!("[MEMORY] Failed to initialize memory manager: {}. Memory will be unavailable.", e);
                    match MemoryManager::new() {
                        Ok(manager) => Arc::new(RwLock::new(manager)),
                        Err(e2) => {
                            return Err(format!("Could not initialize memory system: {}", e2).into());
                        }
                    }
                }
            };

            // Initialize heartbeat manager (disabled by default)
            let heartbeat_manager = Arc::new(HeartbeatManager::new(HeartbeatConfig::default()));
            info!("[HEARTBEAT] Heartbeat system initialized (disabled by default)");

            // Initialize bridge manager
            let bridge_manager = Arc::new(RwLock::new(BridgeManager::new()));
            info!("[BRIDGE] Bridge manager initialized");

            // Initialize Computer Use manager
            let computer_use_config = {
                match config.try_read() {
                    Ok(cfg) => cfg.computer_use.clone(),
                    Err(_) => config::ComputerUseConfig::default(),
                }
            };
            let computer_use = Arc::new(RwLock::new(ComputerUseManager::new(
                computer_use_config.enabled,
                computer_use::DisplayConfig {
                    width: computer_use_config.display_width,
                    height: computer_use_config.display_height,
                },
                computer_use_config.require_confirmation,
            )));
            info!("[COMPUTER_USE] Manager initialized (enabled: {})", computer_use_config.enabled);

            // Initialize Browser manager
            let browser_config = {
                match config.try_read() {
                    Ok(cfg) => cfg.browser.clone(),
                    Err(_) => config::BrowserConfig::default(),
                }
            };
            let browser_manager = Arc::new(RwLock::new(BrowserManager::new(browser_config.clone())));
            info!("[BROWSER] Manager initialized (enabled: {})", browser_config.enabled);

            // Initialize defense pipeline
            let defense_config = {
                match config.try_read() {
                    Ok(cfg) => cfg.defense.clone(),
                    Err(_) => defense::DefenseConfig::default(),
                }
            };
            // Get security level from guardrails config for defense pipeline
            let guardrails_security_level = {
                match config.try_read() {
                    Ok(cfg) => cfg.guardrails.security_level,
                    Err(_) => guardrails::SecurityLevel::Standard,
                }
            };
            let defense_pipeline = Arc::new(RwLock::new(DefensePipeline::new(defense_config, guardrails_security_level)));
            let defense_clone = defense_pipeline.clone();
            let defense_app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tauri::Emitter;
                info!("[DEFENSE] Initializing defense pipeline...");

                // Notify frontend that defense models are loading
                let _ = defense_app_handle.emit("defense:loading", serde_json::json!({
                    "status": "loading",
                    "message": "Loading defense models..."
                }));

                if let Err(e) = defense_clone.write().await.initialize().await {
                    tracing::warn!("[DEFENSE] Failed to initialize defense pipeline: {}", e);
                }

                // Notify frontend that defense initialization is complete
                let status = defense_clone.read().await.get_status();
                let loaded_status = if status.deberta_loaded || status.llama_guard_loaded {
                    "ready"
                } else if status.enabled {
                    "degraded"
                } else {
                    "ready"
                };
                let _ = defense_app_handle.emit("defense:loaded", serde_json::json!({
                    "status": loaded_status,
                    "deberta_loaded": status.deberta_loaded,
                    "llama_guard_loaded": status.llama_guard_loaded,
                }));
            });

            // Initialize MCP manager
            let mcp_manager = Arc::new(RwLock::new(MCPManager::new(config.clone())));
            let mcp_clone = mcp_manager.clone();
            tauri::async_runtime::spawn(async move {
                info!("[MCP] Initializing MCP servers...");
                if let Err(e) = mcp_clone.write().await.initialize().await {
                    tracing::warn!("[MCP] Failed to initialize MCP servers: {}", e);
                }
            });

            // Start bridge service asynchronously
            let bridge_clone = bridge_manager.clone();
            tauri::async_runtime::spawn(async move {
                info!("[BRIDGE] Starting bridge service...");
                match bridge_clone.read().await.ensure_running().await {
                    Ok(_) => info!("[BRIDGE] Bridge service is ready"),
                    Err(e) => tracing::warn!("[BRIDGE] Failed to start bridge service: {}", e),
                }
            });

            // Initialize session overrides
            let session_overrides = Arc::new(RwLock::new(SessionOverrides::default()));

            // Initialize notification dispatcher with the Tauri app handle for GUI events.
            let notification_dispatcher = Arc::new(notifications::NotificationDispatcher::new(
                config.clone(),
                Some(app.handle().clone()),
            ));

            // Initialize scheduler
            let scheduler = Arc::new(Scheduler::new(
                config.clone(),
                claude_client.clone(),
                notification_dispatcher.clone(),
            ));

            // Initialize context manager
            let context_manager = {
                let cfg = config.try_read().map_err(|_| "Config lock failed")?;
                let ctx_config = context_manager::ContextManagerConfig {
                    enabled: cfg.claude.auto_compact_enabled,
                    compaction_threshold: cfg.claude.auto_compact_threshold,
                    approaching_threshold: 0.70,
                    preserve_recent_messages: 20,
                    archive_summaries: true,
                    max_compactions_per_day: 5,
                    ..Default::default()
                };
                Arc::new(context_manager::ContextManager::new(ctx_config))
            };
            info!("[CONTEXT] Context manager initialized");

            // Initialize OAuth manager
            let oauth_manager = match oauth_manager::OAuthManager::new() {
                Ok(mgr) => {
                    info!("[OAUTH] OAuth manager initialized");
                    Arc::new(RwLock::new(mgr))
                }
                Err(e) => {
                    return Err(format!("Could not initialize OAuth system: {}", e).into());
                }
            };

            // Initialize observability services
            let cost_tracker = Arc::new(observability::CostTracker::new());
            let audit_log = Arc::new(observability::AuditLog::new(10_000)); // Keep 10k entries
            info!("[OBSERVABILITY] Cost tracker and audit log initialized");

            // Initialize agent control (killswitch)
            let agent_control = Arc::new(agent_control::AgentControl::new());
            info!("[CONTROL] Agent control initialized (killswitch ready)");

            // Initialize session manager
            let session_manager = {
                let mut mgr = sessions::SessionManager::new();
                let enc_cfg = config.try_read().map(|c| c.session_encryption.clone()).unwrap_or_default();
                let encryptor = build_session_encryptor(&enc_cfg);
                mgr.set_encryptor(encryptor);
                Arc::new(RwLock::new(mgr))
            };

            // Initialize pairing manager
            let pairing_manager = match pairing::PairingManager::new() {
                Ok(mgr) => {
                    info!("[PAIRING] Pairing manager initialized");
                    Arc::new(RwLock::new(mgr))
                }
                Err(e) => {
                    tracing::warn!("[PAIRING] Failed to initialize pairing manager: {}", e);
                    // Create a fallback — try once more
                    Arc::new(RwLock::new(
                        pairing::PairingManager::new()
                            .map_err(|e2| format!("Could not initialize pairing system: {}", e2))?
                    ))
                }
            };

            // Initialize agent manager
            let agent_manager = {
                let cfg = config.try_read().map_err(|_| "Config lock failed")?;
                Arc::new(RwLock::new(agent::AgentManager::new(&cfg, config.clone())))
            };
            info!("[AGENT] Agent manager initialized");

            // Initialize model registry
            let model_registry = Arc::new(RwLock::new(providers::ModelRegistry::new(config.clone())));
            {
                let registry_clone = model_registry.clone();
                tauri::async_runtime::spawn(async move {
                    let mut registry = registry_clone.write().await;
                    registry.initialize().await;
                });
            }
            info!("[MODEL_REGISTRY] Model registry initialized");

            // Initialize user identity manager
            let user_manager = match user_identity::UserIdentityManager::new() {
                Ok(mgr) => {
                    info!("[USER-IDENTITY] User identity manager initialized");
                    Arc::new(RwLock::new(mgr))
                }
                Err(e) => {
                    tracing::warn!("[USER-IDENTITY] Failed to initialize: {}", e);
                    Arc::new(RwLock::new(
                        user_identity::UserIdentityManager::new()
                            .map_err(|e2| format!("Could not initialize user identity system: {}", e2))?
                    ))
                }
            };

            // Initialize agent orchestrator
            let orchestrator = Arc::new(agent_team::AgentOrchestrator::new());
            info!("[ORCHESTRATOR] Agent orchestrator initialized");

            // Initialize key rotation manager
            let key_rotation_manager = Arc::new(key_rotation::KeyRotationManager::new());
            info!("[KEY_ROTATION] Key rotation manager initialized");

            // Spawn a background task that checks rotation schedules daily and
            // emits expiry warnings.  The check interval is 24 hours.
            {
                let krm = key_rotation_manager.clone();
                tauri::async_runtime::spawn(async move {
                    // Use a 24-hour interval so the check runs once per day.
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
                    loop {
                        interval.tick().await;
                        krm.check_rotation_due().await;
                        krm.check_expiry_warnings().await;
                        info!("[KEY_ROTATION] Daily rotation/expiry check completed");
                    }
                });
            }

            // Initialize family mode manager
            let family_mode_manager = Arc::new(family_mode::FamilyModeManager::new());
            info!("[FAMILY] Family mode manager initialized");

            // Initialize database maintenance manager
            let backup_dir = dirs::config_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("nexibot/backups");
            let db_maintenance_manager = Arc::new(
                db_maintenance::DbMaintenanceManager::new(
                    backup_dir,
                    db_maintenance::MaintenanceConfig::default(),
                )
                .expect("Failed to initialize database maintenance manager")
            );
            info!("[DB_MAINTENANCE] Database maintenance manager initialized");

            // Initialize dashboard manager
            let dashboard_manager = Arc::new(dashboard::DashboardManager::new());
            info!("[DASHBOARD] Dashboard manager initialized");

            // Initialize advanced memory manager
            let advanced_memory_manager = Arc::new(memory_advanced::AdvancedMemoryManager::new());
            info!("[MEMORY_ADVANCED] Advanced memory manager initialized");

            // Initialize orchestration subsystems
            let orchestration_manager = Arc::new(RwLock::new(
                orchestration::OrchestrationManager::new(orchestration::OrchestrationConfig::default())
            ));
            let subagent_executor = Arc::new(RwLock::new(
                SubagentExecutor::new(SubagentExecutorConfig::default())
            ));
            let shared_workspace = Arc::new(RwLock::new(SharedWorkspace::new()));
            let circuit_breaker = Arc::new(RwLock::new(
                CircuitBreakerRegistry::new(CircuitBreakerConfig::default())
            ));
            info!("[ORCHESTRATION] Subagent executor, shared workspace, and circuit breaker initialized");

            // Spawn background task to periodically evict expired workspace entries.
            {
                let ws_cleanup = shared_workspace.clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                    loop {
                        interval.tick().await;
                        ws_cleanup.write().await.clear_expired();
                    }
                });
            }

            // Initialize DAG subsystem
            let dag_db_path = dirs::config_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("nexibot/dag/dags.db");
            let dag_store = Arc::new(std::sync::Mutex::new(
                dag::store::DagStore::new(dag_db_path)
                    .expect("Failed to initialize DAG store")
            ));
            let dag_executor = Arc::new(dag::executor::DagExecutor::new(
                dag_store.clone(),
                subagent_executor.clone(),
            ));
            info!("[DAG] DAG store and executor initialized");

            // Initialize agent engine run registry
            let agent_run_registry = Arc::new(RwLock::new(
                commands::AgentRunRegistry::new()
            ));

            // Build service groups (Arc::clone is cheap -- just refcount bump)
            let model_cache = Arc::new(RwLock::new(None));
            let ptt_capture = Arc::new(RwLock::new(None));
            let oauth_state = Arc::new(RwLock::new(None));
            let task_manager = Arc::new(RwLock::new(task_manager::TaskManager::new()));
            let tool_policy_manager = Arc::new(RwLock::new(
                security::tool_policy::ToolPolicyManager::new()
            ));
            let exec_approval_manager = Arc::new(RwLock::new(
                security::exec_approval::ExecApprovalManager::new(
                    security::exec_approval::ApprovalMode::default()
                )
            ));

            // Initialize Smart Key Vault
            let key_interceptor = {
                use sha2::{Digest, Sha256};
                use security::key_vault::KeyVault;
                use security::key_interceptor::KeyInterceptor;

                // Derive a stable machine-specific passphrase (no user prompt, no keyring).
                // Uses SHA-256(home_dir + username + app_id + salt) for determinism per machine/user.
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
                        tracing::warn!("[KEY_VAULT] Failed to initialize vault: {}. Keys will not be intercepted.", e);
                        KeyVault::new(
                            std::path::PathBuf::from(":memory:"),
                            &passphrase,
                            false,
                        ).expect("In-memory vault must always succeed")
                    }
                };

                KeyInterceptor::new(Arc::new(vault))
            };

            // Initialize NexiGate gated shell
            let gated_shell = {
                use gated_shell::GatedShell;

                let gs_config = config
                    .try_read()
                    .map(|c| c.gated_shell.clone())
                    .unwrap_or_default();

                // Populate filter from vault mappings
                let vault_mappings = key_interceptor.vault().all_proxy_to_real();

                Arc::new(GatedShell::new(
                    gs_config,
                    &vault_mappings,
                    app.handle().clone(),
                ))
            };
            info!("[NEXIGATE] Gated shell initialized");

            // Initialize yolo mode manager
            let yolo_config = config
                .try_read()
                .map(|c| c.yolo_mode.clone())
                .unwrap_or_default();
            let yolo_manager = yolo_mode::YoloModeManager::new(yolo_config);
            info!("[YOLO] Yolo mode manager initialized");

            // Initialize managed policy manager (KN Central Management)
            let managed_policy_manager = Arc::new(
                managed_policy::ManagedPolicyManager::new(config.clone())
            );
            info!("[MANAGED_POLICY] Manager initialized");

            // Initialize network policy engine
            let network_policy = Arc::new(
                crate::security::network_policy::NetworkPolicyEngine::new(
                    config
                        .try_read()
                        .map(|c| c.network_policy.clone())
                        .unwrap_or_default(),
                ),
            );

            // Register databases with maintenance manager (spawned async)
            {
                let db_mgr = db_maintenance_manager.clone();
                tauri::async_runtime::spawn(async move {
                    let memory_db_path = dirs::config_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join("nexibot/memory/memories.db");
                    if let Err(e) = db_mgr.register_database(memory_db_path).await {
                        warn!("[DB_MAINTENANCE] Failed to register memory database: {}", e);
                    }

                    let dag_db_path = dirs::config_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join("nexibot/dag/dags.db");
                    if let Err(e) = db_mgr.register_database(dag_db_path).await {
                        warn!("[DB_MAINTENANCE] Failed to register DAG database: {}", e);
                    }

                    info!("[DB_MAINTENANCE] Registered databases for maintenance");
                });
            }

            let core = commands::CoreServices {
                config: config.clone(),
                config_changed: config_changed_tx.clone(),
                user_manager: user_manager.clone(),
                orchestrator: orchestrator.clone(),
            };
            let llm = commands::LlmServices {
                claude_client: claude_client.clone(),
                model_registry: model_registry.clone(),
                agent_manager: agent_manager.clone(),
                session_overrides: session_overrides.clone(),
                session_manager: session_manager.clone(),
                model_cache: model_cache.clone(),
            };
            let safety = commands::SafetyServices {
                guardrails: guardrails.clone(),
                defense_pipeline: defense_pipeline.clone(),
            };
            let integrations_svc = commands::IntegrationServices {
                k2k_client: k2k_client.clone(),
                mcp_manager: mcp_manager.clone(),
                bridge_manager: bridge_manager.clone(),
            };
            let channels_svc = commands::ChannelServices {
                voice_service: voice_service.clone(),
                ptt_capture: ptt_capture.clone(),
            };

            // Create plan mode Arc before AppState so both the tool registry
            // and AppState::plan_mode_state share the SAME allocation.
            let plan_mode_state_arc = std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::tools::plan_mode::PlanModeState::default()
            ));

            // Store state in Tauri (service groups + flat aliases for backward compat)
            let app_state = AppState {
                // Service groups
                core,
                llm,
                safety,
                integrations: integrations_svc,
                channels: channels_svc,
                // Flat aliases (same Arcs, just cloned references)
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
                // Standalone
                agent_control: agent_control.clone(),
                memory_manager,
                skills_manager,
                subscription_manager,
                heartbeat_manager,
                computer_use,
                browser: browser_manager,
                oauth_state,
                openai_device_flow: Arc::new(tokio::sync::RwLock::new(None)),
                scheduler: scheduler.clone(),
                context_manager: context_manager.clone(),
                oauth_manager: oauth_manager.clone(),
                cost_tracker: cost_tracker.clone(),
                audit_log: audit_log.clone(),
                pairing_manager,
                key_rotation_manager,
                family_mode_manager,
                db_maintenance_manager,
                dashboard_manager,
                advanced_memory_manager,
                task_manager,
                tool_policy_manager,
                exec_approval_manager,
                // Orchestration subsystems
                orchestration_manager,
                subagent_executor,
                shared_workspace,
                circuit_breaker,
                // DAG subsystems
                dag_store,
                dag_executor,
                // Agent engine run registry
                agent_run_registry,
                // Smart Key Vault
                key_interceptor,
                // NexiGate gated shell
                gated_shell: Some(gated_shell),
                // Yolo mode
                yolo_manager,
                // KN Central Management
                managed_policy_manager: managed_policy_manager.clone(),
                // Notifications
                notification_dispatcher,
                // Logging
                log_state: Some(log_state.clone()),
                // Telegram tool approval flow
                telegram_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                telegram_last_error: Arc::new(tokio::sync::Mutex::new(None)),
                telegram_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Discord tool approval flow
                discord_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Slack tool approval flow
                slack_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Signal tool approval flow
                signal_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // WhatsApp tool approval flow
                whatsapp_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Teams tool approval flow
                teams_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Teams conversation service URLs for approval replies in background tasks
                teams_conversation_service_urls: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
                // Matrix tool approval flow
                matrix_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // BlueBubbles tool approval flow
                bluebubbles_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Mattermost tool approval flow
                mattermost_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Google Chat tool approval flow
                google_chat_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Twilio tool approval flow
                twilio_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Messenger tool approval flow
                messenger_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Instagram tool approval flow
                instagram_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // LINE tool approval flow
                line_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Mastodon tool approval flow
                mastodon_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Mastodon account id -> acct handle cache for background approvals
                mastodon_account_handles: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
                // Rocket.Chat tool approval flow
                rocketchat_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Rocket.Chat REST auth cache (auth_token, bot_user_id)
                rocketchat_rest_auth: Arc::new(tokio::sync::RwLock::new(None)),
                // WebChat tool approval flow
                webchat_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Active WebChat session websocket senders
                webchat_session_senders: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
                // GUI tool approval flow
                gui_pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                // Native plugin registry
                plugin_registry: Arc::new(RwLock::new(crate::plugins::PluginRegistry::new())),
                // Per-agent workspace isolation
                workspace_manager: Arc::new(std::sync::Mutex::new(crate::agent_workspace::WorkspaceManager::new())),
                // Network policy engine
                network_policy,
                // Skill lifecycle channel
                skill_lifecycle_tx,
                // Shared NATS client for nats_publish tool
                nats_publish_client: Arc::new(tokio::sync::Mutex::new(None)),
                // v0.9.0 tool registry — plan_mode_state_arc is created first so
                // the tool registry and AppState::plan_mode_state share the same Arc.
                tool_registry: {
                    let reg = Arc::new(tokio::sync::RwLock::new(crate::tool_registry::ToolRegistry::new()));
                    {
                        let mut r = reg.try_write().expect("registry lock at startup");
                        crate::tools::register_all(&mut r, plan_mode_state_arc.clone());
                    }
                    reg
                },
                // v0.9.0 per-session file read state
                file_read_state: Arc::new(tokio::sync::RwLock::new(
                    crate::tools::file_read_state::FileReadState::default()
                )),
                // v0.9.0 git context cache
                git_context: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
                // v0.9.0 plan mode state — same Arc passed to register_all above.
                plan_mode_state: plan_mode_state_arc,
                // v0.9.0 cost/token tracking
                session_cost_tracker: std::sync::Arc::new(tokio::sync::RwLock::new(
                    crate::cost_tracker::CostTracker::new(uuid::Uuid::new_v4().to_string())
                )),
                budget_limits: crate::cost_tracker::BudgetLimits::default(),
                session_context_manager: std::sync::Arc::new(tokio::sync::RwLock::new(
                    crate::cost_tracker::ContextManager::new(200_000)
                )),
            };

            // Inject services into heartbeat manager for catch-up notification scan.
            {
                let hb = app_state.heartbeat_manager.clone();
                let dag_store = app_state.dag_store.clone();
                let dispatcher = app_state.notification_dispatcher.clone();
                tauri::async_runtime::spawn(async move {
                    hb.set_services(dag_store, dispatcher).await;
                });
            }

            // Start Telegram bot if enabled.  Store the stop handle so the
            // config hotloading subscriber can restart the bot without a full
            // app restart when bot_token or enabled changes.
            let telegram_shutdown: Arc<tokio::sync::Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>> =
                Arc::new(tokio::sync::Mutex::new(None));
            {
                let telegram_state = app_state.clone();
                let telegram_shutdown_init = telegram_shutdown.clone();
                tauri::async_runtime::spawn(async move {
                    match telegram::start_telegram_bot(telegram_state.clone()).await {
                        Ok(Some(token)) => {
                            *telegram_state.telegram_last_error.lock().await = None;
                            telegram_state.telegram_running.store(true, std::sync::atomic::Ordering::SeqCst);
                            *telegram_shutdown_init.lock().await = Some(token);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!("[TELEGRAM] Failed to start Telegram bot: {}", e);
                            *telegram_state.telegram_last_error.lock().await = Some(e);
                        }
                    }
                });
            }

            // Start Discord bot if enabled
            let discord_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = discord::start_discord_bot(discord_state).await {
                    tracing::warn!("[DISCORD] Failed to start Discord bot: {}", e);
                }
            });

            // Start Signal listener if enabled
            let signal_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = signal::start_signal_listener(signal_state).await {
                    tracing::warn!("[SIGNAL] Failed to start Signal listener: {}", e);
                }
            });

            // Start NATS listener if enabled
            let nats_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = nats::start_nats_listener(nats_state).await {
                    tracing::warn!("[NATS] Failed to start NATS listener: {}", e);
                }
            });

            // Start Matrix sync loop if enabled
            let matrix_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = matrix::start_matrix_sync(matrix_state).await {
                    tracing::warn!("[MATRIX] Failed to start Matrix sync: {}", e);
                }
            });

            // Start BlueBubbles (iMessage) listener
            let bluebubbles_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = bluebubbles::start_bluebubbles_listener(bluebubbles_state).await {
                    tracing::warn!("[BLUEBUBBLES] {}", e);
                }
            });

            // Start Mattermost bot
            let mattermost_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = mattermost::start_mattermost_bot(mattermost_state).await {
                    tracing::warn!("[MATTERMOST] {}", e);
                }
            });

            // Start Mastodon bot
            let mastodon_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = mastodon::start_mastodon_bot(mastodon_state).await {
                    tracing::warn!("[MASTODON] {}", e);
                }
            });

            // Start RocketChat bot
            let rocketchat_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = rocketchat::start_rocketchat_bot(rocketchat_state).await {
                    tracing::warn!("[ROCKETCHAT] {}", e);
                }
            });

            // Start WebChat server
            let webchat_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = webchat::start_webchat_server(webchat_state).await {
                    tracing::warn!("[WEBCHAT] {}", e);
                }
            });

            // Teams webhook routes are mounted by webhooks::start_webhook_server,
            // which owns the request state used by handlers.

            // Start Google Chat integration
            let google_chat_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = google_chat::start_google_chat(google_chat_state).await {
                    tracing::warn!("[GOOGLE_CHAT] {}", e);
                }
            });

            // Start Messenger bot
            let messenger_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = messenger::start_messenger(messenger_state).await {
                    tracing::warn!("[MESSENGER] {}", e);
                }
            });

            // Start Instagram bot
            let instagram_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = instagram::start_instagram(instagram_state).await {
                    tracing::warn!("[INSTAGRAM] {}", e);
                }
            });

            // Start LINE integration
            let line_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = line::start_line(line_state).await {
                    tracing::warn!("[LINE] {}", e);
                }
            });

            // Start Twilio integration
            let twilio_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = twilio::start_twilio(twilio_state).await {
                    tracing::warn!("[TWILIO] {}", e);
                }
            });

            // Start Email (IMAP/SMTP) polling
            let email_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = email::start_email_polling(email_state).await {
                    tracing::warn!("[EMAIL] {}", e);
                }
            });

            // Start Gmail polling
            let gmail_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = gmail::start_gmail_polling(gmail_state).await {
                    tracing::warn!("[GMAIL] {}", e);
                }
            });

            // Start webhook server (hosts generic webhooks + WhatsApp/Slack/Teams routes when enabled)
            let webhook_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = webhooks::start_webhook_server(
                    webhook_state.config.clone(),
                    webhook_state.scheduler.clone(),
                    webhook_state.claude_client.clone(),
                    Some(webhook_state),
                ).await {
                    tracing::warn!("[WEBHOOK] Failed to start webhook server: {}", e);
                }
            });

            // Emit Tauri events on config changes (hot reload → frontend)
            let app_handle = app.handle().clone();
            let mut config_rx = config_changed_tx.subscribe();
            tauri::async_runtime::spawn(async move {
                use tauri::Emitter;
                loop {
                    match config_rx.recv().await {
                        Ok(()) => {
                            info!("[HOT_RELOAD] Emitting config:changed to frontend");
                            if let Err(e) = app_handle.emit("config:changed", ()) {
                                tracing::warn!(
                                    "[HOT_RELOAD] Failed to emit config:changed to frontend: {}",
                                    e
                                );
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("[HOT_RELOAD] Missed {} config change events", n);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
            });

            // Subscribe services to config changes for hot reload

            // --- Telegram: restart bot when token or enabled flag changes ---
            {
                let telegram_restart_state = app_state.clone();
                let telegram_shutdown_sub = telegram_shutdown.clone();
                let mut telegram_rx = app_state.config_changed.subscribe();
                tauri::async_runtime::spawn(async move {
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
                            old.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                        telegram_restart_state.telegram_running.store(false, std::sync::atomic::Ordering::SeqCst);
                        if new_enabled && !new_token.is_empty() {
                            match telegram::start_telegram_bot(telegram_restart_state.clone()).await {
                                Ok(Some(t)) => {
                                    *telegram_restart_state.telegram_last_error.lock().await = None;
                                    telegram_restart_state.telegram_running.store(true, std::sync::atomic::Ordering::SeqCst);
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

            // --- YoloMode: update config on change ---
            {
                let yolo_reload_state = app_state.clone();
                let mut yolo_rx = app_state.config_changed.subscribe();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if yolo_rx.recv().await.is_err() { break; }
                        let new_cfg = yolo_reload_state.config.read().await.yolo_mode.clone();
                        yolo_reload_state.yolo_manager.update_config(new_cfg).await;
                    }
                });
            }

            // --- ModelRegistry: reload API keys and model defaults on config change ---
            {
                let registry_reload_state = app_state.clone();
                let mut registry_rx = app_state.config_changed.subscribe();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if registry_rx.recv().await.is_err() { break; }
                        info!("[HOT_RELOAD] Config changed — reloading model registry");
                        registry_reload_state.model_registry.write().await.reload().await;
                    }
                });
            }

            let mcp_reload_state = app_state.clone();
            let mut mcp_rx = app_state.config_changed.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    if mcp_rx.recv().await.is_err() { break; }
                    info!("[HOT_RELOAD] Config changed, reinitializing MCP servers");
                    let mut mcp = mcp_reload_state.mcp_manager.write().await;
                    if let Err(e) = mcp.initialize().await {
                        tracing::warn!("[HOT_RELOAD] MCP reinitialization failed: {}", e);
                    }
                }
            });

            let defense_reload_state = app_state.clone();
            let mut defense_rx = app_state.config_changed.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    if defense_rx.recv().await.is_err() { break; }
                    let new_defense_config = {
                        let cfg = defense_reload_state.config.read().await;
                        cfg.defense.clone()
                    };
                    info!("[HOT_RELOAD] Config changed, updating defense pipeline");
                    let mut pipeline = defense_reload_state.defense_pipeline.write().await;
                    pipeline.update_config(new_defense_config);
                }
            });

            let voice_reload_state = app_state.clone();
            let mut voice_rx = app_state.config_changed.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    if voice_rx.recv().await.is_err() { break; }
                    info!("[HOT_RELOAD] Config changed, reinitializing voice backends");
                    let voice = voice_reload_state.voice_service.read().await;
                    voice.reinit_backends().await;
                }
            });

            // Guardrails: update security level and config on change
            {
                let reload_state = app_state.clone();
                let mut rx = app_state.config_changed.subscribe();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if rx.recv().await.is_err() { break; }
                        let new_cfg = reload_state.config.read().await.guardrails.clone();
                        let mut g = reload_state.guardrails.write().await;
                        if let Err(e) = g.update_config(new_cfg) {
                            tracing::warn!("[HOT_RELOAD] Guardrails config update failed: {}", e);
                        } else {
                            info!("[HOT_RELOAD] Guardrails config updated");
                        }
                    }
                });
            }

            // GatedShell: update enabled/debug/record flags on change
            {
                let reload_state = app_state.clone();
                let mut rx = app_state.config_changed.subscribe();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if rx.recv().await.is_err() { break; }
                        if let Some(gs) = &reload_state.gated_shell {
                            let new_cfg = reload_state.config.read().await.gated_shell.clone();
                            gs.update_config(new_cfg).await;
                        }
                    }
                });
            }

            // ClaudeClient: update max_history_messages on config change
            {
                let reload_state = app_state.clone();
                let mut rx = app_state.config_changed.subscribe();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if rx.recv().await.is_err() { break; }
                        let max = reload_state.config.read().await.claude.max_history_messages;
                        reload_state.claude_client.read().await.set_max_history_messages(max).await;
                    }
                });
            }

            // --- ContextManager: update thresholds and flush config on change ---
            {
                let reload_state = app_state.clone();
                let mut rx = app_state.config_changed.subscribe();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if rx.recv().await.is_err() { break; }
                        let cfg = reload_state.config.read().await;
                        let ctx_config = context_manager::ContextManagerConfig {
                            enabled: cfg.claude.auto_compact_enabled,
                            compaction_threshold: cfg.claude.auto_compact_threshold,
                            approaching_threshold: 0.70,
                            preserve_recent_messages: 20,
                            archive_summaries: true,
                            max_compactions_per_day: 5,
                            ..Default::default()
                        };
                        drop(cfg);
                        reload_state.context_manager.update_config(ctx_config);
                    }
                });
            }

            // Key Vault → GatedShell: re-sync filter when vault entries change
            // Triggered by the same config_changed broadcast that fires after any
            // vault mutation (add/revoke), so new secrets are immediately masked.
            {
                let reload_state = app_state.clone();
                let mut rx = app_state.config_changed.subscribe();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if rx.recv().await.is_err() { break; }
                        if let Some(gs) = &reload_state.gated_shell {
                            let mappings = reload_state.key_interceptor.vault().all_proxy_to_real();
                            gs.sync_vault(&mappings);
                            info!("[HOT_RELOAD] Key vault synced to GatedShell filter ({} secrets)", mappings.len());
                        }
                    }
                });
            }

            // --- BrowserManager: reload domain allowlist on config change ---
            {
                let reload_state = app_state.clone();
                let mut rx = app_state.config_changed.subscribe();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if rx.recv().await.is_err() { break; }
                        let new_cfg = reload_state.config.read().await.browser.clone();
                        reload_state.browser.write().await.update_config(new_cfg);
                        info!("[HOT_RELOAD] Browser config updated");
                    }
                });
            }

            // --- AgentManager: rebuild agents on config change ---
            {
                let reload_state = app_state.clone();
                let global_config = app_state.config.clone();
                let mut rx = app_state.config_changed.subscribe();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if rx.recv().await.is_err() { break; }
                        let new_cfg = reload_state.config.read().await.clone();
                        reload_state.agent_manager.write().await.reload_agents(&new_cfg, global_config.clone());
                        info!("[HOT_RELOAD] Agent manager reloaded");
                    }
                });
            }

            // Auto-start voice service if wake word is enabled
            // Also sets app_state and app_handle for tool execution and memory
            let voice_state = app_state.clone();
            let voice_app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Pass app_state to voice service for tool execution and memory
                {
                    let voice_state_clone = voice_state.clone();
                    let mut voice = voice_state.voice_service.write().await;
                    voice.set_app_state(voice_state_clone);
                    voice.set_app_handle(voice_app_handle);
                }

                // Voice service is NOT auto-started at launch.
                // User must explicitly enable voice via the GUI toggle or /voice Telegram command.
                // This avoids loading ~300–600 MB of ONNX models at startup when voice is unused.
            });

            app.manage(app_state);

            // Start yolo mode expiry watcher (must run inside async context)
            {
                let yolo_watcher = app.state::<AppState>().yolo_manager.clone();
                tauri::async_runtime::spawn(async move {
                    yolo_watcher.start_expiry_watcher();
                });
            }

            // Start KN Central Management (register + heartbeat loop).
            // Safe to call when disabled — returns immediately.
            {
                let mp_mgr = app.state::<AppState>().managed_policy_manager.clone();
                tauri::async_runtime::spawn(async move {
                    mp_mgr.start().await;
                });
            }

            // Bridge internal yolo broadcast events → Tauri frontend events.
            // Expired and RequestCancelled have no explicit caller that emits them,
            // so this watcher is the only path that delivers them to the UI.
            // Approved / Revoked / RequestPending are already emitted explicitly
            // in yolo_cmds.rs, so this bridge skips them to avoid doubles.
            {
                use crate::yolo_mode::YoloEvent;
                use tauri::Emitter;
                let yolo_event_app = app.handle().clone();
                let mut yolo_event_rx = app.state::<AppState>().yolo_manager.subscribe();
                tauri::async_runtime::spawn(async move {
                    loop {
                        match yolo_event_rx.recv().await {
                            Ok(YoloEvent::Expired) => {
                                info!("[YOLO] Session expired — emitting yolo:expired");
                                if let Err(e) = yolo_event_app.emit("yolo:expired", ()) {
                                    warn!("[YOLO] Failed to emit yolo:expired: {}", e);
                                }
                            }
                            Ok(YoloEvent::RequestCancelled) => {
                                // revoke() was called while inactive; treat as revoked in UI.
                                if let Err(e) = yolo_event_app.emit("yolo:revoked", ()) {
                                    warn!("[YOLO] Failed to emit yolo:revoked: {}", e);
                                }
                            }
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!("[YOLO] Event bridge lagged by {} events; some UI updates may be missed", n);
                            }
                        }
                    }
                });
            }

            // K2K device-agent long-poll notification loop.
            // Polls GET /api/v1/device-agents/poll (30s server-side timeout).
            // When the server pushes a command the event is dispatched to the frontend
            // as "k2k:notification" so the UI can surface it to the user.
            {
                use tauri::Emitter;
                let poll_app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        // Check if K2K is enabled before each poll cycle
                        let (enabled, base_url) = {
                            let poll_state = poll_app_handle.state::<AppState>();
                            let cfg = poll_state.config.read().await;
                            let url = cfg.k2k.router_url
                                .clone()
                                .unwrap_or_else(|| cfg.k2k.local_agent_url.clone());
                            (cfg.k2k.enabled, url)
                        };

                        if !enabled {
                            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                            continue;
                        }

                        let http = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(35))
                            .build()
                            .unwrap_or_default();

                        match http
                            .get(format!("{}/api/v1/device-agents/poll", base_url))
                            .send()
                            .await
                        {
                            Ok(resp) if resp.status().is_success() => {
                                match resp.json::<serde_json::Value>().await {
                                    Ok(payload) => {
                                        // Only emit when the server actually pushed a command
                                        if !payload.is_null()
                                            && payload != serde_json::json!({})
                                            && payload != serde_json::json!({"status": "timeout"})
                                        {
                                            info!("[K2K_POLL] Received notification: {:?}", payload);
                                            if let Err(e) = poll_app_handle
                                                .emit("k2k:notification", &payload)
                                            {
                                                warn!(
                                                    "[K2K_POLL] Failed to emit k2k:notification: {}",
                                                    e
                                                );
                                            }
                                        }
                                        // Server-sent timeout → immediately re-poll (no sleep needed)
                                    }
                                    Err(e) => {
                                        warn!("[K2K_POLL] Failed to parse poll response: {}", e);
                                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                    }
                                }
                            }
                            Ok(resp) => {
                                // Non-success status — back off briefly before retrying
                                warn!(
                                    "[K2K_POLL] Poll returned HTTP {}; backing off 10s",
                                    resp.status()
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                            }
                            Err(e) => {
                                // Network error — back off to avoid hammering a down server
                                warn!("[K2K_POLL] Poll failed: {}; backing off 15s", e);
                                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                            }
                        }
                    }
                });
            }

            // Start scheduler loop
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                scheduler.run_loop(app_handle).await;
            });

            // Setup tray menu
            let show_item = MenuItem::with_id(app, "show", "Show NexiBot", true, None::<&str>)?;
            let search_item = MenuItem::with_id(app, "search", "Open Search UI", true, None::<&str>)?;
            let voice_item = MenuItem::with_id(app, "voice", "Start Voice", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

            let menu = Menu::with_items(app, &[&show_item, &search_item, &voice_item, &quit_item])?;

            // ============================================================================
            // TRAY ICON SETUP - IMPORTANT: Use the correct icon file!
            // ============================================================================
            //
            // CORRECT:  tray-icon.png - Black brain on transparent (template icon)
            //           This icon inverts with macOS light/dark mode
            //
            // WRONG:    Any other icon file (icon.icns, window icons, etc.)
            //           These are for the dock/app icon, NOT the menubar
            //
            // The template icon MUST be:
            //   - Black design on transparent background
            //   - Will be inverted by macOS automatically
            //   - Used with icon_as_template(true)
            //
            // See: src-tauri/icons/README.md for full documentation
            // ============================================================================

            // Load the menubar template icon (NOT the app icon!)
            let tray_icon_bytes = include_bytes!("../icons/tray-icon.png");
            let tray_icon = Image::from_bytes(tray_icon_bytes)?;

            // Compile-time check: Verify we're loading the correct file
            const _TRAY_ICON_CHECK: &str = "If you see a compile error here, you're using the wrong icon file for the tray. Must use tray-icon.png!";
            const _: () = {
                // This will fail to compile if the path is wrong
                let _ = include_bytes!("../icons/tray-icon.png");
            };

            let _tray = TrayIconBuilder::new()
                .icon(tray_icon)
                .menu(&menu)
                .icon_as_template(true)  // macOS template icon (inverts for light/dark mode)
                .show_menu_on_left_click(false)  // Show menu on right-click, not left-click
                .on_menu_event(|app, event| {
                    match event.id.as_ref() {
                        "show" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                                // Activate the app so the window comes to foreground on macOS
                                #[cfg(target_os = "macos")]
                                {
                                    use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
                                    unsafe {
                                        let app_instance = NSApp();
                                        app_instance.activateIgnoringOtherApps_(true);
                                    }
                                }
                            } else {
                                tracing::warn!("[TRAY] Could not find main window for show action");
                            }
                        }
                        "search" => {
                            // Open the K2K Search UI in default browser
                            info!("[TRAY] Opening K2K Search UI at http://localhost:19850");
                            if let Err(e) = open::that("http://localhost:19850") {
                                tracing::warn!("[TRAY] Failed to open Search UI: {}", e);
                            }
                        }
                        "voice" => {
                            // Start voice service
                            let app_state = app.state::<AppState>();
                            let voice_service = app_state.voice_service.clone();
                            tauri::async_runtime::spawn(async move {
                                let mut voice = voice_service.write().await;
                                if let Err(e) = voice.start().await {
                                    tracing::error!("[VOICE] Failed to start voice from tray: {}", e);
                                }
                            });
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            // Toggle visibility
                            if let Ok(is_visible) = window.is_visible() {
                                if is_visible {
                                    let _ = window.hide();
                                } else {
                                    let _ = window.show();
                                    let _ = window.set_focus();
                                    #[cfg(target_os = "macos")]
                                    unsafe {
                                        use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
                                        NSApp().activateIgnoringOtherApps_(true);
                                    }
                                }
                            } else {
                                // If we can't check visibility, just show it
                                let _ = window.show();
                                let _ = window.set_focus();
                                #[cfg(target_os = "macos")]
                                unsafe {
                                    use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
                                    NSApp().activateIgnoringOtherApps_(true);
                                }
                            }
                        } else {
                            tracing::warn!("[TRAY] Could not find main window");
                        }
                    }
                })
                .build(app)?;

            info!("[TRAY] Tray icon initialized with menu");

            // ── Deep-link handler for nexibot:// protocol ──────────────────────
            // The KN OAuth callback redirects to nexibot://oauth-complete?... after
            // the user authorises a connector. We forward the URL to the frontend
            // so ConnectorWizard can finish the flow.
            {
                let app_handle_dl = app.handle().clone();
                app.handle().listen("deep-link://new-url", move |event| {
                    use tauri::Emitter;
                    // Plugin emits Vec<Url> serialized as JSON array of strings
                    let urls: Vec<String> = serde_json::from_str(event.payload()).unwrap_or_default();
                    for url in urls {
                        info!("[DEEPLINK] Received: {}", url);
                        if !url.starts_with("nexibot://oauth-complete") {
                            warn!("[DEEPLINK] Rejected unexpected deep-link URL: {}", url);
                            continue;
                        }
                        if let Some(w) = app_handle_dl.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                            let _ = w.emit("nexibot://deep-link", serde_json::json!({ "url": url }));
                        }
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::agent_control_cmds::agent_emergency_stop,
            commands::agent_control_cmds::agent_pause,
            commands::agent_control_cmds::agent_resume,
            commands::agent_control_cmds::get_agent_status,
            commands::send_message,
            commands::send_message_with_events,
            commands::cancel_message,
            commands::is_supermemory_available,
            commands::compact_conversation,
            commands::get_context_usage,
            commands::get_context_metrics,
            commands::get_compaction_history,
            commands::reset_compaction_counter,
            commands::search_k2k,
            commands::get_platform_info,
            commands::start_voice_service,
            commands::stop_voice_service,
            commands::get_voice_status,
            commands::test_stt,
            commands::test_tts,
            commands::push_to_talk,
            commands::set_voice_response_enabled,
            commands::get_voice_response_enabled,
            commands::voice_stop_listening,
            commands::voice_set_wakeword_enabled,
            commands::ptt_start,
            commands::ptt_stop,
            commands::ptt_cancel,
            commands::start_audio_capture,
            commands::stop_audio_capture,
            commands::get_config,
            commands::update_config,
            commands::is_first_run,
            commands::add_oauth_profile,
            commands::list_oauth_profiles,
            commands::remove_oauth_profile,
            commands::get_oauth_status,
            commands::start_oauth_flow,
            commands::open_oauth_browser,
            commands::complete_oauth_flow,
            commands::get_active_oauth_profile,
            commands::refresh_active_oauth_token,
            commands::set_active_oauth_profile,
            commands::get_oauth_provider_status,
            commands::oauth_provider_has_profiles,
            commands::get_oauth_access_token,
            commands::start_openai_device_flow,
            commands::poll_openai_device_flow,
            commands::observability::get_cost_metrics,
            commands::observability::check_daily_budget,
            commands::observability::record_api_usage,
            commands::observability::get_audit_logs,
            commands::observability::log_audit_event,
            commands::observability::filter_audit_logs,
            commands::start_claude_cli_auth,
            commands::check_subscription,
            commands::get_subscription_credentials,
            commands::list_subscriptions,
            commands::open_subscription_portal,
            commands::refresh_subscriptions,
            commands::get_soul,
            commands::list_soul_templates,
            commands::load_soul_template,
            commands::update_soul,
            commands::get_guardrails_config,
            commands::update_guardrails_config,
            commands::check_command_safety,
            commands::get_security_warnings,
            commands::execute_command_safe,
            commands::run_security_audit,
            commands::auto_fix_finding,
            commands::list_skills,
            commands::get_skill,
            commands::list_user_invocable_skills,
            commands::reload_skills,
            commands::add_memory,
            commands::get_memory,
            commands::search_memories,
            commands::get_memories_by_type,
            commands::delete_memory,
            commands::start_conversation_session,
            commands::add_session_message,
            commands::set_session_title,
            commands::get_current_session,
            commands::list_conversation_sessions,
            commands::end_conversation_session,
            commands::start_heartbeat,
            commands::stop_heartbeat,
            commands::get_heartbeat_config,
            commands::update_heartbeat_config,
            commands::is_heartbeat_running,
            commands::trigger_heartbeat,
            commands::get_bridge_status,
            commands::start_bridge,
            commands::stop_bridge,
            commands::restart_bridge,
            commands::install_bridge,
            commands::check_bridge_health,
            commands::ensure_bridge_running,
            commands::list_mcp_servers,
            commands::list_mcp_tools,
            commands::connect_mcp_server,
            commands::disconnect_mcp_server,
            commands::add_mcp_server,
            commands::remove_mcp_server,
            commands::check_accessibility_permissions,
            commands::request_accessibility_permissions,
            commands::get_defense_status,
            commands::get_tool_permissions,
            commands::update_tool_permissions,
            // Session overrides
            commands::set_session_model,
            commands::toggle_thinking,
            commands::toggle_verbose,
            commands::get_session_overrides,
            commands::reset_session_overrides,
            // Provider
            commands::set_session_provider,
            commands::get_provider_status,
            commands::get_available_models,
            commands::refresh_model_cache,
            commands::validate_provider_models,
            // Conversation history
            commands::load_conversation_session,
            commands::new_conversation,
            // Scheduler
            commands::list_scheduled_tasks,
            commands::add_scheduled_task,
            commands::remove_scheduled_task,
            commands::update_scheduled_task,
            commands::get_scheduler_results,
            commands::trigger_scheduled_task,
            commands::get_scheduler_enabled,
            commands::set_scheduler_enabled,
            // Webhooks
            commands::get_webhook_config,
            commands::set_webhook_enabled,
            commands::add_webhook_endpoint,
            commands::remove_webhook_endpoint,
            commands::regenerate_webhook_token,
            commands::set_discord_enabled,
            commands::set_slack_enabled,
            // Telegram
            commands::get_telegram_status,
            commands::get_telegram_config,
            commands::set_telegram_enabled,
            commands::set_telegram_bot_token,
            commands::set_telegram_allowed_chat_ids,
            commands::set_telegram_voice_enabled,
            commands::send_telegram_test_message,
            // WhatsApp
            commands::get_whatsapp_config,
            commands::set_whatsapp_enabled,
            commands::set_whatsapp_phone_number_id,
            commands::set_whatsapp_access_token,
            commands::set_whatsapp_verify_token,
            commands::set_whatsapp_app_secret,
            commands::set_whatsapp_allowed_numbers,
            // Skills CRUD
            commands::create_skill,
            commands::update_skill,
            commands::delete_skill,
            commands::list_skill_templates,
            commands::test_skill,
            commands::reset_bundled_skills,
            commands::invoke_skill,
            commands::get_skill_config,
            commands::save_skill_config,
            // Multi-agent coordination
            commands::list_agent_capabilities,
            commands::submit_agent_task,
            commands::poll_agent_task,
            // Web search via K2K
            commands::search_web_via_k2k,
            // Knowledge push
            commands::push_knowledge,
            commands::update_knowledge,
            commands::list_knowledge_stores,
            // Knowledge tier promotion + approval workflow
            commands::promote_knowledge,
            commands::list_pending_approvals,
            commands::approve_contribution,
            // On-demand research
            commands::trigger_research,
            commands::poll_research_task,
            commands::resume_research_task,
            commands::list_research_tasks,
            // KB browsing
            commands::list_knowledge,
            commands::get_knowledge_item,
            // ClawHub marketplace
            commands::search_clawhub,
            commands::get_clawhub_skill_info,
            commands::install_clawhub_skill,
            commands::analyze_skill_security,
            // Connector wizard
            commands::get_supported_connectors,
            commands::start_connector_oauth,
            commands::poll_connector_status,
            commands::list_user_connectors,
            commands::delete_connector,
            // Ollama
            commands::discover_ollama_models,
            // Named sessions / inter-agent messaging
            commands::create_named_session,
            commands::list_named_sessions,
            commands::switch_named_session,
            commands::send_inter_session_message,
            commands::get_session_inbox,
            commands::delete_named_session,
            // Autonomous mode
            commands::get_autonomous_config,
            commands::update_autonomous_config,
            // Startup / launch-at-login
            commands::get_startup_config,
            commands::set_nexibot_autostart,
            commands::set_k2k_agent_autostart,
            commands::get_startup_status,
            // DM Pairing
            commands::list_pairing_requests,
            commands::approve_pairing_code,
            commands::deny_pairing_code,
            commands::respond_tool_approval,
            commands::set_telegram_dm_policy,
            commands::set_whatsapp_dm_policy,
            commands::set_discord_dm_policy,
            commands::set_slack_dm_policy,
            commands::set_signal_dm_policy,
            commands::get_runtime_allowlist,
            commands::remove_from_allowlist,
            // Multi-agent
            commands::list_agents,
            commands::get_agent,
            commands::get_active_gui_agent,
            commands::set_active_gui_agent,
            // Orchestration
            commands::nexibot_orchestrate,
            // Agent engine (autonomous workflows + LLM planning)
            commands::run_agent,
            commands::plan_agent,
            commands::get_agent_run_status,
            commands::list_agent_defs,
            commands::save_agent_def,
            // Auto-updater
            commands::get_app_version,
            commands::check_for_updates,
            commands::install_update,
            // Background tasks
            commands::list_background_tasks,
            // Integration credentials
            commands::store_integration_credential,
            commands::delete_integration_credential,
            commands::list_integration_credentials,
            commands::test_integration_credential,
            // DAG workflows
            commands::dag_run_create,
            commands::dag_run_from_template,
            commands::dag_run_status,
            commands::dag_run_cancel,
            commands::dag_task_add,
            commands::dag_run_list,
            commands::dag_run_history,
            commands::dag_template_save,
            commands::dag_template_list,
            commands::dag_template_delete,
            // API Key Rotation
            commands::add_api_key,
            commands::get_active_api_key,
            commands::activate_api_key,
            commands::rotate_api_key,
            commands::disable_api_key,
            commands::list_api_keys,
            commands::get_key_rotation_status,
            commands::set_rotation_schedule,
            commands::get_key_audit_log,
            commands::check_key_expiry_warnings,
            // Family Mode / Multi-User
            commands::create_family,
            commands::get_family,
            commands::list_user_families,
            commands::send_family_invitation,
            commands::accept_family_invitation,
            commands::remove_family_user,
            commands::update_family_user_role,
            commands::get_pending_invitations,
            commands::create_shared_memory_pool,
            commands::get_family_activity,
            commands::get_user_activity,
            commands::log_family_activity,
            // Database Maintenance & Backup
            commands::create_backup,
            commands::restore_backup,
            commands::list_backups,
            commands::delete_backup,
            commands::verify_backup,
            commands::perform_health_check,
            commands::optimize_database,
            commands::get_maintenance_config,
            commands::update_maintenance_config,
            commands::get_backup_stats,
            // Dashboard & Monitoring
            commands::get_dashboard_data,
            commands::update_system_metrics,
            commands::update_service_health,
            commands::add_dashboard_alert,
            commands::record_message_throughput,
            commands::record_api_latency,
            commands::record_error_rate,
            commands::get_historical_metrics,
            commands::get_service_health,
            commands::clear_old_alerts,
            // Advanced Memory Features
            commands::add_advanced_memory,
            commands::link_memories,
            commands::find_similar_memories,
            commands::verify_memory,
            commands::set_memory_importance,
            commands::get_related_memories,
            commands::cleanup_expired_memories,
            commands::get_memory_analytics,
            commands::export_memories,
            commands::search_advanced_memories,
            // Logging
            commands::get_logs,
            commands::get_log_level,
            commands::set_log_level,
            commands::clear_logs,
            commands::export_logs,
            commands::get_log_dir,
            // Smart Key Vault
            commands::list_vault_entries,
            commands::revoke_vault_entry,
            commands::label_vault_entry,
            commands::test_vault_resolve,
            // NexiGate gated shell
            commands::set_gated_shell_enabled,
            commands::set_gated_shell_debug,
            commands::set_gated_shell_record,
            commands::get_gated_shell_status,
            commands::list_shell_sessions,
            commands::get_shell_audit_log,
            commands::close_shell_session,
            commands::open_shell_viewer,
            // NexiGate plugin key management (UI-only, never LLM tools)
            commands::generate_plugin_signing_key,
            commands::sign_plugin_file,
            // NexiGate tmux interactive agent bridge (UI + LLM tool)
            commands::start_interactive_session,
            commands::send_to_interactive_session,
            commands::read_interactive_session,
            commands::wait_for_interactive_session,
            commands::stop_interactive_session,
            commands::list_interactive_sessions,
            // Yolo mode
            commands::request_yolo_mode,
            commands::approve_yolo_mode,
            commands::revoke_yolo_mode,
            commands::get_yolo_status,
            // KN Central Management policy
            commands::get_managed_policy_status,
            commands::force_policy_refresh,
            commands::get_tier_capabilities,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
