//! Chat and K2K search commands

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::{Emitter, Manager, State};
use tracing::{debug, error, info, warn};

use crate::bluebubbles::BlueBubblesObserver;
use crate::browser::BrowserManager;
use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::computer_use::ComputerUseManager;
use crate::config::AutonomyLevel;
use crate::discord::DiscordObserver;
use crate::google_chat::GoogleChatObserver;
use crate::guardrails::{Guardrails, ToolCheckResult};
use crate::instagram::InstagramObserver;
use crate::line::LineObserver;
use crate::mastodon::MastodonObserver;
use crate::matrix::MatrixObserver;
use crate::mattermost::MattermostObserver;
use crate::messenger::MessengerObserver;
use crate::rocketchat::RocketChatObserver;
use crate::router::{self, IncomingMessage, RouteOptions, RouterError};
use crate::security::exec_approval::ApprovalMode;
use crate::session_overrides::SessionOverrides;
use crate::signal::SignalObserver;
use crate::slack::SlackObserver;
use crate::teams::TeamsObserver;
use crate::tool_loop::{self, ToolLoopConfig};
use crate::twilio::TwilioObserver;
use crate::webchat::WebChatObserver;
use crate::whatsapp::WhatsAppObserver;

use super::execute_tool;
use super::fetch_tool;
use super::filesystem_tool;
use super::interactive_tool;
use super::memory_tool;
use super::search_tool;
use super::settings_tool;
use super::soul_tool;
use super::AppState;

// ── Cancel flag for active tool loop ────────────────────────────────────────

static ACTIVE_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Returns true if the frontend has requested cancellation of the current tool loop.
pub(crate) fn is_cancel_requested() -> bool {
    ACTIVE_CANCEL.load(std::sync::atomic::Ordering::Relaxed)
}

/// Clear the cancel flag before starting a new message.
fn clear_cancel_flag() {
    ACTIVE_CANCEL.store(false, std::sync::atomic::Ordering::Relaxed);
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub message: String,
    pub use_streaming: bool,
    /// Optional agent ID for multi-agent routing.
    /// If provided, routes through that agent's Claude client.
    /// If not, uses the active GUI agent.
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub response: String,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct K2KSearchRequest {
    pub query: String,
    pub top_k: Option<usize>,
    pub use_federated: bool,
    /// Optional rich context forwarded to the KB backend for better result weighting.
    /// Should include: conversation topic summary, triggering tool name, whether this
    /// is background enrichment or user-initiated, and confidence threshold hint.
    #[serde(default)]
    pub context: Option<String>,
    /// Whether this query is background enrichment (true) or user-initiated (false).
    #[serde(default)]
    pub is_background: bool,
    /// Confidence threshold 0.0–1.0. If already met the query may be skipped.
    #[serde(default)]
    pub confidence_threshold: Option<f32>,
    /// Name of the tool that triggered this KB query, if any.
    #[serde(default)]
    pub triggering_tool: Option<String>,
    /// Summary of the last N messages from the active conversation.
    #[serde(default)]
    pub conversation_summary: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct K2KSearchResponse {
    pub results: Vec<K2KResult>,
    pub total_results: usize,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct K2KResult {
    pub title: String,
    pub summary: String,
    pub content: String,
    pub confidence: f32,
    pub source_type: String,
}

/// Toast notification event payload for the frontend.
#[derive(Debug, Clone, Serialize)]
struct ToastEvent {
    level: String,
    title: String,
    message: String,
}

/// Collect all tools (MCP + Computer Use + Browser + built-in settings + built-in memory + soul)
pub(crate) async fn collect_all_tools(
    state: &AppState,
    query: Option<&str>,
) -> (Vec<serde_json::Value>, usize, bool, bool) {
    let mcp_tools = {
        let mcp = state.mcp_manager.read().await;
        match query {
            Some(q) => mcp.get_filtered_tools(q),
            None => mcp.get_all_tools(),
        }
    };

    let mut all_tools = mcp_tools.clone();
    let computer_use_enabled = {
        let cu = state.computer_use.read().await;
        // NOTE: The computer_* tool type has been removed from the Anthropic API.
        // Do not include the tool definition until a valid replacement type is available.
        // The execution logic in computer_use.rs is retained for future use.
        if cu.enabled {
            warn!("[TOOLS] Computer Use enabled in config but tool type is deprecated in API — skipping tool definition");
        }
        false
    };

    // Autonomous mode is authoritative: if it says Autonomous for a capability,
    // the tool is registered even if the base config has enabled=false.
    let auto = {
        let config = state.config.read().await;
        if config.autonomous_mode.enabled {
            Some(config.autonomous_mode.clone())
        } else {
            None
        }
    };

    let browser_enabled = {
        let browser = state.browser.read().await;
        let config_enabled = state.config.read().await.browser.enabled;
        let auto_browser = auto.as_ref().is_some_and(|a| {
            a.browser.navigate == AutonomyLevel::Autonomous
                || a.browser.interact == AutonomyLevel::Autonomous
        });
        if browser.enabled || config_enabled || auto_browser {
            all_tools.push(browser.get_tool_definition());
            true
        } else {
            false
        }
    };

    // Add built-in tools
    all_tools.push(settings_tool::nexibot_settings_tool_definition());
    all_tools.push(memory_tool::nexibot_memory_tool_definition());
    all_tools.push(soul_tool::nexibot_soul_tool_definition());

    // Add foundation tools (web search, fetch, filesystem, execute)
    all_tools.push(search_tool::nexibot_web_search_tool_definition());

    {
        let config = state.config.read().await;
        let auto_fetch = auto.as_ref().is_some_and(|a| {
            a.fetch.get_requests == AutonomyLevel::Autonomous
                || a.fetch.post_requests == AutonomyLevel::Autonomous
        });
        let auto_fs = auto.as_ref().is_some_and(|a| {
            a.filesystem.read == AutonomyLevel::Autonomous
                || a.filesystem.write == AutonomyLevel::Autonomous
        });
        let auto_exec = auto.as_ref().is_some_and(|a| {
            a.execute.run_command == AutonomyLevel::Autonomous
                || a.execute.run_python == AutonomyLevel::Autonomous
                || a.execute.run_node == AutonomyLevel::Autonomous
        });
        if config.fetch.enabled || auto_fetch {
            all_tools.push(fetch_tool::nexibot_fetch_tool_definition());
        }
        if config.filesystem.enabled || auto_fs {
            all_tools.push(filesystem_tool::nexibot_filesystem_tool_definition());
        }
        if config.execute.enabled || auto_exec {
            all_tools.push(execute_tool::nexibot_execute_tool_definition());
        }
    }

    // Background task tool for delegating long-running work
    all_tools.push(serde_json::json!({
        "name": "nexibot_background_task",
        "description": "Delegate a task to run in the background. Use when the user asks you to research, write documents, or do work that takes more than a few seconds. You speak the acknowledgment immediately, then the background task runs independently. If the user wants completion alerts, set notify_target accordingly.",
        "input_schema": {
            "type": "object",
            "properties": {
                "task_description": { "type": "string", "description": "What needs to be done (short, user-facing label)" },
                "spoken_acknowledgment": { "type": "string", "description": "Brief message to deliver to the user RIGHT NOW (1-2 sentences)" },
                "instructions": { "type": "string", "description": "Detailed instructions for the background worker" },
                "notify_target": {
                    "type": "object",
                    "description": "Where to send a completion notification. Omit to skip notification. Specific targets must be present in the channel allowlist/admin list or they are blocked. For type=telegram, chat_id is required; use type=telegram_configured to broadcast to configured Telegram chats. type=all_configured currently broadcasts only to telegram, discord, slack, whatsapp, signal, matrix, mattermost, google_chat, messenger, instagram, line, twilio, and gui when those targets are configured. It does not currently deliver to Microsoft Teams, Mastodon, Rocket.Chat, WebChat, or Email. Examples: {\"type\":\"all_configured\"}, {\"type\":\"telegram_configured\"}, {\"type\":\"telegram\",\"chat_id\":123456}, {\"type\":\"discord\",\"channel_id\":987654}, {\"type\":\"slack\",\"channel_id\":\"C123ABC\"}, {\"type\":\"whatsapp\",\"phone_number\":\"15551234567\"}, {\"type\":\"signal\",\"phone_number\":\"+15551234567\"}, {\"type\":\"matrix\",\"room_id\":\"!roomid:matrix.org\"}, {\"type\":\"mattermost\",\"channel_id\":\"channel-id\"}, {\"type\":\"google_chat\"}, {\"type\":\"bluebubbles\",\"chat_guid\":\"iMessage;-;+15551234567\"}, {\"type\":\"messenger\",\"recipient_id\":\"1234567890\"}, {\"type\":\"instagram\",\"recipient_id\":\"17841400000000000\"}, {\"type\":\"line\",\"user_id\":\"U123...\"}, {\"type\":\"twilio\",\"phone_number\":\"+15551234567\"}, or {\"type\":\"gui\"}.",
                    "properties": {
                        "type": { "type": "string", "enum": ["telegram", "telegram_configured", "discord", "slack", "whatsapp", "signal", "matrix", "mattermost", "google_chat", "bluebubbles", "messenger", "instagram", "line", "twilio", "gui", "all_configured"] },
                        "chat_id": { "type": "integer", "description": "Telegram chat_id (for type=telegram)" },
                        "channel_id": { "description": "Discord channel_id (integer), Slack channel_id (string), or Mattermost channel_id (string)"},
                        "phone_number": { "type": "string", "description": "Recipient phone number (for type=whatsapp, type=signal, or type=twilio)" },
                        "room_id": { "type": "string", "description": "Matrix room ID (for type=matrix)" },
                        "chat_guid": { "type": "string", "description": "BlueBubbles chat GUID (for type=bluebubbles)" },
                        "recipient_id": { "type": "string", "description": "Recipient ID (for type=messenger or type=instagram)" },
                        "user_id": { "type": "string", "description": "LINE user ID (for type=line)" }
                    },
                    "required": ["type"]
                }
            },
            "required": ["task_description", "spoken_acknowledgment", "instructions"]
        }
    }));

    // Canvas tool for pushing artifacts to the workspace panel
    all_tools.push(serde_json::json!({
        "name": "canvas_push",
        "description": "Push an artifact (code, HTML page, SVG image, or Mermaid diagram) to the user's Canvas workspace panel for interactive viewing. Use this when generating substantial code, visualizations, or documents that benefit from a dedicated viewer rather than inline display.",
        "input_schema": {
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "enum": ["code", "html", "svg", "mermaid"],
                    "description": "The type of artifact to display"
                },
                "content": {
                    "type": "string",
                    "description": "The artifact content (source code, HTML, SVG markup, or Mermaid diagram code)"
                },
                "title": {
                    "type": "string",
                    "description": "A short title for the artifact tab"
                },
                "language": {
                    "type": "string",
                    "description": "Programming language for syntax highlighting (only for type='code'). E.g. 'python', 'rust', 'javascript'"
                }
            },
            "required": ["type", "content", "title"]
        }
    }));

    // Yolo mode request tool — model can request, never approve
    {
        let cfg = state.config.read().await;
        if cfg.yolo_mode.allow_model_request {
            all_tools.push(serde_json::json!({
                "name": "nexibot_request_yolo_mode",
                "description": "Request time-limited elevated access (yolo mode) for a privileged operation such as writing config, installing packages, or direct file edits outside normal sandboxing. A human must approve via the desktop UI or Telegram before yolo mode activates — you cannot approve it yourself. Use only when a specific operation requires it; explain exactly why.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "reason": {
                            "type": "string",
                            "description": "Explain exactly what you need to do and why it requires elevated access. Shown verbatim to the user in the approval prompt."
                        },
                        "duration_secs": {
                            "type": "integer",
                            "description": "How many seconds yolo mode should stay active. Omit to use the configured default. Prefer the shortest duration needed."
                        }
                    },
                    "required": ["reason"]
                }
            }));
        }
    }

    // Orchestration tools (only when multiple agents are configured)
    {
        let agent_mgr = state.agent_manager.read().await;
        let agents = agent_mgr.list_agents();
        if agents.len() > 1 {
            all_tools.push(crate::orchestration::nexibot_orchestrate_tool_definition());
            all_tools.push(crate::orchestration::nexibot_dag_run_tool_definition());

            // Shared workspace tools for inter-agent data sharing
            all_tools.push(crate::shared_workspace::nexibot_workspace_read_tool_definition());
            all_tools.push(crate::shared_workspace::nexibot_workspace_write_tool_definition());
        }
    }

    // Interactive agent tool — tmux bridge (only when enabled)
    {
        let enabled = state
            .gated_shell
            .as_ref()
            .map(|gs| gs.tmux_bridge.is_enabled())
            .unwrap_or(false);
        if enabled {
            all_tools.push(interactive_tool::nexibot_interactive_agent_tool_definition());
        }
    }

    // Session management tools for inter-agent messaging
    all_tools.push(serde_json::json!({
        "name": "sessions_list",
        "description": "List all named sessions for inter-agent messaging.",
        "input_schema": {
            "type": "object",
            "properties": {}
        }
    }));
    all_tools.push(serde_json::json!({
        "name": "sessions_send",
        "description": "Send a message to another named session.",
        "input_schema": {
            "type": "object",
            "properties": {
                "to": { "type": "string", "description": "Target session name or ID" },
                "content": { "type": "string", "description": "Message content to send" }
            },
            "required": ["to", "content"]
        }
    }));
    all_tools.push(serde_json::json!({
        "name": "sessions_create",
        "description": "Create a new named session for isolating a conversation or delegating a task.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Name for the new session" }
            },
            "required": ["name"]
        }
    }));

    // Add skill tools — skills with command_dispatch="script" become callable LLM tools.
    // Skills without command_dispatch remain prompt-only (injected via get_skills_context()).
    {
        let skills_manager = state.skills_manager.read().await;
        for skill in skills_manager.list_skills() {
            let dispatch = match skill.metadata.command_dispatch.as_deref() {
                Some(d) => d,
                None => continue,
            };
            if dispatch != "script" || skill.scripts.is_empty() {
                continue;
            }
            // Tool names: Anthropic allows [a-zA-Z0-9_-]; map skill id accordingly.
            let tool_name = format!(
                "skill_{}",
                skill
                    .id
                    .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_")
            );
            let description = format!(
                "{}{}",
                skill
                    .metadata
                    .description
                    .as_deref()
                    .unwrap_or(skill.metadata.name.as_deref().unwrap_or(&skill.id)),
                if skill.scripts.len() == 1 {
                    format!(" (script: {})", skill.scripts[0])
                } else {
                    format!(" (scripts: {})", skill.scripts.join(", "))
                }
            );
            let script_enum: Vec<serde_json::Value> = skill
                .scripts
                .iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect();
            all_tools.push(serde_json::json!({
                "name": tool_name,
                "description": description,
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "script": {
                            "type": "string",
                            "enum": script_enum,
                            "description": "Which script to run"
                        },
                        "input": {
                            "type": "string",
                            "description": "Text to pass to the script on stdin (optional)"
                        }
                    },
                    "required": ["script"]
                }
            }));
        }
    }

    let mcp_count = mcp_tools.len();
    (all_tools, mcp_count, computer_use_enabled, browser_enabled)
}

