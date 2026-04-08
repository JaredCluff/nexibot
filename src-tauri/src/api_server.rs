//! HTTP API server for headless/mobile access.
//!
//! Provides REST endpoints for remote clients (React Native mobile app,
//! Docker headless mode, etc.). Designed to be mounted alongside the
//! existing webhook routes on the Axum server.
//!
//! Authentication: Bearer token from `config.webhooks.auth_token`.

use axum::{
    extract::Request,
    extract::{Json as AxumJson, State as AxumState},
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::commands::AppState;
use crate::session_overrides::SessionOverrides;

/// Build the API router to be nested under `/api` on the webhook server.
pub fn api_router(state: AppState) -> Router {
    let shared_state = Arc::new(state);
    Router::new()
        .route("/api/chat/send", post(api_chat_send))
        .route("/api/config", get(api_get_config))
        .route("/api/config", put(api_update_config))
        .route("/api/sessions", get(api_list_sessions))
        .route("/api/models", get(api_get_models))
        .route("/api/skills", get(api_list_skills))
        .route("/api/overrides", get(api_get_overrides))
        .route("/api/overrides", put(api_set_overrides))
        .route("/api/health", get(api_health))
        .with_state(shared_state.clone())
        .layer(middleware::from_fn_with_state(
            shared_state,
            api_auth_middleware,
        ))
}

fn api_path_is_public(path: &str) -> bool {
    path == "/api/health"
}

/// API auth middleware: require `Authorization: Bearer <webhooks.auth_token>`.
async fn api_auth_middleware(
    AxumState(state): AxumState<Arc<AppState>>,
    request: Request,
    next: Next,
) -> axum::response::Response {
    // Keep health checks unauthenticated for liveness probes and CLI reachability checks.
    if api_path_is_public(request.uri().path()) {
        return next.run(request).await;
    }

    let expected_token = {
        let config = state.config.read().await;
        let token = config.webhooks.auth_token.as_deref().unwrap_or("");
        state
            .key_interceptor
            .restore_config_string(token)
            .trim()
            .to_string()
    };

    if expected_token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "API auth token is not configured").into_response();
    }

    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let provided_token = auth_header.strip_prefix("Bearer ").unwrap_or("").trim();

    if !crate::security::constant_time::secure_compare(provided_token, &expected_token) {
        return (StatusCode::UNAUTHORIZED, "Invalid or missing bearer token").into_response();
    }

    next.run(request).await
}

/// Chat send request body.
#[derive(Debug, Deserialize)]
struct ChatSendRequest {
    message: String,
}

/// Chat send response.
#[derive(Debug, Serialize)]
struct ChatSendResponse {
    response: String,
    model: String,
}

