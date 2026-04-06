//! Unified tool-use loop for all channels.
//!
//! Extracts the common agentic tool-use loop pattern from chat.rs, telegram.rs,
//! whatsapp.rs, voice/mod.rs into a single configurable implementation.
//! Channel-specific behavior (typing indicators, progress messages, TTS streaming)
//! is handled via the `ToolLoopObserver` trait.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Maximum number of MCP tools that can be dynamically added to the active tool set
/// within a single tool-loop execution. Prevents unbounded context window growth
/// when many distinct MCP tool names are referenced across iterations.
const MAX_ACTIVE_MCP_TOOLS: usize = 50;

use async_trait::async_trait;
use futures_util::future::join_all;
use tracing::{debug, error, info, warn};

use crate::channel::ChannelSource;
use crate::claude::{ClaudeClient, ClaudeMessageResult, TOOL_PAIRING_ERRORS, TOOL_PAIRING_RECOVERIES};
use crate::commands::chat;
use crate::commands::AppState;
use crate::config::NexiBotConfig;
use crate::security::dangerous_tools;
use crate::session_overrides::SessionOverrides;
use crate::tool_retry::{self, ToolErrorInfo, ToolErrorKind};

/// Configuration for a tool-use loop invocation.
#[derive(Debug, Clone)]
pub struct ToolLoopConfig {
    /// Maximum number of tool-use iterations before stopping.
    pub max_iterations: usize,
    /// Overall timeout for the loop. None = unlimited.
    pub timeout: Option<Duration>,
    /// Maximum cumulative output bytes across all tool results.
    pub max_output_bytes: usize,
    /// Maximum bytes per tool result stored in conversation history.
    /// Results exceeding this are truncated. None = no truncation.
    pub max_tool_result_bytes: Option<usize>,
    /// If true, force a final summary call when the loop exhausts iterations
    /// without producing a text response.
    pub force_summary_on_exhaustion: bool,
    /// Channel source identifier for logging.
    pub channel: Option<ChannelSource>,
    /// Whether to run defense pipeline checks on tool results.
    pub run_defense_checks: bool,
    /// Whether to use streaming for continue_after_tools calls.
    pub streaming: bool,
    /// Normalized sender ID for admin bypass checks. None for local channels.
    pub sender_id: Option<String>,
    /// Milliseconds to sleep between processing individual tool calls in a batch.
    /// A small value (≥50 ms) prevents rapid back-to-back API state mutations that
    /// can trigger "unexpected tool_use_id" 400 errors. Default: 0 (no delay).
    pub between_tool_delay_ms: u64,
}

impl ToolLoopConfig {
    /// GUI default (streaming): 10 iterations, 300s timeout, defense checks on.
    pub fn gui_default() -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Gui),
            run_defense_checks: true,
            streaming: true,
            sender_id: None,
            between_tool_delay_ms: 50,
        }
    }

    /// GUI non-streaming: 10 iterations, 300s timeout, defense checks on.
    pub fn gui_non_streaming() -> Self {
        Self {
            streaming: false,
            ..Self::gui_default()
        }
    }

    /// Telegram: 25 iterations, 600s timeout, 8KB truncation, force summary.
    pub fn telegram(chat_id: i64) -> Self {
        Self {
            max_iterations: 25,
            timeout: Some(Duration::from_secs(600)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: Some(8_000),
            force_summary_on_exhaustion: true,
            channel: Some(ChannelSource::Telegram { chat_id }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(chat_id.to_string()),
            between_tool_delay_ms: 0,
        }
    }

    /// Telegram with optional requester user ID.
    ///
    /// When provided, `sender_id` tracks the requesting user (not the chat),
    /// which lets background tasks keep approval ownership tied to that user.
    pub fn telegram_with_sender(chat_id: i64, requester_user_id: Option<i64>) -> Self {
        let mut config = Self::telegram(chat_id);
        config.sender_id = requester_user_id.map(|id| id.to_string());
        config
    }

    /// WhatsApp: 10 iterations, 300s timeout, defense checks on.
    pub fn whatsapp(phone: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::WhatsApp {
                phone_number: phone.clone(),
            }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(phone),
            between_tool_delay_ms: 0,
        }
    }

    /// Voice: 10 iterations, no timeout, streaming for TTS.
    #[allow(dead_code)]
    pub fn voice() -> Self {
        Self {
            max_iterations: 10,
            timeout: None,
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Voice),
            run_defense_checks: true,
            streaming: true,
            sender_id: None,
            between_tool_delay_ms: 50,
        }
    }

    /// Discord: 25 iterations, 600s timeout, 8KB truncation, defense checks on.
    pub fn discord(channel_id: u64, guild_id: Option<u64>, user_id: u64) -> Self {
        Self {
            max_iterations: 25,
            timeout: Some(Duration::from_secs(600)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: Some(8_000),
            force_summary_on_exhaustion: true,
            channel: Some(ChannelSource::Discord {
                channel_id,
                guild_id,
            }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(user_id.to_string()),
            between_tool_delay_ms: 0,
        }
    }

    /// Slack: 10 iterations, 300s timeout, defense checks on.
    pub fn slack(channel_id: String, user_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Slack { channel_id }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(user_id),
            between_tool_delay_ms: 0,
        }
    }

    /// Signal: 10 iterations, 300s timeout, defense checks on.
    pub fn signal(phone_number: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Signal {
                phone_number: phone_number.clone(),
            }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(phone_number),
            between_tool_delay_ms: 0,
        }
    }

    /// Teams: 25 iterations, 600s timeout, 8KB truncation, defense checks on.
    pub fn teams(conversation_id: String, user_id: String) -> Self {
        Self {
            max_iterations: 25,
            timeout: Some(Duration::from_secs(600)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: Some(8_000),
            force_summary_on_exhaustion: true,
            channel: Some(ChannelSource::Teams { conversation_id }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(user_id),
            between_tool_delay_ms: 0,
        }
    }

    /// Matrix: 10 iterations, 300s timeout, defense checks on.
    pub fn matrix(room_id: String, sender_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Matrix { room_id }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(sender_id),
            between_tool_delay_ms: 0,
        }
    }

    /// BlueBubbles (iMessage): 10 iterations, 300s timeout, defense checks on.
    pub fn bluebubbles(chat_guid: String, sender_handle: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::BlueBubbles { chat_guid }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(sender_handle),
            between_tool_delay_ms: 0,
        }
    }

    /// Google Chat: 10 iterations, 300s timeout, defense checks on.
    pub fn google_chat(space_id: String, sender_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::GoogleChat {
                space_id,
                sender_id: sender_id.clone(),
            }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(sender_id),
            between_tool_delay_ms: 0,
        }
    }

    /// Mattermost: 10 iterations, 300s timeout, defense checks on.
    pub fn mattermost(channel_id: String, user_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Mattermost { channel_id }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(user_id),
            between_tool_delay_ms: 0,
        }
    }

    /// Messenger: 10 iterations, 300s timeout, defense checks on.
    pub fn messenger(sender_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Messenger {
                sender_id: sender_id.clone(),
            }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(sender_id),
            between_tool_delay_ms: 0,
        }
    }

    /// Instagram: 10 iterations, 300s timeout, defense checks on.
    pub fn instagram(sender_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Instagram {
                sender_id: sender_id.clone(),
            }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(sender_id),
            between_tool_delay_ms: 0,
        }
    }

    /// LINE: 10 iterations, 300s timeout, defense checks on.
    pub fn line(user_id: String, conversation_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Line {
                user_id: user_id.clone(),
                conversation_id,
            }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(user_id),
            between_tool_delay_ms: 0,
        }
    }

    /// Twilio SMS/MMS: 10 iterations, 300s timeout, defense checks on.
    pub fn twilio(phone_number: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Twilio {
                phone_number: phone_number.clone(),
            }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(phone_number),
            between_tool_delay_ms: 0,
        }
    }

    /// Mastodon: 10 iterations, 300s timeout, defense checks on.
    pub fn mastodon(account_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Mastodon {
                account_id: account_id.clone(),
            }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(account_id),
            between_tool_delay_ms: 0,
        }
    }

    /// RocketChat: 10 iterations, 300s timeout, defense checks on.
    pub fn rocketchat(room_id: String, user_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::RocketChat { room_id }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(user_id),
            between_tool_delay_ms: 0,
        }
    }

    /// WebChat: 10 iterations, 300s timeout, defense checks on.
    pub fn webchat(session_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::WebChat {
                session_id: session_id.clone(),
            }),
            run_defense_checks: true,
            streaming: false,
            sender_id: Some(session_id),
            between_tool_delay_ms: 0,
        }
    }

    /// Email (IMAP/SMTP): 10 iterations, 300s timeout, defense checks on.
    pub fn email(thread_id: String) -> Self {
        Self {
            max_iterations: 10,
            timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: Some(ChannelSource::Email { thread_id }),
            run_defense_checks: true,
            streaming: false,
            sender_id: None,
            between_tool_delay_ms: 0,
        }
    }

    /// Background task: 10 iterations, no timeout, defense checks on.
    pub fn background() -> Self {
        Self {
            max_iterations: 10,
            timeout: None,
            max_output_bytes: 10 * 1024 * 1024,
            max_tool_result_bytes: None,
            force_summary_on_exhaustion: false,
            channel: None,
            run_defense_checks: true,
            streaming: false,
            sender_id: None,
            between_tool_delay_ms: 0,
        }
    }

    /// Background task that preserves source channel identity for policy checks.
    pub fn background_with_origin(
        channel: Option<ChannelSource>,
        sender_id: Option<String>,
    ) -> Self {
        Self {
            channel,
            sender_id,
            ..Self::background()
        }
    }

    fn channel_label(&self) -> &str {
        match &self.channel {
            Some(ChannelSource::Gui) => "GUI",
            Some(ChannelSource::Telegram { .. }) => "TELEGRAM",
            Some(ChannelSource::WhatsApp { .. }) => "WHATSAPP",
            Some(ChannelSource::Voice) => "VOICE",
            Some(ChannelSource::InterAgent { .. }) => "INTER_AGENT",
            Some(ChannelSource::Discord { .. }) => "DISCORD",
            Some(ChannelSource::Slack { .. }) => "SLACK",
            Some(ChannelSource::Signal { .. }) => "SIGNAL",
            Some(ChannelSource::Teams { .. }) => "TEAMS",
            Some(ChannelSource::Matrix { .. }) => "MATRIX",
            Some(ChannelSource::BlueBubbles { .. }) => "BLUEBUBBLES",
            Some(ChannelSource::GoogleChat { .. }) => "GOOGLE_CHAT",
            Some(ChannelSource::Mattermost { .. }) => "MATTERMOST",
            Some(ChannelSource::Messenger { .. }) => "MESSENGER",
            Some(ChannelSource::Instagram { .. }) => "INSTAGRAM",
            Some(ChannelSource::Line { .. }) => "LINE",
            Some(ChannelSource::Twilio { .. }) => "TWILIO",
            Some(ChannelSource::Mastodon { .. }) => "MASTODON",
            Some(ChannelSource::RocketChat { .. }) => "ROCKETCHAT",
            Some(ChannelSource::WebChat { .. }) => "WEBCHAT",
            Some(ChannelSource::Email { .. }) => "EMAIL",
            Some(ChannelSource::Gmail { .. }) => "GMAIL",
            Some(ChannelSource::Nats { .. }) => "NATS",
            None => "BACKGROUND",
        }
    }
}

