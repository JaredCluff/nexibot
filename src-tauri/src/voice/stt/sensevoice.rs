//! SenseVoice STT backend via sherpa-rs
//!
//! SenseVoice is 15x faster than Whisper, non-autoregressive (no hallucinations),
//! processes 10s of audio in ~70ms. Available as ONNX model via sherpa-onnx.
//! Cross-platform: works on macOS, Windows, and Linux.

use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing::{debug, error, info};

use super::SttBackend;

/// SenseVoice local STT backend
pub struct SenseVoiceStt {
    initialized: bool,
    model_path: Option<PathBuf>,
    recognizer: Mutex<Option<sherpa_rs::sense_voice::SenseVoiceRecognizer>>,
}

impl SenseVoiceStt {
    /// Create a new SenseVoice STT backend
    pub fn new() -> Self {
        Self {
            initialized: false,
            model_path: None,
            recognizer: Mutex::new(None),
        }
    }

    /// Create with a custom model path
    pub fn with_model_path(model_path: PathBuf) -> Self {
        Self {
            initialized: false,
            model_path: Some(model_path),
            recognizer: Mutex::new(None),
        }
    }

    /// Get the default model directory
    pub fn default_model_dir() -> Result<PathBuf> {
        let data_dir = directories::ProjectDirs::from("ai", "nexibot", "desktop")
            .ok_or_else(|| anyhow::anyhow!("Failed to determine project directories for ai.nexibot.desktop"))?
            .data_dir()
            .to_path_buf();
        Ok(data_dir.join("models").join("sensevoice"))
    }

    /// Get the model file path
    fn get_model_path(&self) -> Result<PathBuf> {
        match &self.model_path {
            Some(path) => Ok(path.clone()),
            None => Ok(Self::default_model_dir()?.join("model.onnx")),
        }
    }

    /// Check if the model file exists
    fn model_exists(&self) -> bool {
        self.get_model_path().map(|p| p.exists()).unwrap_or(false)
    }
}

#[async_trait]
impl SttBackend for SenseVoiceStt {
    fn name(&self) -> &str {
        "sensevoice"
    }

    fn is_available(&self) -> bool {
        self.model_exists()
    }

    async fn initialize(&mut self) -> Result<()> {
        let model_path = self.get_model_path()?;

        if !model_path.exists() {
            return Err(anyhow::anyhow!(
                "SenseVoice model not found at {:?}. Download the ONNX model to this location.",
                model_path
            ));
        }

        let tokens_path = model_path
            .parent()
            .unwrap_or(model_path.as_path())
            .join("tokens.txt");

        if !tokens_path.exists() {
            return Err(anyhow::anyhow!(
                "SenseVoice tokens file not found at {:?}",
                tokens_path
            ));
        }

        let ort_config = sherpa_rs::sense_voice::SenseVoiceConfig {
            model: model_path.to_string_lossy().to_string(),
            tokens: tokens_path.to_string_lossy().to_string(),
            language: "auto".to_string(),
            use_itn: true,
            ..Default::default()
        };

        // SenseVoiceRecognizer::new() performs blocking ONNX model loading.
        // Run it on the blocking thread pool to avoid stalling the async runtime.
        let recognizer = tokio::task::spawn_blocking(move || {
            sherpa_rs::sense_voice::SenseVoiceRecognizer::new(ort_config)
                .map_err(|e| anyhow::anyhow!("Failed to create SenseVoice recognizer: {}", e))
        })
        .await
        .map_err(|e| anyhow::anyhow!("SenseVoice init task panicked: {}", e))??;

        *self.recognizer.lock().unwrap_or_else(|e| {
            error!("[STT] SenseVoice recognizer mutex poisoned during initialize: {}", e);
            e.into_inner()
        }) = Some(recognizer);
        self.initialized = true;

        info!(
            "[STT] SenseVoice initialized with model: {:?}",
            self.get_model_path().unwrap_or_default()
        );
        Ok(())
    }

    // TODO: transcribe() calls blocking ONNX inference (recognizer.transcribe()) inside
    // an async fn. Wrap in tokio::task::spawn_blocking() when feasible (&self borrow
    // and Mutex make this non-trivial without Arc refactoring).
    async fn transcribe(&self, audio: &[f32]) -> Result<String> {
        if !self.initialized {
            return Err(anyhow::anyhow!("SenseVoice not initialized"));
        }

        debug!(
            "[STT] Transcribing {} audio samples with SenseVoice",
            audio.len()
        );

        let mut recognizer = self.recognizer.lock().unwrap_or_else(|e| {
            error!("[STT] SenseVoice recognizer mutex poisoned during transcribe: {}", e);
            e.into_inner()
        });
        let recognizer = recognizer
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("SenseVoice recognizer not available"))?;

        let result = recognizer.transcribe(16000, audio);

        info!("[STT] SenseVoice transcribed: {}", result.text);
        Ok(result.text)
    }
}

impl Default for SenseVoiceStt {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensevoice_availability() {
        let stt = SenseVoiceStt::new();
        // Will be false unless model is downloaded
        let _ = stt.is_available();
    }

    #[test]
    fn test_default_model_dir() {
        let dir = SenseVoiceStt::default_model_dir().expect("should resolve project directories");
        assert!(dir.to_string_lossy().contains("sensevoice"));
    }
}