/// Emit a toast notification to the frontend (if window is available).
pub(crate) fn emit_toast(window: Option<&tauri::Window>, level: &str, title: &str, message: &str) {
    if let Some(w) = window {
        if let Err(e) = w.emit(
            "notify:toast",
            ToastEvent {
                level: level.to_string(),
                title: title.to_string(),
                message: message.to_string(),
            },
        ) {
            warn!(
                "[UI] Failed to emit notify:toast (level={} title={}): {}",
                level, title, e
            );
        }
    }
}

async fn request_observer_confirmation<'obs>(
    tool_name: &str,
    reason: &str,
    details: Option<&str>,
    observer: Option<&'obs (dyn crate::tool_loop::ToolLoopObserver + 'obs)>,
) -> bool {
    if let Some(obs) = observer {
        if !obs.supports_approval() {
            return false;
        }
        let normalized = strip_confirmation_prefix(reason);
        obs.request_approval_with_details(tool_name, normalized, details).await
    } else {
        false
    }
}

async fn require_tool_approval_or_block<'obs>(
    tool_name: &str,
    reason: &str,
    details: Option<&str>,
    observer: Option<&'obs (dyn crate::tool_loop::ToolLoopObserver + 'obs)>,
) -> Result<(), String> {
    if request_observer_confirmation(tool_name, reason, details, observer).await {
        return Ok(());
    }

    let normalized_reason = strip_confirmation_prefix(reason);
    let detail = if observer.map(|o| o.supports_approval()).unwrap_or(false) {
        normalized_reason.to_string()
    } else {
        missing_approval_channel_reason(normalized_reason)
    };

    Err(format!("BLOCKED: requires user confirmation - {}", detail))
}

fn strip_confirmation_prefix(reason: &str) -> &str {
    reason
        .strip_prefix("NEEDS_CONFIRMATION:")
        .unwrap_or(reason)
        .trim()
}

fn missing_approval_channel_reason(reason: &str) -> String {
    format!(
        "{} (no in-channel approval path is available on this source; use desktop chat or retry from a channel that supports in-channel approve/deny replies)",
        strip_confirmation_prefix(reason)
    )
}

async fn execute_browser_tool<'obs>(
    tool_name: &str,
    tool_input: &serde_json::Value,
    state: &AppState,
    window: Option<&tauri::Window>,
    observer: Option<&'obs (dyn crate::tool_loop::ToolLoopObserver + 'obs)>,
) -> String {
    let attempt = {
        let browser = state.browser.read().await;
        browser.execute(tool_input).await
    };

    match attempt {
        Ok(output) => serde_json::to_string(&output).unwrap_or_default(),
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.starts_with("NEEDS_CONFIRMATION:") {
                let display_reason = strip_confirmation_prefix(&err_msg).to_string();
                warn!("[BROWSER] {}", err_msg);
                emit_toast(window, "warning", "Confirmation Required", &display_reason);

                if request_observer_confirmation(tool_name, &err_msg, None, observer).await {
                    info!(
                        "[BROWSER] Tool '{}' approved by observer, retrying execution",
                        tool_name
                    );
                    let retry = {
                        let browser = state.browser.read().await;
                        browser.execute(tool_input).await
                    };
                    match retry {
                        Ok(output) => serde_json::to_string(&output).unwrap_or_default(),
                        Err(e2) => {
                            let retry_msg = e2.to_string();
                            if retry_msg.starts_with("NEEDS_CONFIRMATION:") {
                                let retry_reason =
                                    strip_confirmation_prefix(&retry_msg).to_string();
                                warn!("[BROWSER] {}", retry_msg);
                                emit_toast(
                                    window,
                                    "warning",
                                    "Confirmation Required",
                                    &retry_reason,
                                );
                                format!("BLOCKED: requires user confirmation - {}", retry_reason)
                            } else if retry_msg.starts_with("BLOCKED:") {
                                warn!("[BROWSER] {}", retry_msg);
                                emit_toast(window, "warning", "Browser Blocked", &retry_msg);
                                retry_msg
                            } else {
                                warn!("[BROWSER] Tool failed after approval: {}", e2);
                                format!("Error: {}", e2)
                            }
                        }
                    }
                } else {
                    let reason = if observer.map(|o| o.supports_approval()).unwrap_or(false) {
                        display_reason
                    } else {
                        missing_approval_channel_reason(&err_msg)
                    };
                    format!("BLOCKED: requires user confirmation - {}", reason)
                }
            } else if err_msg.starts_with("BLOCKED:") {
                warn!("[BROWSER] {}", err_msg);
                emit_toast(window, "warning", "Browser Blocked", &err_msg);
                err_msg
            } else {
                warn!("[BROWSER] Tool failed: {}", e);
                format!("Error: {}", e)
            }
        }
    }
}

async fn execute_computer_use_tool<'obs>(
    tool_name: &str,
    tool_input: &serde_json::Value,
    state: &AppState,
    window: Option<&tauri::Window>,
    observer: Option<&'obs (dyn crate::tool_loop::ToolLoopObserver + 'obs)>,
) -> String {
    let attempt = {
        let cu = state.computer_use.read().await;
        cu.execute(tool_input).await
    };

    match attempt {
        Ok(output) => serde_json::to_string(&output).unwrap_or_default(),
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.starts_with("NEEDS_CONFIRMATION:") {
                let display_reason = strip_confirmation_prefix(&err_msg).to_string();
                warn!("[COMPUTER_USE] {}", err_msg);
                emit_toast(window, "warning", "Confirmation Required", &display_reason);

                if request_observer_confirmation(tool_name, &err_msg, None, observer).await {
                    info!(
                        "[COMPUTER_USE] Tool '{}' approved by observer, retrying execution",
                        tool_name
                    );
                    let retry = {
                        let cu = state.computer_use.read().await;
                        cu.execute_with_approval(tool_input, true).await
                    };
                    match retry {
                        Ok(output) => serde_json::to_string(&output).unwrap_or_default(),
                        Err(e2) => {
                            let retry_msg = e2.to_string();
                            if retry_msg.starts_with("NEEDS_CONFIRMATION:") {
                                let retry_reason =
                                    strip_confirmation_prefix(&retry_msg).to_string();
                                warn!("[COMPUTER_USE] {}", retry_msg);
                                emit_toast(
                                    window,
                                    "warning",
                                    "Confirmation Required",
                                    &retry_reason,
                                );
                                format!("BLOCKED: requires user confirmation - {}", retry_reason)
                            } else {
                                warn!("[COMPUTER_USE] Tool failed after approval: {}", e2);
                                format!("Error: {}", e2)
                            }
                        }
                    }
                } else {
                    let reason = if observer.map(|o| o.supports_approval()).unwrap_or(false) {
                        display_reason
                    } else {
                        missing_approval_channel_reason(&err_msg)
                    };
                    format!("BLOCKED: requires user confirmation - {}", reason)
                }
            } else {
                warn!("[COMPUTER_USE] Tool failed: {}", e);
                format!("Error: {}", e)
            }
        }
    }
}