/// Observer for channel-specific side effects during the tool loop.
///
/// All methods have default no-op implementations so channels only need
/// to override what they care about.
#[async_trait]
pub trait ToolLoopObserver: Send + Sync {
    /// Called when a tool execution is about to start.
    async fn on_tool_start(&self, _name: &str, _id: &str) {}

    /// Called when a tool execution completes.
    async fn on_tool_result(&self, _name: &str, _id: &str, _success: bool) {}

    /// Called at each iteration with progress info.
    async fn on_progress(&self, _iteration: usize, _total: usize, _elapsed: Duration) {}

    /// Called with text chunks during streaming responses.
    async fn on_text_chunk(&self, _text: &str) {}

    /// Called when a tool execution fails (before any retry attempt).
    /// `error` carries the kind, plain-English message, and retry metadata.
    async fn on_tool_error(&self, _name: &str, _id: &str, _error: &ToolErrorInfo) {}

    /// Called when a streaming trait-based tool emits a progress event.
    /// Default implementation does nothing (backward compatible with all existing impls).
    async fn on_tool_progress(
        &self,
        _name: &str,
        _id: &str,
        _progress: &crate::tool_registry::ToolProgress,
    ) {}

    /// Called to check if the loop should be cancelled early.
    /// Returns true if cancellation is requested.
    async fn should_cancel(&self) -> bool {
        false
    }

    /// Called before the continue_after_tools API call (e.g., to start typing indicators).
    async fn on_before_continue(&self) {}

    /// Called after the continue_after_tools API call completes.
    async fn on_after_continue(&self) {}

    /// Called when the LLM falls back to a different model due to an error.
    async fn on_model_fallback(&self, _from_model: &str, _to_model: &str, _reason: &str) {}

    /// Whether this observer can collect approval decisions from a human.
    fn supports_approval(&self) -> bool {
        false
    }

    /// Request human approval before executing a sensitive tool.
    /// Return true = approved, false = denied/timeout.
    /// Default implementation: deny (safe fallback for unimplemented channels).
    async fn request_approval(&self, _tool_name: &str, _reason: &str) -> bool {
        false
    }

    /// Like `request_approval` but includes optional structured detail text
    /// (command, URL, path, script, etc.) shown in the approval UI so the user
    /// knows exactly what they are approving.
    /// Default delegates to `request_approval`, ignoring the detail.
    async fn request_approval_with_details(
        &self,
        tool_name: &str,
        reason: &str,
        details: Option<&str>,
    ) -> bool {
        let _ = details;
        self.request_approval(tool_name, reason).await
    }

    /// Return a reference to the Tauri window, if available.
    /// Used by execute_tool_call for canvas_push, DAG execution, yolo broadcasts,
    /// and toast notifications. Non-GUI observers return None.
    fn get_window(&self) -> Option<&tauri::Window> {
        None
    }
}

/// A no-op observer for channels that don't need progress reporting.
pub struct NoOpObserver;

#[async_trait]
impl ToolLoopObserver for NoOpObserver {}

/// Observer for GUI streaming mode — emits Tauri events for tool start/result/text.
pub struct GuiStreamingObserver {
    pub window: tauri::Window,
    // NOTE: Potential unbounded growth — if the oneshot receiver is dropped (e.g.
    // disconnected client) before the 300s timeout fires, the entry's sender stays
    // in the map until the next request_approval call times out naturally.  A
    // periodic sweep (e.g. removing entries whose Sender::is_closed() returns true)
    // would prevent slow leaks under sustained disconnects.  Low priority because
    // each entry is small and the map is scoped to a single window lifetime.
    pub pending_approvals: std::sync::Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
}

#[async_trait]
impl ToolLoopObserver for GuiStreamingObserver {
    fn supports_approval(&self) -> bool {
        true
    }

    fn get_window(&self) -> Option<&tauri::Window> {
        Some(&self.window)
    }

    async fn on_tool_start(&self, name: &str, id: &str) {
        use tauri::Emitter;
        if let Err(e) = self.window.emit(
            "chat:tool-start",
            serde_json::json!({
                "name": name, "id": id,
            }),
        ) {
            warn!("[GUI] Failed to emit chat:tool-start: {}", e);
        }
    }

    async fn on_tool_result(&self, name: &str, id: &str, success: bool) {
        use tauri::Emitter;
        if let Err(e) = self.window.emit(
            "chat:tool-result",
            serde_json::json!({
                "name": name, "id": id, "success": success,
            }),
        ) {
            warn!("[GUI] Failed to emit chat:tool-result: {}", e);
        }
    }

    async fn on_text_chunk(&self, text: &str) {
        use tauri::Emitter;
        if let Err(e) = self.window.emit(
            "chat:text-chunk",
            serde_json::json!({
                "text": text,
            }),
        ) {
            debug!("[GUI] Failed to emit chat:text-chunk: {}", e);
        }
    }

    async fn on_tool_error(&self, name: &str, id: &str, error: &ToolErrorInfo) {
        use tauri::Emitter;
        if let Err(e) = self.window.emit(
            "chat:tool-error",
            serde_json::json!({
                "name": name,
                "id": id,
                "kind": error.kind,
                "message": error.message,
                "retry_after_secs": error.retry_after_secs,
                "attempt": error.attempt,
                "max_attempts": error.max_attempts,
            }),
        ) {
            warn!("[GUI] Failed to emit chat:tool-error: {}", e);
        }
    }

    async fn on_progress(&self, iteration: usize, total: usize, elapsed: Duration) {
        use tauri::Emitter;
        if let Err(e) = self.window.emit(
            "chat:progress",
            serde_json::json!({
                "iteration": iteration + 1,
                "total": total,
                "elapsed_secs": elapsed.as_secs(),
            }),
        ) {
            warn!("[GUI] Failed to emit chat:progress: {}", e);
        }
    }

    async fn on_before_continue(&self) {
        use tauri::Emitter;
        if let Err(e) = self.window.emit("chat:thinking", serde_json::json!({})) {
            warn!("[GUI] Failed to emit chat:thinking: {}", e);
        }
    }

    async fn should_cancel(&self) -> bool {
        crate::commands::chat::is_cancel_requested()
    }

    async fn on_model_fallback(&self, from_model: &str, to_model: &str, reason: &str) {
        use tauri::Emitter;
        if let Err(e) = self.window.emit(
            "chat:model-fallback",
            serde_json::json!({
                "from_model": from_model,
                "to_model": to_model,
                "reason": reason,
            }),
        ) {
            warn!("[GUI] Failed to emit chat:model-fallback: {}", e);
        }
        crate::commands::chat::emit_toast(
            Some(&self.window),
            "warning",
            "Model Switch",
            &format!(
                "{} unavailable ({}). Trying {}…",
                from_model, reason, to_model
            ),
        );
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        self.request_approval_with_details(tool_name, reason, None).await
    }

    async fn request_approval_with_details(
        &self,
        tool_name: &str,
        reason: &str,
        details: Option<&str>,
    ) -> bool {
        use tauri::Emitter;

        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_approvals
            .lock()
            .await
            .insert(request_id.clone(), tx);

        let mut payload = serde_json::json!({
            "request_id": &request_id,
            "tool_name": tool_name,
            "reason": reason,
            "timeout_secs": 300_u64,
        });
        if let Some(d) = details {
            payload["details"] = serde_json::Value::String(d.to_string());
        }

        if self.window.emit("chat:tool-approval-request", payload).is_err() {
            warn!(
                "[GUI] Failed to emit approval request for tool '{}'",
                tool_name
            );
            self.pending_approvals.lock().await.remove(&request_id);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            Ok(Err(_)) => {
                warn!(
                    "[GUI] Approval channel dropped for request {} ({})",
                    request_id, tool_name
                );
                self.pending_approvals.lock().await.remove(&request_id);
                false
            }
            Err(_) => {
                self.pending_approvals.lock().await.remove(&request_id);
                if let Err(e) = self.window.emit(
                    "chat:tool-approval-expired",
                    serde_json::json!({
                        "request_id": request_id,
                        "tool_name": tool_name,
                    }),
                ) {
                    warn!(
                        "[GUI] Failed to emit chat:tool-approval-expired for {}: {}",
                        tool_name, e
                    );
                }
                false
            }
        }
    }
}

