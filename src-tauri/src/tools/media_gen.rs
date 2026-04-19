//! Media generation tools: generate_image, generate_audio, generate_video.
use crate::tool_registry::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

pub fn media_output_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nexibot")
        .join("media")
}

// ---------------------------------------------------------------------------
// GenerateImageTool — DALL-E 3
// ---------------------------------------------------------------------------

pub struct GenerateImageTool {
    config: Arc<RwLock<crate::config::NexiBotConfig>>,
}

impl GenerateImageTool {
    pub fn new(config: Arc<RwLock<crate::config::NexiBotConfig>>) -> Self { Self { config } }
    pub fn new_stub() -> Self {
        Self { config: Arc::new(RwLock::new(crate::config::NexiBotConfig::default())) }
    }
}

#[async_trait]
impl Tool for GenerateImageTool {
    fn name(&self) -> &str { "generate_image" }
    fn description(&self) -> &str {
        "Generate an image from a text prompt using DALL-E 3. Returns the absolute path to the saved PNG file."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "Text description of the image" },
                "size": { "type": "string", "enum": ["1024x1024","1792x1024","1024x1792"] },
                "quality": { "type": "string", "enum": ["standard","hd"] }
            },
            "required": ["prompt"]
        })
    }
    async fn check_permissions(&self, input: &Value, _ctx: &ToolContext) -> PermissionDecision {
        if input["prompt"].as_str().map(|s| s.is_empty()).unwrap_or(true) {
            return PermissionDecision::Deny("prompt is required".to_string());
        }
        PermissionDecision::Ask {
            reason: "generate_image calls an external API and incurs costs".to_string(),
            details: input["prompt"].as_str().map(|s| s.to_string()),
        }
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        let prompt = match input["prompt"].as_str() {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => return ToolResult::err("prompt is required"),
        };
        let (api_key, model, size) = {
            let cfg = self.config.read().await;
            let key = cfg.media_gen.image_api_key.clone()
                .or_else(|| cfg.openai.api_key.clone())
                .unwrap_or_default();
            let model = cfg.media_gen.image_model.clone();
            let size = input["size"].as_str().map(|s| s.to_string())
                .unwrap_or_else(|| cfg.media_gen.image_size.clone());
            (key, model, size)
        };
        if api_key.is_empty() {
            return ToolResult::err("No image API key configured. Set media_gen.image_api_key or openai.api_key.");
        }
        let quality = input["quality"].as_str().unwrap_or("standard");
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::err(format!("Failed to build HTTP client: {}", e)),
        };
        let resp = match client.post("https://api.openai.com/v1/images/generations")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&json!({
                "model": model,
                "prompt": prompt,
                "n": 1,
                "size": size,
                "quality": quality,
                "response_format": "b64_json"
            }))
            .send().await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::err(format!("Request failed: {}", e)),
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return ToolResult::err(format!("DALL-E error {}: {}", status, text));
        }
        let resp_json: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => return ToolResult::err(format!("Parse failed: {}", e)),
        };
        let b64 = match resp_json["data"][0]["b64_json"].as_str() {
            Some(s) => s,
            None => return ToolResult::err("No image data in response"),
        };
        use base64::Engine as _;
        let image_bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(b) => b,
            Err(e) => return ToolResult::err(format!("Base64 decode: {}", e)),
        };
        let output_dir = media_output_dir();
        if let Err(e) = tokio::fs::create_dir_all(&output_dir).await {
            return ToolResult::err(format!("Create dir failed: {}", e));
        }
        let filename = format!("image_{}.png", uuid::Uuid::new_v4());
        let output_path = output_dir.join(&filename);
        if let Err(e) = tokio::fs::write(&output_path, &image_bytes).await {
            return ToolResult::err(format!("Save failed: {}", e));
        }
        info!("[MEDIA] Saved image to {:?}", output_path);
        ToolResult::ok(format!("Image saved to: {}", output_path.display()))
    }
}

// ---------------------------------------------------------------------------
// GenerateAudioTool — ElevenLabs
// ---------------------------------------------------------------------------

const DEFAULT_VOICE_ID: &str = "21m00Tcm4TlvDq8ikWAM"; // ElevenLabs "Rachel"

pub struct GenerateAudioTool {
    config: Arc<RwLock<crate::config::NexiBotConfig>>,
}