/// Execute a single tool call, handling built-in tools, guardrails, and MCP routing.
/// Returns the tool result string.
pub(crate) async fn execute_tool_call<'obs>(
    tool_name: &str,
    _tool_id: &str,
    tool_input: &serde_json::Value,
    state: &AppState,
    window: Option<&tauri::Window>,
    session_key: &str,
    agent_id: &str,
    observer: Option<&'obs (dyn crate::tool_loop::ToolLoopObserver + 'obs)>,
    source_channel: Option<&ChannelSource>,
    source_sender_id: Option<&str>,
) -> String {
    // Check yolo mode once — active yolo bypasses all AskUser approval gates.
    let yolo_active = state.yolo_manager.is_active().await;

    // Handle built-in nexibot_settings tool
    if tool_name == "nexibot_settings" {
        // Hard guard check (non-bypassable)
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        // Autonomous mode check
        {
            let config = state.config.read().await;
            if config.autonomous_mode.enabled {
                let level = config.autonomous_mode.settings_modification.level;
                drop(config);
                match level {
                    AutonomyLevel::Blocked if !yolo_active => {
                        return "BLOCKED: Settings modification is disabled in autonomous mode."
                            .to_string()
                    }
                    AutonomyLevel::AskUser if !yolo_active => {
                        if let Err(blocked) = require_tool_approval_or_block(
                            tool_name,
                            "Settings modification requires user confirmation (autonomous mode AskUser).",
                            None,
                            observer,
                        )
                        .await
                        {
                            return blocked;
                        }
                    }
                    _ => {} // Autonomous, or yolo bypasses Blocked/AskUser
                }
            }
        }
        let action = tool_input
            .get("action")
            .and_then(|a| a.as_str())
            .unwrap_or("");
        let mut config = state.config.write().await;
        let (result, persisted_change) =
            settings_tool::execute_settings_tool(tool_input, &mut config).await;
        drop(config);
        if action == "set" && persisted_change {
            if state.config_changed.send(()).is_err() {
                warn!("[CHAT] Failed to broadcast config change notification");
            }
        }
        return result;
    }

    // Handle built-in nexibot_memory tool
    if tool_name == "nexibot_memory" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        {
            let config = state.config.read().await;
            if config.autonomous_mode.enabled {
                let level = config.autonomous_mode.memory_modification.level;
                drop(config);
                match level {
                    AutonomyLevel::Blocked if !yolo_active => {
                        return "BLOCKED: Memory modification is disabled in autonomous mode."
                            .to_string()
                    }
                    AutonomyLevel::AskUser if !yolo_active => {
                        if let Err(blocked) = require_tool_approval_or_block(
                            tool_name,
                            "Memory modification requires user confirmation (autonomous mode AskUser).",
                            None,
                            observer,
                        )
                        .await
                        {
                            return blocked;
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut memory_manager = state.memory_manager.write().await;
        return memory_tool::execute_memory_tool(tool_input, &mut memory_manager);
    }

    // Handle built-in nexibot_soul tool
    if tool_name == "nexibot_soul" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        {
            let config = state.config.read().await;
            if config.autonomous_mode.enabled {
                let level = config.autonomous_mode.soul_modification.level;
                drop(config);
                match level {
                    AutonomyLevel::Blocked if !yolo_active => {
                        return "BLOCKED: Soul modification is disabled in autonomous mode."
                            .to_string()
                    }
                    AutonomyLevel::AskUser if !yolo_active => {
                        if let Err(blocked) = require_tool_approval_or_block(
                            tool_name,
                            "Soul modification requires user confirmation (autonomous mode AskUser).",
                            None,
                            observer,
                        )
                        .await
                        {
                            return blocked;
                        }
                    }
                    _ => {}
                }
            }
        }
        return soul_tool::execute_soul_tool(tool_input);
    }

    // Handle built-in nexibot_web_search tool
    // Lock ordering: config (1) then browser (7)
    if tool_name == "nexibot_web_search" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        let config = state.config.read().await;
        let config_snapshot = config.clone();
        drop(config);
        let browser = state.browser.read().await;
        return search_tool::execute_web_search_tool(tool_input, &config_snapshot, &browser).await;
    }

    // Handle built-in nexibot_fetch tool
    if tool_name == "nexibot_fetch" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        {
            let config = state.config.read().await;
            if config.autonomous_mode.enabled {
                let method = tool_input
                    .get("method")
                    .and_then(|m| m.as_str())
                    .unwrap_or("GET");
                let level = match method.to_uppercase().as_str() {
                    "GET" | "HEAD" | "OPTIONS" => config.autonomous_mode.fetch.get_requests,
                    _ => config.autonomous_mode.fetch.post_requests,
                };
                drop(config);
                let url = tool_input.get("url").and_then(|u| u.as_str()).unwrap_or("");
                let fetch_details = format!("{} {}", method.to_uppercase(), url);
                match level {
                    AutonomyLevel::Blocked if !yolo_active => {
                        return format!(
                            "BLOCKED: Fetch '{}' requests are disabled in autonomous mode.",
                            method
                        )
                    }
                    AutonomyLevel::AskUser if !yolo_active => {
                        let reason = format!(
                            "Fetch '{}' requests require user confirmation (autonomous mode AskUser).",
                            method
                        );
                        let detail = (!url.is_empty()).then_some(fetch_details.as_str());
                        if let Err(blocked) =
                            require_tool_approval_or_block(tool_name, &reason, detail, observer).await
                        {
                            return blocked;
                        }
                    }
                    _ => {}
                }
            }
        }
        let config = state.config.read().await;
        return fetch_tool::execute_fetch_tool(tool_input, &config.fetch).await;
    }

    // Handle built-in nexibot_filesystem tool
    if tool_name == "nexibot_filesystem" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        {
            let config = state.config.read().await;
            if config.autonomous_mode.enabled {
                let action = tool_input
                    .get("action")
                    .and_then(|a| a.as_str())
                    .unwrap_or("");
                let path = tool_input.get("path").and_then(|p| p.as_str()).unwrap_or("");
                let fs_details: Option<String> = if path.is_empty() {
                    None
                } else {
                    Some(format!("{}: {}", action, path))
                };
                let level = match action {
                    "read_file" | "file_info" | "list_directory" => {
                        config.autonomous_mode.filesystem.read
                    }
                    "write_file" | "create_directory" => config.autonomous_mode.filesystem.write,
                    "delete_file" => config.autonomous_mode.filesystem.delete,
                    _ => AutonomyLevel::AskUser,
                };
                drop(config);
                match level {
                    AutonomyLevel::Blocked if !yolo_active => {
                        return format!(
                            "BLOCKED: Filesystem '{}' is disabled in autonomous mode.",
                            action
                        )
                    }
                    AutonomyLevel::AskUser if !yolo_active => {
                        let reason = format!(
                            "Filesystem '{}' requires user confirmation (autonomous mode AskUser).",
                            action
                        );
                        if let Err(blocked) =
                            require_tool_approval_or_block(tool_name, &reason, fs_details.as_deref(), observer).await
                        {
                            return blocked;
                        }
                    }
                    _ => {}
                }
            }
        }
        let config = state.config.read().await;
        return filesystem_tool::execute_filesystem_tool(tool_input, &config.filesystem);
    }

    // Handle built-in nexibot_background_task tool
    if tool_name == "nexibot_background_task" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        let desc = tool_input["task_description"]
            .as_str()
            .unwrap_or("Background task");
        let instructions = tool_input["instructions"].as_str().unwrap_or("");
        let acknowledgment = tool_input["spoken_acknowledgment"]
            .as_str()
            .unwrap_or("Got it, working on it in the background.")
            .to_string();
        // Parse notification target from the structured notify_target field.
        // Also accepts legacy notify_telegram=true for backwards compatibility.
        let notify_target = parse_notify_target(tool_input);

        let mut tm = state.task_manager.write().await;
        let task_id = tm.create_task(desc, notify_target);
        drop(tm);

        // Spawn background worker
        let task_id_clone = task_id.clone();
        let instructions = instructions.to_string();
        let state_clone = state.clone();
        let handle = window.map(|w| w.app_handle().clone());

        // Use spawn_background_task to work around Send bounds on execute_tool_call
        spawn_background_task(
            task_id_clone,
            instructions,
            state_clone,
            handle,
            source_channel.cloned(),
            source_sender_id.map(ToOwned::to_owned),
            agent_id.to_string(),
        );

        // Return a detach sentinel so the tool loop can short-circuit.
        // Format: "NEXIBOT_DETACH:{task_id}:{acknowledgment}"
        // The tool loop strips the sentinel, uses the acknowledgment as the final
        // text response, and skips the continue_after_tools round-trip.
        return format!("NEXIBOT_DETACH:{}:{}", task_id, acknowledgment);
    }

    // Handle canvas_push tool — emit event to frontend
    if tool_name == "canvas_push" {
        if let Some(w) = window {
            if let Err(e) = w.emit("canvas:push", tool_input.clone()) {
                warn!("[CHAT] Failed to emit canvas:push: {}", e);
            }
        }
        let title = tool_input
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Artifact");
        return format!("Artifact '{}' pushed to canvas.", title);
    }

    // Handle nexibot_orchestrate tool
    if tool_name == "nexibot_orchestrate" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        return super::agent_cmds::execute_orchestrate_tool(tool_input, state).await;
    }

    // Handle nexibot_dag_run tool (persistent DAG execution)
    if tool_name == "nexibot_dag_run" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        let name = tool_input
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled DAG");
        let tasks: Vec<crate::dag::DagTaskDefinition> = match tool_input.get("tasks") {
            Some(t) => match serde_json::from_value(t.clone()) {
                Ok(tasks) => tasks,
                Err(e) => return format!("Error parsing DAG tasks: {}", e),
            },
            None => return "Error: 'tasks' field is required".to_string(),
        };

        let run_id = {
            let store = state.dag_store.lock().unwrap_or_else(|e| e.into_inner());
            match store.create_run(name, None, &tasks) {
                Ok(id) => id,
                Err(e) => return format!("Error creating DAG run: {}", e),
            }
        };

        // Start execution (need app_handle — try to get from window)
        if let Some(w) = window {
            state
                .dag_executor
                .start_run(run_id.clone(), state.clone(), w.app_handle().clone());
            return format!(
                "DAG run '{}' started with ID: {}. {} tasks will execute in dependency order. \
                 Listen for 'dag:*' events for progress updates.",
                name,
                run_id,
                tasks.len()
            );
        } else {
            return format!(
                "DAG run '{}' created with ID: {} but could not start (no app handle). \
                 Use the dag_run_create Tauri command instead.",
                name, run_id
            );
        }
    }

    // Handle shared workspace tools
    if tool_name == "nexibot_workspace_read" {
        let ws = state.shared_workspace.read().await;
        // Use a default orchestration ID for direct tool calls (not from orchestration)
        return crate::shared_workspace::execute_workspace_read(&ws, "default", tool_input);
    }
    if tool_name == "nexibot_workspace_write" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        let mut ws = state.shared_workspace.write().await;
        return crate::shared_workspace::execute_workspace_write(
            &mut ws, "default", "user", tool_input,
        );
    }

    // Handle session management tools
    if tool_name == "sessions_list" {
        let mgr = state.session_manager.read().await;
        let sessions = mgr.list_sessions();
        return serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_string());
    }
    if tool_name == "sessions_send" {
        let to = tool_input.get("to").and_then(|v| v.as_str()).unwrap_or("");
        let content = tool_input
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mut mgr = state.session_manager.write().await;
        return match mgr.send_to_session("current", to, content) {
            Ok(()) => format!("Message sent to session '{}'.", to),
            Err(e) => format!("Failed to send message: {}", e),
        };
    }
    if tool_name == "sessions_create" {
        let name = tool_input
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unnamed");
        let mut mgr = state.session_manager.write().await;
        return match mgr.create_session(name) {
            Ok(session) => serde_json::to_string(&session).unwrap_or_else(|_| "{}".to_string()),
            Err(e) => format!("Failed to create session: {}", e),
        };
    }

    // Handle built-in nexibot_execute tool
    // Lock ordering: config (1) then guardrails (2)
    if tool_name == "nexibot_execute" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        // Autonomous mode gate: check if execution is allowed, and track whether
        // the approval manager should be bypassed (Autonomous only).
        let mut skip_exec_approval = false;
        let mut explicit_user_approved = false;
        {
            let config = state.config.read().await;
            if config.autonomous_mode.enabled {
                let action = tool_input
                    .get("action")
                    .and_then(|r| r.as_str())
                    .unwrap_or("run_command");
                let exec_command = tool_input
                    .get("command")
                    .or_else(|| tool_input.get("script"))
                    .or_else(|| tool_input.get("code"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let level = match action {
                    "run_python" => config.autonomous_mode.execute.run_python,
                    "run_node" => config.autonomous_mode.execute.run_node,
                    _ => config.autonomous_mode.execute.run_command,
                };
                drop(config);
                match level {
                    AutonomyLevel::Blocked if !yolo_active => {
                        return format!(
                            "BLOCKED: Execution of '{}' is disabled in autonomous mode.",
                            action
                        )
                    }
                    AutonomyLevel::Blocked | AutonomyLevel::Autonomous => {
                        skip_exec_approval = true;
                    }
                    AutonomyLevel::AskUser => {
                        if yolo_active {
                            skip_exec_approval = true;
                        } else {
                            let reason = format!(
                                "Execution of '{}' requires user confirmation (autonomous mode AskUser).",
                                action
                            );
                            if let Err(blocked) = require_tool_approval_or_block(
                                tool_name,
                                &reason,
                                (!exec_command.is_empty()).then_some(exec_command),
                                observer,
                            )
                            .await
                            {
                                return blocked;
                            }
                            explicit_user_approved = true;
                        }
                    }
                }
            }
        }
        let config = state.config.read().await;
        let mut exec_config = config.execute.clone();
        // Autonomous mode is authoritative: if it approved this call (we got past
        // the autonomy gate above), override the base enabled flag.
        if config.autonomous_mode.enabled {
            exec_config.enabled = true;
        }
        drop(config);

        // Continuity: ExecApproval Prompt mode must route through the same
        // channel/UI approval mechanism, not hard-deny in check_approval().
        let exec_approval_mode = {
            let approval_mgr = state.exec_approval_manager.read().await;
            approval_mgr.mode()
        };
        if !skip_exec_approval && !yolo_active && exec_approval_mode == ApprovalMode::Prompt && !explicit_user_approved
        {
            let action = tool_input
                .get("action")
                .and_then(|r| r.as_str())
                .unwrap_or("run_command");
            let prompt_command = tool_input
                .get("command")
                .or_else(|| tool_input.get("script"))
                .or_else(|| tool_input.get("code"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let reason = format!(
                "Execution of '{}' requires user confirmation (execution approval mode Prompt).",
                action
            );
            if let Err(blocked) = require_tool_approval_or_block(
                tool_name,
                &reason,
                (!prompt_command.is_empty()).then_some(prompt_command),
                observer,
            )
            .await
            {
                return blocked;
            }
        }

        let mut guardrails = state.guardrails.write().await;
        // When autonomous mode is fully autonomous, or execution approval mode
        // is Prompt (already approved above), or the user explicitly approved
        // this specific call via the AskUser gate, use a Full-mode manager so
        // the approval gate remains in the call path but passes the command.
        // Guardrails DCG and blocked_commands still provide safety checks.
        if skip_exec_approval || exec_approval_mode == ApprovalMode::Prompt || explicit_user_approved || yolo_active {
            let full_approval = crate::security::exec_approval::ExecApprovalManager::new(
                ApprovalMode::Full,
            );
            return execute_tool::execute_execute_tool(
                tool_input,
                &exec_config,
                &mut guardrails,
                Some(&full_approval),
                None,
                state.gated_shell.as_deref(),
                session_key,
                agent_id,
            )
            .await;
        }
        // Pre-check approval and route NEEDS_CONFIRMATION to GUI before calling execute
        let approval_mgr = state.exec_approval_manager.read().await;
        let command = tool_input
            .get("command")
            .or_else(|| tool_input.get("script"))
            .or_else(|| tool_input.get("code"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let action = tool_input
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("run_command");
        match approval_mgr.check_approval(command, action) {
            Ok(()) => {
                // Pre-approved (allowlisted) — skip approval manager in execute_tool
            }
            Err(msg) if msg.starts_with("NEEDS_CONFIRMATION:") => {
                drop(approval_mgr);
                // Route to GUI approval dialog
                let display_reason = strip_confirmation_prefix(&msg).to_string();
                if let Err(blocked) = require_tool_approval_or_block(
                    tool_name,
                    &display_reason,
                    (!command.is_empty()).then_some(command),
                    observer,
                )
                .await
                {
                    return blocked;
                }
                // User approved — proceed without approval manager gating
            }
            Err(msg) => {
                // Hard deny from approval manager
                warn!("[EXECUTE] Command blocked by approval system: {}", msg);
                return serde_json::json!({
                    "error": "Command blocked by execution approval policy",
                    "reason": msg,
                })
                .to_string();
            }
        }
        // If we get here, the command is either allowlisted or the user approved
        // it via the GUI dialog. Use a Full-mode manager so the approval gate
        // stays in the call path but permits the already-verified command.
        let full_approval =
            crate::security::exec_approval::ExecApprovalManager::new(ApprovalMode::Full);
        return execute_tool::execute_execute_tool(
            tool_input,
            &exec_config,
            &mut guardrails,
            Some(&full_approval),
            None,
            state.gated_shell.as_deref(),
            session_key,
            agent_id,
        )
        .await;
    }

    // Handle nexibot_interactive_agent tool (tmux bridge)
    if tool_name == "nexibot_interactive_agent" {
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            return format!("BLOCKED: {}", reason);
        }
        return interactive_tool::execute_interactive_agent_tool(tool_input, state).await;
    }

    // Handle skill tool calls (skill_* prefix, registered by collect_all_tools for
    // skills with command_dispatch="script").
    if tool_name.starts_with("skill_") {
        // Hard guard is non-bypassable and applies to all tool types including skills.
        if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
            warn!("[HARD_GUARD] Skill '{}' blocked: {}", tool_name, reason);
            return format!("BLOCKED by hard guard: {}", reason);
        }
        let skill_id_from_name = &tool_name["skill_".len()..];
        let script = match tool_input.get("script").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return "Error: 'script' field is required".to_string(),
        };
        let input = tool_input
            .get("input")
            .and_then(|v| v.as_str())
            .map(String::from);

        let skills_manager = state.skills_manager.read().await;

        // Resolve skill ID: exact match first, then with '_' → '-' substitution
        // (tool names replace '-' with '_' for API compatibility).
        let skill_id = if skills_manager.get_skill(skill_id_from_name).is_some() {
            skill_id_from_name.to_string()
        } else {
            let with_dash = skill_id_from_name.replace('_', "-");
            if skills_manager.get_skill(&with_dash).is_some() {
                with_dash
            } else {
                return format!("Skill '{}' not found", skill_id_from_name);
            }
        };

        info!(
            "[SKILLS] LLM invoking skill '{}', script '{}'",
            skill_id, script
        );
        return match skills_manager
            .execute_skill_script(&skill_id, &script, input.as_deref())
            .await
        {
            Ok(result) if result.success => result.stdout,
            Ok(result) => format!(
                "Script exited with code {}.\n{}",
                result.exit_code,
                if result.stdout.is_empty() {
                    "(no output)".to_string()
                } else {
                    result.stdout
                }
            ),
            Err(e) => format!("Skill execution failed: {}", e),
        };
    }

    // Check if browser tool should bypass guardrails (use_guardrails: false)
    let is_browser = BrowserManager::is_browser_tool(tool_name);
    if is_browser {
        let use_guardrails = {
            let browser = state.browser.read().await;
            browser.config.use_guardrails
        };
        if !use_guardrails {
            // Hard guard is non-bypassable even when browser guardrails are disabled.
            if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
                warn!("[HARD_GUARD] Browser tool '{}' blocked: {}", tool_name, reason);
                return format!("BLOCKED by hard guard: {}", reason);
            }
            // Browser with guardrails disabled — execute directly
            return execute_browser_tool(tool_name, tool_input, state, window, observer).await;
        }
    }

    // Yolo mode request — model-facing, never auto-approved
    if tool_name == "nexibot_request_yolo_mode" {
        let reason = tool_input
            .get("reason")
            .and_then(|v| v.as_str())
            .map(String::from);
        let duration = tool_input.get("duration_secs").and_then(|v| v.as_u64());
        return match state.yolo_manager.request(duration, reason).await {
            Ok(req) => {
                use tauri::Emitter;
                // Broadcast to ALL open windows — not just the initiating window.
                // This ensures the approval banner appears regardless of which window
                // is currently focused, and also works when multiple windows are open.
                if let Some(w) = window {
                    if let Err(e) = w.app_handle().emit("yolo:request-pending", &req) {
                        warn!(
                            "[YOLO] Failed to emit yolo:request-pending for {}: {}",
                            req.id, e
                        );
                    }
                }
                // Fire Telegram notification so the user can approve from their phone
                // even when the desktop UI is not in focus or when running headless.
                {
                    let state_for_tg = state.clone();
                    let req_id = req.id.clone();
                    let req_duration = req.duration_secs;
                    let req_reason = req.reason.clone();
                    tokio::spawn(async move {
                        crate::telegram::send_yolo_approval_request(
                            &state_for_tg,
                            &req_id,
                            req_duration,
                            req_reason.as_deref(),
                        )
                        .await;
                    });
                }
                serde_json::json!({
                    "ok": true,
                    "request_id": req.id,
                    "message": format!(
                        "Yolo mode request submitted (id={}). \
                         The user must approve via the desktop UI or Telegram /yolo command. \
                         Wait for approval before attempting the privileged operation.",
                        req.id
                    )
                })
                .to_string()
            }
            Err(e) => serde_json::json!({ "ok": false, "error": e }).to_string(),
        };
    }

    // Hard guard check (non-bypassable, before any other check)
    if let Some(reason) = Guardrails::hard_guard_check(tool_name, tool_input) {
        warn!("[HARD_GUARD] Tool '{}' blocked: {}", tool_name, reason);
        emit_toast(
            window,
            "warning",
            "Hard Guard",
            &format!("Tool '{}' blocked: {}", tool_name, reason),
        );
        return format!("BLOCKED by hard guard: {}", reason);
    }

    // Check guardrails before executing external tool (browser with use_guardrails, computer_use, MCP)
    let mut guardrails = state.guardrails.write().await;
    let mcp_server_name_resolved;
    let server_name = if is_browser {
        "browser"
    } else if ComputerUseManager::is_computer_use_tool(tool_name) {
        "computer_use"
    } else {
        // Resolve the actual MCP server name for accurate per-server permissions
        let mcp = state.mcp_manager.read().await;
        mcp_server_name_resolved = mcp
            .get_server_name_for_tool(tool_name)
            .unwrap_or_else(|| "mcp".to_string());
        drop(mcp);
        &mcp_server_name_resolved
    };
    let check = guardrails.check_tool_call(tool_name, server_name, tool_input);

    // Apply autonomous mode override if applicable
    let check = {
        let config = state.config.read().await;
        if config.autonomous_mode.enabled {
            match &check {
                ToolCheckResult::NeedsConfirmation { .. } => {
                    // Check if autonomous mode says Autonomous for this tool
                    if let Some(perm) = guardrails.resolve_autonomy(
                        tool_name,
                        server_name,
                        tool_input,
                        &config.autonomous_mode,
                    ) {
                        match perm {
                            crate::guardrails::ToolPermission::AutoApprove
                            | crate::guardrails::ToolPermission::AllowWithLogging => {
                                info!("[AUTONOMOUS] Overriding NeedsConfirmation -> Allowed for tool '{}'", tool_name);
                                ToolCheckResult::Allowed
                            }
                            crate::guardrails::ToolPermission::Block => ToolCheckResult::Blocked {
                                tool_name: tool_name.to_string(),
                                reason: format!(
                                    "Tool '{}' is blocked by autonomous mode",
                                    tool_name
                                ),
                            },
                            _ => check,
                        }
                    } else {
                        check
                    }
                }
                ToolCheckResult::Allowed => {
                    // Even if guardrails say allowed, autonomous mode can block
                    if let Some(perm) = guardrails.resolve_autonomy(
                        tool_name,
                        server_name,
                        tool_input,
                        &config.autonomous_mode,
                    ) {
                        match perm {
                            crate::guardrails::ToolPermission::Block => ToolCheckResult::Blocked {
                                tool_name: tool_name.to_string(),
                                reason: format!(
                                    "Tool '{}' is blocked by autonomous mode",
                                    tool_name
                                ),
                            },
                            _ => check,
                        }
                    } else {
                        check
                    }
                }
                _ => check, // Blocked stays blocked
            }
        } else {
            check
        }
    };

    // If browser tool still needs confirmation but browser config says require_confirmation=false
    // (or yolo mode is active), override to Allowed.
    let check = if is_browser {
        match &check {
            ToolCheckResult::NeedsConfirmation { .. } => {
                let browser = state.browser.read().await;
                if !browser.config.require_confirmation || yolo_active {
                    info!("[BROWSER] Overriding NeedsConfirmation -> Allowed (require_confirmation=false or yolo)");
                    ToolCheckResult::Allowed
                } else {
                    check
                }
            }
            _ => check,
        }
    } else {
        check
    };

    drop(guardrails);

    // If the guardrails say NeedsConfirmation, give the channel observer a chance
    // to request human approval. An approved tool is re-classified as Allowed.
    let check = match check {
        ToolCheckResult::NeedsConfirmation { tool_name, reason } => {
            let approval_supported = observer.map(|o| o.supports_approval()).unwrap_or(false);
            let approved = if yolo_active {
                info!("[GUARDRAILS] Tool '{}' auto-approved (yolo mode active)", tool_name);
                true
            } else if let Some(obs) = observer {
                obs.request_approval(&tool_name, &reason).await
            } else {
                false
            };
            if approved {
                info!("[GUARDRAILS] Tool '{}' approved by observer", tool_name);
                ToolCheckResult::Allowed
            } else {
                let reason = if approval_supported {
                    reason
                } else {
                    missing_approval_channel_reason(&reason)
                };
                ToolCheckResult::NeedsConfirmation { tool_name, reason }
            }
        }
        other => other,
    };

    match check {
        ToolCheckResult::Blocked { tool_name, reason } => {
            warn!("[GUARDRAILS] Tool '{}' blocked: {}", tool_name, reason);
            emit_toast(
                window,
                "warning",
                "Guardrails",
                &format!("Tool '{}' blocked: {}", tool_name, reason),
            );
            format!("BLOCKED by guardrails: {}", reason)
        }
        ToolCheckResult::NeedsConfirmation { tool_name, reason } => {
            warn!(
                "[GUARDRAILS] Tool '{}' needs confirmation: {}",
                tool_name, reason
            );
            emit_toast(
                window,
                "warning",
                "Confirmation Required",
                &format!("Tool '{}': {}", tool_name, reason),
            );
            if let Some(w) = window {
                use tauri::Emitter;
                if let Err(e) = w.emit(
                    "chat:tool-blocked",
                    serde_json::json!({
                        "tool_name": &tool_name, "reason": &reason,
                    }),
                ) {
                    warn!(
                        "[GUARDRAILS] Failed to emit chat:tool-blocked for '{}': {}",
                        tool_name, e
                    );
                }
            }
            format!("BLOCKED: requires user confirmation - {}", reason)
        }
        ToolCheckResult::Allowed => {
            if is_browser {
                execute_browser_tool(tool_name, tool_input, state, window, observer).await
            } else if ComputerUseManager::is_computer_use_tool(tool_name) {
                execute_computer_use_tool(tool_name, tool_input, state, window, observer).await
            } else {
                let mcp = state.mcp_manager.read().await;
                match mcp.call_tool(tool_name, tool_input.clone()).await {
                    Ok(output) => {
                        drop(mcp);
                        output
                    }
                    Err(e) => {
                        drop(mcp);
                        warn!("[MCP] Tool '{}' failed: {}", tool_name, e);
                        format!("Tool execution failed: {}", e)
                    }
                }
            }
        }
    }
}