/// Observer for Telegram — typing indicators and time-based progress messages.
///
/// Sends a meaningful update every 60 seconds that includes:
/// - Elapsed time
/// - Current step / total steps
/// - Name of the last tool that ran and whether it succeeded
/// - Brief indication of what is happening next
pub struct TelegramObserver {
    pub bot: teloxide::Bot,
    pub chat_id: teloxide::types::ChatId,
    pub requester_user_id: Option<i64>,
    /// Wall-clock time of the last user-visible progress message.
    pub last_update: std::sync::Arc<std::sync::Mutex<std::time::Instant>>,
    /// Name of the most recently started tool.
    pub last_tool: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// Whether the most recently completed tool succeeded.
    pub last_tool_ok: std::sync::Arc<std::sync::Mutex<bool>>,
    /// Count of tools completed so far.
    pub tools_done: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    // NOTE: Potential unbounded growth — if the oneshot receiver is dropped (e.g.
    // the Telegram client disconnects) before the 300s timeout fires, the entry
    // remains in the map.  A periodic sweep removing entries whose
    // Sender::is_closed() returns true would prevent slow leaks.  Low priority
    // because the key space is (chat_id, user_id) and duplicate inserts are
    // already rejected, so at most one leaked entry per user.
    /// Pending human approvals: (chat_id, requester_user_id) → oneshot sender.
    pub pending_approvals: std::sync::Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(i64, i64), tokio::sync::oneshot::Sender<bool>>,
        >,
    >,
}

impl TelegramObserver {
    pub fn new(
        bot: teloxide::Bot,
        chat_id: teloxide::types::ChatId,
        requester_user_id: Option<i64>,
        pending_approvals: std::sync::Arc<
            tokio::sync::Mutex<
                std::collections::HashMap<(i64, i64), tokio::sync::oneshot::Sender<bool>>,
            >,
        >,
    ) -> Self {
        Self {
            bot,
            chat_id,
            requester_user_id,
            last_update: std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
            last_tool: std::sync::Arc::new(std::sync::Mutex::new(None)),
            last_tool_ok: std::sync::Arc::new(std::sync::Mutex::new(true)),
            tools_done: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            pending_approvals,
        }
    }
}

#[async_trait]
impl ToolLoopObserver for TelegramObserver {
    fn supports_approval(&self) -> bool {
        self.requester_user_id.is_some()
    }

    async fn on_tool_start(&self, name: &str, _id: &str) {
        *self.last_tool.lock().unwrap_or_else(|e| e.into_inner()) = Some(name.to_string());
    }

    async fn on_tool_result(&self, _name: &str, _id: &str, success: bool) {
        *self.last_tool_ok.lock().unwrap_or_else(|e| e.into_inner()) = success;
        self.tools_done
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    async fn on_progress(&self, _iteration: usize, _total: usize, elapsed: Duration) {
        use teloxide::requests::Requester;
        if let Err(e) = self
            .bot
            .send_chat_action(self.chat_id, teloxide::types::ChatAction::Typing)
            .await
        {
            warn!(
                "[TELEGRAM] Failed to send chat action to {}: {}",
                self.chat_id, e
            );
        }

        // Send a progress message every 60 seconds
        let should_update = {
            let last = self.last_update.lock().unwrap_or_else(|e| e.into_inner());
            last.elapsed().as_secs() >= 60
        };

        if should_update {
            *self.last_update.lock().unwrap_or_else(|e| e.into_inner()) = std::time::Instant::now();

            let secs = elapsed.as_secs();
            let tools_done = self.tools_done.load(std::sync::atomic::Ordering::Relaxed);
            let tool_line = match self
                .last_tool
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_deref()
            {
                Some(name) => {
                    let ok = *self.last_tool_ok.lock().unwrap_or_else(|e| e.into_inner());
                    if ok {
                        format!("Last action: {} ✓", name)
                    } else {
                        format!("Last action: {} (retrying)", name)
                    }
                }
                None => "Thinking...".to_string(),
            };

            let msg = format!(
                "⏳ Still working — {}s elapsed, {} step{} completed\n{}",
                secs,
                tools_done,
                if tools_done == 1 { "" } else { "s" },
                tool_line,
            );

            if let Err(e) = self.bot.send_message(self.chat_id, msg).await {
                warn!(
                    "[TELEGRAM] Could not send progress notice to {}: {}",
                    self.chat_id, e
                );
            }
        }
    }

    async fn on_before_continue(&self) {
        use teloxide::requests::Requester;
        if let Err(e) = self
            .bot
            .send_chat_action(self.chat_id, teloxide::types::ChatAction::Typing)
            .await
        {
            warn!(
                "[TELEGRAM] Failed to send chat action to {}: {}",
                self.chat_id, e
            );
        }
    }

    async fn on_model_fallback(&self, from_model: &str, to_model: &str, reason: &str) {
        use teloxide::requests::Requester;
        let msg = format!(
            "⚠️ {} failed ({}). Retrying with {}…",
            from_model, reason, to_model
        );
        if let Err(e) = self.bot.send_message(self.chat_id, msg).await {
            warn!(
                "[TELEGRAM] Could not send model-fallback notice to {}: {}",
                self.chat_id, e
            );
        }
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        self.request_approval_with_details(tool_name, reason, None).await
    }

    async fn request_approval_with_details(
        &self,
        tool_name: &str,
        reason: &str,
        details: Option<&str>,
    ) -> bool {
        use teloxide::requests::Requester;
        let Some(requester_user_id) = self.requester_user_id else {
            warn!(
                "[TELEGRAM] Missing requester user id in chat {} for tool '{}' approval",
                self.chat_id, tool_name
            );
            return false;
        };
        let key = (self.chat_id.0, requester_user_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending_approvals.lock().await;
            if map.contains_key(&key) {
                drop(map);
                if let Err(e) = self
                    .bot
                    .send_message(
                        self.chat_id,
                        "⚠️ Another approval is already pending for this user. Denying this request.",
                    )
                    .await
                {
                    warn!(
                        "[TELEGRAM] Could not send duplicate-approval notice to {}: {}",
                        self.chat_id, e
                    );
                }
                return false;
            }
            map.insert(key, tx);
        }
        let msg = if let Some(d) = details {
            format!(
                "🔐 Tool approval required\n\nTool: {}\nReason: {}\n\nDetails:\n```\n{}\n```",
                tool_name, reason, d
            )
        } else {
            format!(
                "🔐 Tool approval required\n\nTool: {}\nReason: {}",
                tool_name, reason
            )
        };
        let approve_data = format!("tool:approve:{}:{}", self.chat_id.0, requester_user_id);
        let deny_data = format!("tool:deny:{}:{}", self.chat_id.0, requester_user_id);
        let keyboard = teloxide::types::InlineKeyboardMarkup::new(vec![vec![
            teloxide::types::InlineKeyboardButton::callback("✅ Approve", approve_data),
            teloxide::types::InlineKeyboardButton::callback("❌ Deny", deny_data),
        ]]);
        let mut send_req = self.bot.send_message(self.chat_id, &msg);
        send_req.reply_markup = Some(teloxide::types::ReplyMarkup::InlineKeyboard(keyboard));
        if let Err(e) = send_req.await {
            warn!(
                "[TELEGRAM] Could not send approval request to {}: {}",
                self.chat_id, e
            );
            self.pending_approvals.lock().await.remove(&key);
            return false;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            Ok(Err(_)) => {
                warn!(
                    "[TELEGRAM] Approval channel dropped for chat {} and tool '{}'",
                    self.chat_id, tool_name
                );
                self.pending_approvals.lock().await.remove(&key);
                false
            }
            Err(_) => {
                self.pending_approvals.lock().await.remove(&key);
                if let Err(e) = self
                    .bot
                    .send_message(self.chat_id, "⏰ Approval timed out. Tool blocked.")
                    .await
                {
                    warn!(
                        "[TELEGRAM] Could not send timeout notice to {}: {}",
                        self.chat_id, e
                    );
                }
                false
            }
        }
    }
}

/// Observer for background tasks — emits task:progress and chat:tool events.
pub struct BackgroundObserver {
    pub app_handle: Option<tauri::AppHandle>,
    pub task_id: String,
    pub task_manager: std::sync::Arc<tokio::sync::RwLock<crate::task_manager::TaskManager>>,
    /// Allow GUI approval routing when a desktop app handle exists.
    /// In headless mode (no app handle), approval requests fail closed.
    pub allow_gui_approval: bool,
    // NOTE: Potential unbounded growth — same concern as GuiStreamingObserver.
    // Entries may leak if the oneshot receiver is dropped before the 300s timeout.
    // A periodic sweep of entries where Sender::is_closed() would fix this.
    pub pending_approvals: std::sync::Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
}

#[async_trait]
impl ToolLoopObserver for BackgroundObserver {
    fn supports_approval(&self) -> bool {
        self.allow_gui_approval && self.app_handle.is_some()
    }

    async fn on_tool_start(&self, name: &str, id: &str) {
        if let Some(ref h) = self.app_handle {
            use tauri::Emitter;
            if let Err(e) = h.emit(
                "chat:tool-start",
                serde_json::json!({
                    "name": name, "id": id,
                }),
            ) {
                warn!("[BACKGROUND] Failed to emit chat:tool-start: {}", e);
            }
        }
    }

    async fn on_tool_result(&self, name: &str, id: &str, success: bool) {
        if let Some(ref h) = self.app_handle {
            use tauri::Emitter;
            if let Err(e) = h.emit(
                "chat:tool-result",
                serde_json::json!({
                    "name": name, "id": id, "success": success,
                }),
            ) {
                warn!("[BACKGROUND] Failed to emit chat:tool-result: {}", e);
            }
        }
    }

    async fn on_progress(&self, iteration: usize, _total: usize, _elapsed: Duration) {
        // Update task manager progress
        {
            let mut tm = self.task_manager.write().await;
            tm.update_progress(&self.task_id, &format!("Step {}...", iteration + 1));
        }
        if let Some(ref h) = self.app_handle {
            use tauri::Emitter;
            if let Err(e) = h.emit(
                "task:progress",
                serde_json::json!({
                    "task_id": self.task_id,
                    "progress": format!("Step {}", iteration + 1),
                }),
            ) {
                warn!("[BACKGROUND] Failed to emit task:progress: {}", e);
            }
        }
    }

