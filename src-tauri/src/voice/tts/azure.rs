//! Azure Speech Services Neural TTS backend
//!
//! Provides TTS via Azure Cognitive Services Speech API. Requires a
//! subscription key and region to function.

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::{debug, info};

use super::TtsBackend;

/// Azure Neural TTS backend
pub struct AzureTts {
    subscription_key: Option<String>,
    region: String,
    voice_name: String,
}

impl AzureTts {
    /// Create a new Azure Neural TTS backend
    ///
    /// # Arguments
    /// * `subscription_key` - Azure Speech Services subscription key
    /// * `region` - Azure region (e.g. "westus", "eastus")
    /// * `voice_name` - Voice name (default: "en-US-AriaNeural")
    pub fn new(subscription_key: Option<String>, region: String, voice_name: String) -> Self {
        Self {
            subscription_key,
            region,
            voice_name,
        }
    }
}

#[async_trait]
impl TtsBackend for AzureTts {
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

        info!(
            "[TTS] Azure Neural TTS initialized (region: {}, voice: {})",
            self.region, self.voice_name
        );
        Ok(())
    }

    async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        let key = self
            .subscription_key
            .as_ref()
            .context("Azure Speech subscription key not configured")?;

        debug!("[TTS] Synthesizing with Azure Neural TTS: {}", text);

        let ssml = format!(
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>\
             <voice name='{}'>{}</voice></speak>",
            self.voice_name,
            xml_escape(text)
        );

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let url = format!(
            "https://{}.tts.speech.microsoft.com/cognitiveservices/v1",
            self.region
        );

        let response = client
            .post(&url)
            .header("Ocp-Apim-Subscription-Key", key)
            .header("Content-Type", "application/ssml+xml")
            .header("X-Microsoft-OutputFormat", "audio-16khz-128kbitrate-mono-mp3")
            .body(ssml)
            .send()
            .await
            .context("Failed to call Azure TTS API")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("Azure TTS API error: {}", error_text));
        }

        let audio_bytes = response.bytes().await?.to_vec();
        info!(
            "[TTS] Azure Neural TTS generated {} bytes of audio",
            audio_bytes.len()
        );

        Ok(audio_bytes)
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
