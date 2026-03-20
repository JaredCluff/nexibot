//! HTTP client for communicating with NexiBot API server

use crate::error::CliError;
use reqwest::{Client as HttpClient, StatusCode};
use serde_json::{json, Value};
use std::time::Duration;

pub struct NexiBotClient {
    http_client: HttpClient,
    base_url: String,
    token: Option<String>,
    format: String,
}

impl NexiBotClient {
    /// Create a new NexiBot client
    pub fn new(base_url: String, token: Option<String>, format: String) -> Self {
        let http_client = HttpClient::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| HttpClient::new());

        Self {
            http_client,
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            format,
        }
    }

    /// Check if server is reachable
    pub async fn health_check(&self) -> Result<bool, CliError> {
        match self.get("/api/health").await {
            Ok(_) => Ok(true),
            Err(CliError::ServerUnreachable) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Send a message to Claude
    pub async fn send_message(&self, message: &str) -> Result<Value, CliError> {
        self.post("/api/chat/send", json!({ "message": message }))
            .await
    }

    /// Get current configuration
    pub async fn get_config(&self) -> Result<Value, CliError> {
        self.get("/api/config").await
    }

    /// Update configuration
    pub async fn update_config(&self, config: Value) -> Result<Value, CliError> {
        self.put("/api/config", config).await
    }

    /// List sessions
    pub async fn list_sessions(&self) -> Result<Value, CliError> {
        self.get("/api/sessions").await
    }

    /// Get available models
    pub async fn get_models(&self) -> Result<Value, CliError> {
        self.get("/api/models").await
    }

    /// List skills
    pub async fn list_skills(&self) -> Result<Value, CliError> {
        self.get("/api/skills").await
    }

    /// Get session overrides
    pub async fn get_overrides(&self) -> Result<Value, CliError> {
        self.get("/api/overrides").await
    }

    /// Set session overrides
    pub async fn set_overrides(&self, overrides: Value) -> Result<Value, CliError> {
        self.put("/api/overrides", overrides).await
    }

    /// Generic GET request
    async fn get(&self, path: &str) -> Result<Value, CliError> {
        let url = format!("{}{}", self.base_url, path);

        let mut request = self.http_client.get(&url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await?;

        match response.status() {
            StatusCode::OK => {
                let json = response.json::<Value>().await?;
                Ok(json)
            }
            StatusCode::UNAUTHORIZED => Err(CliError::Unauthorized),
            StatusCode::NOT_FOUND => Err(CliError::NotFound(path.to_string())),
            StatusCode::SERVICE_UNAVAILABLE => Err(CliError::ServerUnreachable),
            status => {
                let text = response.text().await.unwrap_or_default();
                Err(CliError::ApiError {
                    status: status.as_u16(),
                    message: text,
                })
            }
        }
    }

    /// Generic POST request
    async fn post(&self, path: &str, body: Value) -> Result<Value, CliError> {
        let url = format!("{}{}", self.base_url, path);

        let mut request = self.http_client.post(&url).json(&body);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await?;

        match response.status() {
            StatusCode::OK => {
                let json = response.json::<Value>().await?;
                Ok(json)
            }
            StatusCode::UNAUTHORIZED => Err(CliError::Unauthorized),
            StatusCode::NOT_FOUND => Err(CliError::NotFound(path.to_string())),
            StatusCode::SERVICE_UNAVAILABLE => Err(CliError::ServerUnreachable),
            status => {
                let text = response.text().await.unwrap_or_default();
                Err(CliError::ApiError {
                    status: status.as_u16(),
                    message: text,
                })
            }
        }
    }

    /// Generic PUT request
    async fn put(&self, path: &str, body: Value) -> Result<Value, CliError> {
        let url = format!("{}{}", self.base_url, path);

        let mut request = self.http_client.put(&url).json(&body);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await?;

        match response.status() {
            StatusCode::OK => {
                let json = response.json::<Value>().await?;
                Ok(json)
            }
            StatusCode::UNAUTHORIZED => Err(CliError::Unauthorized),
            StatusCode::NOT_FOUND => Err(CliError::NotFound(path.to_string())),
            StatusCode::SERVICE_UNAVAILABLE => Err(CliError::ServerUnreachable),
            status => {
                let text = response.text().await.unwrap_or_default();
                Err(CliError::ApiError {
                    status: status.as_u16(),
                    message: text,
                })
            }
        }
    }

    /// Get base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Get output format
    pub fn format(&self) -> &str {
        &self.format
    }
}