    async fn request_approval(&self, tool_name: &str, reason: &str) -> bool {
        self.request_approval_with_details(tool_name, reason, None).await
    }

    async fn request_approval_with_details(
        &self,
        tool_name: &str,
        reason: &str,
        details: Option<&str>,
    ) -> bool {
        use tauri::Emitter;

        if !self.allow_gui_approval {
            return false;
        }

        let Some(ref app_handle) = self.app_handle else {
            return false;
        };

        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_approvals
            .lock()
            .await
            .insert(request_id.clone(), tx);

        let mut payload = serde_json::json!({
            "request_id": &request_id,
            "tool_name": tool_name,
            "reason": reason,
            "timeout_secs": 300_u64,
        });
        if let Some(d) = details {
            payload["details"] = serde_json::Value::String(d.to_string());
        }

        if app_handle.emit("chat:tool-approval-request", payload).is_err() {
            warn!(
                "[BACKGROUND] Failed to emit approval request for tool '{}'",
                tool_name
            );
            self.pending_approvals.lock().await.remove(&request_id);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            Ok(Err(_)) => {
                warn!(
                    "[BACKGROUND] Approval channel dropped for request {} ({})",
                    request_id, tool_name
                );
                self.pending_approvals.lock().await.remove(&request_id);
                false
            }
            Err(_) => {
                self.pending_approvals.lock().await.remove(&request_id);
                if let Err(e) = app_handle.emit(
                    "chat:tool-approval-expired",
                    serde_json::json!({
                        "request_id": request_id,
                        "tool_name": tool_name,
                    }),
                ) {
                    warn!(
                        "[BACKGROUND] Failed to emit chat:tool-approval-expired for {}: {}",
                        tool_name, e
                    );
                }
                false
            }
        }
    }
}

/// Observer for voice pipeline — checks cancel flag and streams text for TTS.
#[allow(dead_code)]
pub struct VoiceObserver {
    pub cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub text_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    pub app_handle: Option<tauri::AppHandle>,
}

#[async_trait]
impl ToolLoopObserver for VoiceObserver {
    async fn should_cancel(&self) -> bool {
        self.cancel_flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn on_tool_start(&self, name: &str, id: &str) {
        if let Some(ref h) = self.app_handle {
            use tauri::Emitter;
            if let Err(e) = h.emit(
                "chat:tool-start",
                serde_json::json!({
                    "name": name, "id": id,
                }),
            ) {
                warn!("[VOICE] Failed to emit chat:tool-start: {}", e);
            }
        }
    }

    async fn on_tool_result(&self, name: &str, id: &str, success: bool) {
        if let Some(ref h) = self.app_handle {
            use tauri::Emitter;
            if let Err(e) = h.emit(
                "chat:tool-result",
                serde_json::json!({
                    "name": name, "id": id, "success": success,
                }),
            ) {
                warn!("[VOICE] Failed to emit chat:tool-result: {}", e);
            }
        }
    }

    async fn on_text_chunk(&self, text: &str) {
        if let Some(ref tx) = self.text_tx {
            if tx.send(text.to_string()).is_err() {
                debug!("[VOICE] Failed to queue streaming text chunk for TTS");
            }
        }
    }
}

/// Check if a tool is denied by the channel's tool policy.
/// Returns `true` if the tool should be blocked, `false` if allowed.
fn check_channel_tool_policy(
    app_config: &NexiBotConfig,
    loop_config: &ToolLoopConfig,
    tool_name: &str,
) -> bool {
    let (policy, is_admin) = match &loop_config.channel {
        Some(ChannelSource::Telegram { chat_id }) => (
            &app_config.telegram.tool_policy,
            app_config.telegram.admin_chat_ids.contains(chat_id),
        ),
        Some(ChannelSource::WhatsApp { phone_number }) => (
            &app_config.whatsapp.tool_policy,
            app_config
                .whatsapp
                .admin_phone_numbers
                .iter()
                .any(|p| p == phone_number),
        ),
        Some(ChannelSource::Discord { .. }) => (
            &app_config.discord.tool_policy,
            loop_config.sender_id.as_ref().map_or(false, |sid| {
                sid.parse::<u64>().map_or(false, |uid| {
                    app_config.discord.admin_user_ids.contains(&uid)
                })
            }),
        ),
        Some(ChannelSource::Slack { .. }) => (
            &app_config.slack.tool_policy,
            loop_config
                .sender_id
                .as_ref()
                .map_or(false, |sid| app_config.slack.admin_user_ids.contains(sid)),
        ),
        Some(ChannelSource::Signal { phone_number }) => (
            &app_config.signal.tool_policy,
            app_config.signal.admin_numbers.contains(phone_number),
        ),
        Some(ChannelSource::Teams { .. }) => (
            &app_config.teams.tool_policy,
            loop_config
                .sender_id
                .as_ref()
                .map_or(false, |sid| app_config.teams.admin_user_ids.contains(sid)),
        ),
        Some(ChannelSource::Matrix { .. }) => (
            &app_config.matrix.tool_policy,
            loop_config
                .sender_id
                .as_ref()
                .map_or(false, |sid| app_config.matrix.admin_user_ids.contains(sid)),
        ),
        Some(ChannelSource::BlueBubbles { .. }) => (
            &app_config.bluebubbles.tool_policy,
            loop_config.sender_id.as_ref().map_or(false, |sid| {
                app_config.bluebubbles.admin_handles.contains(sid)
            }),
        ),
        Some(ChannelSource::GoogleChat { sender_id, .. }) => (
            &app_config.google_chat.tool_policy,
            app_config.google_chat.admin_user_ids.contains(sender_id),
        ),
        Some(ChannelSource::Mattermost { .. }) => (
            &app_config.mattermost.tool_policy,
            loop_config.sender_id.as_ref().map_or(false, |sid| {
                app_config.mattermost.admin_user_ids.contains(sid)
            }),
        ),
        Some(ChannelSource::Messenger { sender_id }) => (
            &app_config.messenger.tool_policy,
            app_config.messenger.admin_sender_ids.contains(sender_id),
        ),
        Some(ChannelSource::Instagram { sender_id }) => (
            &app_config.instagram.tool_policy,
            app_config.instagram.admin_sender_ids.contains(sender_id),
        ),
        Some(ChannelSource::Line { user_id, .. }) => (
            &app_config.line.tool_policy,
            app_config.line.admin_user_ids.contains(user_id),
        ),
        Some(ChannelSource::Twilio { phone_number }) => (
            &app_config.twilio.tool_policy,
            app_config.twilio.admin_numbers.contains(phone_number),
        ),
        Some(ChannelSource::Mastodon { account_id }) => (
            &app_config.mastodon.tool_policy,
            app_config.mastodon.admin_account_ids.contains(account_id),
        ),
        Some(ChannelSource::RocketChat { .. }) => (
            &app_config.rocketchat.tool_policy,
            loop_config.sender_id.as_ref().map_or(false, |sid| {
                app_config.rocketchat.admin_user_ids.contains(sid)
            }),
        ),
        Some(ChannelSource::WebChat { .. }) => (&app_config.webchat.tool_policy, false),
        Some(ChannelSource::Email { .. }) => (&app_config.email.tool_policy, false),
        Some(ChannelSource::Gmail { .. }) => (&app_config.gmail.tool_policy, false),
        _ => return false, // Gui, Voice, InterAgent, Background — never denied
    };

    if !policy.is_tool_denied(tool_name) {
        return false;
    }
    if policy.admin_bypass && is_admin {
        return false;
    }
    true
}

/// Execute the unified tool-use loop.
///
/// Takes an initial `ClaudeMessageResult` from the first LLM call and continues
/// executing tools until the model stops requesting them, the iteration limit is
/// reached, or the timeout expires.
///
/// Returns the final `ClaudeMessageResult` with the model's text response.
pub async fn execute_tool_loop(
    client: &ClaudeClient,
    tools: &[serde_json::Value],
    overrides: &SessionOverrides,
    initial_result: ClaudeMessageResult,
    config: &ToolLoopConfig,
    state: &AppState,
    observer: &dyn ToolLoopObserver,
) -> Result<ClaudeMessageResult, String> {
    let label = config.channel_label();
    let loop_start = Instant::now();
    let mut cumulative_output_bytes: usize = 0;
    let mut result = initial_result;
    // When a background task is spawned via nexibot_background_task, the tool
    // returns a detach sentinel.  We capture the acknowledgment text here and
    // short-circuit the loop after the current tool batch finishes, returning
    // the acknowledgment as the final response without an extra LLM round-trip.
    let mut detach_text: Option<String> = None;
    let mut output_limit_hit = false;

    // Mutable tool set: starts with filtered tools, dynamically grows if LLM
    // requests an MCP tool that wasn't in the initial filtered set.
    let mut active_tools: Vec<serde_json::Value> = tools.to_vec();
    let mut total_tool_calls: usize = 0;

    for iteration in 0..config.max_iterations {
        // Check stop conditions
        if result.tool_uses.is_empty() {
            break;
        }

        // Some channels (like Telegram) only check tool_uses.is_empty(),
        // others also check stop_reason. We check both for maximum compat,
        // but still execute tool calls if tool_uses is non-empty (Telegram behavior).
        // The break above handles the empty case; we proceed if there are tool uses.

        if let Some(timeout) = config.timeout {
            if loop_start.elapsed() > timeout {
                warn!(
                    "[{}] Tool loop timed out after {:?}",
                    label,
                    loop_start.elapsed()
                );
                break;
            }
        }

        if observer.should_cancel().await {
            info!("[{}] Tool loop cancelled by observer", label);
            break;
        }

        info!(
            "[{}] Tool-use loop iteration {}: {} tool calls",
            label,
            iteration + 1,
            result.tool_uses.len()
        );

        // Notify observer of progress
        observer
            .on_progress(iteration, config.max_iterations, loop_start.elapsed())
            .await;

        total_tool_calls += result.tool_uses.len();

        // Execute tool calls with parallel I/O for multi-tool batches.
        //
        // Phase 1 (serial): MCP dynamic lookup, observer notification, policy checks,
        //   input restoration, and session-key derivation.
        // Phase 2 (parallel): concurrent execution via join_all — independent tool
        //   I/O (web fetches, file reads, etc.) overlaps across all calls in the batch.
        // Phase 3 (serial): cumulative-output accounting, defense pipeline, key-vault
        //   interception, truncation, detach-sentinel handling, and client mutations.

        struct PreparedCall {
            idx:            usize,
            session_key:    String,
            agent_id:       String,
            restored_input: serde_json::Value,
            /// Pre-computed result for policy-blocked calls — skips Phase 2.
            early_result:   Option<String>,
        }

        let agent_id = config.channel_label();
        let mut prepared: Vec<PreparedCall> = Vec::with_capacity(result.tool_uses.len());

        // ── Phase 1: serial preparation ───────────────────────────────────
        for (idx, tool_use) in result.tool_uses.iter().enumerate() {
            info!(
                "[{}] Preparing tool: {} (id: {})",
                label, tool_use.name, tool_use.id
            );

            // Dynamic MCP tool addition (mutates active_tools — must be serial).
            if !active_tools
                .iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some(&tool_use.name))
            {
                let mcp = state.mcp_manager.read().await;
                if let Some(tool_def) = mcp.get_tool_by_name(&tool_use.name) {
                    if active_tools.len() < MAX_ACTIVE_MCP_TOOLS {
                        info!(
                            "[{}] Dynamically adding MCP tool '{}' to active tool set",
                            label, tool_use.name
                        );
                        active_tools.push(tool_def);
                    } else {
                        warn!(
                            "[{}] Active tool limit ({}) reached, skipping dynamic add of '{}'",
                            label, MAX_ACTIVE_MCP_TOOLS, tool_use.name
                        );
                    }
                }
            }

            // Notify observer that a tool is starting.
            observer.on_tool_start(&tool_use.name, &tool_use.id).await;

            // Audit log for dangerous tools.
            if dangerous_tools::is_dangerous_tool(&tool_use.name) {
                warn!(
                    "[{}] Executing dangerous tool: {} — logged for audit",
                    label, tool_use.name
                );
            }

            // Per-channel policy check.
            let channel_denied = {
                let app_config = state.config.read().await;
                check_channel_tool_policy(&app_config, config, &tool_use.name)
            };
            if channel_denied {
                warn!(
                    "[{}] Tool '{}' denied by channel tool policy for {:?}",
                    label, tool_use.name, config.channel
                );
                let denied_msg = format!(
                    "DENIED: Tool '{}' is not available in this channel per its tool policy. \
                     An admin can change this in config.yaml under the channel's tool_policy section.",
                    tool_use.name
                );
                prepared.push(PreparedCall {
                    idx,
                    session_key: String::new(),
                    agent_id: agent_id.to_string(),
                    restored_input: serde_json::Value::Null,
                    early_result: Some(denied_msg),
                });
                continue;
            }

            // Per-agent policy check (yolo mode bypasses).
            {
                let tool_policy_mgr = state.tool_policy_manager.read().await;
                let yolo_active = state.yolo_manager.is_active().await;
                if !yolo_active && !tool_policy_mgr.is_tool_allowed(&agent_id, &tool_use.name) {
                    warn!(
                        "[{}] Tool '{}' denied by agent tool policy",
                        label, tool_use.name
                    );
                    let blocked_msg = format!(
                        "BLOCKED: Tool '{}' is not permitted for agent '{}' by tool policy.",
                        tool_use.name, agent_id
                    );
                    prepared.push(PreparedCall {
                        idx,
                        session_key: String::new(),
                        agent_id: agent_id.to_string(),
                        restored_input: serde_json::Value::Null,
                        early_result: Some(blocked_msg),
                    });
                    continue;
                }
            }

            // Restore proxy keys in tool input before execution.
            let restored_input = {
                let kv_config = {
                    let cfg = state.config.read().await;
                    cfg.key_vault.clone()
                };
                if kv_config.restore_tool_inputs {
                    let mut input_clone = tool_use.input.clone();
                    state.key_interceptor.restore_tool_input(&mut input_clone);
                    input_clone
                } else {
                    tool_use.input.clone()
                }
            };

            // Sanitised session key for NexiGate PTY isolation.
            let session_key: String = {
                let raw = config.sender_id.as_deref().unwrap_or("default");
                let s: String = raw
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '_' || c == '-' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .take(64)
                    .collect();
                if s.is_empty() { "default".to_string() } else { s }
            };

            prepared.push(PreparedCall {
                idx,
                session_key,
                agent_id: agent_id.to_string(),
                restored_input,
                early_result: None,
            });
        }

        // ── Phase 2: parallel execution ───────────────────────────────────
        // All futures share immutable borrows of `state` and `observer`, which
        // is safe because join_all drives them from the same async task without
        // spawning new threads.
        let exec_results: Vec<String> = {
            let futs: Vec<_> = prepared.iter().map(|call| {
                let tool_use = &result.tool_uses[call.idx];
                async {
                    if let Some(ref early) = call.early_result {
                        return early.clone();
                    }

                    let mut attempt: u32 = 1;
                    #[allow(unused_assignments)]
                    let mut last_result = String::new();
                    loop {
                        let raw = chat::execute_tool_call(
                            &tool_use.name,
                            &tool_use.id,
                            &call.restored_input,
                            state,
                            observer.get_window(),
                            &call.session_key,
                            &call.agent_id,
                            Some(observer),
                            config.channel.as_ref(),
                            config.sender_id.as_deref(),
                        )
                        .await;

                        // "BLOCKED" is a policy result — not a transient error.
                        let is_error = raw.starts_with("Error") && !raw.starts_with("BLOCKED");
                        if !is_error {
                            last_result = raw;
                            break;
                        }

                        let kind = tool_retry::classify_error(&raw);
                        let max = tool_retry::max_attempts(&kind);
                        let retry_after = tool_retry::parse_retry_after(&raw);
                        let message = tool_retry::plain_english_message(&kind, &raw, attempt, max);
                        let wait = tool_retry::backoff_secs(attempt + 1, &kind, retry_after);

                        observer
                            .on_tool_error(
                                &tool_use.name,
                                &tool_use.id,
                                &ToolErrorInfo {
                                    kind: kind.clone(),
                                    message: message.clone(),
                                    retry_after_secs: wait,
                                    attempt,
                                    max_attempts: max,
                                },
                            )
                            .await;

                        if attempt >= max
                            || kind == ToolErrorKind::AuthFailed
                            || kind == ToolErrorKind::Other
                        {
                            last_result = raw;
                            break;
                        }

                        warn!(
                            "[tool] '{}' failed (attempt {}/{}), retrying in {}s",
                            tool_use.name, attempt, max, wait
                        );
                        if wait > 0 {
                            tokio::time::sleep(Duration::from_secs(wait)).await;
                        }
                        attempt += 1;
                    }
                    last_result
                }
            }).collect();
            join_all(futs).await
        };

        // ── Phase 3: serial result processing ────────────────────────────
        for (call, tool_result) in prepared.iter().zip(exec_results.into_iter()) {
            let tool_use = &result.tool_uses[call.idx];
            let success =
                !tool_result.starts_with("BLOCKED") && !tool_result.starts_with("Error");

            // Cumulative output gate.
            cumulative_output_bytes += tool_result.len();
            if cumulative_output_bytes > config.max_output_bytes {
                warn!(
                    "[{}] Cumulative output exceeded limit ({} bytes)",
                    label, cumulative_output_bytes
                );
                client
                    .add_tool_result(&tool_use.id, "Output limit exceeded. Results truncated.")
                    .await;
                observer
                    .on_tool_result(&tool_use.name, &tool_use.id, false)
                    .await;
                output_limit_hit = true;
                break;
            }

            // Defense pipeline check.
            let final_result = if config.run_defense_checks {
                let mut defense = state.defense_pipeline.write().await;
                let defense_result = defense.check(&tool_result).await;
                if !defense_result.allowed {
                    let reason = defense_result
                        .blocked_by
                        .unwrap_or_else(|| "Defense pipeline".to_string());
                    warn!("[{}] Tool result sanitized by: {}", label, reason);
                    "[Tool output was filtered by safety checks]".to_string()
                } else {
                    tool_result
                }
            } else {
                tool_result
            };

            // Key vault interception on tool result.
            let final_result = {
                let kv_config = {
                    let cfg = state.config.read().await;
                    cfg.key_vault.clone()
                };
                if kv_config.intercept_tool_results {
                    state.key_interceptor.intercept_message(&final_result)
                } else {
                    final_result
                }
            };

            // Truncate large results if configured.
            let history_result = if let Some(max_bytes) = config.max_tool_result_bytes {
                if final_result.len() > max_bytes {
                    let total = final_result.len();
                    let suffix_template = format!(
                        "\n\n[... truncated: {} total bytes, showing first ",
                        total
                    );
                    let suffix_overhead = suffix_template.len() + 20 + 1;
                    let max_content = max_bytes.saturating_sub(suffix_overhead);
                    let safe_end = (0..=max_content.min(final_result.len()))
                        .rev()
                        .find(|&i| final_result.is_char_boundary(i))
                        .unwrap_or(0);
                    let break_at = final_result[..safe_end].rfind('\n').unwrap_or(safe_end);
                    format!(
                        "{}\n\n[... truncated: {} total bytes, showing first {}]",
                        &final_result[..break_at],
                        total,
                        break_at
                    )
                } else {
                    final_result
                }
            } else {
                final_result
            };

            // Detect detach sentinel from nexibot_background_task.
            let history_result = if history_result.starts_with("NEXIBOT_DETACH:") {
                let without_prefix = &history_result["NEXIBOT_DETACH:".len()..];
                let mut parts = without_prefix.splitn(2, ':');
                let _task_id = parts.next().unwrap_or("");
                let ack = parts
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "Got it, working on it in the background.".to_string());
                detach_text = Some(ack);
                "Background task started successfully.".to_string()
            } else {
                history_result
            };

            client.add_tool_result(&tool_use.id, &history_result).await;
            observer
                .on_tool_result(&tool_use.name, &tool_use.id, success)
                .await;
        }

