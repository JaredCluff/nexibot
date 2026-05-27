//! Azure Speech Services STT backend
//!
//! Provides STT via Azure Cognitive Services Speech API. Requires a
//! subscription key and region to function.

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::{debug, info};

use super::SttBackend;

/// Azure Speech STT backend
pub struct AzureStt {
    subscription_key: Option<String>,
    region: String,
    language: String,
}

impl AzureStt {
    /// Create a new Azure Speech STT backend
    ///
    /// # Arguments
    /// * `subscription_key` - Azure Speech Services subscription key
    /// * `region` - Azure region (e.g. "westus", "eastus")
    /// * `language` - BCP-47 language code (default: "en-US")
    pub fn new(subscription_key: Option<String>, region: String, language: String) -> Self {
        Self {
            subscription_key,
            region,
            language,
        }
    }
}

#[async_trait]
impl SttBackend for AzureStt {
    fn name(&self) -> &str {
        "azure"
    }

    fn is_available(&self) -> bool {
        self.subscription_key.is_some() && !self.region.is_empty()
    }

    async fn initialize(&mut self) -> Result<()> {
        if self.subscription_key.is_none() {
            return Err(anyhow::anyhow!(
                "Azure Speech subscription key not configured. Get one from https://azure.microsoft.com/services/cognitive-services/speech/"
            ));
        }
        if self.region.is_empty() {
            return Err(anyhow::anyhow!(
                "Azure Speech region not configured (e.g. 'westus', 'eastus')"
            ));
        }

        info!("[STT] Azure Speech initialized (region: {})", self.region);
        Ok(())
    }

    async fn transcribe(&self, audio: &[f32]) -> Result<String> {
        let key = self
            .subscription_key
            .as_ref()
            .context("Azure Speech subscription key not configured")?;

        debug!("[STT] Transcribing with Azure Speech");

        // Convert f32 samples to i16 PCM
        let pcm_data: Vec<i16> = audio
            .iter()
            .map(|&sample| (sample * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect();

        let mut bytes = Vec::with_capacity(pcm_data.len() * 2);
        for sample in pcm_data {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let url = format!(
            "https://{}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1?language={}",
            self.region, self.language
        );

        let response = client
            .post(&url)
            .header("Ocp-Apim-Subscription-Key", key)
            .header("Content-Type", "audio/wav; codecs=audio/pcm; samplerate=16000")
            .body(bytes)
            .send()
            .await
            .context("Failed to call Azure Speech API")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("Azure Speech API error: {}", error_text));
        }

        let result: serde_json::Value = response.json().await?;

        // Extract transcript: DisplayText from the first NBest result
        let transcript = result
            .get("NBest")
            .and_then(|n| n.as_array())
            .and_then(|arr| arr.first())
            .and_then(|n| n.get("DisplayText"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        info!("[STT] Azure Speech transcribed: {}", transcript);

        Ok(transcript)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_azure_requires_key_and_region() {
        let stt = AzureStt::new(None, "westus".into(), "en-US".into());
        assert!(!stt.is_available());

        let stt = AzureStt::new(Some("key".into()), "".into(), "en-US".into());
        assert!(!stt.is_available());

        let stt = AzureStt::new(Some("key".into()), "westus".into(), "en-US".into());
        assert!(stt.is_available());
    }

    #[test]
    fn test_azure_name() {
        let stt = AzureStt::new(None, "westus".into(), "en-US".into());
        assert_eq!(stt.name(), "azure");
    }

    #[tokio::test]
    async fn test_azure_initialize_fails_without_key() {
        let mut stt = AzureStt::new(None, "westus".into(), "en-US".into());
        assert!(stt.initialize().await.is_err());
    }

    #[tokio::test]
    async fn test_azure_initialize_fails_without_region() {
        let mut stt = AzureStt::new(Some("key".into()), "".into(), "en-US".into());
        assert!(stt.initialize().await.is_err());
    }

    #[tokio::test]
    async fn test_azure_initialize_succeeds() {
        let mut stt = AzureStt::new(Some("key".into()), "westus".into(), "en-US".into());
        assert!(stt.initialize().await.is_ok());
    }
}
