//! K2K Client integration for NexiBot

use anyhow::{Context, Result};
use k2k::{generate_rsa_keypair, K2KClient as K2KCommonClient, K2KQueryResponse};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::NexiBotConfig;
use crate::security::ssrf::{self, SsrfPolicy};

/// K2K Client wrapper with automatic key management
pub struct K2KIntegration {
    client: Arc<RwLock<Option<K2KCommonClient>>>,
    config: Arc<RwLock<NexiBotConfig>>,
    /// Shared HTTP client — reuses TCP connection pool across all requests.
    http_client: reqwest::Client,
}

/// Validate a URL against SSRF protections before making an HTTP request.
///
/// Uses the same fail-closed SSRF policy as the fetch tool: blocks private/internal
/// networks, cloud metadata endpoints, and non-http(s) schemes.
fn validate_k2k_url(url_str: &str) -> Result<()> {
    let policy = SsrfPolicy::default();
    ssrf::validate_outbound_request(url_str, &policy, &[])
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{}", e))
}

impl K2KIntegration {
    /// Create a new K2K integration
    pub fn new(config: Arc<RwLock<NexiBotConfig>>) -> Self {
        Self {
            client: Arc::new(RwLock::new(None)),
            config,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Initialize K2K client with authentication
    pub async fn initialize(&self) -> Result<()> {
        // Extract all needed config values, then release the read lock so we can
        // acquire the write lock if a key migration is needed.
        let (enabled, local_agent_url, client_id, legacy_key_pem) = {
            let config = self.config.read().await;
            (
                config.k2k.enabled,
                config.k2k.local_agent_url.clone(),
                config.k2k.client_id.clone(),
                config.k2k.private_key_pem.clone(),
            )
        };

        if !enabled {
            info!("K2K integration is disabled");
            return Ok(());
        }

        // Load the RSA private key from the OS keyring (preferred) or fall back to
        // config.yaml for migration from older installs.  The key is never written
        // to config going forward — only the keyring.
        const K2K_KEYRING_KEY: &str = "k2k_private_key_pem";
        let private_key_pem = match crate::security::credentials::get_secret(K2K_KEYRING_KEY) {
            Ok(Some(key)) => {
                info!("[K2K] Loaded private key from OS keyring");
                key
            }
            Ok(None) => {
                if let Some(key) = legacy_key_pem {
                    // Migrate from config.yaml to keyring
                    info!("[K2K] Migrating private key from config.yaml to OS keyring");
                    if let Err(e) = crate::security::credentials::store_secret(K2K_KEYRING_KEY, &key) {
                        warn!("[K2K] Could not store key in keyring ({}); key remains in config", e);
                    } else {
                        let mut config_write = self.config.write().await;
                        config_write.k2k.private_key_pem = None;
                        if let Err(e) = config_write.save() {
                            warn!("[K2K] Could not scrub key from config after migration: {}", e);
                        }
                    }
                    key
                } else {
                    // Generate new keypair
                    info!("[K2K] Generating new RSA keypair for K2K authentication");
                    let (private_pem, public_pem) = generate_rsa_keypair()?;

                    // SSRF validation before registering with local agent
                    validate_k2k_url(&local_agent_url)
                        .context("SSRF validation failed for K2K local_agent_url")?;

                    // Register with local agent
                    let temp_client = K2KCommonClient::new(&private_pem, &client_id)?;
                    temp_client
                        .register(&local_agent_url, &public_pem)
                        .await
                        .context("Failed to register with local K2K agent")?;

                    // Store in keyring only — never write to config.yaml
                    if let Err(e) = crate::security::credentials::store_secret(K2K_KEYRING_KEY, &private_pem) {
                        warn!("[K2K] Could not store new key in keyring: {}. Key is in-memory only for this session.", e);
                    } else {
                        info!("[K2K] RSA private key stored in OS keyring");
                    }

                    private_pem.to_string()
                }
            }
            Err(e) => {
                // Keyring unavailable (e.g., headless server) — fall back to config
                warn!("[K2K] OS keyring unavailable ({}); using config.yaml fallback", e);
                if let Some(key) = legacy_key_pem {
                    key
                } else {
                    anyhow::bail!("K2K private key not found in keyring or config");
                }
            }
        };

        // Create K2K client
        let config = self.config.read().await;
        let client = K2KCommonClient::new(&private_key_pem, &config.k2k.client_id)?;

        // SSRF validation before health check
        validate_k2k_url(&config.k2k.local_agent_url)
            .context("SSRF validation failed for K2K local_agent_url")?;

        // Test connection
        match client.health(&config.k2k.local_agent_url).await {
            Ok(health) => {
                info!(
                    "Connected to local K2K agent: {} files indexed",
                    health.indexed_files
                );
            }
            Err(e) => {
                warn!("Failed to connect to local K2K agent: {}", e);
                warn!("K2K queries may fail until the agent is running");
            }
        }

        *self.client.write().await = Some(client);
        Ok(())
    }

    /// Query local knowledge via K2K
    pub async fn query(&self, query: &str, top_k: usize) -> Result<K2KQueryResponse> {
        self.query_with_context(query, top_k, None).await
    }

    /// Query local knowledge via K2K with optional rich context.
    ///
    /// The `context` string is forwarded to the backend so it can weight
    /// results more accurately (e.g. recent conversation topic, triggering
    /// tool, whether this is background enrichment or user-initiated).
    pub async fn query_with_context(
        &self,
        query: &str,
        top_k: usize,
        context: Option<&str>,
    ) -> Result<K2KQueryResponse> {
        let config = self.config.read().await;
        let client_guard = self.client.read().await;

        let client = client_guard
            .as_ref()
            .context("K2K client not initialized")?;

        // SSRF validation before querying local agent
        validate_k2k_url(&config.k2k.local_agent_url)
            .context("SSRF validation failed for K2K local_agent_url")?;

        let result = client
            .query_with_context(
                &config.k2k.local_agent_url,
                query,
                "nexibot",
                top_k,
                None,
                context,
            )
            .await?;

        Ok(result)
    }

    /// Query federated knowledge via K2K Router (if configured)
    #[allow(dead_code)]
    pub async fn query_federated(&self, query: &str, top_k: usize) -> Result<K2KQueryResponse> {
        self.query_federated_with_context(query, top_k, None).await
    }

    /// Query federated knowledge via K2K Router with optional rich context.
    pub async fn query_federated_with_context(
        &self,
        query: &str,
        top_k: usize,
        context: Option<&str>,
    ) -> Result<K2KQueryResponse> {
        let config = self.config.read().await;
        let client_guard = self.client.read().await;

        let client = client_guard
            .as_ref()
            .context("K2K client not initialized")?;

        let router_url = config
            .k2k
            .router_url
            .as_ref()
            .context("K2K Router URL not configured")?;

        // SSRF validation before querying federated router
        validate_k2k_url(router_url)
            .context("SSRF validation failed for K2K router_url")?;

        let result = client
            .query_with_context(router_url, query, "nexibot", top_k, None, context)
            .await?;

        Ok(result)
    }

    /// Check if K2K is available
    pub async fn is_available(&self) -> bool {
        let client_guard = self.client.read().await;
        client_guard.is_some()
    }

    /// Get a read guard to the underlying K2K client (for multi-agent commands)
    pub async fn get_client(&self) -> tokio::sync::RwLockReadGuard<'_, Option<K2KCommonClient>> {
        self.client.read().await
    }

    /// Get a read guard to the config (for agent orchestration)
    pub async fn get_config(&self) -> tokio::sync::RwLockReadGuard<'_, NexiBotConfig> {
        self.config.read().await
    }