        // All tool results for this batch have been added. Trim the history
        // now (rather than after each individual add_tool_result) to avoid
        // orphaning in-flight tool_use/tool_result pairs during a batch.
        client.trim_history_if_needed().await;

        // If output limit was hit mid-batch, the inner loop broke early.
        // The API requires every tool_use to have a matching tool_result.
        // Add placeholder results for any tool_uses that weren't processed,
        // then break the outer loop to avoid further LLM calls.
        if output_limit_hit {
            // We don't track which tool_uses were processed vs skipped, but
            // add_tool_result for an already-handled ID is safe (the client
            // keeps the last result). So add placeholders for ALL tool_uses —
            // already-processed ones just get their result overwritten with the
            // same truncation message, which is acceptable.
            for tu in &result.tool_uses {
                client.add_tool_result(&tu.id, "Output limit exceeded. Results truncated.").await;
            }
            break;
        }

        // Short-circuit: background task was spawned, return acknowledgment immediately.
        if let Some(ack) = detach_text {
            info!(
                "[{}] Detaching tool loop — background task spawned, returning acknowledgment",
                label
            );
            return Ok(ClaudeMessageResult {
                text: ack,
                tool_uses: vec![],
                stop_reason: "detached".to_string(),
                raw_content: vec![],
                tool_calls_made: iteration + 1,
            });
        }

