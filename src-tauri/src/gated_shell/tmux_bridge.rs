//! TmuxBridge — NexiBot's universal interactive agent controller.
//!
//! Enables NexiBot to start, observe, and interact with ANY text-based interactive
//! program (Claude Code, Aider, Gemini CLI, Python REPL, Node REPL, custom shells,
//! etc.) through tmux as a universal control layer.
//!
//! # Architecture
//!
//! Every interactive session maps to a named tmux session:
//!
//! ```text
//! NexiBot tool call (nexibot_interactive_agent)
//!       │
//!       ▼
//! TmuxBridge::start_session  → tmux new-session -d -s {id} {program} {args}
//! TmuxBridge::send_keys      → tmux send-keys -t {id} "{input}" Enter
//! TmuxBridge::capture_pane   → tmux capture-pane -t {id} -p
//! TmuxBridge::wait_for_state → polling loop: capture → pattern match → state
//!       │
//!       ├─ Pattern match → return named state (Ready/Running/Approval/Error)
//!       └─ No match, stable N ms → return UnknownStable + pane content (LLM decides)
//! ```
//!
//! # State Detection
//!
//! Each agent type has compiled regex patterns for four states.  The polling
//! loop samples pane content every `poll_interval_ms`, compares to the previous
//! snapshot, and evaluates patterns in priority order:
//!
//! 1. `Stopped`  — tmux session no longer exists
//! 2. `Approval` — approval/confirmation prompt visible
//! 3. `Error`    — error message detected
//! 4. `Running`  — activity in progress (spinning cursor, partial output)
//! 5. `Ready`    — interactive prompt waiting for input
//! 6. `UnknownStable` — content unchanged for `content_stable_ms` ms, no match
//!
//! # LLM Fallback
//!
//! When `UnknownStable` fires, `wait_for_state` returns with the full pane
//! content.  The calling LLM tool result exposes this snapshot so NexiBot can
//! reason about it and decide the next action (send keys, wait longer, abort).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex as TokioMutex;
use tracing::{debug, info, warn};

use crate::config::shell::{CustomAgentPattern, TmuxConfig};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Named states that a tmux session can be in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TmuxState {
    /// Session was just launched; initial boot output not yet settled.
    Starting,
    /// Interactive prompt detected — agent is waiting for input.
    Ready,
    /// Output is actively changing — command/task in progress.
    Running,
    /// Approval/confirmation prompt visible (Y/n, Continue?, etc.).
    Approval,
    /// Error output detected.
    Error,
    /// Content stable for `content_stable_ms` with no pattern match.
    /// Full pane content returned for LLM assessment.
    UnknownStable,
    /// tmux session no longer exists.
    Stopped,
    /// wait_for_state exceeded the timeout.
    Timeout,
}

impl TmuxState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TmuxState::Starting => "Starting",
            TmuxState::Ready => "Ready",
            TmuxState::Running => "Running",
            TmuxState::Approval => "Approval",
            TmuxState::Error => "Error",
            TmuxState::UnknownStable => "UnknownStable",
            TmuxState::Stopped => "Stopped",
            TmuxState::Timeout => "Timeout",
        }
    }
}

/// Metadata for an active tmux interactive session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmuxSessionInfo {
    pub session_id: String,
    pub agent_type: String,
    pub program: String,
    pub args: Vec<String>,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub current_state: String,
}

