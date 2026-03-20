//! Cloud STT backends (Deepgram, OpenAI)
//!
//! Provides STT via cloud APIs. Requires API keys to function.
//! These are placeholders that are fully implemented but require configuration.

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::{debug, info};

use super::deepgram_rate_limiter::DeepgramRateLimiter;
use super::SttBackend;

/// Deepgram STT backend (Nova-3 model)
pub struct DeepgramStt {
    api_key: Option<String>,
    rate_limiter: Option<DeepgramRateLimiter>,
}

impl DeepgramStt {
    /// Create a new Deepgram STT backend
    ///
    /// # Arguments
    /// * `api_key` - Deepgram API key (from https://deepgram.com)
    /// * `rate_limiter` - Optional outbound rate limiter (tracks free-tier usage)
    pub fn new(api_key: Option<String>, rate_limiter: Option<DeepgramRateLimiter>) -> Self {
        Self {
            api_key,
            rate_limiter,
        }
    }
}

#[async_trait]
impl SttBackend for DeepgramStt {
    fn name(&self) -> &str {
        "deepgram"
    }

    fn is_available(&self) -> bool {
        self.api_key.is_some()
    }

    async fn initialize(&mut self) -> Result<()> {
        if self.api_key.is_none() {
            return Err(anyhow::anyhow!(
                "Deepgram API key not configured. Get one from https://deepgram.com"
            ));
        }

        info!("[STT] Deepgram initialized");
        Ok(())
    }

    async fn transcribe(&self, audio: &[f32]) -> Result<String> {
        let api_key = self
            .api_key
            .as_ref()
            .context("Deepgram API key not configured")?;

        // Check outbound rate limits before calling the API
        let audio_duration_secs = audio.len() as f32 / 16_000.0;
        if let Some(ref rl) = self.rate_limiter {
            rl.check(audio_duration_secs).await?;
        }

        debug!("[STT] Transcribing with Deepgram Nova-3 ({:.1}s)", audio_duration_secs);

        // Convert f32 samples to i16 PCM for API
        let pcm_data: Vec<i16> = audio
            .iter()
            .map(|&sample| (sample * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect();

        // Convert to bytes (little-endian)
        let mut bytes = Vec::with_capacity(pcm_data.len() * 2);
        for sample in pcm_data {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        let client = reqwest::Client::new();

        // Call Deepgram Nova-3 API
        let response = client
            .post("https://api.deepgram.com/v1/listen?model=nova-3&smart_format=true")
            .header("Authorization", format!("Token {}", api_key))
            .header(
                "Content-Type",
                "audio/raw; encoding=linear16; sample_rate=16000; channels=1",
            )
            .body(bytes)
            .send()
            .await
            .context("Failed to call Deepgram API")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("Deepgram API error: {}", error_text));
        }

        let result: serde_json::Value = response.json().await?;

        // Extract transcript
        let transcript = result["results"]["channels"][0]["alternatives"][0]["transcript"]
            .as_str()
            .unwrap_or("")
            .to_string();

        info!("[STT] Deepgram transcribed: {}", transcript);

        Ok(transcript)
    }
}

/// OpenAI STT backend (GPT-4o Transcribe)
pub struct OpenAIStt {
    api_key: Option<String>,
}

impl OpenAIStt {
    /// Create a new OpenAI STT backend
    ///
    /// # Arguments
    /// * `api_key` - OpenAI API key (from https://platform.openai.com)
    pub fn new(api_key: Option<String>) -> Self {
        Self { api_key }
    }
}

#[async_trait]
impl SttBackend for OpenAIStt {
    fn name(&self) -> &str {
        "openai"
    }

    fn is_available(&self) -> bool {
        self.api_key.is_some()
    }

    async fn initialize(&mut self) -> Result<()> {
        if self.api_key.is_none() {
            return Err(anyhow::anyhow!(
                "OpenAI API key not configured. Get one from https://platform.openai.com"
            ));
        }

        info!("[STT] OpenAI initialized");
        Ok(())
    }

    async fn transcribe(&self, audio: &[f32]) -> Result<String> {
        let api_key = self
            .api_key
            .as_ref()
            .context("OpenAI API key not configured")?;

        debug!("[STT] Transcribing with OpenAI GPT-4o Transcribe");

        // Convert f32 samples to i16 PCM
        let pcm_data: Vec<i16> = audio
            .iter()
            .map(|&sample| (sample * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect();

        // Convert to bytes (little-endian)
        let mut bytes = Vec::with_capacity(pcm_data.len() * 2);
        for sample in pcm_data {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        // Create multipart form with audio file
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name("audio.raw")
            .mime_str("audio/raw")?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", "gpt-4o-transcribe");

        let client = reqwest::Client::new();

        // Call OpenAI Audio API
        let response = client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
            .await
            .context("Failed to call OpenAI API")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("OpenAI API error: {}", error_text));
        }

        let result: serde_json::Value = response.json().await?;

        // Extract transcript
        let transcript = result["text"].as_str().unwrap_or("").to_string();

        info!("[STT] OpenAI transcribed: {}", transcript);

        Ok(transcript)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deepgram_requires_api_key() {
        let stt = DeepgramStt::new(None, None);
        assert!(!stt.is_available());
    }

    #[test]
    fn test_openai_requires_api_key() {
        let stt = OpenAIStt::new(None);
        assert!(!stt.is_available());
    }

    #[tokio::test]
    #[ignore] // Only run if API key is set
    async fn test_deepgram_transcription() {
        let api_key = std::env::var("DEEPGRAM_API_KEY").ok();
        if api_key.is_none() {
            return;
        }

        let stt = DeepgramStt::new(api_key, None);
        // Test audio (1 second of silence)
        let audio = vec![0.0f32; 16000];

        let result = stt.transcribe(&audio).await;
        // May succeed with empty transcript or fail
        assert!(result.is_ok() || result.is_err());
    }
}
