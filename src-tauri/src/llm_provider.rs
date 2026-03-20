//! LLM provider abstraction for multi-provider support.
//!
//! Supports routing to Anthropic (Claude) or OpenAI (GPT-4o, o1, o2, o3) providers
//! based on model name, with capability detection for feature degradation.

use serde::{Deserialize, Serialize};

/// Supported LLM providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LlmProvider {
    Anthropic,
    OpenAI,
    Ollama,
    Google,
    DeepSeek,
    Qwen,
    GitHubCopilot,
    MiniMax,
    Cerebras,
    LMStudio,
}

impl std::fmt::Display for LlmProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmProvider::Anthropic => write!(f, "Anthropic"),
            LlmProvider::OpenAI => write!(f, "OpenAI"),
            LlmProvider::Ollama => write!(f, "Ollama"),
            LlmProvider::Google => write!(f, "Google"),
            LlmProvider::DeepSeek => write!(f, "DeepSeek"),
            LlmProvider::Qwen => write!(f, "Qwen"),
            LlmProvider::GitHubCopilot => write!(f, "GitHub Copilot"),
            LlmProvider::MiniMax => write!(f, "MiniMax"),
            LlmProvider::Cerebras => write!(f, "Cerebras"),
            LlmProvider::LMStudio => write!(f, "LM Studio"),
        }
    }
}

impl LlmProvider {
    /// Returns true for cloud providers that use the OpenAI-compatible chat completions API
    /// and are routed directly (not through the bridge or local HTTP).
    pub fn is_cloud_openai_compat(self) -> bool {
        matches!(
            self,
            LlmProvider::Cerebras
                | LlmProvider::DeepSeek
                | LlmProvider::GitHubCopilot
                | LlmProvider::MiniMax
                | LlmProvider::Qwen
        )
    }
}

/// Provider-specific capability flags for feature degradation.
pub struct ProviderCapabilities {
    /// Whether the provider supports extended thinking (Anthropic-only).
    pub supports_thinking: bool,
    /// Whether the provider supports Computer Use tools (Anthropic-only).
    pub supports_computer_use: bool,
    /// Whether the provider supports tool calling.
    #[allow(dead_code)]
    pub supports_tools: bool,
}

/// Common local model name prefixes for Ollama detection.
const OLLAMA_PREFIXES: &[&str] = &[
    "ollama/",
    "llama",
    "mistral",
    "codellama",
    "phi",
    "gemma",
    "vicuna",
    "neural-chat",
    "starling",
    "orca",
    "nous-hermes",
    "tinyllama",
    "dolphin",
    "wizard",
];

/// Determine the provider for a given model ID.
pub fn provider_for_model(model: &str) -> LlmProvider {
    let lower = model.to_lowercase();

    // Explicit Ollama prefix
    if lower.starts_with("ollama/") {
        return LlmProvider::Ollama;
    }

    // OpenAI models (including ChatGPT-* variants and reasoning models o1/o2/o3).
    // NOTE: o2 is an OpenAI reasoning model — it must NOT default to Anthropic.
    if lower.starts_with("gpt-")
        || lower.starts_with("gpt4")
        || lower.starts_with("chatgpt")
        || lower.starts_with("o1")
        || lower.starts_with("o2")
        || lower.starts_with("o3")
    {
        return LlmProvider::OpenAI;
    }

    // Google Gemini models
    if lower.starts_with("gemini") || lower.starts_with("google/") {
        return LlmProvider::Google;
    }

    // DeepSeek models (hosted API, not local Ollama)
    if lower.starts_with("deepseek-") || lower.starts_with("deepseek/") {
        return LlmProvider::DeepSeek;
    }

    // Qwen models (hosted API, not local Ollama)
    if lower.starts_with("qwen-") || lower.starts_with("qwen/") || lower.starts_with("qwen3") || lower.starts_with("qwen2") {
        return LlmProvider::Qwen;
    }

    // MiniMax models
    if lower.starts_with("minimax") {
        return LlmProvider::MiniMax;
    }

    // GitHub Copilot (explicit prefix)
    if lower.starts_with("github-copilot/") {
        return LlmProvider::GitHubCopilot;
    }

    // Cerebras (explicit prefix, like ollama/)
    if lower.starts_with("cerebras/") {
        return LlmProvider::Cerebras;
    }

    // LM Studio (explicit prefix)
    if lower.starts_with("lmstudio/") {
        return LlmProvider::LMStudio;
    }

    // Check for known local model names (Ollama)
    for prefix in OLLAMA_PREFIXES {
        if lower.starts_with(prefix) {
            return LlmProvider::Ollama;
        }
    }

    LlmProvider::Anthropic
}

