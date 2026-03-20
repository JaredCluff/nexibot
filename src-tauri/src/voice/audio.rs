//! Audio Capture Module (adapted from production)
//!
//! Captures audio from the microphone for wake word detection and speech recognition.
//! Uses cpal for cross-platform audio input, integrates with VAD to filter noise.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};

use super::vad::VoiceActivityDetector;
use super::wakeword::WakeWordDetector;

/// Target sample rate for wake word detection (16kHz)
const TARGET_SAMPLE_RATE: u32 = 16000;

/// Audio frame size for wake word detection (80ms chunks)
const FRAME_SIZE: usize = 1280; // 16000 * 0.08 = 1280 samples

/// Maximum audio buffer samples to prevent unbounded memory growth (5 minutes at 16kHz)
const MAX_AUDIO_BUFFER_SAMPLES: usize = 16_000 * 300;

/// Audio capture service
#[derive(Clone)]
pub struct AudioCapture {
    /// Whether audio capture is running
    running: Arc<AtomicBool>,
    /// Audio buffer for accumulating samples
    audio_buffer: Arc<Mutex<Vec<f32>>>,
}

impl AudioCapture {
    /// Create a new audio capture service
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            audio_buffer: Arc::new(Mutex::new(Vec::with_capacity(FRAME_SIZE * 2))),
        }
    }

    /// Start audio capture
    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.running.load(Ordering::SeqCst) {
            warn!("[AUDIO] Already running");
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);
        info!("[AUDIO] Audio capture started");
        Ok(())
    }

    /// Stop audio capture
    pub fn stop(&mut self) -> anyhow::Result<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("[AUDIO] Audio capture stopped");
        Ok(())
    }

    /// Check if audio capture is running
    pub fn is_recording(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get accumulated audio samples (for STT processing)
    #[allow(dead_code)]
    pub fn get_samples(&self) -> anyhow::Result<Vec<f32>> {
        let buffer = self
            .audio_buffer
            .lock()
            .map_err(|e| anyhow::anyhow!("Audio buffer mutex poisoned: {}", e))?;
        Ok(buffer.clone())
    }

    /// Take (drain) accumulated audio samples — returns and clears the buffer.
    /// Use this in the Listening loop to avoid re-reading the same samples.
    pub fn take_samples(&self) -> anyhow::Result<Vec<f32>> {
        let mut buffer = self
            .audio_buffer
            .lock()
            .map_err(|e| anyhow::anyhow!("Audio buffer mutex poisoned: {}", e))?;
        Ok(buffer.drain(..).collect())
    }

    /// Push audio samples into the buffer (called from the audio capture thread).
    /// Only accumulates when `is_recording()` is true (Listening state).
    pub fn push_samples(&self, samples: &[f32]) {
        if self.running.load(Ordering::SeqCst) {
            if let Ok(mut buffer) = self.audio_buffer.lock() {
                if buffer.len() + samples.len() > MAX_AUDIO_BUFFER_SAMPLES {
                    warn!(
                        "[AUDIO] STT buffer full, dropping {} samples",
                        samples.len()
                    );
                    return;
                }
                buffer.extend_from_slice(samples);
            }
        }
    }

    /// Clear accumulated audio samples
    pub fn clear_samples(&self) -> anyhow::Result<()> {
        let mut buffer = self
            .audio_buffer
            .lock()
            .map_err(|e| anyhow::anyhow!("Audio buffer mutex poisoned: {}", e))?;
        buffer.clear();
        Ok(())
    }
}

impl Default for AudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

