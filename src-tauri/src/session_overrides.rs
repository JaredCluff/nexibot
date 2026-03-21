//! Per-session overrides that temporarily modify behavior without persisting to config.
//!
//! These overrides reset when starting a new conversation and are controlled
//! via slash commands (/model, /think, /verbose, /provider) in the frontend.

use serde::{Deserialize, Serialize};

use crate::config::RoutingConfig;
use crate::llm_provider::LlmProvider;
use crate::query_classifier::{self, QuerySource};
use crate::sandbox::SandboxConfig;

/// Model shorthand aliases mapped to full model IDs.
pub const MODEL_ALIASES: &[(&str, &str)] = &[
    // Anthropic
    ("opus", "claude-opus-4-6-20250918"),
    ("sonnet", "claude-sonnet-4-5-20250929"),
    ("haiku", "claude-haiku-4-5-20251001"),
    // OpenAI
    ("gpt4o", "gpt-4o"),
    ("gpt4", "gpt-4o"),
    ("gpt4o-mini", "gpt-4o-mini"),
    ("o1", "o1"),
    ("o3-mini", "o3-mini"),
    // Cerebras
    ("cerebras", "cerebras/gpt-oss-120b"),
    // Ollama
    ("llama", "ollama/llama3.2"),
    ("llama3", "ollama/llama3.2"),
    ("mistral", "ollama/mistral"),
    ("deepseek", "ollama/deepseek-coder"),
    ("qwen", "ollama/qwen2.5"),
    ("codellama", "ollama/codellama"),
    ("phi", "ollama/phi3"),
    ("gemma", "ollama/gemma2"),
];

/// Known valid model IDs.
const VALID_MODELS: &[&str] = &[
    // Anthropic
    "claude-opus-4-6-20250918",
    "claude-opus-4-6",
    "claude-sonnet-4-5-20250929",
    "claude-haiku-4-5-20251001",
    // OpenAI
    "gpt-4o",
    "gpt-4o-mini",
    "o1",
    "o1-mini",
    "o3-mini",
];

/// Default extended thinking budget (tokens).
pub const DEFAULT_THINKING_BUDGET: usize = 10_000;

/// Per-session sandbox policy overrides (can only tighten, not loosen).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SandboxOverrides {
    /// Memory limit override (e.g., "256m"). Must not exceed base config.
    pub memory_limit: Option<String>,
    /// CPU limit override (e.g., 0.5). Must not exceed base config.
    pub cpu_limit: Option<f64>,
    /// Timeout override in seconds. Must not exceed base config.
    pub timeout_seconds: Option<u64>,
    /// Additional paths to block (merged with base config blocked_paths).
    pub additional_blocked_paths: Option<Vec<String>>,
}

impl SandboxOverrides {
    /// Merge session overrides with base config, producing the effective config.
    /// Overrides can only tighten restrictions — looser values are clamped to the base.
    #[allow(dead_code)]
    pub fn apply_to(&self, mut base: SandboxConfig) -> SandboxConfig {
        if let Some(ref mem) = self.memory_limit {
            // Only apply if it parses to a lower value than base
            if let (Some(override_bytes), Some(base_bytes)) =
                (parse_memory(mem), parse_memory(&base.memory_limit))
            {
                if override_bytes <= base_bytes {
                    base.memory_limit = mem.clone();
                }
            }
        }
        if let Some(cpu) = self.cpu_limit {
            if cpu < base.cpu_limit {
                base.cpu_limit = cpu;
            }
        }
        if let Some(timeout) = self.timeout_seconds {
            if timeout < base.timeout_seconds {
                base.timeout_seconds = timeout;
            }
        }
        if let Some(ref extra_paths) = self.additional_blocked_paths {
            base.blocked_paths.extend(extra_paths.clone());
            base.blocked_paths.sort();
            base.blocked_paths.dedup();
        }
        base
    }
}

