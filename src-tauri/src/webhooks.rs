//! Webhook HTTP server for receiving external triggers.
//!
//! Runs a lightweight axum server that can trigger scheduled tasks
//! or send messages to Claude when receiving webhook requests.
//! Also hosts WhatsApp Cloud API webhook routes when enabled.
#![allow(dead_code)]

use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Path, State as AxumState},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::claude::ClaudeClient;
use crate::commands::AppState;
use crate::config::{NexiBotConfig, WebhookAction};
use crate::rate_limiter::RateLimiter;
use crate::scheduler::Scheduler;
use crate::security::external_content;
use crate::session_overrides::SessionOverrides;
use crate::webhook_dedup::{DedupConfig, WebhookDeduplicator};
use crate::webhook_rate_limit::{WebhookRateLimitConfig, WebhookRateLimiter};

/// Shared state for the webhook server.
pub struct WebhookState {
    pub config: Arc<RwLock<NexiBotConfig>>,
    pub scheduler: Arc<Scheduler>,
    pub claude_client: Arc<RwLock<ClaudeClient>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub webhook_rate_limiter: Arc<WebhookRateLimiter>,
    pub deduplicator: Arc<WebhookDeduplicator>,
    pub agent_control: Arc<crate::agent_control::AgentControl>,
    pub key_interceptor: Option<crate::security::key_interceptor::KeyInterceptor>,
}

/// Webhook request body.
#[derive(Debug, Deserialize)]
pub struct WebhookPayload {
    pub body: Option<String>,
}