/// Run audio capture loop with VAD and wake word detection.
/// The `stt_capture` parameter bridges audio to the voice service for STT:
/// when `stt_capture.is_recording()` is true (Listening state), samples are
/// forwarded to its buffer for speech-to-text processing.
pub fn run_audio_capture_loop(
    vad_enabled: bool,
    vad_threshold: f32,
    vad_require_silero: bool,
    wakeword_enabled: bool,
    sleep_timeout_seconds: u64,
    wake_word: String,
    wakeword_threshold: f32,
    stt_wakeword_enabled: bool,
    stt_capture: AudioCapture,
    suppress_wakeword: Arc<AtomicBool>,
    exit_signal: Arc<AtomicBool>,
    on_wake_word: impl Fn(f32, Vec<f32>) + Send + Sync + 'static,
) -> anyhow::Result<()> {
    let host = cpal::default_host();

    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No input device available"))?;

    info!(
        "[AUDIO] Using input device: {}",
        device.name().unwrap_or_default()
    );

    // Get supported config
    let supported_config = device
        .supported_input_configs()?
        .find(|c| c.sample_format() == SampleFormat::F32)
        .or_else(|| device.supported_input_configs().ok()?.next())
        .ok_or_else(|| anyhow::anyhow!("No supported input config"))?;

    // Prefer 16kHz if supported, otherwise use whatever is available
    let config: StreamConfig = if supported_config.min_sample_rate().0 <= TARGET_SAMPLE_RATE
        && supported_config.max_sample_rate().0 >= TARGET_SAMPLE_RATE
    {
        StreamConfig {
            channels: 1,
            sample_rate: SampleRate(TARGET_SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        }
    } else {
        StreamConfig {
            channels: supported_config.channels().min(2),
            sample_rate: supported_config.max_sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        }
    };

    info!(
        "[AUDIO] Config: {} Hz, {} channels",
        config.sample_rate.0, config.channels
    );

    // Initialize VAD if enabled
    let mut vad = if vad_enabled {
        match VoiceActivityDetector::with_settings_and_require(
            vad_threshold,
            250,
            500,
            vad_require_silero,
        ) {
            Ok(v) => {
                info!("[AUDIO] VAD initialized");
                Some(v)
            }
            Err(e) => {
                if vad_require_silero {
                    return Err(e);
                }
                warn!("[AUDIO] VAD unavailable: {}. Continuing without VAD.", e);
                None
            }
        }
    } else {
        None
    };

    // Initialize wake word detector if enabled
    let mut detector = if wakeword_enabled {
        match WakeWordDetector::new(
            sleep_timeout_seconds,
            Some(&wake_word),
            Some(wakeword_threshold),
        ) {
            Ok(d) => {
                info!("[AUDIO] Wake word detector initialized for '{}'", wake_word);
                Some(d)
            }
            Err(e) => {
                warn!("[AUDIO] Wake word detector unavailable: {}.", e);
                None
            }
        }
    } else {
        None
    };

    // STT-based wake word detection is disabled: CLAUDE.md requires AUDIO-BASED detection only
    // (OpenWakeWord ONNX). The stt_wakeword_enabled flag is intentionally ignored here —
    // transcript-based fallback detection is prohibited. ONNX detection always runs when
    // wakeword_enabled is true, regardless of stt_wakeword_enabled.
    let _ = stt_wakeword_enabled; // consumed to silence unused-variable lint

    // Audio buffer for accumulating samples
    let audio_buffer = Arc::new(Mutex::new(Vec::with_capacity(FRAME_SIZE * 2)));
    let buffer_clone = audio_buffer.clone();
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    // Sample rate conversion ratio if needed
    let sample_rate = config.sample_rate.0;
    let channels = config.channels as usize;
    let resample_ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;
    let needs_resample = sample_rate != TARGET_SAMPLE_RATE;

    if needs_resample {
        info!(
            "[AUDIO] Will resample from {} Hz to {} Hz",
            sample_rate, TARGET_SAMPLE_RATE
        );
    }

    // Build input stream
    let stream = device.build_input_stream(
        &config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if !running_clone.load(Ordering::SeqCst) {
                return;
            }

            // Convert to mono if stereo
            let mono_samples: Vec<f32> = if channels > 1 {
                data.chunks(channels)
                    .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
                    .collect()
            } else {
                data.to_vec()
            };

            // Resample if needed (simple linear interpolation)
            let processed: Vec<f32> = if needs_resample {
                let output_len = (mono_samples.len() as f64 * resample_ratio) as usize;
                let mut output = Vec::with_capacity(output_len);
                for i in 0..output_len {
                    let src_idx = i as f64 / resample_ratio;
                    let idx = src_idx as usize;
                    let frac = src_idx - idx as f64;
                    let sample = if idx + 1 < mono_samples.len() {
                        mono_samples[idx] * (1.0 - frac as f32)
                            + mono_samples[idx + 1] * frac as f32
                    } else if idx < mono_samples.len() {
                        mono_samples[idx]
                    } else {
                        0.0
                    };
                    output.push(sample);
                }
                output
            } else {
                mono_samples
            };

            // Sanitize: replace NaN/Inf with 0.0 (defense-in-depth against
            // hardware glitches or resampling edge cases before reaching VAD)
            let processed: Vec<f32> = processed
                .into_iter()
                .map(|s| if s.is_finite() { s } else { 0.0 })
                .collect();

            // Add to buffer
            if let Ok(mut buffer) = buffer_clone.lock() {
                if buffer.len() + processed.len() > MAX_AUDIO_BUFFER_SAMPLES {
                    warn!(
                        "[AUDIO] Capture buffer full, dropping {} samples",
                        processed.len()
                    );
                    return;
                }
                buffer.extend(processed);
            }
        },
        move |err| {
            error!("[AUDIO] Stream error: {}", err);
        },
        None,
    )?;

    stream.play()?;
    info!("[AUDIO] Audio stream started");

    // Diagnostics counters
    let mut total_frames: u64 = 0;
    let mut vad_passed: u64 = 0;
    let mut vad_blocked: u64 = 0;

    // Rolling audio buffer for two-stage wake word confirmation (last 3 seconds)
    const CONFIRM_BUFFER_SIZE: usize = 16000 * 3; // 3 seconds at 16kHz
    let mut confirm_buffer: Vec<f32> = Vec::with_capacity(CONFIRM_BUFFER_SIZE);

    // Main processing loop
    loop {
        // Check for shutdown signal
        if exit_signal.load(Ordering::SeqCst) {
            info!("[AUDIO] Exit signal received, shutting down audio capture loop");
            break Ok(());
        }

        // Process buffered audio
        let samples_to_process: Option<Vec<f32>> = {
            match audio_buffer.lock() {
                Ok(mut buffer) => {
                    if buffer.len() >= FRAME_SIZE {
                        let samples: Vec<f32> = buffer.drain(..FRAME_SIZE).collect();
                        Some(samples)
                    } else {
                        None
                    }
                }
                Err(e) => {
                    error!("Audio buffer mutex poisoned: {}", e);
                    None
                }
            }
        };

        if let Some(samples) = samples_to_process {
            total_frames += 1;
            if total_frames == 1 {
                info!("[AUDIO] First frame received ({} samples)", samples.len());
            } else if total_frames.is_multiple_of(500) {
                info!(
                    "[AUDIO] Frames: {} total, {} voice, {} silent",
                    total_frames, vad_passed, vad_blocked
                );
            }

            // Apply VAD filtering if enabled
            let has_voice = if let Some(ref mut v) = vad {
                match v.process_chunk(&samples) {
                    Ok(result) => result,
                    Err(e) => {
                        debug!("[VAD] Error: {}", e);
                        true // Default to allowing through on error
                    }
                }
            } else {
                true // No VAD, always process
            };

            if has_voice {
                vad_passed += 1;
            } else {
                vad_blocked += 1;
            }

            // Forward audio to STT buffer when in Listening state
            stt_capture.push_samples(&samples);

            // Maintain rolling buffer for two-stage wake word confirmation
            confirm_buffer.extend_from_slice(&samples);
            if confirm_buffer.len() > CONFIRM_BUFFER_SIZE {
                let excess = confirm_buffer.len() - CONFIRM_BUFFER_SIZE;
                confirm_buffer.drain(..excess);
            }

            // Wake word detection — ONNX audio-based only (CLAUDE.md requirement).
            // STT-based transcript fallback is prohibited and never runs.
            // During TTS/processing (suppress_wakeword=true), feed silence to keep the ONNX
            // pipeline primed without false-triggering on playback audio.
            if suppress_wakeword.load(Ordering::SeqCst) {
                // Suppressed — feed silence to keep ONNX model state valid
                if let Some(ref mut det) = detector {
                    let _ = det.process_audio(&vec![0.0f32; samples.len()]);
                }
            } else {
                // ONNX-based wake word detection — always the primary (and only) mechanism
                if let Some(ref mut det) = detector {
                    match det.process_audio(&samples) {
                        Ok(Some(score)) => {
                            info!("[WAKE] Wake word detected with score: {:.4}", score);
                            let recent_audio = confirm_buffer.clone();
                            on_wake_word(score, recent_audio);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!("[WAKE] Detection error: {}", e);
                        }
                    }
                }
            }
        }

        // Sleep briefly to avoid busy loop
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