/// Run defense and guardrails checks on user input. Returns Err(response) if blocked.
pub(crate) async fn check_input_safety(
    message: &str,
    state: &AppState,
) -> Result<(), SendMessageResponse> {
    // Defense pipeline check on user input
    {
        let mut defense = state.defense_pipeline.write().await;
        let defense_result = defense.check(message).await;
        if !defense_result.allowed {
            let reason = defense_result
                .blocked_by
                .unwrap_or_else(|| "Defense pipeline".to_string());
            warn!("[DEFENSE] User message blocked by: {}", reason);
            return Err(SendMessageResponse {
                response: String::new(),
                error: Some(format!("Message blocked by defense pipeline: {}", reason)),
            });
        }
    }

    // Defense pipeline check on system prompt components
    {
        let mut defense = state.defense_pipeline.write().await;
        if defense.get_status().enabled {
            if let Ok(soul) = crate::soul::Soul::load() {
                let soul_ctx = soul.get_system_prompt_context();
                if !soul_ctx.is_empty() {
                    let result = defense.check(&soul_ctx).await;
                    if !result.allowed {
                        error!(
                            "[DEFENSE] CRITICAL: SOUL.md content flagged: {:?}",
                            result.blocked_by
                        );
                    }
                }
            }
            if let Ok(manager) = crate::skills::SkillsManager::new() {
                let skills_ctx = manager.get_skills_context();
                if !skills_ctx.is_empty() {
                    let result = defense.check(&skills_ctx).await;
                    if !result.allowed {
                        error!(
                            "[DEFENSE] CRITICAL: Skills content flagged: {:?}",
                            result.blocked_by
                        );
                    }
                }
            }
            if let Ok(manager) = crate::memory::MemoryManager::new() {
                let memory_ctx = manager.get_memory_context(20);
                if !memory_ctx.is_empty() {
                    let result = defense.check(&memory_ctx).await;
                    if !result.allowed {
                        error!(
                            "[DEFENSE] CRITICAL: Memory content flagged: {:?}",
                            result.blocked_by
                        );
                    }
                }
            }
        }
    }

    // Guardrails prompt injection check
    {
        let guardrails = state.guardrails.read().await;
        match guardrails.check_prompt_injection_v2(message) {
            Ok(Some(violations)) => {
                for violation in violations {
                    warn!("Prompt injection detected: {:?}", violation);
                }
            }
            Ok(None) => {}
            Err(e) => {
                warn!("[GUARDRAILS] Prompt injection blocked: {}", e);
                return Err(SendMessageResponse {
                    response: String::new(),
                    error: Some(e.to_string()),
                });
            }
        }
    }

    Ok(())
}

/// Auto-save user/assistant messages to the memory session.
/// Starts a new session if needed; auto-titles after first assistant response.
pub(crate) async fn auto_save_message(state: &AppState, role: &str, content: &str) {
    let mut memory_manager = state.memory_manager.write().await;

    // Start a new session if none exists
    if memory_manager.get_current_session().is_none() {
        if let Err(e) = memory_manager.start_session() {
            warn!("[MEMORY] Failed to start session: {}", e);
            return;
        }
    }

    // Add the message
    if let Err(e) = memory_manager.add_message(role.to_string(), content.to_string()) {
        warn!("[MEMORY] Failed to save message: {}", e);
        return;
    }

    // Auto-title: after the first assistant response, use the first ~50 chars as title
    if role == "assistant" {
        if let Some(session) = memory_manager.get_current_session() {
            if session.title.is_none() && session.messages.len() <= 2 {
                let title = if content.len() > 50 {
                    format!("{}...", &content[..50])
                } else {
                    content.to_string()
                };
                if let Err(e) = memory_manager.set_session_title(title) {
                    warn!("[CHAT] Failed to set session title: {}", e);
                }
            }
        }
    }
}

/// Attempt to sync the current session to K2K supermemory (fire-and-forget).
pub(crate) async fn try_sync_supermemory(state: &AppState) {
    let config = state.config.read().await;
    if !config.k2k.supermemory_enabled {
        return;
    }
    drop(config);

    let memory_manager = state.memory_manager.read().await;
    let session = match memory_manager.get_current_session() {
        Some(s) => s.clone(),
        None => return,
    };
    drop(memory_manager);

    // Only sync if we have enough messages (at least one exchange)
    if session.messages.len() < 2 {
        return;
    }

    let k2k = state.k2k_client.read().await;
    if !k2k.is_available().await {
        return;
    }

    let memory_manager = state.memory_manager.read().await;
    if let Err(e) = memory_manager.sync_to_supermemory(&k2k, &session).await {
        warn!("[SUPERMEMORY] Sync failed: {}", e);
    }
}

/// Extract facts/preferences/context from the current session and store as memories.
/// Called alongside try_sync_supermemory to persist local knowledge.
pub(crate) async fn try_extract_session_facts(state: &AppState) {
    let session = {
        let memory_manager = state.memory_manager.read().await;
        match memory_manager.get_current_session() {
            Some(s) => s.clone(),
            None => return,
        }
    };

    // Only extract if we have enough messages (at least 2 user messages)
    let user_msg_count = session.messages.iter().filter(|m| m.role == "user").count();
    if user_msg_count < 2 {
        return;
    }

    let mut memory_manager = state.memory_manager.write().await;
    match memory_manager.extract_facts_from_session(&session) {
        Ok(ids) => {
            if !ids.is_empty() {
                info!("[MEMORY] Auto-extracted {} facts from session", ids.len());
            }
        }
        Err(e) => {
            warn!("[MEMORY] Fact extraction failed: {}", e);
        }
    }
}

// --- Compaction types and helpers ---

#[derive(Debug, Serialize, Deserialize)]
pub struct CompactResponse {
    pub success: bool,
    pub messages_before: usize,
    pub messages_after: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub error: Option<String>,
}

/// Check context usage and auto-compact if threshold exceeded.
/// Fire-and-forget: if compaction fails, the conversation proceeds anyway.
pub(crate) async fn maybe_auto_compact(
    state: &AppState,
    window: Option<&tauri::Window>,
    client: Option<&ClaudeClient>,
) {
    let config = state.config.read().await;
    if !config.claude.auto_compact_enabled {
        return;
    }
    let model = config.claude.model.clone();
    drop(config);

    // Use the provided client (active agent's) or fall back to the global client
    let fallback;
    let claude_client = if let Some(c) = client {
        c
    } else {
        fallback = state.claude_client.read().await;
        &*fallback
    };
    let (total_tokens, window_size, usage_pct) = claude_client.get_context_usage(&model).await;

    // Update context manager with current usage
    let usage = state
        .context_manager
        .update_usage(total_tokens, window_size)
        .unwrap_or_else(|e| {
            warn!("[CONTEXT] Failed to update usage: {}", e);
            return crate::context_manager::ContextUsage {
                tokens: total_tokens,
                window_size,
                usage_percent: usage_pct,
                state: crate::context_manager::ContextState::Normal,
                timestamp: chrono::Utc::now(),
            };
        });

    // Log state transitions
    debug!(
        "[CONTEXT] Usage: {:.1}% ({}/{} tokens), State: {:?}",
        usage_pct, total_tokens, window_size, usage.state
    );

    // Check if compaction should be triggered
    if !state.context_manager.should_compact() {
        return;
    }

    info!(
        "[AUTO-COMPACT] Context at {:.1}% usage, triggering compaction...",
        usage_pct
    );

    if let Some(w) = window {
        if let Err(e) = w.emit(
            "compact:status",
            serde_json::json!({
                "status": "auto_compacting",
                "message": format!("Auto-compacting: context at {:.0}% usage", usage_pct),
            }),
        ) {
            warn!("[COMPACT] Failed to emit compact:status auto_compacting: {}", e);
        }
        emit_toast(
            Some(w),
            "info",
            "Auto-Compaction",
            &format!("Context at {:.0}% — compacting...", usage_pct),
        );
    }

    // Re-resolve the client for compaction (use provided or global)
    let fallback2;
    let compact_client = if let Some(c) = client {
        c
    } else {
        fallback2 = state.claude_client.read().await;
        &*fallback2
    };
    match compact_client.compact_conversation().await {
        Ok(result) if result.was_compacted => {
            // Record metrics in context manager
            if let Err(e) = state.context_manager.record_compaction(
                result.messages_before,
                result.messages_after,
                result.tokens_before,
                result.tokens_after,
                "Auto-compaction triggered",
            ) {
                warn!("[CHAT] Failed to record compaction metrics: {}", e);
            }

            // Record in session memory
            let mut memory_manager = state.memory_manager.write().await;
            let summary = format!(
                "Auto-compacted at {:.0}% usage: {} → {} messages (~{} → ~{} tokens)",
                usage_pct,
                result.messages_before,
                result.messages_after,
                result.tokens_before,
                result.tokens_after,
            );
            if let Err(e) = memory_manager
                .compact_session(&summary, result.messages_before - result.messages_after)
            {
                warn!("[CHAT] Failed to compact session: {}", e);
            }

            if let Some(w) = window {
                if let Err(e) = w.emit(
                    "compact:status",
                    serde_json::json!({
                        "status": "auto_complete",
                        "message": format!(
                            "Auto-compacted: {} → {} messages (~{} → ~{} tokens)",
                            result.messages_before, result.messages_after,
                            result.tokens_before, result.tokens_after,
                        ),
                    }),
                ) {
                    warn!("[COMPACT] Failed to emit compact:status auto_complete: {}", e);
                }
                emit_toast(
                    Some(w),
                    "success",
                    "Auto-Compaction",
                    &format!(
                        "Compacted: {} → {} messages, saved ~{} tokens",
                        result.messages_before,
                        result.messages_after,
                        result.tokens_before - result.tokens_after
                    ),
                );
            }

            info!(
                "[AUTO-COMPACT] Done: {} → {} messages, freed ~{} tokens",
                result.messages_before,
                result.messages_after,
                result.tokens_before - result.tokens_after
            );
        }
        Ok(_) => {
            debug!("[AUTO-COMPACT] Not compacted (conversation too short)");
        }
        Err(e) => {
            warn!("[AUTO-COMPACT] Failed: {}", e);
            if let Some(w) = window {
                emit_toast(
                    Some(w),
                    "warning",
                    "Auto-Compaction",
                    &format!("Compaction failed: {}", e),
                );
            }
        }
    }
}