/// Start the webhook HTTP server on the configured port.
pub async fn start_webhook_server(
    config: Arc<RwLock<NexiBotConfig>>,
    scheduler: Arc<Scheduler>,
    claude_client: Arc<RwLock<ClaudeClient>>,
    app_state: Option<AppState>,
) -> Result<(), String> {
    let (port, webhooks_enabled, tls_config, rate_limit_config) = {
        let cfg = config.read().await;
        let webhooks_enabled = cfg.webhooks.enabled;
        let wa_enabled = cfg.whatsapp.enabled;
        let sl_enabled = cfg.slack.enabled;
        let tm_enabled = cfg.teams.enabled;
        let gc_enabled = cfg.google_chat.enabled;
        let messenger_enabled = cfg.messenger.enabled;
        let instagram_enabled = cfg.instagram.enabled;
        let line_enabled = cfg.line.enabled;
        let twilio_enabled = cfg.twilio.enabled;
        let webchat_enabled = cfg.webchat.enabled;

        // If no webhook-backed channel is enabled, skip server entirely.
        if !webhooks_enabled
            && !wa_enabled
            && !sl_enabled
            && !tm_enabled
            && !gc_enabled
            && !messenger_enabled
            && !instagram_enabled
            && !line_enabled
            && !twilio_enabled
            && !webchat_enabled
        {
            info!("[WEBHOOK] Webhook server disabled (no webhook-backed channels enabled)");
            return Ok(());
        }

        (
            cfg.webhooks.port,
            webhooks_enabled,
            cfg.webhooks.tls.clone(),
            cfg.webhooks.rate_limit.clone(),
        )
    };

    let rate_limiter = Arc::new(RateLimiter::new(rate_limit_config));

    // Spawn periodic rate limiter cleanup
    let cleanup_limiter = rate_limiter.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            cleanup_limiter.cleanup_stale().await;
        }
    });

    // Initialize webhook deduplicator
    let dedup_config = DedupConfig::default();
    let deduplicator = Arc::new(WebhookDeduplicator::new(dedup_config));

    // Spawn periodic deduplicator cleanup
    let cleanup_dedup = deduplicator.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            cleanup_dedup.cleanup_stale().await;
        }
    });

    // Initialize webhook rate limiter (token bucket, multi-scope)
    let webhook_rate_config = WebhookRateLimitConfig::default();
    let webhook_rate_limiter = Arc::new(WebhookRateLimiter::new(webhook_rate_config));

    // Spawn periodic webhook rate limiter cleanup
    let cleanup_webhook_rl = webhook_rate_limiter.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(300)).await; // Cleanup every 5 min
            cleanup_webhook_rl.cleanup_stale().await;
        }
    });

    // Extract agent_control from app_state if available, otherwise create new instance
    let agent_control = if let Some(ref app_st) = app_state {
        app_st.agent_control.clone()
    } else {
        Arc::new(crate::agent_control::AgentControl::new())
    };
    let key_interceptor = app_state
        .as_ref()
        .map(|app_st| app_st.key_interceptor.clone());

    let state = Arc::new(WebhookState {
        config: config.clone(),
        scheduler,
        claude_client,
        rate_limiter,
        webhook_rate_limiter,
        deduplicator,
        agent_control,
        key_interceptor,
    });

    let mut app = Router::new()
        .route("/webhook/health", get(health_handler))
        .route("/webhook/{endpoint_id}", post(webhook_handler))
        .with_state(state)
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024)) // 10 MB max body
        .layer(middleware::from_fn(security_headers_middleware));

    // Optional REST API surface for mobile/headless clients.
    if webhooks_enabled {
        if let Some(ref app_st) = app_state {
            app = app.merge(crate::api_server::api_router(app_st.clone()));
            info!("[WEBHOOK] API routes added under /api/*");
        } else {
            warn!("[WEBHOOK] API routes requested but no app_state provided, skipping API router");
        }
    }

    if let Some(ref app_st) = app_state {
        // Always mount channel routes so runtime config toggles do not require a restart.
        // Each handler enforces its own `enabled` and credential checks.
        let wa_state = Arc::new(crate::whatsapp::WhatsAppState::new(app_st.clone()));
        let wa_cleanup = wa_state.clone();
        tokio::spawn(crate::whatsapp::session_cleanup_loop(wa_cleanup));
        let wa_router = Router::new()
            .route(
                "/whatsapp/webhook",
                get(crate::whatsapp::whatsapp_verify_handler),
            )
            .route(
                "/whatsapp/webhook",
                post(crate::whatsapp::whatsapp_webhook_handler),
            )
            .with_state(wa_state);
        app = app.merge(wa_router);
        info!("[WEBHOOK] WhatsApp webhook routes added");

        let slack_state = Arc::new(crate::slack::SlackState::new(app_st.clone()));
        let slack_cleanup = slack_state.clone();
        tokio::spawn(crate::slack::session_cleanup_loop(slack_cleanup));
        let slack_router = Router::new()
            .route("/slack/events", post(crate::slack::slack_events_handler))
            .with_state(slack_state);
        app = app.merge(slack_router);
        info!("[WEBHOOK] Slack events route added");

        let teams_state = Arc::new(crate::teams::TeamsBotState::new(app_st.clone()));
        let teams_cleanup = teams_state.clone();
        tokio::spawn(crate::teams::session_cleanup_loop(teams_cleanup));
        let teams_router = Router::new()
            .route(
                "/api/teams/messages",
                post(crate::teams::teams_activity_webhook_handler),
            )
            .with_state(teams_state);
        app = app.merge(teams_router);
        info!("[WEBHOOK] Teams activity route added");

        let google_chat_state = Arc::new(crate::google_chat::GoogleChatState::new(app_st.clone()));
        let google_chat_cleanup = google_chat_state.clone();
        tokio::spawn(crate::google_chat::session_cleanup_loop(
            google_chat_cleanup,
        ));
        let google_chat_router = Router::new()
            .route(
                "/api/google-chat/events",
                post(crate::google_chat::google_chat_webhook_handler),
            )
            .with_state(google_chat_state);
        app = app.merge(google_chat_router);
        info!("[WEBHOOK] Google Chat route added");

        let messenger_state = Arc::new(crate::messenger::MessengerState::new(app_st.clone()));
        let messenger_cleanup = messenger_state.clone();
        tokio::spawn(crate::messenger::session_cleanup_loop(messenger_cleanup));
        let messenger_router = Router::new()
            .route(
                "/api/messenger/webhook",
                get(crate::messenger::messenger_verify_handler),
            )
            .route(
                "/api/messenger/webhook",
                post(crate::messenger::messenger_webhook_handler),
            )
            .with_state(messenger_state);
        app = app.merge(messenger_router);
        info!("[WEBHOOK] Messenger routes added");

        let instagram_state = Arc::new(crate::instagram::InstagramState::new(app_st.clone()));
        let instagram_cleanup = instagram_state.clone();
        tokio::spawn(crate::instagram::session_cleanup_loop(instagram_cleanup));
        let instagram_router = Router::new()
            .route(
                "/api/instagram/webhook",
                get(crate::instagram::instagram_verify_handler),
            )
            .route(
                "/api/instagram/webhook",
                post(crate::instagram::instagram_webhook_handler),
            )
            .with_state(instagram_state);
        app = app.merge(instagram_router);
        info!("[WEBHOOK] Instagram routes added");

        let line_state = Arc::new(crate::line::LineState::new(app_st.clone()));
        let line_cleanup = line_state.clone();
        tokio::spawn(crate::line::session_cleanup_loop(line_cleanup));
        let line_router = Router::new()
            .route("/api/line/webhook", post(crate::line::line_webhook_handler))
            .with_state(line_state);
        app = app.merge(line_router);
        info!("[WEBHOOK] LINE routes added");

        let twilio_state = Arc::new(crate::twilio::TwilioState::new(app_st.clone()));
        let twilio_cleanup = twilio_state.clone();
        tokio::spawn(crate::twilio::session_cleanup_loop(twilio_cleanup));
        let twilio_router = Router::new()
            .route(
                "/api/twilio/webhook",
                post(crate::twilio::twilio_webhook_handler),
            )
            .with_state(twilio_state);
        app = app.merge(twilio_router);
        info!("[WEBHOOK] Twilio routes added");
    } else {
        warn!("[WEBHOOK] No app_state provided, skipping channel webhook routes");
    }

    // Keep widget route mounted so runtime config toggles do not require restart.
    let webchat_config = config.clone();
    let webchat_router = Router::new().route(
        "/webchat/widget.js",
        get(move || {
            let webchat_config = webchat_config.clone();
            async move {
                let (enabled, widget_port) = {
                    let cfg = webchat_config.read().await;
                    (cfg.webchat.enabled, cfg.webchat.port)
                };
                if !enabled {
                    return StatusCode::NOT_FOUND.into_response();
                }
                crate::webchat::widget_js_response(widget_port)
            }
        }),
    );
    app = app.merge(webchat_router);
    info!("[WEBHOOK] WebChat widget.js route added");

    let addr = format!("0.0.0.0:{}", port);

    if tls_config.enabled {
        let (cert_path, key_path) = resolve_tls_paths(&tls_config)
            .map_err(|e| format!("TLS configuration error: {}", e))?;

        info!("[WEBHOOK] Starting webhook server with TLS on {}", addr);

        let rustls_config =
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path)
                .await
                .map_err(|e| format!("Failed to load TLS certs: {}", e))?;

        let addr_parsed: SocketAddr = addr
            .parse()
            .map_err(|e| format!("Invalid address: {}", e))?;

        tokio::spawn(async move {
            if let Err(e) = axum_server::bind_rustls(addr_parsed, rustls_config)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await
            {
                warn!("[WEBHOOK] TLS webhook server error: {}", e);
            }
        });

        info!("[WEBHOOK] TLS webhook server started on {}", addr);
    } else {
        info!("[WEBHOOK] Starting webhook server on {}", addr);

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("Failed to bind webhook server to {}: {}", addr, e))?;

        tokio::spawn(async move {
            if let Err(e) = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            {
                warn!("[WEBHOOK] Webhook server error: {}", e);
            }
        });

        info!("[WEBHOOK] Webhook server started on {}", addr);
    }

    Ok(())
}