    /// Health check — test if the System Agent is reachable
    pub async fn health_check(&self) -> Result<bool> {
        let config = self.config.read().await;
        let client_guard = self.client.read().await;

        let client = match client_guard.as_ref() {
            Some(c) => c,
            None => return Ok(false),
        };

        // SSRF validation before health check
        validate_k2k_url(&config.k2k.local_agent_url)?;

        match client.health(&config.k2k.local_agent_url).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Save a conversation to the System Agent's conversation API.
    /// Returns the conversation ID assigned by the System Agent.
    pub async fn save_conversation(
        &self,
        title: &str,
        messages: &[(String, String)], // (role, content) pairs
    ) -> Result<String> {
        let config = self.config.read().await;
        let base_url = &config.k2k.local_agent_url;

        // SSRF validation before saving conversation
        validate_k2k_url(base_url)
            .context("SSRF validation failed for K2K local_agent_url")?;

        let http = self.http_client.clone();

        // Create conversation
        let create_resp = http
            .post(format!("{}/api/v1/conversations", base_url))
            .json(&serde_json::json!({
                "title": title,
                "source": "nexibot",
            }))
            .send()
            .await
            .context("Failed to create conversation on System Agent")?;

        if !create_resp.status().is_success() {
            let err = create_resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to create conversation: {}", err);
        }

        let resp_json: serde_json::Value = create_resp.json().await?;
        let conv_id = resp_json["id"]
            .as_str()
            .context("No conversation ID in response")?
            .to_string();

        // Add messages
        for (i, (role, content)) in messages.iter().enumerate() {
            let resp = http
                .post(format!(
                    "{}/api/v1/conversations/{}/messages",
                    base_url, conv_id
                ))
                .json(&serde_json::json!({
                    "role": role,
                    "content": content,
                }))
                .send()
                .await;
            match resp {
                Ok(r) if !r.status().is_success() => {
                    warn!(
                        "[K2K] Failed to save message {}/{} to conversation {}: HTTP {}",
                        i + 1, messages.len(), conv_id, r.status()
                    );
                }
                Err(e) => {
                    warn!(
                        "[K2K] Failed to save message {}/{} to conversation {}: {}",
                        i + 1, messages.len(), conv_id, e
                    );
                }
                _ => {}
            }
        }

        info!(
            "[K2K] Saved conversation {} with {} messages",
            conv_id,
            messages.len()
        );
        Ok(conv_id)
    }

    /// Extract knowledge from a saved conversation.
    pub async fn extract_conversation_knowledge(&self, conv_id: &str) -> Result<()> {
        let config = self.config.read().await;
        let base_url = &config.k2k.local_agent_url;

        // SSRF validation before extracting knowledge
        validate_k2k_url(base_url)
            .context("SSRF validation failed for K2K local_agent_url")?;

        let http = self.http_client.clone();
        let resp = http
            .post(format!(
                "{}/api/v1/conversations/{}/extract",
                base_url, conv_id
            ))
            .send()
            .await
            .context("Failed to extract knowledge from conversation")?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            warn!("[K2K] Knowledge extraction failed: {}", err);
        } else {
            info!("[K2K] Knowledge extracted from conversation {}", conv_id);
        }

        Ok(())
    }

