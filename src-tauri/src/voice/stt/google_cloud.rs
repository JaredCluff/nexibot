//! Google Cloud Speech-to-Text STT backend
//!
//! Provides STT via Google Cloud Speech API. Requires a GCP API key or
//! service account credentials to function.

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::{debug, info};

use super::SttBackend;

/// Google Cloud Speech STT backend
pub struct GoogleCloudStt {
    api_key: Option<String>,
    language_code: String,
}

impl GoogleCloudStt {
    /// Create a new Google Cloud STT backend
    ///
    /// # Arguments
    /// * `api_key` - Google Cloud API key (from GCP Console)
    /// * `language_code` - BCP-47 language code (default: "en-US")
    pub fn new(api_key: Option<String>, language_code: String) -> Self {
        Self {
            api_key,
            language_code,
        }
    }
}

#[async_trait]
impl SttBackend for GoogleCloudStt {
    fn name(&self) -> &str {
        "google_cloud"
    }

    fn is_available(&self) -> bool {
        self.api_key.is_some()
    }

    async fn initialize(&mut self) -> Result<()> {
        if self.api_key.is_none() {
            return Err(anyhow::anyhow!(
                "Google Cloud API key not configured. Get one from https://cloud.google.com/speech"
            ));
        }

        info!("[STT] Google Cloud Speech initialized");
        Ok(())
    }

    async fn transcribe(&self, audio: &[f32]) -> Result<String> {
        let api_key = self
            .api_key
            .as_ref()
            .context("Google Cloud API key not configured")?;

        debug!("[STT] Transcribing with Google Cloud Speech");

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

        let audio_base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let response = client
            .post(format!(
                "https://speech.googleapis.com/v1/speech:recognize?key={}",
                api_key
            ))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "config": {
                    "encoding": "LINEAR16",
                    "sampleRateHertz": 16000,
                    "languageCode": self.language_code,
                    "model": "latest_long"
                },
                "audio": {
                    "content": audio_base64
                }
            }))
            .send()
            .await
            .context("Failed to call Google Cloud Speech API")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!(
                "Google Cloud Speech API error: {}",
                error_text
            ));
        }

        let result: serde_json::Value = response.json().await?;

        // Extract transcript: results[0].alternatives[0].transcript
        let transcript = result
            .get("results")
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.first())
            .and_then(|r| r.get("alternatives"))
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|a| a.get("transcript"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        info!("[STT] Google Cloud transcribed: {}", transcript);

        Ok(transcript)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_google_cloud_requires_api_key() {
        let stt = GoogleCloudStt::new(None, "en-US".into());
        assert!(!stt.is_available());
    }

    #[test]
    fn test_google_cloud_available_with_key() {
        let stt = GoogleCloudStt::new(Some("test-key".into()), "en-US".into());
        assert!(stt.is_available());
    }

    #[test]
    fn test_google_cloud_name() {
        let stt = GoogleCloudStt::new(None, "en-US".into());
        assert_eq!(stt.name(), "google_cloud");
    }

    #[tokio::test]
    async fn test_google_cloud_initialize_fails_without_key() {
        let mut stt = GoogleCloudStt::new(None, "en-US".into());
        assert!(stt.initialize().await.is_err());
    }

    #[tokio::test]
    async fn test_google_cloud_initialize_succeeds_with_key() {
        let mut stt = GoogleCloudStt::new(Some("test-key".into()), "en-US".into());
        assert!(stt.initialize().await.is_ok());
    }
}