/// Result of a `wait_for_state` call.
#[derive(Debug, Serialize, Deserialize)]
pub struct TmuxWaitResult {
    pub session_id: String,
    pub state: String,
    /// Captured pane content at the time the state was detected.
    pub content: String,
    /// How long wait_for_state ran before returning (ms).
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Internal session record
// ---------------------------------------------------------------------------

struct TmuxSession {
    session_id: String,
    agent_type: String,
    program: String,
    args: Vec<String>,
    created_at: Instant,
    last_activity: Instant,
    last_pane_snapshot: String,
    current_state: TmuxState,
}

impl TmuxSession {
    fn info(&self) -> TmuxSessionInfo {
        TmuxSessionInfo {
            session_id: self.session_id.clone(),
            agent_type: self.agent_type.clone(),
            program: self.program.clone(),
            args: self.args.clone(),
            created_at_ms: {
                SystemTime::UNIX_EPOCH
                    .elapsed()
                    .unwrap_or_default()
                    .as_millis() as u64
                    - self.created_at.elapsed().as_millis() as u64
            },
            last_activity_ms: {
                SystemTime::UNIX_EPOCH
                    .elapsed()
                    .unwrap_or_default()
                    .as_millis() as u64
                    - self.last_activity.elapsed().as_millis() as u64
            },
            current_state: self.current_state.as_str().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// State detection patterns per agent type
// ---------------------------------------------------------------------------

struct AgentPatterns {
    ready: Option<Regex>,
    running: Option<Regex>,
    approval: Option<Regex>,
    error: Option<Regex>,
}

impl AgentPatterns {
    fn for_agent_type(agent_type: &str, custom: &[CustomAgentPattern]) -> Self {
        // Check custom patterns first
        for c in custom {
            if c.name.eq_ignore_ascii_case(agent_type) {
                return Self {
                    ready: c.ready.as_deref().and_then(|p| Regex::new(p).ok()),
                    running: c.running.as_deref().and_then(|p| Regex::new(p).ok()),
                    approval: c.approval.as_deref().and_then(|p| Regex::new(p).ok()),
                    error: c.error.as_deref().and_then(|p| Regex::new(p).ok()),
                };
            }
        }

        // Built-in patterns
        match agent_type {
            "claude_code" | "claude" => Self {
                // Claude Code interactive prompt ends with ❯ or bash $ inside the TUI
                ready: Regex::new(r"(?m)(❯|[>$])\s*$").ok(),
                // Streaming responses have partial lines
                running: Regex::new(r"(?i)(thinking|working|generating|\.\.\.)").ok(),
                approval: Regex::new(
                    r"(?i)(Do you want to|Allow|Y/n|y/N|Continue\?|Press Enter|Would you like|approve|Approve|permit)",
                ).ok(),
                error: Regex::new(r"(?i)(Error:|error:|Failed:|failed:|cannot|permission denied|No such file)").ok(),
            },
            "aider" => Self {
                ready: Regex::new(r"(?m)aider>\s*$").ok(),
                running: Regex::new(r"(?i)(applying|writing|generating|creating|\.\.\.)").ok(),
                approval: Regex::new(r"(?i)(Apply these changes|y/n|Y/n|yes/no)").ok(),
                error: Regex::new(r"(?i)(Error|error:|Cannot|Failed|No module named)").ok(),
            },
            "gemini" | "gemini_cli" => Self {
                ready: Regex::new(r"(?m)(gemini\s*>|>\s*)\s*$").ok(),
                running: Regex::new(r"(?i)(generating|thinking|\.\.\.)").ok(),
                approval: Regex::new(r"(?i)(approve|confirm|y/n|Y/n|proceed)").ok(),
                error: Regex::new(r"(?i)(Error:|error:|Failed:|429|quota exceeded)").ok(),
            },
            "python" => Self {
                ready: Regex::new(r"(?m)>>>\s*$").ok(),
                running: None,
                approval: None,
                error: Regex::new(r"(?m)(Traceback|Error:|SyntaxError|NameError|TypeError)").ok(),
            },
            "node" | "nodejs" => Self {
                ready: Regex::new(r"(?m)>\s*$").ok(),
                running: None,
                approval: None,
                error: Regex::new(r"(?m)(Uncaught|Error:|TypeError|ReferenceError)").ok(),
            },
            "ipython" => Self {
                ready: Regex::new(r"(?m)In \[\d+\]:\s*$").ok(),
                running: None,
                approval: None,
                error: Regex::new(r"(?m)(Error:|Exception|Traceback)").ok(),
            },
            // Generic fallback: any shell prompt
            _ => Self {
                ready: Regex::new(r"(?m)[\$#>]\s*$").ok(),
                running: None,
                approval: Regex::new(r"(?i)(y/n|Y/n|yes/no|Continue\?|Press Enter)").ok(),
                error: Regex::new(r"(?i)(Error:|error:|command not found|permission denied)").ok(),
            },
        }
    }

    /// Classify the current pane content into a TmuxState.
    /// Priority: Approval > Error > Ready > Running (content changed) > None
    fn classify(&self, content: &str, content_changed: bool) -> Option<TmuxState> {
        if let Some(ref r) = self.approval {
            if r.is_match(content) {
                return Some(TmuxState::Approval);
            }
        }
        if let Some(ref r) = self.error {
            if r.is_match(content) {
                return Some(TmuxState::Error);
            }
        }
        if let Some(ref r) = self.ready {
            if r.is_match(content) {
                return Some(TmuxState::Ready);
            }
        }
        if content_changed {
            if let Some(ref r) = self.running {
                if r.is_match(content) {
                    return Some(TmuxState::Running);
                }
            }
            // Content changed but no specific pattern → still Running
            return Some(TmuxState::Running);
        }
        None
    }
}

// ---------------------------------------------------------------------------
// TmuxBridge
// ---------------------------------------------------------------------------

pub struct TmuxBridge {
    sessions: Arc<TokioMutex<HashMap<String, TmuxSession>>>,
    config: Arc<TokioMutex<TmuxConfig>>,
    app_handle: AppHandle,
    /// Unpredictable tmux socket name — passed as `-L <socket_name>` on every
    /// tmux invocation.  Placing sessions on a CSPRNG-named socket prevents
    /// other same-UID processes from enumerating them via `tmux list-sessions`
    /// (which queries the *default* socket only).
    socket_name: String,
}

impl TmuxBridge {
    pub fn new(config: TmuxConfig, app_handle: AppHandle) -> Self {
        // CSPRNG socket name isolates our sessions from other same-UID processes.
        let socket_name = format!("nxb-{}", uuid_short());
        Self {
            sessions: Arc::new(TokioMutex::new(HashMap::new())),
            config: Arc::new(TokioMutex::new(config)),
            app_handle,
            socket_name,
        }
    }

    /// Create a tmux [`Command`] pre-configured with our isolated socket (`-L <name>`).
    /// All tmux subcommands MUST use this helper so they operate on the same server.
    fn tmux(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("tmux");
        cmd.args(["-L", &self.socket_name]);
        cmd
    }

    fn emit_ui_event(&self, event: &str, payload: serde_json::Value) {
        if let Err(e) = self.app_handle.emit(event, payload) {
            warn!("[TMUXBRIDGE] Failed to emit {}: {}", event, e);
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.try_lock().map(|c| c.enabled).unwrap_or(false)
    }

    pub async fn update_config(&self, new_config: TmuxConfig) {
        let mut cfg = self.config.lock().await;
        *cfg = new_config;
    }

    // -----------------------------------------------------------------------
    // Session lifecycle
    // -----------------------------------------------------------------------

    /// Start a new tmux session running `program args`.
    ///
    /// Returns the session ID (used for all subsequent operations).
    pub async fn start_session(
        &self,
        agent_type: &str,
        program: &str,
        args: &[String],
    ) -> Result<String> {
        let cfg = self.config.lock().await.clone();

        if !cfg.enabled {
            return Err(anyhow!(
                "TmuxBridge is disabled — enable gated_shell.tmux.enabled first"
            ));
        }

        // Check tmux is installed
        if !tmux_available().await {
            return Err(anyhow!("tmux is not installed or not in PATH; install tmux to use interactive agent sessions"));
        }

        {
            let sessions = self.sessions.lock().await;
            if sessions.len() >= cfg.max_sessions {
                return Err(anyhow!(
                    "Max interactive sessions ({}) reached; stop a session first",
                    cfg.max_sessions
                ));
            }
        }

        // Generate a short unique session ID
        let session_id = format!("nx-{}", &uuid_short());

        // Build tmux command: new-session -d (detached) -s {id} -x 220 -y 50 {program} {args}
        let mut cmd_args = vec![
            "new-session".to_string(),
            "-d".to_string(),
            "-s".to_string(),
            session_id.clone(),
            "-x".to_string(),
            "220".to_string(),
            "-y".to_string(),
            "50".to_string(),
            program.to_string(),
        ];
        cmd_args.extend_from_slice(args);

        let output = self.tmux()
            .args(&cmd_args)
            .output()
            .await
            .map_err(|e| anyhow!("Failed to run tmux: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("tmux new-session failed: {}", stderr.trim()));
        }

        let session = TmuxSession {
            session_id: session_id.clone(),
            agent_type: agent_type.to_string(),
            program: program.to_string(),
            args: args.to_vec(),
            created_at: Instant::now(),
            last_activity: Instant::now(),
            last_pane_snapshot: String::new(),
            current_state: TmuxState::Starting,
        };

        {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(session_id.clone(), session);
        }

        info!(
            "[TMUXBRIDGE] Started session '{}' for agent_type='{}' program='{}'",
            session_id, agent_type, program
        );

        self.emit_ui_event(
            "shell:interactive-session-started",
            serde_json::json!({
                "session_id": &session_id,
                "agent_type": agent_type,
                "program": program,
                "args": args,
                "timestamp_ms": now_ms(),
            }),
        );

        Ok(session_id)
    }

    /// Stop (kill) an interactive session.
    pub async fn stop_session(&self, session_id: &str) -> Result<()> {
        {
            let mut sessions = self.sessions.lock().await;
            if sessions.remove(session_id).is_none() {
                return Err(anyhow!("Session '{}' not found", session_id));
            }
        }

        // Kill the tmux session (best-effort; might already be gone)
        let _ = self.tmux()
            .args(["kill-session", "-t", session_id])
            .output()
            .await;

        info!("[TMUXBRIDGE] Stopped session '{}'", session_id);

        self.emit_ui_event(
            "shell:interactive-session-stopped",
            serde_json::json!({
                "session_id": session_id,
                "timestamp_ms": now_ms(),
            }),
        );

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Pane operations
    // -----------------------------------------------------------------------

    /// Send keystrokes to the session.
    ///
    /// Newline `\n` → Enter key. Use `send_enter: false` to suppress trailing Enter.
    pub async fn send_keys(&self, session_id: &str, input: &str, send_enter: bool) -> Result<()> {
        self.require_session(session_id).await?;

        // Build tmux send-keys args
        // We pass the input as-is; tmux handles special chars. Newlines → literal Enter.
        let mut args = vec!["send-keys", "-t", session_id, input];
        if send_enter {
            args.push("Enter");
        }

        let output = self.tmux()
            .args(&args)
            .output()
            .await
            .map_err(|e| anyhow!("tmux send-keys failed: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("tmux send-keys error: {}", stderr.trim()));
        }

        // Touch last_activity
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get_mut(session_id) {
            s.last_activity = Instant::now();
        }

        debug!("[TMUXBRIDGE] Sent keys to '{}': {:?}", session_id, input);

        Ok(())
    }

    /// Capture the current pane content.
    pub async fn capture_pane(&self, session_id: &str) -> Result<String> {
        self.require_session(session_id).await?;

        let output = self.tmux()
            .args(["capture-pane", "-t", session_id, "-p"])
            .output()
            .await
            .map_err(|e| anyhow!("tmux capture-pane failed: {}", e))?;

        if !output.status.success() {
            // Session may have ended
            return Ok(String::new());
        }

        let content = String::from_utf8_lossy(&output.stdout).to_string();

        // Update snapshot
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get_mut(session_id) {
            s.last_pane_snapshot = content.clone();
            s.last_activity = Instant::now();
        }

        Ok(content)
    }

    // -----------------------------------------------------------------------
    // State waiting (polling loop)
    // -----------------------------------------------------------------------

    /// Poll until the session reaches one of `target_states`, or timeout/UnknownStable fires.
    ///
    /// If `target_states` is empty, waits for ANY recognized state change.
    pub async fn wait_for_state(
        &self,
        session_id: &str,
        target_states: &[TmuxState],
        timeout_ms: Option<u64>,
    ) -> Result<TmuxWaitResult> {
        let cfg = self.config.lock().await.clone();
        let poll = Duration::from_millis(cfg.poll_interval_ms);
        let stable_threshold = Duration::from_millis(cfg.content_stable_ms);
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(cfg.wait_timeout_ms));

        // Get agent type for pattern selection
        let (agent_type, custom_agents) = {
            let sessions = self.sessions.lock().await;
            let s = sessions
                .get(session_id)
                .ok_or_else(|| anyhow!("Session '{}' not found", session_id))?;
            (s.agent_type.clone(), cfg.custom_agents.clone())
        };

        let patterns = AgentPatterns::for_agent_type(&agent_type, &custom_agents);

        let started = Instant::now();
        let mut last_content = String::new();
        let mut last_change = Instant::now();
        let mut emitted_initial = false;

        loop {
            // Timeout check
            if started.elapsed() >= timeout {
                let content = last_content.clone();
                self.update_state(session_id, TmuxState::Timeout).await;
                return Ok(TmuxWaitResult {
                    session_id: session_id.to_string(),
                    state: TmuxState::Timeout.as_str().to_string(),
                    content,
                    duration_ms: started.elapsed().as_millis() as u64,
                });
            }

            // Check if session still exists in tmux
            if !self.tmux_session_exists(session_id).await {
                self.update_state(session_id, TmuxState::Stopped).await;
                // Remove from our map
                let mut sessions = self.sessions.lock().await;
                sessions.remove(session_id);

                self.emit_ui_event(
                    "shell:interactive-session-stopped",
                    serde_json::json!({ "session_id": session_id, "timestamp_ms": now_ms() }),
                );

                if target_states.is_empty() || target_states.contains(&TmuxState::Stopped) {
                    return Ok(TmuxWaitResult {
                        session_id: session_id.to_string(),
                        state: TmuxState::Stopped.as_str().to_string(),
                        content: last_content,
                        duration_ms: started.elapsed().as_millis() as u64,
                    });
                }
                return Err(anyhow!("Session '{}' stopped unexpectedly", session_id));
            }

            // Capture pane content
            let content = match self.capture_pane_raw(session_id).await {
                Ok(c) => c,
                Err(_) => {
                    tokio::time::sleep(poll).await;
                    continue;
                }
            };

            let content_changed = content != last_content;
            if content_changed {
                last_change = Instant::now();

                // Emit pane update for real-time UI
                if emitted_initial || !content.trim().is_empty() {
                    self.emit_ui_event(
                        "shell:interactive-pane-update",
                        serde_json::json!({
                            "session_id": session_id,
                            "content": &content,
                            "timestamp_ms": now_ms(),
                        }),
                    );
                    emitted_initial = true;
                }

                last_content = content.clone();
            }

            // Classify state
            let detected = patterns.classify(&content, content_changed);

            if let Some(state) = detected {
                // Update internal state
                self.update_state(session_id, state.clone()).await;

                // Return if this is a target state (or we want any state)
                if target_states.is_empty() || target_states.contains(&state) {
                    return Ok(TmuxWaitResult {
                        session_id: session_id.to_string(),
                        state: state.as_str().to_string(),
                        content,
                        duration_ms: started.elapsed().as_millis() as u64,
                    });
                }
            }

            // UnknownStable: content stable for threshold with no pattern match
            if !content_changed
                && last_change.elapsed() >= stable_threshold
                && !content.trim().is_empty()
            {
                let detected_again = patterns.classify(&content, false);
                if detected_again.is_none() {
                    self.update_state(session_id, TmuxState::UnknownStable)
                        .await;

                    return Ok(TmuxWaitResult {
                        session_id: session_id.to_string(),
                        state: TmuxState::UnknownStable.as_str().to_string(),
                        content,
                        duration_ms: started.elapsed().as_millis() as u64,
                    });
                }
            }

            tokio::time::sleep(poll).await;
        }
    }

    // -----------------------------------------------------------------------
    // Session listing
    // -----------------------------------------------------------------------

    pub async fn list_sessions(&self) -> Vec<TmuxSessionInfo> {
        let sessions = self.sessions.lock().await;
        sessions.values().map(|s| s.info()).collect()
    }

    #[allow(dead_code)]
    pub async fn session_info(&self, session_id: &str) -> Option<TmuxSessionInfo> {
        let sessions = self.sessions.lock().await;
        sessions.get(session_id).map(|s| s.info())
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    async fn require_session(&self, session_id: &str) -> Result<()> {
        let sessions = self.sessions.lock().await;
        if sessions.contains_key(session_id) {
            Ok(())
        } else {
            Err(anyhow!(
                "Interactive session '{}' not found; use action='start' first",
                session_id
            ))
        }
    }

    async fn update_state(&self, session_id: &str, state: TmuxState) {
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get_mut(session_id) {
            if s.current_state != state {
                debug!(
                    "[TMUXBRIDGE] Session '{}' state: {:?} → {:?}",
                    session_id, s.current_state, state
                );
                self.emit_ui_event(
                    "shell:interactive-state-changed",
                    serde_json::json!({
                        "session_id": session_id,
                        "state": state.as_str(),
                        "timestamp_ms": now_ms(),
                    }),
                );
            }
            s.current_state = state;
        }
    }

    /// Capture pane without updating the stored session snapshot (used inside polling loop).
    async fn capture_pane_raw(&self, session_id: &str) -> Result<String> {
        let output = self.tmux()
            .args(["capture-pane", "-t", session_id, "-p"])
            .output()
            .await
            .map_err(|e| anyhow!("tmux capture-pane: {}", e))?;

        if !output.status.success() {
            return Err(anyhow!("tmux capture-pane returned non-zero"));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Check whether a tmux session with this ID actually exists in tmux.
    async fn tmux_session_exists(&self, session_id: &str) -> bool {
        self.tmux()
            .args(["has-session", "-t", session_id])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn tmux_available() -> bool {
    // tmux is a Unix-only terminal multiplexer
    #[cfg(windows)]
    {
        false
    }
    #[cfg(not(windows))]
    {
        tokio::process::Command::new("which")
            .arg("tmux")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

fn uuid_short() -> String {
    // 8 hex chars from cryptographically secure random bytes — prevents
    // local session hijacking by brute-forcing predictable timestamp-based IDs.
    use rand::RngCore;
    let mut bytes = [0u8; 4];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{:08x}", u32::from_le_bytes(bytes))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn make_patterns(agent_type: &str) -> AgentPatterns {
        AgentPatterns::for_agent_type(agent_type, &[])
    }

    #[test]
    fn claude_ready_prompt_detected() {
        let p = make_patterns("claude_code");
        let content = "some output\n❯ ";
        assert_eq!(p.classify(content, false), Some(TmuxState::Ready));
    }

    #[test]
    fn claude_approval_detected() {
        let p = make_patterns("claude_code");
        let content = "Do you want to apply these changes? (Y/n)";
        assert_eq!(p.classify(content, false), Some(TmuxState::Approval));
    }

    #[test]
    fn aider_ready_prompt_detected() {
        let p = make_patterns("aider");
        let content = "aider> ";
        assert_eq!(p.classify(content, false), Some(TmuxState::Ready));
    }

    #[test]
    fn aider_approval_detected() {
        let p = make_patterns("aider");
        let content = "Apply these changes? y/n";
        assert_eq!(p.classify(content, false), Some(TmuxState::Approval));
    }

    #[test]
    fn python_ready_prompt_detected() {
        let p = make_patterns("python");
        let content = "Python 3.12.0\n>>> ";
        assert_eq!(p.classify(content, false), Some(TmuxState::Ready));
    }

    #[test]
    fn python_error_detected() {
        let p = make_patterns("python");
        let content =
            "Traceback (most recent call last):\n  ...\nNameError: name 'x' is not defined";
        assert_eq!(p.classify(content, false), Some(TmuxState::Error));
    }

    #[test]
    fn node_ready_prompt_detected() {
        let p = make_patterns("node");
        let content = "Welcome to Node.js v20\n> ";
        assert_eq!(p.classify(content, false), Some(TmuxState::Ready));
    }

    #[test]
    fn generic_shell_prompt_detected() {
        let p = make_patterns("generic");
        let content = "user@host:~$ ";
        assert_eq!(p.classify(content, false), Some(TmuxState::Ready));
    }

    #[test]
    fn gemini_ready_prompt_detected() {
        let p = make_patterns("gemini");
        let content = "gemini> ";
        assert_eq!(p.classify(content, false), Some(TmuxState::Ready));
    }

    #[test]
    fn running_when_content_changes_no_pattern() {
        let p = make_patterns("claude_code");
        // Content changed, no specific pattern → Running
        let content = "Loading models...";
        assert_eq!(p.classify(content, true), Some(TmuxState::Running));
    }

    #[test]
    fn no_state_when_no_pattern_and_stable() {
        let p = make_patterns("claude_code");
        let content = "Loading models...";
        // Not changed → None (will eventually hit UnknownStable in polling loop)
        assert_eq!(p.classify(content, false), None);
    }

    #[test]
    fn approval_takes_priority_over_ready() {
        let p = make_patterns("claude_code");
        // Has both a prompt and an approval text
        let content = "Do you want to continue? (Y/n)\n❯ ";
        assert_eq!(p.classify(content, false), Some(TmuxState::Approval));
    }

    #[test]
    fn tmux_state_as_str() {
        assert_eq!(TmuxState::Ready.as_str(), "Ready");
        assert_eq!(TmuxState::Approval.as_str(), "Approval");
        assert_eq!(TmuxState::UnknownStable.as_str(), "UnknownStable");
        assert_eq!(TmuxState::Stopped.as_str(), "Stopped");
    }

    #[test]
    fn custom_agent_pattern_overrides_builtin() {
        let custom = vec![CustomAgentPattern {
            name: "my_repl".to_string(),
            ready: Some(r"my-repl>\s*$".to_string()),
            running: None,
            approval: None,
            error: None,
        }];
        let p = AgentPatterns::for_agent_type("my_repl", &custom);
        let content = "my-repl> ";
        assert_eq!(p.classify(content, false), Some(TmuxState::Ready));
    }
}