        // Continue after tools — use streaming when configured so the user
        // sees token-by-token output during multi-tool-loop iterations.
        observer.on_before_continue().await;

        // ── Tool-pairing error recovery ────────────────────────────────────
        // Attempt the continue call. If the Anthropic API rejects it with an
        // "unexpected tool_use_id" 400 error (tool_result/tool_use mismatch),
        // apply four recovery strategies in sequence and retry.
        let continue_result = if config.streaming {
            let stream_window = observer.get_window().cloned();
            let (chunk_tx, mut chunk_rx) =
                tokio::sync::mpsc::channel::<String>(512);
            let chunk_tx = std::sync::Arc::new(chunk_tx);
            let chunk_tx_cb = chunk_tx.clone();
            let stream_window_c = stream_window.clone();
            let attempt = client.continue_after_tools_streaming(&active_tools, overrides, config.channel.as_ref(), move |chunk| {
                if let Some(ref w) = stream_window_c {
                    use tauri::Emitter;
                    let _ = w.emit("chat:text-chunk", serde_json::json!({ "text": chunk }));
                } else {
                    let _ = chunk_tx_cb.try_send(chunk);
                }
            }).await;
            match attempt {
                Ok(r) => {
                    chunk_rx.close();
                    while let Some(chunk) = chunk_rx.recv().await {
                        observer.on_text_chunk(&chunk).await;
                    }
                    Ok(r)
                }
                Err(ref e) if ClaudeClient::is_tool_pairing_error(&e.to_string()) => {
                    chunk_rx.close();
                    Err(e.to_string())
                }
                Err(e) => Err(e.to_string()),
            }
        } else {
            client.continue_after_tools(&active_tools, overrides).await.map_err(|e| e.to_string())
        };

        result = match continue_result {
            Ok(r) => {
                observer.on_after_continue().await;
                if !config.streaming && !r.text.is_empty() {
                    observer.on_text_chunk(&r.text).await;
                }
                r
            }
            Err(ref e) if ClaudeClient::is_tool_pairing_error(e) => {
                observer.on_after_continue().await;
                warn!(
                    "[{}] Tool-pairing 400 error (iteration {}): {}",
                    label, iteration + 1, e
                );

                let mut recovered = false;

                // Strategy 1: Re-sanitize history (cheapest — fixes most trim races)
                client.repair_sanitize().await;
                let retry1 = client.continue_after_tools(&active_tools, overrides).await;
                if let Ok(r) = retry1 {
                    info!("[{}] Tool-pairing recovery S1 (sanitize) succeeded", label);
                    TOOL_PAIRING_RECOVERIES.fetch_add(1, Ordering::Relaxed);
                    recovered = true;
                    observer.on_after_continue().await;
                    if !r.text.is_empty() { observer.on_text_chunk(&r.text).await; }
                    result = r;
                }

                // Strategy 2: Compact to last 30 messages
                if !recovered {
                    client.repair_compact(30).await;
                    let retry2 = client.continue_after_tools(&active_tools, overrides).await;
                    if let Ok(r) = retry2 {
                        info!("[{}] Tool-pairing recovery S2 (compact-30) succeeded", label);
                        TOOL_PAIRING_RECOVERIES.fetch_add(1, Ordering::Relaxed);
                        recovered = true;
                        if !r.text.is_empty() { observer.on_text_chunk(&r.text).await; }
                        result = r;
                    }
                }

                // Strategy 3: Strip all tool blocks, keep only text
                if !recovered {
                    client.repair_strip_tool_blocks().await;
                    let retry3 = client.continue_after_tools(&active_tools, overrides).await;
                    if let Ok(r) = retry3 {
                        info!("[{}] Tool-pairing recovery S3 (strip-tools) succeeded", label);
                        TOOL_PAIRING_RECOVERIES.fetch_add(1, Ordering::Relaxed);
                        recovered = true;
                        if !r.text.is_empty() { observer.on_text_chunk(&r.text).await; }
                        result = r;
                    }
                }

                // Strategy 4: Clear history entirely (last resort)
                if !recovered {
                    client.repair_clear().await;
                    let retry4 = client.continue_after_tools(&active_tools, overrides).await;
                    match retry4 {
                        Ok(r) => {
                            info!("[{}] Tool-pairing recovery S4 (clear) succeeded", label);
                            TOOL_PAIRING_RECOVERIES.fetch_add(1, Ordering::Relaxed);
                            if !r.text.is_empty() { observer.on_text_chunk(&r.text).await; }
                            result = r;
                        }
                        Err(final_err) => {
                            error!(
                                "[{}] All 4 tool-pairing recovery strategies exhausted: {}",
                                label, final_err
                            );
                            TOOL_PAIRING_ERRORS.fetch_add(1, Ordering::Relaxed);
                            return Err(final_err.to_string());
                        }
                    }
                }

                TOOL_PAIRING_ERRORS.fetch_add(1, Ordering::Relaxed);
                result
            }
            Err(e) => {
                observer.on_after_continue().await;
                error!("[{}] Failed to continue after tools: {}", label, e);
                return Err(e);
            }
        };
    }

    // Force summary if configured and loop exhausted without text
    if config.force_summary_on_exhaustion && result.text.is_empty() && !result.tool_uses.is_empty()
    {
        info!(
            "[{}] Tool loop ended with no text — forcing summary call",
            label
        );

        // Add a tool_result for EVERY pending tool_use — the API requires each
        // tool_use block to have a matching tool_result.
        let summary_msg = "SYSTEM: You have used all available tool iterations. You MUST now respond to the user \
             with your best and most complete answer based on everything you have gathered so far. \
             Do NOT say you will do something next — deliver your answer NOW. If you were creating \
             a document, provide the key findings in your message. Be thorough and helpful.";
        for (i, tu) in result.tool_uses.iter().enumerate() {
            let msg = if i == 0 { summary_msg } else { "[iteration limit reached]" };
            client.add_tool_result(&tu.id, msg).await;
        }
        client.trim_history_if_needed().await;

        match client.continue_after_tools(&[], overrides).await {
            Ok(r) => result = r,
            Err(e) => {
                warn!("[{}] Summary call failed: {}", label, e);
            }
        }
    }

    result.tool_calls_made = total_tool_calls;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voice_channel_has_defense_checks_enabled() {
        let config = ToolLoopConfig::voice();
        assert!(
            config.run_defense_checks,
            "Voice channel must have defense checks enabled — voice is external untrusted input"
        );
    }

    #[test]
    fn test_all_channel_configs_have_defense_checks() {
        // All external channels must run defense checks
        let configs = vec![
            ("voice", ToolLoopConfig::voice()),
            ("telegram", ToolLoopConfig::telegram(12345)),
            ("whatsapp", ToolLoopConfig::whatsapp("+1234567890".into())),
            ("discord", ToolLoopConfig::discord(12345, None, 99)),
            ("slack", ToolLoopConfig::slack("C123".into(), "U456".into())),
            ("signal", ToolLoopConfig::signal("+15551234567".into())),
            (
                "teams",
                ToolLoopConfig::teams("conv1".into(), "user1".into()),
            ),
            (
                "matrix",
                ToolLoopConfig::matrix("!room:matrix.org".into(), "@user:matrix.org".into()),
            ),
            (
                "bluebubbles",
                ToolLoopConfig::bluebubbles("chat-guid".into(), "+15550001111".into()),
            ),
            (
                "google_chat",
                ToolLoopConfig::google_chat("spaces/AAA".into(), "users/123".into()),
            ),
            (
                "mattermost",
                ToolLoopConfig::mattermost("channel-1".into(), "user-1".into()),
            ),
            ("messenger", ToolLoopConfig::messenger("psid-1".into())),
            ("instagram", ToolLoopConfig::instagram("igid-1".into())),
            (
                "line",
                ToolLoopConfig::line("line-user-1".into(), "line-user-1".into()),
            ),
            ("twilio", ToolLoopConfig::twilio("+15551230000".into())),
            ("mastodon", ToolLoopConfig::mastodon("acct-1".into())),
            (
                "rocketchat",
                ToolLoopConfig::rocketchat("room-1".into(), "user-1".into()),
            ),
            ("webchat", ToolLoopConfig::webchat("session-1".into())),
        ];

        for (name, config) in configs {
            assert!(
                config.run_defense_checks,
                "Channel '{}' must have defense checks enabled",
                name
            );
        }
    }

