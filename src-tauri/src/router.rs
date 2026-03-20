//! Unified message router for all channels.
//!
//! Encapsulates the common orchestration pipeline shared across GUI, Telegram,
//! WhatsApp, and Voice channels. Each channel provides:
//! - A `ClaudeClient` (possibly per-session)
//! - A `ToolLoopConfig` + `ToolLoopObserver` (channel-specific feedback)
//!
//! The router handles everything in between: safety checks, tool collection,
//! auto-compact, LLM call, tool loop, sensitive-data checks, session save,
//! and supermemory sync.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::channel::ChannelSource;
use crate::claude::ClaudeClient;
use crate::commands::chat;
use crate::commands::memory::save_session_history;
use crate::commands::AppState;
use crate::security::external_content;
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::{ToolLoopConfig, ToolLoopObserver};

/// An incoming message from any channel, normalized for the router.
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    /// The user's text.
    pub text: String,
    /// Which channel this came from.
    pub channel: ChannelSource,
    /// Explicit agent ID override (e.g., from GUI agent selector).
    pub agent_id: Option<String>,
    /// Additional metadata (channel-specific).
    #[allow(dead_code)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// The result of routing a message through the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutedResponse {
    /// The final response text.
    pub text: String,
    /// Which agent handled the message.
    pub agent_id: String,
    /// How many tool-use iterations occurred.
    pub tool_calls_made: usize,
    /// Which model actually ran.
    pub model_used: String,
}

/// Errors that can occur during routing.
#[derive(Debug)]
pub enum RouterError {
    /// Message blocked by defense/guardrails.
    Blocked(String),
    /// LLM call failed.
    LlmError(String),
    /// Tool loop failed.
    ToolLoopError(String),
}

impl std::fmt::Display for RouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouterError::Blocked(msg) => write!(f, "Blocked: {}", msg),
            RouterError::LlmError(msg) => write!(f, "LLM error: {}", msg),
            RouterError::ToolLoopError(msg) => write!(f, "Tool loop error: {}", msg),
        }
    }
}

/// Options controlling how the router processes a message.
pub struct RouteOptions<'a> {
    /// The Claude client to use (caller provides, since session mgmt is channel-specific).
    pub claude_client: &'a ClaudeClient,
    /// Session overrides (thinking budget, etc.).
    pub overrides: SessionOverrides,
    /// Tool loop configuration (channel-specific timeouts, iterations, etc.).
    pub loop_config: ToolLoopConfig,
    /// Tool loop observer (channel-specific progress feedback).
    pub observer: &'a dyn ToolLoopObserver,
    /// Whether to stream the initial LLM call (GUI streaming mode).
    pub streaming: bool,
    /// Tauri window for auto-compact events (GUI only).
    pub window: Option<&'a tauri::Window>,
    /// Optional streaming callback for the initial LLM call.
    /// Note: ClaudeClient expects `impl Fn(String) + Send + Sync + 'static`.
    pub on_stream_chunk: Option<Box<dyn Fn(String) + Send + Sync + 'static>>,
    /// Whether to run auto-compact before the message.
    pub auto_compact: bool,
    /// Whether to save messages to memory.
    pub save_to_memory: bool,
    /// Whether to sync to supermemory after response.
    pub sync_supermemory: bool,
    /// Whether to check output for sensitive data.
    pub check_sensitive_data: bool,
}

impl<'a> RouteOptions<'a> {
    /// Create default options for a given client and observer.
    #[allow(dead_code)]
    pub fn new(
        claude_client: &'a ClaudeClient,
        loop_config: ToolLoopConfig,
        observer: &'a dyn ToolLoopObserver,
    ) -> Self {
        Self {
            claude_client,
            overrides: SessionOverrides::default(),
            loop_config,
            observer,
            streaming: false,
            window: None,
            on_stream_chunk: None,
            auto_compact: true,
            save_to_memory: true,
            sync_supermemory: true,
            check_sensitive_data: true,
        }
    }
}

