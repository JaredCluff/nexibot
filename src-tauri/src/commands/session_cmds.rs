//! Session override commands (/model, /think, /verbose, /provider)

use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::info;

use crate::llm_provider::LlmProvider;
use crate::session_overrides::{SessionOverrides, DEFAULT_THINKING_BUDGET};

use super::AppState;

/// Set the session model override. Validates model name and expands shorthand.
#[tauri::command]
pub async fn set_session_model(
    state: State<'_, AppState>,
    model: String,
) -> Result<SessionOverrides, String> {
    let resolved = SessionOverrides::resolve_model_name(&model)?;

    let mut overrides = state.session_overrides.write().await;
    overrides.model = Some(resolved.clone());
    info!("[SESSION] Model override set to: {}", resolved);
    Ok(overrides.clone())
}

/// Toggle extended thinking on/off, or set a specific budget.
/// - None budget: toggles (off if currently on, on with default if currently off)
/// - Some(budget): enables with specified budget
#[tauri::command]
pub async fn toggle_thinking(
    state: State<'_, AppState>,
    budget: Option<usize>,
) -> Result<SessionOverrides, String> {
    let mut overrides = state.session_overrides.write().await;

    match budget {
        Some(b) => {
            overrides.thinking_budget = Some(b);
            info!("[SESSION] Thinking enabled with budget: {}", b);
        }
        None => {
            if overrides.thinking_budget.is_some() {
                overrides.thinking_budget = None;
                info!("[SESSION] Thinking disabled");
            } else {
                overrides.thinking_budget = Some(DEFAULT_THINKING_BUDGET);
                info!(
                    "[SESSION] Thinking enabled with default budget: {}",
                    DEFAULT_THINKING_BUDGET
                );
            }
        }
    }

    Ok(overrides.clone())
}

/// Toggle verbose mode on/off.
#[tauri::command]
pub async fn toggle_verbose(state: State<'_, AppState>) -> Result<SessionOverrides, String> {
    let mut overrides = state.session_overrides.write().await;
    overrides.verbose = !overrides.verbose;
    info!("[SESSION] Verbose mode: {}", overrides.verbose);
    Ok(overrides.clone())
}

/// Get current session overrides.
#[tauri::command]
pub async fn get_session_overrides(state: State<'_, AppState>) -> Result<SessionOverrides, String> {
    let overrides = state.session_overrides.read().await;
    Ok(overrides.clone())
}

/// Reset all session overrides to defaults.
#[tauri::command]
pub async fn reset_session_overrides(
    state: State<'_, AppState>,
) -> Result<SessionOverrides, String> {
    let mut overrides = state.session_overrides.write().await;
    overrides.reset();
    info!("[SESSION] All overrides reset");
    Ok(overrides.clone())
}

/// Set the session provider override.
#[tauri::command]
pub async fn set_session_provider(
    state: State<'_, AppState>,
    provider: String,
) -> Result<SessionOverrides, String> {
    let mut overrides = state.session_overrides.write().await;

    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => {
            overrides.provider = Some(LlmProvider::Anthropic);
            info!("[SESSION] Provider override set to: Anthropic");
        }
        "openai" | "gpt" => {
            overrides.provider = Some(LlmProvider::OpenAI);
            info!("[SESSION] Provider override set to: OpenAI");
        }
        "ollama" | "local" => {
            overrides.provider = Some(LlmProvider::Ollama);
            info!("[SESSION] Provider override set to: Ollama");
        }
        "cerebras" => {
            overrides.provider = Some(LlmProvider::Cerebras);
            info!("[SESSION] Provider override set to: Cerebras");
        }
        "auto" | "reset" => {
            overrides.provider = None;
            info!("[SESSION] Provider override cleared (auto-detect)");
        }
        _ => {
            return Err(format!(
                "Unknown provider '{}'. Valid options: claude, openai, ollama, cerebras, auto",
                provider
            ));
        }
    }

    Ok(overrides.clone())
}

/// Provider status information.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub anthropic_configured: bool,
    pub openai_configured: bool,
    pub ollama_configured: bool,
    pub ollama_url: Option<String>,
    pub cerebras_configured: bool,
    pub lmstudio_configured: bool,
    pub lmstudio_url: Option<String>,
}