/// Get capabilities for a given provider.
pub fn capabilities(provider: LlmProvider) -> ProviderCapabilities {
    match provider {
        LlmProvider::Anthropic => ProviderCapabilities {
            supports_thinking: true,
            supports_computer_use: true,
            supports_tools: true,
        },
        LlmProvider::OpenAI
        | LlmProvider::DeepSeek
        | LlmProvider::Qwen
        | LlmProvider::GitHubCopilot
        | LlmProvider::MiniMax
        | LlmProvider::Cerebras
        | LlmProvider::LMStudio => ProviderCapabilities {
            supports_thinking: false,
            supports_computer_use: false,
            supports_tools: true,
        },
        LlmProvider::Google => ProviderCapabilities {
            supports_thinking: false,
            supports_computer_use: false,
            supports_tools: true,
        },
        LlmProvider::Ollama => ProviderCapabilities {
            supports_thinking: false,
            supports_computer_use: false,
            supports_tools: true,
        },
    }
}

/// Validate a provider endpoint URL to prevent SSRF attacks.
///
/// Rules enforced:
/// 1. Scheme must be `http` or `https` only.
/// 2. For local providers (Ollama, `allow_local = true`), the host must be
///    `localhost`, `127.0.0.1`, or `::1`. Arbitrary hosts are rejected.
/// 3. For externally-hosted providers (`allow_local = false`), all
///    private/internal IP ranges are blocked: loopback (127.x / ::1),
///    RFC-1918 (10.x, 172.16-31.x, 192.168.x), link-local (169.254.x),
///    CGNAT (100.64-127.x), and common cloud metadata hostnames.
///
/// Returns `Ok(())` if the URL is safe, or `Err(message)` if blocked.
pub fn validate_provider_url(url: &str, allow_local: bool) -> Result<(), String> {
    let parsed = url::Url::parse(url)
        .map_err(|e| format!("Invalid provider URL '{}': {}", url, e))?;

    // Rule 1: only http/https
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(format!(
                "Provider URL '{}' uses disallowed scheme '{}' — only http/https permitted",
                url, scheme
            ));
        }
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| format!("Provider URL '{}' has no host", url))?;

    // Normalise for comparison (strip IPv6 brackets, lowercase)
    let host_lower = host.to_lowercase();
    let host_norm = host_lower.trim_matches(|c| c == '[' || c == ']');

    // Try to parse host as a literal IP address for range checks
    let ip_opt: Option<std::net::IpAddr> = host_norm.parse().ok();

    if allow_local {
        // For local providers (e.g. Ollama), only allow loopback addresses.
        let is_loopback = host_norm == "localhost"
            || host_norm == "127.0.0.1"
            || host_norm == "::1"
            || ip_opt.map(|ip| ip.is_loopback()).unwrap_or(false);

        if !is_loopback {
            return Err(format!(
                "Ollama URL '{}' must point to localhost (127.0.0.1 / ::1). \
                 Arbitrary hosts are not permitted — this prevents SSRF attacks where \
                 the Ollama URL is redirected to internal services.",
                url
            ));
        }
    } else {
        // For external providers, block all private/internal address ranges.
        if let Some(ip) = ip_opt {
            if is_private_or_internal_ip(&ip) {
                return Err(format!(
                    "Provider URL '{}' points to a private/internal IP address '{}'. \
                     This is blocked to prevent SSRF attacks.",
                    url, ip
                ));
            }
        } else {
            // Hostname-based blocklist for common internal names
            if host_norm == "localhost"
                || host_norm.ends_with(".localhost")
                || host_norm.ends_with(".local")
                || host_norm.ends_with(".internal")
                || host_norm == "metadata.google.internal"
                || host_norm == "metadata.internal"
                || host_norm == "instance-data"
            {
                return Err(format!(
                    "Provider URL '{}' points to a blocked internal hostname '{}'. \
                     This is blocked to prevent SSRF attacks.",
                    url, host
                ));
            }
        }
    }

    Ok(())
}

