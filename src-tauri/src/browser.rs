//! Browser CDP tool — structured web browsing via Chrome DevTools Protocol

use anyhow::{Context, Result};
use base64::Engine;
use chromiumoxide::browser::{Browser, BrowserConfig as CdpBrowserConfig};
use chromiumoxide::page::Page;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::config::BrowserConfig;
use crate::security::ssrf::{self, SsrfPolicy};

/// Maximum size in bytes for extracted page content (100KB).
const MAX_PAGE_CONTENT_BYTES: usize = 100 * 1024;

/// Read-only browser actions that never need confirmation.
const READ_ONLY_ACTIONS: &[&str] = &[
    "screenshot",
    "get_text",
    "get_url",
    "wait_for_selector",
    "scroll",
];

pub struct BrowserManager {
    pub enabled: bool,
    pub config: BrowserConfig,
    browser: Arc<RwLock<Option<Browser>>>,
    active_page: Arc<RwLock<Option<Page>>>,
}

impl BrowserManager {
    pub fn new(config: BrowserConfig) -> Self {
        let enabled = config.enabled;
        Self {
            enabled,
            config,
            browser: Arc::new(RwLock::new(None)),
            active_page: Arc::new(RwLock::new(None)),
        }
    }

    /// Update config fields that can change at runtime without restarting the browser.
    pub fn update_config(&mut self, new_config: BrowserConfig) {
        self.enabled = new_config.enabled;
        self.config = new_config;
    }