/// Route a message through the unified pipeline.
///
/// This is the ONE entry point for all channels. The pipeline:
/// 1. Safety check (defense + guardrails)
/// 2. Auto-compact (if enabled)
/// 3. Collect tools
/// 4. Save user message (if enabled)
/// 5. Initial LLM call
/// 6. Tool loop
/// 7. Sensitive data check (if enabled)
/// 8. Save assistant response (if enabled)
/// 9. Supermemory sync (if enabled)
pub async fn route_message(
    message: &IncomingMessage,
    options: RouteOptions<'_>,
    state: &AppState,
) -> Result<RoutedResponse, RouterError> {
    // KILLSWITCH CHECK: Verify agent is running before processing
    use tracing::warn;
    match state.agent_control.get_state() {
        crate::agent_control::AgentState::Stopped => {
            warn!("[KILLSWITCH] Message rejected: agent is stopped");
            return Err(RouterError::Blocked(
                "Agent is stopped (killswitch activated)".to_string(),
            ));
        }
        crate::agent_control::AgentState::Paused => {
            // Queue the message instead of processing (could implement message queue here)
            warn!("[KILLSWITCH] Message queued: agent is paused");
            // For now, reject with informative message
            return Err(RouterError::Blocked(
                "Agent is paused (messages queued)".to_string(),
            ));
        }
        crate::agent_control::AgentState::Running => {
            // Continue normal processing
        }
    }

    // 0. External content wrapping for non-GUI channels
    // Messages from external channels (Telegram, WhatsApp, Discord, etc.) are
    // untrusted input and must be wrapped with boundary markers to prevent
    // prompt injection attacks.
    let safe_text = match &message.channel {
        ChannelSource::Gui | ChannelSource::InterAgent { .. } => message.text.clone(),
        other => {
            // Detect suspicious patterns in incoming channel messages
            let findings = external_content::detect_suspicious_patterns(&message.text);
            if !findings.is_empty() {
                let critical_count = findings
                    .iter()
                    .filter(|f| {
                        matches!(
                            f.severity,
                            external_content::PatternSeverity::Critical
                                | external_content::PatternSeverity::High
                        )
                    })
                    .count();
                if critical_count > 0 {
                    warn!(
                        "[ROUTER] Detected {} critical/high suspicious patterns in {:?} message",
                        critical_count, message.channel
                    );
                }
            }
            // Wrap with boundary markers
            let source_label = format!("Channel message: {:?}", other);
            external_content::wrap_external_content(&message.text, &source_label)
        }
    };

    // 0.5. Key vault interception — sanitize the text before the LLM sees it.
    // Real API keys typed/pasted into chat are replaced with proxy keys. The
    // safety check below runs on the ORIGINAL (pre-vault) message text so that
    // injection patterns in real keys are still caught.
    let safe_text = {
        let kv_config = {
            let cfg = state.config.read().await;
            cfg.key_vault.clone()
        };
        if kv_config.intercept_chat_input {
            state.key_interceptor.intercept_message(&safe_text)
        } else {
            safe_text
        }
    };

    // 1. Safety check — run on the ORIGINAL message text, not the wrapped version.
    // The boundary markers added by external_content::wrap_external_content can
    // trigger DeBERTa's prompt injection detector (false positive), which would
    // block all external channel messages. The wrapping is for the LLM's benefit,
    // not for defense input.
    if let Err(resp) = chat::check_input_safety(&message.text, state).await {
        let error_msg = resp
            .error
            .unwrap_or_else(|| "Message blocked by safety checks".to_string());
        return Err(RouterError::Blocked(error_msg));
    }

    // 2. Auto-compact
    if options.auto_compact {
        chat::maybe_auto_compact(state, options.window, Some(options.claude_client)).await;
    }

    // 3. Collect tools (pass user message for semantic MCP tool filtering)
    let (all_tools, mcp_count, computer_use_enabled, browser_enabled) =
        chat::collect_all_tools(state, Some(&message.text)).await;
    if !all_tools.is_empty() {
        info!(
            "[ROUTER] Passing {} tools (MCP: {}, CU: {}, Browser: {}, Built-in: rest)",
            all_tools.len(),
            mcp_count,
            if computer_use_enabled { 1 } else { 0 },
            if browser_enabled { 1 } else { 0 }
        );
    }

    // 4. Save user message (save original text, not wrapped version)
    if options.save_to_memory {
        chat::auto_save_message(state, "user", &message.text).await;
    }

    // 5. Resolve agent ID (for metadata; caller already provides the client)
    let agent_id = resolve_agent_id(message, state).await;

    // 6. Initial LLM call — with single fallback attempt on failover-eligible errors.
    let initial_result = {
        use crate::providers::is_failover_eligible;

        // First attempt: use streaming callback if provided (callback is consumed here).
        let first_try = if options.streaming {
            if let Some(callback) = options.on_stream_chunk {
                options
                    .claude_client
                    .send_message_streaming_with_tools(
                        &safe_text,
                        &all_tools,
                        &options.overrides,
                        callback,
                    )
                    .await
            } else {
                options
                    .claude_client
                    .send_message_with_tools(&safe_text, &all_tools, &options.overrides)
                    .await
            }
        } else {
            options
                .claude_client
                .send_message_with_tools(&safe_text, &all_tools, &options.overrides)
                .await
        };

        match first_try {
            Ok(result) => Ok(result),
            Err(ref e) if is_failover_eligible(e) => {
                // Look up fallback model from config
                let (primary_model, fallback_model) = {
                    let cfg = state.config.read().await;
                    (cfg.claude.model.clone(), cfg.claude.fallback_model.clone())
                };
                if let Some(ref fbm) = fallback_model {
                    let reason = e.to_string();
                    options
                        .observer
                        .on_model_fallback(&primary_model, fbm, &reason)
                        .await;
                    // Retry non-streaming with fallback model override
                    let mut fallback_overrides = options.overrides.clone();
                    fallback_overrides.model = Some(fbm.clone());
                    options
                        .claude_client
                        .send_message_with_tools(&safe_text, &all_tools, &fallback_overrides)
                        .await
                        .map_err(|e2| RouterError::LlmError(e2.to_string()))
                } else {
                    first_try.map_err(|e| RouterError::LlmError(e.to_string()))
                }
            }
            Err(e) => Err(RouterError::LlmError(e.to_string())),
        }
    }?;

    // Capture model_used immediately after the initial LLM call, before the
    // tool loop runs (which may take time and allow concurrent requests to
    // overwrite current_routing_model).
    let model_used = options
        .claude_client
        .get_current_model()
        .await
        .unwrap_or_default();

    // 7. Tool loop
    let result = crate::tool_loop::execute_tool_loop(
        options.claude_client,
        &all_tools,
        &options.overrides,
        initial_result,
        &options.loop_config,
        state,
        options.observer,
    )
    .await
    .map_err(RouterError::ToolLoopError)?;

    let response_text = result.text.clone();

    // 8. Sensitive data check
    if options.check_sensitive_data {
        let guardrails = state.guardrails.read().await;
        if let Err(violations) = guardrails.check_sensitive_data(&response_text, "LLM response") {
            warn!(
                "[ROUTER] Sensitive data in response: {} violations",
                violations.len()
            );
        }
    }

    // 9. Save assistant response
    if options.save_to_memory {
        chat::auto_save_message(state, "assistant", &response_text).await;
    }

    // 10. Persist full Claude history for the active session (enables /resume across channels).
    // Best-effort: failure is logged but does not abort the response.
    {
        let session_id = state.memory_manager.read().await.get_current_session_id();
        if let Some(ref sid) = session_id {
            if let Err(e) = save_session_history(options.claude_client, sid).await {
                warn!("[ROUTER] Failed to save session history: {}", e);
            }
        }
    }

    // 11. Supermemory sync + local fact extraction
    if options.sync_supermemory {
        chat::try_sync_supermemory(state).await;
        chat::try_extract_session_facts(state).await;
    }

    Ok(RoutedResponse {
        text: response_text,
        agent_id,
        tool_calls_made: result.tool_calls_made,
        model_used,
    })
}

