//! macOS Speech Framework STT backend
//!
//! Uses the native macOS Speech framework (SFSpeechRecognizer) for speech-to-text.
//! On-device processing, no network required.

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info};

use super::SttBackend;
use crate::platform;

/// macOS Speech Framework STT backend
pub struct MacOsSpeechStt {
    initialized: bool,
}

impl MacOsSpeechStt {
    /// Create a new macOS Speech STT backend
    pub fn new() -> Self {
        Self { initialized: false }
    }
}

#[async_trait]
impl SttBackend for MacOsSpeechStt {
    fn name(&self) -> &str {
        "macos_speech"
    }

    fn is_available(&self) -> bool {
        platform::has_macos_speech()
    }

    async fn initialize(&mut self) -> Result<()> {
        if !self.is_available() {
            return Err(anyhow::anyhow!(
                "macOS Speech framework not available (requires macOS 10.15+)"
            ));
        }

        self.initialized = true;
        info!("[STT] macOS Speech framework initialized");

        Ok(())
    }

    async fn transcribe(&self, audio: &[f32]) -> Result<String> {
        if !self.initialized {
            return Err(anyhow::anyhow!("macOS Speech not initialized"));
        }

        debug!(
            "[STT] Transcribing {} audio samples with macOS Speech",
            audio.len()
        );

        let transcript = platform::macos_bridge::recognize_speech(audio)?;
        info!("[STT] macOS Speech transcribed: {}", transcript);
        Ok(transcript)
    }
}

impl Default for MacOsSpeechStt {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_availability() {
        let stt = MacOsSpeechStt::new();

        #[cfg(target_os = "macos")]
        {
            assert!(stt.is_available());
        }

        #[cfg(not(target_os = "macos"))]
        {
            assert!(!stt.is_available());
        }
    }

    #[tokio::test]
    async fn test_transcription() {
        if !platform::has_macos_speech() {
            return;
        }

        let mut stt = MacOsSpeechStt::new();
        stt.initialize().await.unwrap();

        // Test audio (1 second of silence at 16kHz)
        let audio = vec![0.0f32; 16000];

        let result = stt.transcribe(&audio).await;
        println!("STT result: {:?}", result);
    }
}