    /// Create a new article in the System Agent.
    /// Returns the created article's ID.
    pub async fn create_article(
        &self,
        title: &str,
        content: &str,
        tags: Vec<String>,
        source_url: Option<&str>,
        store_id: &str,
    ) -> Result<String> {
        let config = self.config.read().await;
        let base_url = &config.k2k.local_agent_url;

        // SSRF validation before creating article
        validate_k2k_url(base_url)
            .context("SSRF validation failed for K2K local_agent_url")?;

        let http = self.http_client.clone();
        let mut payload = serde_json::json!({
            "store_id": store_id,
            "title": title,
            "content": content,
            "tags": tags,
            "source_type": "nexibot",
        });
        if let Some(url) = source_url {
            payload["source_url"] = serde_json::Value::String(url.to_string());
        }

        let resp = http
            .post(format!("{}/api/v1/articles", base_url))
            .json(&payload)
            .send()
            .await
            .context("Failed to POST /api/v1/articles")?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to create article: {}", err);
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let article_id = resp_json["id"]
            .as_str()
            .context("No article ID in response")?
            .to_string();

        info!("[K2K] Created article '{}' (id={})", title, article_id);
        Ok(article_id)
    }

    /// Update an existing article in the System Agent (partial update).
    pub async fn update_article(
        &self,
        article_id: &str,
        title: Option<&str>,
        content: Option<&str>,
        tags: Option<Vec<String>>,
    ) -> Result<()> {
        let config = self.config.read().await;
        let base_url = &config.k2k.local_agent_url;

        // SSRF validation before updating article
        validate_k2k_url(base_url)
            .context("SSRF validation failed for K2K local_agent_url")?;

        let http = self.http_client.clone();
        let mut payload = serde_json::json!({});
        if let Some(t) = title {
            payload["title"] = serde_json::Value::String(t.to_string());
        }
        if let Some(c) = content {
            payload["content"] = serde_json::Value::String(c.to_string());
        }
        if let Some(tags_vec) = tags {
            payload["tags"] = serde_json::json!(tags_vec);
        }

        let resp = http
            .patch(format!("{}/api/v1/articles/{}", base_url, article_id))
            .json(&payload)
            .send()
            .await
            .context("Failed to PATCH /api/v1/articles/{id}")?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to update article {}: {}", article_id, err);
        }

        info!("[K2K] Updated article {}", article_id);
        Ok(())
    }

    /// List available knowledge stores from the System Agent.
    pub async fn list_stores(&self) -> Result<Vec<StoreInfo>> {
        let config = self.config.read().await;
        let base_url = &config.k2k.local_agent_url;

        // SSRF validation before listing stores
        validate_k2k_url(base_url)
            .context("SSRF validation failed for K2K local_agent_url")?;

        let http = self.http_client.clone();
        let resp = http
            .get(format!("{}/k2k/v1/stores", base_url))
            .send()
            .await
            .context("Failed to GET /k2k/v1/stores")?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to list stores: {}", err);
        }

        let stores: Vec<StoreInfo> = resp.json().await?;
        Ok(stores)
    }

    /// Query K2K for contextually relevant results (supermemory recall).
    #[allow(dead_code)]
    pub async fn query_context(&self, query: &str, top_k: usize) -> Result<Vec<SupermemoryResult>> {
        match self.query_with_context(query, top_k, None).await {
            Ok(response) => {
                let results: Vec<SupermemoryResult> = response
                    .results
                    .into_iter()
                    .map(|r| SupermemoryResult {
                        title: r.title,
                        content: r.content,
                        confidence: r.confidence,
                    })
                    .collect();
                Ok(results)
            }
            Err(e) => {
                warn!("[K2K] Context query failed: {}", e);
                Ok(Vec::new())
            }
        }
    }
}

/// A result from supermemory context query
#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SupermemoryResult {
    pub title: String,
    pub content: String,
    pub confidence: f32,
}

/// A knowledge store available on the System Agent
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoreInfo {
    pub id: String,
    pub owner_id: String,
    pub store_type: String,
    pub name: String,
    pub lancedb_collection: String,
    pub created_at: String,
    pub updated_at: String,
}