/// Send a message to Claude and get a response (non-streaming).
async fn api_chat_send(
    AxumState(state): AxumState<Arc<AppState>>,
    AxumJson(body): AxumJson<ChatSendRequest>,
) -> Result<AxumJson<ChatSendResponse>, (StatusCode, String)> {
    let overrides = state.session_overrides.read().await.clone();
    let claude = state.claude_client.read().await;

    let config = state.config.read().await;
    let effective_model = overrides.effective_model(&config.claude.model).to_string();
    drop(config);

    match claude
        .send_message_with_tools(&body.message, &[], &overrides)
        .await
    {
        Ok(result) => Ok(AxumJson(ChatSendResponse {
            response: result.text,
            model: effective_model,
        })),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// Get current config (sanitized — no API keys).
async fn api_get_config(AxumState(state): AxumState<Arc<AppState>>) -> AxumJson<Value> {
    let config = state.config.read().await;
    AxumJson(json!({
        "claude": {
            "model": config.claude.model,
            "max_tokens": config.claude.max_tokens,
        },
        "ollama": {
            "enabled": config.ollama.enabled,
            "url": config.ollama.url,
            "model": config.ollama.model,
        },
        "telegram": { "enabled": config.telegram.enabled },
        "whatsapp": { "enabled": config.whatsapp.enabled },
        "webhooks": { "enabled": config.webhooks.enabled, "port": config.webhooks.port },
    }))
}

/// Update config fields.
async fn api_update_config(
    AxumState(state): AxumState<Arc<AppState>>,
    AxumJson(body): AxumJson<Value>,
) -> Result<AxumJson<Value>, (StatusCode, String)> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();

    // Apply model change if provided
    if let Some(model) = body.get("model").and_then(|v| v.as_str()) {
        config.claude.model = model.to_string();
    }

    // Apply Ollama changes if provided
    if let Some(ollama) = body.get("ollama") {
        if let Some(enabled) = ollama.get("enabled").and_then(|v| v.as_bool()) {
            config.ollama.enabled = enabled;
        }
        if let Some(url) = ollama.get("url").and_then(|v| v.as_str()) {
            if let Err(e) = crate::security::ssrf::validate_outbound_request(
                url,
                &crate::security::ssrf::SsrfPolicy::default(),
                &[],
            ) {
                return Err((StatusCode::BAD_REQUEST, format!("Invalid Ollama URL: {}", e)));
            }
            config.ollama.url = url.to_string();
        }
        if let Some(model) = ollama.get("model").and_then(|v| v.as_str()) {
            config.ollama.model = model.to_string();
        }
    }

    if let Err(e) = config.save() {
        *config = previous_config;
        return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }
    drop(config);
    let _ = state.config_changed.send(());

    Ok(AxumJson(json!({ "status": "updated" })))
}

/// List named sessions.
async fn api_list_sessions(AxumState(state): AxumState<Arc<AppState>>) -> AxumJson<Value> {
    let mgr = state.session_manager.read().await;
    let sessions = mgr.list_sessions();
    AxumJson(json!({ "sessions": sessions }))
}

/// Get available models from all providers.
async fn api_get_models(AxumState(state): AxumState<Arc<AppState>>) -> AxumJson<Value> {
    // Return configured models info
    let config = state.config.read().await;
    AxumJson(json!({
        "default_model": config.claude.model,
        "ollama_enabled": config.ollama.enabled,
        "ollama_model": config.ollama.model,
    }))
}

/// List installed skills.
async fn api_list_skills(AxumState(state): AxumState<Arc<AppState>>) -> AxumJson<Value> {
    let skills = state.skills_manager.read().await;
    let skill_list: Vec<_> = skills
        .list_skills()
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "name": s.metadata.name,
                "description": s.metadata.description,
                "source": s.metadata.source,
            })
        })
        .collect();
    AxumJson(json!({ "skills": skill_list }))
}

/// Get current session overrides.
async fn api_get_overrides(
    AxumState(state): AxumState<Arc<AppState>>,
) -> AxumJson<SessionOverrides> {
    let overrides = state.session_overrides.read().await;
    AxumJson(overrides.clone())
}

/// Set session overrides.
async fn api_set_overrides(
    AxumState(state): AxumState<Arc<AppState>>,
    AxumJson(body): AxumJson<Value>,
) -> Result<AxumJson<SessionOverrides>, (StatusCode, String)> {
    let mut overrides = state.session_overrides.write().await;

    if let Some(model) = body.get("model").and_then(|v| v.as_str()) {
        let resolved = SessionOverrides::resolve_model_name(model)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        overrides.model = Some(resolved);
    }

    if let Some(thinking) = body.get("thinking_budget") {
        if thinking.is_null() {
            overrides.thinking_budget = None;
        } else if let Some(budget) = thinking.as_u64() {
            overrides.thinking_budget = Some(budget as usize);
        }
    }

    if let Some(verbose) = body.get("verbose").and_then(|v| v.as_bool()) {
        overrides.verbose = verbose;
    }

    Ok(AxumJson(overrides.clone()))
}

