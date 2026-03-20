//! `nexibot_interactive_agent` LLM tool — manage tmux interactive sessions.
//!
//! Exposes the TmuxBridge to NexiBot so the AI can start, control, and observe
//! ANY text-based interactive program (Claude Code, Aider, Gemini CLI, Python
//! REPL, etc.) running in a managed tmux session.
//!
//! # Tool Actions
//!
//! | Action | Description |
//! |--------|-------------|
//! | `start`  | Launch a new tmux session running a program |
//! | `send`   | Send keystrokes / text input to a session |
//! | `read`   | Snapshot the current pane content |
//! | `wait`   | Poll until a named state is reached |
//! | `stop`   | Kill a session |
//! | `list`   | List all active sessions |
//!
//! # State Machine
//!
//! `wait` returns one of: `Ready`, `Running`, `Approval`, `Error`,
//! `UnknownStable`, `Stopped`, `Timeout`.
//!
//! When `UnknownStable` is returned, the full `content` field contains the
//! pane snapshot so NexiBot can reason about what's happening and decide the
//! next move.

use serde_json::Value;

use super::AppState;
use crate::gated_shell::tmux_bridge::TmuxState;
use crate::security::safe_bins;

// ---------------------------------------------------------------------------
// Tool definition (presented to the LLM)
// ---------------------------------------------------------------------------

pub fn nexibot_interactive_agent_tool_definition() -> Value {
    serde_json::json!({
        "name": "nexibot_interactive_agent",
        "description": "Start and interact with any text-based interactive program in a managed tmux session. Use this to run Claude Code, Aider, Gemini CLI, Python REPL, Node REPL, or any other interactive tool. The tool handles state detection automatically — it recognizes when a program is waiting for input (Ready), showing an approval prompt (Approval), encountering an error (Error), or running a task (Running). When the state is UnknownStable, the pane content is returned for you to assess and decide what to do next. Requires gated_shell.tmux.enabled: true in config.",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "send", "read", "wait", "stop", "list"],
                    "description": "Action to perform: start=launch session, send=send keystrokes, read=snapshot pane, wait=poll for state, stop=kill session, list=show all sessions"
                },
                "session_id": {
                    "type": "string",
                    "description": "Session ID returned by 'start'. Required for: send, read, wait, stop."
                },
                "agent_type": {
                    "type": "string",
                    "description": "For 'start': the type of agent being launched. Built-in types: claude_code, aider, gemini, python, node, ipython, generic. Controls state-detection patterns. Default: generic."
                },
                "program": {
                    "type": "string",
                    "description": "For 'start': the executable to run (e.g. 'claude', 'aider', 'python3', 'node'). Must be in PATH."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "For 'start': command-line arguments to pass to the program."
                },
                "input": {
                    "type": "string",
                    "description": "For 'send': text to send to the session. A trailing Enter key is added automatically unless send_enter is false. Use '\\n' for explicit newlines within the input."
                },
                "send_enter": {
                    "type": "boolean",
                    "description": "For 'send': whether to append an Enter key after input. Default: true."
                },
                "wait_for": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": ["Ready", "Running", "Approval", "Error", "UnknownStable", "Stopped", "Timeout"]
                    },
                    "description": "For 'wait': list of states to wait for. Returns as soon as ANY of these states is detected. If empty, returns on any state change. Default: [\"Ready\", \"Approval\", \"Error\"]."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "For 'wait': maximum time to wait in milliseconds. Default: 120000 (2 minutes)."
                }
            },
            "required": ["action"]
        }
    })
}

// ---------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------