/// Resolve which agent should handle this message.
async fn resolve_agent_id(message: &IncomingMessage, state: &AppState) -> String {
    if let Some(ref explicit_id) = message.agent_id {
        return explicit_id.clone();
    }

    let agent_mgr = state.agent_manager.read().await;
    match &message.channel {
        ChannelSource::Telegram { chat_id } => agent_mgr
            .resolve_agent("telegram", &chat_id.to_string())
            .to_string(),
        ChannelSource::WhatsApp { phone_number } => agent_mgr
            .resolve_agent("whatsapp", phone_number)
            .to_string(),
        ChannelSource::Gui => agent_mgr.active_gui_agent_id.clone(),
        ChannelSource::Voice => agent_mgr.active_gui_agent_id.clone(),
        ChannelSource::InterAgent { agent_id } => agent_id.clone(),
        ChannelSource::Discord { channel_id, .. } => agent_mgr
            .resolve_agent("discord", &channel_id.to_string())
            .to_string(),
        ChannelSource::Slack { channel_id } => {
            agent_mgr.resolve_agent("slack", channel_id).to_string()
        }
        ChannelSource::Signal { phone_number } => {
            agent_mgr.resolve_agent("signal", phone_number).to_string()
        }
        ChannelSource::Teams { conversation_id } => agent_mgr
            .resolve_agent("teams", conversation_id)
            .to_string(),
        ChannelSource::Matrix { room_id } => agent_mgr.resolve_agent("matrix", room_id).to_string(),
        ChannelSource::BlueBubbles { chat_guid } => agent_mgr
            .resolve_agent("bluebubbles", chat_guid)
            .to_string(),
        ChannelSource::GoogleChat { space_id, .. } => {
            agent_mgr.resolve_agent("google_chat", space_id).to_string()
        }
        ChannelSource::Mattermost { channel_id } => agent_mgr
            .resolve_agent("mattermost", channel_id)
            .to_string(),
        ChannelSource::Messenger { sender_id } => {
            agent_mgr.resolve_agent("messenger", sender_id).to_string()
        }
        ChannelSource::Instagram { sender_id } => {
            agent_mgr.resolve_agent("instagram", sender_id).to_string()
        }
        ChannelSource::Line { user_id, .. } => agent_mgr.resolve_agent("line", user_id).to_string(),
        ChannelSource::Twilio { phone_number } => {
            agent_mgr.resolve_agent("twilio", phone_number).to_string()
        }
        ChannelSource::Mastodon { account_id } => {
            agent_mgr.resolve_agent("mastodon", account_id).to_string()
        }
        ChannelSource::RocketChat { room_id } => {
            agent_mgr.resolve_agent("rocketchat", room_id).to_string()
        }
        ChannelSource::WebChat { session_id } => {
            agent_mgr.resolve_agent("webchat", session_id).to_string()
        }
        ChannelSource::Email { thread_id } => {
            agent_mgr.resolve_agent("email", thread_id).to_string()
        }
        ChannelSource::Gmail { thread_id } => {
            agent_mgr.resolve_agent("gmail", thread_id).to_string()
        }
    }
}