    #[test]
    fn test_telegram_with_sender_uses_requester_user_id() {
        let config = ToolLoopConfig::telegram_with_sender(12345, Some(67890));
        assert_eq!(config.sender_id.as_deref(), Some("67890"));
    }

    #[test]
    fn test_gui_never_denied_by_tool_policy() {
        let app_config = NexiBotConfig::default();
        let loop_config = ToolLoopConfig::gui_default();
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_filesystem"
        ));
    }

    #[test]
    fn test_telegram_default_denies_filesystem_not_execute() {
        let app_config = NexiBotConfig::default();
        let loop_config = ToolLoopConfig::telegram(12345);
        // Execute is NOT denied by default — it's protected by its own layers
        // (guardrails, DCG, autonomous mode, config.execute.enabled)
        assert!(
            !check_channel_tool_policy(&app_config, &loop_config, "nexibot_execute"),
            "nexibot_execute should NOT be denied by default channel policy"
        );
        // Filesystem IS denied by default
        assert!(check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_filesystem"
        ));
        // Other tools should not be denied
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_memory"
        ));
    }

    #[test]
    fn test_telegram_admin_bypasses_denial() {
        let mut app_config = NexiBotConfig::default();
        // Explicitly deny execute so we can test admin bypass
        app_config.telegram.tool_policy.denied_tools = vec!["nexibot_execute".into()];
        app_config.telegram.admin_chat_ids = vec![12345];
        let loop_config = ToolLoopConfig::telegram(12345);
        // Admin should bypass the denial
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));
    }

    #[test]
    fn test_telegram_non_admin_still_denied() {
        let mut app_config = NexiBotConfig::default();
        // Explicitly deny execute so we can test non-admin denial
        app_config.telegram.tool_policy.denied_tools = vec!["nexibot_execute".into()];
        app_config.telegram.admin_chat_ids = vec![99999]; // different admin
        let loop_config = ToolLoopConfig::telegram(12345);
        assert!(check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));
    }

    #[test]
    fn test_allow_all_policy_allows_everything() {
        let mut app_config = NexiBotConfig::default();
        app_config.telegram.tool_policy = crate::config::ChannelToolPolicy::allow_all();
        let loop_config = ToolLoopConfig::telegram(12345);
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_filesystem"
        ));
    }

    #[test]
    fn test_allowed_tools_overrides_denied() {
        let mut app_config = NexiBotConfig::default();
        // Explicitly deny both, then allow execute via allowed_tools override
        app_config.telegram.tool_policy.denied_tools =
            vec!["nexibot_execute".into(), "nexibot_filesystem".into()];
        app_config.telegram.tool_policy.allowed_tools = vec!["nexibot_execute".into()];
        let loop_config = ToolLoopConfig::telegram(12345);
        // allowed_tools should override denied_tools
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));
        // nexibot_filesystem still denied (not in allowed_tools)
        assert!(check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_filesystem"
        ));
    }

    #[test]
    fn test_discord_admin_bypass_with_sender_id() {
        let mut app_config = NexiBotConfig::default();
        app_config.discord.admin_user_ids = vec![42];
        let loop_config = ToolLoopConfig::discord(100, None, 42);
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));
    }

    #[test]
    fn test_background_never_denied() {
        let app_config = NexiBotConfig::default();
        let loop_config = ToolLoopConfig::background();
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));
    }

    #[test]
    fn test_background_with_origin_respects_channel_policy() {
        let mut app_config = NexiBotConfig::default();
        app_config.telegram.tool_policy.denied_tools = vec!["nexibot_execute".into()];

        let loop_config = ToolLoopConfig::background_with_origin(
            Some(ChannelSource::Telegram { chat_id: 12345 }),
            Some("12345".into()),
        );
        assert!(check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));
    }

    // =========================================================================
    // Simulated channel policy tests — validate that config changes
    // for tool_policy, denied_tools, allowed_tools, and admin_bypass
    // are properly respected across ALL channels.
    // =========================================================================

    /// Helper to test all channels with a given tool policy mutation
    fn test_all_channels_with_policy(
        mutate: impl Fn(&mut NexiBotConfig),
        tool_name: &str,
        expect_allowed: bool,
    ) {
        let channels: Vec<(&str, Box<dyn Fn(&NexiBotConfig) -> (ToolLoopConfig, bool)>)> = vec![
            (
                "telegram",
                Box::new(|_| (ToolLoopConfig::telegram(12345), true)),
            ),
            (
                "whatsapp",
                Box::new(|_| (ToolLoopConfig::whatsapp("+1234567890".into()), true)),
            ),
            (
                "discord",
                Box::new(|_| (ToolLoopConfig::discord(100, None, 99), true)),
            ),
            (
                "slack",
                Box::new(|_| (ToolLoopConfig::slack("C123".into(), "U456".into()), true)),
            ),
            (
                "signal",
                Box::new(|_| (ToolLoopConfig::signal("+15551234567".into()), true)),
            ),
            (
                "teams",
                Box::new(|_| (ToolLoopConfig::teams("conv1".into(), "user1".into()), true)),
            ),
            (
                "matrix",
                Box::new(|_| {
                    (
                        ToolLoopConfig::matrix("!r:m.org".into(), "@u:m.org".into()),
                        true,
                    )
                }),
            ),
            (
                "bluebubbles",
                Box::new(|_| {
                    (
                        ToolLoopConfig::bluebubbles("chat-guid".into(), "+15550001111".into()),
                        true,
                    )
                }),
            ),
            (
                "google_chat",
                Box::new(|_| {
                    (
                        ToolLoopConfig::google_chat("spaces/AAA".into(), "users/123".into()),
                        true,
                    )
                }),
            ),
            (
                "mattermost",
                Box::new(|_| {
                    (
                        ToolLoopConfig::mattermost("channel-1".into(), "user-1".into()),
                        true,
                    )
                }),
            ),
            (
                "messenger",
                Box::new(|_| (ToolLoopConfig::messenger("psid-1".into()), true)),
            ),
            (
                "instagram",
                Box::new(|_| (ToolLoopConfig::instagram("igid-1".into()), true)),
            ),
            (
                "line",
                Box::new(|_| {
                    (
                        ToolLoopConfig::line("line-user-1".into(), "line-user-1".into()),
                        true,
                    )
                }),
            ),
            (
                "twilio",
                Box::new(|_| (ToolLoopConfig::twilio("+15551230000".into()), true)),
            ),
            (
                "mastodon",
                Box::new(|_| (ToolLoopConfig::mastodon("acct-1".into()), true)),
            ),
            (
                "rocketchat",
                Box::new(|_| {
                    (
                        ToolLoopConfig::rocketchat("room-1".into(), "user-1".into()),
                        true,
                    )
                }),
            ),
            (
                "webchat",
                Box::new(|_| (ToolLoopConfig::webchat("session-1".into()), true)),
            ),
            ("gui", Box::new(|_| (ToolLoopConfig::gui_default(), false))),
            (
                "background",
                Box::new(|_| (ToolLoopConfig::background(), false)),
            ),
        ];

        for (name, make_config) in &channels {
            let mut app_config = NexiBotConfig::default();
            mutate(&mut app_config);
            let (loop_config, is_external) = make_config(&app_config);
            let denied = check_channel_tool_policy(&app_config, &loop_config, tool_name);

            if !is_external {
                // GUI and background should never be denied
                assert!(!denied, "Channel '{}' should never deny tools", name);
            } else if expect_allowed {
                assert!(
                    !denied,
                    "Channel '{}' should allow '{}' with given config",
                    name, tool_name
                );
            } else {
                assert!(
                    denied,
                    "Channel '{}' should deny '{}' with given config",
                    name, tool_name
                );
            }
        }
    }

    #[test]
    fn test_empty_denied_tools_allows_all_on_all_channels() {
        test_all_channels_with_policy(
            |cfg| {
                cfg.telegram.tool_policy.denied_tools = vec![];
                cfg.whatsapp.tool_policy.denied_tools = vec![];
                cfg.discord.tool_policy.denied_tools = vec![];
                cfg.slack.tool_policy.denied_tools = vec![];
                cfg.signal.tool_policy.denied_tools = vec![];
                cfg.teams.tool_policy.denied_tools = vec![];
                cfg.matrix.tool_policy.denied_tools = vec![];
                cfg.bluebubbles.tool_policy.denied_tools = vec![];
                cfg.google_chat.tool_policy.denied_tools = vec![];
                cfg.mattermost.tool_policy.denied_tools = vec![];
                cfg.messenger.tool_policy.denied_tools = vec![];
                cfg.instagram.tool_policy.denied_tools = vec![];
                cfg.line.tool_policy.denied_tools = vec![];
                cfg.twilio.tool_policy.denied_tools = vec![];
                cfg.mastodon.tool_policy.denied_tools = vec![];
                cfg.rocketchat.tool_policy.denied_tools = vec![];
                cfg.webchat.tool_policy.denied_tools = vec![];
            },
            "nexibot_execute",
            true, // should be allowed
        );
    }

    #[test]
    fn test_explicit_deny_blocks_on_all_channels() {
        test_all_channels_with_policy(
            |cfg| {
                let deny = vec!["nexibot_execute".into()];
                cfg.telegram.tool_policy.denied_tools = deny.clone();
                cfg.whatsapp.tool_policy.denied_tools = deny.clone();
                cfg.discord.tool_policy.denied_tools = deny.clone();
                cfg.slack.tool_policy.denied_tools = deny.clone();
                cfg.signal.tool_policy.denied_tools = deny.clone();
                cfg.teams.tool_policy.denied_tools = deny.clone();
                cfg.matrix.tool_policy.denied_tools = deny.clone();
                cfg.bluebubbles.tool_policy.denied_tools = deny.clone();
                cfg.google_chat.tool_policy.denied_tools = deny.clone();
                cfg.mattermost.tool_policy.denied_tools = deny.clone();
                cfg.messenger.tool_policy.denied_tools = deny.clone();
                cfg.instagram.tool_policy.denied_tools = deny.clone();
                cfg.line.tool_policy.denied_tools = deny.clone();
                cfg.twilio.tool_policy.denied_tools = deny.clone();
                cfg.mastodon.tool_policy.denied_tools = deny.clone();
                cfg.rocketchat.tool_policy.denied_tools = deny.clone();
                cfg.webchat.tool_policy.denied_tools = deny;
            },
            "nexibot_execute",
            false, // should be denied
        );
    }

    #[test]
    fn test_config_change_propagates_removing_denied_tool() {
        // Simulate: start with deny, then "update config" to remove it
        let mut app_config = NexiBotConfig::default();
        app_config.telegram.tool_policy.denied_tools = vec!["nexibot_execute".into()];
        let loop_config = ToolLoopConfig::telegram(12345);
        assert!(
            check_channel_tool_policy(&app_config, &loop_config, "nexibot_execute"),
            "Should be denied before config change"
        );

        // "Config change" — remove execute from denied_tools
        app_config.telegram.tool_policy.denied_tools = vec![];
        assert!(
            !check_channel_tool_policy(&app_config, &loop_config, "nexibot_execute"),
            "Should be allowed after config change"
        );
    }

    #[test]
    fn test_config_change_propagates_adding_denied_tool() {
        let mut app_config = NexiBotConfig::default();
        let loop_config = ToolLoopConfig::telegram(12345);
        assert!(
            !check_channel_tool_policy(&app_config, &loop_config, "nexibot_execute"),
            "Should be allowed with default config"
        );

        // "Config change" — add execute to denied_tools
        app_config
            .telegram
            .tool_policy
            .denied_tools
            .push("nexibot_execute".into());
        assert!(
            check_channel_tool_policy(&app_config, &loop_config, "nexibot_execute"),
            "Should be denied after config change"
        );
    }

    #[test]
    fn test_default_config_allows_execute_on_telegram() {
        // This is the critical regression test — execute should work on Telegram
        // out of the box with default config
        let app_config = NexiBotConfig::default();
        let loop_config = ToolLoopConfig::telegram(12345);
        assert!(
            !check_channel_tool_policy(&app_config, &loop_config, "nexibot_execute"),
            "Default config must allow nexibot_execute on Telegram"
        );
    }

    #[test]
    fn test_default_config_denies_filesystem_on_all_external_channels() {
        let app_config = NexiBotConfig::default();
        let channels: Vec<(&str, ToolLoopConfig)> = vec![
            ("telegram", ToolLoopConfig::telegram(12345)),
            ("whatsapp", ToolLoopConfig::whatsapp("+1".into())),
            ("discord", ToolLoopConfig::discord(1, None, 1)),
            ("slack", ToolLoopConfig::slack("C".into(), "U".into())),
            ("signal", ToolLoopConfig::signal("+1".into())),
            ("teams", ToolLoopConfig::teams("c".into(), "u".into())),
            (
                "matrix",
                ToolLoopConfig::matrix("!r:m".into(), "@u:m".into()),
            ),
            (
                "bluebubbles",
                ToolLoopConfig::bluebubbles("chat-guid".into(), "+15550001111".into()),
            ),
            (
                "google_chat",
                ToolLoopConfig::google_chat("spaces/AAA".into(), "users/123".into()),
            ),
            (
                "mattermost",
                ToolLoopConfig::mattermost("channel-1".into(), "user-1".into()),
            ),
            ("messenger", ToolLoopConfig::messenger("psid-1".into())),
            ("instagram", ToolLoopConfig::instagram("igid-1".into())),
            (
                "line",
                ToolLoopConfig::line("line-user-1".into(), "line-user-1".into()),
            ),
            ("twilio", ToolLoopConfig::twilio("+15551230000".into())),
            ("mastodon", ToolLoopConfig::mastodon("acct-1".into())),
            (
                "rocketchat",
                ToolLoopConfig::rocketchat("room-1".into(), "user-1".into()),
            ),
            ("webchat", ToolLoopConfig::webchat("session-1".into())),
        ];

        for (name, loop_config) in &channels {
            assert!(
                check_channel_tool_policy(&app_config, loop_config, "nexibot_filesystem"),
                "Default config should deny nexibot_filesystem on {} channel",
                name
            );
        }
    }

    #[test]
    fn test_admin_bypass_works_across_all_channels() {
        let mut cfg = NexiBotConfig::default();
        // Deny execute on all channels
        let deny = vec!["nexibot_execute".into()];
        cfg.telegram.tool_policy.denied_tools = deny.clone();
        cfg.whatsapp.tool_policy.denied_tools = deny.clone();
        cfg.discord.tool_policy.denied_tools = deny.clone();
        cfg.slack.tool_policy.denied_tools = deny.clone();
        cfg.signal.tool_policy.denied_tools = deny.clone();
        cfg.teams.tool_policy.denied_tools = deny.clone();
        cfg.matrix.tool_policy.denied_tools = deny.clone();
        cfg.bluebubbles.tool_policy.denied_tools = deny.clone();
        cfg.google_chat.tool_policy.denied_tools = deny.clone();
        cfg.mattermost.tool_policy.denied_tools = deny.clone();
        cfg.messenger.tool_policy.denied_tools = deny.clone();
        cfg.instagram.tool_policy.denied_tools = deny.clone();
        cfg.line.tool_policy.denied_tools = deny.clone();
        cfg.twilio.tool_policy.denied_tools = deny.clone();
        cfg.mastodon.tool_policy.denied_tools = deny.clone();
        cfg.rocketchat.tool_policy.denied_tools = deny.clone();
        cfg.webchat.tool_policy.denied_tools = deny;

        // Set admins
        cfg.telegram.admin_chat_ids = vec![12345];
        cfg.whatsapp.admin_phone_numbers = vec!["+1234567890".into()];
        cfg.discord.admin_user_ids = vec![99];
        cfg.slack.admin_user_ids = vec!["U456".into()];
        cfg.signal.admin_numbers = vec!["+15551234567".into()];
        cfg.teams.admin_user_ids = vec!["user1".into()];
        cfg.matrix.admin_user_ids = vec!["@u:m.org".into()];
        cfg.bluebubbles.admin_handles = vec!["+15550001111".into()];
        cfg.google_chat.admin_user_ids = vec!["users/123".into()];
        cfg.mattermost.admin_user_ids = vec!["user-1".into()];
        cfg.messenger.admin_sender_ids = vec!["psid-1".into()];
        cfg.instagram.admin_sender_ids = vec!["igid-1".into()];
        cfg.line.admin_user_ids = vec!["line-user-1".into()];
        cfg.twilio.admin_numbers = vec!["+15551230000".into()];
        cfg.mastodon.admin_account_ids = vec!["acct-1".into()];
        cfg.rocketchat.admin_user_ids = vec!["user-1".into()];

        let tests: Vec<(&str, ToolLoopConfig)> = vec![
            ("telegram", ToolLoopConfig::telegram(12345)),
            ("whatsapp", ToolLoopConfig::whatsapp("+1234567890".into())),
            ("discord", ToolLoopConfig::discord(100, None, 99)),
            ("slack", ToolLoopConfig::slack("C123".into(), "U456".into())),
            ("signal", ToolLoopConfig::signal("+15551234567".into())),
            (
                "teams",
                ToolLoopConfig::teams("conv1".into(), "user1".into()),
            ),
            (
                "matrix",
                ToolLoopConfig::matrix("!r:m.org".into(), "@u:m.org".into()),
            ),
            (
                "bluebubbles",
                ToolLoopConfig::bluebubbles("chat-guid".into(), "+15550001111".into()),
            ),
            (
                "google_chat",
                ToolLoopConfig::google_chat("spaces/AAA".into(), "users/123".into()),
            ),
            (
                "mattermost",
                ToolLoopConfig::mattermost("channel-1".into(), "user-1".into()),
            ),
            ("messenger", ToolLoopConfig::messenger("psid-1".into())),
            ("instagram", ToolLoopConfig::instagram("igid-1".into())),
            (
                "line",
                ToolLoopConfig::line("line-user-1".into(), "line-user-1".into()),
            ),
            ("twilio", ToolLoopConfig::twilio("+15551230000".into())),
            ("mastodon", ToolLoopConfig::mastodon("acct-1".into())),
            (
                "rocketchat",
                ToolLoopConfig::rocketchat("room-1".into(), "user-1".into()),
            ),
        ];

        for (name, loop_config) in &tests {
            assert!(
                !check_channel_tool_policy(&cfg, loop_config, "nexibot_execute"),
                "Admin on {} should bypass tool policy denial",
                name
            );
        }

        // WebChat has no admin concept; denied tools remain denied.
        let webchat = ToolLoopConfig::webchat("session-1".into());
        assert!(
            check_channel_tool_policy(&cfg, &webchat, "nexibot_execute"),
            "WebChat should remain denied because there is no admin bypass identity"
        );
    }

    #[test]
    fn test_channel_tool_policy_reads_live_config() {
        // Simulates hot-reload: check that policy function uses the config
        // it's given, not any cached state
        let mut app_config = NexiBotConfig::default();
        let loop_config = ToolLoopConfig::telegram(12345);

        // Round 1: default (execute allowed)
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));

        // Round 2: deny execute
        app_config
            .telegram
            .tool_policy
            .denied_tools
            .push("nexibot_execute".into());
        assert!(check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));

        // Round 3: allow it back via allowed_tools override
        app_config
            .telegram
            .tool_policy
            .allowed_tools
            .push("nexibot_execute".into());
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));

        // Round 4: remove from denied entirely and clear allowed
        app_config
            .telegram
            .tool_policy
            .denied_tools
            .retain(|t| t != "nexibot_execute");
        app_config.telegram.tool_policy.allowed_tools.clear();
        assert!(!check_channel_tool_policy(
            &app_config,
            &loop_config,
            "nexibot_execute"
        ));
    }
}
