//! espeak-ng TTS backend (Linux fallback)
//!
//! Uses the espeak-ng command-line tool for text-to-speech.
//! Available on most Linux distributions. Robotic but reliable.
//! Same CLI-wrapper pattern as `macos_say.rs`.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::process::Command;
use tracing::{debug, info};

use super::TtsBackend;

/// espeak-ng TTS backend
pub struct EspeakTts {
    voice: String,
}

impl EspeakTts {
    /// Create a new espeak-ng TTS backend with default English voice
    pub fn new() -> Self {
        Self {
            voice: "en".to_string(),
        }
    }

    /// Create with a specific voice/language
    pub fn with_voice(voice: String) -> Self {
        Self { voice }
    }

    /// Check if espeak-ng is available on PATH
    fn espeak_on_path() -> bool {
        Command::new("espeak-ng")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[async_trait]
impl TtsBackend for EspeakTts {
    fn name(&self) -> &str {
        "espeak"
    }

    fn is_available(&self) -> bool {
        Self::espeak_on_path()
    }

    async fn initialize(&mut self) -> Result<()> {
        if !Self::espeak_on_path() {
            return Err(anyhow::anyhow!(
                "espeak-ng not found on PATH. Install with: sudo apt install espeak-ng"
            ));
        }

        info!("[TTS] espeak-ng initialized with voice: {}", self.voice);
        Ok(())
    }

    async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        if !Self::espeak_on_path() {
            return Err(anyhow::anyhow!("espeak-ng not available"));
        }

        debug!("[TTS] Synthesizing with espeak-ng: {}", text);

        // espeak-ng --stdout outputs WAV to stdout
        let output = Command::new("espeak-ng")
            .arg("-v")
            .arg(&self.voice)
            .arg("--stdout")
            .arg(text)
            .output()
            .context("Failed to execute espeak-ng")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("espeak-ng failed: {}", stderr);
        }

        let audio_bytes = output.stdout;
        info!(
            "[TTS] espeak-ng generated {} bytes of audio",
            audio_bytes.len()
        );

        Ok(audio_bytes)
    }
}

impl Default for EspeakTts {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_espeak_availability() {
        let tts = EspeakTts::new();
        // Will depend on whether espeak-ng is installed
        let _ = tts.is_available();
    }
}