/// Get which providers have API keys configured.
#[tauri::command]
pub async fn get_provider_status(state: State<'_, AppState>) -> Result<ProviderStatus, String> {
    let config = state.config.read().await;

    // Anthropic: check both OAuth and API key
    let anthropic_has_key = config
        .claude
        .api_key
        .as_ref()
        .is_some_and(|k| !k.trim().is_empty());
    let anthropic_has_oauth = crate::oauth::AuthProfileManager::load()
        .ok()
        .and_then(|mut m| m.get_default_profile("anthropic").map(|_| true))
        .unwrap_or(false);

    let openai_has_key = config
        .openai
        .api_key
        .as_ref()
        .is_some_and(|k| !k.trim().is_empty());
    let openai_has_oauth = crate::oauth::AuthProfileManager::load()
        .ok()
        .and_then(|mut m| m.get_default_profile("openai").map(|_| true))
        .unwrap_or(false);
    let ollama_url = Some(config.ollama.url.clone());

    let cerebras_has_key = config
        .cerebras
        .api_key
        .as_ref()
        .is_some_and(|k| !k.trim().is_empty());

    let lmstudio_url = Some(config.lmstudio.url.clone());

    Ok(ProviderStatus {
        anthropic_configured: anthropic_has_key || anthropic_has_oauth,
        openai_configured: openai_has_key || openai_has_oauth,
        ollama_configured: true,
        ollama_url,
        cerebras_configured: cerebras_has_key,
        lmstudio_configured: true,
        lmstudio_url,
    })
}

/// An Ollama model discovered from the local instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaModel {
    pub name: String,
    pub size: Option<u64>,
    pub modified_at: Option<String>,
}

