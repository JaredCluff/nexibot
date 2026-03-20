//! Piper TTS backend (CLI wrapper)
//!
//! Piper is a fast, local neural text-to-speech system using ONNX models (~50MB).
//! Cross-platform: works on macOS, Windows, and Linux when the `piper` binary is on PATH.
//! Same CLI-wrapper pattern as `macos_say.rs`.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info};

use super::TtsBackend;

/// Piper neural TTS backend (CLI wrapper)
pub struct PiperTts {
    model_path: Option<PathBuf>,
    #[allow(dead_code)]
    voice: Option<String>,
}

impl PiperTts {
    /// Create a new Piper TTS backend with default settings
    pub fn new() -> Self {
        Self {
            model_path: None,
            voice: None,
        }
    }

    /// Create with a specific model path and voice
    pub fn with_config(model_path: Option<PathBuf>, voice: Option<String>) -> Self {
        Self { model_path, voice }
    }

    /// Check if piper binary is available on PATH
    // TODO: This uses std::process::Command which blocks. Called from async fns
    // initialize() and synthesize(). Wrap in tokio::task::spawn_blocking() or
    // switch to tokio::process::Command.
    fn piper_on_path() -> bool {
        Command::new("piper")
            .arg("--help")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get the default model directory
    #[allow(dead_code)]
    pub fn default_model_dir() -> Result<PathBuf> {
        let data_dir = directories::ProjectDirs::from("ai", "nexibot", "desktop")
            .ok_or_else(|| anyhow::anyhow!("Failed to determine project directories for ai.nexibot.desktop"))?
            .data_dir()
            .to_path_buf();
        Ok(data_dir.join("models").join("piper"))
    }
}

#[async_trait]
impl TtsBackend for PiperTts {
    fn name(&self) -> &str {
        "piper"
    }

    fn is_available(&self) -> bool {
        Self::piper_on_path()
    }

    async fn initialize(&mut self) -> Result<()> {
        if !Self::piper_on_path() {
            return Err(anyhow::anyhow!(
                "Piper TTS not found on PATH. Install from https://github.com/rhasspy/piper"
            ));
        }

        info!("[TTS] Piper TTS initialized");
        Ok(())
    }

    fn set_voice_model(&mut self, path: std::path::PathBuf) {
        self.model_path = Some(path);
    }

    async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        if !Self::piper_on_path() {
            return Err(anyhow::anyhow!("Piper TTS not available"));
        }

        debug!("[TTS] Synthesizing with Piper: {}", text);

        let model_path = self.model_path.clone();
        let text = text.to_string();

        // Wrap blocking process spawning and file I/O in spawn_blocking
        // to avoid blocking the async runtime.
        let audio_bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let temp_dir = std::env::temp_dir();
            let temp_file = temp_dir.join(format!("nexibot_piper_{}.wav", uuid::Uuid::new_v4()));

            let mut cmd = Command::new("piper");
            cmd.arg("--output_file").arg(&temp_file);

            // Add model path if configured
            if let Some(ref model_path) = model_path {
                cmd.arg("--model").arg(model_path);
            }

            // Pipe text via stdin
            cmd.stdin(std::process::Stdio::piped());

            let mut child = cmd.spawn().context("Failed to start Piper TTS")?;

            // Write text to stdin
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(text.as_bytes())
                    .context("Failed to write to Piper stdin")?;
            }

            let status = child.wait().context("Failed to wait for Piper")?;

            if !status.success() {
                anyhow::bail!("Piper TTS failed with status: {}", status);
            }

            // Set restrictive permissions on temp file (cross-platform)
            let _ = crate::platform::file_security::restrict_file_permissions(&temp_file);

            // Read generated WAV file
            let audio_bytes = std::fs::read(&temp_file).context("Failed to read Piper output")?;

            // Clean up
            let _ = std::fs::remove_file(&temp_file);

            Ok(audio_bytes)
        })
        .await
        .context("spawn_blocking panicked")?
        .context("Piper synthesis failed")?;

        info!("[TTS] Piper generated {} bytes of audio", audio_bytes.len());
        Ok(audio_bytes)
    }
}

impl Default for PiperTts {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_piper_availability() {
        let tts = PiperTts::new();
        // Will be false unless piper is installed
        let _ = tts.is_available();
    }

    #[test]
    fn test_default_model_dir() {
        let dir = PiperTts::default_model_dir().expect("should resolve project directories");
        assert!(dir.to_string_lossy().contains("piper"));
    }
}