/// Extract clean text from a response that might contain raw JSON content blocks.
/// This handles the case where result.text looks like `[{"type":"text","text":"..."},...]`.
pub fn extract_text_from_response(text: &str) -> String {
    let trimmed = text.trim();

    // If it looks like a JSON array of content blocks, extract text from them
    if trimmed.starts_with('[') {
        if let Ok(blocks) = serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
            let extracted: Vec<&str> = blocks
                .iter()
                .filter_map(|b| {
                    if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                        b.get("text").and_then(|t| t.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            if !extracted.is_empty() {
                return extracted.join("\n");
            }
        }
    }

    text.to_string()
}

/// Split a message into chunks of max_len characters, respecting word boundaries.
pub fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }

        // Find a UTF-8-safe byte boundary (max_len may land inside a multi-byte char)
        let mut safe_end = max_len.min(remaining.len());
        while safe_end > 0 && !remaining.is_char_boundary(safe_end) {
            safe_end -= 1;
        }
        if safe_end == 0 {
            // Degenerate case: single char wider than max_len; take one char
            safe_end = remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
        }

        // Find a good split point (newline or space)
        let split_at = remaining[..safe_end]
            .rfind('\n')
            .or_else(|| remaining[..safe_end].rfind(' '))
            .unwrap_or(safe_end);

        chunks.push(remaining[..split_at].to_string());
        remaining = remaining[split_at..].trim_start();
    }

    chunks
}
