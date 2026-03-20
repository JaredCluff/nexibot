//! Conversation history management extracted from claude.rs.
//!
//! Provides a `ConversationManager` that handles message history,
//! trimming, compaction, and session loading.
#![allow(dead_code)]

use std::sync::Arc;

use serde_json::json;
use tokio::sync::RwLock;
use tracing::info;

use crate::claude::Message;
use crate::memory::SessionMessage;

/// Maximum number of messages to retain in conversation history.
const MAX_HISTORY_MESSAGES: usize = 200;

/// Number of recent messages to preserve during compaction (not summarized).
const COMPACT_PRESERVE_RECENT: usize = 6;

/// Manages conversation history for an LLM session.
#[derive(Clone)]
pub struct ConversationManager {
    history: Arc<RwLock<Vec<Message>>>,
}

impl ConversationManager {
    pub fn new() -> Self {
        Self {
            history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a user message to history.
    pub async fn add_user_message(&self, content: &str) {
        let mut history = self.history.write().await;
        history.push(Message {
            role: "user".to_string(),
            content: content.to_string(),
        });
        Self::trim_history(&mut history);
    }

    /// Add an assistant message to history.
    pub async fn add_assistant_message(&self, content: &str) {
        let mut history = self.history.write().await;
        history.push(Message {
            role: "assistant".to_string(),
            content: content.to_string(),
        });
        Self::trim_history(&mut history);
    }

    /// Add a tool result to conversation history.
    ///
    /// Multiple tool results from the same LLM response are batched into a
    /// single user message (matching the Anthropic API requirement that all
    /// tool_result blocks referencing a given assistant turn's tool_uses must
    /// appear in one contiguous user message immediately following it).
    pub async fn add_tool_result(&self, tool_use_id: &str, content: &str) {
        let new_result = serde_json::json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
        });
        let mut history = self.history.write().await;
        // Append to existing user tool_result message if present
        if let Some(last) = history.last_mut() {
            if last.role == "user" {
                if let Ok(mut arr) = serde_json::from_str::<Vec<serde_json::Value>>(&last.content) {
                    if arr
                        .first()
                        .and_then(|v| v.get("type"))
                        .and_then(|t| t.as_str())
                        == Some("tool_result")
                    {
                        arr.push(new_result);
                        last.content = serde_json::to_string(&arr).unwrap_or_default();
                        return;
                    }
                }
            }
        }
        // No existing tool_result message — create a new one
        history.push(Message {
            role: "user".to_string(),
            content: serde_json::to_string(&json!([new_result])).unwrap_or_default(),
        });
        Self::trim_history(&mut history);
    }

    /// Get a read-only snapshot of the current history.
    pub async fn get_history(&self) -> Vec<Message> {
        self.history.read().await.clone()
    }

    /// Clear all conversation history.
    pub async fn clear_history(&self) {
        let mut history = self.history.write().await;
        history.clear();
    }

    /// Load session messages into history (from a saved session).
    pub async fn load_session_messages(&self, messages: &[SessionMessage]) {
        let mut history = self.history.write().await;
        history.clear();
        for msg in messages {
            history.push(Message {
                role: msg.role.clone(),
                content: msg.content.clone(),
            });
        }
        Self::trim_history(&mut history);
        info!("[CONVERSATION] Loaded {} session messages", history.len());
    }

    /// Get estimated context usage.
    pub async fn get_context_usage(&self, model: &str) -> (usize, usize, f64) {
        let history = self.history.read().await;
        let estimated_tokens: usize = history.iter().map(|m| m.content.len() / 4).sum();

        let context_window = Self::context_window_for_model(model);
        let usage_fraction = if context_window > 0 {
            estimated_tokens as f64 / context_window as f64
        } else {
            0.0
        };

        (estimated_tokens, context_window, usage_fraction)
    }

    /// Get the message count.
    pub async fn message_count(&self) -> usize {
        self.history.read().await.len()
    }