/// Parse a Docker-style memory string (e.g., "512m", "1g") into bytes.
#[allow(dead_code)]
fn parse_memory(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    if let Some(val) = s.strip_suffix('g') {
        val.parse::<u64>().ok().map(|v| v * 1024 * 1024 * 1024)
    } else if let Some(val) = s.strip_suffix('m') {
        val.parse::<u64>().ok().map(|v| v * 1024 * 1024)
    } else if let Some(val) = s.strip_suffix('k') {
        val.parse::<u64>().ok().map(|v| v * 1024)
    } else {
        s.parse::<u64>().ok()
    }
}

/// Per-session state that overrides config without persisting.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionOverrides {
    /// Model override. None = use config default.
    pub model: Option<String>,
    /// Extended thinking budget. None = disabled, Some(N) = enabled with N tokens.
    pub thinking_budget: Option<usize>,
    /// Show raw tool results and thinking blocks in the UI.
    pub verbose: bool,
    /// Provider override. None = auto-detect from model name.
    pub provider: Option<LlmProvider>,
    /// Per-session sandbox policy overrides.
    pub sandbox: Option<SandboxOverrides>,
}

impl SessionOverrides {
    /// Get the effective model: override if set, otherwise the config default.
    pub fn effective_model<'a>(&'a self, config_model: &'a str) -> &'a str {
        self.model.as_deref().unwrap_or(config_model)
    }