/// Middleware that adds security headers to all HTTP responses.
///
/// - `X-Content-Type-Options: nosniff` — prevents MIME sniffing attacks
/// - `Referrer-Policy: no-referrer` — prevents referrer leakage
/// - `X-Frame-Options: DENY` — prevents clickjacking via iframe embedding
/// - `Cache-Control: no-store` — prevents caching of sensitive responses
async fn security_headers_middleware(
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

/// Verify an HMAC-SHA256 webhook signature.
///
/// Compares `X-Webhook-Signature` header against HMAC(secret, body).
/// Fails closed: returns false when no signing secret is configured to
/// prevent unauthenticated requests from being processed silently.
fn verify_webhook_signature(
    headers: &HeaderMap,
    body: &[u8],
    signing_secret: Option<&str>,
) -> bool {
    let secret = match signing_secret {
        Some(s) if !s.is_empty() => s,
        _ => {
            warn!(
                "[WEBHOOKS] No signing secret configured — rejecting webhook (fail closed). \
                 Set webhooks.signing_secret or disable signature verification explicitly."
            );
            return false;
        }
    };

    let signature_header = match headers
        .get("x-webhook-signature")
        .and_then(|v| v.to_str().ok())
    {
        Some(sig) => sig,
        None => {
            warn!("[WEBHOOK] Missing X-Webhook-Signature header, rejecting");
            return false;
        }
    };

    // Strip optional "sha256=" prefix
    let provided_sig = signature_header
        .strip_prefix("sha256=")
        .unwrap_or(signature_header);

    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            warn!("[WEBHOOK] Invalid HMAC key length");
            return false;
        }
    };
    mac.update(body);

    let expected = hex::encode(mac.finalize().into_bytes());

    // Constant-time comparison to prevent timing attacks
    crate::security::constant_time::secure_compare(&expected, provided_sig)
}

