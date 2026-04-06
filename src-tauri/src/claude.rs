//! Claude API client with streaming support via Anthropic Bridge Service

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::channel::ChannelSource;
use crate::config::{default_max_tokens_for_model, NexiBotConfig};
use crate::llm_provider::{capabilities, LlmProvider};
use crate::memory::{MemoryManager, SessionMessage};
use crate::oauth::AuthProfileManager;
use crate::session_overrides::SessionOverrides;
use crate::skills::SkillsManager;
use crate::soul::Soul;
use crate::tool_converter;

/// Returns the current date/time and environment context for injection into system prompts.
fn current_datetime_context() -> String {
    let now = chrono::Local::now();
    let home = dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    // Discover common workspace directories
    let mut workspaces = Vec::new();
    if let Some(home_dir) = dirs::home_dir() {
        for candidate in &[
            "gitrepos",
            "projects",
            "repos",
            "code",
            "dev",
            "workspace",
            "src",
        ] {
            let path = home_dir.join(candidate);
            if path.is_dir() {
                workspaces.push(path.to_string_lossy().to_string());
            }
        }
    }

    let mut ctx = format!(
        "Current date and time: {} ({})\nHome directory: {}",
        now.format("%A, %B %-d, %Y at %-I:%M %p"),
        now.format("%Z"),
        home,
    );

    if !workspaces.is_empty() {
        ctx.push_str(&format!(
            "\nWorkspace directories: {}",
            workspaces.join(", ")
        ));
    }
    ctx.push_str(&format!("\nDocuments: {}/Documents", home));

    ctx
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct StreamChunk {
    pub r#type: String,
    pub delta: Option<Delta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Delta {
    pub r#type: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeResponse {
    pub id: String,
    pub r#type: String,
    pub role: String,
    pub content: Vec<serde_json::Value>,
    pub model: String,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ContentBlock {
    pub r#type: String,
    #[serde(default)]
    pub text: Option<String>,
}

/// A tool use request from Claude
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseBlock {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Structured response from send_message_with_tools
#[derive(Debug, Clone)]
pub struct ClaudeMessageResult {
    /// Text content from the response
    pub text: String,
    /// Tool use requests (if stop_reason == "tool_use")
    pub tool_uses: Vec<ToolUseBlock>,
    /// The stop reason
    pub stop_reason: String,
    /// Raw content blocks for conversation history
    pub raw_content: Vec<serde_json::Value>,
    /// Number of tool-loop iterations executed (set by execute_tool_loop)
    pub tool_calls_made: usize,
}

/// Result of a conversation compaction operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionResult {
    pub messages_before: usize,
    pub messages_after: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub was_compacted: bool,
}

/// Maximum number of messages to retain in conversation history.
/// Configurable at runtime via ClaudeClient::set_max_history_messages().
/// Default: 200 (matches config default).
static MAX_HISTORY_MESSAGES: AtomicUsize = AtomicUsize::new(200);

/// Cumulative count of tool-pairing 400 errors detected across all sessions.
pub(crate) static TOOL_PAIRING_ERRORS: AtomicUsize = AtomicUsize::new(0);
/// Cumulative count of successful auto-recoveries from tool-pairing errors.
pub(crate) static TOOL_PAIRING_RECOVERIES: AtomicUsize = AtomicUsize::new(0);

/// Number of recent messages to preserve during compaction (not summarized).
/// Should be even to maintain user/assistant pairing.
const COMPACT_PRESERVE_RECENT: usize = 6;

/// Model to use for summarization (cheaper/faster than primary model).
const SUMMARIZATION_MODEL: &str = "claude-haiku-4-5-20250929";

/// System prompt suffix appended for voice interactions so Claude responds
/// in a way that sounds natural when read aloud by TTS.
const VOICE_MODE_PROMPT: &str = "\n\n\
## Voice Conversation Mode\n\
You are in a live voice conversation. Your response will be spoken aloud via TTS.\n\
\n\
Rules:\n\
- NO markdown: no asterisks, backticks, headers, bullets, numbered lists, or links.\n\
- Talk like a knowledgeable friend. Be concise and natural.\n\
- Never include URLs, file paths, or code snippets — they sound terrible spoken aloud.\n\
- If you have tools available, USE them silently to fulfill the request.\n\
  Do NOT describe what tools you would use. Just do it, then report results.\n\
- After using tools, give a brief spoken summary of what you did and found.\n\
- When writing documents or files, write the FULL content to disk, but only \n\
  SPEAK a brief summary (2-3 sentences about key findings).\n\
- If a task will take time, acknowledge it: 'Let me look into that for you.'\n\
- Only say what's worth hearing. Skip meta-commentary about your process.\n\
- If you don't know, say so briefly. Don't speculate at length.";

/// Claude API client for conversational interactions
/// Routes requests through Anthropic Bridge Service for OAuth support
#[derive(Clone)]
pub struct ClaudeClient {
    config: Arc<RwLock<NexiBotConfig>>,
    http_client: reqwest::Client,
    conversation_history: Arc<RwLock<Vec<Message>>>,
    bridge_url: String,
    /// Model chosen by the query classifier for the current tool-loop turn.
    /// Set at the start of each new user message; read by continue_after_tools
    /// so that all iterations within a single turn use a consistent model.
    current_routing_model: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl ClaudeClient {
    /// Create a new Claude client
    pub fn new(config: Arc<RwLock<NexiBotConfig>>) -> Self {
        // Seed MAX_HISTORY_MESSAGES from config so the limit is correct before the
        // config_changed subscriber fires for the first time.
        if let Ok(cfg) = config.try_read() {
            MAX_HISTORY_MESSAGES.store(cfg.claude.max_history_messages, Ordering::Relaxed);
        }

        let bridge_url = {
            let raw = std::env::var("ANTHROPIC_BRIDGE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:18790".to_string());
            if let Err(e) = crate::security::ssrf::validate_loopback_url(&raw) {
                tracing::error!("[CLAUDE] {e}; falling back to default loopback URL");
                "http://127.0.0.1:18790".to_string()
            } else {
                raw
            }
        };

        info!("[CLAUDE] Using Anthropic Bridge at: {}", bridge_url);

        Self {
            config,
            current_routing_model: Arc::new(tokio::sync::Mutex::new(None)),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .tcp_keepalive(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            conversation_history: Arc::new(RwLock::new(Vec::new())),
            bridge_url,
        }
    }

    /// Get effective max_tokens: use config value if set (non-zero), otherwise model default.
    fn effective_max_tokens(model: &str, configured: usize) -> usize {
        if configured == 0 {
            default_max_tokens_for_model(model)
        } else {
            configured
        }
    }

    /// Check if a token is an OAuth token (vs regular API key)
    #[allow(dead_code)]
    fn is_oauth_token(token: &str) -> bool {
        // OAuth tokens contain "oat" (OAuth Access Token)
        token.contains("sk-ant-oat")
    }

    /// Get authentication token - tries OAuth profile, then Claude Code keychain, then API key.
    /// Serialized via a static mutex to prevent concurrent token refresh races.
    async fn get_auth_token(&self) -> Result<String> {
        static AUTH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _guard = AUTH_LOCK.lock().await;

        info!("[AUTH] Attempting to get authentication token...");

        // 1. Try NexiBot's own OAuth profile (with auto-refresh)
        match AuthProfileManager::load() {
            Ok(mut manager) => {
                info!("[AUTH] OAuth profile manager loaded successfully");
                if let Some(profile) = manager.get_default_profile("anthropic") {
                    info!("[AUTH] Found Anthropic OAuth profile");
                    match profile.get_valid_token().await {
                        Ok(token) => {
                            info!(
                                "[AUTH] Got valid OAuth token starting with: {}...",
                                &token[..20.min(token.len())]
                            );
                            // Save updated profile (in case token was refreshed)
                            if let Err(e) = manager.save() {
                                warn!(
                                    "[AUTH] Failed to persist refreshed OAuth profile after token refresh: {}",
                                    e
                                );
                            }
                            return Ok(token);
                        }
                        Err(e) => {
                            warn!("[AUTH] Failed to get OAuth token: {}", e);
                        }
                    }
                } else {
                    info!("[AUTH] No Anthropic OAuth profile found");
                }
            }
            Err(e) => {
                info!("[AUTH] OAuth profile manager not available: {}", e);
            }
        }

        // 2. Fallback: try to read fresh tokens from Claude Code's keychain
        info!("[AUTH] Attempting Claude Code keychain fallback...");
        match Self::read_claude_code_keychain_token() {
            Ok(token) => {
                info!("[AUTH] Got token from Claude Code keychain");
                return Ok(token);
            }
            Err(e) => {
                info!("[AUTH] Claude Code keychain not available: {}", e);
            }
        }

        // 3. Fall back to API key from config
        info!("[AUTH] Attempting to use API key from config");
        let config = self.config.read().await;
        let api_key = config
            .claude
            .api_key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .context("No Claude authentication configured (neither OAuth, Claude Code keychain, nor API key)")?;
        info!(
            "[AUTH] Got API key from config starting with: {}...",
            &api_key[..20.min(api_key.len())]
        );
        Ok(api_key)
    }

    /// Read a valid OAuth token from Claude Code's macOS keychain.
    #[cfg(target_os = "macos")]
    fn read_claude_code_keychain_token() -> Result<String> {
        use std::process::Command;

        let service_names = ["Claude Code-credentials", "claude-cli", "anthropic-claude"];

        for service_name in &service_names {
            let output = Command::new("security")
                .args(["find-generic-password", "-s", service_name, "-w"])
                .output();

            let output = match output {
                Ok(o) if o.status.success() => o,
                _ => continue,
            };

            let token_data = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Ok(tokens) = serde_json::from_str::<serde_json::Value>(&token_data) {
                let token_obj = tokens.get("claudeAiOauth").unwrap_or(&tokens);

                let session_key = token_obj
                    .get("sessionKey")
                    .or_else(|| token_obj.get("access_token"))
                    .or_else(|| token_obj.get("accessToken"))
                    .and_then(|v| v.as_str());

                if let Some(key) = session_key {
                    // Check expiry if available
                    if let Some(expires_at_ms) = token_obj.get("expiresAt").and_then(|v| v.as_u64())
                    {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        if expires_at_ms <= now_ms + 60_000 {
                            // Token expired or expiring within 1 minute
                            continue;
                        }
                    }
                    info!("[AUTH] Found valid token from {} keychain", service_name);
                    return Ok(key.to_string());
                }
            }
        }
        anyhow::bail!("No valid Claude Code token found in keychain")
    }

    #[cfg(not(target_os = "macos"))]
    fn read_claude_code_keychain_token() -> Result<String> {
        // On non-macOS, try reading from ~/.claude/.credentials.json
        let home = dirs::home_dir().context("Could not determine home directory")?;
        let creds_path = home.join(".claude").join(".credentials.json");
        if !creds_path.exists() {
            anyhow::bail!("No Claude Code credentials file found");
        }
        let contents = std::fs::read_to_string(&creds_path)?;
        let tokens: serde_json::Value = serde_json::from_str(&contents)?;
        let token_obj = tokens.get("claudeAiOauth").unwrap_or(&tokens);
        let key = token_obj
            .get("sessionKey")
            .or_else(|| token_obj.get("access_token"))
            .and_then(|v| v.as_str())
            .context("No token found in credentials file")?;
        Ok(key.to_string())
    }

    /// Send a message to Claude and get a streaming response via bridge
    #[allow(dead_code)]
    pub async fn send_message_streaming(
        &self,
        message: &str,
        callback: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<String> {
        let api_key = self.get_auth_token().await?;
        let config = self.config.read().await;

        let system_prompt = Self::build_full_system_prompt(&config, None);

        // Add user message to history
        let mut history = self.conversation_history.write().await;
        history.push(Message {
            role: "user".to_string(),
            content: message.to_string(),
        });
        Self::trim_history(&mut history);

        // Prepare request for bridge service
        let max_tokens = Self::effective_max_tokens(&config.claude.model, config.claude.max_tokens);
        let request_body = json!({
            "apiKey": api_key,
            "model": config.claude.model,
            "max_tokens": max_tokens,
            "system": system_prompt,
            "messages": *history,
        });

        info!("[BRIDGE] Sending streaming request to bridge");
        info!("[BRIDGE] Model: {}", config.claude.model);
        info!("[BRIDGE] Messages: {}", history.len());
        info!("[BRIDGE] OAuth: {}", Self::is_oauth_token(&api_key));

        // Make streaming request to bridge
        let mut bridge_req = self
            .http_client
            .post(format!("{}/api/messages/stream", self.bridge_url))
            .header("content-type", "application/json");
        if let Some(secret) = crate::bridge::get_bridge_secret() {
            bridge_req = bridge_req.header("x-bridge-secret", secret);
        }
        let mut response = match bridge_req
            .json(&request_body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("[BRIDGE] Request failed, rolling back user message from history");
                history.pop();
                return Err(e).context("Failed to send request to Anthropic Bridge");
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            warn!("[BRIDGE] Streaming request failed with status: {}", status);
            warn!("[BRIDGE] Error response: {}", error_text);
            history.pop();
            anyhow::bail!("Bridge error: {}", error_text);
        }

        info!("[BRIDGE] Streaming response started");

        // Process SSE stream in real-time (true streaming, not buffered)
        let mut full_response = String::new();
        let mut line_buffer = String::new();
        const MAX_LINE_BUFFER: usize = 1024 * 1024; // 1 MB

        while let Some(chunk_result) = response.chunk().await.transpose() {
            let chunk_bytes = match chunk_result {
                Ok(b) => b,
                Err(e) => {
                    warn!("[BRIDGE] Stream read error, rolling back: {}", e);
                    history.pop();
                    return Err(e).context("Failed to read streaming chunk");
                }
            };
            let chunk_text = String::from_utf8_lossy(&chunk_bytes);
            line_buffer.push_str(&chunk_text);
            if line_buffer.len() > MAX_LINE_BUFFER {
                anyhow::bail!("Streaming line buffer exceeded 1 MB — possible malformed response");
            }

            // Process complete lines from the buffer
            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].trim_end_matches('\r').to_string();
                line_buffer = line_buffer[newline_pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break;
                    }

                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                        // Handle error events
                        if parsed["type"] == "error" {
                            let error_msg = parsed["error"]["message"]
                                .as_str()
                                .unwrap_or("Unknown error");
                            warn!("[BRIDGE] API error: {}", error_msg);
                            history.pop();
                            anyhow::bail!("Claude API error: {}", error_msg);
                        }

                        // Handle text deltas
                        if parsed["type"] == "content_block_delta" {
                            if let Some(text) = parsed["delta"]["text"].as_str() {
                                full_response.push_str(text);
                                callback(text.to_string());
                            }
                        }
                    }
                }
            }
        }

        // Add assistant response to history
        history.push(Message {
            role: "assistant".to_string(),
            content: full_response.clone(),
        });
        Self::trim_history(&mut history);

        info!(
            "[BRIDGE] Streaming complete: {} characters",
            full_response.len()
        );

        Ok(full_response)
    }

    /// Send a message to Claude with voice-optimized system prompt, streaming response.
    /// Uses `ChannelSource::Voice` so Claude responds in a natural spoken style.
    pub async fn send_message_streaming_for_voice(
        &self,
        message: &str,
        callback: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<String> {
        let api_key = self.get_auth_token().await?;
        let config = self.config.read().await;

        let system_prompt = Self::build_full_system_prompt(&config, Some(&ChannelSource::Voice));

        // Add user message to history
        let mut history = self.conversation_history.write().await;
        history.push(Message {
            role: "user".to_string(),
            content: message.to_string(),
        });
        Self::trim_history(&mut history);

        // Prepare request for bridge service.
        // Voice responses are spoken aloud — cap at 500 tokens (~375 words) to keep
        // answers concise and natural for conversation.
        let max_tokens = 500.min(Self::effective_max_tokens(
            &config.claude.model,
            config.claude.max_tokens,
        ));
        let request_body = json!({
            "apiKey": api_key,
            "model": config.claude.model,
            "max_tokens": max_tokens,
            "system": system_prompt,
            "messages": *history,
        });

        info!(
            "[BRIDGE] Sending voice-mode streaming request (max_tokens={})",
            max_tokens
        );

        let mut voice_req = self
            .http_client
            .post(format!("{}/api/messages/stream", self.bridge_url))
            .header("content-type", "application/json");
        if let Some(secret) = crate::bridge::get_bridge_secret() {
            voice_req = voice_req.header("x-bridge-secret", secret);
        }
        let response = match voice_req
            .json(&request_body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("[BRIDGE] Voice request failed, rolling back user message from history");
                history.pop();
                return Err(e).context("Failed to send request to Anthropic Bridge");
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            warn!("[BRIDGE] Voice streaming request failed: {}", status);
            history.pop();
            anyhow::bail!("Bridge error: {}", error_text);
        }

        // Process SSE stream progressively (real streaming, not batch download).
        // This ensures TTS starts speaking as Claude generates, rather than waiting
        // for the entire response to finish.
        let mut full_response = String::new();
        let mut line_buffer = String::new();
        let mut stream_error: Option<String> = None;
        const MAX_LINE_BUFFER_TTS: usize = 1024 * 1024; // 1 MB

        use futures_util::StreamExt;
        let mut byte_stream = response.bytes_stream();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk_bytes = match chunk_result {
                Ok(b) => b,
                Err(e) => {
                    warn!("[BRIDGE] Stream read error: {}", e);
                    break;
                }
            };

            let chunk_text = String::from_utf8_lossy(&chunk_bytes);
            line_buffer.push_str(&chunk_text);
            if line_buffer.len() > MAX_LINE_BUFFER_TTS {
                warn!("[BRIDGE] TTS streaming line buffer exceeded 1 MB, aborting stream");
                break;
            }

            // Process complete lines from the buffer
            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].trim_end().to_string();
                line_buffer = line_buffer[newline_pos + 1..].to_string();

                if !line.starts_with("data: ") {
                    continue;
                }

                let data = &line[6..];
                if data == "[DONE]" {
                    break;
                }

                if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                    if event["type"] == "error" {
                        let error_msg = event["error"]["message"]
                            .as_str()
                            .unwrap_or("Unknown error");
                        stream_error = Some(error_msg.to_string());
                        break;
                    }

                    if event["type"] == "content_block_delta" {
                        if let Some(text) = event["delta"]["text"].as_str() {
                            full_response.push_str(text);
                            callback(text.to_string());
                        }
                    }
                }
            }
        }

        if let Some(error_msg) = stream_error {
            history.pop();
            anyhow::bail!("Claude API error: {}", error_msg);
        }

        // Add assistant response to history
        history.push(Message {
            role: "assistant".to_string(),
            content: full_response.clone(),
        });
        Self::trim_history(&mut history);

        info!(
            "[BRIDGE] Voice streaming complete: {} characters",
            full_response.len()
        );

        Ok(full_response)
    }

    /// Build the system prompt from SOUL, Skills, Memory, and Config
    fn build_system_prompt(config_system_prompt: &str, channel: Option<&ChannelSource>) -> String {
        let mut system_prompt = String::new();

        if let Ok(soul) = Soul::load() {
            let ctx = soul.get_system_prompt_context();
            if !ctx.is_empty() {
                system_prompt.push_str(&ctx);
                system_prompt.push_str("\n\n");
            }
        }

        if let Ok(manager) = SkillsManager::new() {
            let ctx = manager.get_skills_context();
            if !ctx.is_empty() {
                system_prompt.push_str(&ctx);
                system_prompt.push_str("\n\n");
            }
        }

        if let Ok(manager) = MemoryManager::new() {
            let ctx = manager.get_memory_context(20);
            if !ctx.is_empty() {
                system_prompt.push_str(&ctx);
                system_prompt.push_str("\n\n");
            }
        }

        // Git context (sync collection from working directory)
        {
            let cwd = std::env::current_dir().unwrap_or_default();
            let git_ctx = crate::git_context::collect_git_context_sync(&cwd);
            if !git_ctx.is_empty() {
                let git_section = format!(
                    "## Git Context\n{}\n",
                    git_ctx.to_prompt_string()
                );
                system_prompt.push_str(&git_section);
                system_prompt.push_str("\n\n");
                system_prompt.push_str(crate::git_context::GIT_SAFETY_RULES);
                system_prompt.push_str("\n\n");
            }
        }

        system_prompt.push_str(&current_datetime_context());
        system_prompt.push_str("\n\n");

        system_prompt.push_str(config_system_prompt);

        // Append voice-mode instructions when channel is Voice
        if let Some(ChannelSource::Voice) = channel {
            system_prompt.push_str(VOICE_MODE_PROMPT);
        }

        system_prompt
    }

    /// Assemble the complete system prompt including capabilities and permissions context.
    /// All send_message_* variants that route to the bridge use this so the LLM always
    /// sees a consistent, fully-populated system prompt.
    fn build_full_system_prompt(config: &NexiBotConfig, channel: Option<&ChannelSource>) -> String {
        let capabilities_ctx = Self::build_capabilities_context(config);
        let permissions_ctx = Self::build_permissions_context(config);
        let mut system_prompt = Self::build_system_prompt(&config.claude.system_prompt, channel);
        if !capabilities_ctx.is_empty() {
            system_prompt = format!("{}\n\n{}", capabilities_ctx, system_prompt);
        }
        if !permissions_ctx.is_empty() {
            system_prompt = format!("{}\n\n{}", permissions_ctx, system_prompt);
        }
        system_prompt
    }

    /// Build a dynamic capabilities context based on what's enabled in config.
    /// This tells the LLM what it can actually do so it doesn't hallucinate or deny capabilities.
    fn build_capabilities_context(config: &NexiBotConfig) -> String {
        let mut caps = Vec::new();

        // Scheduler
        if config.scheduled_tasks.enabled {
            let task_count = config.scheduled_tasks.tasks.len();
            caps.push(format!(
                "- **Scheduler**: You have a built-in task scheduler that runs in the background. \
                 You can schedule recurring tasks using formats like \"daily HH:MM\", \"hourly\", \
                 \"every Nm\", or \"weekly DAY HH:MM\". Tasks execute your prompts automatically. \
                 Currently {} task(s) configured. Users can manage tasks in Settings > Scheduler.",
                task_count
            ));
        } else {
            caps.push(
                "- **Scheduler**: You have a built-in task scheduler (currently disabled). \
                 Users can enable it in Settings > Scheduler. It supports daily, hourly, \
                 weekly, and interval-based recurring tasks that run your prompts automatically."
                    .to_string(),
            );
        }

        // K2K / Supermemory
        if config.k2k.enabled {
            caps.push(format!(
                "- **K2K Local Agent**: Connected to the local Knowledge Nexus System Agent at {}. \
                 You can search the user's local indexed files via semantic search.",
                config.k2k.local_agent_url
            ));
            if config.k2k.supermemory_enabled {
                caps.push(
                    "- **Supermemory**: Conversations are automatically synced to the System Agent \
                     as persistent long-term memory that survives across sessions."
                        .to_string(),
                );
            }
        }

        // MCP
        if config.mcp.enabled && !config.mcp.servers.is_empty() {
            let server_names: Vec<&str> = config
                .mcp
                .servers
                .iter()
                .filter(|s| s.enabled)
                .map(|s| s.name.as_str())
                .collect();
            if !server_names.is_empty() {
                caps.push(format!(
                    "- **MCP Servers**: {} active: {}",
                    server_names.len(),
                    server_names.join(", ")
                ));
            }
        }

        // Filesystem
        if config.filesystem.enabled {
            caps.push(
                "- **Filesystem**: You can read and write files on the user's computer."
                    .to_string(),
            );
        }

        // Execute
        if config.execute.enabled {
            caps.push(
                "- **Command Execution**: You can run shell commands on the user's computer."
                    .to_string(),
            );
        }

        // Web search
        let has_search =
            config.search.brave_api_key.is_some() || config.search.tavily_api_key.is_some();
        if has_search {
            caps.push(
                "- **Web Search**: You can search the web for current information.".to_string(),
            );
        }

        // Fetch
        if config.fetch.enabled {
            caps.push("- **Web Fetch**: You can fetch and read web pages.".to_string());
        }

        // Browser
        if config.browser.enabled {
            caps.push("- **Browser Automation**: You can control a headless browser.".to_string());
        }

        // Voice
        if config.audio.enabled {
            caps.push(
                "- **Voice**: Audio input/output is enabled for voice conversations.".to_string(),
            );
        }

        if caps.is_empty() {
            return String::new();
        }

        format!("## Your Capabilities\n\n{}", caps.join("\n"))
    }

    /// Build a permissions context for the system prompt when autonomous mode is enabled.
    /// This tells the LLM what it can and cannot do without asking.
    fn build_permissions_context(config: &NexiBotConfig) -> String {
        use crate::config::AutonomyLevel;

        if !config.autonomous_mode.enabled {
            return String::new();
        }

        let mut can_do = Vec::new();
        let mut cannot_do = Vec::new();

        // Filesystem
        if config.autonomous_mode.filesystem.read == AutonomyLevel::Autonomous {
            can_do.push("- Read files within allowed paths");
        } else if config.autonomous_mode.filesystem.read == AutonomyLevel::Blocked {
            cannot_do.push("- Read files (disabled by user)");
        }
        if config.autonomous_mode.filesystem.write == AutonomyLevel::Autonomous {
            can_do.push("- Write and create files");
        } else if config.autonomous_mode.filesystem.write == AutonomyLevel::Blocked {
            cannot_do.push("- Write or create files (disabled by user)");
        }
        if config.autonomous_mode.filesystem.delete == AutonomyLevel::Autonomous {
            can_do.push("- Delete files");
        } else if config.autonomous_mode.filesystem.delete == AutonomyLevel::Blocked {
            cannot_do.push("- Delete files (disabled by user)");
        }

        // Execute
        if config.autonomous_mode.execute.run_command == AutonomyLevel::Autonomous {
            can_do.push("- Run shell commands");
        } else if config.autonomous_mode.execute.run_command == AutonomyLevel::Blocked {
            cannot_do.push("- Run shell commands (disabled by user)");
        }
        if config.autonomous_mode.execute.run_python == AutonomyLevel::Autonomous {
            can_do.push("- Run Python scripts");
        } else if config.autonomous_mode.execute.run_python == AutonomyLevel::Blocked {
            cannot_do.push("- Run Python scripts (disabled by user)");
        }
        if config.autonomous_mode.execute.run_node == AutonomyLevel::Autonomous {
            can_do.push("- Run Node.js scripts");
        } else if config.autonomous_mode.execute.run_node == AutonomyLevel::Blocked {
            cannot_do.push("- Run Node.js scripts (disabled by user)");
        }

        // Fetch
        if config.autonomous_mode.fetch.get_requests == AutonomyLevel::Autonomous {
            can_do.push("- Fetch web pages (GET requests)");
        } else if config.autonomous_mode.fetch.get_requests == AutonomyLevel::Blocked {
            cannot_do.push("- Fetch web pages (disabled by user)");
        }
        if config.autonomous_mode.fetch.post_requests == AutonomyLevel::Autonomous {
            can_do.push("- Make POST/PUT/DELETE requests");
        } else if config.autonomous_mode.fetch.post_requests == AutonomyLevel::Blocked {
            cannot_do.push("- Make POST/PUT/DELETE requests (disabled by user)");
        }

        // Browser
        if config.autonomous_mode.browser.navigate == AutonomyLevel::Autonomous {
            can_do.push("- Navigate browser to URLs");
        } else if config.autonomous_mode.browser.navigate == AutonomyLevel::Blocked {
            cannot_do.push("- Navigate browser (disabled by user)");
        }
        if config.autonomous_mode.browser.interact == AutonomyLevel::Autonomous {
            can_do.push("- Interact with browser (click, type, etc.)");
        } else if config.autonomous_mode.browser.interact == AutonomyLevel::Blocked {
            cannot_do.push("- Interact with browser (disabled by user)");
        }

        // Computer use
        if config.autonomous_mode.computer_use.level == AutonomyLevel::Autonomous {
            can_do.push("- Use computer (mouse, keyboard, screenshots)");
        } else if config.autonomous_mode.computer_use.level == AutonomyLevel::Blocked {
            cannot_do.push("- Use computer control (disabled by user)");
        }

        // Self-modification
        if config.autonomous_mode.settings_modification.level == AutonomyLevel::Autonomous {
            can_do.push("- Modify your own settings");
        } else if config.autonomous_mode.settings_modification.level == AutonomyLevel::Blocked {
            cannot_do.push("- Modify settings (disabled by user)");
        }
        if config.autonomous_mode.memory_modification.level == AutonomyLevel::Autonomous {
            can_do.push("- Access and modify memory");
        } else if config.autonomous_mode.memory_modification.level == AutonomyLevel::Blocked {
            cannot_do.push("- Modify memory (disabled by user)");
        }
        if config.autonomous_mode.soul_modification.level == AutonomyLevel::Autonomous {
            can_do.push("- Modify your soul/personality");
        } else if config.autonomous_mode.soul_modification.level == AutonomyLevel::Blocked {
            cannot_do.push("- Modify your soul/personality (disabled by user)");
        }

        let mut sections = Vec::new();
        sections.push("## Your Permissions (Autonomous Mode)\n".to_string());

        if !can_do.is_empty() {
            sections.push("### You CAN do these without asking:".to_string());
            sections.push(can_do.join("\n"));
        }

        if !cannot_do.is_empty() {
            sections.push("\n### You CANNOT do these (refuse and explain if asked):".to_string());
            sections.push(cannot_do.join("\n"));
        }

        sections.push("\n### Hard Safety Limits (always enforced, never attempt):".to_string());
        sections.push("- NEVER execute: rm -rf /, mkfs, fork bombs, dd to raw devices".to_string());
        sections.push(
            "- NEVER access system directories: /etc, /System, /usr, /var, /bin, /sbin".to_string(),
        );
        sections.push(
            "- NEVER expose API keys, passwords, private keys, or credit card numbers".to_string(),
        );
        sections.push("- NEVER modify or delete the config file directly".to_string());

        sections.push("\n### For everything else, proceed with your best judgment.".to_string());

        sections.join("\n")
    }

    /// Extract text from response content blocks
    fn extract_text(content: &[serde_json::Value]) -> String {
        content
            .iter()
            .filter_map(|block| {
                if block.get("type")?.as_str()? == "text" {
                    block.get("text")?.as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Extract tool_use blocks from response content
    fn extract_tool_uses(content: &[serde_json::Value]) -> Vec<ToolUseBlock> {
        content
            .iter()
            .filter_map(|block| {
                if block.get("type")?.as_str()? == "tool_use" {
                    Some(ToolUseBlock {
                        id: block.get("id")?.as_str()?.to_string(),
                        name: block.get("name")?.as_str()?.to_string(),
                        input: block.get("input")?.clone(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Send a message to Claude (non-streaming) via bridge
    pub async fn send_message(&self, message: &str) -> Result<String> {
        let default_overrides = SessionOverrides::default();
        let result = self
            .send_message_with_tools(message, &[], &default_overrides)
            .await?;
        Ok(result.text)
    }

    /// Check if tools list contains Computer Use tools
    fn has_computer_use_tools(tools: &[serde_json::Value]) -> bool {
        tools.iter().any(|t| {
            t.get("type")
                .and_then(|v| v.as_str())
                .map(|s| s.starts_with("computer_"))
                .unwrap_or(false)
        })
    }

    /// Get the API key for the specified provider.
    async fn get_provider_api_key(&self, provider: LlmProvider) -> Result<String> {
        match provider {
            LlmProvider::Anthropic => self.get_auth_token().await,
            LlmProvider::OpenAI => {
                // 1. Try OAuth profile first (device code flow / ChatGPT subscription)
                if let Ok(mut manager) = AuthProfileManager::load() {
                    if let Some(profile) = manager.get_default_profile("openai") {
                        if let Ok(token) = profile.get_valid_token().await {
                            let _ = manager.save(); // persist refreshed token
                            return Ok(token);
                        }
                    }
                }
                // 2. Fall back to API key from config
                let config = self.config.read().await;
                config
                    .openai
                    .api_key
                    .clone()
                    .filter(|k| !k.trim().is_empty())
                    .context("No OpenAI authentication configured (neither OAuth nor API key)")
            }
            LlmProvider::Cerebras => {
                let config = self.config.read().await;
                config
                    .cerebras
                    .api_key
                    .clone()
                    .filter(|k| !k.trim().is_empty())
                    .context("No Cerebras API key configured (set in Settings > Models)")
            }
            LlmProvider::DeepSeek => {
                let config = self.config.read().await;
                config
                    .deepseek
                    .as_ref()
                    .and_then(|d| d.api_key.clone())
                    .filter(|k| !k.trim().is_empty())
                    .context("No DeepSeek API key configured (set in Settings > Models)")
            }
            LlmProvider::GitHubCopilot => {
                let config = self.config.read().await;
                config
                    .github_copilot
                    .as_ref()
                    .and_then(|c| c.token.clone())
                    .filter(|k| !k.trim().is_empty())
                    .context("No GitHub Copilot token configured (set in Settings > Models)")
            }
            LlmProvider::MiniMax => {
                let config = self.config.read().await;
                config
                    .minimax
                    .as_ref()
                    .and_then(|m| m.api_key.clone())
                    .filter(|k| !k.trim().is_empty())
                    .context("No MiniMax API key configured (set in Settings > Models)")
            }
            LlmProvider::Qwen => {
                let config = self.config.read().await;
                config
                    .qwen
                    .as_ref()
                    .and_then(|q| q.api_key.clone())
                    .filter(|k| !k.trim().is_empty())
                    .context("No Qwen/DashScope API key configured (set in Settings > Models)")
            }
            LlmProvider::Google => {
                let config = self.config.read().await;
                config
                    .google
                    .as_ref()
                    .and_then(|g| g.api_key.clone())
                    .filter(|k| !k.trim().is_empty())
                    .context("No Google/Gemini API key configured (set in Settings > Models)")
            }
            LlmProvider::Ollama | LlmProvider::LMStudio => {
                // Local providers require no API key
                Ok("local".to_string())
            }
        }
    }

    /// Get the Ollama API URL from config.
    async fn get_ollama_url(&self) -> String {
        let config = self.config.read().await;
        config.ollama.url.clone()
    }

    /// Get the LM Studio API URL from config.
    async fn get_lmstudio_url(&self) -> String {
        let config = self.config.read().await;
        config.lmstudio.url.clone()
    }

    /// Get the bridge endpoint path for a provider (non-streaming).
    fn api_endpoint(provider: LlmProvider) -> &'static str {
        match provider {
            LlmProvider::Anthropic => "/api/messages",
            LlmProvider::OpenAI => "/api/openai/messages",
            LlmProvider::Ollama => "", // Direct HTTP, not through bridge
            _ => "",
        }
    }

    /// Get the bridge endpoint path for a provider (streaming).
    fn stream_endpoint(provider: LlmProvider) -> &'static str {
        match provider {
            LlmProvider::Anthropic => "/api/messages/stream",
            LlmProvider::OpenAI => "/api/openai/messages/stream",
            LlmProvider::Ollama => "", // Direct HTTP, not through bridge
            _ => "",
        }
    }

    /// Send a message to a local OpenAI-compatible server (Ollama, LM Studio, etc).
    async fn send_message_local_openai_compat(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
        base_url: &str,
        prefix: &str,
        provider: LlmProvider,
    ) -> Result<ClaudeMessageResult> {
        let model_name = model.strip_prefix(prefix).unwrap_or(model);

        // Build OpenAI-format messages with proper tool_result/tool_use conversion
        let oai_messages =
            crate::tool_converter::convert_messages_to_openai(system_prompt, messages);

        let converted_tools = crate::tool_converter::convert_tools(tools, provider);

        let mut request_body = serde_json::json!({
            "model": model_name,
            "messages": oai_messages,
            "stream": false,
        });

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        let url = format!("{}/v1/chat/completions", base_url);
        info!(
            "[{}] Sending request to {} (model: {})",
            provider, url, model_name
        );

        let response = self
            .http_client
            .post(&url)
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context(format!("Failed to send request to {}", provider))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            warn!("[{}] Request failed: {} - {}", provider, status, error_text);
            anyhow::bail!("{} error ({}): {}", provider, status, error_text);
        }

        let resp: serde_json::Value = response
            .json()
            .await
            .context(format!("Failed to parse {} response", provider))?;

        // Extract from OpenAI format
        let choice = resp
            .get("choices")
            .and_then(|c| c.get(0))
            .ok_or_else(|| anyhow::anyhow!("No choices in {} response", provider))?;

        let message = choice
            .get("message")
            .ok_or_else(|| anyhow::anyhow!("No message in {} response", provider))?;

        let text = message
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let finish_reason = choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .unwrap_or("stop");

        // Convert OpenAI tool_calls to internal format
        let mut tool_uses = Vec::new();
        let mut raw_content = vec![serde_json::json!({
            "type": "text",
            "text": text,
        })];

        if let Some(tool_calls) = message.get("tool_calls").and_then(|tc| tc.as_array()) {
            for tc in tool_calls {
                if let Some(internal) = crate::tool_converter::openai_tool_call_to_internal(tc) {
                    tool_uses.push(ToolUseBlock {
                        id: internal["id"].as_str().unwrap_or("").to_string(),
                        name: internal["name"].as_str().unwrap_or("").to_string(),
                        input: internal["input"].clone(),
                    });
                    raw_content.push(internal);
                }
            }
        }

        let stop_reason = if !tool_uses.is_empty() {
            "tool_use".to_string()
        } else {
            match finish_reason {
                "stop" => "end_turn".to_string(),
                other => other.to_string(),
            }
        };

        info!(
            "[{}] Response: {} chars, {} tool_uses, stop_reason={}",
            provider,
            text.len(),
            tool_uses.len(),
            stop_reason
        );

        Ok(ClaudeMessageResult {
            text,
            tool_uses,
            stop_reason,
            raw_content,
            tool_calls_made: 0,
        })
    }

    /// Send a message to Cerebras via its OpenAI-compatible API.
    #[allow(dead_code)]
    async fn send_message_cerebras(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: usize,
    ) -> Result<ClaudeMessageResult> {
        let api_key = self.get_provider_api_key(LlmProvider::Cerebras).await?;

        // Build OpenAI-format messages with proper tool_result/tool_use conversion
        let oai_messages =
            crate::tool_converter::convert_messages_to_openai(system_prompt, messages);

        let converted_tools = crate::tool_converter::convert_tools(tools, LlmProvider::OpenAI);

        let cerebras_model = model.strip_prefix("cerebras/").unwrap_or(model);

        let mut request_body = serde_json::json!({
            "model": cerebras_model,
            "messages": oai_messages,
            "max_tokens": max_tokens,
            "stream": false,
        });

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        let url = "https://api.cerebras.ai/v1/chat/completions";
        info!(
            "[CEREBRAS] Sending request to {} (model: {})",
            url, cerebras_model
        );

        let response = self
            .http_client
            .post(url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to Cerebras")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            warn!("[CEREBRAS] Request failed: {} - {}", status, error_text);
            anyhow::bail!("Cerebras error ({}): {}", status, error_text);
        }

        let resp: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse Cerebras response")?;

        // Extract from OpenAI format
        let choice = resp
            .get("choices")
            .and_then(|c| c.get(0))
            .ok_or_else(|| anyhow::anyhow!("No choices in Cerebras response"))?;

        let message = choice
            .get("message")
            .ok_or_else(|| anyhow::anyhow!("No message in Cerebras response"))?;

        let content = message
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let finish_reason = choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .unwrap_or("stop");

        // gpt-oss-120b returns a `reasoning` field with chain-of-thought
        let reasoning = message
            .get("reasoning")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();

        // If content is empty but reasoning is present, use reasoning as the response
        let text = if content.is_empty() && !reasoning.is_empty() {
            reasoning.clone()
        } else {
            content
        };

        // Convert OpenAI tool_calls to internal format
        let mut tool_uses = Vec::new();
        let mut raw_content = Vec::new();
        if !reasoning.is_empty() {
            raw_content.push(serde_json::json!({
                "type": "thinking",
                "thinking": reasoning,
            }));
        }
        raw_content.push(serde_json::json!({
            "type": "text",
            "text": text,
        }));

        if let Some(tool_calls) = message.get("tool_calls").and_then(|tc| tc.as_array()) {
            for tc in tool_calls {
                if let Some(internal) = crate::tool_converter::openai_tool_call_to_internal(tc) {
                    tool_uses.push(ToolUseBlock {
                        id: internal["id"].as_str().unwrap_or("").to_string(),
                        name: internal["name"].as_str().unwrap_or("").to_string(),
                        input: internal["input"].clone(),
                    });
                    raw_content.push(internal);
                }
            }
        }

        let stop_reason = if !tool_uses.is_empty() {
            "tool_use".to_string()
        } else {
            match finish_reason {
                "stop" => "end_turn".to_string(),
                other => other.to_string(),
            }
        };

        info!(
            "[CEREBRAS] Response: {} chars, {} tool_uses, stop_reason={} (model: {})",
            text.len(),
            tool_uses.len(),
            stop_reason,
            cerebras_model
        );

        Ok(ClaudeMessageResult {
            text,
            tool_uses,
            stop_reason,
            raw_content,
            tool_calls_made: 0,
        })
    }

    /// Get the API URL and model prefix strip for cloud OpenAI-compatible providers.
    async fn get_cloud_openai_compat_config(&self, provider: LlmProvider) -> Result<(String, &'static str)> {
        let config = self.config.read().await;
        let ssrf_policy = crate::security::ssrf::SsrfPolicy::default();
        match provider {
            LlmProvider::DeepSeek => {
                let url = config
                    .deepseek
                    .as_ref()
                    .map(|d| d.api_url.clone())
                    .unwrap_or_else(|| "https://api.deepseek.com/v1".to_string());
                crate::security::ssrf::validate_outbound_request(&url, &ssrf_policy, &[])
                    .map_err(|e| anyhow::anyhow!("DeepSeek api_url SSRF check failed: {}", e))?;
                Ok((format!("{}/chat/completions", url), "deepseek/"))
            }
            LlmProvider::GitHubCopilot => {
                let url = config
                    .github_copilot
                    .as_ref()
                    .map(|c| c.api_url.clone())
                    .unwrap_or_else(|| "https://api.githubcopilot.com".to_string());
                crate::security::ssrf::validate_outbound_request(&url, &ssrf_policy, &[])
                    .map_err(|e| anyhow::anyhow!("GitHub Copilot api_url SSRF check failed: {}", e))?;
                Ok((format!("{}/chat/completions", url), "github-copilot/"))
            }
            LlmProvider::MiniMax => {
                let url = config
                    .minimax
                    .as_ref()
                    .map(|m| m.api_url.clone())
                    .unwrap_or_else(|| "https://api.minimax.chat/v1".to_string());
                crate::security::ssrf::validate_outbound_request(&url, &ssrf_policy, &[])
                    .map_err(|e| anyhow::anyhow!("MiniMax api_url SSRF check failed: {}", e))?;
                Ok((format!("{}/chat/completions", url), "minimax/"))
            }
            LlmProvider::Cerebras => {
                Ok(("https://api.cerebras.ai/v1/chat/completions".to_string(), "cerebras/"))
            }
            LlmProvider::Qwen => {
                let url = config
                    .qwen
                    .as_ref()
                    .map(|q| q.api_url.clone())
                    .unwrap_or_else(|| "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string());
                crate::security::ssrf::validate_outbound_request(&url, &ssrf_policy, &[])
                    .map_err(|e| anyhow::anyhow!("Qwen api_url SSRF check failed: {}", e))?;
                Ok((format!("{}/chat/completions", url), "qwen/"))
            }
            _ => anyhow::bail!("Provider {:?} is not a cloud OpenAI-compatible provider", provider),
        }
    }

    /// Send a message to any cloud OpenAI-compatible provider (Cerebras, DeepSeek, GitHubCopilot, MiniMax).
    async fn send_message_cloud_openai_compat(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: usize,
        provider: LlmProvider,
    ) -> Result<ClaudeMessageResult> {
        // Check if DeepSeek should be routed through the bridge
        if provider == LlmProvider::DeepSeek {
            let config = self.config.read().await;
            let use_bridge = config.deepseek.as_ref().map_or(false, |d| d.use_bridge);
            drop(config);

            if use_bridge {
                return self
                    .send_message_via_deepseek_bridge(model, system_prompt, messages, tools, max_tokens)
                    .await;
            }
        }

        let api_key = self.get_provider_api_key(provider).await?;
        let (url, prefix) = self.get_cloud_openai_compat_config(provider).await?;

        let oai_messages =
            crate::tool_converter::convert_messages_to_openai(system_prompt, messages);
        let converted_tools = crate::tool_converter::convert_tools(tools, LlmProvider::OpenAI);

        let stripped_model = model.strip_prefix(prefix).unwrap_or(model);

        let mut request_body = serde_json::json!({
            "model": stripped_model,
            "messages": oai_messages,
            "max_tokens": max_tokens,
            "stream": false,
        });

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        info!(
            "[{}] Sending request to {} (model: {})",
            provider, url, stripped_model
        );

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context(format!("Failed to send request to {}", provider))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            warn!("[{}] Request failed: {} - {}", provider, status, error_text);
            anyhow::bail!("{} error ({}): {}", provider, status, error_text);
        }

        let resp: serde_json::Value = response
            .json()
            .await
            .context(format!("Failed to parse {} response", provider))?;

        let choice = resp
            .get("choices")
            .and_then(|c| c.get(0))
            .context(format!("{} response missing choices", provider))?;

        let message = choice
            .get("message")
            .context(format!("{} response missing message", provider))?;

        let finish_reason = choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .unwrap_or("stop");

        let content = message
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let reasoning = message
            .get("reasoning_content")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();

        let text = if !reasoning.is_empty() && content.is_empty() {
            reasoning.clone()
        } else {
            content
        };

        let mut tool_uses = Vec::new();
        let mut raw_content = Vec::new();
        if !reasoning.is_empty() {
            raw_content.push(serde_json::json!({
                "type": "thinking",
                "thinking": reasoning,
            }));
        }
        raw_content.push(serde_json::json!({
            "type": "text",
            "text": text,
        }));

        if let Some(tool_calls) = message.get("tool_calls").and_then(|tc| tc.as_array()) {
            for tc in tool_calls {
                if let Some(internal) = crate::tool_converter::openai_tool_call_to_internal(tc) {
                    tool_uses.push(ToolUseBlock {
                        id: internal["id"].as_str().unwrap_or("").to_string(),
                        name: internal["name"].as_str().unwrap_or("").to_string(),
                        input: internal["input"].clone(),
                    });
                    raw_content.push(internal);
                }
            }
        }

        let stop_reason = if !tool_uses.is_empty() {
            "tool_use".to_string()
        } else {
            match finish_reason {
                "stop" => "end_turn".to_string(),
                other => other.to_string(),
            }
        };

        info!(
            "[{}] Response: {} chars, {} tool_uses, stop_reason={} (model: {})",
            provider,
            text.len(),
            tool_uses.len(),
            stop_reason,
            stripped_model
        );

        Ok(ClaudeMessageResult {
            text,
            tool_uses,
            stop_reason,
            raw_content,
            tool_calls_made: 0,
        })
    }

    /// Send a message to a cloud OpenAI-compatible provider with real SSE streaming.
    /// Same providers as `send_message_cloud_openai_compat` but with `"stream": true`
    /// and incremental text delivery via callback.
    async fn send_message_streaming_cloud_openai_compat(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: usize,
        provider: LlmProvider,
        callback: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<ClaudeMessageResult> {
        let api_key = self.get_provider_api_key(provider).await?;
        let (url, prefix) = self.get_cloud_openai_compat_config(provider).await?;

        let oai_messages =
            crate::tool_converter::convert_messages_to_openai(system_prompt, messages);
        let converted_tools = crate::tool_converter::convert_tools(tools, LlmProvider::OpenAI);

        let stripped_model = model.strip_prefix(prefix).unwrap_or(model);

        let mut request_body = serde_json::json!({
            "model": stripped_model,
            "messages": oai_messages,
            "max_tokens": max_tokens,
            "stream": true,
        });

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        info!(
            "[{}] Sending streaming request to {} (model: {})",
            provider, url, stripped_model
        );

        let mut response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context(format!("Failed to send streaming request to {}", provider))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            warn!("[{}] Streaming request failed: {} - {}", provider, status, error_text);
            anyhow::bail!("{} error ({}): {}", provider, status, error_text);
        }

        let mut full_text = String::new();
        let mut tool_uses: Vec<ToolUseBlock> = Vec::new();
        let mut stop_reason = "end_turn".to_string();
        let mut tool_call_accumulators: std::collections::HashMap<usize, (String, String, String)> =
            std::collections::HashMap::new();
        let mut reasoning = String::new();
        let mut line_buffer = String::new();
        const MAX_LINE_BUFFER_OPENAI: usize = 1024 * 1024; // 1 MB

        while let Some(chunk_result) = response.chunk().await.transpose() {
            let chunk_bytes = chunk_result.context(format!("Failed to read streaming chunk from {}", provider))?;
            let chunk_text = String::from_utf8_lossy(&chunk_bytes);
            line_buffer.push_str(&chunk_text);
            if line_buffer.len() > MAX_LINE_BUFFER_OPENAI {
                anyhow::bail!("{} streaming line buffer exceeded 1 MB — possible malformed response", provider);
            }

            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].trim_end_matches('\r').to_string();
                line_buffer = line_buffer[newline_pos + 1..].to_string();

                if !line.starts_with("data: ") {
                    continue;
                }
                let data = &line[6..];
                if data == "[DONE]" {
                    break;
                }

                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(choice) = parsed.get("choices").and_then(|c| c.get(0)) {
                        if let Some(delta) = choice.get("delta") {
                            // Text content
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                full_text.push_str(content);
                                callback(content.to_string());
                            }

                            // Reasoning content (DeepSeek, etc.)
                            if let Some(r) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
                                reasoning.push_str(r);
                            }

                            // Tool calls
                            if let Some(tcs) = delta.get("tool_calls").and_then(|tc| tc.as_array()) {
                                for tc in tcs {
                                    let idx =
                                        tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                    let entry =
                                        tool_call_accumulators.entry(idx).or_insert_with(|| {
                                            let id = tc
                                                .get("id")
                                                .and_then(|i| i.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let name = tc
                                                .get("function")
                                                .and_then(|f| f.get("name"))
                                                .and_then(|n| n.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            (id, name, String::new())
                                        });
                                    if let Some(args) = tc
                                        .get("function")
                                        .and_then(|f| f.get("arguments"))
                                        .and_then(|a| a.as_str())
                                    {
                                        entry.2.push_str(args);
                                    }
                                }
                            }
                        }

                        if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                            stop_reason = match fr {
                                "tool_calls" => "tool_use".to_string(),
                                "stop" => "end_turn".to_string(),
                                other => other.to_string(),
                            };
                        }
                    }
                }
            }
        }

        // If we got reasoning but no text, use reasoning as the text
        let text = if full_text.is_empty() && !reasoning.is_empty() {
            reasoning.clone()
        } else {
            full_text
        };

        // Build raw_content
        let mut raw_content = Vec::new();
        if !reasoning.is_empty() {
            raw_content.push(serde_json::json!({
                "type": "thinking",
                "thinking": reasoning,
            }));
        }
        raw_content.push(serde_json::json!({ "type": "text", "text": text }));

        for (_idx, (id, name, args_json)) in tool_call_accumulators {
            let input: serde_json::Value =
                serde_json::from_str(&args_json).unwrap_or(serde_json::json!({}));
            tool_uses.push(ToolUseBlock {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            });
            raw_content.push(serde_json::json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            }));
        }

        if !tool_uses.is_empty() {
            stop_reason = "tool_use".to_string();
        }

        info!(
            "[{}] Streaming complete: {} chars, {} tool_uses (model: {})",
            provider,
            text.len(),
            tool_uses.len(),
            stripped_model
        );

        Ok(ClaudeMessageResult {
            text,
            tool_uses,
            stop_reason,
            raw_content,
            tool_calls_made: 0,
        })
    }

    /// Send a message to Google Gemini using the GoogleGeminiClient.
    async fn send_message_google_gemini(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
    ) -> Result<ClaudeMessageResult> {
        let api_key = self.get_provider_api_key(LlmProvider::Google).await?;
        let config = self.config.read().await;
        let max_tokens = Self::effective_max_tokens(model, config.claude.max_tokens);
        let use_bridge = config.google.as_ref().map_or(false, |g| g.use_bridge);
        drop(config);

        // Strip "google/" prefix if present
        let model_id = model.strip_prefix("google/").unwrap_or(model);

        let client = if use_bridge {
            let bridge_url = std::env::var("ANTHROPIC_BRIDGE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:18790".to_string());
            crate::providers::google::GoogleGeminiClient::via_bridge(model_id, &api_key, &bridge_url, max_tokens)
        } else {
            crate::providers::google::GoogleGeminiClient::new(model_id, &api_key, max_tokens)
        };

        use crate::providers::LlmClient;
        let overrides = crate::session_overrides::SessionOverrides::default();
        let result = client
            .send_message_with_tools(messages, tools, system_prompt, &overrides)
            .await?;

        // Convert LlmMessageResult → ClaudeMessageResult
        Ok(ClaudeMessageResult {
            text: result.text,
            tool_uses: result
                .tool_uses
                .into_iter()
                .map(|tu| ToolUseBlock {
                    id: tu.id,
                    name: tu.name,
                    input: tu.input,
                })
                .collect(),
            stop_reason: result.stop_reason,
            raw_content: result.raw_content,
            tool_calls_made: 0,
        })
    }

    /// Send a message to Google Gemini with real SSE streaming.
    async fn send_message_streaming_google_gemini(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
        callback: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<ClaudeMessageResult> {
        let api_key = self.get_provider_api_key(LlmProvider::Google).await?;
        let config = self.config.read().await;
        let max_tokens = Self::effective_max_tokens(model, config.claude.max_tokens);
        let use_bridge = config.google.as_ref().map_or(false, |g| g.use_bridge);
        drop(config);

        let model_id = model.strip_prefix("google/").unwrap_or(model);

        let client = if use_bridge {
            let bridge_url = std::env::var("ANTHROPIC_BRIDGE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:18790".to_string());
            crate::providers::google::GoogleGeminiClient::via_bridge(model_id, &api_key, &bridge_url, max_tokens)
        } else {
            crate::providers::google::GoogleGeminiClient::new(model_id, &api_key, max_tokens)
        };

        use crate::providers::LlmClient;
        let overrides = crate::session_overrides::SessionOverrides::default();
        let result = client
            .send_message_streaming_with_tools(
                messages,
                tools,
                system_prompt,
                &overrides,
                Box::new(move |chunk| callback(chunk.to_string())),
            )
            .await?;

        Ok(ClaudeMessageResult {
            text: result.text,
            tool_uses: result
                .tool_uses
                .into_iter()
                .map(|tu| ToolUseBlock {
                    id: tu.id,
                    name: tu.name,
                    input: tu.input,
                })
                .collect(),
            stop_reason: result.stop_reason,
            raw_content: result.raw_content,
            tool_calls_made: 0,
        })
    }

    /// Send a DeepSeek request through the bridge for centralized logging.
    async fn send_message_via_deepseek_bridge(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: usize,
    ) -> Result<ClaudeMessageResult> {
        let api_key = self.get_provider_api_key(LlmProvider::DeepSeek).await?;
        let bridge_url = std::env::var("ANTHROPIC_BRIDGE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:18790".to_string());

        let stripped_model = model.strip_prefix("deepseek/").unwrap_or(model);

        let mut request_body = serde_json::json!({
            "apiKey": api_key,
            "model": stripped_model,
            "max_tokens": max_tokens,
            "system": system_prompt,
            "messages": messages,
        });

        // Forward tools in Anthropic format. The bridge DeepSeek plugin converts
        // them to OpenAI format internally.
        if !tools.is_empty() {
            request_body["tools"] = serde_json::json!(tools);
        }

        info!(
            "[DEEPSEEK] Sending request via bridge (model: {})",
            stripped_model
        );

        let mut deepseek_req = self
            .http_client
            .post(format!("{}/api/deepseek/messages", bridge_url))
            .header("content-type", "application/json");
        if let Some(secret) = crate::bridge::get_bridge_secret() {
            deepseek_req = deepseek_req.header("x-bridge-secret", secret);
        }
        let response = deepseek_req
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to bridge (DeepSeek)")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            anyhow::bail!("Bridge error (DeepSeek, HTTP {}): {}", status, error_text);
        }

        let resp: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse bridge DeepSeek response")?;

        let content = resp["content"].as_array().cloned().unwrap_or_default();
        let text = content
            .iter()
            .filter_map(|b| {
                if b["type"].as_str() == Some("text") {
                    b["text"].as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");

        let tool_uses: Vec<ToolUseBlock> = content
            .iter()
            .filter_map(|b| {
                if b["type"].as_str() == Some("tool_use") {
                    Some(ToolUseBlock {
                        id: b["id"].as_str().unwrap_or("").to_string(),
                        name: b["name"].as_str().unwrap_or("").to_string(),
                        input: b["input"].clone(),
                    })
                } else {
                    None
                }
            })
            .collect();

        let stop_reason = resp["stop_reason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_string();

        Ok(ClaudeMessageResult {
            text,
            tool_uses,
            stop_reason,
            raw_content: content,
            tool_calls_made: 0,
        })
    }

    /// Send a message to a local OpenAI-compatible server with SSE streaming.
    async fn send_message_streaming_local_openai_compat(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[serde_json::Value],
        callback: impl Fn(String) + Send + Sync + 'static,
        base_url: &str,
        prefix: &str,
        provider: LlmProvider,
    ) -> Result<ClaudeMessageResult> {
        let model_name = model.strip_prefix(prefix).unwrap_or(model);

        let mut oai_messages = vec![serde_json::json!({
            "role": "system",
            "content": system_prompt,
        })];
        for msg in messages {
            oai_messages.push(serde_json::json!({
                "role": msg.role,
                "content": msg.content,
            }));
        }

        let converted_tools = crate::tool_converter::convert_tools(tools, provider);

        let mut request_body = serde_json::json!({
            "model": model_name,
            "messages": oai_messages,
            "stream": true,
        });

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        let url = format!("{}/v1/chat/completions", base_url);
        info!(
            "[{}] Sending streaming request to {} (model: {})",
            provider, url, model_name
        );

        let mut response = self
            .http_client
            .post(&url)
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context(format!("Failed to send streaming request to {}", provider))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            anyhow::bail!("{} error ({}): {}", provider, status, error_text);
        }

        let mut full_text = String::new();
        let mut tool_uses: Vec<ToolUseBlock> = Vec::new();
        let mut stop_reason = "end_turn".to_string();
        let mut tool_call_accumulators: std::collections::HashMap<usize, (String, String, String)> =
            std::collections::HashMap::new();

        let mut line_buffer = String::new();
        const MAX_LINE_BUFFER_OLLAMA: usize = 1024 * 1024; // 1 MB

        while let Some(chunk_result) = response.chunk().await.transpose() {
            let chunk_bytes = chunk_result.context("Failed to read streaming chunk from Ollama")?;
            let chunk_text = String::from_utf8_lossy(&chunk_bytes);
            line_buffer.push_str(&chunk_text);
            if line_buffer.len() > MAX_LINE_BUFFER_OLLAMA {
                anyhow::bail!("Ollama streaming line buffer exceeded 1 MB — possible malformed response");
            }

            // Process complete lines from the buffer
            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].trim_end_matches('\r').to_string();
                line_buffer = line_buffer[newline_pos + 1..].to_string();

                if !line.starts_with("data: ") {
                    continue;
                }
                let data = &line[6..];
                if data == "[DONE]" {
                    break;
                }

                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(choice) = parsed.get("choices").and_then(|c| c.get(0)) {
                        if let Some(delta) = choice.get("delta") {
                            // Text content
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                full_text.push_str(content);
                                callback(content.to_string());
                            }

                            // Tool calls
                            if let Some(tcs) = delta.get("tool_calls").and_then(|tc| tc.as_array()) {
                                for tc in tcs {
                                    let idx =
                                        tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                    let entry =
                                        tool_call_accumulators.entry(idx).or_insert_with(|| {
                                            let id = tc
                                                .get("id")
                                                .and_then(|i| i.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let name = tc
                                                .get("function")
                                                .and_then(|f| f.get("name"))
                                                .and_then(|n| n.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            (id, name, String::new())
                                        });
                                    if let Some(args) = tc
                                        .get("function")
                                        .and_then(|f| f.get("arguments"))
                                        .and_then(|a| a.as_str())
                                    {
                                        entry.2.push_str(args);
                                    }
                                }
                            }
                        }

                        if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                            stop_reason = match fr {
                                "tool_calls" => "tool_use".to_string(),
                                "stop" => "end_turn".to_string(),
                                other => other.to_string(),
                            };
                        }
                    }
                }
            }
        }

        // Finalize tool calls
        let mut raw_content = vec![serde_json::json!({ "type": "text", "text": full_text })];
        for (_idx, (id, name, args_json)) in tool_call_accumulators {
            let input: serde_json::Value =
                serde_json::from_str(&args_json).unwrap_or(serde_json::json!({}));
            tool_uses.push(ToolUseBlock {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            });
            raw_content.push(serde_json::json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            }));
        }

        if !tool_uses.is_empty() {
            stop_reason = "tool_use".to_string();
        }

        info!(
            "[{}] Streaming complete: {} chars, {} tool_uses",
            provider,
            full_text.len(),
            tool_uses.len()
        );

        Ok(ClaudeMessageResult {
            text: full_text,
            tool_uses,
            stop_reason,
            raw_content,
            tool_calls_made: 0,
        })
    }

    /// Send a message with MCP tools, returning structured result for tool-use loop
    pub async fn send_message_with_tools(
        &self,
        message: &str,
        tools: &[serde_json::Value],
        overrides: &SessionOverrides,
    ) -> Result<ClaudeMessageResult> {
        let config = self.config.read().await;
        let system_prompt = Self::build_full_system_prompt(&config, None);
        // Classify query and apply routing. Session overrides still take priority
        // (handled inside effective_model_for_query). Store the chosen model so
        // continue_after_tools uses a consistent model for this tool-loop turn.
        let global_default = config.effective_default_model();
        let effective_model = overrides.effective_model_for_query(
            message,
            crate::query_classifier::QuerySource::Text,
            &config.routing,
            global_default,
        );
        {
            let mut crm = self.current_routing_model.lock().await;
            *crm = Some(effective_model.to_string());
        }
        let provider = overrides.effective_provider(effective_model);
        let caps = capabilities(provider);

        // Route Ollama, LM Studio, and Cerebras directly (no bridge)
        if provider == LlmProvider::Ollama || provider == LlmProvider::LMStudio {
            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "user".to_string(),
                content: message.to_string(),
            });
            Self::trim_history(&mut history);

            let (base_url, prefix) = if provider == LlmProvider::LMStudio {
                (self.get_lmstudio_url().await, "lmstudio/")
            } else {
                (self.get_ollama_url().await, "ollama/")
            };
            let result = self
                .send_message_local_openai_compat(effective_model, &system_prompt, &history, tools, &base_url, prefix, provider)
                .await?;

            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        if provider.is_cloud_openai_compat() {
            let cloud_max_tokens = if provider == LlmProvider::Cerebras {
                config.cerebras.max_tokens
            } else {
                Self::effective_max_tokens(effective_model, config.claude.max_tokens)
            };
            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "user".to_string(),
                content: message.to_string(),
            });
            Self::trim_history(&mut history);

            let result = match self
                .send_message_cloud_openai_compat(
                    effective_model,
                    &system_prompt,
                    &history,
                    tools,
                    cloud_max_tokens,
                    provider,
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    // Roll back the user message so history stays consistent.
                    history.pop();
                    return Err(e);
                }
            };

            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        // Route Google Gemini directly (uses its own API format)
        if provider == LlmProvider::Google {
            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "user".to_string(),
                content: message.to_string(),
            });
            Self::trim_history(&mut history);

            let result = match self
                .send_message_google_gemini(effective_model, &system_prompt, &history, tools)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    history.pop();
                    return Err(e);
                }
            };

            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        let api_key = match provider {
            LlmProvider::Anthropic => self.get_auth_token().await?,
            LlmProvider::OpenAI => self.get_provider_api_key(LlmProvider::OpenAI).await?,
            p if p.is_cloud_openai_compat() => {
                anyhow::bail!("Provider {:?} marked as cloud OpenAI-compatible but not handled in dispatch", p)
            }
            p @ (LlmProvider::Ollama | LlmProvider::LMStudio | LlmProvider::Google) => {
                anyhow::bail!("Provider {:?} should have been handled by direct dispatch above", p)
            }
            _ => anyhow::bail!(
                "Provider {:?} not supported via legacy ClaudeClient",
                provider
            ),
        };

        // Convert tools for the target provider
        let converted_tools = tool_converter::convert_tools(tools, provider);

        // Add user message to history
        let mut history = self.conversation_history.write().await;
        history.push(Message {
            role: "user".to_string(),
            content: message.to_string(),
        });
        Self::trim_history(&mut history);
        Self::sanitize_history(&mut history);

        // Prepare request
        let max_tokens = Self::effective_max_tokens(effective_model, config.claude.max_tokens);
        let mut request_body = json!({
            "apiKey": api_key,
            "model": effective_model,
            "max_tokens": max_tokens,
            "system": system_prompt,
            "messages": *history,
        });

        // Add extended thinking if enabled and supported
        if let Some(budget) = overrides.thinking_budget {
            if caps.supports_thinking {
                request_body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": budget,
                });
            }
        }

        // Add tools if any
        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        // Add beta header for Computer Use tools (Anthropic only)
        if caps.supports_computer_use && Self::has_computer_use_tools(tools) {
            request_body["betas"] = json!(["computer-use-2025-01-24"]);
            info!("[BRIDGE] Computer Use beta enabled");
        }

        let endpoint = Self::api_endpoint(provider);
        info!(
            "[BRIDGE] Sending request to {} (provider: {}, tools: {})",
            endpoint,
            provider,
            tools.len()
        );

        let mut tool_req = self
            .http_client
            .post(format!("{}{}", self.bridge_url, endpoint))
            .header("content-type", "application/json");
        if let Some(secret) = crate::bridge::get_bridge_secret() {
            tool_req = tool_req.header("x-bridge-secret", secret);
        }
        let response = match tool_req
            .json(&request_body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("[BRIDGE] Request failed, rolling back user message from history");
                history.pop();
                return Err(e).context("Failed to send request to Bridge");
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            warn!("[BRIDGE] Request failed: {} - {}", status, error_text);
            history.pop();
            anyhow::bail!("Bridge error: {}", error_text);
        }

        let claude_response: ClaudeResponse = match response.json().await {
            Ok(r) => r,
            Err(e) => {
                warn!("[BRIDGE] Response parse failed, rolling back user message from history");
                history.pop();
                return Err(e).context("Failed to parse response");
            }
        };

        let text = Self::extract_text(&claude_response.content);
        let tool_uses = Self::extract_tool_uses(&claude_response.content);
        let stop_reason = claude_response
            .stop_reason
            .unwrap_or_else(|| "end_turn".to_string());

        // Add assistant response to history (raw content blocks)
        history.push(Message {
            role: "assistant".to_string(),
            content: serde_json::to_string(&claude_response.content).unwrap_or_default(),
        });
        Self::trim_history(&mut history);

        info!(
            "[BRIDGE] Response: {} chars text, {} tool_uses, stop_reason={}",
            text.len(),
            tool_uses.len(),
            stop_reason
        );

        Ok(ClaudeMessageResult {
            text,
            tool_calls_made: tool_uses.len(),
            tool_uses,
            stop_reason,
            raw_content: claude_response.content,
        })
    }

    /// Add a tool result to conversation history.
    /// If the last message is already a user-role tool_result message, appends to it
    /// (Anthropic API requires all tool_results in a single user message, not consecutive ones).
    pub async fn add_tool_result(&self, tool_use_id: &str, content: &str) {
        let new_result = json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
        });
        let mut history = self.conversation_history.write().await;

        // Try to append to existing user tool_result message
        if let Some(last) = history.last_mut() {
            if last.role == "user" {
                if let Ok(mut arr) = serde_json::from_str::<Vec<serde_json::Value>>(&last.content)
                {
                    if arr
                        .first()
                        .and_then(|v| v.get("type"))
                        .and_then(|t| t.as_str())
                        == Some("tool_result")
                    {
                        arr.push(new_result);
                        last.content = serde_json::to_string(&arr).unwrap_or_default();
                        // Do NOT trim here — we are mid-batch; trimming could orphan
                        // the preceding assistant message's remaining tool_uses.
                        return;
                    }
                }
            }
        }

        // No existing tool_result message to append to — create a new one.
        // Still defer trimming — the caller (tool_loop) will call
        // trim_history_if_needed() after the entire batch is complete.
        history.push(Message {
            role: "user".to_string(),
            content: serde_json::to_string(&json!([new_result])).unwrap_or_default(),
        });
    }

    /// Trim and sanitize history if it exceeds the configured limit.
    /// Should be called after a complete batch of tool results has been added,
    /// not between individual tool results within the same batch.
    pub async fn trim_history_if_needed(&self) {
        let mut history = self.conversation_history.write().await;
        Self::trim_history(&mut history);
    }

    /// Continue a conversation after tool results (for tool-use loop)
    pub async fn continue_after_tools(
        &self,
        tools: &[serde_json::Value],
        overrides: &SessionOverrides,
    ) -> Result<ClaudeMessageResult> {
        let config = self.config.read().await;
        let system_prompt = Self::build_system_prompt(&config.claude.system_prompt, None);
        // Use the model chosen by the classifier at the start of this turn so
        // all iterations within the same tool-loop use a consistent model.
        // Falls back to the standard session/config resolution if not set.
        let global_default = config.effective_default_model();
        let turn_model: Option<String> = {
            let crm = self.current_routing_model.lock().await;
            crm.clone()
        };
        let effective_model = match turn_model.as_deref() {
            Some(m) if !m.is_empty() => m,
            _ => overrides.effective_model(global_default),
        };
        let provider = overrides.effective_provider(effective_model);
        let caps = capabilities(provider);

        // Route local providers directly
        if provider == LlmProvider::Ollama || provider == LlmProvider::LMStudio {
            let (base_url, prefix) = if provider == LlmProvider::LMStudio {
                (self.get_lmstudio_url().await, "lmstudio/")
            } else {
                (self.get_ollama_url().await, "ollama/")
            };
            let history = self.conversation_history.read().await;
            let result = self
                .send_message_local_openai_compat(effective_model, &system_prompt, &history, tools, &base_url, prefix, provider)
                .await?;
            drop(history);

            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        // Route cloud OpenAI-compatible providers directly
        if provider.is_cloud_openai_compat() {
            let cloud_max_tokens = if provider == LlmProvider::Cerebras {
                config.cerebras.max_tokens
            } else {
                Self::effective_max_tokens(effective_model, config.claude.max_tokens)
            };
            let history = self.conversation_history.read().await;
            let result = self
                .send_message_cloud_openai_compat(
                    effective_model,
                    &system_prompt,
                    &history,
                    tools,
                    cloud_max_tokens,
                    provider,
                )
                .await?;
            drop(history);

            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        // Route Google Gemini directly
        if provider == LlmProvider::Google {
            let history = self.conversation_history.read().await;
            let result = self
                .send_message_google_gemini(effective_model, &system_prompt, &history, tools)
                .await?;
            drop(history);

            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        let api_key = match provider {
            LlmProvider::Anthropic => self.get_auth_token().await?,
            LlmProvider::OpenAI => self.get_provider_api_key(LlmProvider::OpenAI).await?,
            p if p.is_cloud_openai_compat() => {
                anyhow::bail!("Provider {:?} marked as cloud OpenAI-compatible but not handled in dispatch", p)
            }
            p @ (LlmProvider::Ollama | LlmProvider::LMStudio | LlmProvider::Google) => {
                anyhow::bail!("Provider {:?} should have been handled by direct dispatch above", p)
            }
            _ => anyhow::bail!(
                "Provider {:?} not supported via legacy ClaudeClient",
                provider
            ),
        };

        let converted_tools = tool_converter::convert_tools(tools, provider);

        let history = self.conversation_history.read().await;

        let max_tokens = Self::effective_max_tokens(effective_model, config.claude.max_tokens);
        let mut request_body = json!({
            "apiKey": api_key,
            "model": effective_model,
            "max_tokens": max_tokens,
            "system": system_prompt,
            "messages": *history,
        });

        // Add extended thinking if enabled and supported
        if let Some(budget) = overrides.thinking_budget {
            if caps.supports_thinking {
                request_body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": budget,
                });
            }
        }

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        // Add beta header for Computer Use tools (Anthropic only)
        if caps.supports_computer_use && Self::has_computer_use_tools(tools) {
            request_body["betas"] = json!(["computer-use-2025-01-24"]);
        }

        drop(history);

        let endpoint = Self::api_endpoint(provider);
        info!(
            "[BRIDGE] Continuing after tool results (provider: {})",
            provider
        );

        let mut cont_req = self
            .http_client
            .post(format!("{}{}", self.bridge_url, endpoint))
            .header("content-type", "application/json");
        if let Some(secret) = crate::bridge::get_bridge_secret() {
            cont_req = cont_req.header("x-bridge-secret", secret);
        }
        let response = cont_req
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to Bridge")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Bridge error: {}", error_text);
        }

        let claude_response: ClaudeResponse =
            response.json().await.context("Failed to parse response")?;

        let text = Self::extract_text(&claude_response.content);
        let tool_uses = Self::extract_tool_uses(&claude_response.content);
        let stop_reason = claude_response
            .stop_reason
            .unwrap_or_else(|| "end_turn".to_string());

        // Add to history
        let mut history = self.conversation_history.write().await;
        history.push(Message {
            role: "assistant".to_string(),
            content: serde_json::to_string(&claude_response.content).unwrap_or_default(),
        });
        Self::trim_history(&mut history);

        Ok(ClaudeMessageResult {
            text,
            tool_uses,
            stop_reason,
            raw_content: claude_response.content,
            tool_calls_made: 0,
        })
    }

    /// Continue a conversation after tool results with streaming.
    /// Same as `continue_after_tools()` but streams text via callback for progressive display.
    pub async fn continue_after_tools_streaming(
        &self,
        tools: &[serde_json::Value],
        overrides: &SessionOverrides,
        channel: Option<&crate::channel::ChannelSource>,
        callback: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<ClaudeMessageResult> {
        let config = self.config.read().await;
        let system_prompt = Self::build_system_prompt(
            &config.claude.system_prompt,
            channel,
        );
        // Use the model locked in at the start of this tool-loop turn.
        let global_default = config.effective_default_model();
        let turn_model: Option<String> = {
            let crm = self.current_routing_model.lock().await;
            crm.clone()
        };
        let effective_model = match turn_model.as_deref() {
            Some(m) if !m.is_empty() => m,
            _ => overrides.effective_model(global_default),
        };
        let provider = overrides.effective_provider(effective_model);
        let caps = capabilities(provider);

        // Route local providers with real SSE streaming
        if provider == LlmProvider::Ollama || provider == LlmProvider::LMStudio {
            let (base_url, prefix) = if provider == LlmProvider::LMStudio {
                (self.get_lmstudio_url().await, "lmstudio/")
            } else {
                (self.get_ollama_url().await, "ollama/")
            };
            let history = self.conversation_history.read().await;
            let result = self
                .send_message_streaming_local_openai_compat(
                    effective_model,
                    &system_prompt,
                    &history,
                    tools,
                    callback,
                    &base_url,
                    prefix,
                    provider,
                )
                .await?;
            drop(history);

            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        // Route cloud OpenAI-compat providers with real SSE streaming
        if provider.is_cloud_openai_compat() {
            let cloud_max_tokens = if provider == LlmProvider::Cerebras {
                config.cerebras.max_tokens
            } else {
                Self::effective_max_tokens(effective_model, config.claude.max_tokens)
            };
            let history = self.conversation_history.read().await;
            let result = self
                .send_message_streaming_cloud_openai_compat(
                    effective_model,
                    &system_prompt,
                    &history,
                    tools,
                    cloud_max_tokens,
                    provider,
                    callback,
                )
                .await?;
            drop(history);

            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        // Route Google Gemini with real SSE streaming
        if provider == LlmProvider::Google {
            let history = self.conversation_history.read().await;
            let result = self
                .send_message_streaming_google_gemini(
                    effective_model,
                    &system_prompt,
                    &history,
                    tools,
                    callback,
                )
                .await?;
            drop(history);

            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        let api_key = match provider {
            LlmProvider::Anthropic => self.get_auth_token().await?,
            LlmProvider::OpenAI => self.get_provider_api_key(LlmProvider::OpenAI).await?,
            p if p.is_cloud_openai_compat() => {
                anyhow::bail!("Provider {:?} marked as cloud OpenAI-compatible but not handled in dispatch", p)
            }
            p @ (LlmProvider::Ollama | LlmProvider::LMStudio | LlmProvider::Google) => {
                anyhow::bail!("Provider {:?} should have been handled by direct dispatch above", p)
            }
            _ => anyhow::bail!(
                "Provider {:?} not supported via legacy ClaudeClient",
                provider
            ),
        };

        let converted_tools = tool_converter::convert_tools(tools, provider);
        let history = self.conversation_history.read().await;

        let max_tokens = Self::effective_max_tokens(effective_model, config.claude.max_tokens);
        let mut request_body = json!({
            "apiKey": api_key,
            "model": effective_model,
            "max_tokens": max_tokens,
            "system": system_prompt,
            "messages": *history,
        });

        if let Some(budget) = overrides.thinking_budget {
            if caps.supports_thinking {
                request_body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": budget,
                });
            }
        }

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        if caps.supports_computer_use && Self::has_computer_use_tools(tools) {
            request_body["betas"] = json!(["computer-use-2025-01-24"]);
        }

        drop(history);

        let endpoint = Self::stream_endpoint(provider);
        info!(
            "[BRIDGE] Continue after tools (streaming, provider: {})",
            provider
        );

        let mut stream_cont_req = self
            .http_client
            .post(format!("{}{}", self.bridge_url, endpoint))
            .header("content-type", "application/json");
        if let Some(secret) = crate::bridge::get_bridge_secret() {
            stream_cont_req = stream_cont_req.header("x-bridge-secret", secret);
        }
        let mut response = stream_cont_req
            .json(&request_body)
            .send()
            .await
            .context("Failed to send streaming request to Bridge")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Bridge error: {}", error_text);
        }

        // Process SSE stream in real-time (true streaming, not buffered)
        let mut full_text = String::new();
        let mut raw_content: Vec<serde_json::Value> = Vec::new();
        let mut tool_uses: Vec<ToolUseBlock> = Vec::new();
        let mut stop_reason = "end_turn".to_string();
        let mut current_tool_use: Option<(String, String, String)> = None;
        let mut line_buffer = String::new();
        const MAX_LINE_BUFFER_BRIDGE: usize = 1024 * 1024; // 1 MB

        while let Some(chunk_result) = response.chunk().await.transpose() {
            let chunk_bytes = chunk_result.context("Failed to read streaming chunk")?;
            let chunk_text = String::from_utf8_lossy(&chunk_bytes);
            line_buffer.push_str(&chunk_text);
            if line_buffer.len() > MAX_LINE_BUFFER_BRIDGE {
                anyhow::bail!("Bridge streaming line buffer exceeded 1 MB — possible malformed response");
            }

            // Process complete lines from the buffer
            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].trim_end_matches('\r').to_string();
                line_buffer = line_buffer[newline_pos + 1..].to_string();

                if !line.starts_with("data: ") {
                    continue;
                }
                let data = &line[6..];
                if data == "[DONE]" {
                    break;
                }

                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    let event_type = parsed["type"].as_str().unwrap_or("");
                    match event_type {
                        "error" => {
                            let error_msg = parsed["error"]["message"]
                                .as_str()
                                .unwrap_or("Unknown error");
                            anyhow::bail!("Claude API error: {}", error_msg);
                        }
                        "content_block_start" => {
                            let block = &parsed["content_block"];
                            if block["type"].as_str() == Some("tool_use") {
                                let id = block["id"].as_str().unwrap_or("").to_string();
                                let name = block["name"].as_str().unwrap_or("").to_string();
                                current_tool_use = Some((id, name, String::new()));
                            }
                        }
                        "content_block_delta" => {
                            if let Some(delta_text) = parsed["delta"]["text"].as_str() {
                                full_text.push_str(delta_text);
                                callback(delta_text.to_string());
                            }
                            if let Some(partial_json) = parsed["delta"]["partial_json"].as_str() {
                                if let Some(ref mut tu) = current_tool_use {
                                    tu.2.push_str(partial_json);
                                }
                            }
                        }
                        "content_block_stop" => {
                            if let Some((id, name, input_json)) = current_tool_use.take() {
                                let input: serde_json::Value =
                                    serde_json::from_str(&input_json).unwrap_or(json!({}));
                                raw_content.push(json!({
                                    "type": "tool_use", "id": id, "name": name, "input": input,
                                }));
                                tool_uses.push(ToolUseBlock { id, name, input });
                            }
                        }
                        "message_delta" => {
                            if let Some(sr) = parsed["delta"]["stop_reason"].as_str() {
                                stop_reason = sr.to_string();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Build raw_content with text block
        if !full_text.is_empty() {
            let mut final_content = vec![json!({ "type": "text", "text": full_text })];
            final_content.extend(raw_content.clone());
            raw_content = final_content;
        }

        // Add assistant response to history
        let mut history = self.conversation_history.write().await;
        history.push(Message {
            role: "assistant".to_string(),
            content: serde_json::to_string(&raw_content).unwrap_or_default(),
        });
        Self::trim_history(&mut history);

        info!(
            "[BRIDGE] Continue streaming complete: {} chars text, {} tool_uses, stop_reason={}",
            full_text.len(),
            tool_uses.len(),
            stop_reason
        );

        Ok(ClaudeMessageResult {
            text: full_text,
            tool_calls_made: tool_uses.len(),
            tool_uses,
            stop_reason,
            raw_content,
        })
    }

    /// Trim conversation history to stay within bounds.
    /// Keeps the most recent messages, preserving the first message if it's a system-level context.
    fn trim_history(history: &mut Vec<Message>) {
        let max = MAX_HISTORY_MESSAGES.load(Ordering::Relaxed);
        if history.len() > max {
            let excess = history.len() - max;
            // Keep the first message (may be important context) and trim from the front
            if history.len() > 1 {
                let first = history[0].clone();
                history.drain(1..=excess);
                // Ensure first message is preserved
                if history.first().map(|m| &m.content) != Some(&first.content) {
                    history.insert(0, first);
                }
            }
            debug!(
                "[CLAUDE] History trimmed: removed {} old messages, {} remaining",
                excess,
                history.len()
            );
            // Sanitize only when trimming actually occurred — trimming can orphan
            // tool_use/tool_result blocks by removing one half of a pair.
            Self::sanitize_history(history);
        }
    }

    /// Sanitize conversation history by removing orphaned tool_use and tool_result blocks.
    ///
    /// The Claude API requires that every tool_result references a tool_use_id from the
    /// immediately preceding assistant message. When history is trimmed, compacted, or
    /// restored from persistence, this invariant can be violated.
    ///
    /// This function:
    /// 1. Scans for all tool_use IDs in assistant messages
    /// 2. Scans for all tool_use_ids referenced by tool_result blocks in user messages
    /// 3. Strips orphaned tool_result blocks (no matching tool_use)
    /// 4. Strips orphaned tool_use blocks (no matching tool_result in the next message)
    /// 5. Removes empty messages left after stripping
    fn sanitize_history(history: &mut Vec<Message>) {
        use std::collections::HashSet;

        // Phase 1: Structural fixes — ensure valid message ordering regardless of tool blocks.
        // Ensure history doesn't start with an assistant message (API requirement)
        while history.first().map(|m| m.role.as_str()) == Some("assistant") {
            history.remove(0);
        }

        // Phase 2: Tool block orphan detection and removal.
        // Collect all tool_use IDs from assistant messages
        let mut tool_use_ids: HashSet<String> = HashSet::new();
        // Collect all tool_result references from user messages
        let mut tool_result_refs: HashSet<String> = HashSet::new();

        for msg in history.iter() {
            if let Ok(blocks) = serde_json::from_str::<Vec<serde_json::Value>>(&msg.content) {
                for block in &blocks {
                    if let Some(btype) = block.get("type").and_then(|t| t.as_str()) {
                        if btype == "tool_use" {
                            if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                                tool_use_ids.insert(id.to_string());
                            }
                        } else if btype == "tool_result" {
                            if let Some(id) = block.get("tool_use_id").and_then(|i| i.as_str()) {
                                tool_result_refs.insert(id.to_string());
                            }
                        }
                    }
                }
            }
        }

        // Always protect the last assistant message's tool_use IDs from orphan detection.
        //
        // This handles two distinct cases:
        // 1. History ends with assistant (pre-execution): tool_uses are in-flight and no
        //    tool_result has been added yet — they must not be stripped.
        // 2. History ends with user([tool_result:...]) (mid-batch execution): the preceding
        //    assistant message issued N tool_uses; only the first K results have been batched
        //    so far. The remaining (N-K) tool_uses are still in-flight. Without this guard,
        //    trim_history firing between batch members causes sanitize_history to incorrectly
        //    mark those tool_uses as "orphaned" and strip them, which then produces an
        //    "unexpected tool_use_id" API 400 error when continue_after_tools is called.
        let mut last_assistant_tool_use_ids: HashSet<String> = HashSet::new();
        for msg in history.iter().rev() {
            if msg.role == "assistant" {
                if let Ok(blocks) = serde_json::from_str::<Vec<serde_json::Value>>(&msg.content)
                {
                    for block in &blocks {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                                last_assistant_tool_use_ids.insert(id.to_string());
                            }
                        }
                    }
                }
                break;
            }
        }

        // Find orphaned tool_result IDs (reference a tool_use that doesn't exist)
        let orphaned_results: HashSet<&String> =
            tool_result_refs.difference(&tool_use_ids).collect();
        // Find orphaned tool_use IDs (no tool_result references them),
        // excluding in-flight tool_uses only when history ends with the assistant message.
        let orphaned_uses: HashSet<&String> = tool_use_ids
            .difference(&tool_result_refs)
            .filter(|id| !last_assistant_tool_use_ids.contains(*id))
            .collect();

        let result_count = orphaned_results.len();
        let use_count = orphaned_uses.len();

        if !orphaned_results.is_empty() || !orphaned_uses.is_empty() {
            // Strip orphaned blocks from messages
            let mut i = 0;
            while i < history.len() {
                let msg = &history[i];
                if let Ok(mut blocks) = serde_json::from_str::<Vec<serde_json::Value>>(&msg.content)
                {
                    let original_len = blocks.len();
                    blocks.retain(|block| {
                        if let Some(btype) = block.get("type").and_then(|t| t.as_str()) {
                            if btype == "tool_result" {
                                if let Some(id) = block.get("tool_use_id").and_then(|i| i.as_str())
                                {
                                    return !orphaned_results.contains(&id.to_string());
                                }
                            } else if btype == "tool_use" {
                                if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                                    return !orphaned_uses.contains(&id.to_string());
                                }
                            }
                        }
                        true
                    });

                    if blocks.len() != original_len {
                        if blocks.is_empty() {
                            history.remove(i);
                            continue;
                        } else {
                            history[i].content = serde_json::to_string(&blocks).unwrap_or_default();
                        }
                    }
                }
                i += 1;
            }

            warn!(
                "[CLAUDE] Sanitized history: removed {} orphaned tool_results, {} orphaned tool_uses, {} messages remaining",
                result_count, use_count, history.len()
            );
        }

        // Phase 3: Post-cleanup structural fixes.
        // Ensure history doesn't start with assistant after block removal
        while history.first().map(|m| m.role.as_str()) == Some("assistant") {
            history.remove(0);
        }
    }

    // ── Tool-pairing error detection helpers ─────────────────────────────────

    /// Returns true if the error string matches the Anthropic API 400
    /// "unexpected tool_use_id" pattern that signals a tool_result/tool_use
    /// pairing mismatch in the message history.
    pub fn is_tool_pairing_error(error: &str) -> bool {
        error.contains("unexpected tool_use_id")
            || error.contains("tool_result block must have a corresponding tool_use block")
            || error.contains("tool_use ids were found without tool_result blocks")
            || error.contains("tool_use_id")
                && (error.contains("tool_result") || error.contains("immediately after"))
    }

    /// Return cumulative (errors_detected, recoveries_succeeded) metrics.
    #[allow(dead_code)]
    pub fn tool_pairing_stats() -> (usize, usize) {
        (
            TOOL_PAIRING_ERRORS.load(Ordering::Relaxed),
            TOOL_PAIRING_RECOVERIES.load(Ordering::Relaxed),
        )
    }

    // ── Recovery strategies ───────────────────────────────────────────────────

    /// Strategy 1: Re-run sanitize_history on the live history.
    /// Cheapest recovery — corrects any orphaned blocks that slipped through.
    pub async fn repair_sanitize(&self) {
        let mut history = self.conversation_history.write().await;
        Self::sanitize_history(&mut history);
        info!("[CLAUDE] Recovery S1: sanitize applied, {} messages", history.len());
    }

    /// Strategy 2: Compact history — keep the first message and the most recent
    /// `keep_last` messages, dropping everything in between.
    pub async fn repair_compact(&self, keep_last: usize) {
        let mut history = self.conversation_history.write().await;
        if history.len() <= keep_last + 1 {
            Self::sanitize_history(&mut history);
            return;
        }
        let drain_end = history.len() - keep_last;
        // Preserve index 0 (first/system message), drain the middle
        if drain_end > 1 {
            history.drain(1..drain_end);
        }
        Self::sanitize_history(&mut history);
        info!("[CLAUDE] Recovery S2: compacted to {} messages", history.len());
    }

    /// Strategy 3: Strip all tool blocks (tool_use / tool_result) from every
    /// message, leaving only plain text content. Keeps the conversation readable
    /// but removes all tool context — a clean slate for the tool loop.
    pub async fn repair_strip_tool_blocks(&self) {
        let mut history = self.conversation_history.write().await;
        let mut i = 0;
        while i < history.len() {
            let msg = &history[i];
            if let Ok(blocks) = serde_json::from_str::<Vec<serde_json::Value>>(&msg.content) {
                let text_blocks: Vec<serde_json::Value> = blocks
                    .into_iter()
                    .filter(|b| {
                        !matches!(
                            b.get("type").and_then(|t| t.as_str()),
                            Some("tool_use") | Some("tool_result")
                        )
                    })
                    .collect();
                if text_blocks.is_empty() {
                    history.remove(i);
                    continue;
                }
                // If only one text block, unwrap to plain string
                let new_content = if text_blocks.len() == 1 {
                    if let Some(text) = text_blocks[0].get("text").and_then(|t| t.as_str()) {
                        text.to_string()
                    } else {
                        serde_json::to_string(&text_blocks).unwrap_or_default()
                    }
                } else {
                    serde_json::to_string(&text_blocks).unwrap_or_default()
                };
                history[i].content = new_content;
            }
            i += 1;
        }
        // Ensure structural validity after stripping
        while history.first().map(|m| m.role.as_str()) == Some("assistant") {
            history.remove(0);
        }
        info!("[CLAUDE] Recovery S3: stripped tool blocks, {} messages", history.len());
    }

    /// Strategy 4: Clear all history except the very first message.
    /// Last-resort reset — loses all context but guarantees a clean state.
    pub async fn repair_clear(&self) {
        let mut history = self.conversation_history.write().await;
        history.truncate(1);
        info!("[CLAUDE] Recovery S4: history cleared to {} messages", history.len());
    }

    /// Send a message with tools, streaming the initial response via callback.
    /// Returns a ClaudeMessageResult. If stop_reason is "tool_use", the caller
    /// should handle tool execution and call continue_after_tools().
    pub async fn send_message_streaming_with_tools(
        &self,
        message: &str,
        tools: &[serde_json::Value],
        overrides: &SessionOverrides,
        callback: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<ClaudeMessageResult> {
        self.send_message_streaming_with_tools_channel(message, tools, overrides, None, callback)
            .await
    }

    /// Send a message with tools and optional channel source, streaming the response via callback.
    pub async fn send_message_streaming_with_tools_channel(
        &self,
        message: &str,
        tools: &[serde_json::Value],
        overrides: &SessionOverrides,
        channel: Option<&crate::channel::ChannelSource>,
        callback: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<ClaudeMessageResult> {
        let config = self.config.read().await;
        let system_prompt = Self::build_full_system_prompt(&config, channel);
        // Classify query and apply routing (Voice bias when channel is Voice).
        let global_default = config.effective_default_model();
        let query_source = match channel {
            Some(crate::channel::ChannelSource::Voice) => {
                crate::query_classifier::QuerySource::Voice
            }
            _ => crate::query_classifier::QuerySource::Text,
        };
        let effective_model = overrides.effective_model_for_query(
            message,
            query_source,
            &config.routing,
            global_default,
        );
        {
            let mut crm = self.current_routing_model.lock().await;
            *crm = Some(effective_model.to_string());
        }
        let provider = overrides.effective_provider(effective_model);
        let caps = capabilities(provider);

        // Route local providers directly (streaming)
        if provider == LlmProvider::Ollama || provider == LlmProvider::LMStudio {
            let (base_url, prefix) = if provider == LlmProvider::LMStudio {
                (self.get_lmstudio_url().await, "lmstudio/")
            } else {
                (self.get_ollama_url().await, "ollama/")
            };
            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "user".to_string(),
                content: message.to_string(),
            });
            Self::trim_history(&mut history);

            let result = self
                .send_message_streaming_local_openai_compat(
                    effective_model,
                    &system_prompt,
                    &history,
                    tools,
                    callback,
                    &base_url,
                    prefix,
                    provider,
                )
                .await?;

            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        // Route cloud OpenAI-compat providers with real SSE streaming
        if provider.is_cloud_openai_compat() {
            let cloud_max_tokens = if provider == LlmProvider::Cerebras {
                config.cerebras.max_tokens
            } else {
                Self::effective_max_tokens(effective_model, config.claude.max_tokens)
            };
            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "user".to_string(),
                content: message.to_string(),
            });
            Self::trim_history(&mut history);

            let result = match self
                .send_message_streaming_cloud_openai_compat(
                    effective_model,
                    &system_prompt,
                    &history,
                    tools,
                    cloud_max_tokens,
                    provider,
                    callback,
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    history.pop();
                    return Err(e);
                }
            };

            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        // Route Google Gemini with real SSE streaming
        if provider == LlmProvider::Google {
            let mut history = self.conversation_history.write().await;
            history.push(Message {
                role: "user".to_string(),
                content: message.to_string(),
            });
            Self::trim_history(&mut history);

            let result = match self
                .send_message_streaming_google_gemini(
                    effective_model,
                    &system_prompt,
                    &history,
                    tools,
                    callback,
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    history.pop();
                    return Err(e);
                }
            };

            history.push(Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&result.raw_content).unwrap_or_default(),
            });
            Self::trim_history(&mut history);

            return Ok(result);
        }

        let api_key = match provider {
            LlmProvider::Anthropic => self.get_auth_token().await?,
            LlmProvider::OpenAI => self.get_provider_api_key(LlmProvider::OpenAI).await?,
            p if p.is_cloud_openai_compat() => {
                anyhow::bail!("Provider {:?} marked as cloud OpenAI-compatible but not handled in dispatch", p)
            }
            p @ (LlmProvider::Ollama | LlmProvider::LMStudio | LlmProvider::Google) => {
                anyhow::bail!("Provider {:?} should have been handled by direct dispatch above", p)
            }
            _ => anyhow::bail!(
                "Provider {:?} not supported via legacy ClaudeClient",
                provider
            ),
        };

        let converted_tools = tool_converter::convert_tools(tools, provider);

        // Add user message to history
        let mut history = self.conversation_history.write().await;
        history.push(Message {
            role: "user".to_string(),
            content: message.to_string(),
        });
        Self::trim_history(&mut history);
        Self::sanitize_history(&mut history);

        // Prepare request
        let max_tokens = Self::effective_max_tokens(effective_model, config.claude.max_tokens);
        let mut request_body = json!({
            "apiKey": api_key,
            "model": effective_model,
            "max_tokens": max_tokens,
            "system": system_prompt,
            "messages": *history,
        });

        // Add extended thinking if enabled and supported
        if let Some(budget) = overrides.thinking_budget {
            if caps.supports_thinking {
                request_body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": budget,
                });
            }
        }

        if !converted_tools.is_empty() {
            request_body["tools"] = serde_json::Value::Array(converted_tools);
        }

        if caps.supports_computer_use && Self::has_computer_use_tools(tools) {
            request_body["betas"] = json!(["computer-use-2025-01-24"]);
        }

        let endpoint = Self::stream_endpoint(provider);
        info!(
            "[BRIDGE] Sending streaming request with tools (provider: {}, tools: {})",
            provider,
            tools.len()
        );

        // Make streaming request to bridge
        let mut stream_tools_req = self
            .http_client
            .post(format!("{}{}", self.bridge_url, endpoint))
            .header("content-type", "application/json");
        if let Some(secret) = crate::bridge::get_bridge_secret() {
            stream_tools_req = stream_tools_req.header("x-bridge-secret", secret);
        }
        let mut response = match stream_tools_req
            .json(&request_body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("[BRIDGE] Streaming request failed, rolling back user message from history");
                history.pop();
                return Err(e).context("Failed to send streaming request to Bridge");
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            warn!(
                "[BRIDGE] Streaming request failed: {} - {}",
                status, error_text
            );
            history.pop();
            anyhow::bail!("Bridge error: {}", error_text);
        }

        // Process SSE stream in real-time (true streaming, not buffered)
        let mut full_text = String::new();
        let mut raw_content: Vec<serde_json::Value> = Vec::new();
        let mut tool_uses: Vec<ToolUseBlock> = Vec::new();
        let mut stop_reason = "end_turn".to_string();
        let mut current_tool_use: Option<(String, String, String)> = None; // (id, name, input_json)
        let mut line_buffer = String::new();
        const MAX_LINE_BUFFER_TOOL: usize = 1024 * 1024; // 1 MB

        while let Some(chunk_result) = response.chunk().await.transpose() {
            let chunk_bytes = match chunk_result {
                Ok(b) => b,
                Err(e) => {
                    warn!("[BRIDGE] Stream read error, rolling back: {}", e);
                    history.pop();
                    return Err(e).context("Failed to read streaming chunk");
                }
            };
            let chunk_text = String::from_utf8_lossy(&chunk_bytes);
            line_buffer.push_str(&chunk_text);
            if line_buffer.len() > MAX_LINE_BUFFER_TOOL {
                anyhow::bail!("Bridge streaming line buffer exceeded 1 MB — possible malformed response");
            }

            // Process complete lines from the buffer
            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].trim_end_matches('\r').to_string();
                line_buffer = line_buffer[newline_pos + 1..].to_string();

                if !line.starts_with("data: ") {
                    continue;
                }
                let data = &line[6..];
                if data == "[DONE]" {
                    break;
                }

                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    let event_type = parsed["type"].as_str().unwrap_or("");

                    match event_type {
                        "error" => {
                            let error_msg = parsed["error"]["message"]
                                .as_str()
                                .unwrap_or("Unknown error");
                            history.pop();
                            anyhow::bail!("Claude API error: {}", error_msg);
                        }
                        "content_block_start" => {
                            let block = &parsed["content_block"];
                            if block["type"].as_str() == Some("tool_use") {
                                let id = block["id"].as_str().unwrap_or("").to_string();
                                let name = block["name"].as_str().unwrap_or("").to_string();
                                current_tool_use = Some((id, name, String::new()));
                            }
                        }
                        "content_block_delta" => {
                            if let Some(delta_text) = parsed["delta"]["text"].as_str() {
                                full_text.push_str(delta_text);
                                callback(delta_text.to_string());
                            }
                            // Accumulate tool input JSON
                            if let Some(partial_json) = parsed["delta"]["partial_json"].as_str() {
                                if let Some(ref mut tu) = current_tool_use {
                                    tu.2.push_str(partial_json);
                                }
                            }
                        }
                        "content_block_stop" => {
                            if let Some((id, name, input_json)) = current_tool_use.take() {
                                let input: serde_json::Value =
                                    serde_json::from_str(&input_json).unwrap_or(json!({}));
                                raw_content.push(json!({
                                    "type": "tool_use",
                                    "id": id,
                                    "name": name,
                                    "input": input,
                                }));
                                tool_uses.push(ToolUseBlock { id, name, input });
                            }
                        }
                        "message_delta" => {
                            if let Some(sr) = parsed["delta"]["stop_reason"].as_str() {
                                stop_reason = sr.to_string();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Build raw_content with text block if we got text
        if !full_text.is_empty() {
            let mut final_content = vec![json!({
                "type": "text",
                "text": full_text,
            })];
            final_content.extend(raw_content.clone());
            raw_content = final_content;
        }

        // Add assistant response to history
        history.push(Message {
            role: "assistant".to_string(),
            content: serde_json::to_string(&raw_content).unwrap_or_default(),
        });
        Self::trim_history(&mut history);

        info!(
            "[BRIDGE] Streaming complete: {} chars text, {} tool_uses, stop_reason={}",
            full_text.len(),
            tool_uses.len(),
            stop_reason
        );

        Ok(ClaudeMessageResult {
            text: full_text,
            tool_uses,
            stop_reason,
            raw_content,
            tool_calls_made: 0,
        })
    }

    /// Load conversation messages from a saved session into the client history.
    /// Skips "system" role compaction markers.
    pub async fn load_session_messages(&self, messages: &[SessionMessage]) {
        self.load_session_messages_with_age_cutoff(messages, None)
            .await;
    }

    /// Load conversation messages with optional age-based filtering.
    /// Messages older than `max_age_days` are dropped before loading.
    /// Returns the number of messages loaded into history.
    pub async fn load_session_messages_with_age_cutoff(
        &self,
        messages: &[SessionMessage],
        max_age_days: Option<u64>,
    ) -> usize {
        let cutoff = max_age_days.map(|d| chrono::Utc::now() - chrono::Duration::days(d as i64));

        let mut history = self.conversation_history.write().await;
        history.clear();

        for msg in messages {
            // Skip compaction marker messages
            if msg.role == "system" {
                continue;
            }
            // Skip messages older than the cutoff
            if let Some(c) = cutoff {
                if msg.timestamp <= c {
                    continue;
                }
            }
            history.push(Message {
                role: msg.role.clone(),
                content: msg.content.clone(),
            });
        }

        Self::trim_history(&mut history);
        let count = history.len();
        info!(
            "[CLAUDE] Loaded {} messages from session (skipped system markers{})",
            count,
            if cutoff.is_some() {
                ", applied age filter"
            } else {
                ""
            }
        );
        count
    }

    /// Update the maximum conversation history length at runtime.
    /// Immediately trims the current history to the new limit.
    pub async fn set_max_history_messages(&self, max: usize) {
        MAX_HISTORY_MESSAGES.store(max, Ordering::Relaxed);
        let mut history = self.conversation_history.write().await;
        Self::trim_history(&mut history);
        info!("[CLAUDE] max_history_messages updated to {}", max);
    }

    /// Clear conversation history
    pub async fn clear_history(&self) {
        let mut history = self.conversation_history.write().await;
        history.clear();
        info!("Conversation history cleared");
    }

    /// Get conversation history
    pub async fn get_history(&self) -> Vec<Message> {
        self.conversation_history.read().await.clone()
    }

    /// Set conversation history (used to restore persisted history).
    /// Sanitizes the history to remove orphaned tool_use/tool_result blocks
    /// that can accumulate from trimming, compaction, or cross-session persistence.
    #[allow(dead_code)]
    pub async fn set_history(&self, messages: Vec<Message>) {
        let mut history = self.conversation_history.write().await;
        let before = messages.len();
        *history = messages;
        Self::sanitize_history(&mut history);
        Self::trim_history(&mut history);
        info!(
            "[CLAUDE] Restored {} messages from persisted history ({} after sanitization)",
            before,
            history.len()
        );
    }

    /// Compact the conversation history by summarizing older messages.
    ///
    /// Splits the history into [old messages to summarize] + [recent N to keep],
    /// sends the old messages to Claude Haiku for summarization, and replaces the
    /// old messages with a single summary message.
    pub async fn compact_conversation(&self) -> Result<CompactionResult> {
        let history = self.conversation_history.write().await;

        if history.len() <= COMPACT_PRESERVE_RECENT {
            return Ok(CompactionResult {
                messages_before: history.len(),
                messages_after: history.len(),
                tokens_before: 0,
                tokens_after: 0,
                was_compacted: false,
            });
        }

        let messages_before = history.len();
        let tokens_before = crate::token_estimate::estimate_messages_tokens(&history);

        // Split: [messages_to_summarize] + [recent_to_keep]
        // Find a safe split point that doesn't break tool_use/tool_result pairs.
        // Start from the desired split point and walk backward until we find a
        // user message that isn't a tool_result block.
        let desired_split = history.len() - COMPACT_PRESERVE_RECENT;
        let mut split_point = desired_split;
        while split_point > 0 {
            let msg = &history[split_point];
            // Check if this message contains tool_result blocks
            let is_tool_result = msg.role == "user" && msg.content.contains("tool_result");
            if !is_tool_result {
                break;
            }
            // Walk backward to include the preceding assistant tool_use message too
            split_point = split_point.saturating_sub(1);
        }
        // Ensure we don't split at 0 (nothing to summarize)
        if split_point == 0 {
            split_point = desired_split;
        }
        let to_summarize = &history[..split_point];
        let to_keep = history[split_point..].to_vec();

        // Build summarization prompt from old messages
        let conversation_text = to_summarize
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        let summary_prompt = format!(
            "Summarize the following conversation concisely. Preserve:\n\
             - Key decisions and conclusions\n\
             - Important facts mentioned (names, preferences, technical details)\n\
             - User preferences expressed\n\
             - Any context needed to continue the conversation naturally\n\
             Keep the summary under 500 words.\n\n\
             CONVERSATION:\n{}",
            conversation_text
        );

        // Release the history lock before making the API call
        drop(history);

        // Call Claude Haiku for summarization (separate API call)
        let api_key = self.get_auth_token().await?;
        let request_body = json!({
            "apiKey": api_key,
            "model": SUMMARIZATION_MODEL,
            "max_tokens": 2048,
            "system": "You are a conversation summarizer. Be concise and factual. Preserve key details.",
            "messages": [{
                "role": "user",
                "content": summary_prompt,
            }],
        });

        info!(
            "[COMPACT] Sending {} messages to Haiku for summarization",
            split_point
        );

        let mut compact_req = self
            .http_client
            .post(format!("{}/api/messages", self.bridge_url))
            .header("content-type", "application/json");
        if let Some(secret) = crate::bridge::get_bridge_secret() {
            compact_req = compact_req.header("x-bridge-secret", secret);
        }
        let response = compact_req
            .json(&request_body)
            .send()
            .await
            .context("Failed to send summarization request")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            anyhow::bail!("Summarization failed ({}): {}", status, error_text);
        }

        let claude_response: ClaudeResponse = response
            .json()
            .await
            .context("Failed to parse summarization response")?;

        let summary_text = Self::extract_text(&claude_response.content);

        // Re-acquire the history lock and replace messages
        let mut history = self.conversation_history.write().await;

        let summary_message = Message {
            role: "user".to_string(),
            content: format!(
                "[CONVERSATION SUMMARY - The following is a summary of our earlier conversation]\n\n\
                 {}\n\n\
                 [END SUMMARY - The conversation continues below]",
                summary_text
            ),
        };

        let mut new_history = vec![summary_message];
        new_history.extend(to_keep.clone());

        // Detect messages added while the lock was released (during the API call).
        // If the history grew, append the new messages so they aren't silently lost.
        if history.len() > messages_before {
            let new_msgs = &history[messages_before..];
            info!(
                "[COMPACT] {} messages arrived during compaction, preserving them",
                new_msgs.len()
            );
            new_history.extend_from_slice(new_msgs);
        }

        let messages_after = new_history.len();
        let tokens_after = crate::token_estimate::estimate_messages_tokens(&new_history);

        *history = new_history;

        info!(
            "[COMPACT] Done: {} -> {} messages, ~{} -> ~{} tokens",
            messages_before, messages_after, tokens_before, tokens_after
        );

        Ok(CompactionResult {
            messages_before,
            messages_after,
            tokens_before,
            tokens_after,
            was_compacted: true,
        })
    }

    /// Get the model used for the most recent routing decision.
    pub async fn get_current_model(&self) -> Option<String> {
        self.current_routing_model.lock().await.clone()
    }

    /// Get estimated context usage as (total_tokens, window_size, usage_percent).
    pub async fn get_context_usage(&self, model: &str) -> (usize, usize, f64) {
        let history = self.conversation_history.read().await;
        let msg_tokens = crate::token_estimate::estimate_messages_tokens(&history);
        // Approximate system prompt size
        let system_tokens =
            crate::token_estimate::estimate_tokens(&Self::build_system_prompt("", None));
        let total = msg_tokens + system_tokens;
        let window = crate::token_estimate::context_window_for_model(model);
        let pct = (total as f64 / window as f64) * 100.0;
        (total, window, pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_history_no_trim_needed() {
        let mut history: Vec<Message> = (0..10)
            .map(|i| Message {
                role: "user".to_string(),
                content: format!("message {}", i),
            })
            .collect();
        ClaudeClient::trim_history(&mut history);
        assert_eq!(history.len(), 10);
    }

    #[test]
    fn test_trim_history_preserves_first_message() {
        let mut history: Vec<Message> = Vec::new();
        // First message (system context)
        history.push(Message {
            role: "system".to_string(),
            content: "system context".to_string(),
        });
        // Fill beyond MAX_HISTORY_MESSAGES
        let max = MAX_HISTORY_MESSAGES.load(Ordering::Relaxed);
        for i in 1..=(max + 10) {
            history.push(Message {
                role: "user".to_string(),
                content: format!("message {}", i),
            });
        }
        let original_len = history.len();
        assert!(original_len > max);

        ClaudeClient::trim_history(&mut history);

        assert_eq!(history.len(), max);
        // First message should be preserved
        assert_eq!(history[0].content, "system context");
        // Last message should be the most recent
        assert_eq!(
            history.last().unwrap().content,
            format!("message {}", max + 10)
        );
    }

    #[test]
    fn test_trim_history_at_boundary() {
        let max = MAX_HISTORY_MESSAGES.load(Ordering::Relaxed);
        let mut history: Vec<Message> = (0..max)
            .map(|i| Message {
                role: "user".to_string(),
                content: format!("message {}", i),
            })
            .collect();
        ClaudeClient::trim_history(&mut history);
        assert_eq!(history.len(), max);
    }

    #[test]
    fn test_trim_history_just_over() {
        let max = MAX_HISTORY_MESSAGES.load(Ordering::Relaxed);
        let mut history: Vec<Message> = (0..=max)
            .map(|i| Message {
                role: "user".to_string(),
                content: format!("message {}", i),
            })
            .collect();
        assert_eq!(history.len(), max + 1);
        ClaudeClient::trim_history(&mut history);
        assert_eq!(history.len(), max);
    }

    #[test]
    fn test_sanitize_removes_orphaned_tool_results() {
        let mut history =
            vec![
            Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "text", "text": "Let me check."},
                    {"type": "tool_use", "id": "tool_123", "name": "search", "input": {}}
                ])).unwrap(),
            },
            Message {
                role: "user".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "tool_result", "tool_use_id": "tool_123", "content": "result A"}
                ])).unwrap(),
            },
            // Orphaned tool_result — references a tool_use that doesn't exist
            Message {
                role: "user".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "tool_result", "tool_use_id": "tool_ORPHAN", "content": "stale result"}
                ])).unwrap(),
            },
            Message {
                role: "assistant".to_string(),
                content: "Done.".to_string(),
            },
        ];

        ClaudeClient::sanitize_history(&mut history);

        // Orphaned tool_result message should be removed entirely
        assert!(
            !history.iter().any(|m| m.content.contains("tool_ORPHAN")),
            "Orphaned tool_result should be removed: {:?}",
            history
        );
        // Valid tool_result should remain
        assert!(
            history.iter().any(|m| m.content.contains("tool_123")),
            "Valid tool_result should remain"
        );
    }

    #[test]
    fn test_sanitize_removes_orphaned_tool_uses() {
        // Orphaned tool_use in a NON-last assistant message should be removed.
        // (The last assistant's tool_uses are protected as "in-flight".)
        let mut history = vec![
            Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            },
            // Assistant has tool_use but no tool_result follows — orphaned
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "tool_use", "id": "tool_ORPHAN", "name": "search", "input": {}}
                ]))
                .unwrap(),
            },
            // Subsequent messages make the above NOT the last assistant
            Message {
                role: "user".to_string(),
                content: "ok".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "Done.".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "thanks".to_string(),
            },
        ];

        ClaudeClient::sanitize_history(&mut history);

        // Orphaned tool_use should be removed (it's not in the last assistant)
        assert!(
            !history.iter().any(|m| m.content.contains("tool_ORPHAN")),
            "Orphaned tool_use should be removed: {:?}",
            history
        );
    }

    #[test]
    fn test_sanitize_preserves_matched_pairs() {
        let mut history = vec![
            Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "text", "text": "Searching..."},
                    {"type": "tool_use", "id": "tool_A", "name": "search", "input": {}}
                ]))
                .unwrap(),
            },
            Message {
                role: "user".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "tool_result", "tool_use_id": "tool_A", "content": "found it"}
                ]))
                .unwrap(),
            },
            Message {
                role: "assistant".to_string(),
                content: "Here's what I found.".to_string(),
            },
        ];

        let before = history.len();
        ClaudeClient::sanitize_history(&mut history);

        assert_eq!(history.len(), before, "Matched pairs should not be removed");
    }

    #[test]
    fn test_sanitize_handles_plain_text_messages() {
        let mut history = vec![
            Message {
                role: "user".to_string(),
                content: "hi".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "hello".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "how are you?".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "I'm good!".to_string(),
            },
        ];

        let before = history.len();
        ClaudeClient::sanitize_history(&mut history);

        assert_eq!(
            history.len(),
            before,
            "Plain text messages should be untouched"
        );
    }

    #[test]
    fn test_sanitize_removes_leading_assistant_message() {
        let mut history = vec![
            Message {
                role: "assistant".to_string(),
                content: "stale response".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "hi there".to_string(),
            },
        ];

        ClaudeClient::sanitize_history(&mut history);

        assert_eq!(
            history[0].role, "user",
            "History should not start with assistant message"
        );
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_sanitize_preserves_last_assistant_tool_uses() {
        // Regression test: simulates the state after continue_after_tools pushes an
        // assistant message with tool_use, but BEFORE add_tool_result has been called.
        // The tool_use should NOT be stripped as orphaned — it's "in-flight".
        let mut history = vec![
            Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "text", "text": "Let me search for that."},
                    {"type": "tool_use", "id": "tool_INFLIGHT", "name": "search", "input": {}}
                ]))
                .unwrap(),
            },
        ];

        ClaudeClient::sanitize_history(&mut history);

        // The tool_use in the last assistant message should be preserved
        assert!(
            history.iter().any(|m| m.content.contains("tool_INFLIGHT")),
            "In-flight tool_use in last assistant message should be preserved: {:?}",
            history
        );
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_sanitize_full_tool_loop_scenario() {
        // Regression test for the exact failure scenario:
        // 1. continue_after_tools pushes assistant [text, tool_use_X] → trim → sanitize
        // 2. Without the fix, tool_use_X would be stripped as "orphaned" (no result yet)
        // 3. Then add_tool_result pushes [tool_result_X] → trim → sanitize
        // 4. tool_result_X would be orphaned too → removed → history ends with assistant
        //
        // With the fix, tool_use_X in the last assistant is protected.

        // Simulate state right after continue_after_tools pushed the assistant message:
        let mut history = vec![
            Message { role: "user".to_string(), content: "search cats".to_string() },
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "text", "text": "I'll search for that."},
                    {"type": "tool_use", "id": "tool_LIVE", "name": "web_search", "input": {"q": "cats"}}
                ])).unwrap(),
            },
        ];

        // First sanitize pass (as would happen during trim_history after assistant push)
        ClaudeClient::sanitize_history(&mut history);

        // tool_use should be preserved (it's in the last assistant)
        assert!(
            history.iter().any(|m| m.content.contains("tool_LIVE")),
            "In-flight tool_use should survive first sanitize"
        );

        // Now simulate add_tool_result
        history.push(Message {
            role: "user".to_string(),
            content: serde_json::to_string(&serde_json::json!([
                {"type": "tool_result", "tool_use_id": "tool_LIVE", "content": "Found 42 cats"}
            ]))
            .unwrap(),
        });

        // Second sanitize pass (as would happen during trim_history after tool_result push)
        ClaudeClient::sanitize_history(&mut history);

        // Both the tool_use and tool_result should be preserved (matched pair)
        assert!(
            history.iter().any(|m| m.content.contains("tool_LIVE")),
            "Matched tool_use/tool_result pair should survive second sanitize"
        );
        // History should end with the user tool_result message
        assert_eq!(
            history.last().unwrap().role,
            "user",
            "History should end with user (tool_result) message"
        );
        assert_eq!(history.len(), 3);
    }

    /// Regression test for the primary bug:
    /// When an LLM response contains N tool_use blocks and trim_history fires
    /// after the FIRST tool_result is added (so history ends with user[tool_result:A]),
    /// the remaining in-flight tool_uses (B, C…) in the preceding assistant must NOT
    /// be stripped as orphaned — they are still awaiting their results.
    #[test]
    fn test_sanitize_mid_batch_preserves_inflight_tool_uses() {
        // State: assistant has [tool_use:A, tool_use:B], but only tool_result:A
        // has been added so far. trim_history fired → sanitize must not strip tool_use:B.
        let mut history = vec![
            Message { role: "user".to_string(), content: "do two things".to_string() },
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "tool_use", "id": "tool_A", "name": "search", "input": {}},
                    {"type": "tool_use", "id": "tool_B", "name": "fetch", "input": {}}
                ])).unwrap(),
            },
            // Only tool_result:A has been added — tool_B result is still in-flight
            Message {
                role: "user".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "tool_result", "tool_use_id": "tool_A", "content": "result A"}
                ])).unwrap(),
            },
        ];

        ClaudeClient::sanitize_history(&mut history);

        // tool_use:A and tool_use:B must BOTH survive — B is still in-flight
        let content: String = history.iter().map(|m| m.content.as_str()).collect();
        assert!(content.contains("tool_A"), "tool_use:A must survive (has matching result)");
        assert!(content.contains("tool_B"), "tool_use:B must survive (in-flight, result pending)");
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_sanitize_keeps_trailing_assistant_with_tool_use() {
        // A trailing assistant message WITH tool_use blocks should be preserved
        // (in-flight tool calls — tool_results are coming).
        let mut history = vec![
            Message { role: "user".to_string(), content: "search for cats".to_string() },
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_string(&serde_json::json!([
                    {"type": "text", "text": "Searching..."},
                    {"type": "tool_use", "id": "tool_PENDING", "name": "web_search", "input": {"q": "cats"}}
                ])).unwrap(),
            },
        ];

        ClaudeClient::sanitize_history(&mut history);

        // The trailing assistant with tool_use should be kept (Phase 4 exemption)
        assert_eq!(history.len(), 2);
        assert_eq!(history.last().unwrap().role, "assistant");
        assert!(history.last().unwrap().content.contains("tool_PENDING"));
    }
}