/// Compact the conversation by summarizing older messages.
#[tauri::command]
pub async fn compact_conversation(
    state: State<'_, AppState>,
    window: tauri::Window,
) -> Result<CompactResponse, String> {
    info!("[COMPACT] User requested conversation compaction");

    if let Err(e) = window.emit(
        "compact:status",
        serde_json::json!({
            "status": "summarizing",
            "message": "Summarizing conversation history..."
        }),
    ) {
        warn!("[COMPACT] Failed to emit compact:status summarizing: {}", e);
    }

    let claude_client = state.claude_client.read().await;

    match claude_client.compact_conversation().await {
        Ok(result) => {
            if result.was_compacted {
                let mut memory_manager = state.memory_manager.write().await;
                if let Err(e) = memory_manager.compact_session(
                    &format!(
                        "Compacted from {} to {} messages",
                        result.messages_before, result.messages_after
                    ),
                    result.messages_before - result.messages_after,
                ) {
                    warn!("[CHAT] Failed to compact session: {}", e);
                }

                if let Err(e) = window.emit(
                    "compact:status",
                    serde_json::json!({
                        "status": "complete",
                        "message": format!(
                            "Compacted: {} -> {} messages, ~{} -> ~{} tokens",
                            result.messages_before, result.messages_after,
                            result.tokens_before, result.tokens_after,
                        )
                    }),
                ) {
                    warn!("[COMPACT] Failed to emit compact:status complete: {}", e);
                }
            } else {
                if let Err(e) = window.emit(
                    "compact:status",
                    serde_json::json!({
                        "status": "skipped",
                        "message": "Conversation too short to compact."
                    }),
                ) {
                    warn!("[COMPACT] Failed to emit compact:status skipped: {}", e);
                }
            }

            Ok(CompactResponse {
                success: true,
                messages_before: result.messages_before,
                messages_after: result.messages_after,
                tokens_before: result.tokens_before,
                tokens_after: result.tokens_after,
                error: None,
            })
        }
        Err(e) => {
            error!("[COMPACT] Failed: {}", e);
            if let Err(emit_err) = window.emit(
                "compact:status",
                serde_json::json!({
                    "status": "error",
                    "message": format!("Compaction failed: {}", e)
                }),
            ) {
                warn!(
                    "[COMPACT] Failed to emit compact:status error after compaction failure: {}",
                    emit_err
                );
            }
            Ok(CompactResponse {
                success: false,
                messages_before: 0,
                messages_after: 0,
                tokens_before: 0,
                tokens_after: 0,
                error: Some(e.to_string()),
            })
        }
    }
}

/// Get estimated context usage stats.
#[tauri::command]
pub async fn get_context_usage(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let config = state.config.read().await;
    let model = config.claude.model.clone();
    drop(config);

    let claude_client = state.claude_client.read().await;
    let (total_tokens, window_size, usage_pct) = claude_client.get_context_usage(&model).await;

    Ok(serde_json::json!({
        "estimated_tokens": total_tokens,
        "context_window": window_size,
        "usage_percent": usage_pct,
        "model": model,
    }))
}

/// Resolve the Claude client for a GUI message (agent-specific or active GUI agent).
async fn resolve_gui_client(state: &AppState, agent_id: Option<&str>) -> ClaudeClient {
    if let Some(id) = agent_id {
        let agent_mgr = state.agent_manager.read().await;
        if let Some(agent) = agent_mgr.get_agent(id) {
            return agent.claude_client.clone();
        }
    }
    let agent_mgr = state.agent_manager.read().await;
    let active_id = agent_mgr.active_gui_agent_id.clone();
    if let Some(agent) = agent_mgr.get_agent(&active_id) {
        agent.claude_client.clone()
    } else {
        state.claude_client.read().await.clone()
    }
}

/// Config intent detected from natural language.
#[derive(Debug)]
enum ConfigIntent {
    SetVoiceEnabled(bool),
    SetModel(String),
    SetGuardrails(String),
    SetExecuteEnabled(bool),
    SetBrowserEnabled(bool),
    SetFetchEnabled(bool),
}

/// Detect a config management intent from natural language.
///
/// Only intercepts clearly unambiguous patterns.  Ambiguous cases are passed
/// through to the LLM.
fn detect_config_intent(message: &str) -> Option<ConfigIntent> {
    let msg = message.to_lowercase();
    let msg = msg.trim();

    // Voice
    if msg.contains("turn off voice")
        || msg.contains("disable voice")
        || msg.contains("mute voice")
        || msg == "no voice"
    {
        return Some(ConfigIntent::SetVoiceEnabled(false));
    }
    if msg.contains("turn on voice")
        || msg.contains("enable voice")
        || msg.contains("unmute voice")
        || msg == "voice on"
    {
        return Some(ConfigIntent::SetVoiceEnabled(true));
    }

    // Model switching — only handle explicit "use/switch to claude-*" patterns.
    {
        let prefixes = ["switch to model ", "use model ", "change model to "];
        for prefix in &prefixes {
            if let Some(rest) = msg.strip_prefix(prefix) {
                let model = rest.trim().to_string();
                if !model.is_empty() {
                    return Some(ConfigIntent::SetModel(model));
                }
            }
        }
        // "use claude-sonnet…" / "switch to claude-opus…"
        let claude_prefixes = ["switch to claude", "use claude"];
        for prefix in &claude_prefixes {
            if msg.starts_with(prefix) {
                // Extract everything after "to " or "use "
                let after = if let Some(pos) = msg.find(" claude") {
                    msg[pos + 1..].trim()
                } else {
                    ""
                };
                if !after.is_empty() && !after.contains(' ') {
                    return Some(ConfigIntent::SetModel(after.to_string()));
                }
            }
        }
    }

    // Guardrails
    if msg.contains("set guardrails to maximum")
        || msg.contains("guardrails maximum")
        || msg.contains("security level maximum")
    {
        return Some(ConfigIntent::SetGuardrails("maximum".into()));
    }
    if msg.contains("set guardrails to standard")
        || msg.contains("guardrails standard")
        || msg.contains("security level standard")
    {
        return Some(ConfigIntent::SetGuardrails("standard".into()));
    }
    if msg.contains("set guardrails to relaxed")
        || msg.contains("guardrails relaxed")
        || msg.contains("security level relaxed")
    {
        return Some(ConfigIntent::SetGuardrails("relaxed".into()));
    }

    // Execute tool
    if msg.contains("disable execute") || msg.contains("turn off execute") {
        return Some(ConfigIntent::SetExecuteEnabled(false));
    }
    if msg.contains("enable execute") || msg.contains("turn on execute") {
        return Some(ConfigIntent::SetExecuteEnabled(true));
    }

    // Browser tool
    if msg.contains("disable browser") || msg.contains("turn off browser") {
        return Some(ConfigIntent::SetBrowserEnabled(false));
    }
    if msg.contains("enable browser") || msg.contains("turn on browser") {
        return Some(ConfigIntent::SetBrowserEnabled(true));
    }

    // Fetch tool
    if msg.contains("disable fetch") || msg.contains("turn off fetch") {
        return Some(ConfigIntent::SetFetchEnabled(false));
    }
    if msg.contains("enable fetch") || msg.contains("turn on fetch") {
        return Some(ConfigIntent::SetFetchEnabled(true));
    }

    None
}

/// Apply a detected config intent, returning a human-readable confirmation.
async fn apply_config_intent(intent: &ConfigIntent, state: &AppState) -> String {
    let mut cfg = state.config.write().await;
    match intent {
        ConfigIntent::SetVoiceEnabled(enabled) => {
            cfg.wakeword.enabled = *enabled;
            let verb = if *enabled { "enabled" } else { "disabled" };
            if let Err(e) = cfg.save() {
                warn!("[CONFIG_INTENT] Failed to save after voice {}: {}", verb, e);
            }
            format!("Voice has been {}.", verb)
        }
        ConfigIntent::SetModel(model) => {
            let old = cfg.claude.model.clone();
            cfg.claude.model = model.clone();
            if let Err(e) = cfg.save() {
                warn!("[CONFIG_INTENT] Failed to save after model change: {}", e);
            }
            format!("Model changed from '{}' to '{}'.", old, model)
        }
        ConfigIntent::SetGuardrails(level) => {
            use crate::guardrails::SecurityLevel;
            let parsed = match level.as_str() {
                "maximum"  => Some(SecurityLevel::Maximum),
                "standard" => Some(SecurityLevel::Standard),
                "relaxed"  => Some(SecurityLevel::Relaxed),
                _          => None,
            };
            if let Some(sl) = parsed {
                cfg.guardrails.security_level = sl;
                if let Err(e) = cfg.save() {
                    warn!("[CONFIG_INTENT] Failed to save after guardrails change: {}", e);
                }
                format!("Guardrails set to '{}'.", level)
            } else {
                format!("Unknown guardrails level '{}'.", level)
            }
        }
        ConfigIntent::SetExecuteEnabled(enabled) => {
            cfg.execute.enabled = *enabled;
            let verb = if *enabled { "enabled" } else { "disabled" };
            if let Err(e) = cfg.save() {
                warn!("[CONFIG_INTENT] Failed to save after execute {}: {}", verb, e);
            }
            format!("Execute tool has been {}.", verb)
        }
        ConfigIntent::SetBrowserEnabled(enabled) => {
            cfg.browser.enabled = *enabled;
            let verb = if *enabled { "enabled" } else { "disabled" };
            if let Err(e) = cfg.save() {
                warn!("[CONFIG_INTENT] Failed to save after browser {}: {}", verb, e);
            }
            format!("Browser tool has been {}.", verb)
        }
        ConfigIntent::SetFetchEnabled(enabled) => {
            cfg.fetch.enabled = *enabled;
            let verb = if *enabled { "enabled" } else { "disabled" };
            if let Err(e) = cfg.save() {
                warn!("[CONFIG_INTENT] Failed to save after fetch {}: {}", verb, e);
            }
            format!("Fetch tool has been {}.", verb)
        }
    }
}

/// Send a message to Claude (non-streaming, backwards compatible)
#[tauri::command]
pub async fn send_message(
    request: SendMessageRequest,
    state: State<'_, AppState>,
    window: tauri::Window,
) -> Result<SendMessageResponse, String> {
    info!("Sending message to Claude: {}", request.message);

    // Config-via-conversation: intercept clearly-detected intents before LLM.
    if let Some(intent) = detect_config_intent(&request.message) {
        // Only apply from the GUI channel (not from external bots).
        let confirmation = apply_config_intent(&intent, &state).await;
        info!("[CONFIG_INTENT] Applied {:?} — {}", intent, confirmation);
        return Ok(SendMessageResponse {
            response: confirmation,
            error: None,
        });
    }

    let claude_client = resolve_gui_client(&state, request.agent_id.as_deref()).await;
    let overrides = state.session_overrides.read().await.clone();

    let message = IncomingMessage {
        text: request.message.clone(),
        channel: ChannelSource::Gui,
        agent_id: request.agent_id.clone(),
        metadata: HashMap::new(),
    };

    let observer = tool_loop::GuiStreamingObserver {
        window: window.clone(),
        pending_approvals: state.gui_pending_approvals.clone(),
    };
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides,
        loop_config: ToolLoopConfig::gui_non_streaming(),
        observer: &observer,
        streaming: false,
        window: Some(&window),
        on_stream_chunk: None,
        auto_compact: true,
        save_to_memory: true,
        sync_supermemory: true,
        check_sensitive_data: true,
    };

    match router::route_message(&message, options, &state).await {
        Ok(routed) => Ok(SendMessageResponse {
            response: routed.text,
            error: None,
        }),
        Err(RouterError::Blocked(msg)) => Ok(SendMessageResponse {
            response: String::new(),
            error: Some(msg),
        }),
        Err(e) => Ok(SendMessageResponse {
            response: String::new(),
            error: Some(e.to_string()),
        }),
    }
}

// --- Event payload types for streaming ---

#[derive(Debug, Clone, Serialize)]
struct TextChunkEvent {
    text: String,
}

#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
struct ToolStartEvent {
    name: String,
    id: String,
}

#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
struct ToolResultEvent {
    name: String,
    id: String,
    success: bool,
}

#[derive(Debug, Clone, Serialize)]
struct CompleteEvent {
    response: String,
    error: Option<String>,
    model_used: Option<String>,
}

/// Send a message with streaming events emitted to the frontend.
/// Events: chat:text-chunk, chat:tool-start, chat:tool-result, chat:complete
#[tauri::command]
pub async fn send_message_with_events(
    request: SendMessageRequest,
    state: State<'_, AppState>,
    window: tauri::Window,
) -> Result<(), String> {
    info!("Sending message with streaming events: {}", request.message);

    // Config-via-conversation: intercept clearly-detected intents before LLM.
    if let Some(intent) = detect_config_intent(&request.message) {
        let confirmation = apply_config_intent(&intent, &state).await;
        info!("[CONFIG_INTENT] Applied {:?} — {}", intent, confirmation);
        if let Err(e) = window.emit(
            "chat:complete",
            CompleteEvent {
                response: confirmation,
                error: None,
                model_used: None,
            },
        ) {
            warn!("[CHAT] Failed to emit chat:complete for config intent: {}", e);
        }
        return Ok(());
    }

    // Clear any stale cancel flag from a previous message before starting.
    clear_cancel_flag();

    let claude_client = resolve_gui_client(&state, request.agent_id.as_deref()).await;
    let overrides = state.session_overrides.read().await.clone();

    let message = IncomingMessage {
        text: request.message.clone(),
        channel: ChannelSource::Gui,
        agent_id: request.agent_id.clone(),
        metadata: HashMap::new(),
    };

    let window_clone = window.clone();
    let observer = tool_loop::GuiStreamingObserver {
        window: window.clone(),
        pending_approvals: state.gui_pending_approvals.clone(),
    };
    let options = RouteOptions {
        claude_client: &claude_client,
        overrides,
        loop_config: ToolLoopConfig::gui_default(),
        observer: &observer,
        streaming: true,
        window: Some(&window),
        on_stream_chunk: Some(Box::new(move |chunk| {
            if let Err(e) = window_clone.emit("chat:text-chunk", TextChunkEvent { text: chunk }) {
                debug!("[CHAT] Failed to emit chat:text-chunk: {}", e);
            }
        })),
        auto_compact: true,
        save_to_memory: true,
        sync_supermemory: true,
        check_sensitive_data: true,
    };

    match router::route_message(&message, options, &state).await {
        Ok(routed) => {
            if let Err(e) = window.emit(
                "chat:complete",
                CompleteEvent {
                    response: routed.text,
                    error: None,
                    model_used: Some(routed.model_used),
                },
            ) {
                warn!("[CHAT] Failed to emit chat:complete success: {}", e);
            }
        }
        Err(RouterError::Blocked(msg)) => {
            if let Err(e) = window.emit(
                "chat:complete",
                CompleteEvent {
                    response: String::new(),
                    error: Some(msg),
                    model_used: None,
                },
            ) {
                warn!("[CHAT] Failed to emit chat:complete blocked: {}", e);
            }
        }
        Err(e) => {
            if let Err(emit_err) = window.emit(
                "chat:complete",
                CompleteEvent {
                    response: String::new(),
                    error: Some(e.to_string()),
                    model_used: None,
                },
            ) {
                error!("[CHAT] Failed to emit chat:complete error: {}", emit_err);
            }
        }
    }

    Ok(())
}

