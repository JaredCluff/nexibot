//! Windows SAPI TTS backend
//!
//! Uses Windows Speech API (SAPI) via PowerShell for text-to-speech.
//! Available on all Windows versions with System.Speech assembly.

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info};

use super::TtsBackend;

/// Windows SAPI TTS backend
pub struct WindowsSapiTts {
    voice_name: Option<String>,
}

impl WindowsSapiTts {
    pub fn new() -> Self {
        Self { voice_name: None }
    }

    /// Create with a specific voice name
    pub fn with_voice(voice_name: String) -> Self {
        Self {
            voice_name: Some(voice_name),
        }
    }
}

#[async_trait]
impl TtsBackend for WindowsSapiTts {
    fn name(&self) -> &str {
        "windows_sapi"
    }

    fn is_available(&self) -> bool {
        cfg!(target_os = "windows")
    }

    async fn initialize(&mut self) -> Result<()> {
        if !self.is_available() {
            return Err(anyhow::anyhow!(
                "Windows SAPI not available (not on Windows)"
            ));
        }

        info!("[TTS] Windows SAPI initialized");
        Ok(())
    }

    async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        debug!("[TTS] Synthesizing with Windows SAPI: {}", text);

        #[cfg(target_os = "windows")]
        {
            let audio =
                crate::platform::windows_bridge::sapi_synthesize(text, self.voice_name.as_deref())?;
            info!(
                "[TTS] Windows SAPI generated {} bytes of audio",
                audio.len()
            );
            Ok(audio)
        }

        #[cfg(not(target_os = "windows"))]
        {
            Err(anyhow::anyhow!("Windows SAPI only available on Windows"))
        }
    }
}

impl Default for WindowsSapiTts {
    fn default() -> Self {
        Self::new()
    }
}
