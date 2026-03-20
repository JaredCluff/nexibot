//! K2K Client for querying K2K nodes

use crate::models::*;
use anyhow::{Context, Result};
use base64::Engine;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::RsaPrivateKey;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// K2K Client for querying K2K nodes with RSA JWT authentication
pub struct K2KClient {
    client: reqwest::Client,
    private_key: RsaPrivateKey,
    client_id: String,
}

impl K2KClient {
    /// Create a new K2K client with RSA private key
    pub fn new(private_key_pem: &str, client_id: impl Into<String>) -> Result<Self> {
        let private_key = rsa::RsaPrivateKey::from_pkcs8_pem(private_key_pem)
            .or_else(|_| rsa::RsaPrivateKey::from_pkcs1_pem(private_key_pem))
            .context("Failed to parse RSA private key")?;

        Ok(Self {
            client: reqwest::Client::new(),
            private_key,
            client_id: client_id.into(),
        })
    }

    /// Default JWT TTL in seconds (5 minutes).
    const DEFAULT_JWT_TTL: u64 = 300;

    /// Generate RSA-256 signed JWT for K2K authentication with the default TTL.
    fn create_jwt(&self, requesting_store: &str) -> Result<String> {
        self.create_jwt_with_ttl(requesting_store, Self::DEFAULT_JWT_TTL)
    }

    /// Generate RSA-256 signed JWT for K2K authentication with a configurable TTL.
    ///
    /// Use this for long-running operations (e.g., large knowledge transfers) that
    /// may exceed the default 5-minute window.
    pub fn create_jwt_with_ttl(&self, requesting_store: &str, ttl_seconds: u64) -> Result<String> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

        // Create JWT header
        let header = serde_json::json!({
            "alg": "RS256",
            "typ": "JWT"
        });

        // Create JWT claims
        let claims = serde_json::json!({
            "iss": format!("kb:{}", requesting_store),
            "aud": format!("kb:{}", requesting_store),
            "source_kb_id": requesting_store,
            "client_id": self.client_id,
            "iat": now,
            "exp": now + ttl_seconds as i64,
            "jti": uuid::Uuid::new_v4().to_string(),
            "transfer_id": format!("{:x}", uuid::Uuid::new_v4().as_u128() >> 64)
        });

