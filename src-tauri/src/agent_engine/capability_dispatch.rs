//! Local capability dispatch — routes capability IDs to local implementations
//! or falls through to K2K task delegation for unknown capabilities.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::k2k_client::K2KIntegration;
use crate::sandbox::SandboxConfig;

// ── Local capability IDs ──────────────────────────────────────────────────────

/// Capabilities handled locally (never delegated to K2K).
pub const LOCAL_CAPABILITY_IDS: &[&str] = &[
    "llm.complete",
    "llm.embed",
    "kb.read",
    "kb.write",
    "code.execute",
    "http.get",
    "http.post",
];

/// Returns `true` if the capability can be satisfied locally without K2K delegation.
pub fn is_local_capability(capability_id: &str) -> bool {
    LOCAL_CAPABILITY_IDS.contains(&capability_id)
}

// ── Dispatcher ───────────────────────────────────────────────────────────────

/// Dispatches capability invocations to local implementations or K2K.
pub struct LocalCapabilityDispatch {
    k2k: Arc<RwLock<K2KIntegration>>,
    /// Claude client for llm.complete / llm.embed.
    claude: Arc<RwLock<crate::claude::ClaudeClient>>,
    /// Sandbox config for code.execute (Docker-backed).
    sandbox_config: SandboxConfig,
}

impl LocalCapabilityDispatch {
    /// Create a new dispatcher.
    pub fn new(
        k2k: Arc<RwLock<K2KIntegration>>,
        claude: Arc<RwLock<crate::claude::ClaudeClient>>,
        sandbox_config: SandboxConfig,
    ) -> Self {
        Self {
            k2k,
            claude,
            sandbox_config,
        }
    }

    /// Invoke a capability by ID with the given JSON input.
    ///
    /// Local capabilities are handled in-process; unknown capabilities fall
    /// through to K2K task submission (blocking poll up to 120 s).
    pub async fn invoke(&self, capability_id: &str, input: Value) -> Result<Value> {
        info!("[CAP_DISPATCH] Invoking capability '{}'", capability_id);
        match capability_id {
            "llm.complete" => self.invoke_llm_complete(&input).await,
            "llm.embed" => self.invoke_llm_embed(&input).await,
            "kb.read" => self.invoke_kb_read(&input).await,
            "kb.write" => self.invoke_kb_write(&input).await,
            "code.execute" => self.invoke_code_execute(&input).await,
            "http.get" | "http.post" => self.invoke_http(capability_id, &input).await,
            _ => self.invoke_via_k2k(capability_id, input).await,
        }
    }

    // ── llm.complete ─────────────────────────────────────────────────────────

    async fn invoke_llm_complete(&self, input: &Value) -> Result<Value> {
        let prompt = input["prompt"]
            .as_str()
            .context("llm.complete: missing 'prompt' field")?
            .to_string();

        let claude = self.claude.read().await;
        let response = claude
            .send_message(&prompt)
            .await
            .context("llm.complete: LLM call failed")?;

        Ok(serde_json::json!({ "text": response }))
    }

    // ── llm.embed ────────────────────────────────────────────────────────────

    async fn invoke_llm_embed(&self, input: &Value) -> Result<Value> {
        let text = input["text"]
            .as_str()
            .context("llm.embed: missing 'text' field")?;

        // Use the embeddings module if available; fall back gracefully.
        warn!(
            "[CAP_DISPATCH] llm.embed for text ({}chars) — local embeddings not yet wired",
            text.len()
        );
        Ok(serde_json::json!({
            "error": "llm.embed requires local ONNX embeddings — not yet wired in this build",
            "text_length": text.len()
        }))
    }

    // ── kb.read ──────────────────────────────────────────────────────────────