/// Health check endpoint.
async fn health_handler() -> Json<serde_json::Value> {
    // Check bridge service connectivity
    let bridge_healthy = reqwest::Client::new()
        .get("http://127.0.0.1:18790/health")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .is_ok();

    // Check Ollama connectivity — validate URL against SSRF policy before use
    let ollama_url =
        std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let ollama_healthy = {
        let allowed = ollama_url == "http://localhost:11434"
            || ollama_url.starts_with("http://localhost:")
            || ollama_url.starts_with("http://127.0.0.1:");
        if allowed {
            reqwest::Client::new()
                .get(format!("{}/api/tags", ollama_url))
                .timeout(std::time::Duration::from_secs(2))
                .send()
                .await
                .is_ok()
        } else {
            match crate::security::ssrf::validate_outbound_request(
                &ollama_url,
                &crate::security::ssrf::SsrfPolicy::default(),
                &[],
            ) {
                Ok(()) => reqwest::Client::new()
                    .get(format!("{}/api/tags", ollama_url))
                    .timeout(std::time::Duration::from_secs(2))
                    .send()
                    .await
                    .is_ok(),
                Err(_) => false,
            }
        }
    };

    let overall = if bridge_healthy {
        "healthy"
    } else {
        "degraded"
    };

    Json(json!({
        "status": overall,
        "service": "nexibot",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "components": {
            "bridge": if bridge_healthy { "up" } else { "down" },
            "ollama": if ollama_healthy { "up" } else { "unavailable" },
        }
    }))
}

/// Resolve TLS certificate paths, auto-generating if configured.
fn resolve_tls_paths(
    tls_config: &crate::config::TlsConfig,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let home = dirs::home_dir().ok_or("Failed to get home directory")?;
    let tls_dir = home.join(".config/nexibot/tls");

    let cert_path = tls_config
        .cert_path
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| tls_dir.join("cert.pem"));

    let key_path = tls_config
        .key_path
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| tls_dir.join("key.pem"));

    // Auto-generate if enabled and files are missing
    if tls_config.auto_generate && (!cert_path.exists() || !key_path.exists()) {
        info!("[WEBHOOK] Auto-generating self-signed TLS certificate...");
        std::fs::create_dir_all(&tls_dir)
            .map_err(|e| format!("Failed to create TLS dir: {}", e))?;

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .map_err(|e| format!("Failed to generate self-signed cert: {}", e))?;

        let cert_pem = cert.cert.pem();
        let key_pem = cert.key_pair.serialize_pem();

        std::fs::write(&cert_path, &cert_pem)
            .map_err(|e| format!("Failed to write cert: {}", e))?;
        std::fs::write(&key_path, &key_pem).map_err(|e| format!("Failed to write key: {}", e))?;

        // Set restrictive permissions (cross-platform)
        let _ = crate::platform::file_security::restrict_file_permissions(&cert_path);
        let _ = crate::platform::file_security::restrict_file_permissions(&key_path);

        info!(
            "[WEBHOOK] Self-signed TLS certificate generated at {:?}",
            tls_dir
        );
    }

    if !cert_path.exists() || !key_path.exists() {
        return Err(format!(
            "TLS cert or key not found at {:?} / {:?}",
            cert_path, key_path
        ));
    }

    Ok((cert_path, key_path))
}