/// Resolve a pending GUI tool-approval request.
///
/// Returns `Ok(true)` when a pending request was found and resolved,
/// `Ok(false)` when the request was already expired/handled.
#[tauri::command]
pub async fn respond_tool_approval(
    request_id: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let sender = state.gui_pending_approvals.lock().await.remove(&request_id);
    if let Some(tx) = sender {
        if tx.send(approved).is_err() {
            warn!(
                "[GUI] Failed to deliver tool-approval response for request {}",
                request_id
            );
            return Ok(false);
        }
        Ok(true)
    } else {
        info!(
            "[GUI] Tool-approval request {} already resolved or expired",
            request_id
        );
        Ok(false)
    }
}

/// Check if the System Agent supermemory is available
#[tauri::command]
pub async fn is_supermemory_available(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state.config.read().await;
    if !config.k2k.supermemory_enabled {
        return Ok(false);
    }
    drop(config);

    let k2k = state.k2k_client.read().await;
    k2k.health_check().await.map_err(|e| e.to_string())
}

/// Search using K2K integration with optional rich context.
///
/// Rich context fields (`conversation_summary`, `triggering_tool`,
/// `is_background`, `context`) are merged into a single blob forwarded to
/// the KB backend so it can weight results more accurately.
///
/// Background queries with a `confidence_threshold >= 1.0` are skipped
/// immediately to avoid redundant round-trips when NexiBot is already certain.
#[tauri::command]
pub async fn search_k2k(
    request: K2KSearchRequest,
    state: State<'_, AppState>,
) -> Result<K2KSearchResponse, String> {
    info!("Searching via K2K: {}", request.query);

    // Skip background queries that exceed the confidence threshold
    if request.is_background {
        if let Some(threshold) = request.confidence_threshold {
            if threshold >= 1.0 {
                debug!(
                    "[K2K] Skipping background KB query for '{}' — confidence threshold {} met",
                    request.query, threshold
                );
                return Ok(K2KSearchResponse {
                    results: Vec::new(),
                    total_results: 0,
                    error: None,
                });
            }
        }
    }

    // Build rich context string forwarded to the backend
    let rich_context: Option<String> = {
        let mut parts: Vec<String> = Vec::new();
        if let Some(ref summary) = request.conversation_summary {
            parts.push(format!("conversation_summary: {}", summary));
        }
        if let Some(ref tool) = request.triggering_tool {
            parts.push(format!("triggering_tool: {}", tool));
        }
        parts.push(format!(
            "query_type: {}",
            if request.is_background {
                "background_enrichment"
            } else {
                "user_initiated"
            }
        ));
        if let Some(ref explicit_ctx) = request.context {
            parts.push(format!("extra_context: {}", explicit_ctx));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("; "))
        }
    };

    let k2k_client = state.k2k_client.read().await;
    let top_k = request.top_k.unwrap_or(10);
    let ctx_ref = rich_context.as_deref();

    let result = if request.use_federated {
        k2k_client
            .query_federated_with_context(&request.query, top_k, ctx_ref)
            .await
    } else {
        k2k_client
            .query_with_context(&request.query, top_k, ctx_ref)
            .await
    };

    match result {
        Ok(response) => {
            let results = response
                .results
                .into_iter()
                .map(|r| K2KResult {
                    title: r.title,
                    summary: r.summary,
                    content: r.content,
                    confidence: r.confidence,
                    source_type: r.source_type,
                })
                .collect();

            Ok(K2KSearchResponse {
                results,
                total_results: response.total_results,
                error: None,
            })
        }
        Err(e) => {
            error!("Failed to search via K2K: {}", e);
            Ok(K2KSearchResponse {
                results: Vec::new(),
                total_results: 0,
                error: Some(e.to_string()),
            })
        }
    }
}

