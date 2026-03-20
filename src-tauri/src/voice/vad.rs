//! Voice Activity Detection Module
//!
//! Detects speech in audio streams using Silero VAD via sherpa-onnx.
//! Cross-platform: works identically on macOS, Windows, and Linux.
//! Falls back to RMS energy-based detection if Silero model is unavailable.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Default VAD threshold (0.0-1.0)
const DEFAULT_THRESHOLD: f32 = 0.5;

/// Default minimum speech duration before considering it valid
const DEFAULT_MIN_SPEECH_DURATION_MS: u64 = 250;

/// Default minimum silence duration before considering speech ended
const DEFAULT_MIN_SILENCE_DURATION_MS: u64 = 500;

/// Silero VAD window size (number of samples per chunk)
const SILERO_WINDOW_SIZE: usize = 512;


/// Voice Activity Detector using Silero VAD (cross-platform)
pub struct VoiceActivityDetector {
    threshold: f32,
    min_speech_duration: Duration,
    min_silence_duration: Duration,
    speech_start: Option<Instant>,
    silence_start: Option<Instant>,
    is_speaking: bool,
    /// Latched flag: set when speech transitions to silence (cleared by reset())
    speech_ended: bool,
    /// Silero VAD instance (None if model not available, falls back to RMS)
    silero_vad: Option<sherpa_rs::silero_vad::SileroVad>,
    /// Whether we're using Silero (true) or RMS fallback (false)
    using_silero: bool,
    /// Tracks Silero inference count (atomic, for thread-safety)
    inference_count: AtomicU32,
}

impl VoiceActivityDetector {
    /// Create a new VAD with default settings
    pub fn new() -> Result<Self> {
        Self::with_settings(
            DEFAULT_THRESHOLD,
            DEFAULT_MIN_SPEECH_DURATION_MS,
            DEFAULT_MIN_SILENCE_DURATION_MS,
        )
    }

    /// Create a new VAD with custom settings
    ///
    /// # Arguments
    /// * `threshold` - Voice probability threshold (0.0-1.0)
    /// * `min_speech_duration_ms` - Minimum speech duration to trigger detection
    /// * `min_silence_duration_ms` - Minimum silence duration to end speech
    pub fn with_settings(
        threshold: f32,
        min_speech_duration_ms: u64,
        min_silence_duration_ms: u64,
    ) -> Result<Self> {
        Self::with_settings_and_require(
            threshold,
            min_speech_duration_ms,
            min_silence_duration_ms,
            false,
        )
    }

    /// Create a new VAD with custom settings and optional Silero requirement
    ///
    /// # Arguments
    /// * `threshold` - Voice probability threshold (0.0-1.0)
    /// * `min_speech_duration_ms` - Minimum speech duration to trigger detection
    /// * `min_silence_duration_ms` - Minimum silence duration to end speech
    /// * `require_silero` - If true, return Err instead of falling back to RMS when Silero fails
    pub fn with_settings_and_require(
        threshold: f32,
        min_speech_duration_ms: u64,
        min_silence_duration_ms: u64,
        require_silero: bool,
    ) -> Result<Self> {
        // Try to initialize Silero VAD
        let (silero_vad, using_silero) = match Self::init_silero(
            threshold,
            min_silence_duration_ms,
            min_speech_duration_ms,
        ) {
            Ok(vad) => {
                info!(
                    "[VAD] Initialized with Silero VAD (threshold={:.2}, min_speech={}ms, min_silence={}ms)",
                    threshold, min_speech_duration_ms, min_silence_duration_ms
                );
                (Some(vad), true)
            }
            Err(e) => {
                error!(
                    "[VAD] Silero VAD model failed to initialize: {}. Falling back to RMS energy detection.                     Voice accuracy will be significantly degraded (~30% false positive rate).                     Set vad.require_silero=true in config to prevent fallback.",
                    e
                );
                if require_silero {
                    return Err(anyhow::anyhow!(
                        "Silero VAD required but unavailable: {}.                         Set vad.require_silero=false to allow RMS fallback.",
                        e
                    ));
                }
                (None, false)
            }
        };

        info!(
            "[VAD] Using {} mode",
            if using_silero {
                "Silero ONNX"
            } else {
                "RMS energy fallback"
            }
        );

        Ok(Self {
            threshold,
            min_speech_duration: Duration::from_millis(min_speech_duration_ms),
            min_silence_duration: Duration::from_millis(min_silence_duration_ms),
            speech_start: None,
            silence_start: None,
            is_speaking: false,
            speech_ended: false,
            silero_vad,
            using_silero,
            inference_count: AtomicU32::new(0),
        })
    }

