//! Cloud TTS backends (ElevenLabs, Cartesia)
//!
//! Provides TTS via cloud APIs. Requires API keys to function.
//! These are placeholders that are fully implemented but require configuration.

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::{debug, info};

use super::TtsBackend;

/// ElevenLabs TTS backend
pub struct ElevenLabsTts {
    api_key: Option<String>,
    voice_id: String,
    model: String,
}

impl ElevenLabsTts {
    /// Create a new ElevenLabs TTS backend
    ///
    /// # Arguments
    /// * `api_key` - ElevenLabs API key (from https://elevenlabs.io)
    /// * `voice_id` - Voice ID (default: "21m00Tcm4TlvDq8ikWAM" - Rachel)
    /// * `model` - Model ID (default: "eleven_flash_v2_5" for low latency)
    pub fn new(api_key: Option<String>, voice_id: String, model: String) -> Self {
        Self {
            api_key,
            voice_id,
            model,
        }
    }
}

#[async_trait]
impl TtsBackend for ElevenLabsTts {
    fn name(&self) -> &str {
        "elevenlabs"
    }

    fn is_available(&self) -> bool {
        self.api_key.is_some()
    }

    async fn initialize(&mut self) -> Result<()> {
        if self.api_key.is_none() {
            return Err(anyhow::anyhow!(
                "ElevenLabs API key not configured. Get one from https://elevenlabs.io"
            ));
        }

        info!("[TTS] ElevenLabs initialized with model: {}", self.model);
        Ok(())
    }

    async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        let api_key = self
            .api_key
            .as_ref()
            .context("ElevenLabs API key not configured")?;

        debug!("[TTS] Synthesizing with ElevenLabs: {}", text);

        let client = reqwest::Client::new();

        // Call ElevenLabs TTS API
        let url = format!(
            "https://api.elevenlabs.io/v1/text-to-speech/{}/stream",
            self.voice_id
        );

        let response = client
            .post(&url)
            .header("xi-api-key", api_key)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "text": text,
                "model_id": self.model,
                "voice_settings": {
                    "stability": 0.5,
                    "similarity_boost": 0.75
                }
            }))
            .send()
            .await
            .context("Failed to call ElevenLabs API")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("ElevenLabs API error: {}", error_text));
        }

        let audio_bytes = response.bytes().await?.to_vec();
        info!(
            "[TTS] ElevenLabs generated {} bytes of audio",
            audio_bytes.len()
        );

        Ok(audio_bytes)
    }
}

/// Cartesia TTS backend (Sonic model)
pub struct CartesiaTts {
    api_key: Option<String>,
    voice_id: Option<String>,
    model_id: String,
    speed: f64,
}

impl CartesiaTts {
    /// Create a new Cartesia TTS backend
    ///
    /// # Arguments
    /// * `api_key` - Cartesia API key (from https://cartesia.ai)
    /// * `voice_id` - Optional voice ID (default: Confident British Man)
    /// * `model_id` - Model ID (default: "sonic-3")
    /// * `speed` - Speech speed multiplier 0.6–1.5 (default: 1.0)
    pub fn new(
        api_key: Option<String>,
        voice_id: Option<String>,
        model_id: Option<String>,
        speed: Option<f64>,
    ) -> Self {
        Self {
            api_key,
            voice_id,
            model_id: model_id.unwrap_or_else(|| "sonic-3".to_string()),
            speed: speed.unwrap_or(1.0).clamp(0.6, 1.5),
        }
    }
}

#[async_trait]
impl TtsBackend for CartesiaTts {
    fn name(&self) -> &str {
        "cartesia"
    }

    fn is_available(&self) -> bool {
        self.api_key.is_some()
    }

    async fn initialize(&mut self) -> Result<()> {
        if self.api_key.is_none() {
            return Err(anyhow::anyhow!(
                "Cartesia API key not configured. Get one from https://cartesia.ai"
            ));
        }

        info!("[TTS] Cartesia initialized with model: {}", self.model_id);
        Ok(())
    }

    async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        let api_key = self
            .api_key
            .as_ref()
            .context("Cartesia API key not configured")?;

        debug!(
            "[TTS] Synthesizing with Cartesia ({} chars, speed={})",
            text.len(),
            self.speed
        );

        let client = reqwest::Client::new();

        // Default voice: "Confident British Man"
        let voice_id = self
            .voice_id
            .as_deref()
            .unwrap_or("63ff761f-c1e8-414b-b969-d1833d1c870c");

        // Cartesia TTS bytes API — returns WAV for easy PCM extraction
        let response = client
            .post("https://api.cartesia.ai/tts/bytes")
            .header("X-API-Key", api_key)
            .header("Cartesia-Version", "2025-04-16")
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "model_id": self.model_id,
                "transcript": text,
                "voice": {
                    "mode": "id",
                    "id": voice_id,
                },
                "generation_config": {
                    "speed": self.speed,
                },
                "output_format": {
                    "container": "wav",
                    "encoding": "pcm_s16le",
                    "sample_rate": 24000,
                },
                "language": "en",
            }))
            .send()
            .await
            .context("Failed to call Cartesia API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!(
                "Cartesia API error ({}): {}",
                status,
                error_text
            ));
        }

        let audio_bytes = response.bytes().await?.to_vec();
        info!(
            "[TTS] Cartesia generated {} bytes of WAV audio",
            audio_bytes.len()
        );

        Ok(audio_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_elevenlabs_requires_api_key() {
        let tts = ElevenLabsTts::new(None, "voice_id".to_string(), "model".to_string());
        assert!(!tts.is_available());
    }

    #[test]
    fn test_cartesia_requires_api_key() {
        let tts = CartesiaTts::new(None, None, None, None);
        assert!(!tts.is_available());
    }

    #[tokio::test]
    #[ignore] // Only run if API key is set
    async fn test_elevenlabs_synthesis() {
        let api_key = std::env::var("ELEVENLABS_API_KEY").ok();
        if api_key.is_none() {
            return;
        }

        let tts = ElevenLabsTts::new(
            api_key,
            "21m00Tcm4TlvDq8ikWAM".to_string(),
            "eleven_flash_v2_5".to_string(),
        );

        let audio = tts.synthesize("Hello, world!").await.unwrap();
        assert!(!audio.is_empty());
    }
}