/// Parse a `NotificationTarget` from `nexibot_background_task` tool input.
///
/// Accepts the new structured `notify_target` field AND the legacy `notify_telegram: bool`
/// for backwards compatibility.
fn parse_notify_target(
    tool_input: &serde_json::Value,
) -> Option<crate::notifications::NotificationTarget> {
    use crate::notifications::NotificationTarget;

    fn parse_i64_field(obj: &serde_json::Value, key: &str) -> Option<i64> {
        let raw = obj.get(key)?;
        raw.as_i64()
            .or_else(|| raw.as_u64().and_then(|v| i64::try_from(v).ok()))
            .or_else(|| raw.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
    }

    fn parse_u64_field(obj: &serde_json::Value, key: &str) -> Option<u64> {
        let raw = obj.get(key)?;
        raw.as_u64()
            .or_else(|| raw.as_i64().and_then(|v| u64::try_from(v).ok()))
            .or_else(|| raw.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
    }

    fn parse_string_field(obj: &serde_json::Value, key: &str) -> Option<String> {
        let raw = obj.get(key)?;
        if let Some(s) = raw.as_str() {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        } else if raw.is_number() {
            Some(raw.to_string())
        } else {
            None
        }
    }

    // New structured field takes priority. Invalid structured targets fail closed (None).
    if let Some(nt) = tool_input.get("notify_target") {
        return match nt["type"].as_str().unwrap_or("") {
            "all_configured" => Some(NotificationTarget::AllConfigured),
            "gui" => Some(NotificationTarget::Gui),
            "telegram_configured" => Some(NotificationTarget::TelegramConfigured),
            "telegram" => parse_i64_field(nt, "chat_id")
                .filter(|chat_id| *chat_id != 0)
                .map(|chat_id| NotificationTarget::Telegram { chat_id }),
            "discord" => parse_u64_field(nt, "channel_id")
                .filter(|channel_id| *channel_id != 0)
                .map(|channel_id| NotificationTarget::Discord { channel_id }),
            "slack" => parse_string_field(nt, "channel_id")
                .map(|channel_id| NotificationTarget::Slack { channel_id }),
            "whatsapp" => parse_string_field(nt, "phone_number")
                .map(|phone_number| NotificationTarget::WhatsApp { phone_number }),
            "signal" => parse_string_field(nt, "phone_number")
                .map(|phone_number| NotificationTarget::Signal { phone_number }),
            "matrix" => parse_string_field(nt, "room_id")
                .map(|room_id| NotificationTarget::Matrix { room_id }),
            "mattermost" => parse_string_field(nt, "channel_id")
                .map(|channel_id| NotificationTarget::Mattermost { channel_id }),
            "google_chat" => Some(NotificationTarget::GoogleChat),
            "bluebubbles" => parse_string_field(nt, "chat_guid")
                .map(|chat_guid| NotificationTarget::BlueBubbles { chat_guid }),
            "messenger" => parse_string_field(nt, "recipient_id")
                .map(|recipient_id| NotificationTarget::Messenger { recipient_id }),
            "instagram" => parse_string_field(nt, "recipient_id")
                .map(|recipient_id| NotificationTarget::Instagram { recipient_id }),
            "line" => parse_string_field(nt, "user_id")
                .map(|user_id| NotificationTarget::Line { user_id }),
            "twilio" => parse_string_field(nt, "phone_number")
                .map(|phone_number| NotificationTarget::Twilio { phone_number }),
            _ => None,
        };
    }

    // Legacy boolean field: preserve old "notify_telegram" intent without
    // broadening scope to non-Telegram channels.
    if tool_input["notify_telegram"].as_bool().unwrap_or(false) {
        return Some(NotificationTarget::TelegramConfigured);
    }

    None
}

/// Helper to spawn a background task on tokio.
/// `execute_background_task` awaits `execute_tool_call` which takes `Option<&Window>`.
/// Even though we always pass `None`, the compiler can't prove the future is Send.
/// This wrapper uses an unsafe Send projection since the Window ref is always None.
fn spawn_background_task(
    task_id: String,
    instructions: String,
    state: AppState,
    app_handle: Option<tauri::AppHandle>,
    source_channel: Option<ChannelSource>,
    source_sender_id: Option<String>,
    source_agent_id: String,
) {
    struct SendFuture<F>(F);
    // SAFETY: execute_background_task never holds a non-Send &Window across .await;
    // it always passes None to execute_tool_call's window parameter.
    unsafe impl<F: std::future::Future> Send for SendFuture<F> {}
    impl<F: std::future::Future> std::future::Future for SendFuture<F> {
        type Output = F::Output;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            // SAFETY: Pin projection is safe — SendFuture is a transparent wrapper.
            let inner = unsafe { self.map_unchecked_mut(|s| &mut s.0) };
            inner.poll(cx)
        }
    }
    tokio::spawn(SendFuture(execute_background_task(
        task_id,
        instructions,
        state,
        app_handle,
        source_channel,
        source_sender_id,
        source_agent_id,
    )));
}

/// Execute a background task with its own Claude client and tool loop.
/// Spawned by the `nexibot_background_task` tool handler.
pub(crate) async fn execute_background_task(
    task_id: String,
    instructions: String,
    state: AppState,
    app_handle: Option<tauri::AppHandle>,
    source_channel: Option<ChannelSource>,
    source_sender_id: Option<String>,
    source_agent_id: String,
) {
    let bg_client = ClaudeClient::new(state.config.clone());

    // Emit task:started
    if let Some(ref h) = app_handle {
        if let Err(e) = h.emit(
            "task:started",
            serde_json::json!({
                "id": task_id,
                "description": instructions.chars().take(100).collect::<String>(),
                "status": "running",
            }),
        ) {
            warn!("[BACKGROUND_TASK] Failed to emit task:started: {}", e);
        }
    }

    let prompt = format!(
        "You are a background worker. Complete this task:\n\n{}\n\n\
        Use the available tools. Work methodically. When done, provide a brief summary.",
        instructions
    );

    let message = IncomingMessage {
        text: prompt,
        channel: source_channel.clone().unwrap_or(ChannelSource::Gui),
        agent_id: Some(source_agent_id),
        metadata: HashMap::new(),
    };

    let background_observer = tool_loop::BackgroundObserver {
        app_handle: app_handle.clone(),
        task_id: task_id.clone(),
        task_manager: state.task_manager.clone(),
        // Background tasks can fall back to GUI approval when a desktop app
        // handle exists.
        allow_gui_approval: app_handle.is_some(),
        pending_approvals: state.gui_pending_approvals.clone(),
    };
    let mut telegram_observer: Option<tool_loop::TelegramObserver> = None;
    let mut discord_observer: Option<DiscordObserver> = None;
    let mut slack_observer: Option<SlackObserver> = None;
    let mut signal_observer: Option<SignalObserver> = None;
    let mut whatsapp_observer: Option<WhatsAppObserver> = None;
    let mut teams_observer: Option<TeamsObserver> = None;
    let mut matrix_observer: Option<MatrixObserver> = None;
    let mut bluebubbles_observer: Option<BlueBubblesObserver> = None;
    let mut mattermost_observer: Option<MattermostObserver> = None;
    let mut google_chat_observer: Option<GoogleChatObserver> = None;
    let mut twilio_observer: Option<TwilioObserver> = None;
    let mut messenger_observer: Option<MessengerObserver> = None;
    let mut instagram_observer: Option<InstagramObserver> = None;
    let mut line_observer: Option<LineObserver> = None;
    let mut rocketchat_observer: Option<RocketChatObserver> = None;
    let mut mastodon_observer: Option<MastodonObserver> = None;
    let mut webchat_observer: Option<WebChatObserver> = None;
    match source_channel.as_ref() {
        Some(ChannelSource::Telegram { chat_id }) => {
            if *chat_id == 0 {
                warn!(
                    "[BACKGROUND_TASK] Invalid Telegram chat id 0 for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else if let Some(sender_id) = source_sender_id.as_deref() {
                match sender_id.parse::<i64>() {
                    Ok(requester_user_id) => {
                        if requester_user_id <= 0 {
                            warn!(
                                "[BACKGROUND_TASK] Invalid Telegram sender id '{}' for task {}; falling back to GUI-only approvals",
                                sender_id, task_id
                            );
                        } else {
                            let bot_token = {
                                let cfg = state.config.read().await;
                                cfg.telegram.bot_token.clone()
                            };
                            if bot_token.is_empty() {
                                warn!(
                                    "[BACKGROUND_TASK] Telegram source task {} has no bot token configured; falling back to GUI-only approvals",
                                    task_id
                                );
                            } else {
                                telegram_observer = Some(tool_loop::TelegramObserver::new(
                                    teloxide::Bot::new(bot_token),
                                    teloxide::types::ChatId(*chat_id),
                                    Some(requester_user_id),
                                    state.telegram_pending_approvals.clone(),
                                ));
                            }
                        }
                    }
                    Err(_) => {
                        warn!(
                            "[BACKGROUND_TASK] Invalid Telegram sender id '{}' for task {}; falling back to GUI-only approvals",
                            sender_id, task_id
                        );
                    }
                }
            } else {
                warn!(
                    "[BACKGROUND_TASK] Missing Telegram sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            }
        }
        Some(ChannelSource::Discord { channel_id, .. }) => {
            if *channel_id == 0 {
                warn!(
                    "[BACKGROUND_TASK] Invalid Discord channel id 0 for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else if let Some(sender_id) = source_sender_id.as_deref() {
                match sender_id.parse::<u64>() {
                    Ok(requester_user_id) => {
                        if requester_user_id == 0 {
                            warn!(
                                "[BACKGROUND_TASK] Invalid Discord sender id '{}' for task {}; falling back to GUI-only approvals",
                                sender_id, task_id
                            );
                        } else {
                            let bot_token = {
                                let cfg = state.config.read().await;
                                cfg.discord.bot_token.clone()
                            };
                            if bot_token.is_empty() {
                                warn!(
                                    "[BACKGROUND_TASK] Discord source task {} has no bot token configured; falling back to GUI-only approvals",
                                    task_id
                                );
                            } else {
                                let http =
                                    std::sync::Arc::new(serenity::http::Http::new(&bot_token));
                                discord_observer = Some(DiscordObserver::new(
                                    http,
                                    *channel_id,
                                    requester_user_id,
                                    state.discord_pending_approvals.clone(),
                                ));
                            }
                        }
                    }
                    Err(_) => {
                        warn!(
                            "[BACKGROUND_TASK] Invalid Discord sender id '{}' for task {}; falling back to GUI-only approvals",
                            sender_id, task_id
                        );
                    }
                }
            } else {
                warn!(
                    "[BACKGROUND_TASK] Missing Discord sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            }
        }
        Some(ChannelSource::Slack { channel_id }) => {
            if channel_id.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty Slack channel id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else if let Some(requester_user_id) = source_sender_id.as_deref() {
                if requester_user_id.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Empty Slack sender id for task {}; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    let bot_token = {
                        let cfg = state.config.read().await;
                        cfg.slack.bot_token.clone()
                    };
                    if bot_token.is_empty() {
                        warn!(
                            "[BACKGROUND_TASK] Slack source task {} has no bot token configured; falling back to GUI-only approvals",
                            task_id
                        );
                    } else {
                        slack_observer = Some(SlackObserver::new(
                            state.clone(),
                            channel_id.clone(),
                            requester_user_id.to_string(),
                            true,
                            state.slack_pending_approvals.clone(),
                        ));
                    }
                }
            } else {
                warn!(
                    "[BACKGROUND_TASK] Missing Slack sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            }
        }
        Some(ChannelSource::Signal { phone_number }) => {
            let requester_number = source_sender_id
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(phone_number.as_str());
            let (api_url, bot_number) = {
                let cfg = state.config.read().await;
                (cfg.signal.api_url.clone(), cfg.signal.phone_number.clone())
            };
            if api_url.is_empty() || bot_number.is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Signal source task {} is missing signal.api_url or signal.phone_number; falling back to GUI-only approvals",
                    task_id
                );
            } else if requester_number.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty Signal sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else {
                signal_observer = Some(SignalObserver::new(
                    api_url,
                    bot_number,
                    requester_number.to_string(),
                    state.signal_pending_approvals.clone(),
                ));
            }
        }
        Some(ChannelSource::WhatsApp { phone_number }) => {
            let requester_number = source_sender_id
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(phone_number.as_str());
            if requester_number.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty WhatsApp sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else {
                let (phone_number_id, access_token) = {
                    let cfg = state.config.read().await;
                    (
                        cfg.whatsapp.phone_number_id.clone(),
                        cfg.whatsapp.access_token.clone(),
                    )
                };
                if phone_number_id.trim().is_empty() || access_token.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] WhatsApp source task {} has missing phone_number_id/access_token; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    whatsapp_observer = Some(WhatsAppObserver::new(
                        state.clone(),
                        requester_number.to_string(),
                        true,
                        state.whatsapp_pending_approvals.clone(),
                    ));
                }
            }
        }
        Some(ChannelSource::Teams { conversation_id }) => {
            if conversation_id.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty Teams conversation id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else if let Some(requester_user_id) = source_sender_id.as_deref() {
                if requester_user_id.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Empty Teams sender id for task {}; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    let (app_id, app_password) = {
                        let cfg = state.config.read().await;
                        (cfg.teams.app_id.clone(), cfg.teams.app_password.clone())
                    };
                    let service_url = state
                        .teams_conversation_service_urls
                        .read()
                        .await
                        .get(conversation_id)
                        .cloned();
                    if app_id.trim().is_empty() || app_password.trim().is_empty() {
                        warn!(
                            "[BACKGROUND_TASK] Teams source task {} has missing app_id/app_password; falling back to GUI-only approvals",
                            task_id
                        );
                    } else if let Some(service_url) = service_url {
                        if service_url.trim().is_empty() {
                            warn!(
                                "[BACKGROUND_TASK] Empty Teams service_url for conversation {} (task {}); falling back to GUI-only approvals",
                                conversation_id, task_id
                            );
                        } else {
                            teams_observer = Some(TeamsObserver::new(
                                state.clone(),
                                conversation_id.clone(),
                                requester_user_id.to_string(),
                                service_url,
                                true,
                                state.teams_pending_approvals.clone(),
                            ));
                        }
                    } else {
                        warn!(
                            "[BACKGROUND_TASK] Missing Teams service_url for conversation {} (task {}); falling back to GUI-only approvals",
                            conversation_id, task_id
                        );
                    }
                }
            } else {
                warn!(
                    "[BACKGROUND_TASK] Missing Teams sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            }
        }
        Some(ChannelSource::Matrix { room_id }) => {
            if let Some(requester_user_id) = source_sender_id.as_deref() {
                if requester_user_id.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Empty Matrix sender id for task {}; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    let (homeserver_url, access_token) = {
                        let cfg = state.config.read().await;
                        (
                            cfg.matrix.homeserver_url.clone(),
                            cfg.matrix.access_token.clone(),
                        )
                    };
                    if room_id.trim().is_empty() {
                        warn!(
                            "[BACKGROUND_TASK] Empty Matrix room id for task {}; falling back to GUI-only approvals",
                            task_id
                        );
                    } else if homeserver_url.trim().is_empty() || access_token.trim().is_empty() {
                        warn!(
                            "[BACKGROUND_TASK] Matrix source task {} has missing homeserver_url or access_token; falling back to GUI-only approvals",
                            task_id
                        );
                    } else {
                        matrix_observer = Some(MatrixObserver::new(
                            homeserver_url,
                            access_token,
                            room_id.clone(),
                            requester_user_id.to_string(),
                            state.matrix_pending_approvals.clone(),
                        ));
                    }
                }
            } else {
                warn!(
                    "[BACKGROUND_TASK] Missing Matrix sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            }
        }
        Some(ChannelSource::BlueBubbles { chat_guid }) => {
            if chat_guid.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty BlueBubbles chat guid for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else if let Some(requester_handle) = source_sender_id.as_deref() {
                if requester_handle.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Empty BlueBubbles sender handle for task {}; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    let (server_url, password) = {
                        let cfg = state.config.read().await;
                        (
                            cfg.bluebubbles.server_url.clone(),
                            cfg.bluebubbles.password.clone(),
                        )
                    };
                    if server_url.trim().is_empty() || password.trim().is_empty() {
                        warn!(
                            "[BACKGROUND_TASK] BlueBubbles source task {} has missing server_url/password; falling back to GUI-only approvals",
                            task_id
                        );
                    } else {
                        bluebubbles_observer = Some(BlueBubblesObserver::new(
                            state.clone(),
                            chat_guid.clone(),
                            requester_handle.to_string(),
                            true,
                            state.bluebubbles_pending_approvals.clone(),
                        ));
                    }
                }
            } else {
                warn!(
                    "[BACKGROUND_TASK] Missing BlueBubbles sender handle for task {}; falling back to GUI-only approvals",
                    task_id
                );
            }
        }
        Some(ChannelSource::Mattermost { channel_id }) => {
            if channel_id.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty Mattermost channel id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else if let Some(requester_user_id) = source_sender_id.as_deref() {
                if requester_user_id.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Empty Mattermost sender id for task {}; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    let (server_url, bot_token) = {
                        let cfg = state.config.read().await;
                        (
                            cfg.mattermost.server_url.clone(),
                            cfg.mattermost.bot_token.clone(),
                        )
                    };
                    if server_url.trim().is_empty() || bot_token.trim().is_empty() {
                        warn!(
                            "[BACKGROUND_TASK] Mattermost source task {} has missing server_url or bot_token; falling back to GUI-only approvals",
                            task_id
                        );
                    } else {
                        mattermost_observer = Some(MattermostObserver::new(
                            state.clone(),
                            channel_id.clone(),
                            requester_user_id.to_string(),
                            true,
                            state.mattermost_pending_approvals.clone(),
                        ));
                    }
                }
            } else {
                warn!(
                    "[BACKGROUND_TASK] Missing Mattermost sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            }
        }
        Some(ChannelSource::GoogleChat {
            space_id,
            sender_id,
        }) => {
            if space_id.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty Google Chat space id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else {
                let requester_user_id = source_sender_id
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(sender_id.as_str());
                if requester_user_id.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Empty Google Chat sender id for task {}; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    let webhook_url = {
                        let cfg = state.config.read().await;
                        cfg.google_chat.incoming_webhook_url.clone()
                    };
                    if webhook_url.trim().is_empty() {
                        warn!(
                            "[BACKGROUND_TASK] Google Chat source task {} has missing incoming_webhook_url; falling back to GUI-only approvals",
                            task_id
                        );
                    } else {
                        google_chat_observer = Some(GoogleChatObserver::new(
                            state.clone(),
                            space_id.clone(),
                            requester_user_id.to_string(),
                            true,
                            state.google_chat_pending_approvals.clone(),
                        ));
                    }
                }
            }
        }
        Some(ChannelSource::Twilio { phone_number }) => {
            let requester_phone = source_sender_id
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(phone_number.as_str());
            if requester_phone.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty Twilio sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else {
                let (account_sid, auth_token, from_number) = {
                    let cfg = state.config.read().await;
                    (
                        cfg.twilio.account_sid.clone(),
                        cfg.twilio.auth_token.clone(),
                        cfg.twilio.from_number.clone(),
                    )
                };
                if account_sid.trim().is_empty()
                    || auth_token.trim().is_empty()
                    || from_number.trim().is_empty()
                {
                    warn!(
                        "[BACKGROUND_TASK] Twilio source task {} has missing account_sid/auth_token/from_number; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    twilio_observer = Some(TwilioObserver::new(
                        state.clone(),
                        requester_phone.to_string(),
                        true,
                        state.twilio_pending_approvals.clone(),
                    ));
                }
            }
        }
        Some(ChannelSource::Messenger { sender_id }) => {
            let requester_sender_id = source_sender_id
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(sender_id.as_str());
            if requester_sender_id.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty Messenger sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else {
                let page_access_token = {
                    let cfg = state.config.read().await;
                    cfg.messenger.page_access_token.clone()
                };
                if page_access_token.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Messenger source task {} has missing page_access_token; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    messenger_observer = Some(MessengerObserver::new(
                        state.clone(),
                        requester_sender_id.to_string(),
                        true,
                        state.messenger_pending_approvals.clone(),
                    ));
                }
            }
        }
        Some(ChannelSource::Instagram { sender_id }) => {
            let requester_sender_id = source_sender_id
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(sender_id.as_str());
            if requester_sender_id.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty Instagram sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else {
                let (access_token, instagram_account_id) = {
                    let cfg = state.config.read().await;
                    (
                        cfg.instagram.access_token.clone(),
                        cfg.instagram.instagram_account_id.clone(),
                    )
                };
                if access_token.trim().is_empty() || instagram_account_id.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Instagram source task {} has missing access_token/instagram_account_id; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    instagram_observer = Some(InstagramObserver::new(
                        state.clone(),
                        requester_sender_id.to_string(),
                        true,
                        state.instagram_pending_approvals.clone(),
                    ));
                }
            }
        }
        Some(ChannelSource::Line {
            user_id,
            conversation_id,
        }) => {
            if conversation_id.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty LINE conversation id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else {
                let requester_user_id = source_sender_id
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(user_id.as_str());
                if requester_user_id.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Empty LINE sender id for task {}; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    let channel_access_token = {
                        let cfg = state.config.read().await;
                        cfg.line.channel_access_token.clone()
                    };
                    if channel_access_token.trim().is_empty() {
                        warn!(
                            "[BACKGROUND_TASK] LINE source task {} has missing channel_access_token; falling back to GUI-only approvals",
                            task_id
                        );
                    } else {
                        line_observer = Some(LineObserver::new(
                            state.clone(),
                            conversation_id.to_string(),
                            requester_user_id.to_string(),
                            true,
                            state.line_pending_approvals.clone(),
                        ));
                    }
                }
            }
        }
        Some(ChannelSource::RocketChat { room_id }) => {
            if room_id.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty Rocket.Chat room id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else if let Some(requester_user_id) = source_sender_id.as_deref() {
                if requester_user_id.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Empty Rocket.Chat sender id for task {}; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    let server_url = {
                        let cfg = state.config.read().await;
                        cfg.rocketchat.server_url.clone()
                    };
                    if server_url.trim().is_empty() {
                        warn!(
                            "[BACKGROUND_TASK] Rocket.Chat source task {} has missing server_url; falling back to GUI-only approvals",
                            task_id
                        );
                    } else {
                        let has_rest_auth = state.rocketchat_rest_auth.read().await.is_some();
                        if !has_rest_auth {
                            warn!(
                                "[BACKGROUND_TASK] Rocket.Chat source task {} has no cached REST auth; falling back to GUI-only approvals",
                                task_id
                            );
                        } else {
                            rocketchat_observer = Some(RocketChatObserver::new(
                                state.clone(),
                                room_id.clone(),
                                requester_user_id.to_string(),
                                true,
                                state.rocketchat_pending_approvals.clone(),
                            ));
                        }
                    }
                }
            } else {
                warn!(
                    "[BACKGROUND_TASK] Missing Rocket.Chat sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            }
        }
        Some(ChannelSource::Mastodon { account_id }) => {
            let requester_account_id = source_sender_id
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(account_id.as_str());
            if requester_account_id.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty Mastodon sender id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else {
                let (instance_url, access_token) = {
                    let cfg = state.config.read().await;
                    (
                        cfg.mastodon.instance_url.clone(),
                        cfg.mastodon.access_token.clone(),
                    )
                };
                if instance_url.trim().is_empty() || access_token.trim().is_empty() {
                    warn!(
                        "[BACKGROUND_TASK] Mastodon source task {} has missing instance_url/access_token; falling back to GUI-only approvals",
                        task_id
                    );
                } else {
                    let requester_acct = state
                        .mastodon_account_handles
                        .read()
                        .await
                        .get(requester_account_id)
                        .cloned()
                        .unwrap_or_default();
                    if requester_acct.trim().is_empty() {
                        warn!(
                            "[BACKGROUND_TASK] Missing Mastodon acct handle for account {} (task {}); falling back to GUI-only approvals",
                            requester_account_id, task_id
                        );
                    } else {
                        mastodon_observer = Some(MastodonObserver::new(
                            state.clone(),
                            requester_account_id.to_string(),
                            requester_acct,
                            true,
                            state.mastodon_pending_approvals.clone(),
                        ));
                    }
                }
            }
        }
        Some(ChannelSource::WebChat { session_id }) => {
            let requester_session_id = source_sender_id
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(session_id.as_str());
            if requester_session_id.trim().is_empty() {
                warn!(
                    "[BACKGROUND_TASK] Empty WebChat session id for task {}; falling back to GUI-only approvals",
                    task_id
                );
            } else {
                let has_active_session = state
                    .webchat_session_senders
                    .read()
                    .await
                    .contains_key(requester_session_id);
                if !has_active_session {
                    warn!(
                        "[BACKGROUND_TASK] No active WebChat socket for session {} (task {}); falling back to GUI-only approvals",
                        requester_session_id, task_id
                    );
                } else {
                    webchat_observer = Some(WebChatObserver::new(
                        state.clone(),
                        requester_session_id.to_string(),
                        state.webchat_pending_approvals.clone(),
                    ));
                }
            }
        }
        _ => {}
    }
    let observer: &dyn tool_loop::ToolLoopObserver = if let Some(o) = telegram_observer.as_ref() {
        o
    } else if let Some(o) = discord_observer.as_ref() {
        o
    } else if let Some(o) = slack_observer.as_ref() {
        o
    } else if let Some(o) = signal_observer.as_ref() {
        o
    } else if let Some(o) = whatsapp_observer.as_ref() {
        o
    } else if let Some(o) = teams_observer.as_ref() {
        o
    } else if let Some(o) = matrix_observer.as_ref() {
        o
    } else if let Some(o) = bluebubbles_observer.as_ref() {
        o
    } else if let Some(o) = mattermost_observer.as_ref() {
        o
    } else if let Some(o) = google_chat_observer.as_ref() {
        o
    } else if let Some(o) = twilio_observer.as_ref() {
        o
    } else if let Some(o) = messenger_observer.as_ref() {
        o
    } else if let Some(o) = instagram_observer.as_ref() {
        o
    } else if let Some(o) = line_observer.as_ref() {
        o
    } else if let Some(o) = rocketchat_observer.as_ref() {
        o
    } else if let Some(o) = mastodon_observer.as_ref() {
        o
    } else if let Some(o) = webchat_observer.as_ref() {
        o
    } else {
        &background_observer
    };
    let options = RouteOptions {
        claude_client: &bg_client,
        overrides: SessionOverrides::default(),
        loop_config: ToolLoopConfig::background_with_origin(source_channel, source_sender_id),
        observer,
        streaming: false,
        window: None,
        on_stream_chunk: None,
        auto_compact: false,
        save_to_memory: false,
        sync_supermemory: false,
        check_sensitive_data: true,
    };

    match router::route_message(&message, options, &state).await {
        Ok(routed) => {
            // Read notify_target before completing the task (field stays accessible after)
            let notify_target = state
                .task_manager
                .read()
                .await
                .get_task(&task_id)
                .and_then(|t| t.notify_target.clone());
            let mut tm = state.task_manager.write().await;
            tm.complete_task(&task_id, &routed.text);
            drop(tm);
            if let Some(ref h) = app_handle {
                if let Err(e) = h.emit(
                    "task:complete",
                    serde_json::json!({ "task_id": task_id, "summary": routed.text }),
                ) {
                    warn!("[BACKGROUND_TASK] Failed to emit task:complete: {}", e);
                }
            }
            if let Some(ref target) = notify_target {
                let attempted = state
                    .notification_dispatcher
                    .dispatch(target, &format!("✅ Task complete: {}", routed.text))
                    .await;
                if !attempted {
                    warn!(
                        "[BACKGROUND_TASK] No notification delivery target available for completed task {}",
                        task_id
                    );
                }
            }
            info!("[BACKGROUND_TASK] Task {} completed", task_id);
        }
        Err(e) => {
            error!("[BACKGROUND_TASK] Task {} failed: {}", task_id, e);
            let notify_target = state
                .task_manager
                .read()
                .await
                .get_task(&task_id)
                .and_then(|t| t.notify_target.clone());
            let mut tm = state.task_manager.write().await;
            tm.fail_task(&task_id, &e.to_string());
            drop(tm);
            if let Some(ref h) = app_handle {
                if let Err(emit_err) = h.emit(
                    "task:failed",
                    serde_json::json!({ "task_id": task_id, "error": e.to_string() }),
                ) {
                    warn!("[BACKGROUND_TASK] Failed to emit task:failed: {}", emit_err);
                }
            }
            if let Some(ref target) = notify_target {
                let attempted = state
                    .notification_dispatcher
                    .dispatch(target, &format!("❌ Background task failed: {}", e))
                    .await;
                if !attempted {
                    warn!(
                        "[BACKGROUND_TASK] No notification delivery target available for failed task {}",
                        task_id
                    );
                }
            }
        }
    }
}

// --- Context Management Commands ---

#[derive(Debug, Serialize)]
pub struct ContextMetricsResponse {
    pub state: String,
    pub total_compactions: u32,
    pub compactions_today: u32,
    pub total_messages_removed: u32,
    pub total_tokens_freed: u32,
    pub avg_compression_ratio: f32,
    pub last_compaction_time: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CompactionHistoryEvent {
    pub timestamp: String,
    pub messages_before: usize,
    pub messages_after: usize,
    pub tokens_freed: u32,
    pub summary_preview: String,
}

/// Get current context usage and compaction metrics.
#[tauri::command]
pub async fn get_context_metrics(
    state: State<'_, AppState>,
) -> Result<ContextMetricsResponse, String> {
    let metrics = state.context_manager.get_metrics();
    let ctx_state = state.context_manager.get_state();

    Ok(ContextMetricsResponse {
        state: format!("{:?}", ctx_state),
        total_compactions: metrics.total_compactions,
        compactions_today: metrics.compactions_today,
        total_messages_removed: metrics.total_messages_removed,
        total_tokens_freed: metrics.total_tokens_freed,
        avg_compression_ratio: metrics.avg_compression_ratio,
        last_compaction_time: metrics.last_compaction_time.map(|t| t.to_rfc3339()),
    })
}

/// Get compaction history (last N events).
#[tauri::command]
pub async fn get_compaction_history(
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<CompactionHistoryEvent>, String> {
    let limit = limit.unwrap_or(20);
    let history = state.context_manager.get_compaction_history(limit);

    Ok(history
        .into_iter()
        .map(|e| CompactionHistoryEvent {
            timestamp: e.timestamp.to_rfc3339(),
            messages_before: e.messages_before,
            messages_after: e.messages_after,
            tokens_freed: e.tokens_freed,
            summary_preview: e.summary_preview,
        })
        .collect())
}

/// Reset daily compaction counter (usually called once per day by the frontend).
#[tauri::command]
pub async fn reset_compaction_counter(state: State<'_, AppState>) -> Result<(), String> {
    state
        .context_manager
        .reset_daily_counter()
        .map_err(|e| e.to_string())
}

/// Signal the backend to stop the current tool loop at the next iteration.
/// Also signals the frontend abort (handled client-side via Promise.race).
#[tauri::command]
pub async fn cancel_message() -> Result<(), String> {
    info!("Message cancellation requested by user — setting cancel flag");
    ACTIVE_CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::NotificationTarget;
    use async_trait::async_trait;

    struct ApprovingObserver;

    #[async_trait]
    impl crate::tool_loop::ToolLoopObserver for ApprovingObserver {
        fn supports_approval(&self) -> bool {
            true
        }

        async fn request_approval(&self, _tool_name: &str, _reason: &str) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn test_require_tool_approval_or_block_blocks_without_approval_path() {
        let observer = crate::tool_loop::NoOpObserver;
        let result = require_tool_approval_or_block(
            "nexibot_filesystem",
            "Filesystem write requires confirmation",
            None,
            Some(&observer),
        )
        .await;
        assert!(result.is_err());
        let msg = result.err().unwrap_or_default();
        assert!(msg.contains("requires user confirmation"));
        assert!(msg.contains("no in-channel approval path is available"));
        assert!(msg.contains("supports in-channel approve/deny replies"));
    }

    #[tokio::test]
    async fn test_require_tool_approval_or_block_allows_with_approving_observer() {
        let observer = ApprovingObserver;
        let result = require_tool_approval_or_block(
            "nexibot_execute",
            "Execution requires confirmation",
            None,
            Some(&observer),
        )
        .await;
        assert!(result.is_ok());
    }

    // ── parse_notify_target ───────────────────────────────────────────

    #[test]
    fn test_parse_no_field_returns_none() {
        let input = serde_json::json!({ "task_description": "do stuff" });
        assert_eq!(parse_notify_target(&input), None);
    }

    #[test]
    fn test_parse_legacy_notify_telegram_false_returns_none() {
        let input = serde_json::json!({ "notify_telegram": false });
        assert_eq!(parse_notify_target(&input), None);
    }

    #[test]
    fn test_parse_legacy_notify_telegram_true_returns_telegram_configured() {
        let input = serde_json::json!({ "notify_telegram": true });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::TelegramConfigured)
        );
    }

    #[test]
    fn test_parse_structured_all_configured() {
        let input = serde_json::json!({ "notify_target": { "type": "all_configured" } });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::AllConfigured)
        );
    }

    #[test]
    fn test_parse_structured_gui() {
        let input = serde_json::json!({ "notify_target": { "type": "gui" } });
        assert_eq!(parse_notify_target(&input), Some(NotificationTarget::Gui));
    }

    #[test]
    fn test_parse_structured_telegram_with_chat_id() {
        let input = serde_json::json!({
            "notify_target": { "type": "telegram", "chat_id": -100123456 }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Telegram {
                chat_id: -100123456
            })
        );
    }

    #[test]
    fn test_parse_structured_telegram_missing_chat_id_returns_none() {
        let input = serde_json::json!({ "notify_target": { "type": "telegram" } });
        assert_eq!(parse_notify_target(&input), None);
    }

    #[test]
    fn test_parse_structured_telegram_configured() {
        let input = serde_json::json!({ "notify_target": { "type": "telegram_configured" } });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::TelegramConfigured)
        );
    }

    #[test]
    fn test_parse_structured_telegram_string_chat_id() {
        let input = serde_json::json!({
            "notify_target": { "type": "telegram", "chat_id": "-100123456" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Telegram {
                chat_id: -100123456
            })
        );
    }

    #[test]
    fn test_parse_structured_telegram_zero_chat_id_returns_none() {
        let input = serde_json::json!({
            "notify_target": { "type": "telegram", "chat_id": 0 }
        });
        assert_eq!(parse_notify_target(&input), None);
    }

    #[test]
    fn test_parse_structured_discord_with_channel_id() {
        let input = serde_json::json!({
            "notify_target": { "type": "discord", "channel_id": 987654321_u64 }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Discord {
                channel_id: 987654321
            })
        );
    }

    #[test]
    fn test_parse_structured_discord_missing_channel_id_returns_none() {
        let input = serde_json::json!({ "notify_target": { "type": "discord" } });
        assert_eq!(parse_notify_target(&input), None);
    }

    #[test]
    fn test_parse_structured_discord_string_channel_id() {
        let input = serde_json::json!({
            "notify_target": { "type": "discord", "channel_id": "987654321" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Discord {
                channel_id: 987654321
            })
        );
    }

    #[test]
    fn test_parse_structured_discord_zero_channel_id_returns_none() {
        let input = serde_json::json!({
            "notify_target": { "type": "discord", "channel_id": 0 }
        });
        assert_eq!(parse_notify_target(&input), None);
    }

    #[test]
    fn test_parse_structured_slack_with_channel_id() {
        let input = serde_json::json!({
            "notify_target": { "type": "slack", "channel_id": "C0ABC1234" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Slack {
                channel_id: "C0ABC1234".to_string()
            })
        );
    }

    #[test]
    fn test_parse_structured_slack_missing_channel_id_returns_none() {
        let input = serde_json::json!({ "notify_target": { "type": "slack" } });
        assert_eq!(parse_notify_target(&input), None);
    }

    #[test]
    fn test_parse_structured_whatsapp_with_phone_number() {
        let input = serde_json::json!({
            "notify_target": { "type": "whatsapp", "phone_number": "15551234567" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::WhatsApp {
                phone_number: "15551234567".to_string()
            })
        );
    }

    #[test]
    fn test_parse_structured_signal_with_phone_number() {
        let input = serde_json::json!({
            "notify_target": { "type": "signal", "phone_number": "+15557654321" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Signal {
                phone_number: "+15557654321".to_string()
            })
        );
    }

    #[test]
    fn test_parse_structured_matrix_with_room_id() {
        let input = serde_json::json!({
            "notify_target": { "type": "matrix", "room_id": "!roomid:matrix.org" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Matrix {
                room_id: "!roomid:matrix.org".to_string()
            })
        );
    }

    #[test]
    fn test_parse_structured_mattermost_with_channel_id() {
        let input = serde_json::json!({
            "notify_target": { "type": "mattermost", "channel_id": "channel-id" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Mattermost {
                channel_id: "channel-id".to_string()
            })
        );
    }

    #[test]
    fn test_parse_structured_google_chat() {
        let input = serde_json::json!({
            "notify_target": { "type": "google_chat" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::GoogleChat)
        );
    }

    #[test]
    fn test_parse_structured_bluebubbles_with_chat_guid() {
        let input = serde_json::json!({
            "notify_target": { "type": "bluebubbles", "chat_guid": "iMessage;-;+15551234567" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::BlueBubbles {
                chat_guid: "iMessage;-;+15551234567".to_string()
            })
        );
    }

    #[test]
    fn test_parse_structured_bluebubbles_missing_chat_guid_returns_none() {
        let input = serde_json::json!({ "notify_target": { "type": "bluebubbles" } });
        assert_eq!(parse_notify_target(&input), None);
    }

    #[test]
    fn test_parse_structured_messenger_with_recipient_id() {
        let input = serde_json::json!({
            "notify_target": { "type": "messenger", "recipient_id": "1234567890" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Messenger {
                recipient_id: "1234567890".to_string()
            })
        );
    }

    #[test]
    fn test_parse_structured_instagram_with_recipient_id() {
        let input = serde_json::json!({
            "notify_target": { "type": "instagram", "recipient_id": "17841400000000000" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Instagram {
                recipient_id: "17841400000000000".to_string()
            })
        );
    }

    #[test]
    fn test_parse_structured_line_with_user_id() {
        let input = serde_json::json!({
            "notify_target": { "type": "line", "user_id": "U1234567890abcdef" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Line {
                user_id: "U1234567890abcdef".to_string()
            })
        );
    }

    #[test]
    fn test_parse_structured_twilio_with_phone_number() {
        let input = serde_json::json!({
            "notify_target": { "type": "twilio", "phone_number": "+15551234567" }
        });
        assert_eq!(
            parse_notify_target(&input),
            Some(NotificationTarget::Twilio {
                phone_number: "+15551234567".to_string()
            })
        );
    }

    #[test]
    fn test_parse_structured_unknown_type_returns_none() {
        let input = serde_json::json!({ "notify_target": { "type": "carrier_pigeon" } });
        // Unknown type → None (callers should treat as no notification)
        assert_eq!(parse_notify_target(&input), None);
    }

    #[test]
    fn test_structured_takes_priority_over_legacy() {
        // If both fields are present, notify_target wins over notify_telegram
        let input = serde_json::json!({
            "notify_target": { "type": "gui" },
            "notify_telegram": true
        });
        assert_eq!(parse_notify_target(&input), Some(NotificationTarget::Gui));
    }

    #[test]
    fn test_unknown_structured_target_does_not_fall_back_to_legacy() {
        let input = serde_json::json!({
            "notify_target": { "type": "carrier_pigeon" },
            "notify_telegram": true
        });
        assert_eq!(parse_notify_target(&input), None);
    }

    #[test]
    fn test_malformed_structured_telegram_target_does_not_fall_back_to_legacy() {
        let input = serde_json::json!({
            "notify_target": { "type": "telegram", "chat_id": "not-a-number" },
            "notify_telegram": true
        });
        assert_eq!(parse_notify_target(&input), None);
    }

    // ── NEXIBOT_DETACH sentinel format ────────────────────────────────

    #[test]
    fn test_detach_sentinel_format() {
        // Verify the sentinel can be reliably parsed by the tool loop.
        // Format: "NEXIBOT_DETACH:{task_id}:{acknowledgment}"
        // The acknowledgment may contain colons, so we split only on the second colon.
        let task_id = "abc12345";
        let ack = "Got it! I'll research that and let you know: watch this space.";
        let sentinel = format!("NEXIBOT_DETACH:{}:{}", task_id, ack);

        assert!(sentinel.starts_with("NEXIBOT_DETACH:"));
        let without_prefix = &sentinel["NEXIBOT_DETACH:".len()..];
        let colon_pos = without_prefix.find(':').expect("second colon exists");
        let parsed_task_id = &without_prefix[..colon_pos];
        let parsed_ack = &without_prefix[colon_pos + 1..];

        assert_eq!(parsed_task_id, task_id);
        assert_eq!(parsed_ack, ack); // colons in ack text are preserved
    }
}