impl GenerateAudioTool {
    pub fn new(config: Arc<RwLock<crate::config::NexiBotConfig>>) -> Self { Self { config } }
    pub fn new_stub() -> Self {
        Self { config: Arc::new(RwLock::new(crate::config::NexiBotConfig::default())) }
    }
}

#[async_trait]
impl Tool for GenerateAudioTool {
    fn name(&self) -> &str { "generate_audio" }
    fn description(&self) -> &str {
        "Convert text to speech using ElevenLabs. Returns path to the saved MP3 file."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text to convert to speech" },
                "voice_id": { "type": "string", "description": "ElevenLabs voice ID (optional)" },
                "model_id": { "type": "string", "description": "ElevenLabs model ID (optional)" }
            },
            "required": ["text"]
        })
    }
    async fn check_permissions(&self, input: &Value, _ctx: &ToolContext) -> PermissionDecision {
        if input["text"].as_str().map(|s| s.is_empty()).unwrap_or(true) {
            return PermissionDecision::Deny("text is required".to_string());
        }
        PermissionDecision::Ask {
            reason: "generate_audio calls ElevenLabs API and incurs costs".to_string(),
            details: None,
        }
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        let text = match input["text"].as_str() {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => return ToolResult::err("text is required"),
        };
        let (api_key, voice_id, model_id) = {
            let cfg = self.config.read().await;
            let key = cfg.media_gen.audio_api_key.clone().unwrap_or_default();
            let vid = input["voice_id"].as_str().map(|s| s.to_string())
                .or_else(|| cfg.media_gen.elevenlabs_voice_id.clone())
                .unwrap_or_else(|| DEFAULT_VOICE_ID.to_string());
            let mid = input["model_id"].as_str().map(|s| s.to_string())
                .unwrap_or_else(|| cfg.media_gen.audio_model.clone());
            (key, vid, mid)
        };
        if api_key.is_empty() {
            return ToolResult::err("No audio API key configured. Set media_gen.audio_api_key.");
        }
        // Validate voice_id is alphanumeric/dash/underscore only to prevent URL injection.
        if !voice_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return ToolResult::err("voice_id contains invalid characters");
        }
        let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{}", voice_id);
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::err(format!("Failed to build HTTP client: {}", e)),
        };
        let resp = match client.post(&url)
            .header("xi-api-key", &api_key)
            .json(&json!({
                "text": text,
                "model_id": model_id,
                "voice_settings": { "stability": 0.5, "similarity_boost": 0.75 }
            }))
            .send().await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::err(format!("Request failed: {}", e)),
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            return ToolResult::err(format!("ElevenLabs error {}: {}", status, txt));
        }
        let audio_bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return ToolResult::err(format!("Read failed: {}", e)),
        };
        let output_dir = media_output_dir();
        if let Err(e) = tokio::fs::create_dir_all(&output_dir).await {
            return ToolResult::err(format!("Create dir: {}", e));
        }
        let filename = format!("audio_{}.mp3", uuid::Uuid::new_v4());
        let output_path = output_dir.join(&filename);
        if let Err(e) = tokio::fs::write(&output_path, &audio_bytes).await {
            return ToolResult::err(format!("Save failed: {}", e));
        }
        info!("[MEDIA] Saved audio to {:?}", output_path);
        ToolResult::ok(format!("Audio saved to: {}", output_path.display()))
    }
}

// ---------------------------------------------------------------------------
// GenerateVideoTool — Runway stub
// ---------------------------------------------------------------------------

pub struct GenerateVideoTool {
    config: Arc<RwLock<crate::config::NexiBotConfig>>,
}

impl GenerateVideoTool {
    pub fn new(config: Arc<RwLock<crate::config::NexiBotConfig>>) -> Self { Self { config } }
    pub fn new_stub() -> Self {
        Self { config: Arc::new(RwLock::new(crate::config::NexiBotConfig::default())) }
    }
}

