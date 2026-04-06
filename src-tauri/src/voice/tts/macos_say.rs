//! macOS say command TTS backend
//!
//! Uses the built-in macOS `say` command for text-to-speech.
//! Zero-config, works offline, instant availability.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::process::Command;
use tracing::{debug, info};
use tokio::task;

use super::TtsBackend;
use crate::platform;

/// macOS say command TTS backend
pub struct MacOsSayTts {
    voice: String,
    rate: u32,
}

impl MacOsSayTts {
    /// Create a new macOS say TTS backend
    ///
    /// # Arguments
    /// * `voice` - Voice name (e.g., "Samantha", "Alex", "Zoe")
    /// * `rate` - Speech rate in words per minute (default: 200)
    pub fn new(voice: String, rate: u32) -> Self {
        Self { voice, rate }
    }

    /// Get list of available voices on the system
    pub fn list_voices() -> Result<Vec<String>> {
        if !platform::is_macos() {
            return Ok(Vec::new());
        }

        let output = Command::new("say")
            .arg("-v")
            .arg("?")
            .output()
            .context("Failed to list voices")?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("say command failed"));
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let voices: Vec<String> = output_str
            .lines()
            .filter_map(|line| {
                // Each line is like: "Alex                en_US    # Most people recognize me by my voice."
                line.split_whitespace().next().map(|s| s.to_string())
            })
            .collect();

        Ok(voices)
    }
}

#[async_trait]
impl TtsBackend for MacOsSayTts {
    fn name(&self) -> &str {
        "macos_say"
    }

    fn is_available(&self) -> bool {
        platform::is_macos()
    }

    async fn initialize(&mut self) -> Result<()> {
        if !self.is_available() {
            return Err(anyhow::anyhow!(
                "macOS say command not available (not on macOS)"
            ));
        }

        // Test if say command works
        let test = Command::new("say")
            .arg("-v")
            .arg("?")
            .output()
            .context("Failed to test say command")?;

        if !test.status.success() {
            return Err(anyhow::anyhow!("say command not working"));
        }

        info!(
            "[TTS] macOS say initialized with voice: {}, rate: {}",
            self.voice, self.rate
        );
        Ok(())
    }

    async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        if !self.is_available() {
            return Err(anyhow::anyhow!("macOS say not available"));
        }

        debug!("[TTS] Synthesizing with macOS say: {}", text);

        let voice = self.voice.clone();
        let rate = self.rate;
        let text_owned = text.to_string();

        // Run in spawn_blocking: `say` is a synchronous child process that can take
        // several seconds; blocking the async executor would stall voice pipeline tasks.
        let audio_bytes = task::spawn_blocking(move || -> Result<Vec<u8>> {
            // Create temp file for audio output (WAV format for rodio compatibility)
            let temp_dir = std::env::temp_dir();
            let temp_file = temp_dir.join(format!("nexibot_tts_{}.wav", uuid::Uuid::new_v4()));

            // Call say command to generate WAV audio file directly
            // Using --file-format=WAVE and --data-format=LEI16@16000 for 16-bit PCM WAV at 16kHz
            let status = Command::new("say")
                .arg("-v")
                .arg(&voice)
                .arg("-r")
                .arg(rate.to_string())
                .arg("-o")
                .arg(&temp_file)
                .arg("--file-format=WAVE")
                .arg("--data-format=LEI16@16000")
                .arg(&text_owned)
                .status()
                .context("Failed to execute say command")?;

            if !status.success() {
                return Err(anyhow::anyhow!(
                    "say command failed with status: {}",
                    status
                ));
            }

            // Read the generated audio file
            let bytes =
                std::fs::read(&temp_file).context("Failed to read generated audio file")?;

            // Clean up temp file
            let _ = std::fs::remove_file(&temp_file);

            Ok(bytes)
        })
        .await
        .context("TTS task panicked")??;

        info!("[TTS] Generated {} bytes of audio", audio_bytes.len());

        Ok(audio_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_macos_say_synthesis() {
        if !platform::is_macos() {
            return;
        }

        let mut tts = MacOsSayTts::new("Samantha".to_string(), 200);
        tts.initialize().await.unwrap();

        let audio = tts.synthesize("Hello, world!").await.unwrap();
        assert!(!audio.is_empty());
    }

    #[test]
    fn test_list_voices() {
        if !platform::is_macos() {
            return;
        }

        let voices = MacOsSayTts::list_voices().unwrap();
        assert!(!voices.is_empty());
        // Samantha is the default en_US voice on all macOS installs
        assert!(
            voices.contains(&"Samantha".to_string()),
            "Expected Samantha voice, got: {:?}",
            &voices[..voices.len().min(10)]
        );
    }
}