    /// Get the effective model for a specific query, applying intelligent routing.
    ///
    /// Priority:
    /// 1. Session override (user explicitly chose a model) — always respected.
    /// 2. Routing classifier (query text + source → purpose → routed model).
    /// 3. Global config default (`config_model`).
    ///
    /// When `query` is empty (e.g., tool-continuation turns) the classifier
    /// returns `Default`, so the behaviour degrades gracefully to the config
    /// default or voice_default as appropriate.
    pub fn effective_model_for_query<'a>(
        &'a self,
        query: &str,
        source: QuerySource,
        routing: &'a RoutingConfig,
        config_model: &'a str,
    ) -> &'a str {
        // Session override always wins.
        if let Some(ref m) = self.model {
            return m.as_str();
        }
        let purpose = query_classifier::classify(query, source);
        let routed = query_classifier::resolve_model(purpose, source, routing, config_model);
        if routed != config_model {
            tracing::debug!(
                "[ROUTER] {:?} ({:?}) → {} (was: {})",
                purpose,
                source,
                routed,
                config_model
            );
        }
        routed
    }

    /// Get the effective provider: explicit override, or auto-detect from model name.
    pub fn effective_provider(&self, model: &str) -> LlmProvider {
        self.provider
            .unwrap_or_else(|| crate::llm_provider::provider_for_model(model))
    }

    /// Reset all overrides to defaults.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Resolve a model name, expanding shorthand aliases.
    /// Returns Ok(full_model_id) or Err(message) if unknown.
    pub fn resolve_model_name(name: &str) -> Result<String, String> {
        let lower = name.to_lowercase();

        // Check aliases first
        for (alias, full_id) in MODEL_ALIASES {
            if lower == *alias {
                return Ok(full_id.to_string());
            }
        }

        // Check if it's already a valid full model ID
        for valid in VALID_MODELS {
            if lower == valid.to_lowercase() || name == *valid {
                return Ok(valid.to_string());
            }
        }

        // Allow any Cerebras model (cerebras/ prefix)
        if lower.starts_with("cerebras/") {
            return Ok(name.to_string());
        }

        // Allow any Ollama model (ollama/ prefix or known local model names)
        if lower.starts_with("ollama/") {
            return Ok(name.to_string());
        }
        if crate::llm_provider::provider_for_model(&lower)
            == crate::llm_provider::LlmProvider::Ollama
        {
            return Ok(format!("ollama/{}", name));
        }

        Err(format!(
            "Unknown model '{}'. Valid options: opus, sonnet, haiku, gpt4o, gpt4o-mini, o1, o3-mini, cerebras, llama, mistral, deepseek, qwen, or a full model ID.",
            name
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Anthropic alias tests ---

    #[test]
    fn test_resolve_alias_opus() {
        assert_eq!(
            SessionOverrides::resolve_model_name("opus").unwrap(),
            "claude-opus-4-6-20250918"
        );
    }

    #[test]
    fn test_resolve_alias_sonnet() {
        assert_eq!(
            SessionOverrides::resolve_model_name("sonnet").unwrap(),
            "claude-sonnet-4-5-20250929"
        );
    }

    #[test]
    fn test_resolve_alias_haiku() {
        assert_eq!(
            SessionOverrides::resolve_model_name("haiku").unwrap(),
            "claude-haiku-4-5-20251001"
        );
    }

    // --- OpenAI alias tests ---

    #[test]
    fn test_resolve_alias_gpt4o() {
        assert_eq!(
            SessionOverrides::resolve_model_name("gpt4o").unwrap(),
            "gpt-4o"
        );
    }

    #[test]
    fn test_resolve_alias_gpt4o_mini() {
        assert_eq!(
            SessionOverrides::resolve_model_name("gpt4o-mini").unwrap(),
            "gpt-4o-mini"
        );
    }

    #[test]
    fn test_resolve_alias_o1() {
        assert_eq!(SessionOverrides::resolve_model_name("o1").unwrap(), "o1");
    }

    #[test]
    fn test_resolve_alias_o3_mini() {
        assert_eq!(
            SessionOverrides::resolve_model_name("o3-mini").unwrap(),
            "o3-mini"
        );
    }

    // --- Ollama alias tests ---

    #[test]
    fn test_resolve_alias_llama() {
        assert_eq!(
            SessionOverrides::resolve_model_name("llama").unwrap(),
            "ollama/llama3.2"
        );
    }

    #[test]
    fn test_resolve_alias_llama3() {
        assert_eq!(
            SessionOverrides::resolve_model_name("llama3").unwrap(),
            "ollama/llama3.2"
        );
    }

    #[test]
    fn test_resolve_alias_mistral() {
        assert_eq!(
            SessionOverrides::resolve_model_name("mistral").unwrap(),
            "ollama/mistral"
        );
    }

    #[test]
    fn test_resolve_alias_deepseek() {
        assert_eq!(
            SessionOverrides::resolve_model_name("deepseek").unwrap(),
            "ollama/deepseek-coder"
        );
    }

    #[test]
    fn test_resolve_alias_qwen() {
        assert_eq!(
            SessionOverrides::resolve_model_name("qwen").unwrap(),
            "ollama/qwen2.5"
        );
    }

    #[test]
    fn test_resolve_alias_codellama() {
        assert_eq!(
            SessionOverrides::resolve_model_name("codellama").unwrap(),
            "ollama/codellama"
        );
    }

    #[test]
    fn test_resolve_alias_phi() {
        assert_eq!(
            SessionOverrides::resolve_model_name("phi").unwrap(),
            "ollama/phi3"
        );
    }

    #[test]
    fn test_resolve_alias_gemma() {
        assert_eq!(
            SessionOverrides::resolve_model_name("gemma").unwrap(),
            "ollama/gemma2"
        );
    }

    // --- Case-insensitive alias ---

    #[test]
    fn test_resolve_alias_case_insensitive() {
        assert_eq!(
            SessionOverrides::resolve_model_name("OpUs").unwrap(),
            "claude-opus-4-6-20250918"
        );
        assert_eq!(
            SessionOverrides::resolve_model_name("SONNET").unwrap(),
            "claude-sonnet-4-5-20250929"
        );
        assert_eq!(
            SessionOverrides::resolve_model_name("Haiku").unwrap(),
            "claude-haiku-4-5-20251001"
        );
    }

    // --- Full model ID passthrough ---

    #[test]
    fn test_resolve_full_model_id_passthrough() {
        assert_eq!(
            SessionOverrides::resolve_model_name("claude-sonnet-4-5-20250929").unwrap(),
            "claude-sonnet-4-5-20250929"
        );
    }

    #[test]
    fn test_resolve_full_model_id_case_insensitive() {
        assert_eq!(
            SessionOverrides::resolve_model_name("Claude-Sonnet-4-5-20250929").unwrap(),
            "claude-sonnet-4-5-20250929"
        );
    }

    // --- ollama/ prefix passthrough ---

    #[test]
    fn test_resolve_ollama_prefix_passthrough() {
        assert_eq!(
            SessionOverrides::resolve_model_name("ollama/my-custom-model").unwrap(),
            "ollama/my-custom-model"
        );
    }

    // --- Unknown model rejection ---

    #[test]
    fn test_resolve_unknown_model_error() {
        let result = SessionOverrides::resolve_model_name("nonexistent-model-xyz");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Unknown model"));
        assert!(err.contains("nonexistent-model-xyz"));
    }

    // --- effective_model ---

    #[test]
    fn test_effective_model_returns_override_when_set() {
        let mut overrides = SessionOverrides::default();
        overrides.model = Some("gpt-4o".to_string());
        assert_eq!(
            overrides.effective_model("claude-sonnet-4-5-20250929"),
            "gpt-4o"
        );
    }

    #[test]
    fn test_effective_model_returns_config_default_when_no_override() {
        let overrides = SessionOverrides::default();
        assert_eq!(
            overrides.effective_model("claude-sonnet-4-5-20250929"),
            "claude-sonnet-4-5-20250929"
        );
    }

    // --- reset ---

    #[test]
    fn test_reset_clears_all_overrides() {
        let mut overrides = SessionOverrides {
            model: Some("gpt-4o".to_string()),
            thinking_budget: Some(50_000),
            verbose: true,
            provider: Some(LlmProvider::OpenAI),
            sandbox: Some(SandboxOverrides {
                memory_limit: Some("256m".to_string()),
                cpu_limit: Some(0.5),
                timeout_seconds: Some(30),
                additional_blocked_paths: None,
            }),
        };
        overrides.reset();
        assert!(overrides.model.is_none());
        assert!(overrides.thinking_budget.is_none());
        assert!(!overrides.verbose);
        assert!(overrides.provider.is_none());
        assert!(overrides.sandbox.is_none());
    }

    #[test]
    fn test_sandbox_overrides_tighten_only() {
        use crate::sandbox::SandboxConfig;

        let base = SandboxConfig {
            enabled: true,
            image: "debian:bookworm-slim@sha256:98f4b71de414932".to_string(),
            non_root_user: "sandbox".to_string(),
            memory_limit: "512m".to_string(),
            cpu_limit: 1.0,
            network_mode: "none".to_string(),
            timeout_seconds: 60,
            blocked_paths: vec!["/etc".to_string()],
            seccomp_profile: None,
            apparmor_profile: None,
            fallback: crate::sandbox::SandboxFallback::default(),
        };

        // Tighter values should apply
        let overrides = SandboxOverrides {
            memory_limit: Some("256m".to_string()),
            cpu_limit: Some(0.5),
            timeout_seconds: Some(30),
            additional_blocked_paths: Some(vec!["/home".to_string()]),
        };
        let effective = overrides.apply_to(base.clone());
        assert_eq!(effective.memory_limit, "256m");
        assert_eq!(effective.cpu_limit, 0.5);
        assert_eq!(effective.timeout_seconds, 30);
        assert!(effective.blocked_paths.contains(&"/home".to_string()));

        // Looser values should NOT apply
        let loose = SandboxOverrides {
            memory_limit: Some("1g".to_string()),
            cpu_limit: Some(2.0),
            timeout_seconds: Some(120),
            additional_blocked_paths: None,
        };
        let effective = loose.apply_to(base);
        assert_eq!(effective.memory_limit, "512m");
        assert_eq!(effective.cpu_limit, 1.0);
        assert_eq!(effective.timeout_seconds, 60);
    }

    #[test]
    fn test_parse_memory() {
        assert_eq!(super::parse_memory("512m"), Some(512 * 1024 * 1024));
        assert_eq!(super::parse_memory("1g"), Some(1024 * 1024 * 1024));
        assert_eq!(super::parse_memory("256k"), Some(256 * 1024));
        assert_eq!(super::parse_memory("1024"), Some(1024));
        assert_eq!(super::parse_memory("bad"), None);
    }
}
