//! Text-to-Speech Module
//!
//! Provides a trait-based interface for multiple TTS backends.
//! Supports native OS speech, Piper (local), espeak-ng (Linux), and cloud APIs.

pub mod cloud;
#[cfg(target_os = "linux")]
pub mod espeak;
#[cfg(target_os = "macos")]
pub mod macos_say;
pub mod piper;
#[cfg(target_os = "windows")]
pub mod windows_sapi;

use anyhow::Result;
use async_trait::async_trait;

pub use cloud::{CartesiaTts, ElevenLabsTts};
#[cfg(target_os = "macos")]
pub use macos_say::MacOsSayTts;
pub use piper::PiperTts;
#[cfg(target_os = "windows")]
pub use windows_sapi::WindowsSapiTts;

/// TTS backend trait
#[async_trait]
pub trait TtsBackend: Send + Sync {
    /// Get backend name
    fn name(&self) -> &str;

    /// Check if backend is available on this system
    fn is_available(&self) -> bool;

    /// Initialize the backend (load models, check API keys, etc.)
    #[allow(dead_code)]
    async fn initialize(&mut self) -> Result<()>;

    /// Synthesize text to speech
    ///
    /// # Arguments
    /// * `text` - Text to synthesize
    ///
    /// # Returns
    /// * `Ok(Vec<u8>)` - Audio bytes (format depends on backend)
    async fn synthesize(&self, text: &str) -> Result<Vec<u8>>;

    /// Hot-swap the underlying voice model file.
    ///
    /// Only meaningful for file-based backends like Piper.  All other backends
    /// provide a default no-op implementation.
    fn set_voice_model(&mut self, _path: std::path::PathBuf) {}
}

/// TTS manager that handles backend selection and fallback
pub struct TtsManager {
    backends: Vec<Box<dyn TtsBackend>>,
    current_backend: String,
}

impl TtsManager {
    /// Create a new TTS manager
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
            current_backend: String::new(),
        }
    }

    /// Register a TTS backend
    pub fn register_backend(&mut self, backend: Box<dyn TtsBackend>) {
        self.backends.push(backend);
    }

    /// Clear all registered backends (used during hot-reload reinit)
    pub fn clear_backends(&mut self) {
        self.backends.clear();
        self.current_backend.clear();
    }

    /// Set the active backend by name
    pub fn set_backend(&mut self, name: &str) -> Result<()> {
        // Check if backend exists
        if self.backends.iter().any(|b| b.name() == name) {
            self.current_backend = name.to_string();
            Ok(())
        } else {
            Err(anyhow::anyhow!("TTS backend '{}' not found", name))
        }
    }

    /// Get current backend name
    pub fn get_backend_name(&self) -> &str {
        &self.current_backend
    }

    /// Synthesize text using the current backend, with automatic fallback.
    ///
    /// Tries the configured backend first. If that backend fails (either
    /// unavailable on this platform or a runtime error), falls through to the
    /// remaining backends in registration order (highest-priority first, as
    /// established by `register_tts_backends`). This ensures a transient cloud
    /// API failure, a missing Piper model, or any other runtime error
    /// automatically degrades to the next viable backend rather than surfacing
    /// an error to the caller.
    pub async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        // Attempt the configured/preferred backend first.
        if let Some(backend) = self
            .backends
            .iter()
            .find(|b| b.name() == self.current_backend)
        {
            if backend.is_available() {
                match backend.synthesize(text).await {
                    Ok(audio) => return Ok(audio),
                    Err(e) => {
                        tracing::warn!(
                            "[TTS] Preferred backend '{}' failed: {}. Falling back through priority chain.",
                            self.current_backend, e
                        );
                    }
                }
            } else {
                tracing::warn!(
                    "[TTS] Preferred backend '{}' is not available on this platform. Falling back through priority chain.",
                    self.current_backend
                );
            }
        } else {
            tracing::warn!(
                "[TTS] Preferred backend '{}' not registered. Falling back through priority chain.",
                self.current_backend
            );
        }

        // Walk remaining backends in registration order (priority chain).
        for backend in &self.backends {
            if backend.name() == self.current_backend {
                // Already tried above.
                continue;
            }
            if !backend.is_available() {
                continue;
            }
            match backend.synthesize(text).await {
                Ok(audio) => {
                    tracing::info!("[TTS] Fallback backend '{}' succeeded.", backend.name());
                    return Ok(audio);
                }
                Err(e) => {
                    tracing::warn!("[TTS] Fallback backend '{}' failed: {}", backend.name(), e);
                }
            }
        }

        Err(anyhow::anyhow!(
            "All TTS backends failed (preferred: '{}'). No audio produced.",
            self.current_backend
        ))
    }

    /// Hot-swap the Piper voice model for language auto-detection.
    ///
    /// Calls `set_voice_model()` on every registered backend; only Piper
    /// does anything (the default impl on other backends is a no-op).
    pub fn set_piper_model(&mut self, path: std::path::PathBuf) {
        for backend in self.backends.iter_mut() {
            if backend.name() == "piper" {
                backend.set_voice_model(path.clone());
                tracing::info!("[TTS] Piper voice model updated to {:?}", path);
                return;
            }
        }
        tracing::debug!("[TTS] set_piper_model: Piper backend not registered, skipping");
    }

    /// Try backends in priority order until one succeeds
    #[allow(dead_code)]
    pub async fn synthesize_with_fallback(
        &self,
        text: &str,
        priority: &[String],
    ) -> Result<Vec<u8>> {
        for backend_name in priority {
            if let Some(backend) = self.backends.iter().find(|b| b.name() == backend_name) {
                if backend.is_available() {
                    match backend.synthesize(text).await {
                        Ok(audio) => return Ok(audio),
                        Err(e) => {
                            tracing::warn!("[TTS] Backend '{}' failed: {}", backend_name, e);
                            continue;
                        }
                    }
                }
            }
        }

        Err(anyhow::anyhow!("All TTS backends failed"))
    }
}

impl Default for TtsManager {
    fn default() -> Self {
        Self::new()
    }
}