/// Check if an IP address falls within private/internal ranges.
fn is_private_or_internal_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            let (o1, o2) = (octets[0], octets[1]);
            // Loopback 127.0.0.0/8
            o1 == 127
            // RFC-1918 10.0.0.0/8
            || o1 == 10
            // RFC-1918 172.16.0.0/12
            || (o1 == 172 && (16..=31).contains(&o2))
            // RFC-1918 192.168.0.0/16
            || (o1 == 192 && o2 == 168)
            // Link-local 169.254.0.0/16
            || (o1 == 169 && o2 == 254)
            // Current network 0.0.0.0/8
            || o1 == 0
            // CGNAT 100.64.0.0/10
            || (o1 == 100 && (64..=127).contains(&o2))
        }
        std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_for_model_anthropic() {
        assert_eq!(
            provider_for_model("claude-sonnet-4-5-20250929"),
            LlmProvider::Anthropic
        );
        assert_eq!(
            provider_for_model("claude-opus-4-6"),
            LlmProvider::Anthropic
        );
        assert_eq!(
            provider_for_model("claude-haiku-4-5-20251001"),
            LlmProvider::Anthropic
        );
    }

    #[test]
    fn test_provider_for_model_openai() {
        assert_eq!(provider_for_model("gpt-4o"), LlmProvider::OpenAI);
        assert_eq!(provider_for_model("gpt-4o-mini"), LlmProvider::OpenAI);
        assert_eq!(provider_for_model("o1"), LlmProvider::OpenAI);
        assert_eq!(provider_for_model("o2"), LlmProvider::OpenAI);
        assert_eq!(provider_for_model("o2-mini"), LlmProvider::OpenAI);
        assert_eq!(provider_for_model("o3-mini"), LlmProvider::OpenAI);
    }

    #[test]
    fn test_o2_routes_to_openai_not_anthropic() {
        // o2 is an OpenAI reasoning model — must NOT default to Anthropic.
        assert_eq!(provider_for_model("o2"), LlmProvider::OpenAI);
        assert_ne!(provider_for_model("o2"), LlmProvider::Anthropic);
        assert_eq!(provider_for_model("o2-mini"), LlmProvider::OpenAI);
    }

    #[test]
    fn test_provider_for_model_ollama() {
        assert_eq!(provider_for_model("ollama/llama3.2"), LlmProvider::Ollama);
        assert_eq!(provider_for_model("llama3.2"), LlmProvider::Ollama);
        assert_eq!(provider_for_model("mistral"), LlmProvider::Ollama);
        assert_eq!(provider_for_model("codellama:7b"), LlmProvider::Ollama);
        assert_eq!(provider_for_model("phi3"), LlmProvider::Ollama);
        assert_eq!(provider_for_model("gemma2"), LlmProvider::Ollama);
    }

    #[test]
    fn test_provider_for_model_deepseek() {
        // deepseek-* prefix routes to hosted DeepSeek API
        assert_eq!(provider_for_model("deepseek-coder"), LlmProvider::DeepSeek);
        assert_eq!(provider_for_model("deepseek-chat"), LlmProvider::DeepSeek);
    }

    #[test]
    fn test_provider_for_model_qwen() {
        // qwen-* and qwen2/qwen3 prefixes route to hosted Qwen API
        assert_eq!(provider_for_model("qwen-3"), LlmProvider::Qwen);
        assert_eq!(provider_for_model("qwen2.5"), LlmProvider::Qwen);
        assert_eq!(provider_for_model("qwen3-72b"), LlmProvider::Qwen);
    }

    #[test]
    fn test_provider_for_model_google() {
        assert_eq!(provider_for_model("gemini-2.0-flash"), LlmProvider::Google);
        assert_eq!(provider_for_model("gemini-pro"), LlmProvider::Google);
        assert_eq!(
            provider_for_model("google/gemini-2.0-flash"),
            LlmProvider::Google
        );
    }

    #[test]
    fn test_provider_for_model_minimax() {
        assert_eq!(provider_for_model("minimax-2.5"), LlmProvider::MiniMax);
    }

    #[test]
    fn test_provider_for_model_github_copilot() {
        assert_eq!(
            provider_for_model("github-copilot/gpt-4o"),
            LlmProvider::GitHubCopilot
        );
        assert_eq!(
            provider_for_model("github-copilot/o1-mini"),
            LlmProvider::GitHubCopilot
        );
    }

    #[test]
    fn test_capabilities() {
        let anthropic = capabilities(LlmProvider::Anthropic);
        assert!(anthropic.supports_thinking);
        assert!(anthropic.supports_computer_use);
        assert!(anthropic.supports_tools);

        let openai = capabilities(LlmProvider::OpenAI);
        assert!(!openai.supports_thinking);
        assert!(!openai.supports_computer_use);
        assert!(openai.supports_tools);

        let ollama = capabilities(LlmProvider::Ollama);
        assert!(!ollama.supports_thinking);
        assert!(!ollama.supports_computer_use);
        assert!(ollama.supports_tools);
    }

    #[test]
    fn test_model_with_version_tag() {
        assert_eq!(provider_for_model("codellama:7b"), LlmProvider::Ollama);
        assert_eq!(provider_for_model("llama3.2:latest"), LlmProvider::Ollama);
    }

    #[test]
    fn test_unknown_model_defaults_anthropic() {
        assert_eq!(
            provider_for_model("some-random-model"),
            LlmProvider::Anthropic
        );
    }

    #[test]
    fn test_empty_string_defaults_anthropic() {
        assert_eq!(provider_for_model(""), LlmProvider::Anthropic);
    }

    #[test]
    fn test_o1_mini_detected_openai() {
        assert_eq!(provider_for_model("o1-mini"), LlmProvider::OpenAI);
    }

    #[test]
    fn test_all_ollama_prefixes() {
        // Test all items in OLLAMA_PREFIXES that are not already covered
        let extra_prefixes = [
            "vicuna",
            "neural-chat",
            "starling",
            "orca",
            "nous-hermes",
            "tinyllama",
            "dolphin",
            "wizard",
        ];
        for prefix in &extra_prefixes {
            assert_eq!(
                provider_for_model(prefix),
                LlmProvider::Ollama,
                "Expected Ollama for prefix '{}'",
                prefix
            );
        }
    }

    #[test]
    fn test_ollama_prefixes_with_suffix() {
        // Ensure prefix matching works with model versions/suffixes
        assert_eq!(provider_for_model("vicuna-13b"), LlmProvider::Ollama);
        assert_eq!(
            provider_for_model("neural-chat:latest"),
            LlmProvider::Ollama
        );
        assert_eq!(
            provider_for_model("dolphin-2.5-mixtral"),
            LlmProvider::Ollama
        );
        assert_eq!(provider_for_model("wizard-coder:7b"), LlmProvider::Ollama);
    }

    // -- SSRF validation tests --

    #[test]
    fn test_validate_provider_url_ollama_allows_localhost() {
        assert!(validate_provider_url("http://localhost:11434", true).is_ok());
        assert!(validate_provider_url("http://127.0.0.1:11434", true).is_ok());
        assert!(validate_provider_url("http://[::1]:11434", true).is_ok());
    }

    #[test]
    fn test_validate_provider_url_ollama_blocks_arbitrary_host() {
        assert!(validate_provider_url("http://192.168.1.100:11434", true).is_err());
        assert!(validate_provider_url("http://10.0.0.1:11434", true).is_err());
        assert!(validate_provider_url("http://evil.example.com:11434", true).is_err());
    }

    #[test]
    fn test_validate_provider_url_external_blocks_private_ip() {
        assert!(validate_provider_url("http://192.168.1.1/v1", false).is_err());
        assert!(validate_provider_url("http://10.0.0.1/api", false).is_err());
        assert!(validate_provider_url("http://169.254.169.254/metadata", false).is_err());
        assert!(validate_provider_url("http://127.0.0.1/v1", false).is_err());
    }

    #[test]
    fn test_validate_provider_url_external_blocks_internal_hostname() {
        assert!(validate_provider_url("http://localhost/v1", false).is_err());
        assert!(validate_provider_url("http://internal.local/api", false).is_err());
        assert!(validate_provider_url("http://metadata.google.internal/v1", false).is_err());
    }

    #[test]
    fn test_validate_provider_url_blocks_non_http_schemes() {
        assert!(validate_provider_url("file:///etc/passwd", false).is_err());
        assert!(validate_provider_url("ftp://api.example.com/v1", false).is_err());
        assert!(validate_provider_url("gopher://evil.com/", false).is_err());
    }

    #[test]
    fn test_validate_provider_url_allows_public_api_endpoints() {
        // Well-known public API hostnames are allowed for external providers.
        assert!(validate_provider_url("https://api.deepseek.com/v1", false).is_ok());
        assert!(validate_provider_url("https://api.qwen.ai/v1", false).is_ok());
    }
}