/// API health check.
async fn api_health() -> AxumJson<Value> {
    AxumJson(json!({
        "status": "healthy",
        "service": "nexibot-api",
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── api_health tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_health_returns_correct_shape() {
        let AxumJson(body) = api_health().await;

        assert_eq!(body.get("status").and_then(|v| v.as_str()), Some("healthy"));
        assert_eq!(
            body.get("service").and_then(|v| v.as_str()),
            Some("nexibot-api")
        );
        assert!(
            body.get("timestamp").and_then(|v| v.as_str()).is_some(),
            "Response should contain a timestamp field"
        );
    }

    #[tokio::test]
    async fn test_health_timestamp_is_rfc3339() {
        let AxumJson(body) = api_health().await;
        let ts = body.get("timestamp").and_then(|v| v.as_str()).unwrap();
        // Verify it parses as a valid RFC 3339 / ISO 8601 timestamp
        let parsed = chrono::DateTime::parse_from_rfc3339(ts);
        assert!(
            parsed.is_ok(),
            "Timestamp '{}' should be valid RFC 3339",
            ts
        );
    }

    #[tokio::test]
    async fn test_health_timestamp_is_recent() {
        let before = chrono::Utc::now();
        let AxumJson(body) = api_health().await;
        let after = chrono::Utc::now();

        let ts = body.get("timestamp").and_then(|v| v.as_str()).unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(ts).unwrap();
        let parsed_utc = parsed.with_timezone(&chrono::Utc);

        assert!(
            parsed_utc >= before && parsed_utc <= after,
            "Timestamp should be between before ({}) and after ({}), got {}",
            before,
            after,
            parsed_utc
        );
    }

    #[tokio::test]
    async fn test_health_has_exactly_three_fields() {
        let AxumJson(body) = api_health().await;
        let obj = body
            .as_object()
            .expect("Health response should be a JSON object");
        assert_eq!(
            obj.len(),
            3,
            "Health response should have exactly 3 fields (status, service, timestamp), got {:?}",
            obj.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_api_path_is_public_only_for_health() {
        assert!(api_path_is_public("/api/health"));
        assert!(!api_path_is_public("/api/chat/send"));
        assert!(!api_path_is_public("/api/config"));
    }

    // ── ChatSendRequest / ChatSendResponse serde tests ───────────────

    #[test]
    fn test_chat_send_request_deserialize() {
        let json_str = r#"{"message": "Hello, Claude!"}"#;
        let req: ChatSendRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.message, "Hello, Claude!");
    }

    #[test]
    fn test_chat_send_response_serialize() {
        let resp = ChatSendResponse {
            response: "I am fine.".to_string(),
            model: "claude-sonnet-4-5-20250929".to_string(),
        };
        let json_val = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            json_val.get("response").and_then(|v| v.as_str()),
            Some("I am fine.")
        );
        assert_eq!(
            json_val.get("model").and_then(|v| v.as_str()),
            Some("claude-sonnet-4-5-20250929")
        );
    }

    #[test]
    fn test_chat_send_request_missing_message_fails() {
        let json_str = r#"{"not_message": "oops"}"#;
        let result: Result<ChatSendRequest, _> = serde_json::from_str(json_str);
        assert!(
            result.is_err(),
            "Deserializing without 'message' field should fail"
        );
    }

    #[test]
    fn test_chat_send_response_round_trip() {
        let resp = ChatSendResponse {
            response: "Test response with \"quotes\" and \nnewlines".to_string(),
            model: "test-model".to_string(),
        };
        let serialized = serde_json::to_string(&resp).unwrap();
        let deserialized: Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            deserialized.get("response").and_then(|v| v.as_str()),
            Some("Test response with \"quotes\" and \nnewlines")
        );
    }

    // NOTE: Testing api_router(), api_chat_send(), and other stateful handlers
    // requires constructing a full AppState with ClaudeClient, K2KIntegration,
    // VoiceService, etc. These are better covered by integration tests that can
    // spin up the full application context.
}
