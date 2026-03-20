//! Intelligent model routing and Yolo mode configurations.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Time-limited elevated access mode for LLM-initiated config changes.
///
/// When active, the model can modify config files, adjust security settings,
/// and take other privileged actions that are normally restricted. A human
/// must explicitly approve each session via the UI — the model may *request*
/// but never *approve* its own elevated access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoloModeConfig {
    /// Default session duration in seconds. None = no time limit (revoke manually).
    #[serde(default)]
    pub default_duration_secs: Option<u64>,
    /// Whether the model is allowed to send a yolo mode request. Default: true.
    #[serde(default = "default_true")]
    pub allow_model_request: bool,
}

impl Default for YoloModeConfig {
    fn default() -> Self {
        Self {
            default_duration_secs: None,
            allow_model_request: true,
        }
    }
}

/// Per-purpose model assignments for smart routing.
///
/// Each field maps a query purpose to a model ID.  `None` means "use the
/// global `claude.model` default".  Populate only the purposes you want to
/// override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingPurposes {
    /// Very short, trivial conversational turns (< 60 tokens, no code/math/reasoning).
    #[serde(default)]
    pub quick_chat: Option<String>,
    /// Simple code tasks (code detected, short prompt, no architecture keywords).
    #[serde(default)]
    pub code_simple: Option<String>,
    /// Complex code tasks (architecture, debugging, longer prompts with code).
    #[serde(default)]
    pub code_complex: Option<String>,
    /// Analytical / reasoning queries (analyze, compare, evaluate, critique…).
    #[serde(default)]
    pub reasoning: Option<String>,
    /// Long-context tasks (summarisation, document analysis, > 800 tokens).
    #[serde(default)]
    pub long_context: Option<String>,
    /// Multi-step agentic planning and orchestration tasks.
    #[serde(default)]
    pub agentic: Option<String>,
    /// Default for voice when no other purpose matched (latency-sensitive).
    /// Falls back to `quick_chat` if unset.
    #[serde(default)]
    pub voice_default: Option<String>,
}

impl Default for RoutingPurposes {
    fn default() -> Self {
        Self {
            quick_chat: None,
            code_simple: None,
            code_complex: None,
            reasoning: None,
            long_context: None,
            agentic: None,
            voice_default: None,
        }
    }
}

/// Intelligent model routing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Master switch. When false the router is bypassed and `claude.model` is
    /// always used (existing behaviour).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// When true, voice-sourced queries prefer the faster model on ties
    /// (i.e. when the classifier returns `Default`).  Complex voice queries
    /// still escalate to the appropriate tier — this only affects borderline
    /// cases.
    #[serde(default = "default_true")]
    pub voice_latency_bias: bool,

    /// Per-purpose model assignments.
    #[serde(default)]
    pub purposes: RoutingPurposes,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            voice_latency_bias: true,
            purposes: RoutingPurposes::default(),
        }
    }
}
