//! Speech-to-Text Module
//!
//! Provides a trait-based interface for multiple STT backends.
//! Supports native OS speech, SenseVoice (local), and cloud APIs (Deepgram, OpenAI).

pub mod cloud;
pub mod deepgram_rate_limiter;
#[cfg(target_os = "macos")]
pub mod macos_speech;
pub mod sensevoice;
#[cfg(target_os = "windows")]
pub mod windows_speech;

use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;

pub use cloud::{DeepgramStt, OpenAIStt};
pub use deepgram_rate_limiter::DeepgramRateLimiter;
#[cfg(target_os = "macos")]
pub use macos_speech::MacOsSpeechStt;
pub use sensevoice::SenseVoiceStt;
#[cfg(target_os = "windows")]
pub use windows_speech::WindowsSpeechStt;

/// STT backend trait
#[async_trait]
pub trait SttBackend: Send + Sync {
    /// Get backend name
    fn name(&self) -> &str;

    /// Check if backend is available on this system
    fn is_available(&self) -> bool;

    /// Initialize the backend (load models, check API keys, etc.)
    async fn initialize(&mut self) -> Result<()>;

    /// Transcribe audio to text
    ///
    /// # Arguments
    /// * `audio` - Audio samples (f32, 16kHz, mono)
    ///
    /// # Returns
    /// * `Ok(String)` - Transcribed text
    async fn transcribe(&self, audio: &[f32]) -> Result<String>;
}

/// STT manager that handles backend selection and fallback
pub struct SttManager {
    backends: Vec<Box<dyn SttBackend>>,
    current_backend: String,
}

impl SttManager {
    /// Create a new STT manager
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
            current_backend: String::new(),
        }
    }

    /// Register an STT backend
    pub fn register_backend(&mut self, backend: Box<dyn SttBackend>) {
        self.backends.push(backend);
    }

    /// Clear all registered backends (used during hot-reload reinit)
    pub fn clear_backends(&mut self) {
        self.backends.clear();
        self.current_backend.clear();
    }

    /// Set the active backend by name and initialize it
    pub async fn set_backend(&mut self, name: &str) -> Result<()> {
        // Find and initialize the backend
        let backend = self
            .backends
            .iter_mut()
            .find(|b| b.name() == name)
            .ok_or_else(|| anyhow::anyhow!("STT backend '{}' not found", name))?;

        if let Err(e) = backend.initialize().await {
            tracing::warn!("[STT] Backend '{}' initialization failed: {}", name, e);
        }

        self.current_backend = name.to_string();
        Ok(())
    }

    /// Get current backend name
    pub fn get_backend_name(&self) -> &str {
        &self.current_backend
    }

    /// Transcribe audio using the current backend
    pub async fn transcribe(&self, audio: &[f32]) -> Result<String> {
        // Find current backend
        let backend = self
            .backends
            .iter()
            .find(|b| b.name() == self.current_backend)
            .ok_or_else(|| {
                anyhow::anyhow!("Current backend '{}' not found", self.current_backend)
            })?;

        // Check if available
        if !backend.is_available() {
            return Err(anyhow::anyhow!(
                "STT backend '{}' is not available on this system",
                backend.name()
            ));
        }

        // Transcribe with 15-second timeout
        match tokio::time::timeout(Duration::from_secs(15), backend.transcribe(audio)).await {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!("[STT] Backend '{}' timed out after 15s", backend.name());
                Err(anyhow::anyhow!(
                    "STT backend '{}' timed out",
                    self.current_backend
                ))
            }
        }
    }

    /// Try backends in priority order until one succeeds
    #[allow(dead_code)]
    pub async fn transcribe_with_fallback(
        &self,
        audio: &[f32],
        priority: &[String],
    ) -> Result<String> {
        for backend_name in priority {
            if let Some(backend) = self.backends.iter().find(|b| b.name() == backend_name) {
                if backend.is_available() {
                    match tokio::time::timeout(Duration::from_secs(15), backend.transcribe(audio))
                        .await
                    {
                        Ok(Ok(text)) => return Ok(text),
                        Ok(Err(e)) => {
                            tracing::warn!("[STT] Backend '{}' failed: {}", backend_name, e);
                            continue;
                        }
                        Err(_) => {
                            tracing::warn!(
                                "[STT] Backend '{}' timed out after 15s, trying next",
                                backend_name
                            );
                            continue;
                        }
                    }
                }
            }
        }

        Err(anyhow::anyhow!("All STT backends failed"))
    }
}

impl Default for SttManager {
    fn default() -> Self {
        Self::new()
    }
}
