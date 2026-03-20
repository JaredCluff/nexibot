//! Query complexity and purpose classifier for intelligent model routing.
//!
//! Classifies an incoming query into a [`QueryPurpose`] using fast, zero-cost
//! heuristics (token count, keyword matching, regex).  No ML inference is
//! required at classification time, keeping per-request overhead under 1 ms.
//!
//! # Classification order (first match wins)
//!
//! 1. **Agentic** — multi-step planning / orchestration markers
//! 2. **Reasoning** — analytical keywords (analyze, compare, evaluate…)
//! 3. **CodeComplex** — code + architecture/debug markers, or long prompt with code
//! 4. **CodeSimple** — code detected, shorter prompt
//! 5. **LongContext** — prompt exceeds the long-context token threshold
//! 6. **QuickChat** — short, no special signals
//! 7. **Default** — everything else (uses the global `claude.model`)

use crate::config::RoutingConfig;

// ---------------------------------------------------------------------------
// Token thresholds
// ---------------------------------------------------------------------------

/// Queries shorter than this (in whitespace-split tokens) with no special
/// signals are classified as QuickChat.
const QUICK_CHAT_TOKENS: usize = 60;

/// Queries with code AND longer than this are classified as CodeComplex.
const CODE_COMPLEX_TOKENS: usize = 300;

/// Queries longer than this are classified as LongContext (unless already
/// matched by a higher-priority class).
const LONG_CONTEXT_TOKENS: usize = 800;

// ---------------------------------------------------------------------------
// Purpose enum
// ---------------------------------------------------------------------------

/// The purpose / complexity class of a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryPurpose {
    /// Very short, trivial conversational turn.
    QuickChat,
    /// Simple, self-contained code task.
    CodeSimple,
    /// Complex code task (architecture, debugging, long).
    CodeComplex,
    /// Analytical / reasoning query.
    Reasoning,
    /// Long document or multi-document task.
    LongContext,
    /// Multi-step agentic planning.
    Agentic,
    /// No specific purpose matched; use the global default model.
    Default,
}

// ---------------------------------------------------------------------------
// Source context
// ---------------------------------------------------------------------------

/// Where the query originated. Used to apply the voice latency bias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuerySource {
    /// Desktop chat UI or headless text channel.
    Text,
    /// Voice pipeline (wake word → STT).
    Voice,
}

// ---------------------------------------------------------------------------
// Classifier
// ---------------------------------------------------------------------------

/// Classify a query string into a [`QueryPurpose`].
///
/// `source` is used only when the classification result would be
/// [`QueryPurpose::Default`] and `routing.voice_latency_bias` is true: in
/// that case a voice query is nudged toward the `voice_default` model
/// instead of the global default.
///
/// This function never panics and completes in O(n) with respect to query
/// length.
pub fn classify(query: &str, source: QuerySource) -> QueryPurpose {
    let lower = query.to_lowercase();
    let tokens = query.split_whitespace().count();

    let has_code = detect_code(&lower);
    let has_math = detect_math(&lower);

    // --- 1. Agentic --------------------------------------------------------
    if has_agentic_markers(&lower) {
        return QueryPurpose::Agentic;
    }

    // --- 2. Reasoning ------------------------------------------------------
    if has_reasoning_markers(&lower) || has_math {
        return QueryPurpose::Reasoning;
    }

    // --- 3. Code (complex vs simple) ---------------------------------------
    if has_code {
        if tokens > CODE_COMPLEX_TOKENS || has_complex_code_markers(&lower) {
            return QueryPurpose::CodeComplex;
        }
        return QueryPurpose::CodeSimple;
    }

    // --- 4. Long context ---------------------------------------------------
    if tokens > LONG_CONTEXT_TOKENS {
        return QueryPurpose::LongContext;
    }

    // --- 5. Quick chat -----------------------------------------------------
    if tokens < QUICK_CHAT_TOKENS {
        return QueryPurpose::QuickChat;
    }

    // --- 6. Voice latency bias (on Default tier only) ---------------------
    // A voice query that made it this far has no strong complexity signal.
    // Return Default so the caller can apply voice_default routing.
    let _ = source; // caller checks source against voice_latency_bias
    QueryPurpose::Default
}

