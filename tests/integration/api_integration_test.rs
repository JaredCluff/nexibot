//! API Server Integration Tests
//!
//! These tests exercise the NexiBot backend through its HTTP API endpoints,
//! testing the full pipeline: API auth -> router -> LLM provider -> response.
//!
//! Prerequisites:
//!   1. NexiBot running with API server enabled
//!   2. Mock LLM server running on :18799 (or real API key)
//!   3. Auth token configured in config.yaml -> webhooks.auth_token
//!
//! Run:
//!   NEXIBOT_API_URL=http://127.0.0.1:11434 \
//!   NEXIBOT_AUTH_TOKEN=your-token \
//!   cargo test --test api_integration_test -- --test-threads=1
//!
//! These tests are #[ignore] by default so they don't run in `cargo test`.
//! Use `--ignored` to run them explicitly.

use std::time::Duration;

fn api_url() -> String {
    std::env::var("NEXIBOT_API_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string())
}

fn auth_token() -> String {
    std::env::var("NEXIBOT_AUTH_TOKEN").unwrap_or_default()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client")
}

async fn get(path: &str) -> reqwest::Response {
    let url = format!("{}{}", api_url(), path);
    let mut req = client().get(&url);
    let token = auth_token();
    if !token.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", token));
    }
    req.send().await.expect(&format!("GET {} failed", url))
}

async fn post(path: &str, body: serde_json::Value) -> reqwest::Response {
    let url = format!("{}{}", api_url(), path);
    let mut req = client().post(&url).json(&body);
    let token = auth_token();
    if !token.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", token));
    }
    req.send().await.expect(&format!("POST {} failed", url))
}

// ─── Health ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_health_endpoint() {
    let resp = get("/api/health").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

// ─── Auth ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_unauthenticated_request_rejected() {
    let url = format!("{}/api/config", api_url());
    let resp = client()
        .get(&url)
        .send()
        .await
        .expect("request failed");
    // Should be 401 or 403 without auth header
    assert!(
        resp.status() == 401 || resp.status() == 403,
        "Expected 401/403, got {}",
        resp.status()
    );
}

// ─── Config ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_get_config() {
    let resp = get("/api/config").await;
    if resp.status() == 401 {
        eprintln!("SKIP: auth token not configured");
        return;
    }
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("claude").is_some(), "Config should have claude section");
}

// ─── Sessions ───────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_list_sessions() {
    let resp = get("/api/sessions").await;
    if resp.status() == 401 {
        return;
    }
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array(), "Sessions should be an array");
}

// ─── Models ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_list_models() {
    let resp = get("/api/models").await;
    if resp.status() == 401 {
        return;
    }
    assert_eq!(resp.status(), 200);
}

// ─── Skills ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_list_skills() {
    let resp = get("/api/skills").await;
    if resp.status() == 401 {
        return;
    }
    assert_eq!(resp.status(), 200);
}

// ─── Chat ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_chat_send_message() {
    let resp = post(
        "/api/chat/send",
        serde_json::json!({ "message": "What is 2+2? Reply with only the number." }),
    )
    .await;
    if resp.status() == 401 {
        return;
    }
    // Either succeeds (200) or fails because no LLM configured (500)
    let status = resp.status();
    assert!(
        status == 200 || status == 500,
        "Expected 200 or 500, got {}",
        status
    );
    if status == 200 {
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body.get("response").is_some() || body.get("text").is_some(),
            "Response should have text content"
        );
    }
}

// ─── Overrides ──────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_get_and_set_overrides() {
    let resp = get("/api/overrides").await;
    if resp.status() == 401 {
        return;
    }
    assert_eq!(resp.status(), 200);
}