    /// Trim conversation history to stay within bounds.
    ///
    /// Always preserves the first message (system prompt) and trims the oldest
    /// non-first messages when the history exceeds MAX_HISTORY_MESSAGES.
    /// Calls sanitize_history afterward to remove any orphaned tool blocks
    /// created by the trim.
    fn trim_history(history: &mut Vec<Message>) {
        if history.len() <= MAX_HISTORY_MESSAGES {
            return;
        }
        let excess = history.len() - MAX_HISTORY_MESSAGES;
        if history.len() > 1 {
            history.drain(1..=excess);
            info!("[CONVERSATION] Trimmed {} messages from history", excess);
        }
        Self::sanitize_history(history);
    }

    /// Remove orphaned tool_use and tool_result blocks from history.
    ///
    /// Mirrors the logic in ClaudeClient::sanitize_history. Always protects
    /// the last assistant message's tool_uses (in-flight mid-batch execution).
    fn sanitize_history(history: &mut Vec<Message>) {
        use std::collections::HashSet;

        // Remove leading assistant messages
        while history.first().map(|m| m.role.as_str()) == Some("assistant") {
            history.remove(0);
        }

        let mut tool_use_ids: HashSet<String> = HashSet::new();
        let mut tool_result_refs: HashSet<String> = HashSet::new();

        for msg in history.iter() {
            if let Ok(blocks) = serde_json::from_str::<Vec<serde_json::Value>>(&msg.content) {
                for block in &blocks {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("tool_use") => {
                            if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                                tool_use_ids.insert(id.to_string());
                            }
                        }
                        Some("tool_result") => {
                            if let Some(id) =
                                block.get("tool_use_id").and_then(|i| i.as_str())
                            {
                                tool_result_refs.insert(id.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Always protect the last assistant message's tool_uses (in-flight)
        let mut last_assistant_tool_use_ids: HashSet<String> = HashSet::new();
        for msg in history.iter().rev() {
            if msg.role == "assistant" {
                if let Ok(blocks) = serde_json::from_str::<Vec<serde_json::Value>>(&msg.content) {
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

        let orphaned_results: HashSet<&String> =
            tool_result_refs.difference(&tool_use_ids).collect();
        let orphaned_uses: HashSet<&String> = tool_use_ids
            .difference(&tool_result_refs)
            .filter(|id| !last_assistant_tool_use_ids.contains(*id))
            .collect();

        if orphaned_results.is_empty() && orphaned_uses.is_empty() {
            return;
        }

        let mut i = 0;
        while i < history.len() {
            if let Ok(mut blocks) =
                serde_json::from_str::<Vec<serde_json::Value>>(&history[i].content)
            {
                let original_len = blocks.len();
                blocks.retain(|block| {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("tool_result") => {
                            if let Some(id) =
                                block.get("tool_use_id").and_then(|i| i.as_str())
                            {
                                return !orphaned_results.contains(&id.to_string());
                            }
                        }
                        Some("tool_use") => {
                            if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                                return !orphaned_uses.contains(&id.to_string());
                            }
                        }
                        _ => {}
                    }
                    true
                });
                if blocks.len() != original_len {
                    if blocks.is_empty() {
                        history.remove(i);
                        continue;
                    }
                    history[i].content = serde_json::to_string(&blocks).unwrap_or_default();
                }
            }
            i += 1;
        }

        while history.first().map(|m| m.role.as_str()) == Some("assistant") {
            history.remove(0);
        }
    }

    /// Get the context window size for a given model.
    fn context_window_for_model(model: &str) -> usize {
        if model.contains("opus") {
            200_000
        } else if model.contains("sonnet") {
            200_000
        } else if model.contains("haiku") {
            200_000
        } else if model.starts_with("gpt-4o") {
            128_000
        } else if model.starts_with("o1") || model.starts_with("o3") {
            128_000
        } else if model.starts_with("gemini") {
            1_000_000
        } else {
            128_000
        }
    }
}

impl Default for ConversationManager {
    fn default() -> Self {
        Self::new()
    }
}