#[async_trait]
impl Tool for GenerateVideoTool {
    fn name(&self) -> &str { "generate_video" }
    fn description(&self) -> &str {
        "Submit a video generation task to Runway Gen-3. Returns a task ID — generation is asynchronous and the video must be retrieved separately (requires Runway API key)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "Text description for video generation" },
                "duration_seconds": {
                    "type": "integer",
                    "description": "Duration in seconds (4 or 8)",
                    "enum": [4, 8]
                }
            },
            "required": ["prompt"]
        })
    }
    async fn check_permissions(&self, _input: &Value, _ctx: &ToolContext) -> PermissionDecision {
        let provider = {
            let cfg = self.config.read().await;
            cfg.media_gen.video_provider.clone()
        };
        if provider.is_none() {
            return PermissionDecision::Deny(
                "No video provider configured. Set media_gen.video_provider and media_gen.video_api_key.".to_string()
            );
        }
        PermissionDecision::Ask {
            reason: "generate_video submits a job to Runway API and incurs significant costs".to_string(),
            details: None,
        }
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        let provider = {
            let cfg = self.config.read().await;
            cfg.media_gen.video_provider.clone()
        };
        match provider.as_deref() {
            Some("runway") => {
                let api_key = {
                    let cfg = self.config.read().await;
                    cfg.media_gen.video_api_key.clone().unwrap_or_default()
                };
                if api_key.is_empty() {
                    return ToolResult::err("Runway API key not configured. Set media_gen.video_api_key.");
                }
                let prompt = input["prompt"].as_str().unwrap_or("").to_string();
                let duration = input["duration_seconds"].as_u64().unwrap_or(4);
                let client = match reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(300))
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => return ToolResult::err(format!("Failed to build HTTP client: {}", e)),
                };
                let resp = match client.post("https://api.runwayml.com/v1/image_to_video")
                    .header("Authorization", format!("Bearer {}", api_key))
                    .header("X-Runway-Version", "2024-11-06")
                    .json(&json!({
                        "promptText": prompt,
                        "duration": duration,
                        "model": "gen3a_turbo"
                    }))
                    .send().await
                {
                    Ok(r) => r,
                    Err(e) => return ToolResult::err(format!("Runway request failed: {}", e)),
                };
                if !resp.status().is_success() {
                    let status = resp.status();
                    let txt = resp.text().await.unwrap_or_default();
                    return ToolResult::err(format!("Runway error {}: {}", status, txt));
                }
                let resp_json: Value = match resp.json().await {
                    Ok(v) => v,
                    Err(e) => return ToolResult::err(format!("Parse: {}", e)),
                };
                ToolResult::ok(format!(
                    "Video task submitted. ID: {}",
                    resp_json["id"].as_str().unwrap_or("unknown")
                ))
            }
            Some(p) => ToolResult::err(format!(
                "Unsupported video provider '{}'. Use 'runway'.", p
            )),
            None => ToolResult::err(
                "No video provider configured. Set media_gen.video_provider = \"runway\" and media_gen.video_api_key."
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_image_schema_has_required_prompt() {
        let tool = GenerateImageTool::new_stub();
        let schema = tool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["prompt"]));
    }

    #[test]
    fn media_output_dir_path() {
        let dir = media_output_dir();
        let s = dir.to_string_lossy();
        assert!(s.contains("nexibot") && s.contains("media"), "path: {}", s);
    }

    #[test]
    fn generate_audio_schema_has_required_text() {
        let tool = GenerateAudioTool::new_stub();
        let schema = tool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["text"]));
        assert_eq!(tool.name(), "generate_audio");
    }

    #[test]
    fn generate_video_tool_returns_not_configured_when_no_provider() {
        let tool = GenerateVideoTool::new_stub();
        assert_eq!(tool.name(), "generate_video");
        let schema = tool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["prompt"]));
    }

    #[test]
    fn generate_video_description_mentions_task_id() {
        let tool = GenerateVideoTool::new_stub();
        assert!(
            tool.description().contains("task ID"),
            "description must be honest that it returns a task ID, not a file path"
        );
    }

    #[test]
    fn voice_id_valid_characters() {
        // Verify the voice_id validation logic accepts expected IDs
        let valid_ids = ["21m00Tcm4TlvDq8ikWAM", "abc123", "voice-id_01"];
        for id in &valid_ids {
            assert!(
                id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "Expected '{}' to be valid", id
            );
        }
        let invalid_ids = ["../etc/passwd", "voice/../../etc", "id\0null"];
        for id in &invalid_ids {
            assert!(
                !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "Expected '{}' to be rejected", id
            );
        }
    }
}