        // Base64URL encode header and claims
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&header)?);
        let claims_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&claims)?);

        // Create signing input
        let message = format!("{}.{}", header_b64, claims_b64);

        // Hash the message
        let mut hasher = Sha256::new();
        hasher.update(message.as_bytes());
        let hash = hasher.finalize();

        // Sign with RSA private key using PKCS#1 v1.5 padding
        use rsa::pkcs1v15::Pkcs1v15Sign;
        let signature = self
            .private_key
            .sign(Pkcs1v15Sign::new::<Sha256>(), &hash)?;

        // Base64URL encode signature
        let signature_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature);

        Ok(format!("{}.{}", message, signature_b64))
    }

    /// Query a K2K node
    pub async fn query(
        &self,
        node_url: &str,
        query: &str,
        requesting_store: &str,
        top_k: usize,
        filters: Option<QueryFilters>,
    ) -> Result<K2KQueryResponse> {
        self.query_with_context(node_url, query, requesting_store, top_k, filters, None)
            .await
    }

    /// Query a K2K node with optional rich context string.
    ///
    /// The `context` string is forwarded in the `K2KQueryRequest.context` field
    /// so the backend can weight results more accurately (e.g. conversation topic,
    /// triggering tool, whether this is background enrichment or user-initiated).
    pub async fn query_with_context(
        &self,
        node_url: &str,
        query: &str,
        requesting_store: &str,
        top_k: usize,
        filters: Option<QueryFilters>,
        context: Option<&str>,
    ) -> Result<K2KQueryResponse> {
        let jwt = self.create_jwt(requesting_store)?;

        let request = K2KQueryRequest {
            query: query.to_string(),
            requesting_store: requesting_store.to_string(),
            top_k,
            filters,
            context: context.map(|s| s.to_string()),
            target_stores: None,
            trace_id: None,
        };

        let response = self
            .client
            .post(format!("{}/k2k/v1/query", node_url))
            .header("Authorization", format!("Bearer {}", jwt))
            .header("Content-Type", "application/json")
            .header("X-K2K-Protocol-Version", "1")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("K2K query failed: {}", error_text);
        }

        let result = response.json::<K2KQueryResponse>().await?;
        Ok(result)
    }

    /// Get health status from a K2K node
    pub async fn health(&self, node_url: &str) -> Result<HealthResponse> {
        let response = self
            .client
            .get(format!("{}/health", node_url))
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("K2K health check failed: {}", error_text);
        }

        let result = response.json::<HealthResponse>().await?;
        Ok(result)
    }

    /// Get node info from a K2K node
    pub async fn info(&self, node_url: &str) -> Result<NodeInfo> {
        let response = self
            .client
            .get(format!("{}/k2k/v1/info", node_url))
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("K2K info request failed: {}", error_text);
        }

        let result = response.json::<NodeInfo>().await?;
        Ok(result)
    }

    /// List capabilities from a K2K node (public, no auth)
    pub async fn list_capabilities(&self, node_url: &str) -> Result<CapabilitiesResponse> {
        let response = self
            .client
            .get(format!("{}/k2k/v1/capabilities", node_url))
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("K2K list capabilities failed: {}", error_text);
        }

        let result = response.json::<CapabilitiesResponse>().await?;
        Ok(result)
    }

    /// Submit a task to a K2K node (JWT auth)
    pub async fn submit_task(
        &self,
        node_url: &str,
        request: &TaskRequest,
    ) -> Result<TaskSubmitResponse> {
        let jwt = self.create_jwt(&request.requesting_node_id)?;

        let response = self
            .client
            .post(format!("{}/k2k/v1/tasks", node_url))
            .header("Authorization", format!("Bearer {}", jwt))
            .header("Content-Type", "application/json")
            .header("X-K2K-Protocol-Version", "1")
            .json(request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("K2K submit task failed: {}", error_text);
        }

        let result = response.json::<TaskSubmitResponse>().await?;
        Ok(result)
    }

    /// Poll task status from a K2K node (JWT auth)
    pub async fn poll_task(
        &self,
        node_url: &str,
        task_id: &str,
        requesting_store: &str,
    ) -> Result<TaskStatusResponse> {
        let jwt = self.create_jwt(requesting_store)?;

        let response = self
            .client
            .get(format!("{}/k2k/v1/tasks/{}", node_url, task_id))
            .header("Authorization", format!("Bearer {}", jwt))
            .header("X-K2K-Protocol-Version", "1")
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("K2K poll task failed: {}", error_text);
        }

        let result = response.json::<TaskStatusResponse>().await?;
        Ok(result)
    }

    /// Cancel a task on a K2K node (JWT auth)
    pub async fn cancel_task(
        &self,
        node_url: &str,
        task_id: &str,
        requesting_store: &str,
    ) -> Result<()> {
        let jwt = self.create_jwt(requesting_store)?;

        let response = self
            .client
            .delete(format!("{}/k2k/v1/tasks/{}", node_url, task_id))
            .header("Authorization", format!("Bearer {}", jwt))
            .header("X-K2K-Protocol-Version", "1")
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("K2K cancel task failed: {}", error_text);
        }

        Ok(())
    }

    /// Register this client with a K2K node
    pub async fn register(
        &self,
        node_url: &str,
        public_key_pem: &str,
    ) -> Result<RegisterClientResponse> {
        let request = RegisterClientRequest {
            store_id: self.client_id.clone(),
            public_key_pem: public_key_pem.to_string(),
            device_id: self.client_id.clone(),
            key_algorithm: "RS256".to_string(),
            key_purpose: "k2k_signing".to_string(),
        };

        let response = self
            .client
            .post(format!("{}/api/v1/keys/register", node_url))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("K2K registration failed: {}", error_text);
        }

        let result = response.json::<RegisterClientResponse>().await?;
        Ok(result)
    }
}

/// Generate a new RSA-2048 key pair for K2K authentication.
/// Returns (private_key_pem, public_key_pem) in PKCS#8/SPKI format.
pub fn generate_rsa_keypair() -> Result<(String, String)> {
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};

    let private_key = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)?;
    let public_key = private_key.to_public_key();

    let private_pem = private_key.to_pkcs8_pem(LineEnding::LF)?;
    let public_pem = public_key.to_public_key_pem(LineEnding::LF)?;

    Ok((private_pem.to_string(), public_pem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_keypair() {
        let result = generate_rsa_keypair();
        assert!(result.is_ok());
        let (private_pem, public_pem) = result.unwrap();
        assert!(private_pem.contains("BEGIN PRIVATE KEY"));
        assert!(public_pem.contains("BEGIN PUBLIC KEY"));
    }
}
