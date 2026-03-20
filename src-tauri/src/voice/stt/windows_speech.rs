//! Windows SAPI STT backend
//!
//! Uses Windows Speech API (SAPI) via PowerShell for speech-to-text.
//! Available on all Windows versions with System.Speech assembly.

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info};

use super::SttBackend;

/// Windows SAPI Speech Recognition backend
pub struct WindowsSpeechStt {
    initialized: bool,
}

impl WindowsSpeechStt {
    pub fn new() -> Self {
        Self { initialized: false }
    }
}

#[async_trait]
impl SttBackend for WindowsSpeechStt {
    fn name(&self) -> &str {
        "windows_speech"
    }

    fn is_available(&self) -> bool {
        cfg!(target_os = "windows")
    }

    async fn initialize(&mut self) -> Result<()> {
        if !self.is_available() {
            return Err(anyhow::anyhow!(
                "Windows Speech API not available (not on Windows)"
            ));
        }

        self.initialized = true;
        info!("[STT] Windows SAPI initialized");
        Ok(())
    }

    async fn transcribe(&self, audio: &[f32]) -> Result<String> {
        if !self.initialized {
            return Err(anyhow::anyhow!("Windows Speech not initialized"));
        }

        debug!(
            "[STT] Transcribing {} audio samples with Windows SAPI",
            audio.len()
        );

        #[cfg(target_os = "windows")]
        {
            let transcript = crate::platform::windows_bridge::sapi_recognize(audio)?;
            info!("[STT] Windows SAPI transcribed: {}", transcript);
            Ok(transcript)
        }

        #[cfg(not(target_os = "windows"))]
        {
            Err(anyhow::anyhow!("Windows SAPI only available on Windows"))
        }
    }
}

impl Default for WindowsSpeechStt {
    fn default() -> Self {
        Self::new()
    }
}