pub async fn execute_interactive_agent_tool(tool_input: &Value, state: &AppState) -> String {
    let action = match tool_input.get("action").and_then(|v| v.as_str()) {
        Some(a) => a,
        None => return r#"{"error": "Missing required field 'action'"}"#.to_string(),
    };

    // Get gated_shell handle
    let gs = match state.gated_shell.as_deref() {
        Some(gs) => gs,
        None => return r#"{"error": "Gated shell is not available (headless mode)"}"#.to_string(),
    };

    let bridge = &gs.tmux_bridge;

    match action {
        "list" => {
            let sessions = bridge.list_sessions().await;
            match serde_json::to_string(&serde_json::json!({
                "sessions": sessions,
                "count": sessions.len()
            })) {
                Ok(s) => s,
                Err(e) => format!(r#"{{"error": "Serialization failed: {e}"}}"#),
            }
        }

        "start" => {
            let agent_type = tool_input
                .get("agent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("generic");

            let program = match tool_input.get("program").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => {
                    return r#"{"error": "Missing required field 'program' for action='start'"}"#
                        .to_string()
                }
            };

            // Validate that the LLM-supplied program resolves to a trusted system binary.
            // Prevents prompt-injection attacks from spawning arbitrary executables.
            let validated_program = match safe_bins::validate_binary(program) {
                Ok(path) => path,
                Err(e) => {
                    return format!(r#"{{"error": "Program '{}' rejected: {e}"}}"#, program);
                }
            };
            let validated_program_str = validated_program.to_string_lossy();

            let args: Vec<String> = tool_input
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            match bridge.start_session(agent_type, &validated_program_str, &args).await {
                Ok(session_id) => serde_json::json!({
                    "session_id": session_id,
                    "agent_type": agent_type,
                    "program": program,
                    "args": args,
                    "status": "started",
                    "tip": "Use action='wait' with wait_for=['Ready'] to wait for the interactive prompt, then action='send' to interact."
                })
                .to_string(),
                Err(e) => format!(r#"{{"error": "{e}"}}"#),
            }
        }

        "send" => {
            let session_id = match tool_input.get("session_id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => {
                    return r#"{"error": "Missing required field 'session_id' for action='send'"}"#
                        .to_string()
                }
            };
            let input = match tool_input.get("input").and_then(|v| v.as_str()) {
                Some(i) => i,
                None => {
                    return r#"{"error": "Missing required field 'input' for action='send'"}"#
                        .to_string()
                }
            };
            let send_enter = tool_input
                .get("send_enter")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            match bridge.send_keys(session_id, input, send_enter).await {
                Ok(()) => serde_json::json!({
                    "session_id": session_id,
                    "sent": input,
                    "enter": send_enter,
                    "status": "ok",
                    "tip": "Use action='wait' to wait for the program to respond."
                })
                .to_string(),
                Err(e) => format!(r#"{{"error": "{e}"}}"#),
            }
        }

        "read" => {
            let session_id = match tool_input.get("session_id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => {
                    return r#"{"error": "Missing required field 'session_id' for action='read'"}"#
                        .to_string()
                }
            };

            match bridge.capture_pane(session_id).await {
                Ok(content) => serde_json::json!({
                    "session_id": session_id,
                    "content": content,
                    "length": content.len(),
                })
                .to_string(),
                Err(e) => format!(r#"{{"error": "{e}"}}"#),
            }
        }

        "wait" => {
            let session_id = match tool_input.get("session_id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => {
                    return r#"{"error": "Missing required field 'session_id' for action='wait'"}"#
                        .to_string()
                }
            };

            // Parse wait_for states
            let target_states: Vec<TmuxState> = tool_input
                .get("wait_for")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(|s| parse_state(s))
                        .collect()
                })
                .unwrap_or_else(|| {
                    // Default: wait for Ready, Approval, or Error
                    vec![TmuxState::Ready, TmuxState::Approval, TmuxState::Error]
                });

            let timeout_ms = tool_input.get("timeout_ms").and_then(|v| v.as_u64());

            match bridge
                .wait_for_state(session_id, &target_states, timeout_ms)
                .await
            {
                Ok(result) => {
                    let mut obj = serde_json::json!({
                        "session_id": result.session_id,
                        "state": result.state,
                        "duration_ms": result.duration_ms,
                        "content": result.content,
                    });

                    // Add helpful tip based on state
                    let tip = match result.state.as_str() {
                        "Ready" => "The program is waiting for your input. Use action='send' to continue.",
                        "Approval" => "The program is asking for approval. Read the content and use action='send' with 'y' or 'n' as appropriate.",
                        "Error" => "An error was detected in the output. Review the content and decide how to proceed.",
                        "Running" => "The program is still working. Call wait again to continue polling.",
                        "UnknownStable" => "Content is stable but no known pattern matched. Review the 'content' field to determine the state manually.",
                        "Stopped" => "The session has ended.",
                        "Timeout" => "Wait timed out. Use action='read' to get current content, or increase timeout_ms.",
                        _ => "",
                    };
                    if !tip.is_empty() {
                        obj["tip"] = serde_json::Value::String(tip.to_string());
                    }

                    obj.to_string()
                }
                Err(e) => format!(r#"{{"error": "{e}"}}"#),
            }
        }

        "stop" => {
            let session_id = match tool_input.get("session_id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => {
                    return r#"{"error": "Missing required field 'session_id' for action='stop'"}"#
                        .to_string()
                }
            };

            match bridge.stop_session(session_id).await {
                Ok(()) => serde_json::json!({
                    "session_id": session_id,
                    "status": "stopped",
                })
                .to_string(),
                Err(e) => format!(r#"{{"error": "{e}"}}"#),
            }
        }

        unknown => format!(
            r#"{{"error": "Unknown action '{unknown}'. Valid: start, send, read, wait, stop, list"}}"#
        ),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_state(s: &str) -> Option<TmuxState> {
    match s {
        "Starting" => Some(TmuxState::Starting),
        "Ready" => Some(TmuxState::Ready),
        "Running" => Some(TmuxState::Running),
        "Approval" => Some(TmuxState::Approval),
        "Error" => Some(TmuxState::Error),
        "UnknownStable" => Some(TmuxState::UnknownStable),
        "Stopped" => Some(TmuxState::Stopped),
        "Timeout" => Some(TmuxState::Timeout),
        _ => None,
    }
}