/// Discover models installed in the local Ollama instance.
#[tauri::command]
pub async fn discover_ollama_models(
    state: State<'_, AppState>,
) -> Result<Vec<OllamaModel>, String> {
    let config = state.config.read().await;
    let url = format!("{}/api/tags", config.ollama.url);
    drop(config);

    let http = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = http
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Ollama at {}: {}", url, e))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Ollama response: {}", e))?;

    let models = body
        .get("models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let name = m.get("name")?.as_str()?.to_string();
                    let size = m.get("size").and_then(|v| v.as_u64());
                    let modified_at = m
                        .get("modified_at")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    Some(OllamaModel {
                        name,
                        size,
                        modified_at,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

/// A model available for selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableModel {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    pub available: bool,
    #[serde(default)]
    pub size_score: u64,
}

/// Bridge model response shape.
#[derive(Debug, Deserialize)]
struct BridgeModel {
    id: String,
    display_name: Option<String>,
}

/// Cache TTL for model lists (1 hour).
const MODEL_CACHE_TTL_SECS: u64 = 3600;

/// Fetch models from the bridge service, with caching.
#[tauri::command]
pub async fn get_available_models(
    state: State<'_, AppState>,
) -> Result<Vec<AvailableModel>, String> {
    // Check cache first
    {
        let cache = state.model_cache.read().await;
        if let Some((models, fetched_at)) = cache.as_ref() {
            if fetched_at.elapsed().as_secs() < MODEL_CACHE_TTL_SECS {
                return Ok(models.clone());
            }
        }
    }

    // Fetch fresh models
    let models = fetch_models_from_bridge(&state).await;

    // Update cache (even on partial success)
    if !models.is_empty() {
        let mut cache = state.model_cache.write().await;
        *cache = Some((models.clone(), std::time::Instant::now()));
    }

    Ok(models)
}

/// Force-refresh the model cache (called after config changes).
#[tauri::command]
pub async fn refresh_model_cache(
    state: State<'_, AppState>,
) -> Result<Vec<AvailableModel>, String> {
    // Invalidate cache
    {
        let mut cache = state.model_cache.write().await;
        *cache = None;
    }
    get_available_models(state).await
}

async fn fetch_models_from_bridge(state: &AppState) -> Vec<AvailableModel> {
    let bridge_url = std::env::var("ANTHROPIC_BRIDGE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:18790".to_string());
    let http = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());

    // Get API keys and Ollama config
    let config = state.config.read().await;
    let anthropic_key = get_anthropic_token(&config).await;
    let openai_key = get_openai_token(&config).await;
    let cerebras_key = config
        .cerebras
        .api_key
        .clone()
        .filter(|k| !k.trim().is_empty());
    let ollama_url = config.ollama.url.clone();
    let lmstudio_url = config.lmstudio.url.clone();
    drop(config);

    // Fetch all providers concurrently
    let (anthropic_models, openai_models, ollama_models, cerebras_models, lmstudio_models) = tokio::join!(
        fetch_provider_models(&http, &bridge_url, "/api/models", anthropic_key.as_deref()),
        fetch_provider_models(
            &http,
            &bridge_url,
            "/api/openai/models",
            openai_key.as_deref()
        ),
        fetch_ollama_models_internal(&http, &ollama_url),
        fetch_cerebras_models_internal(&http, cerebras_key.as_deref()),
        fetch_lmstudio_models_internal(&http, &lmstudio_url),
    );

    let mut models = Vec::new();

    for m in anthropic_models {
        let score = model_size_score(&m.id);
        models.push(AvailableModel {
            id: m.id.clone(),
            display_name: m.display_name.unwrap_or_else(|| m.id.clone()),
            provider: "Anthropic".to_string(),
            available: true,
            size_score: score,
        });
    }

    for m in openai_models {
        let score = model_size_score(&m.id);
        models.push(AvailableModel {
            id: m.id.clone(),
            display_name: m.display_name.unwrap_or_else(|| m.id.clone()),
            provider: "OpenAI".to_string(),
            available: true,
            size_score: score,
        });
    }

    for m in ollama_models {
        models.push(m);
    }

    for m in cerebras_models {
        models.push(m);
    }

    for m in lmstudio_models {
        models.push(m);
    }

    models
}

async fn fetch_ollama_models_internal(
    http: &reqwest::Client,
    ollama_url: &str,
) -> Vec<AvailableModel> {
    let url = format!("{}/api/tags", ollama_url);
    let resp = match http
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[MODELS] Failed to reach Ollama at {}: {}", url, e);
            return Vec::new();
        }
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("[MODELS] Failed to parse Ollama response: {}", e);
            return Vec::new();
        }
    };

    body.get("models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let name = m.get("name")?.as_str()?;
                    let ollama_id = format!("ollama/{}", name);
                    Some(AvailableModel {
                        id: ollama_id.clone(),
                        display_name: name.to_string(),
                        provider: "Ollama".to_string(),
                        available: true,
                        size_score: model_size_score(&ollama_id),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Fetch models from Cerebras by calling their OpenAI-compatible /v1/models endpoint.
/// Returns models prefixed with "cerebras/" so the provider router can identify them.
async fn fetch_cerebras_models_internal(
    http: &reqwest::Client,
    api_key: Option<&str>,
) -> Vec<AvailableModel> {
    let Some(key) = api_key else {
        return Vec::new();
    };

    let url = "https://api.cerebras.ai/v1/models";
    let resp = match http
        .get(url)
        .header("Authorization", format!("Bearer {}", key))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[MODELS] Failed to reach Cerebras at {}: {}", url, e);
            return Vec::new();
        }
    };

    let status = resp.status();
    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("[MODELS] Failed to parse Cerebras models response: {}", e);
            return Vec::new();
        }
    };

    if !status.is_success() {
        let msg = body.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error");
        tracing::warn!("[MODELS] Cerebras API returned {}: {}", status, msg);
        return Vec::new();
    }

    body.get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id")?.as_str()?;
                    let prefixed_id = format!("cerebras/{}", id);
                    Some(AvailableModel {
                        display_name: id.to_string(),
                        id: prefixed_id.clone(),
                        provider: "Cerebras".to_string(),
                        available: true,
                        size_score: model_size_score(&prefixed_id),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Fetch models from LM Studio by calling its OpenAI-compatible /v1/models endpoint.
/// Returns models prefixed with "lmstudio/" so the provider router can identify them.
async fn fetch_lmstudio_models_internal(
    http: &reqwest::Client,
    lmstudio_url: &str,
) -> Vec<AvailableModel> {
    let url = format!("{}/v1/models", lmstudio_url);
    let resp = match http
        .get(&url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => {
            // LM Studio not running — not an error, just no models
            return Vec::new();
        }
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };

    body.get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id")?.as_str()?;
                    let prefixed_id = format!("lmstudio/{}", id);
                    Some(AvailableModel {
                        display_name: id.to_string(),
                        id: prefixed_id.clone(),
                        provider: "LM Studio".to_string(),
                        available: true,
                        size_score: model_size_score(&prefixed_id),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse parameter counts from model names to rank by size.
fn model_size_score(model_id: &str) -> u64 {
    let lower = model_id.to_lowercase();
    let mut max_score: u64 = 0;
    for part in lower.split(|c: char| !c.is_alphanumeric() && c != '.') {
        if let Some(num_str) = part.strip_suffix('b') {
            if let Ok(n) = num_str.parse::<f64>() {
                max_score = max_score.max((n * 1_000_000_000.0) as u64);
            }
        }
    }
    // Fallback heuristics for models without explicit param count
    if max_score == 0 {
        if lower.contains("gpt-oss") {
            max_score = 120_000_000_000;
        } else if lower.contains("opus") {
            max_score = 200_000_000_000;
        } else if lower.contains("sonnet") {
            max_score = 100_000_000_000;
        } else if lower.contains("haiku") {
            max_score = 20_000_000_000;
        } else if lower.contains("gpt-4o-mini") {
            max_score = 8_000_000_000;
        } else if lower.contains("gpt-4o") {
            max_score = 200_000_000_000;
        } else if lower.contains("o3-mini") {
            max_score = 50_000_000_000;
        } else if lower.contains("o1") {
            max_score = 200_000_000_000;
        }
    }
    max_score
}

/// Send a minimal completion request to verify a model actually works with the given API key.
async fn probe_model_works(
    http: &reqwest::Client,
    provider: &str,
    model_id: &str,
    api_key: &str,
) -> bool {
    let (url, body) = match provider {
        "Cerebras" => (
            "https://api.cerebras.ai/v1/chat/completions".to_string(),
            serde_json::json!({
                "model": model_id.strip_prefix("cerebras/").unwrap_or(model_id),
                "messages": [{"role": "user", "content": "hi"}],
                "max_tokens": 1
            }),
        ),
        _ => return true,
    };

    match http
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
    {
        Ok(r) => {
            let status = r.status().as_u16();
            // 200 = works, 429 = rate/quota limited but model exists,
            // 402 = payment required but model exists.
            // 401/403/404 = model doesn't work with this key.
            matches!(status, 200 | 429 | 402)
        }
        Err(_) => false,
    }
}

/// Validate which models actually work for a given provider by probing each one.
/// Returns only validated models, sorted by size_score descending.
#[tauri::command]
pub async fn validate_provider_models(
    state: State<'_, AppState>,
    provider: String,
) -> Result<Vec<AvailableModel>, String> {
    let all_models = get_available_models(state.clone()).await?;
    let provider_models: Vec<AvailableModel> = all_models
        .into_iter()
        .filter(|m| m.provider == provider)
        .collect();

    if provider_models.is_empty() {
        return Ok(Vec::new());
    }

    // Get API key/token for the provider
    let api_key = {
        let config = state.config.read().await;
        match provider.as_str() {
            "Cerebras" => config.cerebras.api_key.clone().filter(|k| !k.trim().is_empty()),
            _ => return Ok(provider_models),
        }
    };

    let Some(key) = api_key else {
        return Ok(Vec::new());
    };

    let http = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let mut validated = Vec::new();

    for model in provider_models {
        if probe_model_works(&http, &provider, &model.id, &key).await {
            validated.push(model);
        } else {
            tracing::info!(
                "[MODELS] Probe failed for {} — excluding from validated list",
                model.id
            );
        }
    }

    // Sort by size_score descending (largest first)
    validated.sort_by(|a, b| b.size_score.cmp(&a.size_score));
    Ok(validated)
}

/// Get the Anthropic auth token (OAuth first, then API key).
async fn get_anthropic_token(config: &crate::config::NexiBotConfig) -> Option<String> {
    // Try OAuth first
    if let Ok(mut manager) = crate::oauth::AuthProfileManager::load() {
        if let Some(profile) = manager.get_default_profile("anthropic") {
            if let Ok(token) = profile.get_valid_token().await {
                if let Err(e) = manager.save() {
                    tracing::warn!(
                        "[MODELS] Failed to persist refreshed Anthropic OAuth profile: {}",
                        e
                    );
                }
                return Some(token);
            }
        }
    }
    // Fall back to API key
    config
        .claude
        .api_key
        .clone()
        .filter(|k| !k.trim().is_empty())
}

/// Get the OpenAI auth token (OAuth first, then API key).
async fn get_openai_token(config: &crate::config::NexiBotConfig) -> Option<String> {
    // Try OAuth first (device code flow tokens)
    if let Ok(mut manager) = crate::oauth::AuthProfileManager::load() {
        if let Some(profile) = manager.get_default_profile("openai") {
            if let Ok(token) = profile.get_valid_token().await {
                let _ = manager.save();
                return Some(token);
            }
        }
    }
    // Fall back to API key
    config
        .openai
        .api_key
        .clone()
        .filter(|k| !k.trim().is_empty())
}

async fn fetch_provider_models(
    http: &reqwest::Client,
    bridge_url: &str,
    path: &str,
    api_key: Option<&str>,
) -> Vec<BridgeModel> {
    let Some(key) = api_key else {
        return Vec::new();
    };

    let url = format!("{}{}", bridge_url, path);

    let mut models_req = http
        .get(&url)
        .header("x-api-key", key)
        .timeout(std::time::Duration::from_secs(10));
    if let Some(secret) = crate::bridge::get_bridge_secret() {
        models_req = models_req.header("x-bridge-secret", secret);
    }
    match models_req
        .send()
        .await
    {
        Ok(resp) => match resp.json::<Vec<BridgeModel>>().await {
            Ok(models) => models,
            Err(e) => {
                tracing::warn!("[MODELS] Failed to parse response from {}: {}", path, e);
                Vec::new()
            }
        },
        Err(e) => {
            tracing::warn!("[MODELS] Failed to fetch from {}: {}", path, e);
            Vec::new()
        }
    }
}