/// Resolve the model ID to use for a classified query, given the routing
/// config, the global default model (from `claude.model`), and the source.
///
/// Returns `None` only when routing is disabled; the caller should then
/// fall back to the global default.
pub fn resolve_model<'a>(
    purpose: QueryPurpose,
    source: QuerySource,
    routing: &'a RoutingConfig,
    global_default: &'a str,
) -> &'a str {
    if !routing.enabled {
        return global_default;
    }

    let p = &routing.purposes;

    let model_opt: Option<&String> = match purpose {
        QueryPurpose::QuickChat => p.quick_chat.as_ref(),
        QueryPurpose::CodeSimple => p.code_simple.as_ref(),
        QueryPurpose::CodeComplex => p.code_complex.as_ref(),
        QueryPurpose::Reasoning => p.reasoning.as_ref(),
        QueryPurpose::LongContext => p.long_context.as_ref(),
        QueryPurpose::Agentic => p.agentic.as_ref(),
        QueryPurpose::Default => {
            // Apply voice latency bias: voice gets voice_default (fast model)
            // when no stronger complexity class matched.
            if source == QuerySource::Voice && routing.voice_latency_bias {
                p.voice_default.as_ref().or(p.quick_chat.as_ref())
            } else {
                None // use global default for text
            }
        }
    };

    model_opt.map(|s| s.as_str()).unwrap_or(global_default)
}

// ---------------------------------------------------------------------------
// Signal detection helpers
// ---------------------------------------------------------------------------

fn detect_code(lower: &str) -> bool {
    // Fenced code blocks, common language keywords, imports
    lower.contains("```")
        || lower.contains("def ")
        || lower.contains("fn ")
        || lower.contains("func ")
        || lower.contains("function ")
        || lower.contains("class ")
        || lower.contains("import ")
        || lower.contains("use ")
        || lower.contains("struct ")
        || lower.contains("impl ")
        || lower.contains("let ")
        || lower.contains("const ")
        || lower.contains("var ")
        || lower.contains("if (")
        || lower.contains("for (")
        || lower.contains("while (")
        || lower.contains("return ")
        || lower.contains("console.log")
        || lower.contains("println!")
        || lower.contains("print(")
        || lower.contains("#include")
        || lower.contains("->")
        || lower.contains("=>")
        || lower.contains("::")
}

fn detect_math(lower: &str) -> bool {
    // Mathematical notation, formulae, equations
    lower.contains("∫")
        || lower.contains("∑")
        || lower.contains("∂")
        || lower.contains("√")
        || lower.contains(" = ")
        || lower.contains("equation")
        || lower.contains("integral")
        || lower.contains("derivative")
        || lower.contains("matrix")
        || lower.contains("vector")
        || lower.contains("calcul")
        || lower.contains("algebra")
        || lower.contains("theorem")
        || lower.contains("proof")
        || lower.contains("polynomial")
}