/// Main webhook handler.
async fn webhook_handler(
    AxumState(state): AxumState<Arc<WebhookState>>,
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    Path(endpoint_id): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // KILLSWITCH CHECK: Verify agent is running before processing webhooks
    match state.agent_control.get_state() {
        crate::agent_control::AgentState::Stopped => {
            tracing::warn!("[KILLSWITCH] Webhook rejected: agent is stopped");
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Agent is stopped (killswitch activated)"})),
            ));
        }
        crate::agent_control::AgentState::Paused => {
            tracing::warn!("[KILLSWITCH] Webhook rejected: agent is paused");
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Agent is paused (webhooks queued)"})),
            ));
        }
        crate::agent_control::AgentState::Running => {
            // Continue normal processing
        }
    }

    // Rate limiting check
    if state.rate_limiter.is_blocked(client_addr.ip()).await {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": "Too many failed attempts. Try again later."})),
        ));
    }

    let config = state.config.read().await;

    if !config.webhooks.enabled {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Webhook endpoints are disabled"})),
        ));
    }

    let expected_token_raw = config.webhooks.auth_token.as_deref().unwrap_or("").trim();
    let expected_token_owned = state
        .key_interceptor
        .as_ref()
        .map(|interceptor| interceptor.restore_config_string(expected_token_raw))
        .unwrap_or_else(|| expected_token_raw.to_string());
    let expected_token = expected_token_owned.trim();
    if expected_token.is_empty() {
        warn!("[WEBHOOK] Rejecting request: webhooks.auth_token is not configured");
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Webhook auth token is not configured"})),
        ));
    }

    // Validate bearer token (fail closed).
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let provided_token = auth_header.strip_prefix("Bearer ").unwrap_or("");
    if !crate::security::constant_time::secure_compare(provided_token, expected_token) {
        // Record auth failure for rate limiting
        let blocked = state.rate_limiter.record_failure(client_addr.ip()).await;
        if blocked {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error": "Too many failed attempts. Try again later."})),
            ));
        }
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Invalid or missing bearer token"})),
        ));
    }

    // Find endpoint
    let endpoint = config
        .webhooks
        .endpoints
        .iter()
        .find(|e| e.id == endpoint_id)
        .cloned();

    let endpoint = match endpoint {
        Some(e) => e,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("Endpoint not found: {}", endpoint_id)})),
            ));
        }
    };

    drop(config);

    // Webhook deduplication check (Stage 1: Event ID, Stage 2: Content hash)
    // Run after auth + endpoint resolution so unauthenticated callers cannot
    // poison dedup state or trigger duplicate short-circuit responses.
    let channel = "generic"; // Reserved scope for generic /webhook endpoints.
    let user_id = payload
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or(&endpoint_id);
    let event_id = payload.get("event_id").and_then(|v| v.as_str());
    let content = payload.get("body").and_then(|v| v.as_str()).unwrap_or("");

    match state
        .deduplicator
        .check_and_record(channel, user_id, event_id, content)
        .await
    {
        Ok(true) => {
            // Not a duplicate, continue processing
        }
        Ok(false) => {
            // Duplicate detected, silently return 200 OK to prevent webhook retries
            tracing::debug!(
                "[WEBHOOK] Duplicate event filtered, endpoint={}",
                endpoint_id
            );
            return Ok(Json(json!({"status": "ok", "note": "duplicate filtered"})));
        }
        Err(e) => {
            // Dedup check failed, log but continue (fail-open)
            tracing::warn!("[WEBHOOK] Dedup check failed: {}, continuing anyway", e);
        }
    }

    // Webhook per-scope rate limiting check (global, per-user, per-channel, per-IP)
    let rate_limit_result = state
        .webhook_rate_limiter
        .check_limit(user_id, channel, client_addr.ip())
        .await;

    if !rate_limit_result.allowed {
        let reason = rate_limit_result
            .limit_exceeded
            .unwrap_or_else(|| "unknown".to_string());
        tracing::warn!(
            "[WEBHOOK] Rate limit exceeded: {}, user={}, channel={}, ip={}",
            reason,
            user_id,
            channel,
            client_addr.ip()
        );
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": format!("Rate limit exceeded: {}", reason),
                "global_remaining": rate_limit_result.global_remaining,
                "user_remaining": rate_limit_result.user_remaining,
                "channel_remaining": rate_limit_result.channel_remaining,
                "ip_remaining": rate_limit_result.ip_remaining,
            })),
        ));
    }

    info!(
        "[WEBHOOK] Triggered endpoint '{}' ({})",
        endpoint.name, endpoint_id
    );

    let body_text = payload
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Wrap webhook payload with external content boundary markers to prevent prompt injection
    let body_text = external_content::wrap_external_content(&body_text, "webhook");

    match endpoint.action {
        WebhookAction::TriggerTask => match state.scheduler.execute_task(&endpoint.target).await {
            Ok(result) => Ok(Json(json!({
                "status": "ok",
                "task_name": result.task_name,
                "success": result.success,
                "response": result.response,
            }))),
            Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e})))),
        },
        WebhookAction::SendMessage => {
            let prompt = endpoint.target.replace("{{body}}", &body_text);
            let claude = state.claude_client.read().await;
            let default_overrides = SessionOverrides::default();

            match claude
                .send_message_with_tools(&prompt, &[], &default_overrides)
                .await
            {
                Ok(result) => Ok(Json(json!({
                    "status": "ok",
                    "response": result.text,
                }))),
                Err(e) => Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("{}", e)})),
                )),
            }
        }
    }
}