    /// Returns the Anthropic tool definition for the browser tool
    pub fn get_tool_definition(&self) -> serde_json::Value {
        serde_json::json!({
            "name": "browser",
            "description": "Control a web browser to navigate pages, click elements, type text, take screenshots, and extract content.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["navigate", "screenshot", "screenshot_to_base64", "click", "type", "evaluate_js", "wait_for_selector", "get_text", "get_page_content", "scroll", "get_url", "back", "forward", "close"],
                        "description": "The browser action to perform"
                    },
                    "url": { "type": "string", "description": "URL to navigate to (for 'navigate' action)" },
                    "selector": { "type": "string", "description": "CSS selector for element interaction" },
                    "text": { "type": "string", "description": "Text to type (for 'type' action)" },
                    "script": { "type": "string", "description": "JavaScript to evaluate (for 'evaluate_js' action)" },
                    "delta_x": { "type": "integer", "description": "Horizontal scroll amount in pixels" },
                    "delta_y": { "type": "integer", "description": "Vertical scroll amount in pixels" },
                    "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds for this operation" }
                },
                "required": ["action"]
            }
        })
    }

    /// Check if a tool name is the browser tool
    pub fn is_browser_tool(name: &str) -> bool {
        name == "browser"
    }

    /// Check if a URL's domain is in the allowed list.
    fn is_domain_allowed(&self, url: &str) -> bool {
        if self.config.allowed_domains.is_empty() {
            return true;
        }
        if let Ok(parsed) = url::Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                return self
                    .config
                    .allowed_domains
                    .iter()
                    .any(|d| host == d.as_str() || host.ends_with(&format!(".{}", d)));
            }
        }
        false
    }

    /// Execute a browser tool call
    pub async fn execute(&self, input: &serde_json::Value) -> Result<serde_json::Value> {
        let action = input["action"].as_str().context("Missing 'action' field")?;

        let _is_read_only = READ_ONLY_ACTIONS.contains(&action);

        // Domain allowlist check for navigation
        if action == "navigate" {
            if let Some(url_str) = input["url"].as_str() {
                if !self.is_domain_allowed(url_str) {
                    anyhow::bail!("BLOCKED: domain not in allowed list");
                }

                // SSRF validation: block navigation to private/internal IPs
                if let Ok(parsed_url) = url::Url::parse(url_str) {
                    let policy = SsrfPolicy::default();
                    if let Err(ssrf_err) = ssrf::validate_url(&parsed_url, &policy) {
                        warn!(
                            "[BROWSER] SSRF blocked navigation to {}: {}",
                            url_str, ssrf_err
                        );
                        anyhow::bail!("BLOCKED: {}", ssrf_err);
                    }
                    debug!("[BROWSER] SSRF validation passed for {}", url_str);
                }
            }
        }

        // NOTE: confirmation is handled by guardrails/autonomous mode in chat.rs
        // before execute() is called. The require_confirmation config is checked
        // there, not here, to avoid double-blocking.

        match action {
            "navigate" => {
                let url = input["url"]
                    .as_str()
                    .context("Missing 'url' for navigate")?;
                let timeout_ms = input["timeout_ms"]
                    .as_u64()
                    .unwrap_or(self.config.default_timeout_ms);

                self.ensure_browser().await?;
                self.ensure_page().await?;

                let page_lock = self.active_page.clone();
                let url_owned = url.to_string();

                let result = self
                    .with_timeout(Duration::from_millis(timeout_ms), async move {
                        let page_guard = page_lock.read().await;
                        let page = page_guard.as_ref().context("No active page")?;
                        page.goto(&url_owned).await.context("Navigation failed")?;
                        page.wait_for_navigation().await.ok();

                        let current_url =
                            page.url().await?.map(|u| u.to_string()).unwrap_or_default();
                        let title = page.get_title().await?.unwrap_or_default();

                        Ok(serde_json::json!({
                            "success": true,
                            "url": current_url,
                            "title": title
                        }))
                    })
                    .await?;

                Ok(result)
            }

            "screenshot" => {
                let page_guard = self.active_page.read().await;
                let page = page_guard
                    .as_ref()
                    .context("No active page — navigate first")?;

                let screenshot_bytes = page
                    .screenshot(
                        chromiumoxide::page::ScreenshotParams::builder()
                            .full_page(false)
                            .build(),
                    )
                    .await
                    .context("Screenshot failed")?;

                let b64 = base64::engine::general_purpose::STANDARD.encode(&screenshot_bytes);

                Ok(serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": b64
                    }
                }))
            }

            "click" => {
                let selector = input["selector"]
                    .as_str()
                    .context("Missing 'selector' for click")?;

                let page_guard = self.active_page.read().await;
                let page = page_guard.as_ref().context("No active page")?;

                let element = page
                    .find_element(selector)
                    .await
                    .context("Element not found")?;
                element.click().await.context("Click failed")?;

                Ok(serde_json::json!({ "success": true, "action": "click", "selector": selector }))
            }

            "type" => {
                let selector = input["selector"]
                    .as_str()
                    .context("Missing 'selector' for type")?;
                let text = input["text"].as_str().context("Missing 'text' for type")?;

                let page_guard = self.active_page.read().await;
                let page = page_guard.as_ref().context("No active page")?;

                let element = page
                    .find_element(selector)
                    .await
                    .context("Element not found")?;
                element.type_str(text).await.context("Type failed")?;

                Ok(serde_json::json!({ "success": true, "action": "type", "selector": selector }))
            }

            "evaluate_js" => {
                let script = input["script"]
                    .as_str()
                    .context("Missing 'script' for evaluate_js")?;

                let page_guard = self.active_page.read().await;
                let page = page_guard.as_ref().context("No active page")?;

                let result: serde_json::Value = page
                    .evaluate(script)
                    .await
                    .context("JavaScript evaluation failed")?
                    .into_value()
                    .context("Failed to convert JS result")?;

                Ok(serde_json::json!({ "success": true, "result": result }))
            }

            "wait_for_selector" => {
                let selector = input["selector"]
                    .as_str()
                    .context("Missing 'selector' for wait_for_selector")?;

                let page_guard = self.active_page.read().await;
                let page = page_guard.as_ref().context("No active page")?;

                page.find_element(selector)
                    .await
                    .context("Element not found within timeout")?;

                Ok(
                    serde_json::json!({ "success": true, "action": "wait_for_selector", "selector": selector }),
                )
            }

            "get_text" => {
                let selector = input["selector"]
                    .as_str()
                    .context("Missing 'selector' for get_text")?;

                let page_guard = self.active_page.read().await;
                let page = page_guard.as_ref().context("No active page")?;

                let element = page
                    .find_element(selector)
                    .await
                    .context("Element not found")?;
                let text = element
                    .inner_text()
                    .await
                    .context("Failed to get text")?
                    .unwrap_or_default();

                Ok(serde_json::json!({ "success": true, "text": text }))
            }

            "scroll" => {
                let delta_x = input["delta_x"].as_i64().unwrap_or(0);
                let delta_y = input["delta_y"].as_i64().unwrap_or(0);

                let page_guard = self.active_page.read().await;
                let page = page_guard.as_ref().context("No active page")?;

                let script = format!("window.scrollBy({}, {})", delta_x, delta_y);
                page.evaluate(script).await.context("Scroll failed")?;

                Ok(
                    serde_json::json!({ "success": true, "action": "scroll", "delta_x": delta_x, "delta_y": delta_y }),
                )
            }

            "get_url" => {
                let page_guard = self.active_page.read().await;
                let page = page_guard.as_ref().context("No active page")?;

                let url = page.url().await?.map(|u| u.to_string()).unwrap_or_default();
                let title = page.get_title().await?.unwrap_or_default();

                Ok(serde_json::json!({ "url": url, "title": title }))
            }

            "back" => {
                let page_guard = self.active_page.read().await;
                let page = page_guard.as_ref().context("No active page")?;

                page.evaluate("window.history.back()")
                    .await
                    .context("Back navigation failed")?;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                Ok(serde_json::json!({ "success": true, "action": "back" }))
            }

            "forward" => {
                let page_guard = self.active_page.read().await;
                let page = page_guard.as_ref().context("No active page")?;

                page.evaluate("window.history.forward()")
                    .await
                    .context("Forward navigation failed")?;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                Ok(serde_json::json!({ "success": true, "action": "forward" }))
            }

            "screenshot_to_base64" => {
                let b64 = self.screenshot_to_base64().await?;
                Ok(serde_json::json!({
                    "success": true,
                    "base64": b64
                }))
            }

            "get_page_content" => {
                let page_guard = self.active_page.read().await;
                let page = page_guard
                    .as_ref()
                    .context("No active page — navigate first")?;

                // Extract text content from the page body via JavaScript
                let script = r#"
                    (function() {
                        var body = document.body;
                        if (!body) return '';
                        return body.innerText || body.textContent || '';
                    })()
                "#;

                let result: serde_json::Value = page
                    .evaluate(script)
                    .await
                    .context("Failed to extract page content")?
                    .into_value()
                    .context("Failed to convert page content result")?;

                let mut content = result.as_str().unwrap_or("").to_string();

                // Enforce size limit (UTF-8 safe — find nearest char boundary)
                let truncated = if content.len() > MAX_PAGE_CONTENT_BYTES {
                    let mut safe_end = MAX_PAGE_CONTENT_BYTES;
                    while safe_end > 0 && !content.is_char_boundary(safe_end) {
                        safe_end -= 1;
                    }
                    content.truncate(safe_end);
                    // Try to truncate at a word boundary
                    if let Some(last_space) = content.rfind(' ') {
                        if last_space > safe_end.saturating_sub(200) {
                            content.truncate(last_space);
                        }
                    }
                    content.push_str("\n... [content truncated at 100KB]");
                    true
                } else {
                    false
                };

                let url = page.url().await?.map(|u| u.to_string()).unwrap_or_default();
                let title = page.get_title().await?.unwrap_or_default();

                debug!(
                    "[BROWSER] Extracted page content: {} bytes, truncated={}",
                    content.len(),
                    truncated
                );

                Ok(serde_json::json!({
                    "success": true,
                    "url": url,
                    "title": title,
                    "content": content,
                    "content_length": content.len(),
                    "truncated": truncated
                }))
            }

            "close" => {
                // Drop page and browser
                {
                    let mut page_guard = self.active_page.write().await;
                    *page_guard = None;
                }
                {
                    let mut browser_guard = self.browser.write().await;
                    *browser_guard = None;
                }
                info!("[BROWSER] Browser closed");
                Ok(serde_json::json!({ "success": true, "action": "close" }))
            }

            _ => {
                anyhow::bail!("Unknown browser action: {}", action);
            }
        }
    }

    /// Take a screenshot of the active page and return it as a base64-encoded PNG string.
    ///
    /// Returns an error if no page is active.
    pub async fn screenshot_to_base64(&self) -> Result<String> {
        let page_guard = self.active_page.read().await;
        let page = page_guard
            .as_ref()
            .context("No active page — navigate first")?;

        let screenshot_bytes = page
            .screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .full_page(false)
                    .build(),
            )
            .await
            .context("Screenshot failed")?;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&screenshot_bytes);
        debug!("[BROWSER] Screenshot captured: {} bytes base64", b64.len());

        Ok(b64)
    }

    /// Execute an async operation with a timeout.
    ///
    /// Wraps the given future with a tokio timeout. If the timeout expires,
    /// returns an error describing the timeout. Uses the config's
    /// `default_timeout_ms` when no explicit duration is provided.
    async fn with_timeout<F, T>(&self, timeout: Duration, future: F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        match tokio::time::timeout(timeout, future).await {
            Ok(result) => result,
            Err(_) => {
                warn!("[BROWSER] Operation timed out after {:?}", timeout);
                anyhow::bail!(
                    "Browser operation timed out after {} ms",
                    timeout.as_millis()
                )
            }
        }
    }

    /// Launch Chrome if not already running
    async fn ensure_browser(&self) -> Result<()> {
        let has_browser = self.browser.read().await.is_some();
        if has_browser {
            return Ok(());
        }

        info!(
            "[BROWSER] Launching browser (headless: {})",
            self.config.headless
        );

        let mut builder = CdpBrowserConfig::builder();

        if self.config.headless {
            builder = builder.arg("--headless=new");
        }

        builder = builder
            .arg("--disable-gpu")
            .arg("--no-sandbox")
            .arg(format!(
                "--window-size={},{}",
                self.config.viewport_width, self.config.viewport_height
            ));

        // Use explicit chrome_path from config, or auto-detect
        let chrome_path = self.config.chrome_path.clone().or_else(|| {
            #[cfg(target_os = "macos")]
            {
                let candidates = [
                    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                    "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
                    "/Applications/Chromium.app/Contents/MacOS/Chromium",
                    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
                    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
                ];
                for path in &candidates {
                    if std::path::Path::new(path).exists() {
                        info!("[BROWSER] Auto-detected Chrome at: {}", path);
                        return Some(path.to_string());
                    }
                }
            }
            #[cfg(target_os = "windows")]
            {
                let program_files = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".to_string());
                let program_files_x86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".to_string());
                let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
                    dirs::home_dir()
                        .map(|h| h.join("AppData").join("Local").to_string_lossy().to_string())
                        .unwrap_or_default()
                });

                let candidates = [
                    format!(r"{}\Google\Chrome\Application\chrome.exe", program_files),
                    format!(r"{}\Google\Chrome\Application\chrome.exe", program_files_x86),
                    format!(r"{}\Google\Chrome\Application\chrome.exe", local_app_data),
                    format!(r"{}\Microsoft\Edge\Application\msedge.exe", program_files),
                    format!(r"{}\Microsoft\Edge\Application\msedge.exe", program_files_x86),
                    format!(r"{}\BraveSoftware\Brave-Browser\Application\brave.exe", program_files),
                    format!(r"{}\BraveSoftware\Brave-Browser\Application\brave.exe", program_files_x86),
                ];
                for path in &candidates {
                    if std::path::Path::new(path).exists() {
                        info!("[BROWSER] Auto-detected browser at: {}", path);
                        return Some(path.clone());
                    }
                }
            }
            None
        });

        if let Some(ref path) = chrome_path {
            builder = builder.chrome_executable(path);
        }

        // Clean up stale SingletonLock from previous crashed Chrome sessions
        // chromiumoxide uses a temp dir like /tmp/chromiumoxide-runner/
        if let Ok(tmp) = std::env::var("TMPDIR") {
            let lock = std::path::Path::new(&tmp)
                .join("chromiumoxide-runner")
                .join("SingletonLock");
            if lock.exists() {
                info!("[BROWSER] Removing stale SingletonLock at {:?}", lock);
                let _ = std::fs::remove_file(&lock);
            }
        }
        // Also check standard temp locations for SingletonLock
        {
            let mut lock_dirs: Vec<std::path::PathBuf> = vec![
                std::env::temp_dir().join("chromiumoxide-runner"),
            ];
            #[cfg(not(windows))]
            {
                lock_dirs.push(std::path::PathBuf::from("/tmp/chromiumoxide-runner"));
                lock_dirs.push(std::path::PathBuf::from("/private/tmp/chromiumoxide-runner"));
            }
            for dir in &lock_dirs {
                let lock = dir.join("SingletonLock");
                if lock.exists() {
                    info!("[BROWSER] Removing stale SingletonLock at {:?}", lock);
                    let _ = std::fs::remove_file(&lock);
                }
            }
        }

        let browser_config = builder.build().map_err(|e| anyhow::anyhow!("{}", e))?;

        let (browser, mut handler) = Browser::launch(browser_config).await.map_err(|e| {
            tracing::error!("[BROWSER] Chrome launch error details: {:?}", e);
            anyhow::anyhow!("Failed to launch Chrome browser: {}", e)
        })?;

        // Spawn the handler to process CDP events
        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    warn!("[BROWSER] CDP event error: {}", e);
                }
            }
        });

        *self.browser.write().await = Some(browser);
        info!("[BROWSER] Browser launched");

        Ok(())
    }

    /// Create a new page if none exists
    async fn ensure_page(&self) -> Result<()> {
        let has_page = self.active_page.read().await.is_some();
        if has_page {
            return Ok(());
        }

        let browser_guard = self.browser.read().await;
        let browser = browser_guard.as_ref().context("No browser instance")?;

        let page = browser
            .new_page("about:blank")
            .await
            .context("Failed to create new page")?;

        drop(browser_guard);
        *self.active_page.write().await = Some(page);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BrowserConfig;

    fn make_manager(
        domains: Vec<String>,
        enabled: bool,
        require_confirmation: bool,
    ) -> BrowserManager {
        BrowserManager::new(BrowserConfig {
            enabled,
            headless: true,
            default_timeout_ms: 30000,
            chrome_path: None,
            viewport_width: 1280,
            viewport_height: 720,
            require_confirmation,
            allowed_domains: domains,
            use_guardrails: true,
        })
    }

    #[test]
    fn test_domain_allowed_empty_list_allows_all() {
        let mgr = make_manager(vec![], true, false);
        assert!(mgr.is_domain_allowed("https://anything.com/path"));
        assert!(mgr.is_domain_allowed("https://evil.org"));
    }

    #[test]
    fn test_domain_allowed_exact_match() {
        let mgr = make_manager(vec!["example.com".into()], true, false);
        assert!(mgr.is_domain_allowed("https://example.com/path"));
    }

    #[test]
    fn test_domain_allowed_subdomain_match() {
        let mgr = make_manager(vec!["example.com".into()], true, false);
        assert!(mgr.is_domain_allowed("https://sub.example.com/path"));
    }

    #[test]
    fn test_domain_blocked_different_domain() {
        let mgr = make_manager(vec!["example.com".into()], true, false);
        assert!(!mgr.is_domain_allowed("https://evil.com"));
    }

    #[test]
    fn test_domain_blocked_partial_match_not_subdomain() {
        let mgr = make_manager(vec!["example.com".into()], true, false);
        assert!(!mgr.is_domain_allowed("https://notexample.com"));
    }

    #[test]
    fn test_domain_invalid_url_blocked() {
        let mgr = make_manager(vec!["example.com".into()], true, false);
        assert!(!mgr.is_domain_allowed("not a url at all"));
    }

    #[test]
    fn test_read_only_actions_classification() {
        let read_only = READ_ONLY_ACTIONS;
        assert_eq!(read_only.len(), 5);
        assert!(read_only.contains(&"screenshot"));
        assert!(read_only.contains(&"get_text"));
        assert!(read_only.contains(&"get_url"));
        assert!(read_only.contains(&"wait_for_selector"));
        assert!(read_only.contains(&"scroll"));
        // Mutating actions should NOT be in the list
        for action in &[
            "navigate",
            "click",
            "type",
            "evaluate_js",
            "back",
            "forward",
            "close",
        ] {
            assert!(!read_only.contains(action), "{} should be mutating", action);
        }
    }

    #[test]
    fn test_max_page_content_bytes() {
        assert_eq!(MAX_PAGE_CONTENT_BYTES, 100 * 1024);
    }

    #[test]
    fn test_is_browser_tool() {
        assert!(BrowserManager::is_browser_tool("browser"));
        assert!(!BrowserManager::is_browser_tool("computer"));
        assert!(!BrowserManager::is_browser_tool("Browser"));
        assert!(!BrowserManager::is_browser_tool(""));
    }

    #[test]
    fn test_manager_new_sets_enabled() {
        let enabled = make_manager(vec![], true, false);
        assert!(enabled.enabled);
        let disabled = make_manager(vec![], false, false);
        assert!(!disabled.enabled);
    }

    #[test]
    fn test_tool_definition_structure() {
        let mgr = make_manager(vec![], true, false);
        let def = mgr.get_tool_definition();
        assert_eq!(def["name"], "browser");
        assert!(!def["description"].as_str().unwrap().is_empty());
        let actions = &def["input_schema"]["properties"]["action"]["enum"];
        assert!(actions.is_array());
        let actions_arr = actions.as_array().unwrap();
        assert_eq!(actions_arr.len(), 14); // 5 read-only + 9 mutating (includes new actions)
        assert!(actions_arr.contains(&serde_json::json!("navigate")));
        assert!(actions_arr.contains(&serde_json::json!("screenshot")));
        assert!(actions_arr.contains(&serde_json::json!("screenshot_to_base64")));
        assert!(actions_arr.contains(&serde_json::json!("get_page_content")));
    }
}
