//! Built-in nexibot_fetch tool — HTTP fetch with safety checks and HTML-to-text conversion.

use serde_json::{json, Value};
use std::time::Duration;
use tracing::{info, warn};

use crate::config::FetchConfig;
use crate::security::external_content;
use crate::security::ssrf::{self, SsrfPolicy};

/// Get the tool definition to pass to Claude.
pub fn nexibot_fetch_tool_definition() -> Value {
    json!({
        "name": "nexibot_fetch",
        "description": "Fetch content from a URL. HTML pages are automatically converted to readable text. JSON responses are pretty-printed. Supports GET, POST, PUT, DELETE methods. Blocks requests to localhost and cloud metadata endpoints for security.",
        "input_schema": {
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "method": {
                    "type": "string",
                    "enum": ["get", "post", "put", "delete", "head"],
                    "description": "HTTP method (default: get)"
                },
                "headers": {
                    "type": "object",
                    "description": "Optional HTTP headers as key-value pairs",
                    "additionalProperties": { "type": "string" }
                },
                "body": {
                    "type": "string",
                    "description": "Optional request body (for POST/PUT)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Request timeout in milliseconds (default: 30000)"
                }
            },
            "required": ["url"]
        }
    })
}

/// Execute the fetch tool.
pub async fn execute_fetch_tool(input: &Value, config: &FetchConfig) -> String {
    if !config.enabled {
        return "Error: Fetch tool is disabled in settings. Enable it under fetch.enabled in config.yaml.".to_string();
    }

    let url_str = match input.get("url").and_then(|u| u.as_str()) {
        Some(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => return "Error: 'url' is required".to_string(),
    };

    // SSRF protection: validate URL scheme, hostname, private IPs, blocked domains, DNS
    let ssrf_policy = SsrfPolicy::default();
    let parsed_url =
        match ssrf::validate_outbound_request(&url_str, &ssrf_policy, &config.blocked_domains) {
            Ok(u) => u,
            Err(e) => return format!("Error: {}", e),
        };

    // Check allowed domains (if configured).
    // Each entry must contain at least one dot so bare TLDs (e.g. "com", "io")
    // cannot accidentally allowlist every host under that TLD.
    if !config.allowed_domains.is_empty() {
        let host = parsed_url.host_str().unwrap_or("");
        let host_lower = host.to_lowercase();
        let allowed = config.allowed_domains.iter().any(|d| {
            let d_lower = d.to_lowercase();
            // Reject bare TLD entries — they must contain at least one dot.
            if !d_lower.contains('.') {
                return false;
            }
            host_lower == d_lower || host_lower.ends_with(&format!(".{}", d_lower))
        });
        if !allowed {
            return format!(
                "Error: Domain '{}' is not in the allowed domains list.",
                host
            );
        }
    }

    let method_str = input
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("get")
        .to_lowercase();

    let timeout = input
        .get("timeout_ms")
        .and_then(|t| t.as_u64())
        .unwrap_or(config.default_timeout_ms);

    info!("[FETCH] {} {}", method_str.to_uppercase(), url_str);

    // Custom redirect policy: re-validate every redirect target through the SSRF
    // guard to prevent bypass via open-redirect chains (e.g. initial URL is public
    // but redirects to 169.254.169.254/latest/meta-data/).
    let redirect_ssrf_policy = ssrf_policy.clone();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.stop();
            }
            match ssrf::validate_redirect(attempt.url(), &redirect_ssrf_policy) {
                Ok(()) => attempt.follow(),
                Err(e) => attempt.error(e),
            }
        }))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut request = match method_str.as_str() {
        "get" => client.get(&url_str),
        "post" => client.post(&url_str),
        "put" => client.put(&url_str),
        "delete" => client.delete(&url_str),
        "head" => client.head(&url_str),
        _ => {
            return format!(
                "Error: Unsupported method '{}'. Use get, post, put, delete, or head.",
                method_str
            )
        }
    };

    // Add custom headers
    if let Some(headers) = input.get("headers").and_then(|h| h.as_object()) {
        for (key, value) in headers {
            if let Some(v) = value.as_str() {
                if let (Ok(name), Ok(val)) = (
                    reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                    reqwest::header::HeaderValue::from_str(v),
                ) {
                    request = request.header(name, val);
                }
            }
        }
    }

    // Add body
    if let Some(body) = input.get("body").and_then(|b| b.as_str()) {
        request = request.body(body.to_string());
    }

    // Execute request
    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("[FETCH] Request failed: {}", e);
            if e.is_timeout() {
                return format!("Error: Request timed out after {}ms", timeout);
            }
            return format!("Error: Request failed: {}", e);
        }
    };

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    // Read body with size limit
    let body_bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => return format!("Error: Failed to read response body: {}", e),
    };

    let truncated = body_bytes.len() > config.max_response_bytes;
    let limited_bytes = if truncated {
        &body_bytes[..config.max_response_bytes]
    } else {
        &body_bytes[..]
    };

    let body_text = String::from_utf8_lossy(limited_bytes).to_string();

    // Convert HTML to text, or pretty-print JSON
    let processed_body = if content_type.contains("text/html") {
        let converted = html2text::from_read(body_text.as_bytes(), 100);
        if truncated {
            format!(
                "{}\n\n[Response truncated at {} bytes]",
                converted, config.max_response_bytes
            )
        } else {
            converted
        }
    } else if content_type.contains("application/json") {
        // Try to pretty-print JSON
        match serde_json::from_str::<Value>(&body_text) {
            Ok(v) => {
                let pretty = serde_json::to_string_pretty(&v).unwrap_or(body_text.clone());
                if truncated {
                    format!(
                        "{}\n\n[Response truncated at {} bytes]",
                        pretty, config.max_response_bytes
                    )
                } else {
                    pretty
                }
            }
            Err(_) => body_text,
        }
    } else if truncated {
        format!(
            "{}\n\n[Response truncated at {} bytes]",
            body_text, config.max_response_bytes
        )
    } else {
        body_text
    };

    // Wrap fetched content with external content boundary markers
    let wrapped_body = external_content::wrap_web_content(&url_str, &processed_body);

    json!({
        "status": status,
        "content_type": content_type,
        "body": wrapped_body,
    })
    .to_string()
}