    /// Initialize Silero VAD model
    fn init_silero(
        threshold: f32,
        min_silence_duration_ms: u64,
        min_speech_duration_ms: u64,
    ) -> Result<sherpa_rs::silero_vad::SileroVad> {
        let model_path = Self::silero_model_path()?;

        if !model_path.exists() {
            anyhow::bail!("Silero VAD model not found at {:?}", model_path);
        }

        let config = sherpa_rs::silero_vad::SileroVadConfig {
            model: model_path.to_string_lossy().to_string(),
            threshold,
            min_silence_duration: min_silence_duration_ms as f32 / 1000.0,
            min_speech_duration: min_speech_duration_ms as f32 / 1000.0,
            max_speech_duration: f32::MAX,
            sample_rate: 16000,
            window_size: SILERO_WINDOW_SIZE as i32,
            ..Default::default()
        };

        // Buffer 30 seconds of audio
        let vad = sherpa_rs::silero_vad::SileroVad::new(config, 30.0)
            .map_err(|e| anyhow::anyhow!("Failed to create Silero VAD: {}", e))?;
        Ok(vad)
    }

    /// Get the expected path for the Silero VAD ONNX model
    pub fn silero_model_path() -> Result<PathBuf> {
        let data_dir = directories::ProjectDirs::from("ai", "nexibot", "desktop")
            .ok_or_else(|| anyhow::anyhow!("Failed to determine project directories for ai.nexibot.desktop"))?
            .data_dir()
            .to_path_buf();
        Ok(data_dir.join("models").join("silero_vad.onnx"))
    }

    /// Check if Silero VAD model is available on disk
    #[allow(dead_code)]
    pub fn is_silero_available() -> bool {
        Self::silero_model_path().map(|p| p.exists()).unwrap_or(false)
    }

    /// Process an audio chunk and return whether voice is detected
    ///
    /// # Arguments
    /// * `samples` - Audio samples (f32, 16kHz, mono)
    ///
    /// # Returns
    /// * `Ok(true)` - Voice detected
    /// * `Ok(false)` - Silence or noise only
    pub fn process_chunk(&mut self, samples: &[f32]) -> Result<bool> {
        let probability = if self.using_silero {
            match self.process_silero(samples) {
                Ok(p) => p,
                Err(e) => {
                    warn!(
                        "[VAD] Silero inference failed, falling back to RMS for this frame: {}",
                        e
                    );
                    Self::detect_voice_activity_rms(samples)
                }
            }
        } else {
            Self::detect_voice_activity_rms(samples)
        };

        debug!(
            "[VAD] Probability: {:.4} ({})",
            probability,
            if self.using_silero { "silero" } else { "rms" }
        );

        let has_voice = probability >= self.threshold;
        self.update_state(has_voice);

        Ok(self.is_speaking)
    }

    /// Process audio through Silero VAD
    fn process_silero(&mut self, samples: &[f32]) -> Result<f32> {
        if samples.is_empty() {
            return Ok(0.0);
        }

        let vad = self
            .silero_vad
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Silero VAD not initialized"))?;

        // Sanitize input: replace NaN/Inf with 0.0, clamp to [-1.0, 1.0]
        let sanitized: Vec<f32> = samples
            .iter()
            .map(|&s| {
                if s.is_finite() {
                    s.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect();

        // Increment inference counter atomically (relaxed: only used as a diagnostic counter)
        self.inference_count.fetch_add(1, Ordering::Relaxed);

        vad.accept_waveform(sanitized);

        // Check if speech is currently detected
        if vad.is_speech() {
            Ok(1.0)
        } else {
            Ok(0.0)
        }
    }

    /// RMS energy-based voice activity detection (fallback)
    fn detect_voice_activity_rms(audio: &[f32]) -> f32 {
        let rms = Self::calculate_rms(audio);

        // Simple threshold: audio with RMS > 0.01 is considered voice
        // Scale and clamp to 0-1 range
        if rms > 0.01 {
            (rms * 10.0).min(1.0)
        } else {
            0.0
        }
    }

    /// Calculate Root Mean Square energy of audio samples
    fn calculate_rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }

        let sum_squares: f32 = samples.iter().map(|&s| s * s).sum();
        (sum_squares / samples.len() as f32).sqrt()
    }

    /// Update internal state based on voice detection
    fn update_state(&mut self, has_voice: bool) {
        let now = Instant::now();

        if has_voice {
            // Voice detected
            if self.speech_start.is_none() {
                self.speech_start = Some(now);
                debug!("[VAD] Speech started");
            }
            self.silence_start = None;

            // Mark as speaking if speech duration exceeds minimum
            if let Some(start) = self.speech_start {
                if now.duration_since(start) >= self.min_speech_duration && !self.is_speaking {
                    self.is_speaking = true;
                    debug!("[VAD] Speaking confirmed");
                }
            }
        } else {
            // Silence detected
            if self.is_speaking && self.silence_start.is_none() {
                self.silence_start = Some(now);
                debug!("[VAD] Silence started");
            }

            // Mark as not speaking if silence duration exceeds minimum
            if let Some(silence_start) = self.silence_start {
                if now.duration_since(silence_start) >= self.min_silence_duration {
                    self.is_speaking = false;
                    self.speech_ended = true;
                    self.speech_start = None;
                    self.silence_start = None;
                    debug!("[VAD] Speaking ended (speech_ended latched)");
                }
            }

            // Reset speech start if we haven't confirmed speaking yet
            if self.speech_start.is_some() && !self.is_speaking {
                self.speech_start = None;
            }
        }
    }

    /// Check if currently in speech state
    #[allow(dead_code)]
    pub fn is_speaking(&self) -> bool {
        self.is_speaking
    }

    /// Check if using Silero VAD (true) or RMS fallback (false)
    #[allow(dead_code)]
    pub fn is_using_silero(&self) -> bool {
        self.using_silero
    }

    /// Check if end-of-speech silence has been detected.
    /// Returns true once speech transitions to silence (latched until reset()).
    pub fn is_silence_detected(&self) -> bool {
        self.speech_ended
    }

    /// Reset VAD state
    pub fn reset(&mut self) {
        self.speech_start = None;
        self.silence_start = None;
        self.is_speaking = false;
        self.speech_ended = false;
        self.inference_count.store(0, Ordering::Relaxed);
        if let Some(ref mut vad) = self.silero_vad {
            vad.clear();
        }
        debug!("[VAD] State reset");
    }

    /// Update threshold
    #[allow(dead_code)]
    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.clamp(0.0, 1.0);
    }

