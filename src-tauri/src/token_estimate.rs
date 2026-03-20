//! Token estimation utilities for context window management.
//!
//! Uses a character-based heuristic (~3.5 chars/token) to estimate token counts
//! without requiring a tokenizer dependency. Slightly conservative to trigger
//! compaction early rather than too late.

use crate::claude::Message;

/// Approximate characters per token for Claude tokenization.
/// English text averages ~4 chars/token; we use 3.5 to be conservative
/// (overestimate tokens → compact earlier → safer).
const CHARS_PER_TOKEN: f64 = 3.5;

/// Per-message overhead in tokens (role markers, separators).
const MESSAGE_OVERHEAD_TOKENS: usize = 4;

/// Get the context window size (in tokens) for a given model.
pub fn context_window_for_model(model: &str) -> usize {
    // All current Claude models have 200K context windows
    if model.starts_with("claude-opus")
        || model.starts_with("claude-sonnet")
        || model.starts_with("claude-haiku")
    {
        200_000
    } else if model.starts_with("gpt-4o") {
        128_000
    } else if model.starts_with("o1") || model.starts_with("o3") {
        128_000
    } else {
        // Conservative fallback for unknown models
        100_000
    }
}

/// Estimate token count for a string.
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() as f64 / CHARS_PER_TOKEN).ceil() as usize
}

/// Estimate total tokens for a slice of messages.
pub fn estimate_messages_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| MESSAGE_OVERHEAD_TOKENS + estimate_tokens(&m.content))
        .sum()
}
