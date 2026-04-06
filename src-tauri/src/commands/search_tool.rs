//! Built-in nexibot_web_search tool — web search without requiring MCP.
//!
//! Priority-based provider selection:
//! 1. Brave Search API (if brave_api_key configured)
//! 2. Tavily API (if tavily_api_key configured)
//! 3. Browser-based DuckDuckGo (zero config, uses existing BrowserManager)

use serde_json::{json, Value};
use tracing::{info, warn};

use crate::browser::BrowserManager;
use crate::config::NexiBotConfig;

/// Search result from any provider.
#[derive(Debug, Clone)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Get the tool definition to pass to Claude.
pub fn nexibot_web_search_tool_definition() -> Value {
    json!({
        "name": "nexibot_web_search",
        "description": "Search the web for information. Returns titles, URLs, and snippets from web results. Works without any MCP servers — uses Brave, Tavily, or DuckDuckGo depending on configuration.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (default: 5, max: 20)"
                }
            },
            "required": ["query"]
        }
    })
}

/// Execute the web search tool. Requires read access to config (lock 1) and browser (lock 7).
pub async fn execute_web_search_tool(
    input: &Value,
    config: &NexiBotConfig,
    browser: &BrowserManager,
) -> String {
    let query = match input.get("query").and_then(|q| q.as_str()) {
        Some(q) if !q.trim().is_empty() => q.to_string(),
        _ => return "Error: 'query' is required".to_string(),
    };

    let num_results = input
        .get("num_results")
        .and_then(|n| n.as_u64())
        .map(|n| n.min(20) as u32)
        .unwrap_or(config.search.default_result_count);

    info!(
        "[WEB_SEARCH] Searching for: {} (num_results: {})",
        query, num_results
    );

    // Try providers in priority order
    for provider in &config.search.search_priority {
        match provider.as_str() {
            "brave" => {
                if let Some(ref api_key) = config.search.brave_api_key {
                    // Try up to 2 times with a delay for rate limiting (free tier: 1 req/sec)
                    let mut brave_result = search_brave(&query, num_results, api_key).await;
                    if brave_result.is_err() {
                        info!("[WEB_SEARCH] Brave rate limited, retrying after 1.5s");
                        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                        brave_result = search_brave(&query, num_results, api_key).await;
                    }
                    match brave_result {
                        Ok(results) => return format_results(&query, &results, "Brave"),
                        Err(e) => {
                            warn!(
                                "[WEB_SEARCH] Brave search failed: {}, trying next provider",
                                e
                            );
                            continue;
                        }
                    }
                }
            }
            "tavily" => {
                if let Some(ref api_key) = config.search.tavily_api_key {
                    match search_tavily(&query, num_results, api_key).await {
                        Ok(results) => return format_results(&query, &results, "Tavily"),
                        Err(e) => {
                            warn!(
                                "[WEB_SEARCH] Tavily search failed: {}, trying next provider",
                                e
                            );
                            continue;
                        }
                    }
                }
            }
            "browser" => {
                match search_duckduckgo(&query, num_results, browser, config.browser.enabled).await
                {
                    Ok(results) => return format_results(&query, &results, "DuckDuckGo"),
                    Err(e) => {
                        warn!("[WEB_SEARCH] DuckDuckGo browser search failed: {}", e);
                        continue;
                    }
                }
            }
            other => {
                warn!("[WEB_SEARCH] Unknown search provider: {}", other);
            }
        }
    }

    "Error: All search providers failed. Check your configuration or ensure a browser is available for DuckDuckGo fallback.".to_string()
}

/// Search using Brave Search API.
async fn search_brave(
    query: &str,
    num_results: u32,
    api_key: &str,
) -> Result<Vec<SearchResult>, String> {
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", api_key)
        .header("Accept", "application/json")
        .query(&[("q", query), ("count", &num_results.to_string())])
        .send()
        .await
        .map_err(|e| format!("Brave API request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Brave API returned status {}", resp.status()));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Brave response: {}", e))?;

    let results = body
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .take(num_results as usize)
                .map(|r| SearchResult {
                    title: r
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string(),
                    url: r
                        .get("url")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string(),
                    snippet: r
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(results)
}

/// Search using Tavily API.
async fn search_tavily(
    query: &str,
    num_results: u32,
    api_key: &str,
) -> Result<Vec<SearchResult>, String> {
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .post("https://api.tavily.com/search")
        .json(&json!({
            "api_key": api_key,
            "query": query,
            "max_results": num_results,
        }))
        .send()
        .await
        .map_err(|e| format!("Tavily API request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Tavily API returned status {}", resp.status()));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Tavily response: {}", e))?;

    let results = body
        .get("results")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .take(num_results as usize)
                .map(|r| SearchResult {
                    title: r
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string(),
                    url: r
                        .get("url")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string(),
                    snippet: r
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(results)
}

/// Zero-config web search via bridge proxy to DuckDuckGo.
/// Routes through the Node.js bridge to avoid TLS fingerprint-based CAPTCHA blocking.
async fn search_duckduckgo(
    query: &str,
    num_results: u32,
    _browser: &BrowserManager,
    _config_browser_enabled: bool,
) -> Result<Vec<SearchResult>, String> {
    let bridge_url = std::env::var("ANTHROPIC_BRIDGE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:18790".to_string());
    crate::security::ssrf::validate_loopback_url(&bridge_url)
        .map_err(|e| format!("ANTHROPIC_BRIDGE_URL rejected: {}", e))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let mut search_req = client
        .post(format!("{}/api/search", bridge_url))
        .json(&serde_json::json!({
            "query": query,
            "num_results": num_results,
        }));
    if let Some(secret) = crate::bridge::get_bridge_secret() {
        search_req = search_req.header("x-bridge-secret", secret);
    }
    let resp = search_req
        .send()
        .await
        .map_err(|e| format!("Bridge search request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Bridge search returned {}: {}", status, body));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse bridge search response: {}", e))?;

    let results: Vec<SearchResult> = body
        .get("results")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .take(num_results as usize)
                .map(|r| SearchResult {
                    title: r
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string(),
                    url: r
                        .get("url")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string(),
                    snippet: r
                        .get("snippet")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    if results.is_empty() {
        info!(
            "[WEB_SEARCH] DuckDuckGo (via bridge) returned no results for: {}",
            query
        );
    } else {
        info!(
            "[WEB_SEARCH] DuckDuckGo (via bridge) returned {} results",
            results.len()
        );
    }

    Ok(results)
}

/// Format search results for Claude.
fn format_results(query: &str, results: &[SearchResult], provider: &str) -> String {
    if results.is_empty() {
        return format!("No results found for \"{}\" (via {})", query, provider);
    }

    let results_json: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "title": r.title,
                "url": r.url,
                "snippet": r.snippet,
            })
        })
        .collect();

    json!({
        "query": query,
        "provider": provider,
        "results": results_json,
        "total": results.len(),
    })
    .to_string()
}