    /// Get current threshold
    #[allow(dead_code)]
    pub fn get_threshold(&self) -> f32 {
        self.threshold
    }

    /// Reset the Silero VAD LSTM hidden state.
    /// Call this when transitioning from Processing → Idle in the voice pipeline
    /// state machine to prevent LSTM state from carrying over between utterances.
    /// Do NOT call mid-stream — only between utterances.
    #[allow(dead_code)]
    pub fn reset_lstm_state(&mut self) {
        if let Some(ref mut vad) = self.silero_vad {
            vad.clear();
            debug!("[VAD] LSTM state reset (between utterances)");
        }
    }

}

impl Default for VoiceActivityDetector {
    fn default() -> Self {
        match Self::new() {
            Ok(vad) => vad,
            Err(e) => {
                warn!("[VAD] Failed to create VAD with default settings, falling back to RMS-only: {}", e);
                Self {
                    threshold: DEFAULT_THRESHOLD,
                    min_speech_duration: Duration::from_millis(DEFAULT_MIN_SPEECH_DURATION_MS),
                    min_silence_duration: Duration::from_millis(DEFAULT_MIN_SILENCE_DURATION_MS),
                    speech_start: None,
                    silence_start: None,
                    is_speaking: false,
                    speech_ended: false,
                    silero_vad: None,
                    using_silero: false,
                    inference_count: AtomicU32::new(0),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vad_creation() {
        let vad = VoiceActivityDetector::with_settings(0.5, 250, 500);
        assert!(vad.is_ok());
    }

    #[test]
    fn test_vad_process_silence() {
        let mut vad = VoiceActivityDetector::with_settings(0.5, 250, 500).unwrap();
        let silence = vec![0.0f32; 16000];
        let result = vad.process_chunk(&silence).unwrap();
        assert!(!result, "Silence should not be detected as voice");
    }

    #[test]
    fn test_vad_threshold() {
        let mut vad = VoiceActivityDetector::new().unwrap();
        vad.set_threshold(0.8);
        assert_eq!(vad.get_threshold(), 0.8);

        // Test clamping
        vad.set_threshold(1.5);
        assert_eq!(vad.get_threshold(), 1.0);

        vad.set_threshold(-0.5);
        assert_eq!(vad.get_threshold(), 0.0);
    }

    #[test]
    fn test_rms_calculation() {
        // Silence
        assert_eq!(VoiceActivityDetector::calculate_rms(&[0.0; 100]), 0.0);

        // Known signal
        let samples = vec![0.5f32; 100];
        let rms = VoiceActivityDetector::calculate_rms(&samples);
        assert!((rms - 0.5).abs() < 0.001);

        // Empty
        assert_eq!(VoiceActivityDetector::calculate_rms(&[]), 0.0);
    }

    #[test]
    fn test_rms_fallback_detection() {
        // Silence should return 0
        let prob = VoiceActivityDetector::detect_voice_activity_rms(&[0.0; 100]);
        assert_eq!(prob, 0.0);

        // Loud signal should return > 0
        let prob = VoiceActivityDetector::detect_voice_activity_rms(&[0.5; 100]);
        assert!(prob > 0.0);
    }
}