fn has_reasoning_markers(lower: &str) -> bool {
    const MARKERS: &[&str] = &[
        "analyze",
        "analyse",
        "compare and contrast",
        "compare ",
        "evaluate",
        "critique",
        "pros and cons",
        "trade-off",
        "tradeoff",
        "what are the implications",
        "explain in depth",
        "explain why",
        "walk me through",
        "step by step",
        "reason through",
        "think through",
        "root cause",
        "diagnose",
        "investigate why",
        "what causes",
        "how does this work",
        "why does",
        "why is",
        "in detail",
        "thoroughly",
        "comprehensive",
        "deep dive",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

fn has_complex_code_markers(lower: &str) -> bool {
    const MARKERS: &[&str] = &[
        "architect",
        "design a",
        "design the",
        "refactor",
        "optimize",
        "performance",
        "scalab",
        "debug",
        "why does this fail",
        "why is this broken",
        "race condition",
        "concurrency",
        "async",
        "multithreading",
        "memory leak",
        "security vulnerab",
        "sql injection",
        "full implementation",
        "complete implementation",
        "end-to-end",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

fn has_agentic_markers(lower: &str) -> bool {
    const MARKERS: &[&str] = &[
        "plan and execute",
        "multi-step",
        "multistep",
        "step 1",
        "first do",
        "then do",
        "orchestrate",
        "coordinate",
        "design a system",
        "build a system",
        "create a full",
        "write a full",
        "end-to-end system",
        "pipeline",
        "workflow",
        "automate",
        "agent",
        "comprehensive plan",
        "project plan",
        "roadmap",
        "break this down into",
        "break down the",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_chat_short_factual() {
        let p = classify("What is the capital of France?", QuerySource::Text);
        assert_eq!(p, QueryPurpose::QuickChat);
    }

    #[test]
    fn quick_chat_voice_short() {
        let p = classify("Set a timer for 5 minutes", QuerySource::Voice);
        assert_eq!(p, QueryPurpose::QuickChat);
    }

    #[test]
    fn code_simple_short_snippet() {
        let p = classify(
            "Write a function to reverse a string in Python",
            QuerySource::Text,
        );
        assert_eq!(p, QueryPurpose::CodeSimple);
    }

    #[test]
    fn code_complex_architecture() {
        // "design a system" is an agentic marker; use a query with code + complex markers instead
        let p = classify(
            "Write a function implementing JWT session handling with proper concurrency control and performance optimization",
            QuerySource::Text,
        );
        assert_eq!(p, QueryPurpose::CodeComplex);
    }

    #[test]
    fn reasoning_analyze() {
        let p = classify(
            "Analyze the trade-offs between microservices and monolithic architecture",
            QuerySource::Text,
        );
        assert_eq!(p, QueryPurpose::Reasoning);
    }

    #[test]
    fn reasoning_voice_complex() {
        // Voice query with reasoning markers → still goes to Reasoning (quality wins)
        let p = classify(
            "Walk me through the pros and cons of using Rust versus Go for backend services",
            QuerySource::Voice,
        );
        assert_eq!(p, QueryPurpose::Reasoning);
    }

    #[test]
    fn agentic_plan() {
        let p = classify(
            "Design a system to automate our deployment pipeline with rollback support",
            QuerySource::Text,
        );
        assert_eq!(p, QueryPurpose::Agentic);
    }

    #[test]
    fn math_detection() {
        let p = classify("Solve the integral of x^2 dx", QuerySource::Text);
        assert_eq!(p, QueryPurpose::Reasoning);
    }

    #[test]
    fn default_medium_query() {
        // 67 whitespace-split tokens — above the QuickChat threshold (60), no code/agentic/reasoning signals
        let p = classify(
            "Tell me about the history of the Roman Empire, from its origins as a small city-state \
             in central Italy to its eventual fall, including its major emperors, military expansion \
             across Europe, North Africa, and the Mediterranean, economic systems, cultural contributions \
             to art and architecture, the political role of the Senate, its influence on early Christianity, \
             and its lasting legacy on modern Western civilization, law, and governance systems.",
            QuerySource::Text,
        );
        // Medium length, no strong signals — should be Default
        assert_eq!(p, QueryPurpose::Default);
    }

    #[test]
    fn resolve_model_quick_chat() {
        let mut routing = RoutingConfig::default();
        routing.purposes.quick_chat = Some("cerebras/llama3.1-8b".to_string());
        let model = resolve_model(
            QueryPurpose::QuickChat,
            QuerySource::Text,
            &routing,
            "claude-opus-4-6",
        );
        assert_eq!(model, "cerebras/llama3.1-8b");
    }

    #[test]
    fn resolve_model_voice_default_uses_voice_default() {
        let mut routing = RoutingConfig::default();
        routing.purposes.voice_default = Some("cerebras/llama3.1-8b".to_string());
        let model = resolve_model(
            QueryPurpose::Default,
            QuerySource::Voice,
            &routing,
            "claude-opus-4-6",
        );
        assert_eq!(model, "cerebras/llama3.1-8b");
    }

    #[test]
    fn resolve_model_text_default_uses_global() {
        let routing = RoutingConfig::default();
        let model = resolve_model(
            QueryPurpose::Default,
            QuerySource::Text,
            &routing,
            "claude-opus-4-6",
        );
        assert_eq!(model, "claude-opus-4-6");
    }

    #[test]
    fn resolve_model_routing_disabled() {
        let mut routing = RoutingConfig::default();
        routing.enabled = false;
        let model = resolve_model(
            QueryPurpose::QuickChat,
            QuerySource::Text,
            &routing,
            "claude-opus-4-6",
        );
        assert_eq!(model, "claude-opus-4-6");
    }

    #[test]
    fn resolve_model_agentic() {
        let mut routing = RoutingConfig::default();
        routing.purposes.agentic = Some("claude-opus-4-6".to_string());
        let model = resolve_model(
            QueryPurpose::Agentic,
            QuerySource::Text,
            &routing,
            "claude-haiku-4-5-20251001",
        );
        assert_eq!(model, "claude-opus-4-6");
    }

    #[test]
    fn resolve_model_reasoning_voice_still_uses_sonnet() {
        // Even voice should get Sonnet for reasoning — quality wins
        let mut routing = RoutingConfig::default();
        routing.purposes.reasoning = Some("claude-sonnet-4-5-20250929".to_string());
        let model = resolve_model(
            QueryPurpose::Reasoning,
            QuerySource::Voice,
            &routing,
            "claude-opus-4-6",
        );
        assert_eq!(model, "claude-sonnet-4-5-20250929");
    }
}