    async fn invoke_kb_read(&self, input: &Value) -> Result<Value> {
        let query = input["query"]
            .as_str()
            .context("kb.read: missing 'query' field")?;
        let top_k = input["top_k"].as_u64().unwrap_or(10) as usize;

        let k2k = self.k2k.read().await;
        let response = k2k
            .query(query, top_k)
            .await
            .context("kb.read: K2K query failed")?;

        let results: Vec<Value> = response
            .results
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "title": r.title,
                    "content": r.content,
                    "confidence": r.confidence,
                })
            })
            .collect();

        Ok(serde_json::json!({ "results": results }))
    }

    // ── kb.write ─────────────────────────────────────────────────────────────

    async fn invoke_kb_write(&self, input: &Value) -> Result<Value> {
        let title = input["title"]
            .as_str()
            .context("kb.write: missing 'title' field")?;
        let content = input["content"]
            .as_str()
            .context("kb.write: missing 'content' field")?;
        let tags: Vec<String> = input["tags"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let source_url = input["source_url"].as_str();
        let store_id = input["store_id"].as_str().unwrap_or("default");

        let k2k = self.k2k.read().await;
        let article_id = k2k
            .create_article(title, content, tags, source_url, store_id)
            .await
            .context("kb.write: create_article failed")?;

        Ok(serde_json::json!({ "article_id": article_id }))
    }

    // ── code.execute ─────────────────────────────────────────────────────────

    async fn invoke_code_execute(&self, input: &Value) -> Result<Value> {
        if !self.sandbox_config.enabled {
            return Ok(serde_json::json!({
                "error": "Sandbox is disabled — set sandbox.enabled=true in config to allow code.execute"
            }));
        }

        let code = input["code"]
            .as_str()
            .context("code.execute: missing 'code' field")?;
        let language = input["language"].as_str().unwrap_or("bash");
        let timeout_s = input["timeout_seconds"]
            .as_u64()
            .unwrap_or(self.sandbox_config.timeout_seconds);

        use crate::sandbox::docker::DockerSandbox;
        let mut sandbox = DockerSandbox::new(self.sandbox_config.clone());

        let cmd = match language {
            "python" | "python3" => format!("python3 -c {}", shell_escape(code)),
            "node" | "javascript" | "js" => format!("node -e {}", shell_escape(code)),
            "bash" | "sh" | "" => code.to_string(),
            other => {
                return Ok(serde_json::json!({
                    "error": format!("Unsupported language '{}' for code.execute", other)
                }));
            }
        };

        // The sandbox lifecycle: create → start → exec → stop → remove
        match sandbox.create_container().await {
            Ok(_) => {}
            Err(e) => {
                return Ok(serde_json::json!({ "error": format!("Sandbox create failed: {}", e) }));
            }
        }
        if let Err(e) = sandbox.start_container().await {
            let _ = sandbox.remove_container().await;
            return Ok(serde_json::json!({ "error": format!("Sandbox start failed: {}", e) }));
        }

        let exec_result = sandbox
            .exec_in_container(&cmd, Duration::from_secs(timeout_s))
            .await;

        let _ = sandbox.stop_container().await;
        let _ = sandbox.remove_container().await;

        match exec_result {
            Ok(output) => Ok(serde_json::json!({
                "stdout": output.stdout,
                "stderr": output.stderr,
                "exit_code": output.exit_code,
                "timed_out": output.timed_out,
            })),
            Err(e) => Ok(serde_json::json!({
                "error": e.to_string()
            })),
        }
    }

    // ── http.get / http.post ─────────────────────────────────────────────────

    async fn invoke_http(&self, method: &str, input: &Value) -> Result<Value> {
        let url = input["url"]
            .as_str()
            .context("http: missing 'url' field")?;

        // SSRF guard: validate the URL
        crate::llm_provider::validate_provider_url(url, false)
            .map_err(|e| anyhow::anyhow!("SSRF guard rejected URL: {}", e))?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("http: failed to build reqwest client")?;

        let resp = match method {
            "http.get" => client.get(url).send().await.context("http.get failed")?,
            "http.post" => {
                let body = input.get("body").cloned().unwrap_or(Value::Null);
                client
                    .post(url)
                    .json(&body)
                    .send()
                    .await
                    .context("http.post failed")?
            }
            _ => anyhow::bail!("Unknown HTTP method capability: {}", method),
        };

        let status = resp.status().as_u16();
        let body_text = resp.text().await.unwrap_or_default();

        // Try to parse body as JSON, otherwise return as string.
        let body_value: Value = serde_json::from_str(&body_text)
            .unwrap_or(Value::String(body_text));

        Ok(serde_json::json!({
            "status": status,
            "body": body_value,
        }))
    }

    // ── K2K fall-through ─────────────────────────────────────────────────────

    async fn invoke_via_k2k(&self, capability_id: &str, input: Value) -> Result<Value> {
        let k2k = self.k2k.read().await;
        let config = k2k.get_config().await;
        let base_url = config.k2k.local_agent_url.clone();
        let client_id = config.k2k.client_id.clone();
        drop(config);

        let client_guard = k2k.get_client().await;
        let client = client_guard
            .as_ref()
            .context("K2K client not initialised — cannot delegate capability")?;

        let request = k2k::TaskRequest {
            capability_id: capability_id.to_string(),
            input,
            requesting_node_id: client_id,
            client_id: String::new(),
            timeout_seconds: Some(120),
            context: None,
            priority: "normal".to_string(),
            trace_id: None,
        };

        let submit = client
            .submit_task(&base_url, &request)
            .await
            .context("K2K submit_task failed")?;

        let task_id = submit.task_id.clone();
        info!(
            "[CAP_DISPATCH] K2K task '{}' submitted for capability '{}'",
            task_id, capability_id
        );

        // Poll until complete (up to 120 iterations × 1 s)
        for attempt in 0..120u32 {
            tokio::time::sleep(Duration::from_secs(1)).await;

            match client.poll_task(&base_url, &task_id, "nexibot").await {
                Ok(status_resp) => {
                    let status_str = format!("{:?}", status_resp.status);
                    match status_str.as_str() {
                        "Completed" => {
                            let data = status_resp
                                .result
                                .map(|r| r.data)
                                .unwrap_or(Value::Null);
                            return Ok(data);
                        }
                        "Failed" | "Cancelled" => {
                            let err = status_resp
                                .error
                                .unwrap_or_else(|| format!("Task {}", status_str.to_lowercase()));
                            anyhow::bail!("K2K task {} failed: {}", task_id, err);
                        }
                        _ => {
                            if attempt % 10 == 0 {
                                info!(
                                    "[CAP_DISPATCH] K2K task {} still {} (attempt {}/120)",
                                    task_id,
                                    status_str,
                                    attempt + 1
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "[CAP_DISPATCH] K2K poll attempt {} for task {} failed: {}",
                        attempt + 1,
                        task_id,
                        e
                    );
                }
            }
        }

        anyhow::bail!(
            "K2K task {} timed out after 120 seconds (capability: {})",
            task_id,
            capability_id
        )
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Minimal shell-escape: wraps the string in single quotes, escaping internal
/// single quotes as `'\''`.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ── Execution context ────────────────────────────────────────────────────────

/// Mutable execution context for a workflow run.
///
/// Holds variable bindings set by `output_var` and the initial inputs.
#[derive(Debug, Default)]
pub struct ExecutionContext {
    vars: HashMap<String, Value>,
}

impl ExecutionContext {
    /// Create a context pre-seeded with the workflow's input variables.
    pub fn from_inputs(inputs: &HashMap<String, Value>) -> Self {
        Self {
            vars: inputs.clone(),
        }
    }

    /// Store a step result under the given variable name.
    pub fn set(&mut self, key: &str, value: Value) {
        self.vars.insert(key.to_string(), value);
    }

    /// Look up a variable.
    #[allow(dead_code)]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.vars.get(key)
    }

    /// Expose the full variable map for template substitution.
    pub fn vars(&self) -> &HashMap<String, Value> {
        &self.vars
    }
}
